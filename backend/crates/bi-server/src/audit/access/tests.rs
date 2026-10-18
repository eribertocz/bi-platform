//! Who reads the audit trail, on real policy files and members: the base,
//! explicit grants, rules written for other actions, `forbid`, broken
//! configurations, revocation, and that nothing else changed.

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use sef_authz::{AttrValue, PolicySource};

use super::*;
use crate::semantic::access::PLATFORM_SCHEMA;

const ROLES: [&str; 6] = [
    "viewer",
    "analyst",
    "modeler",
    "admin",
    "auditor_reader",
    "otro",
];

const AUDITOR: &str = r#"
@id("auditor.read")
permit (
  principal in Role::"auditor_reader",
  action == Action::"view_audit",
  resource == AuditLog::"platform:audit"
);
"#;

/// A project with only `policies/`: the decision never reads anything else.
fn project(members: &str, policies: &[(&str, &str)]) -> tempfile::TempDir {
    let p = tempfile::tempdir().unwrap();
    std::fs::create_dir(p.path().join("policies")).unwrap();
    std::fs::write(p.path().join("policies/members.yaml"), members).unwrap();
    for (name, text) in policies {
        std::fs::write(p.path().join("policies").join(name), text).unwrap();
    }
    p
}

/// `members.yaml` v1 with these users and groups, each with its roles.
fn members(users: &[(&str, &[&str])], groups: &[(&str, &[&str])]) -> String {
    use std::fmt::Write as _;
    let section = |text: &mut String, title: &str, entries: &[(&str, &[&str])]| {
        writeln!(text, "{title}:").unwrap();
        for (name, roles) in entries {
            writeln!(text, "  {name}: [{}]", roles.join(", ")).unwrap();
        }
    };
    let mut text = String::from("version: 1\n");
    if users.is_empty() {
        text.push_str("users: {}\n");
    } else {
        section(&mut text, "users", users);
    }
    if !groups.is_empty() {
        section(&mut text, "groups", groups);
    }
    text
}

fn with_base(extra: &str) -> String {
    format!("{}\n{extra}", sef_authz::BASE_POLICIES)
}

fn reason(verdict: &Verdict) -> Option<&'static str> {
    match verdict {
        Verdict::Allowed { .. } => None,
        Verdict::Denied { reason, .. } => Some(reason),
        Verdict::Unavailable => Some("unavailable"),
    }
}

