//! The single semantic path (ADR 0010 and 0012, #98): every semantic query
//! is bound, authorized, validated and planned here before anything can
//! run. `POST /api/v1/semantic/explain` answers with the decision, the
//! certified plan and a receipt, and executes nothing; execution (#47) is a
//! later step of this same path.
//!
//! Who asks comes only from the server session. Their roles come from the
//! project's `policies/members.yaml`; the policies from `policies/*.cedar`,
//! or the base roles. An unreadable or invalid policy set closes the path.

pub mod access;
pub mod context;
pub mod prepare;
pub mod request;
pub mod result;

use std::collections::BTreeMap;
use std::time::Instant;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use sef_authz::{Constraint, QueryDecision, ResourceKind};
use sef_correctness::SemanticContext;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::ModelDefinition;
use sef_model_ir::types::{Literal, ValueType};
use sef_query_ir::query::{SelectionItem, SemanticQueryBlock};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tower_http::request_id::RequestId;
use utoipa::ToSchema;

use self::access::Access;
use self::request::ExplainRequest;
use crate::AppState;
use crate::auth::Principal;
use crate::models::{self, ModelDiagnostic};
use crate::problem::Problem;

/// The answer: what was decided, the certified plan and its receipt.
#[derive(Debug, Serialize, ToSchema)]
pub struct SemanticExplanation {
    pub decision: ExplainDecision,
    pub plan: ExplainPlan,
    pub receipt: SemanticReceipt,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExplainDecision {
    /// Stable reason code (`permitted`).
    pub reason: String,
    /// Selections returned as typed `NULL`, flagged as masked.
    pub masked: Vec<String>,
    /// The roles the project grants to the session's user.
    pub roles: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ExplainPlan {
    /// Stable explanation: one line per operator, then type and unit contracts.
    pub explanation: String,
    /// Correctness obligations the certificate discharges.
    pub obligations: u32,
}

/// Fingerprints of everything the answer depended on.
#[derive(Debug, Serialize, ToSchema)]
pub struct SemanticReceipt {
    /// Covers every fingerprint below.
    pub id: String,
    pub query: String,
    pub model: String,
    pub plan: String,
    pub decision: String,
    pub policies: String,
    /// The verified session's row-policy values, as an HMAC under a secret
    /// of this instance: equal within one instance for the same project,
    /// principal and values; never comparable across restarts, never the
    /// values.
    pub context: String,
}

/// A query the engine cannot answer as asked, with stable diagnostics.
#[derive(Debug, Serialize, ToSchema)]
pub struct SemanticRejection {
    #[serde(flatten)]
    pub problem: Problem,
    pub diagnostics: Vec<SemanticDiagnostic>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SemanticDiagnostic {
    /// Stable code (§32).
    pub code: String,
    /// `binding`, `correctness`, `planning` or `model`.
    pub phase: String,
    pub message: String,
    /// JSON pointer into the request, when the binder knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A use the session's roles do not allow.
#[derive(Debug, Serialize, ToSchema)]
pub struct SemanticDenial {
    #[serde(flatten)]
    pub problem: Problem,
    /// `no_permit`, `forbidden`, `evaluation_error` or `invalid_request`.
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied: Option<SemanticDeniedUse>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SemanticDeniedUse {
    /// `metric`, `dimension`, `dataset` or `model`.
    pub kind: String,
    /// Only for objects the query named; what the plan reads stays unnamed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `project`, `filter`, `order`, `group`, `join` or `lineage`.
    pub usage: String,
}

/// Explains a semantic query without executing it.
#[utoipa::path(
    post,
    path = "/api/v1/semantic/explain",
    tag = "semantic",
    request_body = ExplainRequest,
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Decisión, plan certificado y recibo", body = SemanticExplanation),
        (status = 401, description = "Sin sesión", body = Problem, content_type = "application/problem+json"),
        (status = 403, description = "Un uso no permitido. Sin permiso de inspección del catálogo, solo si todo lo que la consulta nombra (y lo que unen los roles que escribe) es visible para la sesión con `view` o `view_masked`; si no, responde el 404. Lo que el plan lee por linaje se deniega sin nombre", body = SemanticDenial, content_type = "application/problem+json"),
        (status = 404, description = "`model_not_found`: el modelo no existe en el proyecto, o la consulta no se puede explicar a esta sesión. Sin permiso de inspección del catálogo (`view` sobre `Model::\"project:catalog\"`), un archivo del proyecto ausente o inválido, cualquier fallo al enlazar la consulta con el modelo (nombres desconocidos o ambiguos incluidos) y un miembro o un rol hacia datos que la sesión no puede ver responden igual, con el detalle fijo «No se puede explicar la consulta solicitada.»: no confirma qué existe", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "Petición con formato inválido; consulta incorrecta sobre miembros visibles; con permiso de inspección del catálogo, también modelo con errores y fallos al enlazar la consulta", body = SemanticRejection, content_type = "application/problem+json"),
        (status = 429, description = "Demasiadas compilaciones simultáneas", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "Políticas no válidas, motor o auditoría no disponibles, o metadatos no listos (`metadata_not_ready`)", body = Problem, content_type = "application/problem+json"),
        (status = 504, description = "Tiempo agotado", body = Problem, content_type = "application/problem+json")
    )
)]
pub async fn explain(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    id: Option<Extension<RequestId>>,
    body: Result<Json<ExplainRequest>, JsonRejection>,
) -> Response {
    let started = Instant::now();
    let request_id = id
        .as_ref()
        .and_then(|Extension(id)| id.header_value().to_str().ok().map(str::to_owned))
        .unwrap_or_default();
    let outcome = match body {
        Ok(Json(request)) => run(&state, &principal, request).await,
        Err(_) => Outcome::Refused(
            "La petición no es un JSON válido con el formato de la consulta semántica.".to_owned(),
        ),
    };
    let audited = match &outcome {
        Outcome::Explained { audit, .. }
        | Outcome::Denied { audit, .. }
        | Outcome::Invalid {
            audit: Some(audit), ..
        }
        | Outcome::Undisclosed { audit: Some(audit) } => {
            record(
                &state,
                &principal,
                id.as_ref().map(|Extension(id)| id),
                audit,
            )
            .await
        }
        _ => Ok(()),
    };
    let audited = match (&outcome, audited) {
        (Outcome::Inspected { fingerprint, .. }, Ok(())) => {
            models::catalog::Event::answered(fingerprint.clone(), true)
                .record(
                    &state.db,
                    &principal.session.user.id,
                    &request_id,
                    state.query_timeout,
                )
                .await
        }
        (_, audited) => audited,
    };
    let label = outcome.label();
    tracing::info!(
        request_id,
        outcome = label,
        elapsed_ms = started.elapsed().as_millis(),
        "semantic query explained"
    );
    if let Err(code) = audited {
        // Nothing is answered without its audit event.
        return Problem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            code,
            Some("No se pudo registrar la auditoría. Reintenta.".to_owned()),
            &request_id,
        )
        .into_response();
    }
    outcome.into_response(&request_id)
}

/// What an explanation needs from the audit, decided before writing it.
#[derive(Clone)]
struct Audit {
    allowed: bool,
    valid: bool,
    model: String,
    decision: String,
    shape: (u32, u32, u32, u32),
}

enum Outcome {
    Explained {
        explanation: Box<SemanticExplanation>,
        audit: Audit,
    },
    Denied {
        denial: (String, Option<SemanticDeniedUse>),
        audit: Audit,
    },
    Invalid {
        code: &'static str,
        message: String,
        diagnostics: Vec<SemanticDiagnostic>,
        audit: Option<Audit>,
    },
    Refused(String),
    Failed(StatusCode, &'static str, &'static str),
    /// A failure that would tell a session without inspection of the
    /// catalog what the project holds (#156): one answer for all of them,
    /// with the audit of the decision when one was taken.
    Undisclosed {
        audit: Option<Audit>,
    },
    /// Something of the catalog released under inspection: its decision is
    /// recorded first, and nothing leaves without that event.
    Inspected {
        outcome: Box<Outcome>,
        fingerprint: String,
    },
}

/// The only detail a session without inspection gets for a failure that
/// depends on the catalog.
const UNDISCLOSED: &str = "No se puede explicar la consulta solicitada.";

impl Outcome {
    const fn label(&self) -> &'static str {
        match self {
            Self::Explained { .. } => "explained",
            Self::Denied { .. } => "denied",
            Self::Invalid { .. } | Self::Refused(_) => "invalid",
            Self::Failed(..) => "failed",
            Self::Undisclosed { .. } => "undisclosed",
            Self::Inspected { outcome, .. } => outcome.label(),
        }
    }

