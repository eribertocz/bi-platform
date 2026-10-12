//! Authorization core of the Semantic Execution Fabric (ADR 0012).
//!
//! Cedar decides who may do what on which object. This crate wraps it so
//! that every decision:
//! - **denies by default and on any error**: an evaluation error, an
//!   invalid entity or context, or an unknown principal, action or resource
//!   is a denial, never a skipped policy;
//! - **explains itself**: a stable reason, the determining policies and a
//!   fingerprint of everything it depended on, for audit and cache keys;
//! - **is evaluated against an immutable snapshot** of policies and schema.
//!
//! Column-level security for semantic queries ([`authorize_query`]) decides
//! before execution whether each object may be shown, shown masked or not
//! used at all. Row-level security stays in CSMIR policies.
//!
//! The crate knows no HTTP, storage or product module: the edges supply
//! neutral principals and resources ([`Principal`], [`Resource`]).

mod authorize;
mod columns;
mod contract;
mod decision;
mod schema;
mod snapshot;

pub use authorize::authorize;
pub use columns::{QueryDecision, Usage, Use, authorize_query};
pub use contract::{Action, AttrValue, Context, Principal, Resource, ResourceKind};
pub use decision::{Constraint, Decision, Fingerprint, Reason};
pub use schema::{BASE_POLICIES, SEMANTIC_SCHEMA, SEMANTIC_SCHEMA_VERSION};
pub use snapshot::{PolicySnapshot, PolicySource, Problem, Severity};
