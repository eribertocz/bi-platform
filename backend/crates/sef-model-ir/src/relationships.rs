//! Declared relationship/data contracts, separate from runtime evidence freshness.

use crate::canonical::{Canonical, CanonicalValue as C};
use crate::expressions::ScalarExpr;
use crate::grain::Grain;
use crate::identity::StableId;
use crate::model::{ObjectMetadata, canonical_record, canonical_unit_enum};
use crate::types::DecimalValue;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractEvidenceState {
    Verified {
        evidence_id: String,
        observed_at: String,
        source_snapshot: String,
    },
    DeclaredUnverified,
    Invalid {
        evidence_id: String,
        observed_at: String,
    },
}

impl ContractEvidenceState {
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    OneToOne,
    ManyToOne,
    OneToMany,
    ManyToMany,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Optionality {
    Required,
    Optional,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Relationship {
    pub metadata: ObjectMetadata,
    pub from_dataset: StableId,
    pub to_dataset: StableId,
    pub role: String,
    pub cardinality: Cardinality,
    pub key_pairs: Vec<KeyPair>,
    pub optionality: Optionality,
    pub temporal_constraint: Option<TemporalConstraint>,
    pub evidence: ContractEvidenceState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPair {
    pub from_field: StableId,
    pub to_field: StableId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemporalConstraint {
    pub event_time: StableId,
    pub valid_from: StableId,
    pub valid_to: Option<StableId>,
    pub interval_boundary: IntervalBoundary,
    pub match_cardinality: MatchCardinality,
    pub evidence: ContractEvidenceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntervalBoundary {
    ClosedOpen,
    ClosedClosed,
    OpenClosed,
    OpenOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchCardinality {
    ExactlyOne,
    AtMostOne,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bridge {
    pub metadata: ObjectMetadata,
    pub left_dataset: StableId,
    pub bridge_dataset: StableId,
    pub right_dataset: StableId,
    pub left_relationship: StableId,
    pub right_relationship: StableId,
    pub allocation: Option<Allocation>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Allocation {
    pub weight_expression: ScalarExpr,
    pub normalization_scope: Grain,
    pub required_total: DecimalValue,
    pub tolerance: DecimalValue,
    pub evidence: ContractEvidenceState,
}

canonical_unit_enum!(Cardinality, "cardinality", OneToOne => "one-to-one", ManyToOne => "many-to-one", OneToMany => "one-to-many", ManyToMany => "many-to-many");
canonical_unit_enum!(Optionality, "optionality", Required => "required", Optional => "optional");
canonical_unit_enum!(IntervalBoundary, "interval-boundary", ClosedOpen => "closed-open", ClosedClosed => "closed-closed", OpenClosed => "open-closed", OpenOpen => "open-open");
canonical_unit_enum!(MatchCardinality, "match-cardinality", ExactlyOne => "exactly-one", AtMostOne => "at-most-one");
canonical_record!(KeyPair, "relationship-key-pair", from_field, to_field);
canonical_record!(
    Bridge,
    "bridge",
    metadata,
    left_dataset,
    bridge_dataset,
    right_dataset,
    left_relationship,
    right_relationship,
    allocation
);

impl Canonical for Relationship {
    fn canonical_value(&self) -> C {
        C::record(
            "relationship",
            [
                ("metadata", self.metadata.canonical_value()),
                ("from_dataset", self.from_dataset.canonical_value()),
                ("to_dataset", self.to_dataset.canonical_value()),
                ("role", self.role.canonical_value()),
                ("cardinality", self.cardinality.canonical_value()),
                ("key_pairs", C::set(&self.key_pairs)),
                ("optionality", self.optionality.canonical_value()),
                (
                    "temporal_constraint",
                    self.temporal_constraint.canonical_value(),
                ),
            ],
        )
    }
}

impl Canonical for TemporalConstraint {
    fn canonical_value(&self) -> C {
        C::record(
            "temporal-constraint",
            [
                ("event_time", self.event_time.canonical_value()),
                ("valid_from", self.valid_from.canonical_value()),
                ("valid_to", self.valid_to.canonical_value()),
                (
                    "interval_boundary",
                    self.interval_boundary.canonical_value(),
                ),
                (
                    "match_cardinality",
                    self.match_cardinality.canonical_value(),
                ),
            ],
        )
    }
}

impl Canonical for Allocation {
    fn canonical_value(&self) -> C {
        C::record(
            "allocation",
            [
                (
                    "weight_expression",
                    self.weight_expression.canonical_value(),
                ),
                (
                    "normalization_scope",
                    self.normalization_scope.canonical_value(),
                ),
                ("required_total", self.required_total.canonical_value()),
                ("tolerance", self.tolerance.canonical_value()),
            ],
        )
    }
}
