//! Explicit, versioned canonical values; Rust/Serde layouts never define IR bytes.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use ciborium::value::Value;
use sha2::{Digest, Sha256};

pub const CANONICAL_FORMAT: &str = "sef-canonical-value-v1";
pub const FINGERPRINT_PROFILE: &str = "sef-dcbor-sha256-v1";
pub const SEMANTIC_PROFILE: &str = "0.1";
pub const IR_PROFILE: &str = "0.1";
pub const MAX_CANONICAL_DEPTH: usize = 128;
pub const MAX_CANONICAL_NODES: usize = 100_000;
pub const MAX_CANONICAL_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Domain identifiers reserve separation for later IRs, not their implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Domain {
    Csmir,
    Sqir,
    Seir,
}

impl Domain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Csmir => "CSMIR",
            Self::Sqir => "SQIR",
            Self::Seir => "SEIR",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CanonicalValue {
    Null,
    Bool(bool),
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Bytes(Vec<u8>),
    Text(String),
    Sequence(Vec<Self>),
    Set(Vec<Self>),
    Map(Vec<(String, Self)>),
    Variant {
        tag: String,
        fields: Vec<(String, Self)>,
    },
    Absent,
    Present(Box<Self>),
}

/// Every domain implementation names its own stable fields and union tags.
pub trait Canonical {
    fn canonical_value(&self) -> CanonicalValue;
}

