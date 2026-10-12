//! Column-level security of a query (ADR 0012 §5): a masked object can
//! only be projected; any other use of it, or any use of an object without
//! permission, denies the whole query before it runs.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use common::{governed, metric, none, principal, risk, snapshot};
use sef_authz::{Constraint, Reason, Resource, ResourceKind, Usage, Use, authorize_query};

fn uses(items: &[(&Resource, Usage)]) -> Vec<Use> {
    items
        .iter()
        .map(|(resource, usage)| Use {
            resource: (*resource).clone(),
            usage: *usage,
        })
        .collect()
}

fn mask(id: &str) -> Constraint {
    Constraint::Mask {
        kind: ResourceKind::Metric,
        id: id.to_owned(),
    }
}

#[test]
fn a_masked_object_can_only_be_projected() {
    let base = snapshot("");
    let viewer = principal(&["viewer"]);
    let salary = metric("salary", "confidential");
    let projected = authorize_query(&base, &viewer, &uses(&[(&salary, Usage::Project)]), &none());
    assert!(projected.allowed);
    assert_eq!(projected.constraints, [mask("salary")]);
    for usage in [
        Usage::Filter,
        Usage::Order,
        Usage::Group,
        Usage::Join,
        Usage::Lineage,
    ] {
        let decision = authorize_query(
            &base,
            &viewer,
            &uses(&[(&salary, Usage::Project), (&salary, usage)]),
            &none(),
        );
        assert!(!decision.allowed, "{usage:?}");
        assert_eq!(decision.reason, Reason::NoPermit);
        assert_eq!(
            decision.denied,
            Some((ResourceKind::Metric, "salary".to_owned(), usage))
        );
        assert_eq!(
            decision.constraints,
            Vec::<Constraint>::new(),
            "nothing runs"
        );
    }
}

#[test]
fn without_view_or_view_masked_the_query_is_denied_not_trimmed() {
    let base = snapshot("");
    let viewer = principal(&["viewer"]);
    let revenue = metric("revenue", "public");
    let margin = metric("margin", "restricted");
    let decision = authorize_query(
        &base,
        &viewer,
        &uses(&[(&revenue, Usage::Project), (&margin, Usage::Project)]),
        &none(),
    );
    assert!(!decision.allowed);
    assert_eq!(
        decision.denied,
        Some((ResourceKind::Metric, "margin".to_owned(), Usage::Project))
    );
    // Both objects were decided, for the audit.
    assert_eq!(decision.decisions.len(), 3);
}

#[test]
fn visible_objects_carry_no_mask() {
    let base = snapshot("");
    let analyst = principal(&["analyst"]);
    let revenue = metric("revenue", "confidential");
    let decision = authorize_query(
        &base,
        &analyst,
        &uses(&[
            (&revenue, Usage::Project),
            (&revenue, Usage::Filter),
            (&revenue, Usage::Group),
        ]),
        &none(),
    );
    assert!(decision.allowed);
    assert_eq!(decision.reason, Reason::Permitted);
    assert_eq!(decision.constraints, Vec::<Constraint>::new());
}

#[test]
fn a_forbid_on_view_masked_prevails() {
    let base = snapshot(
        r#"@id("no-pii-masks")
forbid (principal, action == Action::"view_masked", resource)
when { resource has tags && resource.tags.contains("pii") };"#,
    );
    let viewer = principal(&["viewer"]);
    let ssn = governed(ResourceKind::Metric, "ssn", "confidential", &["pii"]);
    let decision = authorize_query(&base, &viewer, &uses(&[(&ssn, Usage::Project)]), &none());
    assert!(!decision.allowed);
    assert_eq!(decision.reason, Reason::Forbidden);
}

#[test]
fn an_error_on_view_never_falls_back_to_a_mask() {
    let base = snapshot(
        r#"@id("masks")
permit (principal, action == Action::"view_masked", resource);

@id("risky")
forbid (principal, action == Action::"view", resource)
when { context has risk && context.risk + 9223372036854775807 > 0 };"#,
    );
    let viewer = principal(&["viewer"]);
    let revenue = metric("revenue", "public");
    let decision = authorize_query(
        &base,
        &viewer,
        &uses(&[(&revenue, Usage::Project)]),
        &risk(1),
    );
    assert!(!decision.allowed);
    assert_eq!(decision.reason, Reason::EvaluationError);
    assert_eq!(decision.decisions.len(), 1, "view_masked was never asked");
}

