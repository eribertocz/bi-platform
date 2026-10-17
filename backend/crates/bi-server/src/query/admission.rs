//! Admission of the G1 query route (#118). It compiles catalog queries and
//! runs DuckDB without the semantic plan or the permission engine, so it is
//! closed except for the local synthetic demo: every condition below comes
//! from the server (configuration, the stored session, the runtime's own
//! bootstrap), never from the request.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bi_runtime::Runtime;

use crate::AppState;
use crate::analytics::Provenance;
use crate::auth::Principal;
use crate::metadata::MetadataMode;
use crate::problem::Problem;

/// Shown to whoever reaches the closed route: why, and what to expect.
pub(crate) const CLOSED_DETAIL: &str = "Esta consulta de demostración solo está disponible en desarrollo sintético local. El flujo semántico de consulta aún no está disponible.";

/// What the edge checks, all derived by the server before reading the body.
// Six independent facts, each verified and reported by its own name.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Conditions {
    /// The synthetic dev login is compiled in (`dev-auth`).
    pub dev_auth: bool,
    /// `BI_METADATA_MODE=development`, already validated by #115.
    pub development_metadata: bool,
    /// Synthetic users are configured.
    pub synthetic_users: bool,
    /// No federated login is configured.
    pub oidc_disabled: bool,
    /// `BI_PUBLIC_ORIGIN` is loopback.
    pub loopback_origin: bool,
    /// The session came from the synthetic dev login, not from OIDC.
    pub local_session: bool,
}

/// Why the route is closed for this request; a fixed vocabulary for logs
/// and metrics, never shown to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Closed {
    DevAuthAbsent,
    MetadataRestricted,
    NoSyntheticUsers,
    OidcEnabled,
    PublicOrigin,
    FederatedSession,
    UnverifiedRuntime,
}

impl Closed {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::DevAuthAbsent => "dev_auth_absent",
            Self::MetadataRestricted => "metadata_restricted",
            Self::NoSyntheticUsers => "no_synthetic_users",
            Self::OidcEnabled => "oidc_enabled",
            Self::PublicOrigin => "public_origin",
            Self::FederatedSession => "federated_session",
            Self::UnverifiedRuntime => "unverified_runtime",
        }
    }
}

/// The decision, with the runtime it was taken for: execution uses that
/// same instance, never one resolved again later.
pub(crate) enum Admission {
    Admitted(Runtime),
    /// Every condition holds but the runtime has not finished its bootstrap.
    Pending,
    Closed(Closed),
}

/// The single admission rule. The runtime is consulted only once every
/// other condition holds, and it is admitted only when its own bootstrap
/// vouched for the synthetic `sales` fixture.
pub(crate) fn evaluate(
    conditions: Conditions,
    instance: Option<(Runtime, Provenance)>,
) -> Admission {
    let Conditions {
        dev_auth,
        development_metadata,
        synthetic_users,
        oidc_disabled,
        loopback_origin,
        local_session,
    } = conditions;
    let closed = [
        (dev_auth, Closed::DevAuthAbsent),
        (development_metadata, Closed::MetadataRestricted),
        (synthetic_users, Closed::NoSyntheticUsers),
        (oidc_disabled, Closed::OidcEnabled),
        (loopback_origin, Closed::PublicOrigin),
        (local_session, Closed::FederatedSession),
    ]
    .into_iter()
    .find_map(|(holds, reason)| (!holds).then_some(reason));
    if let Some(reason) = closed {
        return Admission::Closed(reason);
    }
    match instance {
        None => Admission::Pending,
        Some((runtime, Provenance::SyntheticSales(_))) => Admission::Admitted(runtime),
        Some((_, Provenance::Unknown)) => Admission::Closed(Closed::UnverifiedRuntime),
    }
}

/// Reads the conditions and one snapshot of the runtime from `state`.
pub(crate) fn admit(state: &AppState, principal: &Principal) -> Admission {
    let conditions = Conditions {
        dev_auth: cfg!(feature = "dev-auth"),
        development_metadata: state.db.mode() == MetadataMode::Development,
        synthetic_users: state.auth.has_synthetic_users(),
        oidc_disabled: state.auth.oidc.is_none(),
        loopback_origin: state.auth.loopback_origin(),
        local_session: principal.federated_identity.is_none(),
    };
    evaluate(
        conditions,
        state
            .analytics
            .instance()
            .map(|instance| (instance.runtime, instance.provenance)),
    )
}

