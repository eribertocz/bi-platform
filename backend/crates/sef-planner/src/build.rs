//! The canonical planning pipeline (§14): one deterministic baseline plan
//! per validated query block, built from the correctness engine's facts and
//! obligations without re-deciding validity. The certificate is computed
//! afterwards by an independent checker ([`crate::verify`]), never from
//! what this builder believes it did.

use std::collections::{BTreeMap, BTreeSet};

use sef_correctness::{BlockValidation, Fact, Obligation, ValidatedQuery};
use sef_exec_ir::{Contract, NodeId, ObjectRef, Operator, PlanBlock, SeirGraph, SemanticPlan};
use sef_model_ir::diagnostic::{Diagnostic, ErrorCode};
use sef_model_ir::expressions::{AggregateFunction, ScalarExpr};
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{
    CompiledSemanticModel, Measure, Metric, ModelDefinition, TemporalSelection, TemporalTies,
};
use sef_model_ir::policy::PolicyScope;
use sef_model_ir::relationships::ContractEvidenceState;
use sef_query_ir::query::{
    FilterExpr, FilterTarget, QualifiedDimensionRef, RequestedAtom, SelectionItem,
    SemanticQueryBlock,
};

use crate::certificate::Certificate;
use crate::verify::{certify, hop_fields, metric_inputs, owned, planning_error, weight_fields};

/// A certified semantic plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedQuery {
    pub plan: SemanticPlan,
    pub certificate: Certificate,
}

/// Builds the canonical baseline plan of a validated query and certifies
/// it.
///
/// # Errors
/// Planner capability diagnostics (phase `Planning`): an obligation the
/// plan does not discharge, or a broken internal invariant. Never a
/// different semantic interpretation of the query (§25).
pub fn plan(
    model: &CompiledSemanticModel,
    validated: &ValidatedQuery,
) -> Result<PlannedQuery, Vec<Diagnostic>> {
    let mut graph = SeirGraph::new();
    let mut blocks = Vec::new();
    let mut errors = Vec::new();
    for (b, (block, validation)) in validated
        .query
        .blocks
        .iter()
        .zip(&validated.blocks)
        .enumerate()
    {
        let mut builder = Builder {
            compiled: model,
            model: model.definition(),
            graph: &mut graph,
            validation,
            path: format!("/blocks/{b}"),
            errors: Vec::new(),
        };
        match builder.block(block) {
            Some(root) => blocks.push(PlanBlock {
                block_id: block.block_id.clone(),
                root,
            }),
            // Every early return names its cause; this one never stays silent.
            None if builder.errors.is_empty() => builder.fail(
                ErrorCode::PlannerInternalInvariantViolation,
                "El plan del bloque quedó incompleto: el modelo no tiene un objeto que la                  validación dio por existente."
                    .to_owned(),
            ),
            None => {}
        }
        errors.extend(builder.errors);
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    let plan = SemanticPlan { graph, blocks };
    let certificate = certify(model, validated, &plan)?;
    Ok(PlannedQuery { plan, certificate })
}

/// The metrics one fact branch computes: its measures, the metrics they
/// serve, and the metric whose private semantics (period-end selection,
/// intrinsic filter, metric policy) shape the branch, if any.
#[derive(Default)]
struct Branch<'m> {
    measures: BTreeMap<StableId, &'m Measure>,
    serves: BTreeSet<StableId>,
}

struct Builder<'p, 'm> {
    compiled: &'m CompiledSemanticModel,
    model: &'m ModelDefinition,
    graph: &'p mut SeirGraph,
    validation: &'p BlockValidation,
    path: String,
    errors: Vec<Diagnostic>,
}

impl<'p, 'm> Builder<'p, 'm> {
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

    /// A model object with its content fingerprint.
    fn object(&mut self, id: &StableId) -> Option<ObjectRef> {
        let found = self.compiled.object_hash(id).map(|content| ObjectRef {
            id: id.clone(),
            content: *content,
        });
        if found.is_none() {
            self.fail(
                ErrorCode::PlannerInternalInvariantViolation,
                format!("{id} no tiene huella en el modelo compilado."),
            );
        }
        found
    }

