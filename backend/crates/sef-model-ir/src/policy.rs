//! Typed policy declarations. No runtime principal or slot values live here.

use crate::canonical::{Canonical, CanonicalValue as C};
use crate::expressions::ScalarExpr;
use crate::identity::StableId;
use crate::model::{ObjectMetadata, canonical_record, canonical_unit_enum};
use crate::types::ValueType;

#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub metadata: ObjectMetadata,
    pub scope: PolicyScope,
    pub enforcement: Enforcement,
    pub predicate: ScalarExpr,
    pub context_slots: Vec<ContextSlot>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PolicyScope {
    Dataset(StableId),
    Metric(StableId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enforcement {
    Mandatory,
    OptionalGoverned,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextSlot {
    pub name: String,
    pub value_type: ValueType,
    pub required: bool,
    pub sensitivity: Sensitivity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensitivity {
    Public,
    Sensitive,
    Secret,
}

canonical_unit_enum!(Enforcement, "policy-enforcement", Mandatory => "mandatory", OptionalGoverned => "optional-governed");
canonical_unit_enum!(Sensitivity, "context-sensitivity", Public => "public", Sensitive => "sensitive", Secret => "secret");
canonical_record!(
    ContextSlot,
    "context-slot",
    name,
    value_type,
    required,
    sensitivity
);

impl Canonical for PolicyScope {
    fn canonical_value(&self) -> C {
        match self {
            Self::Dataset(id) => {
                C::record("policy-scope.dataset", [("dataset", id.canonical_value())])
            }
            Self::Metric(id) => {
                C::record("policy-scope.metric", [("metric", id.canonical_value())])
            }
        }
    }
}

impl Canonical for Policy {
    fn canonical_value(&self) -> C {
        C::record(
            "policy",
            [
                ("metadata", self.metadata.canonical_value()),
                ("scope", self.scope.canonical_value()),
                ("enforcement", self.enforcement.canonical_value()),
                ("predicate", self.predicate.canonical_value()),
                ("context_slots", C::set(&self.context_slots)),
            ],
        )
    }
}