    fn into_response(self, request_id: &str) -> Response {
        let no_store = [(header::CACHE_CONTROL, "no-store")];
        match self {
            Self::Explained { explanation, .. } => (no_store, Json(explanation)).into_response(),
            Self::Denied {
                denial: (reason, denied),
                ..
            } => {
                let detail = if denied.as_ref().is_some_and(|d| d.name.is_some()) {
                    "Tus roles en este proyecto no permiten ese uso de la consulta."
                } else {
                    "La consulta lee datos que tus roles en este proyecto no permiten usar."
                };
                let mut response = Json(SemanticDenial {
                    problem: Problem::new(
                        StatusCode::FORBIDDEN,
                        "semantic_access_denied",
                        Some(detail.to_owned()),
                        request_id,
                    ),
                    reason,
                    denied,
                })
                .into_response();
                *response.status_mut() = StatusCode::FORBIDDEN;
                problem_headers(response)
            }
            Self::Invalid {
                code,
                message,
                diagnostics,
                ..
            } => {
                let mut response = Json(SemanticRejection {
                    problem: Problem::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        code,
                        Some(message),
                        request_id,
                    ),
                    diagnostics,
                })
                .into_response();
                *response.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
                problem_headers(response)
            }
            Self::Refused(message) => Self::Invalid {
                code: "invalid_request",
                message,
                diagnostics: Vec::new(),
                audit: None,
            }
            .into_response(request_id),
            Self::Failed(status, code, message) => {
                Problem::new(status, code, Some(message.to_owned()), request_id).into_response()
            }
            Self::Inspected { outcome, .. } => outcome.into_response(request_id),
            Self::Undisclosed { .. } => Problem::new(
                StatusCode::NOT_FOUND,
                "model_not_found",
                Some(UNDISCLOSED.to_owned()),
                request_id,
            )
            .into_response(),
        }
    }
}

