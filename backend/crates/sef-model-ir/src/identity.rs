use crate::diagnostic::{Diagnostic, ErrorCode};
use std::fmt;

pub fn valid_segment(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct StableId(String);

impl StableId {
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let parts: Vec<_> = value.split('/').collect();
        if parts.len() != 3 || !parts.iter().all(|part| valid_segment(part)) {
            return Err(Diagnostic::error(
                ErrorCode::InvalidStableId,
                None,
                "Stable IDs require three lowercase ASCII namespace/kind/name segments",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn namespace(&self) -> &str {
        self.0.split('/').next().unwrap_or_default()
    }
    pub fn kind(&self) -> &str {
        self.0.split('/').nth(1).unwrap_or_default()
    }
}

impl fmt::Display for StableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SemanticVersion(semver::Version);

impl SemanticVersion {
    pub fn new(value: &str) -> Result<Self, Diagnostic> {
        semver::Version::parse(value).map(Self).map_err(|_| {
            Diagnostic::error(
                ErrorCode::InvalidSemanticVersion,
                None,
                "Expected SemVer 2.0.0",
            )
        })
    }
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl fmt::Display for SemanticVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectIdentity {
    pub stable_id: StableId,
    pub semantic_version: SemanticVersion,
}

/// Logical catalog token. This type never stores a URI or executable source text.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct SourceHandle(String);

impl SourceHandle {
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if !value.split('/').all(valid_segment) {
            return Err(Diagnostic::error(
                ErrorCode::InvalidSourceHandle,
                None,
                "Source handles must be logical lowercase tokens",
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
