//! The Apache Ossie boundary of the Semantic Execution Fabric (ADR 0006).
//!
//! Ossie is the interchange format of `metrics/*.yaml` (ADR 0003); it is
//! never the internal model (CSMIR is). This crate reads and writes Ossie
//! documents, applies the official validator's checks and converts them to
//! CSMIR with the exact source types a [`catalog::Catalog`] provides.

pub mod catalog;
pub mod csmir;
pub mod diagnostic;
pub mod document;
pub mod expression;
pub mod extension;
pub mod ids;
pub mod load;
pub mod positions;
pub mod validate;

pub use diagnostic::{Code, Diagnostic, Severity};
pub use document::{SPEC_VERSION, SemanticModel};
pub use load::{Loaded, load, parse, to_json, to_yaml};
pub use positions::Positions;