fn problem_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(crate::problem::CONTENT_TYPE),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

async fn run(state: &AppState, principal: &Principal, request: ExplainRequest) -> Outcome {
    // Rendered where it was prepared: within the admission permit and the
    // query deadline, off the async executor.
    // Without the instance's key there is no public context fingerprint,
    // and nothing is read to find out.
    if !state.context_key.is_available() {
        return Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "explain_unavailable",
            "No se pudo proteger la huella del contexto.",
        );
    }
    let key = state.context_key.clone();
    let project = state
        .models
        .directory
        .as_deref()
        .map(|directory| directory.to_string_lossy().into_owned())
        .unwrap_or_default();
    let subject = principal.session.user.id.clone();
    let session = prepare::Session::of(principal);
    let render = move |prepared: prepare::Prepared| {
        let scope = context::Scope {
            project: &project,
            principal: &subject,
        };
        explained(&prepared, &key, &scope)
    };
    match prepare::prepare_with(state, session, &request, render).await {
        Ok(outcome) => outcome,
        Err(prepare::Refusal(outcome)) => *outcome,
    }
}

/// Explain, as one consumer of the preparation: the decision, the certified
/// plan and the receipt, all read from what was prepared.
fn explained(
    prepared: &prepare::Prepared,
    key: &context::ContextKey,
    scope: &context::Scope<'_>,
) -> Outcome {
    let decision = prepared.decision();
    let Some(block) = prepared.query().blocks.first() else {
        return Outcome::Refused("La consulta no tiene bloques.".to_owned());
    };
    let Some(context) = key.fingerprint(scope, prepared.context()) else {
        return Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "explain_unavailable",
            "No se pudo proteger la huella del contexto.",
        );
    };
    let receipt = receipt(
        prepared.query_fingerprint().to_owned(),
        prepared.model().content_hash().to_string(),
        prepared.plan_fingerprint().to_owned(),
        decision.fingerprint.to_string(),
        prepared.policies().to_owned(),
        context,
    );
    let obligations = prepared
        .validated()
        .blocks
        .iter()
        .map(|block| block.obligations.len())
        .sum::<usize>();
    Outcome::Explained {
        explanation: Box::new(SemanticExplanation {
            decision: ExplainDecision {
                reason: decision.reason.code().to_owned(),
                masked: masked(decision, block),
                roles: prepared.roles().to_vec(),
            },
            plan: ExplainPlan {
                explanation: sef_planner::explain(prepared.model(), prepared.plan()),
                obligations: u32::try_from(obligations).unwrap_or(u32::MAX),
            },
            receipt,
        }),
        audit: prepared.audit().clone(),
    }
}