    fn objects(&mut self, ids: impl IntoIterator<Item = StableId>) -> Option<Vec<ObjectRef>> {
        ids.into_iter().map(|id| self.object(&id)).collect()
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

    /// Every metric of the block's lineage.
    fn closure(&self, selected: &[&'m Metric]) -> BTreeMap<StableId, &'m Metric> {
        let mut found: BTreeMap<StableId, &'m Metric> = BTreeMap::new();
        let mut pending: Vec<&'m Metric> = selected.to_vec();
        while let Some(metric) = pending.pop() {
            if found.insert(metric.metadata.id().clone(), metric).is_none() {
                pending.extend(
                    metric_inputs(metric)
                        .iter()
                        .filter_map(|id| self.metric(id)),
                );
            }
        }
        found
    }

    /// A metric gets a private branch when part of its meaning applies to
    /// its rows only: period-end selection, an intrinsic filter, or a
    /// policy scoped to it.
    fn private(&self, metric: &Metric) -> bool {
        let id = metric.metadata.id();
        metric.intrinsic_filter.is_some()
            || self.obligations().any(|obligation| match obligation {
                Obligation::ApplyPeriodEndSelection { metric: owner, .. } => owner == id,
                Obligation::ApplyMandatoryPolicy { scope, .. } => scope == id,
                _ => false,
            })
    }

    // ---- The block ---------------------------------------------------------------

    fn block(&mut self, block: &SemanticQueryBlock) -> Option<NodeId> {
        let requested = block.requested_grain();
        let selected: Vec<&'m Metric> = block
            .selections
            .iter()
            .filter_map(|selection| match &selection.item {
                SelectionItem::Metric(id) => self.metric(id),
                _ => None,
            })
            .collect();
        let closure = self.closure(&selected);
        // Phase 2: metric requirements grouped by fact and private meaning.
        let mut branches: BTreeMap<(StableId, Option<StableId>), Branch<'m>> = BTreeMap::new();
        for metric in closure.values() {
            let profile = self.private(metric).then(|| metric.metadata.id().clone());
            for id in metric.expression.measure_refs() {
                let Some(measure) = self.model.measures.iter().find(|m| m.metadata.id() == &id)
                else {
                    continue;
                };
                let branch = branches
                    .entry((measure.dataset.clone(), profile.clone()))
                    .or_default();
                branch.measures.insert(id, measure);
                branch.serves.insert(metric.metadata.id().clone());
            }
        }
        if branches.is_empty() {
            // SQIR binding rejects a block without metrics (`NoMetric`).
            self.fail(
                ErrorCode::PlannerInternalInvariantViolation,
                "Un bloque sin métricas no debería llegar al planificador: SQIR lo rechaza al \
                 vincular."
                    .to_owned(),
            );
            return None;
        }
        let usages = usages(block, &requested);
        // Phases 3–9: one protected, aggregated branch per fact and meaning.
        let mut outputs = Vec::new();
        for ((anchor, profile), branch) in &branches {
            let private = profile.as_ref().and_then(|id| self.metric(id));
            outputs.push(self.branch(anchor, private, branch, &usages, block, &requested)?);
        }
        // Phase 10: alignment of independent branches.
        let mut node = self.align(outputs, &requested)?;
        // Phases 11–12: derived metrics in dependency order.
        node = self.derived(node, &closure)?;
        // Phase 13: post-aggregate filter and result shape.
        if let Some(predicate) = &block.post_aggregate_filter {
            node = self.add(Operator::ApplyPostAggregateFilter {
                input: node,
                predicate: predicate.clone(),
            })?;
        }
        let mut definitions: Vec<StableId> = selected
            .iter()
            .map(|metric| metric.metadata.id().clone())
            .collect();
        definitions.extend(usages.iter().map(|usage| usage.dimension.clone()));
        let definitions = self.objects(definitions)?;
        node = self.add(Operator::ProjectSemanticOutput {
            input: node,
            selections: block.selections.clone(),
            definitions,
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
        Some(node)
    }

    fn align(&mut self, outputs: Vec<NodeId>, requested: &[RequestedAtom]) -> Option<NodeId> {
        if let [single] = outputs.as_slice() {
            return Some(*single);
        }
        self.add(Operator::AlignSemanticBranches {
            inputs: outputs,
            grain: requested.to_vec(),
        })
    }

    /// One fact branch, in semantic order: read, guard, relate, bucket,
    /// filter, select, aggregate.
    fn branch(
        &mut self,
        anchor: &StableId,
        private: Option<&'m Metric>,
        branch: &Branch<'m>,
        shared: &[QualifiedDimensionRef],
        block: &SemanticQueryBlock,
        requested: &[RequestedAtom],
    ) -> Option<NodeId> {
        let mut usages = shared.to_vec();
        let mut filters: Vec<&ScalarExpr> = branch
            .measures
            .values()
            .filter_map(|measure| measure.intrinsic_filter.as_ref())
            .collect();
        filters.extend(private.and_then(|metric| metric.intrinsic_filter.as_ref()));
        for filter in &filters {
            for dimension in filter.dimension_refs() {
                let usage = QualifiedDimensionRef {
                    dimension,
                    roles: Vec::new(),
                };
                if !usages.contains(&usage) {
                    usages.push(usage);
                }
            }
        }
        let fields = self.read_fields(anchor, private, branch, &usages, &filters);
        let dataset = self.object(anchor)?;
        let mut node = self.add(Operator::ReadSemanticDataset { dataset, fields })?;
        node = self.guard(node, anchor)?;
        if let Some(metric) = private {
            node = self.guard(node, metric.metadata.id())?;
        }
        for usage in &usages {
            node = self.relate(node, anchor, usage, branch)?;
        }
        node = self.time(node, anchor, &usages)?;
        if let Some(predicate) = &block.filter {
            node = self.add(Operator::ApplyRowFilter {
                input: node,
                predicate: predicate.clone(),
            })?;
        }
        if let Some(metric) = private {
            if metric.intrinsic_filter.is_some() {
                let owner = self.object(metric.metadata.id())?;
                node = self.add(Operator::ApplyIntrinsicFilter { input: node, owner })?;
            }
            node = self.period_end(node, metric)?;
        }
        self.aggregate(node, branch, requested)
    }

    /// Every field the branch's source must provide: measures and their
    /// filters, policy predicates, the keys and bindings of its paths and
    /// dimensions, and the time, scope and tie fields of a period-end
    /// selection (§7.1, §33.5).
    fn read_fields(
        &self,
        anchor: &StableId,
        private: Option<&'m Metric>,
        branch: &Branch<'m>,
        usages: &[QualifiedDimensionRef],
        filters: &[&ScalarExpr],
    ) -> Vec<StableId> {
        let Some(dataset) = self
            .model
            .datasets
            .iter()
            .find(|d| d.metadata.id() == anchor)
        else {
            return Vec::new();
        };
        let own: BTreeSet<&StableId> = dataset.fields.iter().map(|f| f.metadata.id()).collect();
        let mut wanted: Vec<StableId> = Vec::new();
        for measure in branch.measures.values() {
            wanted.extend(
                measure
                    .scalar_expression
                    .iter()
                    .flat_map(ScalarExpr::field_refs),
            );
        }
        for filter in filters {
            wanted.extend(filter.field_refs());
        }
        let scopes: Vec<&StableId> = std::iter::once(anchor)
            .chain(private.map(|metric| metric.metadata.id()))
            .collect();
        for policy in &self.model.policies {
            let applies = match &policy.scope {
                PolicyScope::Dataset(scope) | PolicyScope::Metric(scope) => scopes.contains(&scope),
            };
            if applies {
                wanted.extend(policy.predicate.field_refs());
            }
        }
        for usage in usages {
            match self.path(anchor, usage).as_deref() {
                Some([]) => wanted.extend(self.binding_fields(&usage.dimension, anchor)),
                Some([first, ..]) => wanted.extend(self.key_fields(first, anchor)),
                None => {}
            }
        }
        if let Some(behavior) = private.and_then(|metric| metric.time_behavior.as_ref()) {
            let mut terms: Vec<&StableId> = behavior.order_by.iter().map(|o| &o.value).collect();
            match &behavior.ties {
                TemporalTies::SecondaryKey { fields } => {
                    terms.extend(fields.iter().map(|o| &o.value));
                }
                TemporalTies::UniqueByContract { key } => terms.push(key),
                TemporalTies::Unspecified => {}
            }
            if let sef_model_ir::grain::GrainAtom::DimensionRef { dimension, .. } = &behavior.axis {
                terms.push(dimension);
            }
            for atom in behavior.selection_scope.atoms() {
                if let sef_model_ir::grain::GrainAtom::EntityKeyRef(key) = atom {
                    terms.push(key);
                }
            }
            for term in terms {
                wanted.push(term.clone());
                wanted.extend(self.binding_fields(term, anchor));
                for key in &dataset.key_bindings {
                    if &key.entity_key == term {
                        wanted.extend(key.fields.iter().cloned());
                    }
                }
            }
        }
        wanted.retain(|field| own.contains(field));
        wanted
    }

    fn binding_fields(&self, dimension: &StableId, dataset: &StableId) -> Vec<StableId> {
        self.model
            .dimensions
            .iter()
            .find(|candidate| candidate.metadata.id() == dimension)
            .into_iter()
            .flat_map(|found| &found.bindings)
            .filter(|binding| &binding.dataset == dataset)
            .flat_map(|binding| binding.expression.field_refs())
            .collect()
    }

    /// The fields a hop through `relationship` reads on `dataset`.
    fn key_fields(&self, relationship: &StableId, dataset: &StableId) -> Vec<StableId> {
        owned(self.model, dataset, hop_fields(self.model, relationship))
    }

    /// The path the correctness engine proved from `anchor` to a dimension.
    fn path(&self, anchor: &StableId, dimension: &QualifiedDimensionRef) -> Option<Vec<StableId>> {
        self.validation.facts.iter().find_map(|fact| match fact {
            Fact::RelationshipPathUnique {
                anchor: found,
                dimension: proved,
                relationships,
            } if found == anchor && proved == dimension => Some(relationships.clone()),
            _ => None,
        })
    }

    /// Bridges a metric of this branch allocates through.
    fn allocations(&self, branch: &Branch<'m>) -> Vec<StableId> {
        self.obligations()
            .filter_map(|obligation| match obligation {
                Obligation::ApplyBridgeAllocation { metric, bridge }
                    if branch.serves.contains(metric) =>
                {
                    Some(bridge.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Mandatory policies scoped to `scope`, as guards right after `node`.
    fn guard(&mut self, mut node: NodeId, scope: &StableId) -> Option<NodeId> {
        let policies: Vec<StableId> = self
            .obligations()
            .filter_map(|obligation| match obligation {
                Obligation::ApplyMandatoryPolicy {
                    policy,
                    scope: owner,
                } if owner == scope => Some(policy.clone()),
                _ => None,
            })
            .collect();
        for id in policies {
            let slots = self
                .model
                .policies
                .iter()
                .find(|candidate| candidate.metadata.id() == &id)
                .map(|found| {
                    found
                        .context_slots
                        .iter()
                        .map(|slot| slot.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            let policy = self.object(&id)?;
            node = self.add(Operator::PolicyGuard {
                input: node,
                policy,
                slots,
            })?;
        }
        Some(node)
    }

    /// Attaches a dimension through the proved path, with temporal,
    /// allocation and uniqueness semantics on the way; each dataset the path
    /// enters is read and guarded by its own lookup.
    fn relate(
        &mut self,
        mut node: NodeId,
        anchor: &StableId,
        dimension: &QualifiedDimensionRef,
        branch: &Branch<'m>,
    ) -> Option<NodeId> {
        let Some(path) = self.path(anchor, dimension) else {
            self.fail(
                ErrorCode::PlannerInternalInvariantViolation,
                format!(
                    "El motor de corrección no probó un camino de {anchor} a {}.",
                    dimension.dimension
                ),
            );
            return None;
        };
        let mut lookups = Vec::new();
        let mut at = anchor.clone();
        for (i, id) in path.iter().enumerate() {
            node = self.traverse(node, id, branch)?;
            let relationship = self
                .model
                .relationships
                .iter()
                .find(|candidate| candidate.metadata.id() == id)?;
            at = if relationship.from_dataset == at {
                relationship.to_dataset.clone()
            } else {
                relationship.from_dataset.clone()
            };
            let mut fields = self.key_fields(id, &at);
            match path.get(i + 1) {
                Some(next) => fields.extend(self.key_fields(next, &at)),
                None => fields.extend(self.binding_fields(&dimension.dimension, &at)),
            }
            for bridge in self.allocations(branch) {
                let enters = self
                    .model
                    .bridges
                    .iter()
                    .any(|b| b.metadata.id() == &bridge && b.bridge_dataset == at);
                if enters {
                    fields.extend(owned(self.model, &at, weight_fields(self.model, &bridge)));
                }
            }
            let dataset = self.object(&at)?;
            let read = self.add(Operator::ReadSemanticDataset { dataset, fields })?;
            lookups.push(self.guard(read, &at.clone())?);
        }
        let definition = self.object(&dimension.dimension)?;
        let path = self.objects(path)?;
        self.add(Operator::AttachDimension {
            input: node,
            lookups,
            dimension: dimension.clone(),
            definition,
            path,
        })
    }

    fn traverse(&mut self, mut node: NodeId, id: &StableId, branch: &Branch<'m>) -> Option<NodeId> {
        let relationship = self
            .model
            .relationships
            .iter()
            .find(|relationship| relationship.metadata.id() == id)?;
        let reference = self.object(id)?;
        if relationship.temporal_constraint.is_some() {
            node = self.add(Operator::TemporalAsOf {
                input: node,
                relationship: reference.clone(),
            })?;
            node = self.add(Operator::ContractGuard {
                input: node,
                contract: Contract::TemporalSingleMatch {
                    relationship: reference,
                },
            })?;
        } else if relationship.evidence == ContractEvidenceState::DeclaredUnverified {
            // A declared, not yet certified, unique target (§27: CASE-003).
            node = self.add(Operator::ContractGuard {
                input: node,
                contract: Contract::UniqueTarget {
                    relationship: reference,
                },
            })?;
        }
        // Allocation happens where the path enters a bridge that a metric
        // of this branch allocates through.
        for bridge_id in self.allocations(branch) {
            let enters = self.model.bridges.iter().any(|bridge| {
                bridge.metadata.id() == &bridge_id && &bridge.left_relationship == id
            });
            if enters {
                let bridge = self.object(&bridge_id)?;
                node = self.add(Operator::BridgeAllocate {
                    input: node,
                    bridge: bridge.clone(),
                })?;
                node = self.add(Operator::ContractGuard {
                    input: node,
                    contract: Contract::AllocationNormalization { bridge },
                })?;
            }
        }
        Some(node)
    }

    /// Instant → business zone → calendar bucket, and calendar buckets of
    /// dates, where the dimension's values arrive on this branch.
    fn time(
        &mut self,
        mut node: NodeId,
        anchor: &StableId,
        usages: &[QualifiedDimensionRef],
    ) -> Option<NodeId> {
        for usage in usages {
            let Some(path) = self.path(anchor, usage) else {
                continue;
            };
            let arrival = self.arrival(anchor, &path);
            let wanted: Vec<Obligation> = self
                .obligations()
                .filter(|obligation| match obligation {
                    Obligation::ConvertTimezoneBeforeBucketing {
                        dimension, dataset, ..
                    }
                    | Obligation::ApplyCalendarBucket {
                        dimension, dataset, ..
                    } => dimension == &usage.dimension && dataset == &arrival,
                    _ => false,
                })
                .cloned()
                .collect();
            for obligation in wanted {
                match obligation {
                    Obligation::ConvertTimezoneBeforeBucketing {
                        dimension,
                        timezone,
                        calendar,
                        grain,
                        ..
                    } => {
                        let reference = self.object(&dimension)?;
                        let normalized = self.add(Operator::NormalizeTime {
                            input: node,
                            dimension: reference.clone(),
                            timezone,
                            calendar: calendar.clone(),
                        })?;
                        node = self.add(Operator::TimeBucket {
                            input: normalized,
                            dimension: reference,
                            grain,
                            calendar,
                        })?;
                    }
                    Obligation::ApplyCalendarBucket {
                        dimension,
                        calendar,
                        grain,
                        ..
                    } => {
                        let reference = self.object(&dimension)?;
                        node = self.add(Operator::TimeBucket {
                            input: node,
                            dimension: reference,
                            grain,
                            calendar,
                        })?;
                    }
                    _ => {}
                }
            }
        }
        Some(node)
    }

    fn arrival(&self, anchor: &StableId, path: &[StableId]) -> StableId {
        let mut at = anchor.clone();
        for id in path {
            if let Some(relationship) = self
                .model
                .relationships
                .iter()
                .find(|r| r.metadata.id() == id)
            {
                at = if relationship.from_dataset == at {
                    relationship.to_dataset.clone()
                } else {
                    relationship.from_dataset.clone()
                };
            }
        }
        at
    }

    /// Semi-additive selection on the metric's own branch, before any
    /// aggregation across the other axes (§18).
    fn period_end(&mut self, mut node: NodeId, metric: &'m Metric) -> Option<NodeId> {
        let selections: Vec<Obligation> = self
            .obligations()
            .filter(|obligation| {
                matches!(obligation, Obligation::ApplyPeriodEndSelection { metric: owner, .. }
                    if owner == metric.metadata.id())
            })
            .cloned()
            .collect();
        for obligation in selections {
            let Obligation::ApplyPeriodEndSelection {
                axis,
                rule,
                period,
                scope,
                ..
            } = obligation
            else {
                continue;
            };
            let rule = match rule {
                "period-start" => TemporalSelection::PeriodStart,
                _ => TemporalSelection::PeriodEnd,
            };
            let reference = self.object(metric.metadata.id())?;
            node = self.add(Operator::SelectPeriodEnd {
                input: node,
                metric: reference,
                axis,
                rule,
                period,
                scope,
            })?;
        }
        Some(node)
    }

    /// Distinct measures are computed apart and never summed (§19); the
    /// others aggregate together at the requested grain.
    fn aggregate(
        &mut self,
        node: NodeId,
        branch: &Branch<'m>,
        requested: &[RequestedAtom],
    ) -> Option<NodeId> {
        let (distinct, additive): (Vec<&'m Measure>, Vec<&'m Measure>) = branch
            .measures
            .values()
            .copied()
            .partition(|measure| measure.aggregation == AggregateFunction::CountDistinct);
        let mut outputs = Vec::new();
        if !additive.is_empty() {
            let measures = self.objects(additive.iter().map(|m| m.metadata.id().clone()))?;
            let serves = self.served(branch, &additive)?;
            outputs.push(self.add(Operator::AggregateAtGrain {
                input: node,
                grain: requested.to_vec(),
                measures,
                serves,
            })?);
        }
        for measure in distinct {
            let reference = self.object(measure.metadata.id())?;
            let serves = self.served(branch, &[measure])?;
            outputs.push(self.add(Operator::DistinctAggregate {
                input: node,
                grain: requested.to_vec(),
                measure: reference,
                serves,
            })?);
        }
        self.align(outputs, requested)
    }

    /// The metrics of the branch that read any of `measures` directly.
    fn served(&mut self, branch: &Branch<'m>, measures: &[&'m Measure]) -> Option<Vec<ObjectRef>> {
        let ids: BTreeSet<&StableId> = measures.iter().map(|m| m.metadata.id()).collect();
        let metrics: Vec<StableId> = branch
            .serves
            .iter()
            .filter(|id| {
                self.metric(id).is_some_and(|metric| {
                    metric
                        .expression
                        .measure_refs()
                        .iter()
                        .any(|m| ids.contains(m))
                })
            })
            .cloned()
            .collect();
        self.objects(metrics)
    }

    /// Metrics computed from other metrics — recompositions, windows and
    /// plain references — once everything they read exists, whatever their
    /// kind (§10.4, §12.1).
    fn derived(
        &mut self,
        mut node: NodeId,
        closure: &BTreeMap<StableId, &'m Metric>,
    ) -> Option<NodeId> {
        let mut pending: Vec<(StableId, Option<Obligation>)> = Vec::new();
        for (id, metric) in closure {
            let window = self.obligations().find(|obligation| {
                matches!(obligation, Obligation::ApplyOrderedCumulative { metric: m, .. } if m == id)
            });
            let recomposed = self.obligations().any(|obligation| {
                matches!(obligation, Obligation::RecomputeAtTargetGrain { metric: m, .. } if m == id)
            });
            if window.is_some() || recomposed || metric.expression.measure_refs().is_empty() {
                pending.push((id.clone(), window.cloned()));
            }
        }
        let mut done: BTreeSet<StableId> = BTreeSet::new();
        while !pending.is_empty() {
            let waiting: BTreeSet<&StableId> = pending.iter().map(|(id, _)| id).collect();
            let ready = pending.iter().position(|(id, _)| {
                closure.get(id).is_some_and(|metric| {
                    metric_inputs(metric)
                        .iter()
                        .all(|input| !waiting.contains(input) || done.contains(input))
                })
            });
            let Some(index) = ready else {
                self.fail(
                    ErrorCode::PlannerIncomplete,
                    "Las métricas derivadas dependen unas de otras en ciclo.".to_owned(),
                );
                return None;
            };
            let (id, window) = pending.remove(index);
            let metric = self.object(&id)?;
            node = match window {
                Some(Obligation::ApplyOrderedCumulative {
                    base,
                    order_by,
                    partition_by,
                    ..
                }) => self.add(Operator::ComputeWindowMetric {
                    input: node,
                    metric,
                    base,
                    order_by,
                    partition_by,
                })?,
                _ => self.add(Operator::RecomposeMetric {
                    input: node,
                    metric,
                })?,
            };
            done.insert(id);
        }
        Some(node)
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
