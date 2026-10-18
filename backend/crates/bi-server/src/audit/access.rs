//! Who may read the audit trail (#158): `view_audit` on the fixed resource
//! `AuditLog::"platform:audit"`, decided with the project's own policies and
//! members, loaded again for each request.
//!
//! The action is its own: no rule written for the semantic actions reaches
//! it, not even a modeler's `view` on any resource, in this version of the
//! base policies or in a copy a project already keeps. The base grants it to
//! `admin` alone, whose rule names no action; a project grants it explicitly
//! to whom it chooses, and a `forbid` prevails over both. Nothing reads
//! events yet: this only decides (#159 and #160 read).

use std::collections::BTreeMap;
use std::path::Path;

use sef_authz::{Action, Context, PolicySnapshot, Principal, Resource, ResourceKind};

use crate::semantic::access::{self, AccessError};

/// The audit trail of this instance. Fixed: never a path, nor anything a
/// request supplies.
pub const RESOURCE_ID: &str = "platform:audit";

/// What the access configuration decided for one session.
#[derive(Debug)]
pub enum Verdict {
    Allowed {
        fingerprint: String,
    },
    Denied {
        fingerprint: String,
        /// `no_permit`, `forbidden`, `evaluation_error` or `invalid_request`.
        reason: &'static str,
    },
    /// Policies or members unreadable or invalid: closed, and no decision
    /// was taken. A grant of an earlier, valid configuration is not kept.
    Unavailable,
}

#[must_use]
pub fn resource() -> Resource {
    Resource {
        kind: ResourceKind::AuditLog,
        id: RESOURCE_ID.to_owned(),
        attributes: BTreeMap::new(),
        parents: Vec::new(),
    }
}

/// Loads the access configuration once and decides on that snapshot, so a
/// revocation holds from the next request on. Blocking: it reads
/// `policies/`.
#[must_use]
pub fn decide(project: &Path, user: &str, groups: &[String]) -> Verdict {
    let access = match access::load(project) {
        Ok(access) => access,
        Err(AccessError::Invalid(problems)) => {
            tracing::warn!(problems = problems.len(), "project policies are invalid");
            return Verdict::Unavailable;
        }
        Err(AccessError::Unreadable) => {
            tracing::warn!("project policies are unreadable");
            return Verdict::Unavailable;
        }
    };
    decide_for(
        &access.snapshot,
        &access::principal(&access.members, user, groups),
    )
}

/// Decides on a snapshot already loaded for the request.
#[must_use]
pub fn decide_for(snapshot: &PolicySnapshot, principal: &Principal) -> Verdict {
    let decision = sef_authz::authorize(
        snapshot,
        principal,
        Action::ViewAudit,
        &resource(),
        &Context::new(),
    );
    let fingerprint = decision.fingerprint.to_string();
    if decision.allowed {
        Verdict::Allowed { fingerprint }
    } else {
        Verdict::Denied {
            fingerprint,
            reason: decision.reason.code(),
        }
    }
}

#[cfg(test)]
mod tests;