/// Whether everything the request itself refers to may be shown to this
/// session: each object it named, and each dataset a relationship role it
/// wrote joins. Shown means an effective `view`, or `view_masked` after a
/// plain refusal, on the same snapshot, as `sef_authz::authorize_query`
/// decides projection. Writing a name proves nothing, and neither does a
/// `forbid`, an evaluation error or an invalid request.
fn request_is_disclosable(
    access: &Access,
    principal: &sef_authz::Principal,
    uses: &access::Uses,
) -> bool {
    // Before planning, a join use comes only from a role the request wrote.
    uses.list
        .iter()
        .filter(|used| uses.is_named(&used.resource.id) || used.usage == sef_authz::Usage::Join)
        .all(|used| shown(access, principal, &used.resource))
}

/// Whether `resource` may be shown to this session on this snapshot: an
/// effective `view`, or `view_masked` after a plain refusal, as
/// `sef_authz::authorize_query` decides projection.
fn shown(
    access: &Access,
    principal: &sef_authz::Principal,
    resource: &sef_authz::Resource,
) -> bool {
    use sef_authz::{Action, Reason};
    let context = sef_authz::Context::new();
    let decide =
        |action| sef_authz::authorize(&access.snapshot, principal, action, resource, &context);
    let view = decide(Action::View);
    view.allowed
        || (matches!(view.reason, Reason::NoPermit | Reason::Forbidden)
            && decide(Action::ViewMasked).allowed)
}

/// A denial names the object only when the request named it: what a
/// relationship joins or a metric derives from stays unnamed.
fn denied(
    decision: &QueryDecision,
    model: &ModelDefinition,
    uses: &access::Uses,
    audit: Audit,
) -> Outcome {
    let denied = decision
        .denied
        .as_ref()
        .map(|(kind, id, usage)| SemanticDeniedUse {
            kind: kind_code(*kind).to_owned(),
            name: uses.is_named(id).then(|| display_name(model, id)),
            usage: usage.code().to_owned(),
        });
    Outcome::Denied {
        denial: (decision.reason.code().to_owned(), denied),
        audit,
    }
}

const fn kind_code(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Model => "model",
        ResourceKind::Dataset => "dataset",
        ResourceKind::Metric => "metric",
        ResourceKind::Dimension => "dimension",
        ResourceKind::Dashboard => "dashboard",
        ResourceKind::Source => "source",
        ResourceKind::User => "user",
        ResourceKind::Role => "role",
        ResourceKind::AuditLog => "audit_log",
    }
}

/// The name a person knows an object by: its label, or its identifier's name.
fn display_name(model: &ModelDefinition, id: &str) -> String {
    let label = model
        .metrics
        .iter()
        .map(|m| &m.metadata)
        .chain(model.dimensions.iter().map(|d| &d.metadata))
        .chain(model.datasets.iter().map(|d| &d.metadata))
        .find(|metadata| metadata.id().as_str() == id)
        .and_then(|metadata| metadata.label.clone());
    label.unwrap_or_else(|| id.rsplit('/').next().unwrap_or(id).to_owned())
}

