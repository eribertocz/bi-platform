//! Properties of the base roles and of request validation (ADR 0012).
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeSet;

use common::{expected, metric, none, principal, snapshot};
use proptest::prelude::*;
use sef_authz::{Action, AttrValue, Reason, authorize};

const ROLES: [&str; 6] = ["viewer", "analyst", "modeler", "admin", "finance", "guest"];
const CLASSES: [&str; 4] = ["public", "internal", "confidential", "restricted"];
const ACTIONS: [Action; 3] = [Action::View, Action::ViewMasked, Action::UseInDashboard];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Every combination of roles gets exactly what the role table says.
    #[test]
    fn base_roles_follow_the_role_table(
        roles in proptest::sample::subsequence(ROLES.to_vec(), 0..=ROLES.len()),
        class in proptest::sample::select(CLASSES.to_vec()),
        action in proptest::sample::select(ACTIONS.to_vec()),
    ) {
        let base = snapshot("");
        let decision = authorize(&base, &principal(&roles), action, &metric("m", class), &none());
        let set: BTreeSet<String> = roles.iter().map(|r| (*r).to_owned()).collect();
        prop_assert_eq!(decision.allowed, expected(&set, class, action.id()));
    }

    /// No combination of roles sees a restricted metric unmasked without
    /// the modeler or administrator role.
    #[test]
    fn restricted_metrics_need_modeler_or_admin(
        roles in proptest::sample::subsequence(
            vec!["viewer", "analyst", "finance", "guest"], 0..=4),
    ) {
        let base = snapshot("");
        let decision = authorize(&base, &principal(&roles), Action::View, &metric("m", "restricted"), &none());
        prop_assert!(!decision.allowed);
    }

    /// An attribute the schema does not declare never reaches a policy:
    /// whatever its name and value, the request is invalid, not allowed.
    #[test]
    fn undeclared_attributes_are_always_invalid(
        name in "[a-z][a-z_]{0,12}",
        value in prop_oneof![
            any::<bool>().prop_map(AttrValue::Bool),
            any::<i64>().prop_map(AttrValue::Long),
            "[ -~]{0,16}".prop_map(AttrValue::String),
        ],
    ) {
        prop_assume!(!matches!(name.as_str(), "classification" | "tags" | "deprecated"));
        let base = snapshot("");
        let mut resource = metric("m", "public");
        resource.attributes.insert(name, value);
        let decision = authorize(&base, &principal(&["admin"]), Action::View, &resource, &none());
        prop_assert!(!decision.allowed);
        prop_assert_eq!(decision.reason, Reason::InvalidRequest);
    }
}
