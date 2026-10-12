//! The semantic half of the Cedar schema and the base role policies, both
//! owned by SEF and versioned with this crate (ADR 0012 §4 and §7).

/// Entity types of semantic objects, with the governance attributes CSMIR
/// carries, and the actions of the owner's security model. It refers to
/// `User`, `Role`, `Dashboard`, `Source` and `RequestContext`, which the
/// edge declares in the platform fragment.
pub const SEMANTIC_SCHEMA: &str = include_str!("semantic.cedarschema");

/// Changes whenever [`SEMANTIC_SCHEMA`] changes meaning; part of every
/// snapshot version and therefore of every decision fingerprint.
pub const SEMANTIC_SCHEMA_VERSION: &str = "sef-authz/semantic/1";

/// The editable defaults for `viewer`, `analyst`, `modeler` and `admin`.
pub const BASE_POLICIES: &str = include_str!("base.cedar");