/// Selections whose object the decision masks.
fn masked(decision: &QueryDecision, block: &SemanticQueryBlock) -> Vec<String> {
    let masked: Vec<&str> = decision
        .constraints
        .iter()
        .map(|Constraint::Mask { id, .. }| id.as_str())
        .collect();
    block
        .selections
        .iter()
        .filter(|selection| {
            let id = match &selection.item {
                SelectionItem::Metric(metric) => metric,
                SelectionItem::Dimension(dimension) => &dimension.dimension,
                SelectionItem::TimeDimension(time) => &time.dimension.dimension,
            };
            masked.contains(&id.as_str())
        })
        .map(|selection| selection.selection_id.clone())
        .collect()
}

/// Values for the context slots of the model's row policies, from the
/// verified session attributes of the matching type; a missing one stays
/// missing and validation reports it.
fn context_slots(
    model: &ModelDefinition,
    attributes: &BTreeMap<String, serde_json::Value>,
) -> SemanticContext {
    let mut context = SemanticContext::default();
    for slot in model.policies.iter().flat_map(|p| &p.context_slots) {
        let value = attributes
            .get(&slot.name)
            .and_then(|value| match (&slot.value_type, value) {
                (ValueType::Utf8, serde_json::Value::String(text)) => {
                    Some(Literal::Utf8(text.clone()))
                }
                (ValueType::Boolean, serde_json::Value::Bool(flag)) => {
                    Some(Literal::Boolean(*flag))
                }
                (ValueType::Integer { .. }, serde_json::Value::Number(number)) => {
                    number.as_i64().map(|n| Literal::SignedInteger(n.into()))
                }
                _ => None,
            });
        if let Some(value) = value {
            context.slots.insert(slot.name.clone(), value);
        }
    }
    context
}

