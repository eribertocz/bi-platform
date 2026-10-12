//! The validation pipeline (§5), one query block at a time. Each step either
//! proves facts and records obligations, or rejects with a structured error;
//! nothing is guessed and nothing is silently repaired.

use std::collections::{BTreeMap, BTreeSet};

use sef_model_ir::diagnostic::{Diagnostic, ErrorCode as Code};
use sef_model_ir::expressions::{
    AggregateFunction, AggregateNullSemantics, MetricExprKind, ScalarExpr, ScalarExprKind,
};
use sef_model_ir::grain::{Grain, GrainAtom};
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{
    AggregationClass, AxisBehavior, CompiledSemanticModel, Dimension, Measure, Metric,
    ModelDefinition, TemporalSelection, WindowBehavior,
};
use sef_model_ir::policy::{Enforcement, PolicyScope};
use sef_model_ir::time::{CalendarProfile, IanaZoneId, TimeBucket, TimeGrain, TimestampSemantics};
use sef_model_ir::types::{DecimalType, DecimalValue, Literal, ValueType};
use sef_query_ir::query::{
    FilterExpr, FilterTarget, QualifiedDimensionRef, RequestedAtom, SelectionItem,
    SemanticQueryBlock, SemanticQuerySet, TimeDimensionSelection,
};

use crate::error::{canonical, rejection};
use crate::graph::{self, Path, Traversal};
use crate::outcome::{BlockValidation, Fact, Obligation, ValidatedQuery, atom_text};

/// Runtime values that affect semantic validity: the bindings of policy
/// context slots (§3.1). No statistics, cache or backend state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticContext {
    pub slots: BTreeMap<String, Literal>,
}

/// Proves a bound query meaningful against a compiled model, or rejects it.
///
/// # Errors
/// Every rejection found, as stable diagnostics (phase `Correctness`) in
/// canonical order. A rejected query produces no facts and no obligations.
pub fn validate(
    model: &CompiledSemanticModel,
    query: &SemanticQuerySet,
    context: &SemanticContext,
) -> Result<ValidatedQuery, Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let mut blocks = Vec::with_capacity(query.blocks.len());
    for (b, block) in query.blocks.iter().enumerate() {
        match Analysis::new(model.definition(), context, format!("/blocks/{b}")).block(block) {
            Ok(validated) => blocks.push(validated),
            Err(found) => errors.extend(found),
        }
    }
    if errors.is_empty() {
        Ok(ValidatedQuery {
            query: query.clone(),
            blocks,
        })
    } else {
        Err(canonical(errors))
    }
}

/// A dimension the request needs, where it comes from, the time bucket
/// applied to it (if any), and the facts it must be reached from (`None`:
/// every fact of the block).
#[derive(Clone)]
struct Usage<'q> {
    dimension: QualifiedDimensionRef,
    path: String,
    bucket: Option<&'q TimeDimensionSelection>,
    anchors: Option<BTreeSet<StableId>>,
}

/// The values a time dimension holds: the grain they are bucketed at, the
/// zone they were bucketed in (none for calendar dates) and the calendar.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Domain {
    grain: TimeGrain,
    timezone: Option<IanaZoneId>,
    calendar: CalendarProfile,
}

struct Analysis<'m> {
    model: &'m ModelDefinition,
    context: &'m SemanticContext,
    path: String,
    facts: BTreeSet<Fact>,
    obligations: BTreeSet<Obligation>,
    errors: Vec<Diagnostic>,
    /// Resolved paths by (anchor dataset, dimension); `None` once rejected.
    paths: BTreeMap<(StableId, QualifiedDimensionRef), Option<Path<'m>>>,
    /// Datasets whose rows the request reads, for policy attachment.
    datasets: BTreeSet<StableId>,
}

impl<'m> Analysis<'m> {
    fn new(model: &'m ModelDefinition, context: &'m SemanticContext, path: String) -> Self {
        Self {
            model,
            context,
            path,
            facts: BTreeSet::new(),
            obligations: BTreeSet::new(),
            errors: Vec::new(),
            paths: BTreeMap::new(),
            datasets: BTreeSet::new(),
        }
    }

    fn reject(&mut self, error: Diagnostic) {
        self.errors.push(error);
    }

    fn block(mut self, block: &SemanticQueryBlock) -> Result<BlockValidation, Vec<Diagnostic>> {
        let requested = block.requested_grain();
        self.facts.insert(Fact::RequestedGrain(requested.clone()));
        let selected = self.metrics(block);
        // Every metric the selections depend on is analysed with its own
        // contracts, under the selection that brought it in (§13, §15.2).
        let closure = self.closure(&selected);
        let mut usages = self.usages(block);
        usages.extend(self.intrinsic(&closure));
        let mut anchors = BTreeSet::new();
        for (metric, _) in &selected {
            let allocations = self.allocations(metric);
            for measure in self.measures(metric) {
                anchors.insert(measure.dataset.clone());
                self.datasets.insert(measure.dataset.clone());
                for usage in &usages {
                    let applies = usage
                        .anchors
                        .as_ref()
                        .is_none_or(|anchors| anchors.contains(&measure.dataset));
                    if applies {
                        self.reach(metric, &allocations, &measure.dataset, usage);
                    }
                }
                self.null_semantics(measure);
            }
        }
        for (metric, path) in &closure {
            self.facts.insert(Fact::MetricClass {
                metric: metric.metadata.id().clone(),
                class: class_name(metric.aggregation_contract.classification),
            });
            self.facts.insert(Fact::MetricNativeGrain {
                metric: metric.metadata.id().clone(),
                grain: metric.native_grain.clone(),
            });
            self.algebra(metric, path, &requested);
            self.temporal(metric, path, &requested);
        }
        if anchors.len() > 1 {
            self.align(&selected, &requested);
        }
        for usage in &usages {
            self.time(usage, &anchors);
        }
        self.policies(&closure);
        if self.errors.is_empty() {
            Ok(BlockValidation {
                block_id: block.block_id.clone(),
                facts: self.facts,
                obligations: self.obligations,
            })
        } else {
            Err(self.errors)
        }
    }

