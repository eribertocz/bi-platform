//! The validation pipeline (§5), one query block at a time. Each step either
//! proves facts and records obligations, or rejects with a structured error;
//! nothing is guessed and nothing is silently repaired.

use std::collections::{BTreeMap, BTreeSet};

use sef_model_ir::expressions::{
    AggregateFunction, AggregateNullSemantics, MetricExprKind, ScalarExpr, ScalarExprKind,
};
use sef_model_ir::grain::{Grain, GrainAtom};
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{
    AggregationClass, AxisBehavior, CompiledSemanticModel, Dimension, Measure, Metric,
    ModelDefinition, TemporalSelection, WindowBehavior,
};
use sef_model_ir::policy::Enforcement;
use sef_model_ir::time::{TimeBucket, TimeGrain, TimestampSemantics};
use sef_model_ir::types::{Literal, ValueType};
use sef_query_ir::query::{
    FilterExpr, FilterTarget, QualifiedDimensionRef, RequestedAtom, SelectionItem,
    SemanticQueryBlock, SemanticQuerySet, TimeDimensionSelection,
};

use crate::error::{SemanticError, SemanticErrorCode as Code};
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
/// Every [`SemanticError`] found, in canonical order. A rejected query
/// produces no facts and no obligations.
pub fn validate(
    model: &CompiledSemanticModel,
    query: &SemanticQuerySet,
    context: &SemanticContext,
) -> Result<ValidatedQuery, Vec<SemanticError>> {
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
        errors.sort();
        errors.dedup();
        Err(errors)
    }
}

/// A requested dimension, where in the query it was asked for, and the
/// time bucket applied to it, if any.
struct Usage<'q> {
    dimension: &'q QualifiedDimensionRef,
    path: String,
    bucket: Option<&'q TimeDimensionSelection>,
}

