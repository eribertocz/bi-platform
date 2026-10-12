//! SQIR v0.1: the canonical semantic query of SEF (`docs/semantica/
//! 03_MODEL_AND_QUERY_IR.md`, Part II).
//!
//! A front end produces an [`unbound::UnboundQuerySet`]; [`bind`] resolves
//! it against a CSMIR model into a [`query::SemanticQuerySet`], whose
//! canonical fingerprint identifies the query's meaning. Grain, fanout,
//! path ambiguity and policies are decided later by the Correctness Engine;
//! identity, policy values and execution requirements travel outside SQIR,
//! in the request envelope.

pub mod bind;
pub mod error;
pub mod query;
pub mod unbound;

pub use bind::bind;
pub use error::{QueryError, QueryErrorCode};