#[test]
fn the_query_fingerprint_covers_each_use() {
    let base = snapshot("");
    let analyst = principal(&["analyst"]);
    let revenue = metric("revenue", "public");
    let projected = authorize_query(
        &base,
        &analyst,
        &uses(&[(&revenue, Usage::Project)]),
        &none(),
    );
    let filtered = authorize_query(
        &base,
        &analyst,
        &uses(&[(&revenue, Usage::Filter)]),
        &none(),
    );
    assert!(projected.allowed && filtered.allowed);
    assert_ne!(projected.fingerprint, filtered.fingerprint);
    assert_eq!(
        projected.fingerprint,
        authorize_query(
            &base,
            &analyst,
            &uses(&[(&revenue, Usage::Project)]),
            &none()
        )
        .fingerprint
    );
}

// ---- One coherent request (review of #90) ---------------------------------------------

#[test]
fn an_object_described_two_ways_in_one_query_is_invalid() {
    let base = snapshot("");
    let viewer = principal(&["viewer"]);
    let masked = metric("salary", "confidential");
    let public = metric("salary", "public");
    for usage in [
        Usage::Filter,
        Usage::Order,
        Usage::Group,
        Usage::Join,
        Usage::Lineage,
    ] {
        let decision = authorize_query(
            &base,
            &viewer,
            &uses(&[(&masked, Usage::Project), (&public, usage)]),
            &none(),
        );
        assert!(!decision.allowed, "{usage:?}");
        assert_eq!(decision.reason, Reason::InvalidRequest);
        assert_eq!(decision.constraints, Vec::<Constraint>::new());
    }
}

#[test]
fn a_shared_container_described_two_ways_is_invalid() {
    let base = snapshot("");
    let analyst = principal(&["analyst"]);
    let revenue = metric("revenue", "public");
    let mut margin = metric("margin", "public");
    margin.parents[0]
        .attributes
        .insert("classification".to_owned(), common::text("restricted"));
    let decision = authorize_query(
        &base,
        &analyst,
        &uses(&[(&revenue, Usage::Project), (&margin, Usage::Project)]),
        &none(),
    );
    assert!(!decision.allowed);
    assert_eq!(decision.reason, Reason::InvalidRequest);
}

#[test]
fn a_query_without_uses_is_invalid_and_still_validates_nothing_silently() {
    let base = snapshot("");
    let mut stranger = principal(&["admin"]);
    stranger
        .attributes
        .insert("undeclared".to_owned(), sef_authz::AttrValue::Bool(true));
    let decision = authorize_query(&base, &stranger, &[], &none());
    assert!(!decision.allowed);
    assert_eq!(decision.reason, Reason::InvalidRequest);
    assert_eq!(decision.denied, None);
    // Even a single valid use with an invalid principal is invalid.
    let revenue = metric("revenue", "public");
    let decision = authorize_query(
        &base,
        &stranger,
        &uses(&[(&revenue, Usage::Project)]),
        &none(),
    );
    assert_eq!(decision.reason, Reason::InvalidRequest);
}

#[test]
fn the_query_fingerprint_always_covers_its_scope() {
    let base = snapshot("");
    let revoked = snapshot("forbid (principal, action, resource);");
    let revenue = metric("revenue", "public");
    let empty = authorize_query(&base, &principal(&["viewer"]), &[], &none());
    for other in [
        authorize_query(&revoked, &principal(&["viewer"]), &[], &none()),
        authorize_query(&base, &principal(&["admin"]), &[], &none()),
        authorize_query(&base, &principal(&["viewer"]), &[], &risk(1)),
    ] {
        assert_ne!(empty.fingerprint, other.fingerprint);
    }
    // Denials keep the scope too: the same denied query under another
    // snapshot is another fingerprint.
    let one = uses(&[(&revenue, Usage::Project)]);
    assert_ne!(
        authorize_query(&revoked, &principal(&["viewer"]), &one, &none()).fingerprint,
        authorize_query(&revoked, &principal(&["viewer"]), &one, &risk(1)).fingerprint
    );
}
