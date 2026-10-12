//! Single decisions (ADR 0012 §2–4): the base roles, denial on any error,
//! entities and context validated per request, explained reasons and
//! fingerprints, and policy sets that never load when invalid.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;

use cedar_policy::RestrictedExpression;
use common::{
    PLATFORM, expected, governed, metric, model, none, principal, risk, snapshot, source, text,
};
use sef_authz::{
    Action, AttrValue, BASE_POLICIES, Context, PolicySnapshot, Reason, Resource, ResourceKind,
    Severity, authorize,
};

#[test]
fn base_roles_decide_by_classification() {
    let base = snapshot("");
    for roles in [&["viewer"][..], &["analyst"], &["modeler"], &["admin"]] {
        for classification in ["public", "internal", "confidential", "restricted"] {
            for action in [Action::View, Action::ViewMasked, Action::UseInDashboard] {
                let decision = authorize(
                    &base,
                    &principal(roles),
                    action,
                    &metric("revenue", classification),
                    &none(),
                );
                let set: BTreeSet<String> = roles.iter().map(|r| (*r).to_owned()).collect();
                assert_eq!(
                    decision.allowed,
                    expected(&set, classification, action.id()),
                    "{roles:?} {classification} {}",
                    action.id()
                );
                let reason = if decision.allowed {
                    Reason::Permitted
                } else {
                    Reason::NoPermit
                };
                assert_eq!(decision.reason, reason);
            }
        }
    }
}

#[test]
fn an_allowed_decision_names_its_permits_and_a_default_denial_names_none() {
    let base = snapshot("");
    let allowed = authorize(
        &base,
        &principal(&["viewer"]),
        Action::View,
        &metric("revenue", "public"),
        &none(),
    );
    assert_eq!(allowed.determining, ["viewer.view"]);
    let denied = authorize(
        &base,
        &principal(&["viewer"]),
        Action::View,
        &metric("revenue", "restricted"),
        &none(),
    );
    assert_eq!(denied.reason, Reason::NoPermit);
    assert_eq!(denied.determining, Vec::<String>::new());
}

#[test]
fn a_forbid_wins_and_is_named() {
    let base = snapshot(
        r#"@id("no-pii")
forbid (principal, action == Action::"view", resource)
when { resource has tags && resource.tags.contains("pii") };"#,
    );
    let decision = authorize(
        &base,
        &principal(&["admin"]),
        Action::View,
        &governed(ResourceKind::Metric, "salary", "internal", &["pii"]),
        &none(),
    );
    assert_eq!(decision.reason, Reason::Forbidden);
    assert_eq!(decision.determining, ["no-pii"]);
}

#[test]
fn containment_grants_through_the_model() {
    let base = snapshot(
        r#"@id("finance.readers")
permit (principal in Role::"finance", action == Action::"view", resource in Model::"finance");"#,
    );
    let mut inside = metric("margin", "restricted");
    inside.parents = vec![model("finance")];
    let reader = principal(&["finance"]);
    assert!(authorize(&base, &reader, Action::View, &inside, &none()).allowed);
    assert!(
        !authorize(
            &base,
            &reader,
            Action::View,
            &metric("margin", "public"),
            &none()
        )
        .allowed
    );
}

// ---- Denial on any error (ADR 0012 §2) ---------------------------------------------

const OVERFLOW: &str = r#"@id("all")
permit (principal, action, resource);

@id("risky")
forbid (principal, action, resource)
when { context has risk && context.risk + 9223372036854775807 > 0 };"#;