/// Answers a request the route is closed for: records the denial, then
/// `403 legacy_query_disabled`, or `503` when the denial cannot be recorded.
/// The body is never read, so nothing the client sent is reflected.
pub(crate) async fn refuse(
    state: &AppState,
    principal: &Principal,
    request_id: &str,
    reason: Closed,
) -> Response {
    let recorded = record_denial(state, principal, request_id).await;
    state.metrics.legacy_denied(
        reason.label(),
        if recorded.is_ok() {
            "recorded"
        } else {
            "unavailable"
        },
    );
    match recorded {
        Ok(()) => {
            tracing::info!(
                request_id,
                admission = "closed",
                reason = reason.label(),
                "legacy query route closed"
            );
            Problem::new(
                StatusCode::FORBIDDEN,
                "legacy_query_disabled",
                Some(CLOSED_DETAIL.to_owned()),
                request_id,
            )
            .into_response()
        }
        Err(code) => {
            tracing::warn!(
                request_id,
                admission = "closed",
                reason = reason.label(),
                event_code = "legacy_query_denial_unrecorded",
                "legacy query route closed but the denial could not be recorded"
            );
            Problem::new(StatusCode::SERVICE_UNAVAILABLE, code, None, request_id).into_response()
        }
    }
}

/// One denial event of this route, within `query_timeout`
/// (see [`crate::audit::record_within`]).
async fn record_denial(
    state: &AppState,
    principal: &Principal,
    request_id: &str,
) -> Result<(), &'static str> {
    crate::audit::record_within(
        &state.db,
        crate::audit::HttpRecord {
            actor: &principal.session.user.id,
            request_id,
            action: bi_audit::Action::Denial,
            result: bi_audit::ResultCode::Denied,
            resource_type: "query_endpoint",
            resource_id: "legacy_queries",
            // An operational closure, not a decision of the permission engine;
            // the refused body is never read.
            policy_decision_id: None,
        },
        state.query_timeout,
    )
    .await
}

#[cfg(test)]
mod tests {
    use bi_fixtures::sales::FixtureSpec;
    use proptest::prelude::*;

    use super::*;

    const OPEN: Conditions = Conditions {
        dev_auth: true,
        development_metadata: true,
        synthetic_users: true,
        oidc_disabled: true,
        loopback_origin: true,
        local_session: true,
    };
    const SALES: Provenance = Provenance::SyntheticSales(FixtureSpec {
        seed: 42,
        rows: 1_000,
    });

    /// One runtime for every case: only its identity matters here.
    fn runtime() -> Runtime {
        static SHARED: std::sync::OnceLock<Runtime> = std::sync::OnceLock::new();
        SHARED
            .get_or_init(|| {
                bi_runtime::Bootstrap::open(bi_runtime::RuntimeConfig::in_memory(1))
                    .unwrap()
                    .finish()
                    .unwrap()
            })
            .clone()
    }

    fn closed(admission: &Admission) -> Option<Closed> {
        match admission {
            Admission::Closed(reason) => Some(*reason),
            Admission::Admitted(_) | Admission::Pending => None,
        }
    }

    #[test]
    fn each_condition_alone_closes_the_route_with_its_own_reason() {
        type Invalidate = fn(&mut Conditions);
        let cases: [(Invalidate, Closed); 6] = [
            (|c| c.dev_auth = false, Closed::DevAuthAbsent),
            (
                |c| c.development_metadata = false,
                Closed::MetadataRestricted,
            ),
            (|c| c.synthetic_users = false, Closed::NoSyntheticUsers),
            (|c| c.oidc_disabled = false, Closed::OidcEnabled),
            (|c| c.loopback_origin = false, Closed::PublicOrigin),
            (|c| c.local_session = false, Closed::FederatedSession),
        ];
        for (invalidate, reason) in cases {
            let mut conditions = OPEN;
            invalidate(&mut conditions);
            let admission = evaluate(conditions, Some((runtime(), SALES)));
            assert_eq!(closed(&admission), Some(reason), "{conditions:?}");
        }
    }

    #[test]
    fn only_a_vouched_synthetic_runtime_is_admitted() {
        assert!(matches!(
            evaluate(OPEN, Some((runtime(), SALES))),
            Admission::Admitted(_)
        ));
        assert_eq!(
            closed(&evaluate(OPEN, Some((runtime(), Provenance::Unknown)))),
            Some(Closed::UnverifiedRuntime)
        );
        assert!(matches!(evaluate(OPEN, None), Admission::Pending));
    }

    fn conditions() -> impl Strategy<Value = Conditions> {
        any::<[bool; 6]>().prop_map(|[a, b, c, d, e, f]| Conditions {
            dev_auth: a,
            development_metadata: b,
            synthetic_users: c,
            oidc_disabled: d,
            loopback_origin: e,
            local_session: f,
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Admission requires every condition and a vouched runtime: any
        /// failing condition closes the route, whatever the runtime.
        #[test]
        fn any_failing_condition_never_admits(
            conditions in conditions(),
            provenance in prop_oneof![Just(SALES), Just(Provenance::Unknown)],
            ready in any::<bool>(),
        ) {
            let instance = ready.then(|| (runtime(), provenance));
            let admission = evaluate(conditions, instance);
            let admitted = matches!(admission, Admission::Admitted(_));
            prop_assert_eq!(admitted, conditions == OPEN && ready && provenance == SALES);
            if conditions != OPEN {
                prop_assert!(closed(&admission).is_some());
            }
        }
    }
}