fn receipt(
    query: String,
    model: String,
    plan: String,
    decision: String,
    policies: String,
    context: String,
) -> SemanticReceipt {
    // Length-prefixed, so no two different inputs encode alike.
    let digest = |parts: &[&str]| {
        let mut hasher = Sha256::new();
        for part in parts {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        hex(&hasher.finalize())
    };
    let id = format!(
        "receipt:{}",
        digest(&[&query, &model, &plan, &decision, &policies, &context])
    );
    SemanticReceipt {
        id,
        query,
        model,
        plan,
        decision,
        policies,
        context,
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// A model error comes before any authorization: only its stable code
/// leaves the server, never the objects its message names. Whoever
/// maintains the model reads the details in the Models screen.
fn model_diagnostic(diagnostic: ModelDiagnostic) -> SemanticDiagnostic {
    SemanticDiagnostic {
        code: diagnostic.code,
        phase: "model".to_owned(),
        message: "El modelo tiene un error en su definición.".to_owned(),
        path: None,
    }
}

/// Binding messages repeat what the request wrote, except an ambiguous
/// name, which lists the identifiers it could mean.
fn binding_diagnostic(error: sef_query_ir::QueryError) -> SemanticDiagnostic {
    let message = if error.code == sef_query_ir::QueryErrorCode::AmbiguousName {
        "Ese nombre corresponde a más de un objeto del modelo; usa un nombre que lo distinga."
            .to_owned()
    } else {
        error.message
    };
    SemanticDiagnostic {
        code: error.code.as_str().to_owned(),
        phase: "binding".to_owned(),
        message,
        path: Some(error.path.replacen("/blocks/0", "/query", 1)),
    }
}

/// A correctness or planning message is shown only when everything it is
/// about was named by the request and its text mentions no other object of
/// the model, by identifier or by name: references need not cover every
/// name a message quotes. Otherwise only its stable code leaves.
fn semantic_diagnostic(
    diagnostic: sef_model_ir::diagnostic::Diagnostic,
    model: &ModelDefinition,
    uses: &access::Uses,
) -> SemanticDiagnostic {
    use sef_model_ir::diagnostic::Phase;
    let refs: Vec<&StableId> = diagnostic
        .primary_ref
        .iter()
        .chain(&diagnostic.related_refs)
        .collect();
    let disclosed = !refs.is_empty()
        && refs.iter().all(|id| uses.is_named(id.as_str()))
        && !mentions_unnamed(&diagnostic.message, model, uses);
    let (phase, generic) = match diagnostic.phase {
        Phase::Model => ("model", "El modelo tiene un error en su definición."),
        Phase::Correctness => (
            "correctness",
            "La consulta no tiene un resultado correcto con este modelo.",
        ),
        Phase::Planning => (
            "planning",
            "El planificador no puede garantizar el resultado de esta consulta.",
        ),
    };
    SemanticDiagnostic {
        code: diagnostic.code.as_str().to_owned(),
        phase: phase.to_owned(),
        message: if disclosed {
            diagnostic.message
        } else {
            generic.to_owned()
        },
        path: None,
    }
}

/// Whether `message` quotes an object of the model the request did not
/// name, by its identifier or by the last segment of it. Erring towards
/// silence is safe: a redacted message keeps its stable code.
fn mentions_unnamed(message: &str, model: &ModelDefinition, uses: &access::Uses) -> bool {
    let ids = std::iter::once(&model.metadata)
        .chain(model.datasets.iter().map(|d| &d.metadata))
        .chain(
            model
                .datasets
                .iter()
                .flat_map(|d| d.fields.iter().map(|f| &f.metadata)),
        )
        .chain(model.dimensions.iter().map(|d| &d.metadata))
        .chain(model.measures.iter().map(|m| &m.metadata))
        .chain(model.metrics.iter().map(|m| &m.metadata))
        .chain(model.relationships.iter().map(|r| &r.metadata))
        .chain(model.bridges.iter().map(|b| &b.metadata))
        .chain(model.policies.iter().map(|p| &p.metadata))
        .chain(model.entities.iter().map(|e| &e.metadata))
        .map(|metadata| metadata.id().as_str());
    let short = |id: &str| id.rsplit('/').next().unwrap_or(id).to_owned();
    // A short name the request itself used reveals nothing new.
    let known: std::collections::BTreeSet<String> = uses.named_ids().map(short).collect();
    ids.filter(|id| !uses.is_named(id)).any(|id| {
        let name = short(id);
        message.contains(id)
            || (!name.is_empty() && !known.contains(&name) && message.contains(&name))
    })
}

/// Appends the explanation's audit event (#71): who, which model, what
/// was decided and the redacted shape of the query.
async fn record(
    state: &AppState,
    principal: &Principal,
    id: Option<&RequestId>,
    audit: &Audit,
) -> Result<(), &'static str> {
    const UNAVAILABLE: &str = "audit_unavailable";
    let event_id = crate::auth::random_token()
        .await
        .map_err(|()| UNAVAILABLE)?;
    let (request_id, trace_id) = crate::audit::correlation(
        id.and_then(|id| id.header_value().to_str().ok())
            .unwrap_or_default(),
    );
    let (dimensions, measures, filters, limit) = audit.shape;
    let event = bi_audit::Event {
        event_id,
        occurred_at: std::time::SystemTime::now().into(),
        actor_id: principal.session.user.id.clone(),
        actor_type: bi_audit::ActorType::User,
        session_id: None,
        action: if audit.allowed {
            bi_audit::Action::Query
        } else {
            bi_audit::Action::Denial
        },
        resource_type: "semantic_model".to_owned(),
        resource_id: audit.model.clone(),
        result: match (audit.allowed, audit.valid) {
            (false, _) => bi_audit::ResultCode::Denied,
            (true, true) => bi_audit::ResultCode::Success,
            (true, false) => bi_audit::ResultCode::Failure,
        },
        request_id,
        trace_id,
        policy_decision_id: Some(audit.decision.clone()),
        source: bi_audit::Source::Http,
        metadata_redacted: bi_audit::MetadataRedacted {
            query_shape: Some(bi_audit::QueryShape {
                dimensions,
                measures,
                filters,
                limit,
            }),
        },
        schema_version: bi_audit::SCHEMA_VERSION,
    };
    let mut tx = state
        .db
        .begin()
        .await
        .map_err(|error| error.problem_code(UNAVAILABLE))?;
    crate::audit::append(&mut tx, &event)
        .await
        .map_err(|_| UNAVAILABLE)?;
    tx.commit().await.map_err(|_| UNAVAILABLE)
}
