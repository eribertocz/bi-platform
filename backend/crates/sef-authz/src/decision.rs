//! What a decision says (ADR 0012 §3): whether it allows, a stable reason,
//! the policies that determined it, the constraints the engine must apply
//! and a fingerprint of everything it depended on.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::contract::{Action, AttrValue, Context, Principal, Resource};

/// Why a decision came out as it did. The codes are a stable contract for
/// audit records, diagnostics and the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reason {
    /// A `permit` applies and no `forbid` does.
    Permitted,
    /// No `permit` applies: denied by default, with no determining policy.
    NoPermit,
    /// A `forbid` applies.
    Forbidden,
    /// Some policy failed to evaluate: denied, whatever the others say.
    EvaluationError,
    /// The principal, action, resource, entities or context do not match
    /// the schema: denied before evaluating anything.
    InvalidRequest,
}

impl Reason {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Permitted => "permitted",
            Self::NoPermit => "no_permit",
            Self::Forbidden => "forbidden",
            Self::EvaluationError => "evaluation_error",
            Self::InvalidRequest => "invalid_request",
        }
    }
}

/// What the engine must do for an allowed use to stay within the decision.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Constraint {
    /// Return this object's values as typed `NULL`, flagged as masked.
    Mask {
        kind: crate::contract::ResourceKind,
        id: String,
    },
}

/// SHA-256 over a domain-separated, length-prefixed encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("sef-authz-sha256:")?;
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// One authorization decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub reason: Reason,
    /// Ids of the determining policies: the permits of an allowed decision,
    /// the forbids of a forbidden one, empty otherwise.
    pub determining: Vec<String>,
    pub constraints: Vec<Constraint>,
    /// Covers the snapshot version (policies and schema), the action, the
    /// principal, the resource and the context the decision read.
    pub fingerprint: Fingerprint,
}

/// An injective encoding: every value is tagged and length-prefixed, so no
/// two different inputs share bytes.
pub(crate) struct Encoder(Sha256);

impl Encoder {
    pub(crate) fn new(domain: &str) -> Self {
        let mut encoder = Self(Sha256::new());
        encoder.text(domain);
        encoder
    }

    pub(crate) fn text(&mut self, value: &str) {
        self.bytes(b's', value.as_bytes());
    }

    fn bytes(&mut self, tag: u8, value: &[u8]) {
        self.0.update([tag]);
        self.0.update((value.len() as u64).to_be_bytes());
        self.0.update(value);
    }

    pub(crate) fn count(&mut self, value: usize) {
        self.0.update(*b"n");
        self.0.update((value as u64).to_be_bytes());
    }

    fn value(&mut self, value: &AttrValue) {
        match value {
            AttrValue::Bool(flag) => self.bytes(b'b', &[u8::from(*flag)]),
            AttrValue::Long(number) => self.bytes(b'l', &number.to_be_bytes()),
            AttrValue::String(text) => self.text(text),
            AttrValue::Set(items) => {
                self.count(items.len());
                for item in items {
                    self.value(item);
                }
            }
        }
    }

    pub(crate) fn attributes<'a>(
        &mut self,
        attributes: impl ExactSizeIterator<Item = (&'a String, &'a AttrValue)>,
    ) {
        self.count(attributes.len());
        for (name, value) in attributes {
            self.text(name);
            self.value(value);
        }
    }

    pub(crate) fn principal(&mut self, principal: &Principal) {
        self.text(&principal.id);
        self.count(principal.roles.len());
        for role in &principal.roles {
            self.text(role);
        }
        self.attributes(principal.attributes.iter());
    }

    pub(crate) fn resource(&mut self, resource: &Resource) {
        self.text(resource.kind.entity_type());
        self.text(&resource.id);
        self.attributes(resource.attributes.iter());
        self.count(resource.parents.len());
        for parent in &resource.parents {
            self.resource(parent);
        }
    }

    pub(crate) fn request(
        &mut self,
        snapshot: &str,
        principal: &Principal,
        action: Action,
        resource: &Resource,
        context: &Context,
    ) {
        self.text(snapshot);
        self.text(action.id());
        self.principal(principal);
        self.resource(resource);
        self.attributes(context.iter());
    }

    pub(crate) fn fingerprint(&mut self, value: &Fingerprint) {
        self.bytes(b'f', &value.0);
    }

    pub(crate) fn finish(self) -> Fingerprint {
        Fingerprint(self.0.finalize().into())
    }
}