struct Analysis<'m> {
    model: &'m ModelDefinition,
    context: &'m SemanticContext,
    path: String,
    facts: BTreeSet<Fact>,
    obligations: BTreeSet<Obligation>,
    errors: Vec<SemanticError>,
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

    fn reject(&mut self, error: SemanticError) {
        self.errors.push(error);
    }

    fn block(mut self, block: &SemanticQueryBlock) -> Result<BlockValidation, Vec<SemanticError>> {
        let requested = block.requested_grain();
        self.facts.insert(Fact::RequestedGrain(requested.clone()));
        let usages = self.usages(block);
        let metrics = self.metrics(block);
        let mut anchors = BTreeSet::new();
        for (metric, path) in &metrics {
            let measures = self.lineage(metric);
            for measure in &measures {
                anchors.insert(measure.dataset.clone());
                self.datasets.insert(measure.dataset.clone());
                for usage in &usages {
                    self.reach(metric, &measure.dataset, usage);
                }
                self.null_semantics(measure);
            }
            self.algebra(metric, path, &requested);
            self.temporal(metric, path, &requested);
        }
        if anchors.len() > 1 {
            self.align(&metrics, &requested);
        }
        for usage in &usages {
            self.time(usage, &anchors);
        }
        self.policies(&metrics);
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
                    dimension,
                    path,
                    bucket: None,
                }),
                SelectionItem::TimeDimension(time) => usages.push(Usage {
                    dimension: &time.dimension,
                    path,
                    bucket: Some(time),
                }),
            }
        }
        if let Some(filter) = &block.filter {
            let mut targets = Vec::new();
            filter_dimensions(filter, &mut targets);
            for dimension in targets {
                if !usages.iter().any(|usage| usage.dimension == dimension) {
                    usages.push(Usage {
                        dimension,
                        path: format!("{}/filter", self.path),
                        bucket: None,
                    });
                }
            }
        }
        usages
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

    /// The primitive measures a metric is computed from, through metric
    /// references and window bases.
    fn lineage(&mut self, metric: &'m Metric) -> Vec<&'m Measure> {
        self.facts.insert(Fact::MetricClass {
            metric: metric.metadata.id().clone(),
            class: class_name(metric.aggregation_contract.classification),
        });
        self.facts.insert(Fact::MetricNativeGrain {
            metric: metric.metadata.id().clone(),
            grain: metric.native_grain.clone(),
        });
        let mut measures = BTreeMap::new();
        let mut pending = vec![metric];
        let mut seen = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current.metadata.id()) {
                continue;
            }
            for id in current.expression.measure_refs() {
                if let Some(measure) = self.model.measures.iter().find(|m| m.metadata.id() == &id) {
                    measures.insert(id, measure);
                }
            }
            for id in current.expression.metric_refs() {
                if let Some(next) = self.metric(&id) {
                    pending.push(next);
                }
            }
        }
        measures.into_values().collect()
    }

    // ---- 5–6. Relationships, paths and fanout --------------------------------------

    /// Proves that `usage` is reachable from `anchor` by exactly one path
    /// and that the path cannot multiply the metric's observations, or that
    /// a declared allocation resolves it.
    fn reach(&mut self, metric: &'m Metric, anchor: &StableId, usage: &Usage<'_>) {
        let key = (anchor.clone(), usage.dimension.clone());
        if !self.paths.contains_key(&key) {
            let resolved = self.resolve(anchor, usage);
            self.paths.insert(key.clone(), resolved);
        }
        let Some(Some(path)) = self.paths.get(&key).cloned() else {
            return;
        };
        let fans_out = path.steps.iter().any(|step| !step.traversal.is_single());
        if fans_out {
            self.allocate(metric, anchor, &path, usage);
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
        let all = graph::paths(self.model, anchor, &targets);
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
                let suggestions: Vec<String> = many
                    .iter()
                    .map(|path| {
                        let roles: Vec<&str> = path.roles().into_iter().collect();
                        format!("Califica la dimensión con el rol {}.", roles.join(" + "))
                    })
                    .collect();
                self.reject(
                    SemanticError::new(
                        Code::AmbiguousRelationshipPath,
                        usage.path.clone(),
                        [anchor.clone(), usage.dimension.dimension.clone()],
                        format!(
                            "{} se alcanza desde {anchor} por {} caminos con significados distintos; \
                             indica cuál con un rol.",
                            usage.dimension.dimension,
                            many.len()
                        ),
                    )
                    .suggest(suggestions),
                );
                None
            }
        }
    }

    fn unreachable(&mut self, anchor: &StableId, usage: &Usage<'_>, all: &[Path<'m>]) {
        let dimension = &usage.dimension.dimension;
        if !usage.dimension.roles.is_empty() && !all.is_empty() {
            self.reject(SemanticError::new(
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
        self.reject(SemanticError::new(
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

    /// A path that may multiply observations is legal only through the
    /// bridge whose allocation the metric declares (§13.2).
    fn allocate(
        &mut self,
        metric: &'m Metric,
        anchor: &StableId,
        path: &Path<'m>,
        usage: &Usage<'_>,
    ) {
        let used: BTreeSet<StableId> = path.relationships().into_iter().collect();
        let bridge = metric.allocation_bridge.as_ref().and_then(|id| {
            self.model.bridges.iter().find(|bridge| {
                bridge.metadata.id() == id
                    && &bridge.left_dataset == anchor
                    && used.iter().all(|relationship| {
                        relationship == &bridge.left_relationship
                            || relationship == &bridge.right_relationship
                    })
            })
        });
        match bridge {
            Some(bridge) if bridge.allocation.is_some() => {
                let id = bridge.metadata.id().clone();
                self.obligations.insert(Obligation::ApplyBridgeAllocation {
                    metric: metric.metadata.id().clone(),
                    bridge: id.clone(),
                });
                self.obligations
                    .insert(Obligation::ValidateAllocationNormalization { bridge: id });
            }
            _ => {
                let mut refs = vec![metric.metadata.id().clone()];
                refs.extend(path.relationships());
                self.reject(SemanticError::new(
                    Code::UnresolvedFanout,
                    usage.path.clone(),
                    refs,
                    format!(
                        "Agrupar {} por {} multiplica sus filas a través de {}: el resultado \
                         contaría de más. Declara un puente con reparto o agrupa por otra dimensión.",
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
        }
    }

    /// Independent facts align only after each is aggregated at its own
    /// grain (§13.1, §14).
    fn align(&mut self, metrics: &[(&'m Metric, String)], requested: &[RequestedAtom]) {
        for (metric, _) in metrics {
            for measure in self.lineage(metric) {
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
                self.reject(SemanticError::new(
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
            let requested_atoms = as_grain(requested);
            if requested_atoms.as_ref() == Some(prerequisite_grain) {
                self.obligations.insert(Obligation::ApplyOrderedCumulative {
                    metric: id,
                    base: base_metric.clone(),
                    order_by: order_by.iter().map(|order| order.value.clone()).collect(),
                    partition_by: partition_by.clone(),
                });
            } else {
                self.reject(SemanticError::new(
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
                self.reject(SemanticError::new(
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

    /// Time bucketing (§17.1): an instant is converted to its business zone
    /// before any calendar bucket; a derived date keeps the zone it was
    /// derived in and only rolls up to coarser compatible grains.
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
            match derivation(&binding.expression) {
                Some(bucket) => self.derived(usage, dimension, &dataset, bucket),
                None => self.raw(usage, dimension, &dataset),
            }
        }
    }

    fn derived(
        &mut self,
        usage: &Usage<'_>,
        dimension: &'m Dimension,
        dataset: &StableId,
        bucket: &TimeBucket,
    ) {
        let id = dimension.metadata.id().clone();
        let mut grain = bucket.grain;
        if let Some(time) = usage.bucket {
            if time
                .timezone
                .as_ref()
                .is_some_and(|zone| zone != &bucket.timezone)
            {
                self.reject(SemanticError::new(
                    Code::InvalidGrainTransition,
                    format!("{}/timezone", usage.path),
                    [id.clone()],
                    format!(
                        "{id} ya se calcula en la zona {}: no se puede volver a agrupar en otra.",
                        bucket.timezone.as_str()
                    ),
                ));
                return;
            }
            if !rolls_up(bucket.grain, time.grain) {
                self.reject(SemanticError::new(
                    Code::GrainRefinementUnsupported,
                    usage.path.clone(),
                    [id.clone()],
                    format!(
                        "{id} ya está agrupada por {:?}: no se puede pasar a {:?}.",
                        bucket.grain, time.grain
                    ),
                ));
                return;
            }
            grain = time.grain;
        }
        self.obligations
            .insert(Obligation::ConvertTimezoneBeforeBucketing {
                dimension: id,
                dataset: dataset.clone(),
                timezone: bucket.timezone.clone(),
                grain,
            });
    }

    fn raw(&mut self, usage: &Usage<'_>, dimension: &'m Dimension, dataset: &StableId) {
        let Some(time) = usage.bucket else {
            return;
        };
        let id = dimension.metadata.id().clone();
        match &dimension.value_type {
            ValueType::Date => {
                if time.timezone.is_some() {
                    self.reject(SemanticError::new(
                        Code::InvalidGrainTransition,
                        format!("{}/timezone", usage.path),
                        [id.clone()],
                        format!("{id} es una fecha de calendario: no tiene zona horaria."),
                    ));
                } else if time.grain < TimeGrain::Day {
                    self.reject(SemanticError::new(
                        Code::GrainRefinementUnsupported,
                        usage.path.clone(),
                        [id.clone()],
                        format!(
                            "{id} es una fecha: no se puede agrupar por {:?}.",
                            time.grain
                        ),
                    ));
                }
            }
            ValueType::Timestamp {
                timezone_semantics: TimestampSemantics::Instant,
            } => {
                let zone = time.timezone.clone().or_else(|| {
                    dimension
                        .time_semantics
                        .as_ref()
                        .map(|semantics| semantics.timezone.clone())
                });
                match zone {
                    Some(timezone) => {
                        self.obligations
                            .insert(Obligation::ConvertTimezoneBeforeBucketing {
                                dimension: id,
                                dataset: dataset.clone(),
                                timezone,
                                grain: time.grain,
                            });
                    }
                    None => self.reject(SemanticError::new(
                        Code::InvalidGrainTransition,
                        format!("{}/timezone", usage.path),
                        [id.clone()],
                        format!(
                            "{id} es un instante sin zona de negocio declarada: indica en qué \
                             zona agrupar por {:?}.",
                            time.grain
                        ),
                    )),
                }
            }
            _ => {
                if time.timezone.is_some() {
                    self.reject(SemanticError::new(
                        Code::InvalidGrainTransition,
                        format!("{}/timezone", usage.path),
                        [id.clone()],
                        format!("{id} es una fecha y hora local: no admite otra zona."),
                    ));
                }
            }
        }
    }

    // ---- 9. Policies ---------------------------------------------------------------

    /// Every mandatory policy on a dataset the request reads, or on a metric
    /// it computes, must have its context bound (§20); the caller can only
    /// narrow what the policy allows, never remove it.
    fn policies(&mut self, metrics: &[(&'m Metric, String)]) {
        let mut scoped: BTreeSet<(StableId, StableId)> = BTreeSet::new();
        for dataset in &self.datasets {
            if let Some(found) = graph::dataset_of(self.model, dataset) {
                for policy in &found.policy_refs {
                    scoped.insert((policy.clone(), dataset.clone()));
                }
            }
        }
        for (metric, _) in metrics {
            for policy in &metric.policy_refs {
                scoped.insert((policy.clone(), metric.metadata.id().clone()));
            }
        }
        for (policy_id, scope) in scoped {
            let Some(policy) = self
                .model
                .policies
                .iter()
                .find(|policy| policy.metadata.id() == &policy_id)
            else {
                continue;
            };
            if policy.enforcement != Enforcement::Mandatory {
                continue;
            }
            let mut bound = true;
            for slot in policy.context_slots.iter().filter(|slot| slot.required) {
                match self.context.slots.get(&slot.name) {
                    None => {
                        bound = false;
                        self.reject(SemanticError::new(
                            Code::PolicyContextMissing,
                            format!("/context/slots/{}", slot.name),
                            [policy_id.clone(), scope.clone()],
                            format!(
                                "La política obligatoria {policy_id} sobre {scope} necesita el \
                                 valor de contexto {:?}.",
                                slot.name
                            ),
                        ));
                    }
                    Some(value) if !compatible(&value.value_type(), &slot.value_type) => {
                        bound = false;
                        self.reject(SemanticError::new(
                            Code::IncompatibleTypes,
                            format!("/context/slots/{}", slot.name),
                            [policy_id.clone()],
                            format!(
                                "El valor de contexto {:?} no es del tipo que declara la política \
                                 {policy_id}.",
                                slot.name
                            ),
                        ));
                    }
                    Some(_) => {}
                }
            }
            if bound {
                self.facts.insert(Fact::PolicyRequired {
                    scope: scope.clone(),
                    policy: policy_id.clone(),
                });
                self.obligations.insert(Obligation::ApplyMandatoryPolicy {
                    policy: policy_id,
                    scope,
                });
            }
        }
    }
}

// ---- Helpers ---------------------------------------------------------------------

fn unknown(id: &StableId, path: String) -> SemanticError {
    SemanticError::new(
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

/// The requested grain as CSMIR atoms, when it has no time buckets.
fn as_grain(requested: &[RequestedAtom]) -> Option<Grain> {
    requested
        .iter()
        .map(|atom| match atom {
            RequestedAtom::Dimension(dimension) => Some(GrainAtom::DimensionRef {
                dimension: dimension.dimension.clone(),
                roles: dimension.roles.clone(),
            }),
            RequestedAtom::TimeBucket(_) => None,
        })
        .collect::<Option<Vec<_>>>()
        .map(Grain::new)
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

fn compatible(value: &ValueType, declared: &ValueType) -> bool {
    match (value, declared) {
        (ValueType::Integer { signed: a, .. }, ValueType::Integer { signed: b, .. }) => a == b,
        (value, declared) => value == declared,
    }
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
