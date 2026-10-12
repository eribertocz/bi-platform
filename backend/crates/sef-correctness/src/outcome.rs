//! What a successful validation proves (facts) and requires of every valid
//! plan (obligations), per query block (§4). Obligations say *what*
//! correctness needs, never how a backend implements it.

use std::collections::BTreeSet;
use std::fmt;

use sef_model_ir::grain::{Grain, GrainAtom};
use sef_model_ir::identity::StableId;
use sef_model_ir::time::{CalendarProfile, IanaZoneId, TimeGrain};
use sef_query_ir::query::{QualifiedDimensionRef, RequestedAtom, SemanticQuerySet};

/// A proven property of the request (§4.1), not a planner choice.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fact {
    RequestedGrain(Vec<RequestedAtom>),
    /// A metric's aggregation class (`fully-additive`, `semi-additive`…).
    MetricClass {
        metric: StableId,
        class: &'static str,
    },
    MetricNativeGrain {
        metric: StableId,
        grain: Grain,
    },
    /// The unique relationship path from a fact dataset to a dimension;
    /// empty when the dimension is bound on the dataset itself.
    RelationshipPathUnique {
        anchor: StableId,
        dimension: QualifiedDimensionRef,
        relationships: Vec<StableId>,
    },
    /// Traversing `relationship` from `anchor` matches at most one row.
    CardinalityProven {
        relationship: StableId,
        class: &'static str,
    },
    PolicyRequired {
        scope: StableId,
        policy: StableId,
    },
}

/// A constraint every valid semantic plan must satisfy (§4.2).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Obligation {
    /// Aggregate this measure at its own grain, isolated from other facts.
    PreserveMetricGrain {
        measure: StableId,
        grain: Grain,
    },
    /// Independent facts meet only after each is aggregated to this grain.
    AggregateBeforeAlignment {
        grain: Vec<RequestedAtom>,
    },
    /// Recompose the metric from its components at the requested grain;
    /// never aggregate lower-grain results of it.
    RecomputeAtTargetGrain {
        metric: StableId,
        components: Vec<StableId>,
    },
    /// Compute from rows at every requested grain; partial results of this
    /// metric are never combined.
    DoNotReaggregate {
        metric: StableId,
    },
    /// Keep one snapshot per period and scope before any other aggregation.
    /// `period` is the requested bucket of the axis; `None` selects over the
    /// whole requested range.
    ApplyPeriodEndSelection {
        metric: StableId,
        axis: StableId,
        rule: &'static str,
        period: Option<TimeGrain>,
        scope: Grain,
    },
    /// A cumulative operation after the base metric's aggregation.
    ApplyOrderedCumulative {
        metric: StableId,
        base: StableId,
        order_by: Vec<StableId>,
        partition_by: Grain,
    },
    UseTemporalRelationship {
        relationship: StableId,
    },
    UseDeclaredRelationshipRole {
        relationship: StableId,
        role: String,
    },
    ApplyBridgeAllocation {
        metric: StableId,
        bridge: StableId,
    },
    ValidateAllocationNormalization {
        bridge: StableId,
    },
    ApplyMandatoryPolicy {
        policy: StableId,
        scope: StableId,
    },
    /// Instant → business zone → calendar bucket, in that order (§17.1),
    /// with the calendar whose periods the bucket follows.
    ConvertTimezoneBeforeBucketing {
        dimension: StableId,
        dataset: StableId,
        timezone: IanaZoneId,
        calendar: CalendarProfile,
        grain: TimeGrain,
    },
    /// Calendar values (dates) grouped into the periods of a calendar; the
    /// calendar is part of the contract, never a backend default.
    ApplyCalendarBucket {
        dimension: StableId,
        dataset: StableId,
        calendar: CalendarProfile,
        grain: TimeGrain,
    },
    PreserveExactDecimalSemantics {
        metric: StableId,
    },
    PreserveDeclaredNullSemantics {
        measure: StableId,
        semantics: &'static str,
    },
}

/// The facts and obligations of one query block, in canonical order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockValidation {
    pub block_id: String,
    pub facts: BTreeSet<Fact>,
    pub obligations: BTreeSet<Obligation>,
}

/// A query proven meaningful, ready for the semantic planner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedQuery {
    pub query: SemanticQuerySet,
    pub blocks: Vec<BlockValidation>,
}

// ---- Readable rendering (golden files, diagnostics) ---------------------------

pub(crate) fn atom_text(atom: &GrainAtom) -> String {
    match atom {
        GrainAtom::EntityKeyRef(key) => format!("key {key}"),
        GrainAtom::DimensionRef { dimension, roles } => dimension_text(dimension, roles),
        GrainAtom::TimeBucketRef(bucket) => format!(
            "{} by {:?} in {}",
            bucket.source,
            bucket.grain,
            bucket.timezone.as_str()
        ),
    }
}

fn dimension_text(dimension: &StableId, roles: &[String]) -> String {
    if roles.is_empty() {
        dimension.to_string()
    } else {
        format!("{dimension} via {}", roles.join("+"))
    }
}

