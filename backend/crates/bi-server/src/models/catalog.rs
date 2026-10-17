//! Who may inspect the project's catalog (#124): `view` on the reserved
//! resource `Model::"project:catalog"`, decided with the project's own
//! policies and members before anything of `metrics/` is read. The grant
//! covers the whole listing, restricted metadata included, and nothing of
//! the data: queries keep deciding each member on its own.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use bi_audit::ResultCode;
use sef_authz::{Action, AttrValue, Context, Resource, ResourceKind};

use crate::audit::HttpRecord;
use crate::db::Database;
use crate::semantic::access::{self, AccessError};

/// The catalog of the one project this instance serves. Fixed: never a
/// path, nor anything a request supplies.
pub(crate) const RESOURCE_ID: &str = "project:catalog";

/// What the project's access configuration decided for one session.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Allowed {
        fingerprint: String,
    },
    Denied {
        fingerprint: String,
        /// `no_permit`, `forbidden`, `evaluation_error` or `invalid_request`.
        reason: &'static str,
    },
    /// Policies or members unreadable or invalid: closed, and no decision
    /// was taken.
    Unavailable,
}

pub(crate) fn resource() -> Resource {
    Resource {
        kind: ResourceKind::Model,
        id: RESOURCE_ID.to_owned(),
        attributes: BTreeMap::from([(
            "classification".to_owned(),
            AttrValue::String("restricted".to_owned()),
        )]),
        parents: Vec::new(),
    }
}

/// Loads the project's access once and decides on that snapshot. Blocking:
/// it reads `policies/`.
pub(crate) fn decide(project: &Path, user: &str, groups: &[String]) -> Verdict {
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
    let principal = access::principal(&access.members, user, groups);
    let decision = sef_authz::authorize(
        &access.snapshot,
        &principal,
        Action::View,
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

/// What the audit records about one catalog request (#71). `Query` is a
/// query of metadata here, never an analytical one.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Event {
    action: bi_audit::Action,
    result: ResultCode,
    /// The decision's fingerprint; `None` when no decision was taken.
    fingerprint: Option<String>,
}

impl Event {
    pub(crate) const fn denied(fingerprint: String) -> Self {
        Self {
            action: bi_audit::Action::Denial,
            result: ResultCode::Denied,
            fingerprint: Some(fingerprint),
        }
    }

    /// An allowed request, ended in the listing or in an HTTP error.
    pub(crate) const fn answered(fingerprint: String, listed: bool) -> Self {
        Self {
            action: bi_audit::Action::Query,
            result: if listed {
                ResultCode::Success
            } else {
                ResultCode::Failure
            },
            fingerprint: Some(fingerprint),
        }
    }

    /// The access configuration could not be used: no decision, no
    /// fingerprint.
    pub(crate) const fn undecided() -> Self {
        Self {
            action: bi_audit::Action::Query,
            result: ResultCode::Failure,
            fingerprint: None,
        }
    }

    /// Appends the event within `budget` (see [`crate::audit::record_within`]);
    /// the error is the problem code to answer with.
    pub(crate) async fn record(
        self,
        db: &Database,
        actor: &str,
        request_id: &str,
        budget: Duration,
    ) -> Result<(), &'static str> {
        let record = HttpRecord {
            actor,
            request_id,
            action: self.action,
            result: self.result,
            resource_type: "project_catalog",
            resource_id: RESOURCE_ID,
            policy_decision_id: self.fingerprint,
        };
        crate::audit::record_within(db, record, budget).await
    }
}
