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
pub const RESOURCE_ID: &str = "project:catalog";

/// What the project's access configuration decided for one session.
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
    /// was taken.
    Unavailable,
}

#[must_use]
pub fn resource() -> Resource {
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
#[derive(Debug)]
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const ROLES: [&str; 6] = [
        "viewer",
        "analyst",
        "modeler",
        "admin",
        "catalog_reader",
        "otro",
    ];

    /// A project with only `policies/`: the decision never needs `metrics/`.
    fn project(members: &str, policies: Option<&str>) -> tempfile::TempDir {
        let p = tempfile::tempdir().unwrap();
        std::fs::create_dir(p.path().join("policies")).unwrap();
        std::fs::write(p.path().join("policies/members.yaml"), members).unwrap();
        if let Some(policies) = policies {
            std::fs::write(p.path().join("policies/project.cedar"), policies).unwrap();
        }
        p
    }

    fn base_with(extra: &str) -> String {
        format!(
            "{}
{extra}",
            sef_authz::BASE_POLICIES
        )
    }

    fn allowed(verdict: &Verdict) -> bool {
        matches!(verdict, Verdict::Allowed { .. })
    }

    #[test]
    fn the_resource_is_the_fixed_restricted_catalog() {
        let resource = resource();
        assert_eq!(resource.kind, ResourceKind::Model);
        assert_eq!(resource.id, "project:catalog");
        assert_eq!(
            resource.attributes,
            BTreeMap::from([(
                "classification".to_owned(),
                AttrValue::String("restricted".to_owned())
            )])
        );
        assert_eq!(resource.parents.len(), 0);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Under the base policies the catalog opens exactly to `modeler`
        /// and `admin`, whether held directly or through a mapped group;
        /// a privileged-looking identifier or an unmapped group grants
        /// nothing.
        #[test]
        fn base_policies_open_the_catalog_exactly_to_modelers_and_admins(
            user_roles in prop::collection::btree_set(prop::sample::select(&ROLES[..]), 0..3),
            group_roles in prop::collection::btree_set(prop::sample::select(&ROLES[..]), 0..3),
            mapped in any::<bool>(),
            user in prop::sample::select(&["ana", "admin", "modeler", "Role::\"admin\""][..]),
        ) {
            let list = |roles: &std::collections::BTreeSet<&str>| {
                roles.iter().copied().collect::<Vec<_>>().join(", ")
            };
            let p = project(
                &format!(
                    "version: 1
users:
  ana: [{}]
groups:
  equipo: [{}]
",
                    list(&user_roles),
                    list(&group_roles)
                ),
                None,
            );
            let groups = vec![if mapped { "equipo" } else { "otro-equipo" }.to_owned()];
            let mut held = if user == "ana" { user_roles } else { std::collections::BTreeSet::default() };
            if mapped {
                held.extend(group_roles);
            }
            let expected = held.contains("modeler") || held.contains("admin");
            let verdict = decide(p.path(), user, &groups);
            prop_assert_eq!(allowed(&verdict), expected, "{:?}", verdict);
            if !expected {
                let no_permit = matches!(verdict, Verdict::Denied { reason: "no_permit", .. });
                prop_assert!(no_permit, "{:?}", verdict);
            }
        }

        /// From an allowed configuration, invalidating any one necessary
        /// condition never allows: it denies, or closes when the access
        /// configuration cannot be used.
        #[test]
        fn invalidating_a_necessary_condition_never_allows(
            case in 0..7_usize,
            role in prop::sample::select(&["modeler", "admin"][..]),
        ) {
            let granted = format!("version: 1
users:
  ana: [{role}]
");
            let p = project(&granted, None);
            prop_assert!(allowed(&decide(p.path(), "ana", &[])));
            let forbid = r#"forbid (principal, action == Action::"view", resource == Model::"project:catalog");"#;
            let failing = "@id(\"all\")
permit (principal, action, resource);
                           @id(\"overflow\")
forbid (principal, action, resource)                            when { 9223372036854775807 + 1 > 0 };";
            let (members, policies, user, reason) = match case {
                0 => ("version: 1
users: {}
".to_owned(), None, "ana", Some("no_permit")),
                1 => ("version: 1
users:
  ana: [viewer]
".to_owned(), None, "ana", Some("no_permit")),
                2 => (granted.clone(), None, "otra", Some("no_permit")),
                3 => (granted.clone(), Some(base_with(forbid)), "ana", Some("forbidden")),
                4 => (granted.clone(), Some(failing.to_owned()), "ana", Some("evaluation_error")),
                5 => (granted.replace("version: 1", "version: 2"), None, "ana", None),
                _ => (granted.clone(), Some("permit (principal, action, resource) when { x };".to_owned()), "ana", None),
            };
            let p = project(&members, policies.as_deref());
            let verdict = decide(p.path(), user, &[]);
            match reason {
                Some(expected) => {
                    let denied = matches!(verdict, Verdict::Denied { reason, .. } if reason == expected);
                    prop_assert!(denied, "{:?}", verdict);
                }
                None => prop_assert!(matches!(verdict, Verdict::Unavailable), "{:?}", verdict),
            }
        }
    }
}
