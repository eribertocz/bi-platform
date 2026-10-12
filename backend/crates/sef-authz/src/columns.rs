//! Column-level security of a semantic query (ADR 0012 §5), decided before
//! anything executes:
//! - with `view`, an object can be used in any way;
//! - with only `view_masked`, it can only be projected, as typed `NULL`
//!   flagged as masked; filtering, ordering, grouping, joining or deriving
//!   a metric from it would reveal its values, so the query is denied;
//! - with neither, any use denies the whole query. A selection is never
//!   dropped silently and the grain never changes.
//!
//! An evaluation error or invalid request on `view` denies outright: it is
//! never retried as `view_masked`.

use std::collections::BTreeSet;

use crate::authorize::{authorize, coherent};
use crate::contract::{Action, Context, Principal, Resource, ResourceKind};
use crate::decision::{Constraint, Decision, Encoder, Fingerprint, Reason};
use crate::snapshot::PolicySnapshot;

/// How a query uses an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Usage {
    /// Returned as a result column.
    Project,
    /// Compared in a row or post-aggregate filter.
    Filter,
    /// Used to order the result.
    Order,
    /// Used as a grouping atom.
    Group,
    /// Used as a join key or relationship path.
    Join,
    /// Read to compute another metric.
    Lineage,
}

impl Usage {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Filter => "filter",
            Self::Order => "order",
            Self::Group => "group",
            Self::Join => "join",
            Self::Lineage => "lineage",
        }
    }
}

/// One use of one object by a query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Use {
    pub resource: Resource,
    pub usage: Usage,
}

/// The decision over a whole query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryDecision {
    pub allowed: bool,
    /// [`Reason::Permitted`], or the reason of the first denied use.
    pub reason: Reason,
    /// The first denied use, in the order the query listed them; `None`
    /// when the request as a whole is invalid (no use, or one object
    /// described in contradictory ways).
    pub denied: Option<(ResourceKind, String, Usage)>,
    /// Masks to apply; empty when the query is denied.
    pub constraints: Vec<Constraint>,
    /// Every decision taken, for the audit.
    pub decisions: Vec<Decision>,
    /// Covers every use and every decision taken.
    pub fingerprint: Fingerprint,
}

/// Decides whether `principal` may run a query that makes `uses` of model
/// objects, and which of them it sees masked.
#[must_use]
pub fn authorize_query(
    snapshot: &PolicySnapshot,
    principal: &Principal,
    uses: &[Use],
    context: &Context,
) -> QueryDecision {
    // The whole scope, always: the snapshot (a revocation), who asks, the
    // context and every use, whatever the outcome.
    let mut encoder = Encoder::new("sef-authz/query");
    encoder.text(snapshot.version());
    encoder.principal(principal);
    encoder.attributes(context.iter());
    encoder.count(uses.len());
    for item in uses {
        encoder.text(item.usage.code());
        encoder.resource(&item.resource);
    }
    // A query uses at least one object, and describes each object once.
    if uses.is_empty() || !coherent(snapshot, principal, uses.iter().map(|u| &u.resource)) {
        return QueryDecision {
            allowed: false,
            reason: Reason::InvalidRequest,
            denied: None,
            constraints: Vec::new(),
            decisions: Vec::new(),
            fingerprint: encoder.finish(),
        };
    }
    let mut decisions = Vec::new();
    let mut masks = BTreeSet::new();
    let mut denied = None;
    let mut reason = Reason::Permitted;
    for item in uses {
        let resource = &item.resource;
        let view = authorize(snapshot, principal, Action::View, resource, context);
        encoder.text(item.usage.code());
        encoder.fingerprint(&view.fingerprint);
        let outcome = if view.allowed {
            Ok(false)
        } else if item.usage == Usage::Project
            && matches!(view.reason, Reason::NoPermit | Reason::Forbidden)
        {
            let masked = authorize(snapshot, principal, Action::ViewMasked, resource, context);
            encoder.fingerprint(&masked.fingerprint);
            let outcome = if masked.allowed {
                Ok(true)
            } else {
                Err(masked.reason)
            };
            decisions.push(masked);
            outcome
        } else {
            Err(view.reason)
        };
        decisions.push(view);
        match outcome {
            Ok(true) => {
                masks.insert(Constraint::Mask {
                    kind: resource.kind,
                    id: resource.id.clone(),
                });
            }
            Ok(false) => {}
            Err(why) => {
                if denied.is_none() {
                    denied = Some((resource.kind, resource.id.clone(), item.usage));
                    reason = why;
                }
            }
        }
    }
    let allowed = denied.is_none();
    QueryDecision {
        allowed,
        reason,
        denied,
        constraints: if allowed {
            masks.into_iter().collect()
        } else {
            Vec::new()
        },
        decisions,
        fingerprint: encoder.finish(),
    }
}
