//! The typed relationship graph (§10–§12): every traversal is classified
//! relative to the observations it starts from. Cardinality is a declared
//! contract, never an optimizer statistic; CSMIR compilation rejects one
//! whose "one" side has no unique key, so an unproven cardinality never
//! reaches this engine.

use std::collections::BTreeSet;

use sef_model_ir::identity::StableId;
use sef_model_ir::model::{ModelDefinition, SemanticDataset};
use sef_model_ir::relationships::{Cardinality, Relationship};

/// Steps the path search may explore. Simple paths are finite, so the
/// search is complete within the budget; exhausting it is reported, because
/// a truncated search proves neither uniqueness nor absence (§11).
const SEARCH_BUDGET: usize = 100_000;

/// The path search exhausted [`SEARCH_BUDGET`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Exhausted;

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
) -> Result<Vec<Path<'m>>, Exhausted> {
    let mut search = Search {
        model,
        targets,
        visited: vec![anchor.clone()],
        steps: Vec::new(),
        found: Vec::new(),
        budget: SEARCH_BUDGET,
    };
    search.walk(anchor)?;
    Ok(search.found)
}

struct Search<'m, 't> {
    model: &'m ModelDefinition,
    targets: &'t BTreeSet<&'t StableId>,
    visited: Vec<StableId>,
    steps: Vec<Step<'m>>,
    found: Vec<Path<'m>>,
    budget: usize,
}

impl Search<'_, '_> {
    fn walk(&mut self, at: &StableId) -> Result<(), Exhausted> {
        if self.targets.contains(at) {
            self.found.push(Path {
                steps: self.steps.clone(),
            });
        }
        for relationship in &self.model.relationships {
            let (next, forward) = if &relationship.from_dataset == at {
                (&relationship.to_dataset, true)
            } else if &relationship.to_dataset == at {
                (&relationship.from_dataset, false)
            } else {
                continue;
            };
            if self.visited.contains(next) {
                continue;
            }
            self.budget = self.budget.checked_sub(1).ok_or(Exhausted)?;
            self.visited.push(next.clone());
            self.steps.push(Step {
                relationship,
                traversal: traversal(relationship, forward),
            });
            self.walk(next)?;
            self.steps.pop();
            self.visited.pop();
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::collections::BTreeSet;

    use sef_model_ir::identity::{ObjectIdentity, SemanticVersion, StableId};
    use sef_model_ir::model::{ModelDefinition, ObjectMetadata};
    use sef_model_ir::relationships::{
        Cardinality, ContractEvidenceState, Optionality, Relationship,
    };

    use super::{Exhausted, paths};

    fn id(text: &str) -> StableId {
        StableId::new(text).expect("id")
    }

    fn metadata(text: &str) -> ObjectMetadata {
        ObjectMetadata::new(ObjectIdentity {
            stable_id: id(text),
            semantic_version: SemanticVersion::new("0.1.0").expect("version"),
        })
    }

    fn model(edges: &[(&str, &str)]) -> ModelDefinition {
        let mut model = ModelDefinition::new(metadata("t/model/m"));
        for (i, (from, to)) in edges.iter().enumerate() {
            model.relationships.push(Relationship {
                metadata: metadata(&format!("t/relationship/r{i}")),
                from_dataset: id(&format!("t/dataset/{from}")),
                to_dataset: id(&format!("t/dataset/{to}")),
                role: format!("r{i}"),
                cardinality: Cardinality::ManyToOne,
                key_pairs: Vec::new(),
                optionality: Optionality::Required,
                temporal_constraint: None,
                evidence: ContractEvidenceState::DeclaredUnverified,
            });
        }
        model
    }

    fn found(model: &ModelDefinition, from: &str, to: &str) -> Result<Vec<usize>, Exhausted> {
        let target = id(&format!("t/dataset/{to}"));
        let targets: BTreeSet<&StableId> = [&target].into_iter().collect();
        let mut lengths: Vec<usize> = paths(model, &id(&format!("t/dataset/{from}")), &targets)?
            .iter()
            .map(|path| path.steps.len())
            .collect();
        lengths.sort_unstable();
        Ok(lengths)
    }

    #[test]
    fn a_long_second_path_is_found_not_truncated() {
        // sales → store → region, and sales → c1 → … → c6 → region (7 hops):
        // both meanings are seen, so the engine can call them ambiguous.
        let mut edges = vec![("sales", "store"), ("store", "region"), ("sales", "c1")];
        for i in 1..6 {
            edges.push((
                ["c1", "c2", "c3", "c4", "c5"][i - 1],
                ["c2", "c3", "c4", "c5", "c6"][i - 1],
            ));
        }
        edges.push(("c6", "region"));
        assert_eq!(found(&model(&edges), "sales", "region"), Ok(vec![2, 7]));
        let long_only: Vec<(&str, &str)> = edges[2..].to_vec();
        assert_eq!(found(&model(&long_only), "sales", "region"), Ok(vec![7]));
    }

    #[test]
    fn an_exhausted_search_says_so() {
        // A complete graph of 12 datasets has millions of simple paths.
        let names: Vec<String> = (0..12).map(|i| format!("d{i}")).collect();
        let mut edges = Vec::new();
        for (i, a) in names.iter().enumerate() {
            for b in &names[i + 1..] {
                edges.push((a.as_str(), b.as_str()));
            }
        }
        assert_eq!(found(&model(&edges), "d0", "d11"), Err(Exhausted));
    }
}
