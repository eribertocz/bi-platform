//! Canonical SEIR operators (`04_CORRECTNESS_AND_PLANNING.md` Part II §6–§13):
//! each one is a semantic operation that must survive translation to any
//! engine. None of them carries SQL, a join algorithm, statistics, a cache
//! key or a materialization.

use std::fmt;

use sef_model_ir::canonical::{Canonical, CanonicalValue as C, Fingerprint};
use sef_model_ir::grain::Grain;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::TemporalSelection;
use sef_model_ir::time::{CalendarProfile, IanaZoneId, TimeGrain};
use sef_query_ir::query::{FilterExpr, OrderTerm, QualifiedDimensionRef, RequestedAtom, Selection};

use crate::graph::NodeId;

/// A model object a plan relies on: its stable identity **and** the
/// fingerprint of its canonical content, so a plan over a changed
/// definition is a different plan (§24) while a changed label or
/// description is not.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectRef {
    pub id: StableId,
    pub content: Fingerprint,
}

impl fmt::Display for ObjectRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.id)
    }
}

impl Canonical for ObjectRef {
    fn canonical_value(&self) -> C {
        C::record(
            "object-ref",
            [
                ("id", self.id.canonical_value()),
                ("content", C::Bytes(self.content.as_bytes().to_vec())),
            ],
        )
    }
}

/// A data contract whose truth the plan relies on (§7.3). How the evidence
/// is produced (certificate or runtime check) is decided later.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Contract {
    /// The "one" side of a relationship is unique on its join key.
    UniqueTarget { relationship: ObjectRef },
    /// An as-of relationship matches exactly one version per event.
    TemporalSingleMatch { relationship: ObjectRef },
    /// Bridge weights are complete and sum to the declared total.
    AllocationNormalization { bridge: ObjectRef },
}

