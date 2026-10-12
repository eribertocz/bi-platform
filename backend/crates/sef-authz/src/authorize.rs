//! One decision (ADR 0012 §2–3). Cedar's authorizer skips a policy whose
//! evaluation fails, so a general `permit` beside a failing `forbid` would
//! allow; here any evaluation error, and any request or entity that does
//! not match the schema, is a denial.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;

use cedar_policy::{
    Authorizer, Context as CedarContext, Decision as CedarDecision, Entities, Entity, EntityId,
    EntityTypeName, EntityUid, Request, RestrictedExpression,
};

use crate::contract::{Action, AttrValue, Context, Principal, Resource};
use crate::decision::{Decision, Encoder, Reason};
use crate::snapshot::PolicySnapshot;

/// Decides whether `principal` may perform `action` on `resource`.
///
/// Never fails: a request that cannot be evaluated is a denial with reason
/// [`Reason::InvalidRequest`] or [`Reason::EvaluationError`].
#[must_use]
pub fn authorize(
    snapshot: &PolicySnapshot,
    principal: &Principal,
    action: Action,
    resource: &Resource,
    context: &Context,
) -> Decision {
    let mut encoder = Encoder::new("sef-authz/decision");
    encoder.request(snapshot.version(), principal, action, resource, context);
    let fingerprint = encoder.finish();
    let decision = |allowed, reason, mut determining: Vec<String>| {
        determining.sort();
        Decision {
            allowed,
            reason,
            determining,
            constraints: Vec::new(),
            fingerprint,
        }
    };
    let (Some(request), Some(entities)) = (
        request(snapshot, principal, action, resource, context),
        entities(snapshot, principal, std::iter::once(resource)),
    ) else {
        return decision(false, Reason::InvalidRequest, Vec::new());
    };
    let response = Authorizer::new().is_authorized(&request, snapshot.policies(), &entities);
    if response.diagnostics().errors().next().is_some() {
        return decision(false, Reason::EvaluationError, Vec::new());
    }
    let determining: Vec<String> = response
        .diagnostics()
        .reason()
        .map(ToString::to_string)
        .collect();
    match response.decision() {
        CedarDecision::Allow => decision(true, Reason::Permitted, determining),
        CedarDecision::Deny if determining.is_empty() => {
            decision(false, Reason::NoPermit, Vec::new())
        }
        CedarDecision::Deny => decision(false, Reason::Forbidden, determining),
    }
}

fn uid(entity_type: &str, id: &str) -> Option<EntityUid> {
    EntityTypeName::from_str(entity_type)
        .ok()
        .map(|name| EntityUid::from_type_name_and_id(name, EntityId::new(id)))
}

fn expression(value: &AttrValue) -> RestrictedExpression {
    match value {
        AttrValue::Bool(flag) => RestrictedExpression::new_bool(*flag),
        AttrValue::Long(number) => RestrictedExpression::new_long(*number),
        AttrValue::String(text) => RestrictedExpression::new_string(text.clone()),
        AttrValue::Set(items) => RestrictedExpression::new_set(items.iter().map(expression)),
    }
}

fn attributes(values: &BTreeMap<String, AttrValue>) -> HashMap<String, RestrictedExpression> {
    values
        .iter()
        .map(|(name, value)| (name.clone(), expression(value)))
        .collect()
}

fn request(
    snapshot: &PolicySnapshot,
    principal: &Principal,
    action: Action,
    resource: &Resource,
    context: &Context,
) -> Option<Request> {
    let context = CedarContext::from_pairs(
        context
            .iter()
            .map(|(name, value)| (name.clone(), expression(value))),
    )
    .ok()?;
    Request::new(
        uid("User", &principal.id)?,
        uid("Action", action.id())?,
        uid(resource.kind.entity_type(), &resource.id)?,
        context,
        Some(snapshot.schema()),
    )
    .ok()
}

/// Entities collected for one request, keyed by their uid's text.
type Collected<'a> = BTreeMap<String, (EntityUid, &'a BTreeMap<String, AttrValue>, Vec<EntityUid>)>;

static NO_ATTRIBUTES: BTreeMap<String, AttrValue> = BTreeMap::new();

/// Adds an entity once. The same object supplied twice with different
/// contents is an invalid request, never a silent choice.
fn collect<'a>(
    found: &mut Collected<'a>,
    uid: EntityUid,
    attributes: &'a BTreeMap<String, AttrValue>,
    parents: Vec<EntityUid>,
) -> Option<()> {
    let key = uid.to_string();
    if let Some((_, known, known_parents)) = found.get(&key) {
        return (*known == attributes && *known_parents == parents).then_some(());
    }
    found.insert(key, (uid, attributes, parents));
    Some(())
}

/// The principal, its roles, the resource and its containers, validated
/// against the schema.
/// Whether the principal and every resource of one request describe each
/// object once and consistently, and match the schema together: a query
/// cannot show an object as one thing in one use and another in the next.
pub(crate) fn coherent<'a>(
    snapshot: &PolicySnapshot,
    principal: &'a Principal,
    resources: impl IntoIterator<Item = &'a Resource>,
) -> bool {
    entities(snapshot, principal, resources).is_some()
}

fn entities<'a>(
    snapshot: &PolicySnapshot,
    principal: &'a Principal,
    resources: impl IntoIterator<Item = &'a Resource>,
) -> Option<Entities> {
    let mut found = Collected::new();
    let mut roles = Vec::new();
    for role in &principal.roles {
        let role = uid("Role", role)?;
        collect(&mut found, role.clone(), &NO_ATTRIBUTES, Vec::new())?;
        roles.push(role);
    }
    collect(
        &mut found,
        uid("User", &principal.id)?,
        &principal.attributes,
        roles,
    )?;
    let mut pending: Vec<&Resource> = resources.into_iter().collect();
    while let Some(current) = pending.pop() {
        let parents = current
            .parents
            .iter()
            .map(|parent| uid(parent.kind.entity_type(), &parent.id))
            .collect::<Option<Vec<_>>>()?;
        collect(
            &mut found,
            uid(current.kind.entity_type(), &current.id)?,
            &current.attributes,
            parents,
        )?;
        pending.extend(&current.parents);
    }
    let entities = found
        .into_values()
        .map(|(uid, values, parents)| {
            Entity::new(
                uid,
                attributes(values),
                parents.into_iter().collect::<HashSet<_>>(),
            )
            .ok()
        })
        .collect::<Option<Vec<_>>>()?;
    Entities::from_entities(entities, Some(snapshot.schema())).ok()
}
