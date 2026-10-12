//! Canonical SEIR operators (`04_CORRECTNESS_AND_PLANNING.md` Part II §6–§13):
//! each one is a semantic operation that must survive translation to any
//! engine. None of them carries SQL, a join algorithm, statistics, a cache
//! key or a materialization.

use sef_model_ir::canonical::{Canonical, CanonicalValue as C};
use sef_model_ir::grain::Grain;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::TemporalSelection;
use sef_model_ir::time::{CalendarProfile, IanaZoneId, TimeGrain};
use sef_query_ir::query::{FilterExpr, OrderTerm, QualifiedDimensionRef, RequestedAtom, Selection};

use crate::graph::NodeId;

/// A data contract whose truth the plan relies on (§7.3). How the evidence
/// is produced (certificate or runtime check) is decided later.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Contract {
    /// The "one" side of a relationship is unique on its join key.
    UniqueTarget { relationship: StableId },
    /// An as-of relationship matches exactly one version per event.
    TemporalSingleMatch { relationship: StableId },
    /// Bridge weights are complete and sum to the declared total.
    AllocationNormalization { bridge: StableId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operator {
    // ---- Source and governance (§7) ----
    /// The semantic fields of a dataset the plan needs, never table syntax.
    ReadSemanticDataset {
        dataset: StableId,
        fields: Vec<StableId>,
    },
    /// Mandatory row policy; it must dominate everything derived from the
    /// rows it protects. Slot *names* only: values are runtime context.
    PolicyGuard {
        input: NodeId,
        policy: StableId,
        slots: Vec<String>,
    },
    ContractGuard {
        input: NodeId,
        contract: Contract,
    },
    /// The caller's pre-aggregate predicate, at row phase.
    ApplyRowFilter {
        input: NodeId,
        predicate: FilterExpr,
    },
    // ---- Dimensions and relationships (§8) ----
    /// Attaches a dimension through the validated relationship path.
    AttachDimension {
        input: NodeId,
        dimension: QualifiedDimensionRef,
        path: Vec<StableId>,
    },
    TemporalAsOf {
        input: NodeId,
        relationship: StableId,
    },
    BridgeAllocate {
        input: NodeId,
        bridge: StableId,
    },
    // ---- Time (§9) ----
    NormalizeTime {
        input: NodeId,
        dimension: StableId,
        timezone: IanaZoneId,
        calendar: CalendarProfile,
    },
    TimeBucket {
        input: NodeId,
        dimension: StableId,
        grain: TimeGrain,
        calendar: CalendarProfile,
    },
    SelectPeriodEnd {
        input: NodeId,
        metric: StableId,
        axis: StableId,
        rule: TemporalSelection,
        period: Option<TimeGrain>,
        scope: Grain,
    },
    // ---- Metrics and aggregation (§10) ----
    AggregateAtGrain {
        input: NodeId,
        grain: Vec<RequestedAtom>,
        measures: Vec<StableId>,
    },
    /// Exact distinct count at the grain; never summed afterwards.
    DistinctAggregate {
        input: NodeId,
        grain: Vec<RequestedAtom>,
        measure: StableId,
    },
    /// Builds a metric from components already at the target grain.
    RecomposeMetric {
        input: NodeId,
        metric: StableId,
    },
    // ---- Multi-fact alignment (§11) ----
    AlignSemanticBranches {
        inputs: Vec<NodeId>,
        grain: Vec<RequestedAtom>,
    },
    // ---- Window and post-aggregate (§12) ----
    ComputeWindowMetric {
        input: NodeId,
        metric: StableId,
        base: StableId,
        order_by: Vec<StableId>,
        partition_by: Grain,
    },
    ApplyPostAggregateFilter {
        input: NodeId,
        predicate: FilterExpr,
    },
    // ---- Result shape (§13) ----
    ProjectSemanticOutput {
        input: NodeId,
        selections: Vec<Selection>,
    },
    OrderSemanticOutput {
        input: NodeId,
        order: Vec<OrderTerm>,
    },
    LimitSemanticOutput {
        input: NodeId,
        limit: Option<u64>,
        offset: Option<u64>,
    },
}

impl Operator {
    /// The operator's name, as in the specification.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ReadSemanticDataset { .. } => "ReadSemanticDataset",
            Self::PolicyGuard { .. } => "PolicyGuard",
            Self::ContractGuard { .. } => "ContractGuard",
            Self::ApplyRowFilter { .. } => "ApplyRowFilter",
            Self::AttachDimension { .. } => "AttachDimension",
            Self::TemporalAsOf { .. } => "TemporalAsOf",
            Self::BridgeAllocate { .. } => "BridgeAllocate",
            Self::NormalizeTime { .. } => "NormalizeTime",
            Self::TimeBucket { .. } => "TimeBucket",
            Self::SelectPeriodEnd { .. } => "SelectPeriodEnd",
            Self::AggregateAtGrain { .. } => "AggregateAtGrain",
            Self::DistinctAggregate { .. } => "DistinctAggregate",
            Self::RecomposeMetric { .. } => "RecomposeMetric",
            Self::AlignSemanticBranches { .. } => "AlignSemanticBranches",
            Self::ComputeWindowMetric { .. } => "ComputeWindowMetric",
            Self::ApplyPostAggregateFilter { .. } => "ApplyPostAggregateFilter",
            Self::ProjectSemanticOutput { .. } => "ProjectSemanticOutput",
            Self::OrderSemanticOutput { .. } => "OrderSemanticOutput",
            Self::LimitSemanticOutput { .. } => "LimitSemanticOutput",
        }
    }

    /// Puts set-valued parameters (inputs of an alignment, grains, measures,
    /// fields) in canonical order, so a stored operator is exactly what its
    /// identity encodes, whatever order it was written in.
    pub(crate) fn normalize(&mut self) {
        fn canonical<T: Ord>(items: &mut Vec<T>) {
            items.sort();
            items.dedup();
        }
        match self {
            Self::ReadSemanticDataset { fields, .. } => canonical(fields),
            Self::AggregateAtGrain {
                grain, measures, ..
            } => {
                canonical(grain);
                canonical(measures);
            }
            Self::DistinctAggregate { grain, .. } => canonical(grain),
            Self::AlignSemanticBranches { inputs, grain } => {
                canonical(inputs);
                canonical(grain);
            }
            _ => {}
        }
    }

    /// The nodes this operator reads, in their semantic order.
    #[must_use]
    pub fn inputs(&self) -> Vec<NodeId> {
        match self {
            Self::ReadSemanticDataset { .. } => Vec::new(),
            Self::AlignSemanticBranches { inputs, .. } => inputs.clone(),
            Self::PolicyGuard { input, .. }
            | Self::ContractGuard { input, .. }
            | Self::ApplyRowFilter { input, .. }
            | Self::AttachDimension { input, .. }
            | Self::TemporalAsOf { input, .. }
            | Self::BridgeAllocate { input, .. }
            | Self::NormalizeTime { input, .. }
            | Self::TimeBucket { input, .. }
            | Self::SelectPeriodEnd { input, .. }
            | Self::AggregateAtGrain { input, .. }
            | Self::DistinctAggregate { input, .. }
            | Self::RecomposeMetric { input, .. }
            | Self::ComputeWindowMetric { input, .. }
            | Self::ApplyPostAggregateFilter { input, .. }
            | Self::ProjectSemanticOutput { input, .. }
            | Self::OrderSemanticOutput { input, .. }
            | Self::LimitSemanticOutput { input, .. } => vec![*input],
        }
    }
}

