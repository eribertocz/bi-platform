//! The continuation token of a page walk (#160): opaque to its holder and
//! authenticated with HMAC-SHA-256 under a key of this instance, so it can
//! be neither forged nor edited. Authenticated, not encrypted: it carries
//! only positions in the chain, an expiry and a keyed fingerprint of the
//! filters, nothing a holder may not see.
//!
//! Version 1, before base64url without padding (89 bytes, 119 characters):
//!
//! | bytes | field                                                  |
//! |-------|--------------------------------------------------------|
//! | 1     | version, `1`                                           |
//! | 8     | last sequence returned, big endian                     |
//! | 8     | upper bound captured on the first page, big endian     |
//! | 8     | expiry, Unix milliseconds, big endian                  |
//! | 32    | HMAC of the normalized filters and limit               |
//! | 32    | HMAC of everything above                               |

use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::PageError;

/// The only version this build issues and accepts.
const VERSION: u8 = 1;
/// Everything the tag covers.
const BODY: usize = 1 + 8 + 8 + 8 + 32;
const TOKEN: usize = BODY + 32;
/// The longest token accepted, before anything is decoded.
pub const MAX_ENCODED: usize = 1024;

/// Each use of the key has its own domain: a fingerprint is never a valid
/// tag, nor a tag of another purpose.
const TAG_DOMAIN: &[u8] = b"bi-audit-page-cursor-v1\0";
const FINGERPRINT_DOMAIN: &[u8] = b"bi-audit-page-scope-v1\0";

/// The secret that authenticates page cursors, born with the instance and
/// never stored, logged or returned: a restart invalidates every cursor.
/// Its own key, never the semantic context's.
#[derive(Clone)]
pub struct CursorKey(Arc<[u8; 32]>);

impl CursorKey {
    /// A new secret from the operating system's entropy; `None` without
    /// it: there is no fallback to a constant or an empty key.
    #[must_use]
    pub fn generate() -> Option<Self> {
        Self::from_entropy(getrandom::fill)
    }

    pub(crate) fn from_entropy(
        fill: impl FnOnce(&mut [u8]) -> Result<(), getrandom::Error>,
    ) -> Option<Self> {
        let mut bytes = [0_u8; 32];
        fill(&mut bytes).ok().map(|()| Self(Arc::new(bytes)))
    }

    /// A known secret, for reproducible tests; only in the crate's tests.
    #[cfg(test)]
    pub(crate) fn fixed(bytes: [u8; 32]) -> Self {
        Self(Arc::new(bytes))
    }

    fn mac(&self, domain: &[u8]) -> Hmac<Sha256> {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(self.0.as_slice()).expect("HMAC takes any key length");
        mac.update(domain);
        mac
    }

    fn fingerprint(&self, scope: &[u8]) -> Hmac<Sha256> {
        let mut mac = self.mac(FINGERPRINT_DOMAIN);
        mac.update(scope);
        mac
    }

    /// The token continuing after `position`, bound to `scope`.
    pub(super) fn seal(&self, position: Position, scope: &[u8]) -> String {
        let mut token = Vec::with_capacity(TOKEN);
        token.push(VERSION);
        token.extend_from_slice(&position.last.to_be_bytes());
        token.extend_from_slice(&position.upper.to_be_bytes());
        token.extend_from_slice(&position.expires_ms.to_be_bytes());
        token.extend_from_slice(&self.fingerprint(scope).finalize().into_bytes());
        let mut tag = self.mac(TAG_DOMAIN);
        tag.update(&token);
        token.extend_from_slice(&tag.finalize().into_bytes());
        URL_SAFE_NO_PAD.encode(token)
    }

    /// The position `token` continues from, if this key issued it for
    /// `scope` and it has not expired at `now_ms`. Nothing of the token is
    /// part of the error.
    ///
    /// # Errors
    /// [`PageError::CursorInvalid`] when it is malformed, edited or issued
    /// under another key (indistinguishable by design);
    /// [`PageError::CursorVersion`], [`PageError::CursorExpired`] and
    /// [`PageError::CursorMismatch`] (other filters or limit).
    pub(super) fn open(
        &self,
        token: &str,
        scope: &[u8],
        now_ms: u64,
    ) -> Result<Position, PageError> {
        if token.len() > MAX_ENCODED {
            return Err(PageError::CursorInvalid);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| PageError::CursorInvalid)?;
        match bytes.first() {
            None => return Err(PageError::CursorInvalid),
            Some(&VERSION) => {}
            Some(_) => return Err(PageError::CursorVersion),
        }
        if bytes.len() != TOKEN {
            return Err(PageError::CursorInvalid);
        }
        let (body, tag) = bytes.split_at(BODY);
        let mut expected = self.mac(TAG_DOMAIN);
        expected.update(body);
        expected
            .verify_slice(tag)
            .map_err(|_| PageError::CursorInvalid)?;
        let field = |at: usize| {
            u64::from_be_bytes(body[at..at + 8].try_into().expect("eight bytes in range"))
        };
        let position = Position {
            last: field(1),
            upper: field(9),
            expires_ms: field(17),
        };
        // Only ever issued while more rows remained below the bound.
        if position.last >= position.upper {
            return Err(PageError::CursorInvalid);
        }
        if now_ms >= position.expires_ms {
            return Err(PageError::CursorExpired);
        }
        self.fingerprint(scope)
            .verify_slice(&body[25..BODY])
            .map_err(|_| PageError::CursorMismatch)?;
        Ok(position)
    }
}

/// Never the secret.
impl fmt::Debug for CursorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CursorKey(<redacted>)")
    }
}

/// Where a walk stands: after `last`, up to `upper`, until `expires_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Position {
    pub last: u64,
    pub upper: u64,
    pub expires_ms: u64,
}

#[cfg(test)]
pub(super) mod layout {
    //! The token's layout, for tests that edit one field at a time.
    pub const VERSION_AT: usize = 0;
    pub const LAST_AT: usize = 1;
    pub const TAG_AT: usize = super::BODY;
    pub const TOKEN: usize = super::TOKEN;
    pub const TAG_DOMAIN: &[u8] = super::TAG_DOMAIN;
}
