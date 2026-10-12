//! Semantic Execution IR (SEIR) of the Semantic Execution Fabric
//! (`docs/semantica/04_CORRECTNESS_AND_PLANNING.md`, Part II).
//!
//! A typed, content-addressed DAG of semantic operations: what must happen,
//! in which semantic order, for a validated query to keep its meaning. It is
//! not SQL, not a relational algebra plan and not a physical plan; adapters
//! lower it later. Identity is derived from canonical content, so the same
//! meaning always yields the same plan fingerprint.

mod graph;
mod operator;

pub use graph::{GraphError, Node, NodeId, PlanBlock, SeirGraph, SemanticPlan};
pub use operator::{Contract, Operator};
