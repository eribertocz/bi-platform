//! Audit v1 contract and SHA-256 verification. No transport, storage or runtime.
//! IDs are opaque references, not names, credentials or session bearer tokens.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: u32 = 1;
pub const GENESIS: [u8; 32] = [0; 32];
/// Hash framing: domain, positive sequence as 8-byte big endian, previous hash,
/// then the exact UTF-8 bytes returned by `Event::canonical_json`.
pub const HASH_DOMAIN: &[u8] = b"bi-platform.audit.v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorType {
    User,
    Service,
    Anonymous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Login,
    Logout,
    Query,
    Denial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultCode {
    Success,
    Failure,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Http,
    Cli,
    Internal,
}

/// Allowlist by construction: only counts, never SQL, filter values, headers,
/// credentials, emails, names, error text or arbitrary JSON/string properties.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataRedacted {
    #[serde(deserialize_with = "Option::deserialize")]
    pub query_shape: Option<QueryShape>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryShape {
    pub dimensions: u32,
    pub measures: u32,
    pub filters: u32,
    pub limit: u32,
}

/// Versioned input contract. The producer supplies server-derived references.
/// Anonymous authentication attempts have no session; a policy decision may be
/// absent until the permission engine exists (ADR 0010).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub actor_id: String,
    pub actor_type: ActorType,
    #[serde(deserialize_with = "Option::deserialize")]
    pub session_id: Option<String>,
    pub action: Action,
    pub resource_type: String,
    pub resource_id: String,
    pub result: ResultCode,
    pub request_id: String,
    pub trace_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub policy_decision_id: Option<String>,
    pub source: Source,
    pub metadata_redacted: MetadataRedacted,
    pub schema_version: u32,
}

/// Stable, redacted errors: never include the invalid input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidEvent,
    SequenceMismatch,
    PreviousHashMismatch,
    HashMismatch,
    HeadMismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidEvent => "audit_invalid_event",
            Self::SequenceMismatch => "audit_sequence_mismatch",
            Self::PreviousHashMismatch => "audit_previous_hash_mismatch",
            Self::HashMismatch => "audit_hash_mismatch",
            Self::HeadMismatch => "audit_head_mismatch",
        })
    }
}

impl std::error::Error for Error {}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

impl Event {
    /// # Errors
    /// Rejects unsupported versions, missing/bad references and invalid trace IDs.
    pub fn validate(&self) -> Result<(), Error> {
        let required = [
            &self.event_id,
            &self.actor_id,
            &self.resource_type,
            &self.resource_id,
            &self.request_id,
        ];
        if self.schema_version != SCHEMA_VERSION
            || required.iter().any(|v| !identifier(v))
            || self.session_id.as_deref().is_some_and(|v| !identifier(v))
            || self
                .policy_decision_id
                .as_deref()
                .is_some_and(|v| !identifier(v))
            || self.trace_id.len() != 32
            || !self
                .trace_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.trace_id.bytes().all(|b| b == b'0')
        {
            return Err(Error::InvalidEvent);
        }
        Ok(())
    }

    /// V1 uses compact JSON in declaration order, including explicit nulls.
    /// Stored bytes are preserved verbatim, never round-tripped through JSONB.
    /// # Errors
    /// Returns `InvalidEvent` for malformed input or serialization failure.
    pub fn canonical_json(&self) -> Result<String, Error> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| Error::InvalidEvent)
    }
}

#[must_use]
pub fn event_hash(sequence: u64, previous: &[u8; 32], canonical_json: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(HASH_DOMAIN);
    hash.update(sequence.to_be_bytes());
    hash.update(previous);
    hash.update(canonical_json.as_bytes());
    hash.finalize().into()
}

/// A trusted checkpoint can be kept outside the database to detect rewriting
/// of the entire history by a database owner. A head in the same DB detects
/// accidental/isolated tail deletions, not a malicious owner replacing both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Checkpoint {
    pub sequence: u64,
    pub hash: [u8; 32],
}

/// Whether a stored payload is exactly what a well-formed producer writes:
/// it parses, every field is in range, and it serializes back byte-for-byte
/// (catches anything that reached storage before this contract, or by a
/// path this crate never validated).
fn content_valid(payload: &str) -> bool {
    serde_json::from_str::<Event>(payload)
        .is_ok_and(|event| event.canonical_json().as_deref() == Ok(payload))
}

#[derive(Default)]
pub struct Verifier {
    checkpoint: Checkpoint,
    invalid_content_count: u64,
    first_invalid_content_sequence: Option<u64>,
}

impl Verifier {
    /// Feed every row in order, starting at sequence 1. Memory stays constant.
    ///
    /// Cryptographic integrity (sequence, link, hash) and content validity are
    /// independent: a row with a well-formed hash chain but invalid content
    /// (written before this contract, or by a path that skipped validation)
    /// does **not** stop the chain here. It is counted
    /// ([`Self::invalid_content_count`]) and the walk continues, so a later
    /// cryptographic break is still detected and reported with priority over
    /// an earlier content defect. [`Self::finish`] stays cryptographic-only;
    /// a caller that must also require valid content checks
    /// [`Self::invalid_content_count`] after a successful `finish`.
    ///
    /// # Errors
    /// Rejects gaps, reordered/injected rows and altered payloads/links.
    pub fn push(
        &mut self,
        sequence: u64,
        payload: &str,
        previous: &[u8; 32],
        hash: &[u8; 32],
    ) -> Result<(), Error> {
        if self.checkpoint.sequence.checked_add(1) != Some(sequence) {
            return Err(Error::SequenceMismatch);
        }
        if previous != &self.checkpoint.hash {
            return Err(Error::PreviousHashMismatch);
        }
        if &event_hash(sequence, previous, payload) != hash {
            return Err(Error::HashMismatch);
        }
        self.checkpoint = Checkpoint {
            sequence,
            hash: *hash,
        };
        if !content_valid(payload) {
            self.invalid_content_count += 1;
            self.first_invalid_content_sequence.get_or_insert(sequence);
        }
        Ok(())
    }

    /// The cryptographic state after every row pushed so far, whether or not
    /// their content was valid.
    #[must_use]
    pub const fn checkpoint(&self) -> Checkpoint {
        self.checkpoint
    }

    /// How many pushed rows had invalid content (zero means every row seen
    /// so far is a well-formed `Event`, not that the whole history is clean:
    /// a caller must still reach the expected head to know that).
    #[must_use]
    pub const fn invalid_content_count(&self) -> u64 {
        self.invalid_content_count
    }

    /// The sequence of the first row with invalid content, if any.
    #[must_use]
    pub const fn first_invalid_content_sequence(&self) -> Option<u64> {
        self.first_invalid_content_sequence
    }

    /// # Errors
    /// Detects deletion of the tail (including truncation of the entire
    /// table). Cryptographic-only: a cryptographically complete chain with
    /// invalid content still succeeds here; check
    /// [`Self::invalid_content_count`] separately when content must be valid
    /// too.
    pub fn finish(self, expected: Checkpoint) -> Result<Checkpoint, Error> {
        if self.checkpoint != expected {
            return Err(Error::HeadMismatch);
        }
        Ok(expected)
    }
}