    // ---- 2. Query: what the block asks for ----------------------------------------

    /// Every dimension the block groups or filters by, first use first.
    fn usages<'q>(&self, block: &'q SemanticQueryBlock) -> Vec<Usage<'q>> {
        let mut usages: Vec<Usage<'q>> = Vec::new();
        for (s, selection) in block.selections.iter().enumerate() {
            let path = format!("{}/selections/{s}", self.path);
            match &selection.item {
                SelectionItem::Metric(_) => {}
                SelectionItem::Dimension(dimension) => usages.push(Usage {
                    dimension: dimension.clone(),
                    path,
                    bucket: None,
                    anchors: None,
                }),
                SelectionItem::TimeDimension(time) => usages.push(Usage {
                    dimension: time.dimension.clone(),
                    path,
                    bucket: Some(time),
                    anchors: None,
                }),
            }
        }
        if let Some(filter) = &block.filter {
            let mut targets = Vec::new();
            filter_dimensions(filter, &mut targets);
            for dimension in targets {
                if !usages.iter().any(|usage| &usage.dimension == dimension) {
                    usages.push(Usage {
                        dimension: dimension.clone(),
                        path: format!("{}/filter", self.path),
                        bucket: None,
                        anchors: None,
                    });
                }
            }
        }
        usages
    }

    /// Dimensions read by the intrinsic filters of every metric and measure
    /// in the lineage, reached only from that metric's own facts; fields
    /// they read mark their datasets as read (policy lineage, §20).
    fn intrinsic<'q>(&mut self, closure: &[(&'m Metric, String)]) -> Vec<Usage<'q>> {
        let mut found = Vec::new();
        for (metric, path) in closure {
            let measures = self.measures(metric);
            let anchors: BTreeSet<StableId> = measures
                .iter()
                .map(|measure| measure.dataset.clone())
                .collect();
            let mut filters: Vec<&ScalarExpr> = metric.intrinsic_filter.iter().collect();
            filters.extend(measures.iter().filter_map(|m| m.intrinsic_filter.as_ref()));
            for filter in filters {
                for field in filter.field_refs() {
                    if let Some(dataset) = self.dataset_of_field(&field) {
                        self.datasets.insert(dataset);
                    }
                }
                for dimension in filter.dimension_refs() {
                    found.push(Usage {
                        dimension: QualifiedDimensionRef {
                            dimension,
                            roles: Vec::new(),
                        },
                        path: format!("{path}/intrinsic_filter"),
                        bucket: None,
                        anchors: Some(anchors.clone()),
                    });
                }
            }
        }
        found
    }

    fn dataset_of_field(&self, field: &StableId) -> Option<StableId> {
        self.model
            .datasets
            .iter()
            .find(|dataset| dataset.fields.iter().any(|f| f.metadata.id() == field))
            .map(|dataset| dataset.metadata.id().clone())
    }

    fn metrics(&mut self, block: &SemanticQueryBlock) -> Vec<(&'m Metric, String)> {
        let mut found = Vec::new();
        for (s, selection) in block.selections.iter().enumerate() {
            let SelectionItem::Metric(id) = &selection.item else {
                continue;
            };
            let path = format!("{}/selections/{s}", self.path);
            match self.metric(id) {
                Some(metric) => found.push((metric, path)),
                None => self.reject(unknown(id, path)),
            }
        }
        found
    }

    fn metric(&self, id: &StableId) -> Option<&'m Metric> {
        self.model
            .metrics
            .iter()
            .find(|metric| metric.metadata.id() == id)
    }

    /// The metrics a metric is built from, itself included: references,
    /// window bases and recomposition components.
    fn dependencies(&self, metric: &'m Metric) -> Vec<&'m Metric> {
        let mut found: BTreeMap<StableId, &'m Metric> = BTreeMap::new();
        let mut pending = vec![metric];
        while let Some(current) = pending.pop() {
            if found
                .insert(current.metadata.id().clone(), current)
                .is_some()
            {
                continue;
            }
            let mut next = current.expression.metric_refs();
            if let Some(decomposition) = &current.aggregation_contract.decomposition {
                next.extend(decomposition.components.iter().cloned());
            }
            if let Some(WindowBehavior::Cumulative { base_metric, .. }) = &current.window_behavior {
                next.push(base_metric.clone());
            }
            pending.extend(next.iter().filter_map(|id| self.metric(id)));
        }
        found.into_values().collect()
    }

    /// Every metric of the block's lineage, each under the first selection
    /// that needs it.
    fn closure(&self, selected: &[(&'m Metric, String)]) -> Vec<(&'m Metric, String)> {
        let mut found: BTreeMap<StableId, (&'m Metric, String)> = BTreeMap::new();
        for (metric, path) in selected {
            for dependency in self.dependencies(metric) {
                found
                    .entry(dependency.metadata.id().clone())
                    .or_insert_with(|| (dependency, path.clone()));
            }
        }
        found.into_values().collect()
    }

    /// The primitive measures a metric is computed from.
    fn measures(&self, metric: &'m Metric) -> Vec<&'m Measure> {
        let mut measures: BTreeMap<StableId, &'m Measure> = BTreeMap::new();
        for dependency in self.dependencies(metric) {
            for id in dependency.expression.measure_refs() {
                if let Some(measure) = self.model.measures.iter().find(|m| m.metadata.id() == &id) {
                    measures.insert(id, measure);
                }
            }
        }
        measures.into_values().collect()
    }

    /// The bridges a metric's lineage declares, with the metric declaring
    /// each: an allocation stated on a dependency covers its users.
    fn allocations(&self, metric: &'m Metric) -> Vec<(StableId, StableId)> {
        self.dependencies(metric)
            .into_iter()
            .filter_map(|dependency| {
                dependency
                    .allocation_bridge
                    .as_ref()
                    .map(|bridge| (dependency.metadata.id().clone(), bridge.clone()))
            })
            .collect()
    }

    // ---- 5–6. Relationships, paths and fanout --------------------------------------

    /// Proves that `usage` is reachable from `anchor` by exactly one path
    /// and that the path cannot multiply the metric's observations, or that
    /// a declared allocation resolves it.
    fn reach(
        &mut self,
        metric: &'m Metric,
        allocations: &[(StableId, StableId)],
        anchor: &StableId,
        usage: &Usage<'_>,
    ) {
        let key = (anchor.clone(), usage.dimension.clone());
        if !self.paths.contains_key(&key) {
            let resolved = self.resolve(anchor, usage);
            self.paths.insert(key.clone(), resolved);
        }
        let Some(Some(path)) = self.paths.get(&key).cloned() else {
            return;
        };
        if path.steps.iter().any(|step| !step.traversal.is_single()) {
            self.allocate(metric, allocations, anchor, &path, usage);
        }
    }

    fn resolve(&mut self, anchor: &StableId, usage: &Usage<'_>) -> Option<Path<'m>> {
        let Some(dimension) = self.dimension(&usage.dimension.dimension) else {
            self.reject(unknown(&usage.dimension.dimension, usage.path.clone()));
            return None;
        };
        let targets: BTreeSet<&StableId> = dimension
            .bindings
            .iter()
            .map(|binding| &binding.dataset)
            .collect();
        let Ok(all) = graph::paths(self.model, anchor, &targets) else {
            // A truncated search proves neither uniqueness nor absence.
            self.reject(rejection(
                Code::UnsupportedSemanticConstruct,
                usage.path.clone(),
                [anchor.clone(), usage.dimension.dimension.clone()],
                format!(
                    "El grafo de relaciones entre {anchor} y {} supera el presupuesto de \
                     búsqueda: no se puede demostrar un camino único.",
                    usage.dimension.dimension
                ),
            ));
            return None;
        };
        let roles: BTreeSet<&str> = usage.dimension.roles.iter().map(String::as_str).collect();
        let candidates: Vec<Path<'m>> = if roles.is_empty() {
            match all.iter().find(|path| path.steps.is_empty()) {
                Some(local) => vec![local.clone()],
                None => all.clone(),
            }
        } else {
            all.iter()
                .filter(|path| roles.is_subset(&path.roles()))
                .cloned()
                .collect()
        };
        match candidates.as_slice() {
            [path] => {
                let path = path.clone();
                self.prove(anchor, usage, &path);
                Some(path)
            }
            [] => {
                self.unreachable(anchor, usage, &all);
                None
            }
            many => {
                let mut error = rejection(
                    Code::AmbiguousRelationshipPath,
                    usage.path.clone(),
                    [anchor.clone(), usage.dimension.dimension.clone()],
                    format!(
                        "{} se alcanza desde {anchor} por {} caminos con significados distintos; \
                         indica cuál con un rol.",
                        usage.dimension.dimension,
                        many.len()
                    ),
                );
                // Machine-stable suggestions: the role set of each candidate.
                error.suggestion_codes = many
                    .iter()
                    .map(|path| {
                        let roles: Vec<&str> = path.roles().into_iter().collect();
                        format!("qualify-role:{}", roles.join("+"))
                    })
                    .collect();
                self.reject(error);
                None
            }
        }
    }

    fn unreachable(&mut self, anchor: &StableId, usage: &Usage<'_>, all: &[Path<'m>]) {
        let dimension = &usage.dimension.dimension;
        if !usage.dimension.roles.is_empty() && !all.is_empty() {
            self.reject(rejection(
                Code::InvalidRelationshipRole,
                usage.path.clone(),
                [anchor.clone(), dimension.clone()],
                format!(
                    "Ningún camino desde {anchor} hasta {dimension} usa los roles {}.",
                    usage.dimension.roles.join(" + ")
                ),
            ));
            return;
        }
        // A dataset observed at dimensions (already aggregated) cannot be
        // split into a finer grain; one observed at entity keys simply has
        // no relationship to the dimension.
        let aggregated = graph::dataset_of(self.model, anchor).is_some_and(|dataset| {
            !dataset
                .observation_grain
                .atoms()
                .any(|atom| matches!(atom, GrainAtom::EntityKeyRef(_)))
        });
        let (code, message) = if aggregated {
            (
                Code::GrainRefinementUnsupported,
                format!(
                    "{anchor} ya está agregado a un grano que no incluye {dimension}: \
                     no se puede descomponer sin una regla de reparto."
                ),
            )
        } else {
            (
                Code::InvalidGrainTransition,
                format!("Ninguna relación lleva de {anchor} a {dimension}."),
            )
        };
        self.reject(rejection(
            code,
            usage.path.clone(),
            [anchor.clone(), dimension.clone()],
            message,
        ));
    }

    /// Records the facts and obligations of a unique path.
    fn prove(&mut self, anchor: &StableId, usage: &Usage<'_>, path: &Path<'m>) {
        for step in &path.steps {
            let relationship = step.relationship;
            let id = relationship.metadata.id().clone();
            self.datasets.insert(relationship.from_dataset.clone());
            self.datasets.insert(relationship.to_dataset.clone());
            if step.traversal == Traversal::RequiresTemporalSingleMatch {
                self.obligations
                    .insert(Obligation::UseTemporalRelationship {
                        relationship: id.clone(),
                    });
            }
            if step.traversal.is_single() {
                self.facts.insert(Fact::CardinalityProven {
                    relationship: id.clone(),
                    class: step.traversal.as_str(),
                });
            }
            if usage.dimension.roles.contains(&relationship.role) {
                self.obligations
                    .insert(Obligation::UseDeclaredRelationshipRole {
                        relationship: id,
                        role: relationship.role.clone(),
                    });
            }
        }
        self.facts.insert(Fact::RelationshipPathUnique {
            anchor: anchor.clone(),
            dimension: usage.dimension.clone(),
            relationships: path.relationships(),
        });
    }

    /// A path that may multiply observations is legal only through a bridge
    /// whose allocation the metric's lineage declares (§13.2).
    fn allocate(
        &mut self,
        metric: &'m Metric,
        allocations: &[(StableId, StableId)],
        anchor: &StableId,
        path: &Path<'m>,
        usage: &Usage<'_>,
    ) {
        let used: BTreeSet<StableId> = path.relationships().into_iter().collect();
        let declared = allocations.iter().find_map(|(declaring, id)| {
            self.model
                .bridges
                .iter()
                .find(|bridge| {
                    bridge.metadata.id() == id
                        && &bridge.left_dataset == anchor
                        && bridge.allocation.is_some()
                        && used.iter().all(|relationship| {
                            relationship == &bridge.left_relationship
                                || relationship == &bridge.right_relationship
                        })
                })
                .map(|bridge| (declaring.clone(), bridge.metadata.id().clone()))
        });
        if let Some((declaring, bridge)) = declared {
            self.obligations.insert(Obligation::ApplyBridgeAllocation {
                metric: declaring,
                bridge: bridge.clone(),
            });
            self.obligations
                .insert(Obligation::ValidateAllocationNormalization { bridge });
            return;
        }
        let mut refs = vec![metric.metadata.id().clone()];
        refs.extend(path.relationships());
        self.reject(rejection(
            Code::UnresolvedFanout,
            usage.path.clone(),
            refs,
            format!(
                "Agrupar {} por {} multiplica sus filas a través de {}: el resultado contaría \
                 de más. Declara un puente con reparto o agrupa por otra dimensión.",
                metric.metadata.id(),
                usage.dimension.dimension,
                path.relationships()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    /// Independent facts align only after each is aggregated at its own
    /// grain (§13.1, §14).
    fn align(&mut self, metrics: &[(&'m Metric, String)], requested: &[RequestedAtom]) {
        for (metric, _) in metrics {
            for measure in self.measures(metric) {
                self.obligations.insert(Obligation::PreserveMetricGrain {
                    measure: measure.metadata.id().clone(),
                    grain: measure.input_grain.clone(),
                });
            }
        }
        self.obligations
            .insert(Obligation::AggregateBeforeAlignment {
                grain: requested.to_vec(),
            });
    }

    fn dimension(&self, id: &StableId) -> Option<&'m Dimension> {
        self.model
            .dimensions
            .iter()
            .find(|dimension| dimension.metadata.id() == id)
    }

    // ---- 7. Metric algebra ---------------------------------------------------------

    fn algebra(&mut self, metric: &'m Metric, path: &str, requested: &[RequestedAtom]) {
        let id = metric.metadata.id().clone();
        let composite = matches!(
            metric.expression.kind,
            MetricExprKind::Arithmetic { .. }
                | MetricExprKind::Ratio { .. }
                | MetricExprKind::ExplicitCast { .. }
        );
        if let Some(decomposition) = &metric.aggregation_contract.decomposition {
            self.obligations.insert(Obligation::RecomputeAtTargetGrain {
                metric: id.clone(),
                components: sorted(decomposition.components.clone()),
            });
        } else if composite {
            self.obligations.insert(Obligation::RecomputeAtTargetGrain {
                metric: id.clone(),
                components: sorted(metric.dependencies.clone()),
            });
        }
        if metric.aggregation_contract.classification == AggregationClass::NonAdditive {
            self.obligations
                .insert(Obligation::DoNotReaggregate { metric: id.clone() });
        }
        if matches!(metric.value_type, ValueType::Decimal(_)) {
            self.obligations
                .insert(Obligation::PreserveExactDecimalSemantics { metric: id.clone() });
        }
        for rule in &metric.aggregation_contract.axis_rules {
            if rule.behavior == AxisBehavior::Forbidden && !grouped_exactly(&rule.axis, requested) {
                let temporal = self.is_temporal_axis(&rule.axis);
                self.reject(rejection(
                    if temporal {
                        Code::IllegalTemporalReaggregation
                    } else {
                        Code::IllegalReaggregation
                    },
                    path,
                    [id.clone()],
                    format!(
                        "{id} no se puede agregar a lo largo de {}: agrupa por {} para obtener \
                         un valor por cada uno.",
                        atom_text(&rule.axis),
                        atom_text(&rule.axis)
                    ),
                ));
            }
        }
        if let Some(WindowBehavior::Cumulative {
            base_metric,
            prerequisite_grain,
            order_by,
            partition_by,
            ..
        }) = &metric.window_behavior
        {
            if self.as_grain(requested).as_ref() == Some(prerequisite_grain) {
                self.obligations.insert(Obligation::ApplyOrderedCumulative {
                    metric: id,
                    base: base_metric.clone(),
                    order_by: order_by.iter().map(|order| order.value.clone()).collect(),
                    partition_by: partition_by.clone(),
                });
            } else {
                self.reject(rejection(
                    Code::InvalidWindowSemantics,
                    path,
                    [id.clone(), base_metric.clone()],
                    format!(
                        "{id} es un acumulado que solo tiene sentido agrupado exactamente por {}.",
                        prerequisite_grain
                            .atoms()
                            .map(atom_text)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
    }

    /// An axis is temporal when its dimension holds dates or instants, or
    /// is bound to a field with a snapshot or event time role.
    fn is_temporal_axis(&self, axis: &GrainAtom) -> bool {
        match axis {
            GrainAtom::TimeBucketRef(_) => true,
            GrainAtom::EntityKeyRef(_) => false,
            GrainAtom::DimensionRef { dimension, .. } => {
                self.dimension(dimension).is_some_and(|dimension| {
                    matches!(
                        dimension.value_type,
                        ValueType::Date | ValueType::Timestamp { .. }
                    ) || dimension.time_semantics.is_some()
                })
            }
        }
    }

    /// The requested grain as CSMIR atoms. A time bucket at exactly its
    /// dimension's own grain, zone and calendar groups the same values as
    /// the plain dimension (§8.1), so it is that dimension; any other
    /// bucket has no plain-atom equivalent.
    fn as_grain(&self, requested: &[RequestedAtom]) -> Option<Grain> {
        requested
            .iter()
            .map(|atom| match atom {
                RequestedAtom::Dimension(dimension) => Some(GrainAtom::DimensionRef {
                    dimension: dimension.dimension.clone(),
                    roles: dimension.roles.clone(),
                }),
                RequestedAtom::TimeBucket(time) => {
                    let dimension = self.dimension(&time.dimension.dimension)?;
                    let domain = self.domain(dimension)?;
                    let same = domain.grain == time.grain
                        && time
                            .timezone
                            .as_ref()
                            .is_none_or(|zone| domain.timezone.as_ref() == Some(zone))
                        && self
                            .requested_calendar(time)
                            .is_none_or(|calendar| calendar == domain.calendar);
                    same.then(|| GrainAtom::DimensionRef {
                        dimension: time.dimension.dimension.clone(),
                        roles: time.dimension.roles.clone(),
                    })
                }
            })
            .collect::<Option<Vec<_>>>()
            .map(Grain::new)
    }

    fn null_semantics(&mut self, measure: &'m Measure) {
        let nullable_input = measure
            .scalar_expression
            .as_ref()
            .is_some_and(|expression| {
                expression.field_refs().iter().any(|field| {
                    self.model
                        .datasets
                        .iter()
                        .flat_map(|dataset| &dataset.fields)
                        .any(|candidate| candidate.metadata.id() == field && candidate.nullable)
                })
            });
        if nullable_input || measure.aggregation == AggregateFunction::CountRows {
            self.obligations
                .insert(Obligation::PreserveDeclaredNullSemantics {
                    measure: measure.metadata.id().clone(),
                    semantics: match measure.null_semantics {
                        AggregateNullSemantics::IgnoreNullsNullIfEmpty => {
                            "ignore-nulls-null-if-empty"
                        }
                        AggregateNullSemantics::CountNonNull => "count-non-null",
                        AggregateNullSemantics::CountRows => "count-rows",
                    },
                });
        }
    }

    // ---- 8. Time -------------------------------------------------------------------

    /// Semi-additive selection (§17.2–17.3): keep one snapshot per period
    /// unless the request groups by each snapshot; with no declared rule a
    /// collapse of the time axis is rejected.
    fn temporal(&mut self, metric: &'m Metric, path: &str, requested: &[RequestedAtom]) {
        let id = metric.metadata.id().clone();
        if let Some(behavior) = &metric.time_behavior {
            if !grouped_exactly(&behavior.axis, requested) {
                let axis = match &behavior.axis {
                    GrainAtom::DimensionRef { dimension, .. } => dimension.clone(),
                    GrainAtom::EntityKeyRef(key) => key.clone(),
                    GrainAtom::TimeBucketRef(bucket) => bucket.source.clone(),
                };
                self.obligations
                    .insert(Obligation::ApplyPeriodEndSelection {
                        metric: id,
                        axis,
                        rule: match behavior.rule {
                            TemporalSelection::PeriodEnd => "period-end",
                            TemporalSelection::PeriodStart => "period-start",
                        },
                        period: bucket_of(&behavior.axis, requested),
                        scope: behavior.selection_scope.clone(),
                    });
            }
            return;
        }
        for rule in &metric.aggregation_contract.axis_rules {
            let selects = matches!(
                rule.behavior,
                AxisBehavior::PeriodEnd | AxisBehavior::PeriodStart
            );
            if selects && !grouped_exactly(&rule.axis, requested) {
                self.reject(rejection(
                    Code::IllegalTemporalReaggregation,
                    path,
                    [id.clone()],
                    format!(
                        "{id} pide un valor por periodo a lo largo de {}, pero no declara qué \
                         instantánea conservar: agrupa por {} o declara la regla temporal.",
                        atom_text(&rule.axis),
                        atom_text(&rule.axis)
                    ),
                ));
            }
        }
    }

    /// The domain of a time dimension: the coarsest bucket any of its
    /// (conformed) bindings derives or any dataset observes it at; a day for
    /// plain dates; none for instants, which take a zone when bucketed.
    fn domain(&self, dimension: &'m Dimension) -> Option<Domain> {
        let id = dimension.metadata.id();
        let mut buckets: Vec<&TimeBucket> = dimension
            .bindings
            .iter()
            .filter_map(|binding| derivation(&binding.expression))
            .collect();
        for dataset in &self.model.datasets {
            for atom in dataset.observation_grain.atoms() {
                if let GrainAtom::TimeBucketRef(bucket) = atom
                    && &bucket.source == id
                {
                    buckets.push(bucket);
                }
            }
        }
        if let Some(coarsest) = buckets.iter().max_by_key(|bucket| bucket.grain) {
            return Some(Domain {
                grain: coarsest.grain,
                timezone: Some(coarsest.timezone.clone()),
                calendar: coarsest.calendar.clone(),
            });
        }
        let calendar = dimension
            .time_semantics
            .as_ref()
            .map_or(CalendarProfile::Gregorian, |semantics| {
                semantics.calendar.clone()
            });
        matches!(dimension.value_type, ValueType::Date).then_some(Domain {
            grain: TimeGrain::Day,
            timezone: None,
            calendar,
        })
    }

    /// The calendar profile a time selection asks for, if it names one.
    fn requested_calendar(&self, time: &TimeDimensionSelection) -> Option<CalendarProfile> {
        let id = time.calendar.as_ref()?;
        self.model
            .calendars
            .iter()
            .find(|calendar| calendar.metadata.id() == id)
            .map(|calendar| calendar.profile.clone())
    }

    /// Time bucketing (§17.1, §8.1): an instant is converted to its business
    /// zone before any calendar bucket; values already bucketed keep their
    /// zone and calendar and only roll up to coarser compatible grains.
    fn time(&mut self, usage: &Usage<'_>, anchors: &BTreeSet<StableId>) {
        let Some(dimension) = self.dimension(&usage.dimension.dimension) else {
            return;
        };
        for anchor in anchors {
            let Some(Some(path)) = self
                .paths
                .get(&(anchor.clone(), usage.dimension.clone()))
                .cloned()
            else {
                continue;
            };
            let dataset = arrival(anchor, &path);
            let Some(binding) = dimension
                .bindings
                .iter()
                .find(|binding| binding.dataset == dataset)
            else {
                continue;
            };
            let derived = derivation(&binding.expression).cloned();
            match usage.bucket {
                Some(time) => self.bucket(usage, time, dimension, &dataset, derived.as_ref()),
                None => {
                    // A plain derived dimension still converts before its
                    // own bucket.
                    if let Some(bucket) = derived {
                        self.obligations
                            .insert(Obligation::ConvertTimezoneBeforeBucketing {
                                dimension: dimension.metadata.id().clone(),
                                dataset,
                                timezone: bucket.timezone,
                                calendar: bucket.calendar,
                                grain: bucket.grain,
                            });
                    }
                }
            }
        }
    }

    fn bucket(
        &mut self,
        usage: &Usage<'_>,
        time: &TimeDimensionSelection,
        dimension: &'m Dimension,
        dataset: &StableId,
        derived: Option<&TimeBucket>,
    ) {
        let id = dimension.metadata.id().clone();
        let mut requested_calendar = None;
        if let Some(calendar) = &time.calendar {
            let Some(profile) = self.requested_calendar(time) else {
                self.reject(unknown(calendar, format!("{}/calendar", usage.path)));
                return;
            };
            requested_calendar = Some(profile);
        }
        let domain = self.domain(dimension);
        if let Some(domain) = &domain
            && !self.within_domain(usage, time, &id, domain, requested_calendar.as_ref())
        {
            return;
        }
        let calendar = requested_calendar
            .or_else(|| domain.as_ref().map(|domain| domain.calendar.clone()))
            .unwrap_or(CalendarProfile::Gregorian);
        let Ok(zone) = self.bucket_zone(usage, time, dimension, derived) else {
            return;
        };
        self.obligations.insert(match zone {
            Some(timezone) => Obligation::ConvertTimezoneBeforeBucketing {
                dimension: id,
                dataset: dataset.clone(),
                timezone,
                calendar,
                grain: time.grain,
            },
            None => Obligation::ApplyCalendarBucket {
                dimension: id,
                dataset: dataset.clone(),
                calendar,
                grain: time.grain,
            },
        });
    }

    /// A bucket over already-bucketed values keeps their zone and calendar
    /// and only rolls up (§8.1): the domain proves which transitions exist.
    fn within_domain(
        &mut self,
        usage: &Usage<'_>,
        time: &TimeDimensionSelection,
        id: &StableId,
        domain: &Domain,
        requested_calendar: Option<&CalendarProfile>,
    ) -> bool {
        let (code, path, message) = match (&time.timezone, &domain.timezone) {
            (Some(zone), Some(fixed)) if zone != fixed => (
                Code::InvalidGrainTransition,
                format!("{}/timezone", usage.path),
                format!(
                    "{id} ya se agrupó en la zona {}: no se puede volver a agrupar en otra.",
                    fixed.as_str()
                ),
            ),
            (Some(_), None) => (
                Code::InvalidGrainTransition,
                format!("{}/timezone", usage.path),
                format!("{id} es una fecha de calendario: no tiene zona horaria."),
            ),
            _ if requested_calendar.is_some_and(|calendar| calendar != &domain.calendar) => (
                Code::InvalidGrainTransition,
                format!("{}/calendar", usage.path),
                format!(
                    "{id} ya está en el calendario {:?}: otro calendario partiría sus periodos.",
                    domain.calendar
                ),
            ),
            _ if !rolls_up(domain.grain, time.grain) => (
                Code::GrainRefinementUnsupported,
                usage.path.clone(),
                format!(
                    "Los valores de {id} están agrupados por {:?}: no se pueden repartir en {:?} sin una regla de reparto.",
                    domain.grain, time.grain
                ),
            ),
            _ => return true,
        };
        self.reject(rejection(code, path, [id.clone()], message));
        false
    }

    /// Where the instant becomes a calendar value: the binding's own
    /// derivation, or the zone of a bucket over a raw instant. `Ok(None)`
    /// for values that are already calendar values.
    fn bucket_zone(
        &mut self,
        usage: &Usage<'_>,
        time: &TimeDimensionSelection,
        dimension: &'m Dimension,
        derived: Option<&TimeBucket>,
    ) -> Result<Option<IanaZoneId>, ()> {
        let id = dimension.metadata.id();
        match (derived, &dimension.value_type) {
            (Some(bucket), _) => Ok(Some(bucket.timezone.clone())),
            (
                None,
                ValueType::Timestamp {
                    timezone_semantics: TimestampSemantics::Instant,
                },
            ) => {
                let zone = time.timezone.clone().or_else(|| {
                    dimension
                        .time_semantics
                        .as_ref()
                        .map(|semantics| semantics.timezone.clone())
                });
                if zone.is_none() {
                    self.reject(rejection(
                        Code::InvalidGrainTransition,
                        format!("{}/timezone", usage.path),
                        [id.clone()],
                        format!(
                            "{id} es un instante sin zona de negocio declarada: indica en qué zona agrupar por {:?}.",
                            time.grain
                        ),
                    ));
                    return Err(());
                }
                Ok(zone)
            }
            (None, ValueType::Timestamp { .. }) if time.timezone.is_some() => {
                self.reject(rejection(
                    Code::InvalidGrainTransition,
                    format!("{}/timezone", usage.path),
                    [id.clone()],
                    format!("{id} es una fecha y hora local: no admite otra zona."),
                ));
                Err(())
            }
            _ => Ok(None),
        }
    }

    // ---- 9. Policies ---------------------------------------------------------------

    /// Every mandatory policy on a dataset the request reads (sources, path
    /// datasets, intrinsic filters) or on a metric of its lineage must have
    /// its context bound and well typed (§20); the caller can only narrow
    /// what the policy allows, never remove it.
    fn policies(&mut self, closure: &[(&'m Metric, String)]) {
        let lineage: BTreeSet<&StableId> = closure
            .iter()
            .map(|(metric, _)| metric.metadata.id())
            .collect();
        let mut scoped: BTreeSet<(StableId, StableId)> = BTreeSet::new();
        for dataset in &self.datasets {
            if let Some(found) = graph::dataset_of(self.model, dataset) {
                for policy in &found.policy_refs {
                    scoped.insert((policy.clone(), dataset.clone()));
                }
            }
        }
        for (metric, _) in closure {
            for policy in &metric.policy_refs {
                scoped.insert((policy.clone(), metric.metadata.id().clone()));
            }
        }
        // A policy scoped to an object of the lineage applies even if the
        // object does not list it.
        for policy in &self.model.policies {
            let scope = match &policy.scope {
                PolicyScope::Dataset(dataset) if self.datasets.contains(dataset) => dataset,
                PolicyScope::Metric(metric) if lineage.contains(metric) => metric,
                _ => continue,
            };
            scoped.insert((policy.metadata.id().clone(), scope.clone()));
        }
        for (policy_id, scope) in scoped {
            self.attach(&policy_id, scope);
        }
    }

    fn attach(&mut self, policy_id: &StableId, scope: StableId) {
        let Some(policy) = self
            .model
            .policies
            .iter()
            .find(|policy| policy.metadata.id() == policy_id)
        else {
            return;
        };
        if policy.enforcement != Enforcement::Mandatory {
            return;
        }
        let mut bound = true;
        for slot in &policy.context_slots {
            let path = format!("/context/slots/{}", slot.name);
            match self.context.slots.get(&slot.name) {
                None if slot.required => {
                    bound = false;
                    self.reject(rejection(
                        Code::PolicyContextMissing,
                        path,
                        [policy_id.clone(), scope.clone()],
                        format!(
                            "La política obligatoria {policy_id} sobre {scope} necesita el valor \
                             de contexto {:?}.",
                            slot.name
                        ),
                    ));
                }
                // Every value supplied, required or not, must be exactly of
                // the declared type and range.
                Some(value) if !fits(value, &slot.value_type) => {
                    bound = false;
                    self.reject(rejection(
                        Code::IncompatibleTypes,
                        path,
                        [policy_id.clone()],
                        format!(
                            "El valor de contexto {:?} no cabe en el tipo {:?} que declara la \
                             política {policy_id}.",
                            slot.name, slot.value_type
                        ),
                    ));
                }
                _ => {}
            }
        }
        if bound {
            self.facts.insert(Fact::PolicyRequired {
                scope: scope.clone(),
                policy: policy_id.clone(),
            });
            self.obligations.insert(Obligation::ApplyMandatoryPolicy {
                policy: policy_id.clone(),
                scope,
            });
        }
    }
}

// ---- Helpers ---------------------------------------------------------------------

fn unknown(id: &StableId, path: String) -> Diagnostic {
    rejection(
        Code::UnsupportedSemanticConstruct,
        path,
        [id.clone()],
        format!(
            "{id} no existe en el modelo compilado: la consulta no se vinculó con este modelo."
        ),
    )
}

fn filter_dimensions<'q>(filter: &'q FilterExpr, found: &mut Vec<&'q QualifiedDimensionRef>) {
    match filter {
        FilterExpr::Compare { target, .. }
        | FilterExpr::Between { target, .. }
        | FilterExpr::In { target, .. }
        | FilterExpr::IsNull(target)
        | FilterExpr::IsNotNull(target) => {
            if let FilterTarget::Dimension(dimension) = target {
                found.push(dimension);
            }
        }
        FilterExpr::And(operands) | FilterExpr::Or(operands) => {
            for operand in operands {
                filter_dimensions(operand, found);
            }
        }
        FilterExpr::Not(operand) => filter_dimensions(operand, found),
    }
}

/// The request groups by exactly this axis (a coarser bucket of it would
/// aggregate across it).
fn grouped_exactly(axis: &GrainAtom, requested: &[RequestedAtom]) -> bool {
    match axis {
        GrainAtom::DimensionRef { dimension, roles } => requested.iter().any(|atom| {
            matches!(atom, RequestedAtom::Dimension(found)
                if &found.dimension == dimension && &found.roles == roles)
        }),
        GrainAtom::EntityKeyRef(_) | GrainAtom::TimeBucketRef(_) => false,
    }
}

/// The grain at which the request buckets this axis, if it does.
fn bucket_of(axis: &GrainAtom, requested: &[RequestedAtom]) -> Option<TimeGrain> {
    let GrainAtom::DimensionRef { dimension, roles } = axis else {
        return None;
    };
    requested.iter().find_map(|atom| match atom {
        RequestedAtom::TimeBucket(time)
            if &time.dimension.dimension == dimension && &time.dimension.roles == roles =>
        {
            Some(time.grain)
        }
        _ => None,
    })
}

/// The dataset a path arrives at, walked from its anchor.
fn arrival(anchor: &StableId, path: &Path<'_>) -> StableId {
    let mut at = anchor.clone();
    for step in &path.steps {
        at = if step.relationship.from_dataset == at {
            step.relationship.to_dataset.clone()
        } else {
            step.relationship.from_dataset.clone()
        };
    }
    at
}

/// The calendar bucket a dimension binding derives from an instant.
fn derivation(expression: &ScalarExpr) -> Option<&TimeBucket> {
    match &expression.kind {
        ScalarExprKind::TemporalDerivation { bucket, .. } => Some(bucket),
        ScalarExprKind::Unary { operand, .. }
        | ScalarExprKind::NullTest { operand, .. }
        | ScalarExprKind::ExplicitCast { operand, .. }
        | ScalarExprKind::UnitConversion { operand, .. } => derivation(operand),
        _ => None,
    }
}

/// Whether values bucketed at `from` can be grouped at `to` without
/// splitting a bucket: finer-than-week grains roll up to anything coarser;
/// weeks straddle months, so they only stay weeks.
fn rolls_up(from: TimeGrain, to: TimeGrain) -> bool {
    use TimeGrain::{Day, Month, Quarter, Week, Year};
    from == to
        || (from <= Day && to > from)
        || (from == Month && matches!(to, Quarter | Year))
        || (from == Quarter && to == Year)
        || (from == Week && to == Week)
}

/// A context value is usable only if it is exactly representable in the
/// declared type: integer range by width and sign, decimal precision and
/// scale without loss, and the same type otherwise.
fn fits(value: &Literal, declared: &ValueType) -> bool {
    match (value, declared) {
        (Literal::SignedInteger(value), ValueType::Integer { signed: true, bits }) => {
            let bits = bits.unwrap_or(64).min(128);
            bits == 128 || {
                let limit = 1_i128 << (bits - 1);
                (-limit..limit).contains(value)
            }
        }
        (
            Literal::UnsignedInteger(value),
            ValueType::Integer {
                signed: false,
                bits,
            },
        ) => {
            let bits = bits.unwrap_or(64).min(128);
            bits == 128 || value >> bits == 0
        }
        (Literal::Decimal(value), ValueType::Decimal(declared)) => decimal_fits(value, *declared),
        (Literal::Float(value), ValueType::Float { bits: 32 }) => {
            #[allow(clippy::cast_possible_truncation)] // the narrowing is the check
            let narrowed = value.value() as f32;
            narrowed.is_finite() && f64::from(narrowed).to_bits() == value.value().to_bits()
        }
        (Literal::Null(_), _) => false,
        (value, declared) => &value.value_type() == declared,
    }
}

/// `value` written at the declared scale keeps every significant digit and
/// fits the declared precision.
fn decimal_fits(value: &DecimalValue, declared: DecimalType) -> bool {
    let digits = value.coefficient().trim_start_matches('-');
    let scale = usize::from(value.decimal_type().scale());
    let significant = digits.trim_end_matches('0');
    let trailing = digits.len() - significant.len();
    let needed_scale = if significant.is_empty() {
        0
    } else {
        scale.saturating_sub(trailing)
    };
    let whole = if digits.len() > scale {
        digits[..digits.len() - scale].trim_start_matches('0').len()
    } else {
        0
    };
    let target_scale = usize::from(declared.scale());
    needed_scale <= target_scale
        && whole <= usize::from(declared.precision()).saturating_sub(target_scale)
}

fn class_name(class: AggregationClass) -> &'static str {
    match class {
        AggregationClass::FullyAdditive => "fully-additive",
        AggregationClass::SemiAdditive => "semi-additive",
        AggregationClass::NonAdditive => "non-additive",
        AggregationClass::Recomputable => "recomputable",
        AggregationClass::OrderedWindow => "ordered-window",
    }
}

fn sorted(mut ids: Vec<StableId>) -> Vec<StableId> {
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use sef_model_ir::types::{DecimalType, DecimalValue, Literal, ValueType};

    use super::fits;

    fn decimal(coefficient: &str, precision: u8, scale: u8) -> Literal {
        Literal::Decimal(
            DecimalValue::new(
                coefficient,
                DecimalType::new(precision, scale).expect("type"),
            )
            .expect("value"),
        )
    }

    fn declared(precision: u8, scale: u8) -> ValueType {
        ValueType::Decimal(DecimalType::new(precision, scale).expect("type"))
    }

    #[test]
    fn decimals_fit_only_without_loss() {
        // 12.50 written as DECIMAL(5,2) fits DECIMAL(4,1) as 12.5 …
        assert!(fits(&decimal("1250", 5, 2), &declared(4, 1)));
        // … but 12.55 would lose a digit, and 123.4 has too many integers.
        assert!(!fits(&decimal("1255", 5, 2), &declared(4, 1)));
        assert!(!fits(&decimal("1234", 5, 1), &declared(3, 1)));
        assert!(fits(&decimal("-0", 5, 2), &declared(1, 0)));
    }

    #[test]
    fn integers_fit_their_sign_and_width() {
        let int = |signed, bits| ValueType::Integer {
            signed,
            bits: Some(bits),
        };
        assert!(fits(&Literal::SignedInteger(127), &int(true, 8)));
        assert!(!fits(&Literal::SignedInteger(128), &int(true, 8)));
        assert!(fits(&Literal::UnsignedInteger(255), &int(false, 8)));
        assert!(!fits(&Literal::UnsignedInteger(256), &int(false, 8)));
        assert!(
            !fits(&Literal::SignedInteger(1), &int(false, 8)),
            "sign differs"
        );
        assert!(fits(&Literal::SignedInteger(i128::MAX), &int(true, 128)));
        assert!(!fits(&Literal::Utf8("7".to_owned()), &int(true, 64)));
    }
}
