//! The independent certificate checker (§4.1, §27.1, §33). It reads only
//! the compiled model, the validated query and the finished plan — never
//! what the builder believes it did — and finds, for every obligation, plan
//! evidence with exactly the obligation's parameters. Graph properties are
//! checked transitively over the whole plan. A plan it cannot certify is
//! rejected; there is no partial certificate.

use std::collections::BTreeSet;

use sef_correctness::{BlockValidation, Fact, Obligation, ValidatedQuery};
use sef_exec_ir::{Contract, NodeId, ObjectRef, Operator, SemanticPlan};
use sef_model_ir::diagnostic::{Diagnostic, ErrorCode, Evidence as Ev, Phase, Severity};
use sef_model_ir::expressions::ScalarExpr;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{
    CompiledSemanticModel, Metric, ModelDefinition, TemporalSelection, TemporalTies, WindowBehavior,
};
use sef_model_ir::policy::{Enforcement, PolicyScope};
use sef_query_ir::query::{QualifiedDimensionRef, RequestedAtom, SelectionItem};

use crate::certificate::{Certificate, Discharge, Evidence};

pub(crate) fn planning_error(code: ErrorCode, path: &str, message: String) -> Diagnostic {
    Diagnostic {
        schema_version: "0.1",
        code,
        phase: Phase::Planning,
        severity: Severity::Error,
        primary_ref: None,
        related_refs: Vec::new(),
        query_refs: vec![path.to_owned()],
        evidence: vec![Ev::Constraint {
            name: code.as_str().to_owned(),
        }],
        suggestion_codes: Vec::new(),
        message,
        source_span: None,
    }
}

/// Metrics a metric is computed from: references, recomposition components
/// and the base of a window.
pub(crate) fn metric_inputs(metric: &Metric) -> Vec<StableId> {
    let mut inputs = metric.expression.metric_refs();
    if let Some(decomposition) = &metric.aggregation_contract.decomposition {
        inputs.extend(decomposition.components.iter().cloned());
    }
    if let Some(WindowBehavior::Cumulative { base_metric, .. }) = &metric.window_behavior {
        inputs.push(base_metric.clone());
    }
    inputs.sort();
    inputs.dedup();
    inputs
}

/// The fields a hop through `relationship` reads: its join keys and, for an
/// as-of relationship, the event time and validity interval.
pub(crate) fn hop_fields(model: &ModelDefinition, relationship: &StableId) -> Vec<StableId> {
    let Some(found) = model
        .relationships
        .iter()
        .find(|candidate| candidate.metadata.id() == relationship)
    else {
        return Vec::new();
    };
    let mut fields: Vec<StableId> = found
        .key_pairs
        .iter()
        .flat_map(|pair| [pair.from_field.clone(), pair.to_field.clone()])
        .collect();
    if let Some(temporal) = &found.temporal_constraint {
        fields.push(temporal.event_time.clone());
        fields.push(temporal.valid_from.clone());
        fields.extend(temporal.valid_to.clone());
    }
    fields
}

/// The weight fields of an allocating bridge, on its bridge dataset.
pub(crate) fn weight_fields(model: &ModelDefinition, bridge: &StableId) -> Vec<StableId> {
    model
        .bridges
        .iter()
        .filter(|candidate| candidate.metadata.id() == bridge)
        .filter_map(|found| found.allocation.as_ref())
        .flat_map(|allocation| allocation.weight_expression.field_refs())
        .collect()
}

/// The subset of `fields` that belongs to `dataset`.
pub(crate) fn owned(
    model: &ModelDefinition,
    dataset: &StableId,
    fields: Vec<StableId>,
) -> Vec<StableId> {
    let own: BTreeSet<&StableId> = model
        .datasets
        .iter()
        .filter(|candidate| candidate.metadata.id() == dataset)
        .flat_map(|found| found.fields.iter().map(|field| field.metadata.id()))
        .collect();
    fields
        .into_iter()
        .filter(|field| own.contains(field))
        .collect()
}

