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

#[derive(Default)]
pub struct Verifier {
    checkpoint: Checkpoint,
}

impl Verifier {
    /// Feed every row in order, starting at sequence 1. Memory stays constant.
    /// # Errors
    /// Rejects gaps, reordered/injected rows, altered payloads/links and schemas.
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
        let event: Event = serde_json::from_str(payload).map_err(|_| Error::InvalidEvent)?;
        if event.canonical_json()? != payload {
            return Err(Error::InvalidEvent);
        }
        self.checkpoint = Checkpoint {
            sequence,
            hash: *hash,
        };
        Ok(())
    }

    /// # Errors
    /// Detects deletion of the tail (including truncation of the entire table).
    pub fn finish(self, expected: Checkpoint) -> Result<Checkpoint, Error> {
        if self.checkpoint != expected {
            return Err(Error::HeadMismatch);
        }
        Ok(expected)
    }
}