fn raw_cedar_allows_despite_the_error() {
    let fragment = |text: &str| {
        cedar_policy::SchemaFragment::from_cedarschema_str(text)
            .expect("fragment")
            .0
    };
    let schema = cedar_policy::Schema::from_schema_fragments([
        fragment(sef_authz::SEMANTIC_SCHEMA),
        fragment(PLATFORM),
    ])
    .expect("schema");
    let policies = cedar_policy::PolicySet::from_str(OVERFLOW).expect("policies");
    let uid = |text: &str| cedar_policy::EntityUid::from_str(text).expect("uid");
    let string = |value: &str| RestrictedExpression::new_string(value.to_owned());
    let entity = |id: &str, attributes: Vec<(&str, RestrictedExpression)>, parents: Vec<&str>| {
        cedar_policy::Entity::new(
            uid(id),
            attributes
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value))
                .collect::<HashMap<_, _>>(),
            parents.into_iter().map(uid).collect::<HashSet<_>>(),
        )
        .expect("entity")
    };
    let entities = cedar_policy::Entities::from_entities(
        [
            entity(r#"User::"ana""#, Vec::new(), Vec::new()),
            entity(
                r#"Model::"sales""#,
                vec![("classification", string("internal"))],
                Vec::new(),
            ),
            entity(
                r#"Metric::"revenue""#,
                vec![
                    ("classification", string("public")),
                    ("tags", RestrictedExpression::new_set([])),
                    ("deprecated", RestrictedExpression::new_bool(false)),
                ],
                vec![r#"Model::"sales""#],
            ),
        ],
        Some(&schema),
    )
    .expect("entities");
    let context =
        cedar_policy::Context::from_pairs([("risk".to_owned(), RestrictedExpression::new_long(1))])
            .expect("context");
    let request = cedar_policy::Request::new(
        uid(r#"User::"ana""#),
        uid(r#"Action::"view""#),
        uid(r#"Metric::"revenue""#),
        context,
        Some(&schema),
    )
    .expect("request");
    let raw = cedar_policy::Authorizer::new().is_authorized(&request, &policies, &entities);
    assert_eq!(raw.decision(), cedar_policy::Decision::Allow);
    assert!(raw.diagnostics().errors().next().is_some());
}

#[test]
fn a_failing_forbid_denies_even_beside_a_general_permit() {
    // Cedar alone skips the failing forbid and allows: checked first, so the
    // test proves the case it guards against.
    raw_cedar_allows_despite_the_error();
    let guarded = snapshot(OVERFLOW);
    let decision = authorize(
        &guarded,
        &principal(&[]),
        Action::View,
        &metric("revenue", "public"),
        &risk(1),
    );
    assert!(!decision.allowed);
    assert_eq!(decision.reason, Reason::EvaluationError);
    assert_eq!(decision.determining, Vec::<String>::new());
}

#[test]
fn an_evaluation_error_denies_whatever_the_role() {
    let guarded = snapshot(OVERFLOW);
    let decision = authorize(
        &guarded,
        &principal(&["admin"]),
        Action::Publish,
        &model("sales"),
        &risk(1),
    );
    assert_eq!(decision.reason, Reason::EvaluationError);
    // Without the context value the forbid does not apply and nothing fails.
    let quiet = authorize(
        &guarded,
        &principal(&[]),
        Action::Publish,
        &model("sales"),
        &none(),
    );
    assert_eq!(quiet.reason, Reason::Permitted);
}

#[test]
fn entities_and_context_must_match_the_schema() {
    let base = snapshot("");
    let admin = principal(&["admin"]);
    let invalid = |resource: &Resource, action: Action, context: &Context| {
        let decision = authorize(&base, &admin, action, resource, context);
        assert!(!decision.allowed);
        assert_eq!(decision.reason, Reason::InvalidRequest, "{resource:?}");
    };
    // A required attribute missing.
    let mut missing = metric("revenue", "public");
    missing.attributes.remove("classification");
    invalid(&missing, Action::View, &none());
    // An attribute the schema does not declare.
    let mut extra = metric("revenue", "public");
    extra.attributes.insert("owner".to_owned(), text("ana"));
    invalid(&extra, Action::View, &none());
    // A wrongly typed attribute.
    let mut typed = metric("revenue", "public");
    typed.attributes.insert("deprecated".to_owned(), text("no"));
    invalid(&typed, Action::View, &none());
    // A parent of a type the schema does not allow.
    let mut parent = metric("revenue", "public");
    parent.parents = vec![governed(ResourceKind::Dataset, "orders", "public", &[])];
    invalid(&parent, Action::View, &none());
    // An action that does not apply to the resource.
    invalid(&metric("revenue", "public"), Action::Connect, &none());
    // A context value of the wrong type, or not declared.
    invalid(
        &metric("revenue", "public"),
        Action::View,
        &BTreeMap::from([("risk".to_owned(), text("high"))]),
    );
    invalid(
        &metric("revenue", "public"),
        Action::View,
        &BTreeMap::from([("ip".to_owned(), text("10.0.0.1"))]),
    );
    // A principal attribute the platform does not declare.
    let mut stranger = principal(&["admin"]);
    stranger
        .attributes
        .insert("clearance".to_owned(), AttrValue::Long(9));
    let decision = authorize(
        &base,
        &stranger,
        Action::View,
        &metric("revenue", "public"),
        &none(),
    );
    assert_eq!(decision.reason, Reason::InvalidRequest);
}

#[test]
fn the_same_object_twice_must_be_identical() {
    let base = snapshot("");
    let admin = principal(&["admin"]);
    let myself = Resource {
        kind: ResourceKind::User,
        id: "ana".to_owned(),
        attributes: BTreeMap::new(),
        parents: Vec::new(),
    };
    let mut with_roles = myself.clone();
    with_roles.parents = vec![Resource {
        kind: ResourceKind::Role,
        id: "admin".to_owned(),
        attributes: BTreeMap::new(),
        parents: Vec::new(),
    }];
    assert!(authorize(&base, &admin, Action::Manage, &with_roles, &none()).allowed);
    let mut other = with_roles.clone();
    other
        .attributes
        .insert("department".to_owned(), text("finanzas"));
    assert_eq!(
        authorize(&base, &admin, Action::Manage, &other, &none()).reason,
        Reason::InvalidRequest
    );
    // Supplied without its roles, it contradicts the principal.
    assert_eq!(
        authorize(&base, &admin, Action::Manage, &myself, &none()).reason,
        Reason::InvalidRequest
    );
}

// ---- Fingerprints (ADR 0012 §3) ---------------------------------------------------------

#[test]
fn the_fingerprint_covers_policies_attributes_and_context() {
    let base = snapshot("");
    let viewer = principal(&["viewer"]);
    let revenue = metric("revenue", "public");
    let first = authorize(&base, &viewer, Action::View, &revenue, &none());
    assert_eq!(
        first.fingerprint,
        authorize(&base, &viewer, Action::View, &revenue, &none()).fingerprint
    );
    let mut changed = Vec::new();
    // Another snapshot (a revocation), even with the same outcome.
    let revoked = snapshot(r#"@id("x") forbid (principal, action, resource) when { false };"#);
    changed.push(authorize(
        &revoked,
        &viewer,
        Action::View,
        &revenue,
        &none(),
    ));
    // Another attribute of the resource or of the principal.
    changed.push(authorize(
        &base,
        &viewer,
        Action::View,
        &metric("revenue", "internal"),
        &none(),
    ));
    let mut moved = viewer.clone();
    moved
        .attributes
        .insert("department".to_owned(), text("ventas"));
    changed.push(authorize(&base, &moved, Action::View, &revenue, &none()));
    // Another role, action or context value.
    changed.push(authorize(
        &base,
        &principal(&["viewer", "analyst"]),
        Action::View,
        &revenue,
        &none(),
    ));
    changed.push(authorize(
        &base,
        &viewer,
        Action::UseInDashboard,
        &revenue,
        &none(),
    ));
    changed.push(authorize(&base, &viewer, Action::View, &revenue, &risk(0)));
    for other in changed {
        assert_ne!(first.fingerprint, other.fingerprint);
    }
}

// ---- Loading policies (ADR 0012 §8) -----------------------------------------------------

#[test]
fn an_invalid_policy_set_never_loads() {
    let rejected = |extra: &str, needle: &str| {
        let problems = PolicySnapshot::load(
            PLATFORM,
            &[
                source("policies/base.cedar", BASE_POLICIES),
                source("policies/extra.cedar", extra),
            ],
        )
        .expect_err("must not load");
        assert!(
            problems.iter().any(|p| p.severity == Severity::Error
                && p.path == "policies/extra.cedar"
                && p.message.contains(needle)),
            "{needle}: {problems:#?}"
        );
    };
    // Syntax.
    rejected("permit (principal, action, resource", "");
    // Strict validation: an attribute the schema does not declare.
    rejected(
        r#"permit (principal, action == Action::"view", resource) when { resource.owner == "ana" };"#,
        "",
    );
    // A template.
    rejected(
        "permit (principal == ?principal, action, resource);",
        "plantillas",
    );
    // A repeated id, here across files.
    rejected(
        r#"@id("admin") permit (principal, action, resource);"#,
        "admin",
    );
    // A platform fragment that misses a type the semantic one uses.
    let problems = PolicySnapshot::load(
        "type RequestContext = {}; entity Role; entity User in [Role]; entity Dashboard = { classification: String };",
        &[source("policies/base.cedar", BASE_POLICIES)],
    )
    .expect_err("Source is missing");
    assert!(
        problems
            .iter()
            .any(|p| p.path == "policies/platform.cedarschema"),
        "{problems:#?}"
    );
}

#[test]
fn the_version_identifies_schema_and_policies() {
    let version = |platform: &str, text: &str| {
        PolicySnapshot::load(platform, &[source("policies/base.cedar", text)])
            .expect("loads")
            .0
            .version()
            .to_owned()
    };
    let base = version(PLATFORM, BASE_POLICIES);
    assert_eq!(base, version(PLATFORM, BASE_POLICIES));
    assert_ne!(
        base,
        version(PLATFORM, &format!("{BASE_POLICIES}\n// nota"))
    );
    assert_ne!(
        base,
        version(
            &PLATFORM.replace(
                "department?: String,",
                "department?: String,\n  site?: String,"
            ),
            BASE_POLICIES
        )
    );
}

/// Cost of one decision on the query path: run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement, not a correctness check"]
fn authorization_latency() {
    let base = snapshot("");
    let viewer = principal(&["viewer"]);
    let revenue = metric("revenue", "internal");
    let runs = 5_000_u32;
    let start = std::time::Instant::now();
    for _ in 0..runs {
        assert!(authorize(&base, &viewer, Action::View, &revenue, &none()).allowed);
    }
    println!(
        "one decision: {:.1} µs",
        start.elapsed().as_secs_f64() * 1e6 / f64::from(runs)
    );
}