#[test]
fn the_resource_is_the_fixed_audit_trail() {
    let resource = resource();
    assert_eq!(resource.kind, ResourceKind::AuditLog);
    assert_eq!(resource.kind.entity_type(), "AuditLog");
    assert_eq!(Action::ViewAudit.id(), "view_audit");
    assert_eq!(resource.id, "platform:audit");
    assert!(resource.attributes.is_empty() && resource.parents.is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Under the base policies the audit opens to `admin` alone, held
    /// directly or through a mapped group. A modeler, an analyst, a viewer,
    /// an unmapped group or a privileged-looking identifier gets nothing.
    #[test]
    fn base_policies_open_the_audit_only_to_admins(
        user_roles in prop::collection::btree_set(prop::sample::select(&ROLES[..]), 0..3),
        group_roles in prop::collection::btree_set(prop::sample::select(&ROLES[..]), 0..3),
        mapped in any::<bool>(),
        user in prop::sample::select(&["ana", "admin", "auditor_reader", "Role::\"admin\""][..]),
    ) {
        let user_list: Vec<&str> = user_roles.iter().copied().collect();
        let group_list: Vec<&str> = group_roles.iter().copied().collect();
        let p = project(&members(&[("ana", &user_list)], &[("equipo", &group_list)]), &[]);
        let groups = vec![if mapped { "equipo" } else { "otro-equipo" }.to_owned()];
        let mut held = if user == "ana" { user_roles } else { BTreeSet::new() };
        if mapped {
            held.extend(group_roles);
        }
        let verdict = decide(p.path(), user, &groups);
        let expected = if held.contains("admin") { None } else { Some("no_permit") };
        prop_assert_eq!(reason(&verdict), expected, "{:?}", verdict);
    }

    /// With an explicit grant, exactly admins and the granted role read it.
    #[test]
    fn an_explicit_grant_opens_it_to_its_role_only(
        roles in prop::collection::btree_set(prop::sample::select(&ROLES[..]), 0..3),
    ) {
        let list: Vec<&str> = roles.iter().copied().collect();
        let p = project(
            &members(&[("ana", &list)], &[]),
            &[("project.cedar", &with_base(AUDITOR))],
        );
        let verdict = decide(p.path(), "ana", &[]);
        let expected = if roles.contains("admin") || roles.contains("auditor_reader") {
            None
        } else {
            Some("no_permit")
        };
        prop_assert_eq!(reason(&verdict), expected, "{:?}", verdict);
    }
}

/// Rules written for the semantic actions never reach the audit: a modeler
/// with `view` on everything, anyone with every semantic action, or the
/// base policies as a project already copied them.
#[test]
fn no_rule_for_another_action_reaches_it() {
    let everything = r#"
@id("modeler.everything")
permit (principal in Role::"modeler", action == Action::"view", resource);
@id("all.semantic")
permit (
  principal,
  action in [Action::"view", Action::"view_masked", Action::"use_in_dashboard",
             Action::"propose_change", Action::"approve", Action::"publish",
             Action::"connect", Action::"configure", Action::"manage"],
  resource
);
"#;
    let people = members(
        &[
            ("modeladora", &["modeler"]),
            ("analista", &["analyst"]),
            ("lectora", &["viewer"]),
        ],
        &[],
    );
    for policies in [
        vec![("project.cedar", with_base(everything))],
        vec![("base.cedar", sef_authz::BASE_POLICIES.to_owned())],
    ] {
        let files: Vec<(&str, &str)> = policies.iter().map(|(n, t)| (*n, t.as_str())).collect();
        let p = project(&people, &files);
        for user in ["modeladora", "analista", "lectora", "nadie"] {
            let verdict = decide(p.path(), user, &[]);
            assert_eq!(reason(&verdict), Some("no_permit"), "{user}: {verdict:?}");
        }
    }
}

/// A semantic action written about the audit can never apply: Cedar warns
/// the author that the rule is impossible, and it grants nothing. Nor does
/// a request mixing the action of one with the resource of the other pass.
#[test]
fn the_audit_and_the_semantic_actions_never_mix() {
    let through_view =
        r#"permit (principal, action == Action::"view", resource == AuditLog::"platform:audit");"#;
    let (_, warnings) = PolicySnapshot::load(
        PLATFORM_SCHEMA,
        &[PolicySource {
            path: "policies/project.cedar".to_owned(),
            text: through_view.to_owned(),
        }],
    )
    .unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.message.contains("policy is impossible")),
        "{warnings:?}"
    );
    let p = project(
        &members(&[("ana", &["viewer"])], &[]),
        &[("project.cedar", &with_base(through_view))],
    );
    assert_eq!(reason(&decide(p.path(), "ana", &[])), Some("no_permit"));
    let (snapshot, _) = PolicySnapshot::load(
        PLATFORM_SCHEMA,
        &[PolicySource {
            path: "policies/base.cedar".to_owned(),
            text: sef_authz::BASE_POLICIES.to_owned(),
        }],
    )
    .unwrap();
    let admin = Principal {
        id: "ana".to_owned(),
        roles: BTreeSet::from(["admin".to_owned()]),
        attributes: BTreeMap::new(),
    };
    let model = Resource {
        kind: ResourceKind::Model,
        id: "m".to_owned(),
        attributes: BTreeMap::from([(
            "classification".to_owned(),
            AttrValue::String("public".to_owned()),
        )]),
        parents: Vec::new(),
    };
    for (action, resource) in [(Action::View, resource()), (Action::ViewAudit, model)] {
        let decision = sef_authz::authorize(&snapshot, &admin, action, &resource, &Context::new());
        assert!(!decision.allowed, "{action:?} on {:?}", resource.kind);
        assert_eq!(decision.reason.code(), "invalid_request");
    }
}

#[test]
fn a_forbid_prevails_even_for_an_admin() {
    let admins = members(&[("ana", &["admin"]), ("eva", &["auditor_reader"])], &[]);
    let forbid = r#"
@id("audit.closed")
forbid (principal, action == Action::"view_audit", resource == AuditLog::"platform:audit");
"#;
    let p = project(
        &admins,
        &[("project.cedar", &with_base(&format!("{AUDITOR}{forbid}")))],
    );
    for user in ["ana", "eva"] {
        assert_eq!(
            reason(&decide(p.path(), user, &[])),
            Some("forbidden"),
            "{user}"
        );
    }
    // A rule that fails to evaluate denies; it never allows.
    let failing = r#"
@id("overflow")
forbid (principal, action, resource) when { 9223372036854775807 + 1 > 0 };
"#;
    let p = project(&admins, &[("project.cedar", &with_base(failing))]);
    assert_eq!(
        reason(&decide(p.path(), "ana", &[])),
        Some("evaluation_error")
    );
}

