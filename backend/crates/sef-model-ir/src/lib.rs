//! CSMIR v0.1: immutable compiled semantic definitions and canonical fingerprints.
//!
//! Definition objects are construction inputs. Only successful compilation creates
//! a [`model::CompiledSemanticModel`]. Query validity and execution are later stages.

// Definition compilation returns structured diagnostics by value. Keep one stable
// error shape; boxing solely for this lint would complicate these cold error paths.
#![allow(clippy::result_large_err)]

pub mod canonical;
mod canonical_projection;
pub mod diagnostic;
pub mod expressions;
pub mod functions;
pub mod grain;
pub mod identity;
pub mod model;
pub mod policy;
pub mod relationships;
pub mod time;
pub mod types;
pub mod units;
mod validation;
