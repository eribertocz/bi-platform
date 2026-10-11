//! Ossie names as CSMIR stable-ID segments.
//!
//! A CSMIR segment is lowercase ASCII (`[a-z0-9][a-z0-9._-]*`); an Ossie
//! name is any identifier, compared after normalization (unquoted names
//! upper-cased, quoted names exact). The encoding is **injective** over
//! normalized names, so two different names never share an ID:
//! - `A`–`Z` become `a`–`z`; digits and `_` stay;
//! - any other character becomes `-<hex code point>-` (`región` →
//!   `regi-f3-n`), which no plain character can produce;
//! - a result that does not start with a letter or digit gets the prefix
//!   `x-` (`_tmp` → `x-_tmp`); a plain encoding never contains `x--` or
//!   `x-_`, and an escape after a leading `x` is always `x-<hex digit>`.
//!
//! IDs are internal: people see names and labels, never these segments.

use std::fmt::Write as _;

use sef_model_ir::identity::{ObjectIdentity, SemanticVersion, StableId};

use crate::validate::normalize_identifier;

/// The segment of a declared Ossie name.
#[must_use]
pub fn segment(name: &str) -> String {
    encode(&normalize_identifier(name))
}

/// The segment of an already normalized identifier.
#[must_use]
pub fn encode(normalized: &str) -> String {
    let mut encoded = String::with_capacity(normalized.len());
    for c in normalized.chars() {
        match c {
            'A'..='Z' => encoded.push(c.to_ascii_lowercase()),
            '0'..='9' | '_' => encoded.push(c),
            _ => {
                // Writing to a String cannot fail.
                let _ = write!(encoded, "-{:x}-", u32::from(c));
            }
        }
    }
    if encoded.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit()) {
        encoded
    } else {
        format!("x-{encoded}")
    }
}

/// Builds the stable IDs of one model: `<namespace>/<kind>/<name>`.
#[derive(Debug, Clone)]
pub(crate) struct Ids {
    namespace: String,
    version: SemanticVersion,
}

impl Ids {
    pub(crate) fn new(namespace: String, version: SemanticVersion) -> Self {
        Self { namespace, version }
    }

    /// Callers pass segments built by [`encode`] (or `.`-joined pairs of
    /// them), which are always valid, so a failure is a bug: it surfaces as
    /// an error, never a panic.
    pub(crate) fn id(&self, kind: &str, name: &str) -> Result<StableId, String> {
        StableId::new(format!("{}/{kind}/{name}", self.namespace))
            .map_err(|_| format!("{}/{kind}/{name}", self.namespace))
    }

    pub(crate) fn identity(&self, kind: &str, name: &str) -> Result<ObjectIdentity, String> {
        Ok(ObjectIdentity {
            stable_id: self.id(kind, name)?,
            semantic_version: self.version.clone(),
        })
    }
}