fn requested_text(atom: &RequestedAtom) -> String {
    match atom {
        RequestedAtom::Dimension(dimension) => {
            dimension_text(&dimension.dimension, &dimension.roles)
        }
        RequestedAtom::TimeBucket(time) => format!(
            "{} by {:?}{}",
            dimension_text(&time.dimension.dimension, &time.dimension.roles),
            time.grain,
            time.timezone
                .as_ref()
                .map(|zone| format!(" in {}", zone.as_str()))
                .unwrap_or_default()
        ),
    }
}

fn grain_text(grain: &Grain) -> String {
    let atoms: Vec<String> = grain.atoms().map(atom_text).collect();
    format!("[{}]", atoms.join(", "))
}

fn list(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(", "))
}

fn ids(items: &[StableId]) -> String {
    list(items.iter().map(ToString::to_string))
}

impl fmt::Display for Fact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestedGrain(atoms) => {
                write!(
                    f,
                    "requested grain {}",
                    list(atoms.iter().map(requested_text))
                )
            }
            Self::MetricClass { metric, class } => write!(f, "{metric} is {class}"),
            Self::MetricNativeGrain { metric, grain } => {
                write!(f, "{metric} native grain {}", grain_text(grain))
            }
            Self::RelationshipPathUnique {
                anchor,
                dimension,
                relationships,
            } => write!(
                f,
                "{} reaches {} {}",
                anchor,
                dimension_text(&dimension.dimension, &dimension.roles),
                if relationships.is_empty() {
                    "locally".to_owned()
                } else {
                    format!("through {}", ids(relationships))
                }
            ),
            Self::CardinalityProven {
                relationship,
                class,
            } => write!(f, "{relationship} proven {class}"),
            Self::PolicyRequired { scope, policy } => write!(f, "{scope} requires policy {policy}"),
        }
    }
}

impl fmt::Display for Obligation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PreserveMetricGrain { measure, grain } => {
                write!(f, "PreserveMetricGrain {measure} at {}", grain_text(grain))
            }
            Self::AggregateBeforeAlignment { grain } => write!(
                f,
                "AggregateBeforeAlignment {}",
                list(grain.iter().map(requested_text))
            ),
            Self::RecomputeAtTargetGrain { metric, components } => {
                write!(
                    f,
                    "RecomputeAtTargetGrain {metric} from {}",
                    ids(components)
                )
            }
            Self::DoNotReaggregate { metric } => write!(f, "DoNotReaggregate {metric}"),
            Self::ApplyPeriodEndSelection {
                metric,
                axis,
                rule,
                period,
                scope,
            } => write!(
                f,
                "ApplyPeriodEndSelection {metric} {rule} on {axis} per {} within {}",
                period.map_or_else(|| "whole range".to_owned(), |grain| format!("{grain:?}")),
                grain_text(scope)
            ),
            Self::ApplyOrderedCumulative {
                metric,
                base,
                order_by,
                partition_by,
            } => write!(
                f,
                "ApplyOrderedCumulative {metric} over {base} ordered by {} partitioned by {}",
                ids(order_by),
                grain_text(partition_by)
            ),
            Self::UseTemporalRelationship { relationship } => {
                write!(f, "UseTemporalRelationship {relationship}")
            }
            Self::UseDeclaredRelationshipRole { relationship, role } => {
                write!(f, "UseDeclaredRelationshipRole {relationship} as {role}")
            }
            Self::ApplyBridgeAllocation { metric, bridge } => {
                write!(f, "ApplyBridgeAllocation {metric} through {bridge}")
            }
            Self::ValidateAllocationNormalization { bridge } => {
                write!(f, "ValidateAllocationNormalization {bridge}")
            }
            Self::ApplyMandatoryPolicy { policy, scope } => {
                write!(f, "ApplyMandatoryPolicy {policy} on {scope}")
            }
            Self::ConvertTimezoneBeforeBucketing {
                dimension,
                dataset,
                timezone,
                calendar,
                grain,
            } => write!(
                f,
                "ConvertTimezoneBeforeBucketing {dimension} on {dataset} to {} then {grain:?} ({calendar:?})",
                timezone.as_str()
            ),
            Self::ApplyCalendarBucket {
                dimension,
                dataset,
                calendar,
                grain,
            } => write!(
                f,
                "ApplyCalendarBucket {dimension} on {dataset} by {grain:?} ({calendar:?})"
            ),
            Self::PreserveExactDecimalSemantics { metric } => {
                write!(f, "PreserveExactDecimalSemantics {metric}")
            }
            Self::PreserveDeclaredNullSemantics { measure, semantics } => {
                write!(f, "PreserveDeclaredNullSemantics {measure} {semantics}")
            }
        }
    }
}

impl fmt::Display for ValidatedQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for block in &self.blocks {
            writeln!(f, "block {}", block.block_id)?;
            for fact in &block.facts {
                writeln!(f, "  fact {fact}")?;
            }
            for obligation in &block.obligations {
                writeln!(f, "  must {obligation}")?;
            }
        }
        Ok(())
    }
}