/// Certifies a plan against the query it claims to answer.
///
/// # Errors
/// `PLANNER_OBLIGATION_UNSATISFIED` for an obligation without exact
/// evidence; `PLANNER_INTERNAL_INVARIANT_VIOLATION` for a plan whose shape
/// breaks a semantic invariant whatever the obligations say.
pub fn certify(
    model: &CompiledSemanticModel,
    validated: &ValidatedQuery,
    plan: &SemanticPlan,
) -> Result<Certificate, Vec<Diagnostic>> {
    let shaped = plan.blocks.len() == validated.blocks.len()
        && plan
            .blocks
            .iter()
            .zip(&validated.blocks)
            .all(|(planned, validation)| planned.block_id == validation.block_id);
    if !shaped {
        return Err(vec![planning_error(
            ErrorCode::PlannerInternalInvariantViolation,
            "/blocks",
            "El plan no tiene exactamente un resultado por cada bloque validado.".to_owned(),
        )]);
    }
    let mut discharges = Vec::new();
    let mut errors = Vec::new();
    for (b, ((planned, validation), block)) in plan
        .blocks
        .iter()
        .zip(&validated.blocks)
        .zip(&validated.query.blocks)
        .enumerate()
    {
        let mut checker = Checker {
            compiled: model,
            model: model.definition(),
            plan,
            validation,
            reachable: plan.graph.reachable(&[planned.root]),
            root: planned.root,
            path: format!("/blocks/{b}"),
            errors: Vec::new(),
        };
        checker.structure();
        for obligation in &validation.obligations {
            match checker.discharge(obligation) {
                Ok(evidence) => discharges.push(Discharge {
                    block_id: planned.block_id.clone(),
                    obligation: obligation.clone(),
                    evidence,
                }),
                Err(reason) => {
                    let path = checker.path.clone();
                    checker.errors.push(planning_error(
                        ErrorCode::PlannerObligationUnsatisfied,
                        &path,
                        format!("El plan no demuestra «{obligation}»: {reason}."),
                    ));
                }
            }
        }
        for selection in &block.selections {
            if let SelectionItem::Metric(metric) = &selection.item
                && !checker.produced(metric, &checker.reachable)
            {
                checker.violation(&format!("la métrica seleccionada {metric} no se calcula"));
            }
        }
        errors.extend(checker.errors);
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let encoding = |what: &str| {
        vec![planning_error(
            ErrorCode::PlannerInternalInvariantViolation,
            "/",
            format!("La codificación canónica de {what} superó su presupuesto."),
        )]
    };
    Ok(Certificate {
        model_hash: *model.content_hash(),
        query_hash: validated
            .query
            .fingerprint()
            .map_err(|_| encoding("la consulta"))?,
        seir_hash: plan.fingerprint().map_err(|_| encoding("el plan"))?,
        discharges,
    })
}

/// Row phase, aggregation, or result phase (§14): rows are only read
/// before aggregating, and every result-phase input is already aggregated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Row,
    Aggregate,
    Result,
}

const fn stage(operator: &Operator) -> Stage {
    match operator {
        Operator::AggregateAtGrain { .. } | Operator::DistinctAggregate { .. } => Stage::Aggregate,
        Operator::AlignSemanticBranches { .. }
        | Operator::RecomposeMetric { .. }
        | Operator::ComputeWindowMetric { .. }
        | Operator::ApplyPostAggregateFilter { .. }
        | Operator::ProjectSemanticOutput { .. }
        | Operator::OrderSemanticOutput { .. }
        | Operator::LimitSemanticOutput { .. } => Stage::Result,
        _ => Stage::Row,
    }
}

fn rule_text(rule: TemporalSelection) -> &'static str {
    match rule {
        TemporalSelection::PeriodEnd => "period-end",
        TemporalSelection::PeriodStart => "period-start",
    }
}

fn sorted<T: Ord + Clone>(items: &[T]) -> Vec<T> {
    let mut items = items.to_vec();
    items.sort();
    items.dedup();
    items
}

