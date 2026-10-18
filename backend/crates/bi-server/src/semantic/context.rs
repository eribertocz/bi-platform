//! The public fingerprint of a query's context (#204): an HMAC-SHA-256 under
//! a secret of this instance. A plain hash of the row-policy values is not
//! published: with few possible values (`Norte`, `Sur`) a dictionary would
//! reverse it.
//!
//! The key is born with the instance and never stored, logged or returned,
//! so two fingerprints compare only within one instance and its key epoch:
//! a restart changes them all.

use std::fmt;
use std::sync::Arc;

use hmac::{Hmac, KeyInit, Mac};
use sef_correctness::SemanticContext;
use sef_model_ir::canonical::{Canonical, CanonicalValue as C};

/// What the fingerprint is for, and its version: another use of the same
/// key never yields the same value.
const DOMAIN: &[u8] = b"bi-explain-context-v1";
/// Names the algorithm and version inside the opaque value.
const PREFIX: &str = "ctx-hmac-sha256-v1:";

/// The secret of this instance, shared by its requests. Without entropy at
/// birth it stays unavailable: nothing falls back to a plain hash, a
/// constant or an empty key.
#[derive(Clone)]
pub struct ContextKey(Option<Arc<[u8; 32]>>);

impl ContextKey {
    /// A new secret from the operating system's entropy.
    #[must_use]
    pub fn generate() -> Self {
        Self::from_entropy(getrandom::fill)
    }

    pub(crate) fn from_entropy(
        fill: impl FnOnce(&mut [u8]) -> Result<(), getrandom::Error>,
    ) -> Self {
        let mut bytes = [0_u8; 32];
        Self(fill(&mut bytes).ok().map(|()| Arc::new(bytes)))
    }

    /// A known secret, for reproducible tests; only in the crate's tests.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn fixed(bytes: [u8; 32]) -> Self {
        Self(Some(Arc::new(bytes)))
    }

    #[must_use]
    pub const fn is_available(&self) -> bool {
        self.0.is_some()
    }

    /// The fingerprint of `context` for `scope`: the same within this key
    /// for the same project, principal and values, and different when any
    /// of them, a value's type or the key changes. `None` without a key, or
    /// for a context with no canonical form.
    #[must_use]
    pub(crate) fn fingerprint(
        &self,
        scope: &Scope<'_>,
        context: &SemanticContext,
    ) -> Option<String> {
        let slots = C::Map(
            context
                .slots
                .iter()
                .map(|(name, value)| (name.clone(), value.canonical_value()))
                .collect(),
        );
        self.mac(DOMAIN, PREFIX, scope, ("slots", slots))
    }

    /// A commitment to evidence kept private (#146): binds it, under the
    /// same key and scope, without revealing it; another use of the key
    /// never yields the same value.
    #[must_use]
    pub(crate) fn commit(&self, scope: &Scope<'_>, evidence: C) -> Option<String> {
        self.mac(
            b"bi-run-evidence-v1",
            "ev-hmac-sha256-v1:",
            scope,
            ("evidence", evidence),
        )
    }

    /// The public form of a decision's fingerprint (#146): that one covers
    /// the principal and its roles, and a guess of them would reproduce it.
    #[must_use]
    pub(crate) fn decision(&self, scope: &Scope<'_>, fingerprint: &str) -> Option<String> {
        self.mac(
            b"bi-run-decision-v1",
            "dec-hmac-sha256-v1:",
            scope,
            ("decision", C::text(fingerprint)),
        )
    }

    fn mac(
        &self,
        domain: &[u8],
        prefix: &str,
        scope: &Scope<'_>,
        (name, value): (&str, C),
    ) -> Option<String> {
        let key = self.0.as_deref()?;
        let encoded = C::Map(vec![
            ("project".to_owned(), C::text(scope.project)),
            ("principal".to_owned(), C::text(scope.principal)),
            (name.to_owned(), value),
        ])
        .encode()
        .ok()?;
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key).ok()?;
        mac.update(domain);
        mac.update(&encoded);
        Some(format!("{prefix}{}", hex(&mac.finalize().into_bytes())))
    }
}

/// Never the secret.
impl fmt::Debug for ContextKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.is_available() {
            "ContextKey(available)"
        } else {
            "ContextKey(unavailable)"
        })
    }
}

/// Who and where a context was bound: the project and the verified
/// principal, never taken from the request: only this crate builds one.
pub struct Scope<'a> {
    pub(crate) project: &'a str,
    pub(crate) principal: &'a str,
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

#[cfg(test)]
mod tests;
