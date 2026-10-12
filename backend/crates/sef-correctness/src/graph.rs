//! The typed relationship graph (§10–§12): every traversal is classified
//! relative to the observations it starts from. Cardinality is a declared
//! contract, never an optimizer statistic; CSMIR compilation rejects one
//! whose "one" side has no unique key, so an unproven cardinality never
//! reaches this engine.

use std::collections::BTreeSet;

use sef_model_ir::identity::StableId;
use sef_model_ir::model::{ModelDefinition, SemanticDataset};
use sef_model_ir::relationships::{Cardinality, Relationship};

/// Longest path considered. Corpus paths have at most two hops; a longer
/// declared chain is still found, an unbounded search is never run.
const MAX_HOPS: usize = 6;

/// What one traversal does to the observations it starts from (§12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Traversal {
    AtMostOneMatchPerSourceObservation,
    RequiresTemporalSingleMatch,
    MayMultiplySourceObservation,
    RequiresAllocation,
}

impl Traversal {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::AtMostOneMatchPerSourceObservation => "at-most-one-match",
            Self::RequiresTemporalSingleMatch => "temporal-single-match",
            Self::MayMultiplySourceObservation => "may-multiply",
            Self::RequiresAllocation => "requires-allocation",
        }
    }

    /// Never changes how many times a source observation is counted.
    pub(crate) const fn is_single(self) -> bool {
        matches!(
            self,
            Self::AtMostOneMatchPerSourceObservation | Self::RequiresTemporalSingleMatch
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Step<'m> {
    pub(crate) relationship: &'m Relationship,
    pub(crate) traversal: Traversal,
}

/// A simple path (no dataset visited twice) from an anchor dataset.
#[derive(Clone, Debug)]
pub(crate) struct Path<'m> {
    pub(crate) steps: Vec<Step<'m>>,
}

impl Path<'_> {
    pub(crate) fn relationships(&self) -> Vec<StableId> {
        self.steps
            .iter()
            .map(|step| step.relationship.metadata.id().clone())
            .collect()
    }

    pub(crate) fn roles(&self) -> BTreeSet<&str> {
        self.steps
            .iter()
            .map(|step| step.relationship.role.as_str())
            .collect()
    }
}

/// Every simple path from `anchor` to any dataset in `targets`, including
/// the empty path when the anchor is itself a target.
pub(crate) fn paths<'m>(
    model: &'m ModelDefinition,
    anchor: &StableId,
    targets: &BTreeSet<&StableId>,
) -> Vec<Path<'m>> {
    let mut found = Vec::new();
    let mut visited = vec![anchor.clone()];
    walk(
        model,
        anchor,
        targets,
        &mut visited,
        &mut Vec::new(),
        &mut found,
    );
    found
}

fn walk<'m>(
    model: &'m ModelDefinition,
    at: &StableId,
    targets: &BTreeSet<&StableId>,
    visited: &mut Vec<StableId>,
    steps: &mut Vec<Step<'m>>,
    found: &mut Vec<Path<'m>>,
) {
    if targets.contains(at) {
        found.push(Path {
            steps: steps.clone(),
        });
    }
    if steps.len() == MAX_HOPS {
        return;
    }
    for relationship in &model.relationships {
        let (next, forward) = if &relationship.from_dataset == at {
            (&relationship.to_dataset, true)
        } else if &relationship.to_dataset == at {
            (&relationship.from_dataset, false)
        } else {
            continue;
        };
        if visited.contains(next) {
            continue;
        }
        visited.push(next.clone());
        steps.push(Step {
            relationship,
            traversal: traversal(relationship, forward),
        });
        walk(model, next, targets, visited, steps, found);
        steps.pop();
        visited.pop();
    }
}

/// How traversing `relationship` (forward: from its many side to its one
/// side) affects the source observations. A compiled model has already
/// proven every "one" side unique and every contract valid
/// (`sef-model-ir` validation), so a to-one traversal is proven here.
fn traversal(relationship: &Relationship, forward: bool) -> Traversal {
    match (relationship.cardinality, forward) {
        (Cardinality::ManyToMany, _) => Traversal::RequiresAllocation,
        (Cardinality::ManyToOne, false) | (Cardinality::OneToMany, true) => {
            Traversal::MayMultiplySourceObservation
        }
        _ if relationship.temporal_constraint.is_some() => Traversal::RequiresTemporalSingleMatch,
        _ => Traversal::AtMostOneMatchPerSourceObservation,
    }
}

pub(crate) fn dataset_of<'m>(
    model: &'m ModelDefinition,
    id: &StableId,
) -> Option<&'m SemanticDataset> {
    model
        .datasets
        .iter()
        .find(|dataset| dataset.metadata.id() == id)
}
