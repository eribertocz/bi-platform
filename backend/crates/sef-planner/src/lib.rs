//! Deterministic semantic planner of the Semantic Execution Fabric
//! (`docs/semantica/04_CORRECTNESS_AND_PLANNING.md`, Part II).
//!
//! From a query the correctness engine validated, it builds the single
//! canonical baseline plan in SEIR and a certificate pairing every
//! correctness obligation with the plan evidence that discharges it. It
//! chooses no engine, no join algorithm, no materialization and no cost:
//! those belong to later stages, which must prove themselves equivalent to
//! this baseline.

mod build;
mod certificate;

pub use build::{PlannedQuery, plan};
pub use certificate::{Certificate, Discharge, Evidence};