struct Checker<'a> {
    compiled: &'a CompiledSemanticModel,
    model: &'a ModelDefinition,
    plan: &'a SemanticPlan,
    validation: &'a BlockValidation,
    reachable: BTreeSet<NodeId>,
    root: NodeId,
    path: String,
    errors: Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    fn violation(&mut self, message: &str) {
        let path = self.path.clone();
        self.errors.push(planning_error(
            ErrorCode::PlannerInternalInvariantViolation,
            &path,
            format!("El plan rompe un invariante semántico: {message}."),
        ));
    }

    fn op(&self, id: &NodeId) -> Option<&'a Operator> {
        self.plan.graph.operator(id)
    }

    fn nodes(&self) -> Vec<(NodeId, &'a Operator)> {
        self.reachable
            .iter()
            .filter_map(|id| self.op(id).map(|operator| (*id, operator)))
            .collect()
    }

    /// Everything a node reads, transitively, without itself.
    fn below(&self, id: &NodeId) -> BTreeSet<NodeId> {
        self.op(id)
            .map(|operator| self.plan.graph.reachable(&operator.inputs()))
            .unwrap_or_default()
    }

    /// The row pipeline under a node, following its main input down to the
    /// dataset it reads; nearest first.
    fn chain(&self, id: &NodeId) -> Vec<(NodeId, &'a Operator)> {
        let mut found = Vec::new();
        let mut next = self
            .op(id)
            .and_then(|operator| operator.inputs().first().copied());
        while let Some(current) = next {
            let Some(operator) = self.op(&current) else {
                break;
            };
            found.push((current, operator));
            next = operator.inputs().first().copied();
        }
        found
    }

    /// The dataset read at the bottom of a node's chain (or the node itself).
    fn source(&self, id: &NodeId) -> Option<(NodeId, &'a ObjectRef, &'a [StableId])> {
        let mut nodes = vec![(*id, self.op(id)?)];
        nodes.extend(self.chain(id));
        nodes
            .into_iter()
            .find_map(|(node, operator)| match operator {
                Operator::ReadSemanticDataset { dataset, fields } => {
                    Some((node, dataset, &fields[..]))
                }
                _ => None,
            })
    }

    /// Aggregation nodes with the metrics they serve.
    fn aggregates(&self) -> Vec<(NodeId, &'a [ObjectRef], Vec<&'a ObjectRef>)> {
        self.nodes()
            .into_iter()
            .filter_map(|(id, operator)| match operator {
                Operator::AggregateAtGrain {
                    serves, measures, ..
                } => Some((id, &serves[..], measures.iter().collect())),
                Operator::DistinctAggregate {
                    serves, measure, ..
                } => Some((id, &serves[..], vec![measure])),
                _ => None,
            })
            .collect()
    }

    fn serving(&self, metric: &StableId) -> Vec<NodeId> {
        self.aggregates()
            .into_iter()
            .filter(|(_, serves, _)| serves.iter().any(|served| &served.id == metric))
            .map(|(id, _, _)| id)
            .collect()
    }

    /// Whether some node within `scope` yields `metric` (or measure).
    fn produced(&self, metric: &StableId, scope: &BTreeSet<NodeId>) -> bool {
        scope.iter().any(|id| match self.op(id) {
            Some(Operator::AggregateAtGrain {
                serves, measures, ..
            }) => serves.iter().chain(measures).any(|r| &r.id == metric),
            Some(Operator::DistinctAggregate {
                serves, measure, ..
            }) => &measure.id == metric || serves.iter().any(|r| &r.id == metric),
            Some(
                Operator::RecomposeMetric {
                    metric: produced, ..
                }
                | Operator::ComputeWindowMetric {
                    metric: produced, ..
                },
            ) => &produced.id == metric,
            _ => false,
        })
    }

    fn metric(&self, id: &StableId) -> Option<&'a Metric> {
        self.model
            .metrics
            .iter()
            .find(|metric| metric.metadata.id() == id)
    }

    fn requested(&self) -> Vec<RequestedAtom> {
        self.validation
            .facts
            .iter()
            .find_map(|fact| match fact {
                Fact::RequestedGrain(atoms) => Some(sorted(atoms)),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn proved_path(
        &self,
        anchor: &StableId,
        dimension: &QualifiedDimensionRef,
    ) -> Option<&'a [StableId]> {
        self.validation.facts.iter().find_map(|fact| match fact {
            Fact::RelationshipPathUnique {
                anchor: found,
                dimension: proved,
                relationships,
            } if found == anchor && proved == dimension => Some(&relationships[..]),
            _ => None,
        })
    }

    /// The datasets a path enters from `anchor`, in order.
    fn arrivals(&self, anchor: &StableId, path: &[StableId]) -> Vec<StableId> {
        let mut at = anchor.clone();
        let mut found = Vec::new();
        for id in path {
            if let Some(relationship) = self
                .model
                .relationships
                .iter()
                .find(|candidate| candidate.metadata.id() == id)
            {
                at = if relationship.from_dataset == at {
                    relationship.to_dataset.clone()
                } else {
                    relationship.from_dataset.clone()
                };
            }
            found.push(at.clone());
        }
        found
    }

    // ---- Whole-plan invariants -------------------------------------------------------

    fn structure(&mut self) {
        self.contents();
        self.stages();
        self.grains();
        self.protection();
        self.period_end_privacy();
        self.instants();
        self.derivations();
        self.attachments();
        self.read_fields();
    }

    /// Every model object the plan names still has the content it was
    /// planned with (§24).
    fn contents(&mut self) {
        for (_, operator) in self.nodes() {
            for object in operator.objects() {
                if self.compiled.object_hash(&object.id) != Some(&object.content) {
                    self.violation(&format!(
                        "{} no coincide con su definición en el modelo compilado",
                        object.id
                    ));
                }
            }
        }
    }

    /// Rows before aggregation, aggregation once, results after (§14):
    /// nothing aggregates an aggregate, and alignment, recomposition and
    /// windows only read aggregated values.
    fn stages(&mut self) {
        if self.op(&self.root).map(stage) != Some(Stage::Result) {
            self.violation("el resultado del bloque no es un nodo de resultado");
        }
        for (_, operator) in self.nodes() {
            let own = stage(operator);
            for input in operator.inputs() {
                let Some(read) = self.op(&input) else {
                    continue;
                };
                let allowed = match own {
                    Stage::Row | Stage::Aggregate => stage(read) == Stage::Row,
                    Stage::Result => stage(read) != Stage::Row,
                };
                if !allowed {
                    self.violation(&format!("{} lee {}", operator.name(), read.name()));
                }
            }
        }
    }

    /// Every aggregation and alignment is at exactly the requested grain.
    fn grains(&mut self) {
        let requested = self.requested();
        for (_, operator) in self.nodes() {
            let (Operator::AggregateAtGrain { grain, .. }
            | Operator::DistinctAggregate { grain, .. }
            | Operator::AlignSemanticBranches { grain, .. }) = operator
            else {
                continue;
            };
            if sorted(grain) != requested {
                self.violation(&format!("{} no agrega al grano pedido", operator.name()));
            }
        }
    }

    /// Every mandatory policy of every dataset read is an obligation of
    /// the block, whatever the correctness engine concluded (§20).
    fn protection(&mut self) {
        for (_, operator) in self.nodes() {
            let Operator::ReadSemanticDataset { dataset, .. } = operator else {
                continue;
            };
            let listed = self
                .model
                .datasets
                .iter()
                .find(|candidate| candidate.metadata.id() == &dataset.id)
                .map(|found| found.policy_refs.clone())
                .unwrap_or_default();
            for policy in &self.model.policies {
                let scoped = matches!(&policy.scope, PolicyScope::Dataset(scope) if scope == &dataset.id)
                    || listed.contains(policy.metadata.id());
                let required = Obligation::ApplyMandatoryPolicy {
                    policy: policy.metadata.id().clone(),
                    scope: dataset.id.clone(),
                };
                if scoped
                    && policy.enforcement == Enforcement::Mandatory
                    && !self.validation.obligations.contains(&required)
                {
                    self.violation(&format!(
                        "{} se lee sin exigir su política obligatoria {}",
                        dataset.id,
                        policy.metadata.id()
                    ));
                }
            }
        }
    }

    /// A period-end selection belongs to its metric alone: an aggregate
    /// over selected rows serves that metric and nothing else (§18).
    fn period_end_privacy(&mut self) {
        for (id, serves, _) in self.aggregates() {
            for (_, operator) in self.chain(&id) {
                if let Operator::SelectPeriodEnd { metric, .. } = operator
                    && serves.iter().any(|served| served.id != metric.id)
                {
                    self.violation(&format!(
                        "la selección de fin de periodo de {} alimenta otras métricas",
                        metric.id
                    ));
                }
            }
        }
    }

    /// A bucket over an instant reads its own normalization directly: same
    /// dimension, zone and calendar (§17.1).
    fn instants(&mut self) {
        for (_, operator) in self.nodes() {
            let Operator::TimeBucket {
                input,
                dimension,
                calendar,
                ..
            } = operator
            else {
                continue;
            };
            let instant = self.validation.obligations.iter().any(|obligation| {
                matches!(obligation, Obligation::ConvertTimezoneBeforeBucketing { dimension: d, .. }
                    if d == &dimension.id)
            });
            let normalized = matches!(self.op(input), Some(Operator::NormalizeTime {
                dimension: d, calendar: c, ..
            }) if d == dimension && c == calendar);
            if instant && !normalized {
                self.violation(&format!(
                    "{} se agrupa sin haberse convertido antes a su zona",
                    dimension.id
                ));
            }
        }
    }

    /// Recompositions and windows run after everything they read exists
    /// (§10.4, §12.1).
    fn derivations(&mut self) {
        for (id, operator) in self.nodes() {
            let metric = match operator {
                Operator::RecomposeMetric { metric, .. }
                | Operator::ComputeWindowMetric { metric, .. } => &metric.id,
                _ => continue,
            };
            let Some(found) = self.metric(metric) else {
                continue;
            };
            let below = self.below(&id);
            let mut needed = metric_inputs(found);
            needed.extend(found.expression.measure_refs());
            for input in needed {
                if !self.produced(&input, &below) {
                    self.violation(&format!("{metric} se calcula antes de {input}"));
                }
            }
        }
    }

    /// Every dimension is attached through the path the correctness engine
    /// proved, entering each dataset of the path through its own read.
    fn attachments(&mut self) {
        let requested = self.requested();
        for (id, _, _) in self.aggregates() {
            let Some((_, anchor, _)) = self.source(&id) else {
                self.violation("una agregación no lee ningún conjunto de datos");
                continue;
            };
            let chain = self.chain(&id);
            for atom in &requested {
                let (dimension, bucket) = match atom {
                    RequestedAtom::Dimension(dimension) => (dimension, None),
                    RequestedAtom::TimeBucket(time) => (&time.dimension, Some(time.grain)),
                };
                let attached = chain.iter().any(|(_, operator)| {
                    matches!(operator, Operator::AttachDimension { dimension: d, .. } if d == dimension)
                });
                let bucketed = bucket.is_none_or(|grain| {
                    chain.iter().any(|(_, operator)| {
                        matches!(operator, Operator::TimeBucket { dimension: d, grain: g, .. }
                            if d.id == dimension.dimension && *g == grain)
                    })
                });
                if !attached || !bucketed {
                    self.violation(&format!(
                        "una rama de {} no agrupa por {}",
                        anchor.id, dimension.dimension
                    ));
                }
            }
            for (_, operator) in &chain {
                let Operator::AttachDimension {
                    lookups,
                    dimension,
                    definition,
                    path,
                    ..
                } = operator
                else {
                    continue;
                };
                let ids: Vec<StableId> = path.iter().map(|r| r.id.clone()).collect();
                let proved = self.proved_path(&anchor.id, dimension) == Some(&ids[..]);
                let entered: Vec<StableId> = lookups
                    .iter()
                    .filter_map(|lookup| self.source(lookup).map(|(_, d, _)| d.id.clone()))
                    .collect();
                if !proved
                    || definition.id != dimension.dimension
                    || entered != self.arrivals(&anchor.id, &ids)
                {
                    self.violation(&format!(
                        "{} se une por un camino distinto del probado desde {}",
                        dimension.dimension, anchor.id
                    ));
                }
            }
        }
    }

    /// Each read provides every field its rows are used for: measures and
    /// their filters, policy predicates, join keys and dimension bindings,
    /// and the order, ties and scope of a period-end selection (§7.1).
    fn read_fields(&mut self) {
        for (id, operator) in self.nodes() {
            let mut wanted: Vec<StableId> = Vec::new();
            match operator {
                Operator::PolicyGuard { policy, .. } => {
                    if let Some(found) = self
                        .model
                        .policies
                        .iter()
                        .find(|p| p.metadata.id() == &policy.id)
                    {
                        wanted.extend(found.predicate.field_refs());
                    }
                }
                Operator::AggregateAtGrain { measures, .. } => {
                    for measure in measures {
                        wanted.extend(self.measure_fields(&measure.id));
                    }
                }
                Operator::DistinctAggregate { measure, .. } => {
                    wanted.extend(self.measure_fields(&measure.id));
                }
                Operator::ApplyIntrinsicFilter { owner, .. } => {
                    if let Some(filter) = self
                        .metric(&owner.id)
                        .and_then(|m| m.intrinsic_filter.as_ref())
                    {
                        wanted.extend(filter.field_refs());
                    }
                }
                Operator::SelectPeriodEnd { metric, .. } => {
                    wanted.extend(self.selection_terms(&metric.id));
                }
                Operator::AttachDimension {
                    dimension,
                    lookups,
                    path,
                    ..
                } => {
                    self.lookup_fields(&id, dimension, lookups, path);
                    continue;
                }
                _ => continue,
            }
            self.require(&id, &wanted, operator.name());
        }
    }

    fn measure_fields(&self, id: &StableId) -> Vec<StableId> {
        self.model
            .measures
            .iter()
            .find(|measure| measure.metadata.id() == id)
            .map(|measure| {
                measure
                    .scalar_expression
                    .iter()
                    .chain(&measure.intrinsic_filter)
                    .flat_map(ScalarExpr::field_refs)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Order, tie and scope terms of a metric's period-end selection.
    fn selection_terms(&self, metric: &StableId) -> Vec<StableId> {
        let Some(behavior) = self.metric(metric).and_then(|m| m.time_behavior.as_ref()) else {
            return Vec::new();
        };
        let mut terms: Vec<StableId> = behavior.order_by.iter().map(|o| o.value.clone()).collect();
        match &behavior.ties {
            TemporalTies::SecondaryKey { fields } => {
                terms.extend(fields.iter().map(|o| o.value.clone()));
            }
            TemporalTies::UniqueByContract { key } => terms.push(key.clone()),
            TemporalTies::Unspecified => {}
        }
        terms
    }

    /// The dimension's binding fields on its arrival dataset, and the join
    /// keys on both sides of every hop.
    fn lookup_fields(
        &mut self,
        id: &NodeId,
        dimension: &QualifiedDimensionRef,
        lookups: &[NodeId],
        path: &[ObjectRef],
    ) {
        let binding = |dataset: &StableId| -> Vec<StableId> {
            self.model
                .dimensions
                .iter()
                .find(|d| d.metadata.id() == &dimension.dimension)
                .into_iter()
                .flat_map(|d| &d.bindings)
                .filter(|b| &b.dataset == dataset)
                .flat_map(|b| b.expression.field_refs())
                .collect()
        };
        let keys = |relationship: &StableId| hop_fields(self.model, relationship);
        let allocated: Vec<StableId> = self
            .chain(id)
            .into_iter()
            .filter_map(|(_, operator)| match operator {
                Operator::BridgeAllocate { bridge, .. } => Some(bridge.id.clone()),
                _ => None,
            })
            .collect();
        let mut checks: Vec<(NodeId, Vec<StableId>)> = Vec::new();
        if let Some((read, anchor, _)) = self.source(id) {
            let mut wanted = path
                .first()
                .map(|first| keys(&first.id))
                .unwrap_or_default();
            if path.is_empty() {
                wanted.extend(binding(&anchor.id));
            }
            checks.push((read, wanted));
        }
        for (i, lookup) in lookups.iter().enumerate() {
            let Some((read, dataset, _)) = self.source(lookup) else {
                continue;
            };
            let mut wanted = path.get(i).map(|hop| keys(&hop.id)).unwrap_or_default();
            match path.get(i + 1) {
                Some(next) => wanted.extend(keys(&next.id)),
                None => wanted.extend(binding(&dataset.id)),
            }
            for bridge in &allocated {
                let enters = self
                    .model
                    .bridges
                    .iter()
                    .any(|b| b.metadata.id() == bridge && b.bridge_dataset == dataset.id);
                if enters {
                    wanted.extend(weight_fields(self.model, bridge));
                }
            }
            checks.push((read, wanted));
        }
        for (read, wanted) in checks {
            self.require(&read, &wanted, "AttachDimension");
        }
    }

    /// The read under `node` has every wanted field that belongs to it.
    fn require(&mut self, node: &NodeId, wanted: &[StableId], user: &str) {
        let Some((_, dataset, fields)) = self.source(node) else {
            return;
        };
        let own: BTreeSet<&StableId> = self
            .model
            .datasets
            .iter()
            .find(|d| d.metadata.id() == &dataset.id)
            .into_iter()
            .flat_map(|d| d.fields.iter().map(|f| f.metadata.id()))
            .collect();
        let missing: Vec<&StableId> = wanted
            .iter()
            .filter(|field| own.contains(field) && !fields.contains(field))
            .collect();
        if let Some(field) = missing.first() {
            self.violation(&format!(
                "la lectura de {} omite {field}, que necesita {user}",
                dataset.id
            ));
        }
    }

    // ---- Obligations ----------------------------------------------------------------

    fn discharge(&self, obligation: &Obligation) -> Result<Vec<Evidence>, String> {
        match obligation {
            Obligation::PreserveMetricGrain { measure, .. } => self.measure_grain(measure),
            Obligation::AggregateBeforeAlignment { grain } => self.alignment(grain),
            Obligation::RecomputeAtTargetGrain { metric, components } => {
                self.recomposition(metric, components)
            }
            Obligation::DoNotReaggregate { metric } => {
                self.producers(metric, "single-aggregation-at-requested-grain")
            }
            Obligation::PreserveExactDecimalSemantics { metric } => {
                self.producers(metric, "typed-definition-and-no-type-changing-operator")
            }
            Obligation::ApplyPeriodEndSelection {
                metric,
                axis,
                rule,
                period,
                scope,
            } => self.period_end(metric, axis, rule, *period, scope),
            Obligation::ApplyOrderedCumulative {
                metric,
                base,
                order_by,
                partition_by,
            } => self.cumulative(metric, base, order_by, partition_by),
            Obligation::UseTemporalRelationship { relationship } => self.temporal(relationship),
            Obligation::UseDeclaredRelationshipRole { relationship, role } => {
                self.role(relationship, role)
            }
            Obligation::ApplyBridgeAllocation { metric, bridge } => self.allocation(metric, bridge),
            Obligation::ValidateAllocationNormalization { bridge } => {
                let node = self.single(|operator| {
                    matches!(operator, Operator::ContractGuard { contract: Contract::AllocationNormalization { bridge: guarded }, input }
                        if &guarded.id == bridge
                            && matches!(self.op(input), Some(Operator::BridgeAllocate { bridge: allocated, .. }) if &allocated.id == bridge))
                })?;
                Ok(vec![Evidence::Node(node)])
            }
            Obligation::ApplyMandatoryPolicy { policy, scope } => self.policy(policy, scope),
            Obligation::ConvertTimezoneBeforeBucketing {
                dimension,
                dataset,
                timezone,
                calendar,
                grain,
            } => {
                let node = self.bucket(dimension, dataset, calendar, *grain, |input| {
                    matches!(self.op(input), Some(Operator::NormalizeTime { dimension: normalized, timezone: zone, calendar: profile, .. })
                        if &normalized.id == dimension && zone == timezone && profile == calendar)
                })?;
                Ok(vec![
                    Evidence::Node(node),
                    Evidence::Invariant("normalize-before-bucket"),
                ])
            }
            Obligation::ApplyCalendarBucket {
                dimension,
                dataset,
                calendar,
                grain,
            } => {
                let node = self.bucket(dimension, dataset, calendar, *grain, |_| true)?;
                Ok(vec![Evidence::Node(node)])
            }
            Obligation::PreserveDeclaredNullSemantics { measure, .. } => {
                // The measure's content (null semantics included) is fixed
                // by its fingerprint in the aggregate.
                let node = self.single(|operator| match operator {
                    Operator::AggregateAtGrain { measures, .. } => {
                        measures.iter().any(|m| &m.id == measure)
                    }
                    Operator::DistinctAggregate { measure: m, .. } => &m.id == measure,
                    _ => false,
                })?;
                Ok(vec![Evidence::Node(node)])
            }
        }
    }

    fn alignment(&self, grain: &[RequestedAtom]) -> Result<Vec<Evidence>, String> {
        let mut evidence = vec![Evidence::Invariant("aligned-inputs-are-aggregated")];
        for (id, operator) in self.nodes() {
            if let Operator::AlignSemanticBranches { grain: aligned, .. } = operator {
                if sorted(aligned) != sorted(grain) {
                    return Err("una alineación no es al grano pedido".to_owned());
                }
                evidence.push(Evidence::Node(id));
            }
        }
        Ok(evidence)
    }

    fn recomposition(
        &self,
        metric: &StableId,
        components: &[StableId],
    ) -> Result<Vec<Evidence>, String> {
        let node = self.single(|operator| {
            matches!(operator, Operator::RecomposeMetric { metric: m, .. } if &m.id == metric)
        })?;
        let below = self.below(&node);
        match components.iter().find(|c| !self.produced(c, &below)) {
            Some(missing) => Err(format!("{missing} no existe antes de recomponer")),
            None => Ok(vec![
                Evidence::Node(node),
                Evidence::Invariant("components-before-recomposition"),
            ]),
        }
    }

    /// Every node computing `metric`, plus the whole-plan property that
    /// makes it correct (checked by [`Self::structure`]).
    fn producers(
        &self,
        metric: &StableId,
        invariant: &'static str,
    ) -> Result<Vec<Evidence>, String> {
        let mut evidence: Vec<Evidence> = self
            .reachable
            .iter()
            .filter(|id| self.produced(metric, &BTreeSet::from([**id])))
            .map(|id| Evidence::Node(*id))
            .collect();
        if evidence.is_empty() {
            return Err(format!("nada calcula {metric}"));
        }
        evidence.push(Evidence::Invariant(invariant));
        Ok(evidence)
    }

    fn cumulative(
        &self,
        metric: &StableId,
        base: &StableId,
        order_by: &[StableId],
        partition_by: &sef_model_ir::grain::Grain,
    ) -> Result<Vec<Evidence>, String> {
        let node = self.single(|operator| {
            matches!(operator, Operator::ComputeWindowMetric { metric: m, base: b, order_by: o, partition_by: p, .. }
                if &m.id == metric && b == base && o == order_by && p == partition_by)
        })?;
        if self.produced(base, &self.below(&node)) {
            Ok(vec![Evidence::Node(node)])
        } else {
            Err(format!("{base} no existe antes del acumulado"))
        }
    }

    fn role(&self, relationship: &StableId, role: &str) -> Result<Vec<Evidence>, String> {
        let declared = self
            .model
            .relationships
            .iter()
            .any(|r| r.metadata.id() == relationship && r.role == role);
        let node = self.single(|operator| {
            matches!(operator, Operator::AttachDimension { path, .. }
                if path.iter().any(|r| &r.id == relationship))
        })?;
        if declared {
            Ok(vec![Evidence::Node(node)])
        } else {
            Err(format!("{relationship} no declara el rol {role}"))
        }
    }

    /// The first reachable node satisfying `test`, in canonical order.
    fn single(&self, test: impl Fn(&Operator) -> bool) -> Result<NodeId, String> {
        self.nodes()
            .into_iter()
            .find(|(_, operator)| test(operator))
            .map(|(id, _)| id)
            .ok_or_else(|| "no hay ningún nodo que lo haga".to_owned())
    }

    fn measure_grain(&self, measure: &StableId) -> Result<Vec<Evidence>, String> {
        let owner = self
            .model
            .measures
            .iter()
            .find(|m| m.metadata.id() == measure)
            .map(|m| &m.dataset)
            .ok_or_else(|| format!("{measure} no existe"))?;
        let nodes: Vec<NodeId> = self
            .aggregates()
            .into_iter()
            .filter(|(_, _, measures)| measures.iter().any(|m| &m.id == measure))
            .map(|(id, _, _)| id)
            .collect();
        if nodes.is_empty() {
            return Err(format!("ninguna agregación calcula {measure}"));
        }
        for node in &nodes {
            if self.source(node).map(|(_, d, _)| &d.id) != Some(owner) {
                return Err(format!("{measure} se agrega fuera de {owner}"));
            }
        }
        Ok(nodes.into_iter().map(Evidence::Node).collect())
    }

    fn period_end(
        &self,
        metric: &StableId,
        axis: &StableId,
        rule: &str,
        period: Option<sef_model_ir::time::TimeGrain>,
        scope: &sef_model_ir::grain::Grain,
    ) -> Result<Vec<Evidence>, String> {
        let aggregates = self.serving(metric);
        if aggregates.is_empty() {
            return Err(format!("ninguna agregación calcula {metric}"));
        }
        let mut evidence = Vec::new();
        for aggregate in aggregates {
            let found = self.chain(&aggregate).into_iter().find(|(_, operator)| {
                matches!(operator, Operator::SelectPeriodEnd { metric: selected, axis: on, rule: kept, period: per, scope: within, .. }
                    if &selected.id == metric && on == axis && rule_text(*kept) == rule && *per == period && within == scope)
            });
            match found {
                Some((node, _)) => evidence.push(Evidence::Node(node)),
                None => {
                    return Err(format!(
                        "una rama de {metric} agrega sin seleccionar el periodo"
                    ));
                }
            }
        }
        evidence.push(Evidence::Invariant("period-end-private-to-its-metric"));
        Ok(evidence)
    }

    fn temporal(&self, relationship: &StableId) -> Result<Vec<Evidence>, String> {
        let mut evidence = Vec::new();
        for (id, operator) in self.nodes() {
            let Operator::AttachDimension { path, .. } = operator else {
                continue;
            };
            if !path.iter().any(|r| &r.id == relationship) {
                continue;
            }
            let chain = self.chain(&id);
            let asof = chain.iter().position(|(_, op)| {
                matches!(op, Operator::TemporalAsOf { relationship: r, .. } if &r.id == relationship)
            });
            let guarded = asof.is_some_and(|i| {
                i > 0
                    && matches!(chain[i - 1].1, Operator::ContractGuard { contract: Contract::TemporalSingleMatch { relationship: r }, .. }
                        if &r.id == relationship)
            });
            match (asof, guarded) {
                (Some(i), true) => {
                    evidence.push(Evidence::Node(chain[i].0));
                    evidence.push(Evidence::Node(chain[i - 1].0));
                }
                _ => {
                    return Err(format!(
                        "{relationship} se recorre sin corte temporal único"
                    ));
                }
            }
        }
        if evidence.is_empty() {
            Err(format!("ningún camino recorre {relationship}"))
        } else {
            Ok(evidence)
        }
    }

    fn allocation(&self, metric: &StableId, bridge: &StableId) -> Result<Vec<Evidence>, String> {
        let entry = self
            .model
            .bridges
            .iter()
            .find(|b| b.metadata.id() == bridge)
            .map(|b| &b.left_relationship)
            .ok_or_else(|| format!("{bridge} no existe"))?;
        let mut evidence = Vec::new();
        for aggregate in self.serving(metric) {
            let chain = self.chain(&aggregate);
            let crosses = chain.iter().any(|(_, op)| {
                matches!(op, Operator::AttachDimension { path, .. } if path.iter().any(|r| &r.id == entry))
            });
            let allocated = chain.iter().find(|(_, op)| {
                matches!(op, Operator::BridgeAllocate { bridge: b, .. } if &b.id == bridge)
            });
            match (crosses, allocated) {
                (true, Some((node, _))) => evidence.push(Evidence::Node(*node)),
                (true, None) => return Err(format!("{metric} cruza {bridge} sin repartir")),
                (false, _) => {}
            }
        }
        if evidence.is_empty() {
            Err(format!("{metric} no se reparte por {bridge}"))
        } else {
            Ok(evidence)
        }
    }

    /// A dataset policy dominates every read of its dataset: no path from
    /// the result reaches such a read without crossing the policy's guard,
    /// and the guard sits directly on reads of that dataset. A metric
    /// policy guards the rows of every aggregate serving the metric.
    fn policy(&self, policy: &StableId, scope: &StableId) -> Result<Vec<Evidence>, String> {
        let guards: Vec<NodeId> = self
            .nodes()
            .into_iter()
            .filter(
                |(_, op)| matches!(op, Operator::PolicyGuard { policy: p, .. } if &p.id == policy),
            )
            .map(|(id, _)| id)
            .collect();
        if guards.is_empty() {
            return Err(format!("ninguna protección aplica {policy}"));
        }
        let dataset = self.model.datasets.iter().any(|d| d.metadata.id() == scope);
        if dataset {
            for guard in &guards {
                let on_source = self.chain(guard).iter().all(|(_, op)| {
                    matches!(op, Operator::PolicyGuard { .. })
                        || matches!(op, Operator::ReadSemanticDataset { dataset: d, .. } if &d.id == scope)
                });
                if !on_source {
                    return Err(format!(
                        "{policy} no protege directamente la lectura de {scope}"
                    ));
                }
            }
            // Search from the result without entering this policy's guards.
            let mut seen: BTreeSet<NodeId> = BTreeSet::new();
            let mut pending = vec![self.root];
            while let Some(id) = pending.pop() {
                if guards.contains(&id) || !seen.insert(id) {
                    continue;
                }
                let Some(operator) = self.op(&id) else {
                    continue;
                };
                if matches!(operator, Operator::ReadSemanticDataset { dataset: d, .. } if &d.id == scope)
                {
                    return Err(format!("una lectura de {scope} escapa a {policy}"));
                }
                pending.extend(operator.inputs());
            }
        } else {
            let mut lineage: BTreeSet<StableId> = BTreeSet::from([scope.clone()]);
            let mut pending = vec![scope.clone()];
            while let Some(id) = pending.pop() {
                for input in self.metric(&id).map(metric_inputs).unwrap_or_default() {
                    if lineage.insert(input.clone()) {
                        pending.push(input);
                    }
                }
            }
            for (aggregate, serves, _) in self.aggregates() {
                if !serves.iter().any(|served| lineage.contains(&served.id)) {
                    continue;
                }
                let guarded = self
                    .chain(&aggregate)
                    .iter()
                    .any(|(id, _)| guards.contains(id));
                if !guarded {
                    return Err(format!("una rama de {scope} no aplica {policy}"));
                }
            }
        }
        let mut evidence: Vec<Evidence> = guards.into_iter().map(Evidence::Node).collect();
        evidence.push(Evidence::Invariant("policy-guard-dominates-source"));
        Ok(evidence)
    }

    /// A bucket of `dimension` with the exact grain and calendar, whose
    /// values arrive from `dataset`, and whose input passes `before`.
    fn bucket(
        &self,
        dimension: &StableId,
        dataset: &StableId,
        calendar: &sef_model_ir::time::CalendarProfile,
        grain: sef_model_ir::time::TimeGrain,
        before: impl Fn(&NodeId) -> bool,
    ) -> Result<NodeId, String> {
        let mut found = None;
        for (id, operator) in self.nodes() {
            let Operator::TimeBucket {
                input,
                dimension: d,
                grain: g,
                calendar: c,
            } = operator
            else {
                continue;
            };
            if &d.id != dimension || *g != grain || c != calendar {
                continue;
            }
            if !before(input) {
                return Err(format!("{dimension} se agrupa sin el paso previo exigido"));
            }
            let arrives = self.source(&id).is_some_and(|(_, anchor, _)| {
                self.chain(&id).iter().any(|(_, op)| match op {
                    Operator::AttachDimension {
                        dimension: q, path, ..
                    } if &q.dimension == dimension => {
                        let ids: Vec<StableId> = path.iter().map(|r| r.id.clone()).collect();
                        self.arrivals(&anchor.id, &ids).last().unwrap_or(&anchor.id) == dataset
                    }
                    _ => false,
                })
            });
            if arrives {
                found = Some(id);
            }
        }
        found.ok_or_else(|| format!("ningún cubo de {dimension} sale de {dataset}"))
    }
}