/// Each request reads the configuration again: an invalid one closes
/// without keeping the grant it replaced, and a revocation holds at once.
#[test]
fn an_invalid_configuration_closes_and_a_revocation_holds_next() {
    let p = project(
        &members(&[("eva", &["auditor_reader"])], &[]),
        &[("project.cedar", &with_base(AUDITOR))],
    );
    let policies = p.path().join("policies/project.cedar");
    let people = p.path().join("policies/members.yaml");
    assert_eq!(reason(&decide(p.path(), "eva", &[])), None);
    std::fs::write(
        &policies,
        "permit (principal, action, resource) when { x };",
    )
    .unwrap();
    assert_eq!(reason(&decide(p.path(), "eva", &[])), Some("unavailable"));
    std::fs::write(&policies, with_base(AUDITOR)).unwrap();
    std::fs::write(&people, "version: 2\n").unwrap();
    assert_eq!(reason(&decide(p.path(), "eva", &[])), Some("unavailable"));
    std::fs::write(&people, members(&[("eva", &["auditor_reader"])], &[])).unwrap();
    assert_eq!(reason(&decide(p.path(), "eva", &[])), None);
    std::fs::write(&people, members(&[("eva", &["viewer"])], &[])).unwrap();
    assert_eq!(reason(&decide(p.path(), "eva", &[])), Some("no_permit"));
}

/// The platform fragment before #158.
fn previous_schema() -> String {
    let added = "entity AuditLog;\naction \"view_audit\" appliesTo {\n  principal: [User],\n  resource: [AuditLog],\n  context: RequestContext,\n};\n";
    assert_eq!(PLATFORM_SCHEMA.matches(added).count(), 1);
    PLATFORM_SCHEMA.replace(added, "")
}

fn governed(kind: ResourceKind, classification: &str) -> Resource {
    let mut attributes = BTreeMap::from([(
        "classification".to_owned(),
        AttrValue::String(classification.to_owned()),
    )]);
    if matches!(
        kind,
        ResourceKind::Dataset | ResourceKind::Metric | ResourceKind::Dimension
    ) {
        attributes.insert("tags".to_owned(), AttrValue::Set(BTreeSet::new()));
        attributes.insert("deprecated".to_owned(), AttrValue::Bool(false));
    }
    Resource {
        kind,
        id: "x".to_owned(),
        attributes,
        parents: Vec::new(),
    }
}

/// Every decision that existed before is the same: each base role, each
/// earlier action, on each earlier kind of object and classification. Only
/// the snapshot version, and so the fingerprints, change.
#[test]
fn every_earlier_decision_is_unchanged() {
    let base = [PolicySource {
        path: "policies/base.cedar".to_owned(),
        text: sef_authz::BASE_POLICIES.to_owned(),
    }];
    let (before, _) = PolicySnapshot::load(&previous_schema(), &base).unwrap();
    let (after, _) = PolicySnapshot::load(PLATFORM_SCHEMA, &base).unwrap();
    assert_ne!(before.version(), after.version());
    let mut resources = Vec::new();
    for classification in ["public", "internal", "confidential", "restricted"] {
        for kind in [
            ResourceKind::Model,
            ResourceKind::Dataset,
            ResourceKind::Metric,
            ResourceKind::Dimension,
            ResourceKind::Dashboard,
        ] {
            resources.push(governed(kind, classification));
        }
    }
    for kind in [ResourceKind::Source, ResourceKind::User, ResourceKind::Role] {
        resources.push(Resource {
            kind,
            id: "x".to_owned(),
            attributes: BTreeMap::new(),
            parents: Vec::new(),
        });
    }
    let mut compared = 0;
    for role in ["viewer", "analyst", "modeler", "admin", "otro"] {
        let principal = Principal {
            id: "ana".to_owned(),
            roles: BTreeSet::from([role.to_owned()]),
            attributes: BTreeMap::new(),
        };
        for action in Action::ALL.into_iter().filter(|a| *a != Action::ViewAudit) {
            for resource in &resources {
                let decide = |snapshot| {
                    let d = sef_authz::authorize(
                        snapshot,
                        &principal,
                        action,
                        resource,
                        &Context::new(),
                    );
                    (d.allowed, d.reason.code())
                };
                assert_eq!(
                    decide(&before),
                    decide(&after),
                    "{role} {action:?} {:?} {:?}",
                    resource.kind,
                    resource.attributes.get("classification")
                );
                compared += 1;
            }
        }
    }
    assert_eq!(compared, 5 * 12 * 23);
}
