//! Semantic correctness engine of the Semantic Execution Fabric
//! (`docs/semantica/04_CORRECTNESS_AND_PLANNING.md`, Part I).
//!
//! Given a compiled model (CSMIR), a bound query (SQIR) and the runtime
//! semantic context, it decides deterministically whether the request is
//! meaningful. If it is, it returns the proven facts and the obligations
//! every valid plan must satisfy; if not, it rejects with the stable
//! semantic diagnostic shared with model compilation (§32). It never
//! generates SQL, never chooses an execution strategy, and depends on no
//! engine, dialect, Ossie DTO or transport.

mod error;
mod graph;
mod outcome;
mod validate;

pub use error::ErrorDomain;
pub use outcome::{BlockValidation, Fact, Obligation, ValidatedQuery};
pub use validate::{SemanticContext, validate};