impl Contract {
    /// The model object whose contract this is.
    #[must_use]
    pub const fn object(&self) -> &ObjectRef {
        match self {
            Self::UniqueTarget { relationship } | Self::TemporalSingleMatch { relationship } => {
                relationship
            }
            Self::AllocationNormalization { bridge } => bridge,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operator {
    // ---- Source and governance (§7) ----
    /// The semantic fields of a dataset the plan needs, never table syntax.
    ReadSemanticDataset {
        dataset: ObjectRef,
        fields: Vec<StableId>,
    },
    /// Mandatory row policy; it must dominate everything derived from the
    /// rows it protects. Slot *names* only: values are runtime context.
    PolicyGuard {
        input: NodeId,
        policy: ObjectRef,
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
    /// The intrinsic filter of a metric, part of its definition (§7.4),
    /// applied on that metric's own branch before aggregation.
    ApplyIntrinsicFilter {
        input: NodeId,
        owner: ObjectRef,
    },
    // ---- Dimensions and relationships (§8) ----
    /// Attaches a dimension through the validated relationship path; each
    /// dataset the path enters is read (and guarded) by its own lookup.
    AttachDimension {
        input: NodeId,
        lookups: Vec<NodeId>,
        dimension: QualifiedDimensionRef,
        definition: ObjectRef,
        path: Vec<ObjectRef>,
    },
    TemporalAsOf {
        input: NodeId,
        relationship: ObjectRef,
    },
    BridgeAllocate {
        input: NodeId,
        bridge: ObjectRef,
    },
    // ---- Time (§9) ----
    NormalizeTime {
        input: NodeId,
        dimension: ObjectRef,
        timezone: IanaZoneId,
        calendar: CalendarProfile,
    },
    TimeBucket {
        input: NodeId,
        dimension: ObjectRef,
        grain: TimeGrain,
        calendar: CalendarProfile,
    },
    SelectPeriodEnd {
        input: NodeId,
        metric: ObjectRef,
        axis: StableId,
        rule: TemporalSelection,
        period: Option<TimeGrain>,
        scope: Grain,
    },
    // ---- Metrics and aggregation (§10) ----
    /// Aggregates `measures` at `grain`; `serves` are the metrics whose
    /// values these aggregates are, under this branch's semantics.
    AggregateAtGrain {
        input: NodeId,
        grain: Vec<RequestedAtom>,
        measures: Vec<ObjectRef>,
        serves: Vec<ObjectRef>,
    },
    /// Exact distinct count at the grain; never summed afterwards.
    DistinctAggregate {
        input: NodeId,
        grain: Vec<RequestedAtom>,
        measure: ObjectRef,
        serves: Vec<ObjectRef>,
    },
    /// Builds a metric from components already at the target grain.
    RecomposeMetric {
        input: NodeId,
        metric: ObjectRef,
    },
    // ---- Multi-fact alignment (§11) ----
    AlignSemanticBranches {
        inputs: Vec<NodeId>,
        grain: Vec<RequestedAtom>,
    },
    // ---- Window and post-aggregate (§12) ----
    ComputeWindowMetric {
        input: NodeId,
        metric: ObjectRef,
        base: StableId,
        order_by: Vec<StableId>,
        partition_by: Grain,
    },
    ApplyPostAggregateFilter {
        input: NodeId,
        predicate: FilterExpr,
    },
    // ---- Result shape (§13) ----
    /// The result columns; `definitions` are the selected metrics and
    /// dimensions, so a changed definition is a changed plan.
    ProjectSemanticOutput {
        input: NodeId,
        selections: Vec<Selection>,
        definitions: Vec<ObjectRef>,
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
            Self::ApplyIntrinsicFilter { .. } => "ApplyIntrinsicFilter",
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
    /// served metrics, fields) in canonical order, so a stored operator is
    /// exactly what its identity encodes, whatever order it was written in.
    pub(crate) fn normalize(&mut self) {
        fn canonical<T: Ord>(items: &mut Vec<T>) {
            items.sort();
            items.dedup();
        }
        match self {
            Self::ReadSemanticDataset { fields, .. } => canonical(fields),
            Self::AggregateAtGrain {
                grain,
                measures,
                serves,
                ..
            } => {
                canonical(grain);
                canonical(measures);
                canonical(serves);
            }
            Self::DistinctAggregate { grain, serves, .. } => {
                canonical(grain);
                canonical(serves);
            }
            Self::AlignSemanticBranches { inputs, grain } => {
                canonical(inputs);
                canonical(grain);
            }
            Self::ProjectSemanticOutput { definitions, .. } => canonical(definitions),
            _ => {}
        }
    }

    /// The nodes this operator reads, in their semantic order.
    #[must_use]
    pub fn inputs(&self) -> Vec<NodeId> {
        match self {
            Self::ReadSemanticDataset { .. } => Vec::new(),
            Self::AlignSemanticBranches { inputs, .. } => inputs.clone(),
            Self::AttachDimension { input, lookups, .. } => {
                let mut all = vec![*input];
                all.extend(lookups);
                all
            }
            Self::PolicyGuard { input, .. }
            | Self::ContractGuard { input, .. }
            | Self::ApplyRowFilter { input, .. }
            | Self::ApplyIntrinsicFilter { input, .. }
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

    /// The model objects the operator relies on, so a checker can confirm
    /// each still has the content the plan was built from.
    #[must_use]
    pub fn objects(&self) -> Vec<&ObjectRef> {
        match self {
            Self::ReadSemanticDataset { dataset, .. } => vec![dataset],
            Self::PolicyGuard { policy, .. } => vec![policy],
            Self::ContractGuard { contract, .. } => vec![contract.object()],
            Self::ApplyIntrinsicFilter { owner, .. } => vec![owner],
            Self::AttachDimension {
                definition, path, ..
            } => std::iter::once(definition).chain(path).collect(),
            Self::TemporalAsOf { relationship, .. } => vec![relationship],
            Self::BridgeAllocate { bridge, .. } => vec![bridge],
            Self::NormalizeTime { dimension, .. } | Self::TimeBucket { dimension, .. } => {
                vec![dimension]
            }
            Self::SelectPeriodEnd { metric, .. }
            | Self::RecomposeMetric { metric, .. }
            | Self::ComputeWindowMetric { metric, .. } => vec![metric],
            Self::AggregateAtGrain {
                measures, serves, ..
            } => measures.iter().chain(serves).collect(),
            Self::DistinctAggregate {
                measure, serves, ..
            } => std::iter::once(measure).chain(serves).collect(),
            Self::ProjectSemanticOutput { definitions, .. } => definitions.iter().collect(),
            Self::ApplyRowFilter { .. }
            | Self::AlignSemanticBranches { .. }
            | Self::ApplyPostAggregateFilter { .. }
            | Self::OrderSemanticOutput { .. }
            | Self::LimitSemanticOutput { .. } => Vec::new(),
        }
    }

    /// The operator's semantic parameters, for the explanation (§26).
    /// Context values never appear: policies show only slot names.
    #[must_use]
    #[allow(clippy::too_many_lines)] // one arm per operator of the specification
    pub fn describe(&self) -> String {
        let list = |items: &[ObjectRef]| {
            items
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let grain =
            |atoms: &[RequestedAtom]| atoms.iter().map(atom_text).collect::<Vec<_>>().join(" × ");
        match self {
            Self::ReadSemanticDataset { dataset, fields } => format!(
                "{dataset} [{}]",
                fields
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::PolicyGuard { policy, slots, .. } => {
                format!("{policy} with :{}", slots.join(", :"))
            }
            Self::ContractGuard { contract, .. } => match contract {
                Contract::UniqueTarget { relationship } => {
                    format!("unique target of {relationship}")
                }
                Contract::TemporalSingleMatch { relationship } => {
                    format!("single as-of match of {relationship}")
                }
                Contract::AllocationNormalization { bridge } => {
                    format!("allocation of {bridge} sums to its total")
                }
            },
            Self::ApplyRowFilter { .. } => "caller filter (row phase)".to_owned(),
            Self::ApplyIntrinsicFilter { owner, .. } => format!("intrinsic filter of {owner}"),
            Self::AttachDimension {
                dimension, path, ..
            } => format!(
                "{}{} through [{}]",
                dimension.dimension,
                if dimension.roles.is_empty() {
                    String::new()
                } else {
                    format!(" as {}", dimension.roles.join("+"))
                },
                list(path)
            ),
            Self::TemporalAsOf { relationship, .. } => format!("{relationship} at event time"),
            Self::BridgeAllocate { bridge, .. } => format!("{bridge} by weight"),
            Self::NormalizeTime {
                dimension,
                timezone,
                calendar,
                ..
            } => format!("{dimension} to {} ({calendar:?})", timezone.as_str()),
            Self::TimeBucket {
                dimension,
                grain,
                calendar,
                ..
            } => format!("{dimension} by {grain:?} ({calendar:?})"),
            Self::SelectPeriodEnd {
                metric,
                axis,
                rule,
                period,
                scope,
                ..
            } => format!(
                "{metric}: {rule:?} on {axis} per {} within {} atom(s)",
                period.map_or_else(|| "whole range".to_owned(), |g| format!("{g:?}")),
                scope.atoms().len()
            ),
            Self::AggregateAtGrain {
                grain: atoms,
                measures,
                serves,
                ..
            } => format!(
                "[{}] at [{}] for [{}]",
                list(measures),
                grain(atoms),
                list(serves)
            ),
            Self::DistinctAggregate {
                grain: atoms,
                measure,
                serves,
                ..
            } => format!(
                "distinct {measure} at [{}] for [{}]",
                grain(atoms),
                list(serves)
            ),
            Self::RecomposeMetric { metric, .. } => format!("{metric}"),
            Self::AlignSemanticBranches { grain: atoms, .. } => format!("on [{}]", grain(atoms)),
            Self::ComputeWindowMetric {
                metric,
                base,
                order_by,
                ..
            } => format!(
                "{metric} = cumulative {base} ordered by [{}]",
                order_by
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::ApplyPostAggregateFilter { .. } => "metric filter (after aggregation)".to_owned(),
            Self::ProjectSemanticOutput { selections, .. } => format!(
                "[{}]",
                selections
                    .iter()
                    .map(|selection| selection.selection_id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::OrderSemanticOutput { order, .. } => format!(
                "by [{}]",
                order
                    .iter()
                    .map(|term| term.selection_id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::LimitSemanticOutput { limit, offset, .. } => {
                format!("limit {limit:?} offset {offset:?}")
            }
        }
    }
}

fn atom_text(atom: &RequestedAtom) -> String {
    match atom {
        RequestedAtom::Dimension(dimension) => dimension.dimension.to_string(),
        RequestedAtom::TimeBucket(time) => format!(
            "{} by {:?}{}",
            time.dimension.dimension,
            time.grain,
            time.timezone
                .as_ref()
                .map(|zone| format!(" in {}", zone.as_str()))
                .unwrap_or_default()
        ),
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
            Self::ApplyIntrinsicFilter { input, owner } => vec![
                ("input", input.canonical_value()),
                ("owner", owner.canonical_value()),
            ],
            Self::AttachDimension {
                input,
                lookups,
                dimension,
                definition,
                path,
            } => vec![
                ("input", input.canonical_value()),
                ("lookups", C::sequence(lookups)),
                ("dimension", dimension.canonical_value()),
                ("definition", definition.canonical_value()),
                ("path", C::sequence(path)),
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
                serves,
            } => vec![
                ("input", input.canonical_value()),
                ("grain", C::set(grain)),
                ("measures", C::set(measures)),
                ("serves", C::set(serves)),
            ],
            Self::DistinctAggregate {
                input,
                grain,
                measure,
                serves,
            } => vec![
                ("input", input.canonical_value()),
                ("grain", C::set(grain)),
                ("measure", measure.canonical_value()),
                ("serves", C::set(serves)),
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
            Self::ProjectSemanticOutput {
                input,
                selections,
                definitions,
            } => vec![
                ("input", input.canonical_value()),
                // Output order is the result's shape.
                ("selections", C::sequence(selections)),
                ("definitions", C::set(definitions)),
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
