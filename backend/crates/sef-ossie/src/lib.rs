//! The Apache Ossie boundary of the Semantic Execution Fabric (ADR 0006).
//!
//! Ossie is the interchange format of `metrics/*.yaml` (ADR 0003); it is
//! never the internal model (CSMIR is). This crate reads and writes Ossie
//! documents and applies the official validator's checks; compiling them
//! to CSMIR comes next (Issue #48).

pub mod diagnostic;
pub mod document;
pub mod load;
pub mod validate;

pub use diagnostic::{Code, Diagnostic, Severity};
pub use document::{SPEC_VERSION, SemanticModel};
pub use load::{Loaded, load, parse, to_json, to_yaml};