fn ids(items: &[StableId]) -> C {
    C::sequence(items)
}

fn texts(items: &[String]) -> C {
    C::Sequence(items.iter().map(|item| C::text(item.clone())).collect())
}

impl Canonical for Contract {
    fn canonical_value(&self) -> C {
        match self {
            Self::UniqueTarget { relationship } => C::record(
                "contract.unique-target",
                [("relationship", relationship.canonical_value())],
            ),
            Self::TemporalSingleMatch { relationship } => C::record(
                "contract.temporal-single-match",
                [("relationship", relationship.canonical_value())],
            ),
            Self::AllocationNormalization { bridge } => C::record(
                "contract.allocation-normalization",
                [("bridge", bridge.canonical_value())],
            ),
        }
    }
}

impl Canonical for Operator {
    #[allow(clippy::too_many_lines)] // one arm per operator of the specification
    fn canonical_value(&self) -> C {
        let tag = format!("seir.{}", self.name());
        let fields: Vec<(&str, C)> = match self {
            Self::ReadSemanticDataset { dataset, fields } => vec![
                ("dataset", dataset.canonical_value()),
                ("fields", C::set(fields)),
            ],
            Self::PolicyGuard {
                input,
                policy,
                slots,
            } => vec![
                ("input", input.canonical_value()),
                ("policy", policy.canonical_value()),
                ("slots", texts(slots)),
            ],
            Self::ContractGuard { input, contract } => vec![
                ("input", input.canonical_value()),
                ("contract", contract.canonical_value()),
            ],
            Self::ApplyRowFilter { input, predicate }
            | Self::ApplyPostAggregateFilter { input, predicate } => vec![
                ("input", input.canonical_value()),
                ("predicate", predicate.canonical_value()),
            ],
            Self::AttachDimension {
                input,
                dimension,
                path,
            } => vec![
                ("input", input.canonical_value()),
                ("dimension", dimension.canonical_value()),
                ("path", ids(path)),
            ],
            Self::TemporalAsOf {
                input,
                relationship,
            } => vec![
                ("input", input.canonical_value()),
                ("relationship", relationship.canonical_value()),
            ],
            Self::BridgeAllocate { input, bridge } => vec![
                ("input", input.canonical_value()),
                ("bridge", bridge.canonical_value()),
            ],
            Self::NormalizeTime {
                input,
                dimension,
                timezone,
                calendar,
            } => vec![
                ("input", input.canonical_value()),
                ("dimension", dimension.canonical_value()),
                ("timezone", timezone.canonical_value()),
                ("calendar", calendar.canonical_value()),
            ],
            Self::TimeBucket {
                input,
                dimension,
                grain,
                calendar,
            } => vec![
                ("input", input.canonical_value()),
                ("dimension", dimension.canonical_value()),
                ("grain", grain.canonical_value()),
                ("calendar", calendar.canonical_value()),
            ],
            Self::SelectPeriodEnd {
                input,
                metric,
                axis,
                rule,
                period,
                scope,
            } => vec![
                ("input", input.canonical_value()),
                ("metric", metric.canonical_value()),
                ("axis", axis.canonical_value()),
                ("rule", rule.canonical_value()),
                ("period", C::optional(period.as_ref())),
                ("scope", scope.canonical_value()),
            ],
            Self::AggregateAtGrain {
                input,
                grain,
                measures,
            } => vec![
                ("input", input.canonical_value()),
                ("grain", C::set(grain)),
                ("measures", C::set(measures)),
            ],
            Self::DistinctAggregate {
                input,
                grain,
                measure,
            } => vec![
                ("input", input.canonical_value()),
                ("grain", C::set(grain)),
                ("measure", measure.canonical_value()),
            ],
            Self::RecomposeMetric { input, metric } => vec![
                ("input", input.canonical_value()),
                ("metric", metric.canonical_value()),
            ],
            Self::AlignSemanticBranches { inputs, grain } => vec![
                // Alignment is symmetric: the branch order is not meaning.
                ("inputs", C::set(inputs)),
                ("grain", C::set(grain)),
            ],
            Self::ComputeWindowMetric {
                input,
                metric,
                base,
                order_by,
                partition_by,
            } => vec![
                ("input", input.canonical_value()),
                ("metric", metric.canonical_value()),
                ("base", base.canonical_value()),
                ("order_by", ids(order_by)),
                ("partition_by", partition_by.canonical_value()),
            ],
            Self::ProjectSemanticOutput { input, selections } => vec![
                ("input", input.canonical_value()),
                // Output order is the result's shape.
                ("selections", C::sequence(selections)),
            ],
            Self::OrderSemanticOutput { input, order } => vec![
                ("input", input.canonical_value()),
                ("order", C::sequence(order)),
            ],
            Self::LimitSemanticOutput {
                input,
                limit,
                offset,
            } => vec![
                ("input", input.canonical_value()),
                ("limit", C::optional(limit.as_ref())),
                ("offset", C::optional(offset.as_ref())),
            ],
        };
        C::record(&tag, fields)
    }
}
