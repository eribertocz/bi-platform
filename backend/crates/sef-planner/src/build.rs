//! The canonical planning pipeline (§14): one deterministic baseline plan
//! per validated query block, built from the correctness engine's facts and
//! obligations without re-deciding validity, and checked complete (§33).

use std::collections::{BTreeMap, BTreeSet};

use sef_correctness::{BlockValidation, Fact, Obligation, ValidatedQuery};
use sef_exec_ir::{Contract, NodeId, Operator, PlanBlock, SeirGraph, SemanticPlan};
use sef_model_ir::diagnostic::{Diagnostic, ErrorCode, Evidence as Ev, Phase, Severity};
use sef_model_ir::expressions::AggregateFunction;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{
    CompiledSemanticModel, Measure, Metric, ModelDefinition, TemporalSelection,
};
use sef_model_ir::relationships::ContractEvidenceState;
use sef_model_ir::time::CalendarProfile;
use sef_query_ir::query::{
    FilterExpr, FilterTarget, QualifiedDimensionRef, RequestedAtom, SelectionItem,
    SemanticQueryBlock,
};

use crate::certificate::{Certificate, Discharge, Evidence};

/// A certified semantic plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedQuery {
    pub plan: SemanticPlan,
    pub certificate: Certificate,
}

/// Builds the canonical baseline plan of a validated query.
///
/// # Errors
/// Planner capability diagnostics (phase `Planning`): an obligation this
/// planner cannot discharge, or a broken internal invariant. Never a
/// different semantic interpretation of the query (§25).
pub fn plan(
    model: &CompiledSemanticModel,
    validated: &ValidatedQuery,
) -> Result<PlannedQuery, Vec<Diagnostic>> {
    let mut graph = SeirGraph::new();
    let mut blocks = Vec::new();
    let mut discharges = Vec::new();
    let mut errors = Vec::new();
    for (b, (block, validation)) in validated
        .query
        .blocks
        .iter()
        .zip(&validated.blocks)
        .enumerate()
    {
        let mut planner = BlockPlanner {
            model: model.definition(),
            graph: &mut graph,
            validation,
            block_id: block.block_id.clone(),
            path: format!("/blocks/{b}"),
            witnesses: BTreeMap::new(),
            errors: Vec::new(),
        };
        match planner.block(block) {
            Some(root) => {
                discharges.extend(planner.discharges(root));
                errors.extend(planner.errors);
                blocks.push(PlanBlock {
                    block_id: block.block_id.clone(),
                    root,
                });
            }
            None => errors.extend(planner.errors),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let plan = SemanticPlan { graph, blocks };
    let encoding = |what: &str| {
        vec![planning_error(
            ErrorCode::PlannerInternalInvariantViolation,
            "/",
            format!("La codificación canónica de {what} superó su presupuesto."),
        )]
    };
    let certificate = Certificate {
        model_hash: *model.content_hash(),
        query_hash: validated
            .query
            .fingerprint()
            .map_err(|_| encoding("la consulta"))?,
        seir_hash: plan.fingerprint().map_err(|_| encoding("el plan"))?,
        discharges,
    };
    Ok(PlannedQuery { plan, certificate })
}

fn planning_error(code: ErrorCode, path: &str, message: String) -> Diagnostic {
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

struct BlockPlanner<'p, 'm> {
    model: &'m ModelDefinition,
    graph: &'p mut SeirGraph,
    validation: &'p BlockValidation,
    block_id: String,
    path: String,
    witnesses: BTreeMap<Obligation, Vec<Evidence>>,
    errors: Vec<Diagnostic>,
}

impl<'p, 'm> BlockPlanner<'p, 'm> {
    fn fail(&mut self, code: ErrorCode, message: String) {
        let path = self.path.clone();
        self.errors.push(planning_error(code, &path, message));
    }

    fn add(&mut self, operator: Operator) -> Option<NodeId> {
        match self.graph.add(operator) {
            Ok(id) => Some(id),
            Err(error) => {
                self.fail(
                    ErrorCode::PlannerInternalInvariantViolation,
                    format!("No se pudo añadir un nodo al plan: {error}."),
                );
                None
            }
        }
    }

    fn witness(&mut self, obligation: Obligation, evidence: Evidence) {
        self.witnesses.entry(obligation).or_default().push(evidence);
    }

    fn obligations(&self) -> impl Iterator<Item = &'p Obligation> + use<'p, 'm> {
        self.validation.obligations.iter()
    }

    fn metric(&self, id: &StableId) -> Option<&'m Metric> {
        self.model
            .metrics
            .iter()
            .find(|metric| metric.metadata.id() == id)
    }

    /// Primitive measures of a metric, through metric references.
    fn lineage(&self, metric: &'m Metric) -> BTreeMap<StableId, &'m Measure> {
        let mut found = BTreeMap::new();
        let mut pending = vec![metric];
        let mut seen = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current.metadata.id().clone()) {
                continue;
            }
            for id in current.expression.measure_refs() {
                if let Some(measure) = self.model.measures.iter().find(|m| m.metadata.id() == &id) {
                    found.insert(id, measure);
                }
            }
            for id in current.expression.metric_refs() {
                if let Some(next) = self.metric(&id) {
                    pending.push(next);
                }
            }
        }
        found
    }

    // ---- The block ---------------------------------------------------------------

    fn block(&mut self, block: &SemanticQueryBlock) -> Option<NodeId> {
        let requested = block.requested_grain();
        let metrics: Vec<&'m Metric> = block
            .selections
            .iter()
            .filter_map(|selection| match &selection.item {
                SelectionItem::Metric(id) => self.metric(id),
                _ => None,
            })
            .collect();
        // Phase 2: metric dependency requirements, by fact dataset.
        let mut anchors: BTreeMap<StableId, BTreeMap<StableId, &'m Measure>> = BTreeMap::new();
        for metric in &metrics {
            for (id, measure) in self.lineage(metric) {
                anchors
                    .entry(measure.dataset.clone())
                    .or_default()
                    .insert(id, measure);
            }
        }
        let usages = usages(block, &requested);
        // Phases 3–9: one protected, aggregated branch per fact.
        let mut branches = Vec::new();
        for (anchor, measures) in &anchors {
            branches.push(self.branch(anchor, measures, &usages, block, &requested)?);
        }
        // Phase 10: alignment of independent facts.
        let mut node = if let [single] = branches.as_slice() {
            *single
        } else if branches.is_empty() {
            // SQIR binding rejects a block without metrics (`NoMetric`): one
            // here means the input skipped binding. Never an empty plan.
            self.fail(
                ErrorCode::PlannerInternalInvariantViolation,
                "Un bloque sin métricas no debería llegar al planificador: SQIR lo rechaza al                  vincular."
                    .to_owned(),
            );
            return None;
        } else {
            let aligned = self.add(Operator::AlignSemanticBranches {
                inputs: branches.clone(),
                grain: requested.clone(),
            })?;
            for obligation in self.obligations().cloned().collect::<Vec<_>>() {
                if matches!(obligation, Obligation::AggregateBeforeAlignment { .. }) {
                    self.witness(obligation.clone(), Evidence::Node(aligned));
                    self.witness(
                        obligation,
                        Evidence::Invariant("aligned-inputs-are-aggregated"),
                    );
                }
            }
            aligned
        };
        // Phases 11–12: recomposition, then windows.
        node = self.recompose(node)?;
        node = self.windows(node)?;
        // Phase 13: post-aggregate filter and result shape.
        if let Some(predicate) = &block.post_aggregate_filter {
            node = self.add(Operator::ApplyPostAggregateFilter {
                input: node,
                predicate: predicate.clone(),
            })?;
        }
        node = self.add(Operator::ProjectSemanticOutput {
            input: node,
            selections: block.selections.clone(),
        })?;
        if !block.order_by.is_empty() {
            node = self.add(Operator::OrderSemanticOutput {
                input: node,
                order: block.order_by.clone(),
            })?;
        }
        if block.limit.is_some() || block.offset.is_some() {
            node = self.add(Operator::LimitSemanticOutput {
                input: node,
                limit: block.limit,
                offset: block.offset,
            })?;
        }
        self.annotations();
        Some(node)
    }

    /// One fact branch: read, guard, relate, bucket, filter, select and
    /// aggregate, in that semantic order.
    fn branch(
        &mut self,
        anchor: &StableId,
        measures: &BTreeMap<StableId, &'m Measure>,
        usages: &[QualifiedDimensionRef],
        block: &SemanticQueryBlock,
        requested: &[RequestedAtom],
    ) -> Option<NodeId> {
        let fields: Vec<StableId> = measures
            .values()
            .filter_map(|measure| measure.scalar_expression.as_ref())
            .flat_map(sef_model_ir::expressions::ScalarExpr::field_refs)
            .collect();
        let mut node = self.add(Operator::ReadSemanticDataset {
            dataset: anchor.clone(),
            fields,
        })?;
        node = self.guard(node, anchor)?;
        for dimension in usages {
            node = self.relate(node, anchor, dimension)?;
        }
        node = self.time(node, anchor, requested)?;
        if let Some(predicate) = &block.filter {
            node = self.add(Operator::ApplyRowFilter {
                input: node,
                predicate: predicate.clone(),
            })?;
        }
        node = self.period_end(node, measures)?;
        self.aggregate(node, measures, requested)
    }

    /// A mandatory policy on `dataset` guards its rows right after they are
    /// read, so it dominates everything derived from them.
    fn guard(&mut self, mut node: NodeId, dataset: &StableId) -> Option<NodeId> {
        let policies: Vec<Obligation> = self
            .obligations()
            .filter(|obligation| {
                matches!(obligation, Obligation::ApplyMandatoryPolicy { scope, .. } if scope == dataset)
            })
            .cloned()
            .collect();
        for obligation in policies {
            let Obligation::ApplyMandatoryPolicy { policy, .. } = &obligation else {
                continue;
            };
            let slots = self
                .model
                .policies
                .iter()
                .find(|candidate| candidate.metadata.id() == policy)
                .map(|found| {
                    found
                        .context_slots
                        .iter()
                        .map(|slot| slot.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            node = self.add(Operator::PolicyGuard {
                input: node,
                policy: policy.clone(),
                slots,
            })?;
            self.witness(obligation.clone(), Evidence::Node(node));
            self.witness(
                obligation,
                Evidence::Invariant("policy-guard-dominates-source"),
            );
        }
        Some(node)
    }

    /// Attaches a dimension through the path the correctness engine proved,
    /// with its temporal, allocation and uniqueness semantics on the way.
    fn relate(
        &mut self,
        mut node: NodeId,
        anchor: &StableId,
        dimension: &QualifiedDimensionRef,
    ) -> Option<NodeId> {
        let Some(path) = self.validation.facts.iter().find_map(|fact| match fact {
            Fact::RelationshipPathUnique {
                anchor: found,
                dimension: proved,
                relationships,
            } if found == anchor && proved == dimension => Some(relationships.clone()),
            _ => None,
        }) else {
            self.fail(
                ErrorCode::PlannerInternalInvariantViolation,
                format!(
                    "El motor de corrección no probó un camino de {anchor} a {}.",
                    dimension.dimension
                ),
            );
            return None;
        };
        for id in &path {
            node = self.traverse(node, id)?;
        }
        node = self.add(Operator::AttachDimension {
            input: node,
            dimension: dimension.clone(),
            path: path.clone(),
        })?;
        for obligation in self.obligations().cloned().collect::<Vec<_>>() {
            if let Obligation::UseDeclaredRelationshipRole { relationship, .. } = &obligation
                && path.contains(relationship)
            {
                self.witness(obligation, Evidence::Node(node));
            }
        }
        Some(node)
    }

    fn traverse(&mut self, mut node: NodeId, id: &StableId) -> Option<NodeId> {
        let relationship = self
            .model
            .relationships
            .iter()
            .find(|relationship| relationship.metadata.id() == id)?;
        if relationship.temporal_constraint.is_some() {
            node = self.add(Operator::TemporalAsOf {
                input: node,
                relationship: id.clone(),
            })?;
            let asof = node;
            node = self.add(Operator::ContractGuard {
                input: node,
                contract: Contract::TemporalSingleMatch {
                    relationship: id.clone(),
                },
            })?;
            let obligation = Obligation::UseTemporalRelationship {
                relationship: id.clone(),
            };
            if self.validation.obligations.contains(&obligation) {
                self.witness(obligation, Evidence::Node(asof));
            }
        } else if relationship.evidence == ContractEvidenceState::DeclaredUnverified {
            // A declared, not yet certified, unique target (§27: CASE-003).
            node = self.add(Operator::ContractGuard {
                input: node,
                contract: Contract::UniqueTarget {
                    relationship: id.clone(),
                },
            })?;
        }
        let allocations: Vec<Obligation> = self
            .obligations()
            .filter(|obligation| matches!(obligation, Obligation::ApplyBridgeAllocation { .. }))
            .cloned()
            .collect();
        for obligation in allocations {
            let Obligation::ApplyBridgeAllocation { bridge, .. } = &obligation else {
                continue;
            };
            let Some(declared) = self
                .model
                .bridges
                .iter()
                .find(|b| b.metadata.id() == bridge)
            else {
                continue;
            };
            // Allocation happens once, where the path enters the bridge.
            if &declared.left_relationship == id {
                node = self.add(Operator::BridgeAllocate {
                    input: node,
                    bridge: bridge.clone(),
                })?;
                self.witness(obligation.clone(), Evidence::Node(node));
                node = self.add(Operator::ContractGuard {
                    input: node,
                    contract: Contract::AllocationNormalization {
                        bridge: bridge.clone(),
                    },
                })?;
                self.witness(
                    Obligation::ValidateAllocationNormalization {
                        bridge: bridge.clone(),
                    },
                    Evidence::Node(node),
                );
            }
        }
        Some(node)
    }

    /// Instant → business zone → calendar bucket (§9, §22), and plain
    /// buckets of calendar dates.
    fn time(
        &mut self,
        mut node: NodeId,
        anchor: &StableId,
        requested: &[RequestedAtom],
    ) -> Option<NodeId> {
        let conversions: Vec<Obligation> = self
            .obligations()
            .filter(|obligation| {
                matches!(obligation, Obligation::ConvertTimezoneBeforeBucketing { dataset, .. } if dataset == anchor)
            })
            .cloned()
            .collect();
        let mut converted = BTreeSet::new();
        for obligation in conversions {
            let Obligation::ConvertTimezoneBeforeBucketing {
                dimension,
                timezone,
                grain,
                ..
            } = &obligation
            else {
                continue;
            };
            let calendar = self.calendar(dimension);
            let normalized = self.add(Operator::NormalizeTime {
                input: node,
                dimension: dimension.clone(),
                timezone: timezone.clone(),
                calendar: calendar.clone(),
            })?;
            node = self.add(Operator::TimeBucket {
                input: normalized,
                dimension: dimension.clone(),
                grain: *grain,
                calendar,
            })?;
            converted.insert(dimension.clone());
            self.witness(obligation.clone(), Evidence::Node(normalized));
            self.witness(obligation.clone(), Evidence::Node(node));
            self.witness(obligation, Evidence::Invariant("normalize-before-bucket"));
        }
        for atom in requested {
            if let RequestedAtom::TimeBucket(time) = atom
                && !converted.contains(&time.dimension.dimension)
            {
                let calendar = self.calendar(&time.dimension.dimension);
                node = self.add(Operator::TimeBucket {
                    input: node,
                    dimension: time.dimension.dimension.clone(),
                    grain: time.grain,
                    calendar,
                })?;
            }
        }
        Some(node)
    }

    fn calendar(&self, dimension: &StableId) -> CalendarProfile {
        self.model
            .dimensions
            .iter()
            .find(|candidate| candidate.metadata.id() == dimension)
            .and_then(|found| found.time_semantics.as_ref())
            .map_or(CalendarProfile::Gregorian, |semantics| {
                semantics.calendar.clone()
            })
    }

    /// Semi-additive selection before any aggregation across accounts (§18).
    fn period_end(
        &mut self,
        mut node: NodeId,
        measures: &BTreeMap<StableId, &'m Measure>,
    ) -> Option<NodeId> {
        let selections: Vec<Obligation> = self
            .obligations()
            .filter(|obligation| matches!(obligation, Obligation::ApplyPeriodEndSelection { .. }))
            .cloned()
            .collect();
        for obligation in selections {
            let Obligation::ApplyPeriodEndSelection {
                metric,
                axis,
                period,
                scope,
                ..
            } = &obligation
            else {
                continue;
            };
            let Some(found) = self.metric(metric) else {
                continue;
            };
            if !self
                .lineage(found)
                .keys()
                .any(|id| measures.contains_key(id))
            {
                continue;
            }
            let rule = found
                .time_behavior
                .as_ref()
                .map_or(TemporalSelection::PeriodEnd, |behavior| behavior.rule);
            node = self.add(Operator::SelectPeriodEnd {
                input: node,
                metric: metric.clone(),
                axis: axis.clone(),
                rule,
                period: *period,
                scope: scope.clone(),
            })?;
            self.witness(obligation, Evidence::Node(node));
        }
        Some(node)
    }

    /// Distinct measures are computed apart and never summed (§19); the
    /// others aggregate together at the requested grain.
    fn aggregate(
        &mut self,
        node: NodeId,
        measures: &BTreeMap<StableId, &'m Measure>,
        requested: &[RequestedAtom],
    ) -> Option<NodeId> {
        let (distinct, additive): (Vec<&'m Measure>, Vec<&'m Measure>) = measures
            .values()
            .copied()
            .partition(|measure| measure.aggregation == AggregateFunction::CountDistinct);
        let mut outputs = Vec::new();
        if !additive.is_empty() {
            let aggregated = self.add(Operator::AggregateAtGrain {
                input: node,
                grain: requested.to_vec(),
                measures: additive.iter().map(|m| m.metadata.id().clone()).collect(),
            })?;
            for measure in &additive {
                self.preserve(measure.metadata.id(), aggregated);
            }
            outputs.push(aggregated);
        }
        for measure in &distinct {
            let counted = self.add(Operator::DistinctAggregate {
                input: node,
                grain: requested.to_vec(),
                measure: measure.metadata.id().clone(),
            })?;
            self.preserve(measure.metadata.id(), counted);
            outputs.push(counted);
        }
        match outputs.as_slice() {
            [single] => Some(*single),
            _ => self.add(Operator::AlignSemanticBranches {
                inputs: outputs,
                grain: requested.to_vec(),
            }),
        }
    }

    fn preserve(&mut self, measure: &StableId, node: NodeId) {
        let matching: Vec<Obligation> = self
            .obligations()
            .filter(|obligation| {
                matches!(obligation, Obligation::PreserveMetricGrain { measure: found, .. } if found == measure)
            })
            .cloned()
            .collect();
        for obligation in matching {
            self.witness(obligation, Evidence::Node(node));
        }
    }

    /// Recomposed metrics after their components reach the target grain
    /// (§17), dependencies first.
    fn recompose(&mut self, mut node: NodeId) -> Option<NodeId> {
        let mut pending: Vec<(StableId, Obligation)> = self
            .obligations()
            .filter_map(|obligation| match obligation {
                Obligation::RecomputeAtTargetGrain { metric, .. } => {
                    Some((metric.clone(), obligation.clone()))
                }
                _ => None,
            })
            .collect();
        let mut done: BTreeSet<StableId> = BTreeSet::new();
        while !pending.is_empty() {
            let waiting: BTreeSet<StableId> =
                pending.iter().map(|(metric, _)| metric.clone()).collect();
            let ready = pending.iter().position(|(metric, _)| {
                self.metric(metric).is_some_and(|found| {
                    found.expression.metric_refs().iter().all(|dependency| {
                        !waiting.contains(dependency) || done.contains(dependency)
                    })
                })
            });
            let Some(index) = ready else {
                self.fail(
                    ErrorCode::PlannerInternalInvariantViolation,
                    "Las métricas a recomponer dependen unas de otras en ciclo.".to_owned(),
                );
                return None;
            };
            let (metric, obligation) = pending.remove(index);
            node = self.add(Operator::RecomposeMetric {
                input: node,
                metric: metric.clone(),
            })?;
            self.witness(obligation, Evidence::Node(node));
            done.insert(metric);
        }
        Some(node)
    }

    fn windows(&mut self, mut node: NodeId) -> Option<NodeId> {
        let windows: Vec<Obligation> = self
            .obligations()
            .filter(|obligation| matches!(obligation, Obligation::ApplyOrderedCumulative { .. }))
            .cloned()
            .collect();
        for obligation in windows {
            let Obligation::ApplyOrderedCumulative {
                metric,
                base,
                order_by,
                partition_by,
            } = &obligation
            else {
                continue;
            };
            node = self.add(Operator::ComputeWindowMetric {
                input: node,
                metric: metric.clone(),
                base: base.clone(),
                order_by: order_by.clone(),
                partition_by: partition_by.clone(),
            })?;
            self.witness(obligation, Evidence::Node(node));
        }
        Some(node)
    }

    /// Exactness and null semantics travel as typed annotations (§22).
    fn annotations(&mut self) {
        for obligation in self.obligations().cloned().collect::<Vec<_>>() {
            let annotation = match &obligation {
                Obligation::PreserveExactDecimalSemantics { metric } => self
                    .metric(metric)
                    .map(|found| format!("{metric} {}", type_text(&found.value_type))),
                Obligation::PreserveDeclaredNullSemantics { measure, semantics } => {
                    Some(format!("{measure} {semantics}"))
                }
                Obligation::DoNotReaggregate { .. } => {
                    self.witness(
                        obligation.clone(),
                        Evidence::Invariant("distinct-result-never-reaggregated"),
                    );
                    None
                }
                _ => None,
            };
            if let Some(text) = annotation {
                self.witness(obligation, Evidence::Annotation(text));
            }
        }
    }

    // ---- Completeness (§33) -----------------------------------------------------

    /// Every obligation discharged by evidence reachable from the block's
    /// root, and the graph invariants the evidence names actually hold.
    fn discharges(&mut self, root: NodeId) -> Vec<Discharge> {
        let reachable = self.graph.reachable(&[root]);
        let mut discharges = Vec::new();
        for obligation in self.validation.obligations.clone() {
            let evidence = self.witnesses.get(&obligation).cloned().unwrap_or_default();
            let holds = !evidence.is_empty()
                && evidence.iter().all(|item| match item {
                    Evidence::Node(id) => reachable.contains(id),
                    Evidence::Invariant(name) => self.invariant(name, &reachable),
                    Evidence::Annotation(_) => true,
                });
            if holds {
                discharges.push(Discharge {
                    block_id: self.block_id(),
                    obligation,
                    evidence,
                });
            } else {
                self.fail(
                    ErrorCode::PlannerObligationUnsatisfied,
                    format!("El plan no demuestra la obligación «{obligation}»."),
                );
            }
        }
        discharges
    }

    fn block_id(&self) -> String {
        self.block_id.clone()
    }

    fn invariant(&self, name: &str, reachable: &BTreeSet<NodeId>) -> bool {
        let operators: Vec<&Operator> = reachable
            .iter()
            .filter_map(|id| self.graph.operator(id))
            .collect();
        let is =
            |id: &NodeId, test: fn(&Operator) -> bool| self.graph.operator(id).is_some_and(test);
        match name {
            // Nothing reads a distinct result to aggregate it again.
            "distinct-result-never-reaggregated" => operators.iter().all(|operator| {
                !matches!(operator, Operator::AggregateAtGrain { .. })
                    || operator.inputs().iter().all(|input| {
                        !is(input, |op| matches!(op, Operator::DistinctAggregate { .. }))
                    })
            }),
            // Every alignment input is already aggregated.
            "aligned-inputs-are-aggregated" => operators.iter().all(|operator| match operator {
                Operator::AlignSemanticBranches { inputs, .. } => inputs.iter().all(|input| {
                    is(input, |op| {
                        matches!(
                            op,
                            Operator::AggregateAtGrain { .. }
                                | Operator::DistinctAggregate { .. }
                                | Operator::AlignSemanticBranches { .. }
                        )
                    })
                }),
                _ => true,
            }),
            // Each bucket of a converted dimension reads its normalization.
            "normalize-before-bucket" => operators.iter().all(|operator| match operator {
                Operator::NormalizeTime { dimension, .. } => operators.iter().any(|reader| {
                    matches!(reader, Operator::TimeBucket { dimension: bucketed, input, .. }
                        if bucketed == dimension
                            && is(input, |op| matches!(op, Operator::NormalizeTime { .. })))
                }),
                _ => true,
            }),
            // A protected source is read only by its guard.
            "policy-guard-dominates-source" => operators.iter().all(|operator| {
                operator.inputs().iter().all(|input| {
                    !is(input, |op| {
                        matches!(op, Operator::ReadSemanticDataset { .. })
                    }) || matches!(operator, Operator::PolicyGuard { .. })
                        || !self.protected(input)
                })
            }),
            _ => false,
        }
    }

    /// Whether a read node reads a dataset with a mandatory policy here.
    fn protected(&self, read: &NodeId) -> bool {
        let Some(Operator::ReadSemanticDataset { dataset, .. }) = self.graph.operator(read) else {
            return false;
        };
        self.validation.obligations.iter().any(|obligation| {
            matches!(obligation, Obligation::ApplyMandatoryPolicy { scope, .. } if scope == dataset)
        })
    }
}

/// The exact type a result must keep, as a reader expects it.
fn type_text(value_type: &sef_model_ir::types::ValueType) -> String {
    match value_type {
        sef_model_ir::types::ValueType::Decimal(decimal) => {
            format!("DECIMAL({},{})", decimal.precision(), decimal.scale())
        }
        other => format!("{other:?}"),
    }
}

/// The dimensions a block groups or filters by, in canonical order.
fn usages(block: &SemanticQueryBlock, requested: &[RequestedAtom]) -> Vec<QualifiedDimensionRef> {
    let mut found: BTreeSet<QualifiedDimensionRef> = requested
        .iter()
        .map(|atom| match atom {
            RequestedAtom::Dimension(dimension) => dimension.clone(),
            RequestedAtom::TimeBucket(time) => time.dimension.clone(),
        })
        .collect();
    if let Some(filter) = &block.filter {
        collect(filter, &mut found);
    }
    found.into_iter().collect()
}

fn collect(filter: &FilterExpr, found: &mut BTreeSet<QualifiedDimensionRef>) {
    match filter {
        FilterExpr::Compare { target, .. }
        | FilterExpr::Between { target, .. }
        | FilterExpr::In { target, .. }
        | FilterExpr::IsNull(target)
        | FilterExpr::IsNotNull(target) => {
            if let FilterTarget::Dimension(dimension) = target {
                found.insert(dimension.clone());
            }
        }
        FilterExpr::And(operands) | FilterExpr::Or(operands) => {
            for operand in operands {
                collect(operand, found);
            }
        }
        FilterExpr::Not(operand) => collect(operand, found),
    }
}
