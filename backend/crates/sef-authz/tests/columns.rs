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