impl CanonicalValue {
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    pub fn record<K: Into<String>>(
        tag: impl Into<String>,
        fields: impl IntoIterator<Item = (K, Self)>,
    ) -> Self {
        Self::Variant {
            tag: tag.into(),
            fields: fields.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }

    pub fn sequence<T: Canonical>(values: impl IntoIterator<Item = T>) -> Self {
        Self::Sequence(values.into_iter().map(|v| v.canonical_value()).collect())
    }

    pub fn set<T: Canonical>(values: impl IntoIterator<Item = T>) -> Self {
        Self::Set(values.into_iter().map(|v| v.canonical_value()).collect())
    }

    pub fn optional<T: Canonical + ?Sized>(value: Option<&T>) -> Self {
        match value {
            None => Self::Absent,
            Some(value) => Self::Present(Box::new(value.canonical_value())),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, CanonicalError> {
        let mut budget = Budget::default();
        let value = self.to_cbor(0, &mut budget)?;
        encode_cbor(&value)
    }

    pub fn fingerprint(&self, domain: Domain) -> Result<Fingerprint, CanonicalError> {
        fingerprint(domain, self)
    }

    fn to_cbor(&self, depth: usize, budget: &mut Budget) -> Result<Value, CanonicalError> {
        budget.visit(depth)?;
        let (tag, payload) = match self {
            Self::Null => return Ok(Value::Array(vec![integer(0)])),
            Self::Bool(value) => (1, Value::Bool(*value)),
            Self::Signed(value) => (2, Value::Integer((*value).into())),
            Self::Unsigned(value) => (3, integer(*value)),
            Self::Float(value) => {
                if !value.is_finite() {
                    return Err(CanonicalError::NonFiniteFloat);
                }
                // Float equality makes both signs of zero one canonical value.
                (4, Value::Float(if *value == 0.0 { 0.0 } else { *value }))
            }
            Self::Bytes(value) => {
                budget.payload(value.len())?;
                (5, Value::Bytes(value.clone()))
            }
            Self::Text(value) => {
                budget.payload(value.len())?;
                (6, Value::Text(value.clone()))
            }
            Self::Sequence(values) => {
                let values = values
                    .iter()
                    .map(|value| value.to_cbor(depth + 1, budget))
                    .collect::<Result<Vec<_>, _>>()?;
                (7, Value::Array(values))
            }
            Self::Set(values) => {
                let mut items = Vec::new();
                for value in values {
                    let value = value.to_cbor(depth + 1, budget)?;
                    let bytes = encode_cbor(&value)?;
                    let hash: [u8; 32] = Sha256::digest(&bytes).into();
                    items.push((hash, bytes, value));
                }
                items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
                items.dedup_by(|a, b| a.1 == b.1);
                (
                    8,
                    Value::Array(items.into_iter().map(|(_, _, v)| v).collect()),
                )
            }
            Self::Map(fields) => (9, canonical_map(fields, depth, budget)?),
            Self::Variant { tag, fields } => {
                if !valid_variant_tag(tag) {
                    return Err(CanonicalError::InvalidVariantTag(tag.clone()));
                }
                budget.payload(tag.len())?;
                return Ok(Value::Array(vec![
                    integer(10),
                    Value::Text(tag.clone()),
                    canonical_map(fields, depth, budget)?,
                ]));
            }
            Self::Absent => return Ok(Value::Array(vec![integer(11)])),
            Self::Present(value) => (12, value.to_cbor(depth + 1, budget)?),
        };
        Ok(Value::Array(vec![integer(tag), payload]))
    }
}

fn valid_variant_tag(tag: &str) -> bool {
    tag.split('.').all(|part| {
        let mut bytes = part.bytes();
        bytes
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
            && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    })
}

fn integer(value: u64) -> Value {
    Value::Integer(value.into())
}

fn canonical_map(
    fields: &[(String, CanonicalValue)],
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, CanonicalError> {
    let mut seen = BTreeSet::new();
    let mut ordered = Vec::new();
    for (key, value) in fields {
        budget.payload(key.len())?;
        if !seen.insert(key) {
            return Err(CanonicalError::DuplicateMapKey(key.clone()));
        }
        let key = Value::Text(key.clone());
        ordered.push((encode_cbor(&key)?, key, value.to_cbor(depth + 1, budget)?));
    }
    // RFC 8949 section 4.2.1 orders the encoded key bytes, not Rust strings.
    ordered.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Value::Map(
        ordered
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect(),
    ))
}

fn encode_cbor(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes)
        .map_err(|error| CanonicalError::Encoding(error.to_string()))?;
    Ok(bytes)
}

#[derive(Default)]
struct Budget {
    nodes: usize,
    payload_bytes: usize,
}

impl Budget {
    fn visit(&mut self, depth: usize) -> Result<(), CanonicalError> {
        if depth > MAX_CANONICAL_DEPTH {
            return Err(CanonicalError::ResourceLimit("depth"));
        }
        self.nodes += 1;
        if self.nodes > MAX_CANONICAL_NODES {
            return Err(CanonicalError::ResourceLimit("nodes"));
        }
        Ok(())
    }

    fn payload(&mut self, bytes: usize) -> Result<(), CanonicalError> {
        self.payload_bytes = self
            .payload_bytes
            .checked_add(bytes)
            .ok_or(CanonicalError::ResourceLimit("payload_bytes"))?;
        if self.payload_bytes > MAX_CANONICAL_PAYLOAD_BYTES {
            return Err(CanonicalError::ResourceLimit("payload_bytes"));
        }
        Ok(())
    }
}

/// The exact bytes hashed by `sef-dcbor-sha256-v1` (not only the inner value).
pub fn fingerprint_bytes<T: Canonical + ?Sized>(
    domain: Domain,
    value: &T,
) -> Result<Vec<u8>, CanonicalError> {
    let mut budget = Budget::default();
    let payload = value.canonical_value().to_cbor(0, &mut budget)?;
    encode_cbor(&Value::Array(vec![
        Value::Text(CANONICAL_FORMAT.into()),
        Value::Text(FINGERPRINT_PROFILE.into()),
        Value::Text(domain.as_str().into()),
        Value::Text(SEMANTIC_PROFILE.into()),
        Value::Text(IR_PROFILE.into()),
        payload,
    ]))
}

pub fn fingerprint<T: Canonical + ?Sized>(
    domain: Domain,
    value: &T,
) -> Result<Fingerprint, CanonicalError> {
    let bytes = fingerprint_bytes(domain, value)?;
    Ok(Fingerprint(Sha256::digest(bytes).into()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("sef-sha256-v1:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for Fingerprint {
    type Err = CanonicalError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let raw = value
            .strip_prefix("sef-sha256-v1:")
            .ok_or(CanonicalError::InvalidFingerprint)?;
        if raw.len() != 64
            || !raw
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(CanonicalError::InvalidFingerprint);
        }
        let mut bytes = [0; 32];
        for (index, chunk) in raw.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
            bytes[index] = (digit(chunk[0]) << 4) | digit(chunk[1]);
        }
        Ok(Self(bytes))
    }
}

impl Canonical for CanonicalValue {
    fn canonical_value(&self) -> CanonicalValue {
        self.clone()
    }
}

impl<T: Canonical + ?Sized> Canonical for &T {
    fn canonical_value(&self) -> CanonicalValue {
        (*self).canonical_value()
    }
}

impl Canonical for str {
    fn canonical_value(&self) -> CanonicalValue {
        CanonicalValue::Text(self.into())
    }
}

impl Canonical for String {
    fn canonical_value(&self) -> CanonicalValue {
        self.as_str().canonical_value()
    }
}

impl Canonical for bool {
    fn canonical_value(&self) -> CanonicalValue {
        CanonicalValue::Bool(*self)
    }
}

impl Canonical for i64 {
    fn canonical_value(&self) -> CanonicalValue {
        CanonicalValue::Signed(*self)
    }
}

macro_rules! unsigned_canonical {
    ($($ty:ty),+ $(,)?) => {$(
        impl Canonical for $ty {
            fn canonical_value(&self) -> CanonicalValue {
                CanonicalValue::Unsigned(u64::from(*self))
            }
        }
    )+};
}

unsigned_canonical!(u8, u16, u32, u64);

impl<T: Canonical> Canonical for Option<T> {
    fn canonical_value(&self) -> CanonicalValue {
        CanonicalValue::optional(self.as_ref())
    }
}

// Vec is an ordered sequence by default. Set-like domain fields explicitly use set().
impl<T: Canonical> Canonical for Vec<T> {
    fn canonical_value(&self) -> CanonicalValue {
        CanonicalValue::sequence(self)
    }
}

impl Canonical for Fingerprint {
    fn canonical_value(&self) -> CanonicalValue {
        CanonicalValue::text(self.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalError {
    DuplicateMapKey(String),
    InvalidVariantTag(String),
    NonFiniteFloat,
    InvalidFingerprint,
    ResourceLimit(&'static str),
    Encoding(String),
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateMapKey(key) => write!(f, "duplicate canonical map key: {key}"),
            Self::InvalidVariantTag(tag) => write!(f, "invalid canonical variant tag: {tag}"),
            Self::NonFiniteFloat => f.write_str("canonical float must be finite"),
            Self::InvalidFingerprint => f.write_str("invalid sef-sha256-v1 fingerprint"),
            Self::ResourceLimit(resource) => write!(f, "canonical resource limit: {resource}"),
            Self::Encoding(message) => write!(f, "canonical CBOR encoding failed: {message}"),
        }
    }
}

impl std::error::Error for CanonicalError {}
