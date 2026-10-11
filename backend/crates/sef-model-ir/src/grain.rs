use crate::identity::StableId;
use crate::time::TimeBucket;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum GrainAtom {
    EntityKeyRef(StableId),
    DimensionRef {
        dimension: StableId,
        roles: Vec<String>,
    },
    TimeBucketRef(TimeBucket),
}

/// Normalized semantic atoms only; functional-dependency closure is a later phase.
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct Grain(BTreeSet<GrainAtom>);

impl Grain {
    pub fn new(atoms: impl IntoIterator<Item = GrainAtom>) -> Self {
        Self(atoms.into_iter().collect())
    }
    #[must_use]
    pub fn global() -> Self {
        Self::default()
    }
    pub fn atoms(&self) -> impl ExactSizeIterator<Item = &GrainAtom> {
        self.0.iter()
    }
    #[must_use]
    pub fn is_global(&self) -> bool {
        self.0.is_empty()
    }
}
