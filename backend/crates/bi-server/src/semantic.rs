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
pub mod request;

use std::collections::BTreeMap;
use std::time::Instant;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Extension, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use sef_authz::{Constraint, QueryDecision, ResourceKind, authorize_query};
use sef_correctness::SemanticContext;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::ModelDefinition;
use sef_model_ir::types::{Literal, ValueType};
use sef_query_ir::query::{SelectionItem, SemanticQueryBlock};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tower_http::request_id::RequestId;
use utoipa::ToSchema;

use self::access::{Access, AccessError, Resources};
use self::request::ExplainRequest;
use crate::AppState;
use crate::auth::Principal;
use crate::models::{self, ModelDiagnostic, ProjectFile};
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
    /// The session values the model's row policies read; never the values.
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
        (status = 403, description = "Un uso no permitido para los roles de la sesión", body = SemanticDenial, content_type = "application/problem+json"),
        (status = 404, description = "El modelo no existe en el proyecto", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "Consulta mal formada, modelo con errores o consulta incorrecta", body = SemanticRejection, content_type = "application/problem+json"),
        (status = 429, description = "Demasiadas compilaciones simultáneas", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "Políticas no válidas, motor o auditoría no disponibles", body = Problem, content_type = "application/problem+json"),
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
        } => {
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
    let label = outcome.label();
    tracing::info!(
        request_id,
        outcome = label,
        elapsed_ms = started.elapsed().as_millis(),
        "semantic query explained"
    );
    if audited.is_err() {
        // Nothing is answered without its audit event.
        return Problem::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "audit_unavailable",
            Some("No se pudo registrar la auditoría. Reintenta.".to_owned()),
            &request_id,
        )
        .into_response();
    }
    outcome.into_response(&request_id)
}

/// What an explanation needs from the audit, decided before writing it.
struct Audit {
    allowed: bool,
    valid: bool,
    model: String,
    decision: String,
    shape: (u32, u32, u32, u32),
}

enum Outcome {
    Explained {
        explanation: SemanticExplanation,
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
}

impl Outcome {
    const fn label(&self) -> &'static str {
        match self {
            Self::Explained { .. } => "explained",
            Self::Denied { .. } => "denied",
            Self::Invalid { .. } | Self::Refused(_) => "invalid",
            Self::Failed(..) => "failed",
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
    // Bounds first: nothing is read for a request over them.
    let unbound = match request.query.unbound() {
        Ok(unbound) => unbound,
        Err(message) => return Outcome::Refused(message),
    };
    let (dimensions, metrics, filters) = request.query.shape();
    let limit = request
        .query
        .limit
        .map_or(0, |limit| u32::try_from(limit).unwrap_or(u32::MAX));
    let shape = (dimensions, metrics, filters, limit);
    let Some(directory) = state.models.directory.clone() else {
        return Outcome::Failed(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "No hay un proyecto configurado.",
        );
    };
    let Some(file) = request.model.strip_prefix("metrics/").map(str::to_owned) else {
        return Outcome::Failed(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "El modelo no existe en el proyecto. Usa el archivo que lista /api/v1/models.",
        );
    };
    let Ok(permit) = state.models.admission.clone().try_acquire_owned() else {
        return Outcome::Failed(
            StatusCode::TOO_MANY_REQUESTS,
            "models_busy",
            "Ya hay dos compilaciones en curso. Reintenta en unos segundos.",
        );
    };
    let span = tracing::Span::current();
    let read = tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        let project = models::read_project(&directory);
        let access = access::load(&directory);
        (project, access, permit)
    })
    .await;
    let Ok((project, access, permit)) = read else {
        return Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "models_unavailable",
            "No se pudo leer el proyecto.",
        );
    };
    let Some(file) = project
        .ok()
        .and_then(|files| files.into_iter().find(|f| f.name == file))
    else {
        return Outcome::Failed(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "El modelo no existe en el proyecto. Usa el archivo que lista /api/v1/models.",
        );
    };
    let access = match access {
        Ok(access) => access,
        Err(error) => {
            if let AccessError::Invalid(problems) = &error {
                tracing::warn!(problems = problems.len(), "project policies are invalid");
            }
            return Outcome::Failed(
                StatusCode::SERVICE_UNAVAILABLE,
                "policies_invalid",
                "Las políticas o los miembros del proyecto no son válidos: nadie puede consultar \
                 hasta corregirlos.",
            );
        }
    };
    let Some(runtime) = state.analytics.runtime() else {
        return Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "analytics_unavailable",
            "El motor analítico no está listo. Reintenta en unos segundos.",
        );
    };
    let session = Session::of(principal);
    let span = tracing::Span::current();
    let work = runtime.run(move |conn| {
        let _permit = permit;
        let _entered = span.enter();
        Ok(explain_file(file, conn, &access, &session, &unbound, shape))
    });
    match tokio::time::timeout(state.query_timeout, work).await {
        Err(_) => Outcome::Failed(
            StatusCode::GATEWAY_TIMEOUT,
            "explain_timeout",
            "La explicación superó el tiempo máximo. Simplifica la consulta y reintenta.",
        ),
        Ok(Err(_)) => Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "analytics_unavailable",
            "No se pudo consultar el catálogo analítico. Reintenta.",
        ),
        Ok(Ok(outcome)) => outcome,
    }
}

/// What the path knows of who asks, copied from the verified session.
struct Session {
    user: String,
    groups: Vec<String>,
    attributes: BTreeMap<String, serde_json::Value>,
}

impl Session {
    fn of(principal: &Principal) -> Self {
        let identity = principal.federated_identity.as_ref();
        Self {
            user: principal.session.user.id.clone(),
            groups: identity.map(|i| i.groups.clone()).unwrap_or_default(),
            attributes: identity.map(|i| i.attributes.clone()).unwrap_or_default(),
        }
    }
}

fn explain_file(
    file: ProjectFile,
    conn: &bi_runtime::duckdb::Connection,
    access: &Access,
    session: &Session,
    unbound: &sef_query_ir::unbound::UnboundQuerySet,
    shape: (u32, u32, u32, u32),
) -> Outcome {
    let compiled = match models::compile(file, conn) {
        Ok(compiled) => compiled,
        Err(rejected) => {
            return Outcome::Invalid {
                code: "model_invalid",
                message: "El modelo tiene errores. Quien lo mantiene los ve en la pantalla \
                          Modelos."
                    .to_owned(),
                diagnostics: rejected
                    .diagnostics
                    .into_iter()
                    .map(model_diagnostic)
                    .collect(),
                audit: None,
            };
        }
    };
    let model = &compiled.document.semantic;
    let definition = model.definition();
    let bound = match sef_query_ir::bind(unbound, definition) {
        Ok(bound) => bound,
        Err(errors) => {
            return Outcome::Invalid {
                code: "query_invalid",
                message: "La consulta no corresponde al modelo.".to_owned(),
                diagnostics: errors.into_iter().map(binding_diagnostic).collect(),
                audit: None,
            };
        }
    };
    decide(model, &bound, access, session, shape)
}

/// Authorizes, validates and plans a bound query.
fn decide(
    model: &sef_model_ir::model::CompiledSemanticModel,
    bound: &sef_query_ir::query::SemanticQuerySet,
    access: &Access,
    session: &Session,
    shape: (u32, u32, u32, u32),
) -> Outcome {
    let definition = model.definition();
    let Some(block) = bound.blocks.first() else {
        return Outcome::Refused("La consulta no tiene bloques.".to_owned());
    };
    let resources = Resources::of(definition);
    let principal = access::principal(&access.members, &session.user, &session.groups);
    let roles: Vec<String> = principal.roles.iter().cloned().collect();
    let context = sef_authz::Context::new();
    let mut uses = access::requested(&resources, definition, block);
    let model_hash = model.content_hash().to_string();
    let audit = |decision: &QueryDecision, valid: bool| Audit {
        allowed: decision.allowed,
        valid,
        model: model_hash.clone(),
        decision: decision.fingerprint.to_string(),
        shape,
    };
    // First what the query asks for, before validating anything.
    let asked = authorize_query(&access.snapshot, &principal, &uses.list, &context);
    if !asked.allowed {
        return denied(&asked, definition, &uses, audit(&asked, true));
    }
    let slots = context_slots(definition, &session.attributes);
    let validated = match sef_correctness::validate(model, bound, &slots) {
        Ok(validated) => validated,
        Err(diagnostics) => {
            return Outcome::Invalid {
                code: "query_incorrect",
                message: "La consulta no tiene un resultado correcto con este modelo.".to_owned(),
                diagnostics: diagnostics
                    .into_iter()
                    .map(|d| semantic_diagnostic(d, &uses))
                    .collect(),
                audit: Some(audit(&asked, false)),
            };
        }
    };
    let planned = match sef_planner::plan(model, &validated) {
        Ok(planned) => planned,
        Err(diagnostics) => {
            return Outcome::Invalid {
                code: "query_unplannable",
                message: "El planificador no puede garantizar el resultado de esta consulta."
                    .to_owned(),
                diagnostics: diagnostics
                    .into_iter()
                    .map(|d| semantic_diagnostic(d, &uses))
                    .collect(),
                audit: Some(audit(&asked, false)),
            };
        }
    };
    // Then everything the plan reads, on the same snapshot.
    access::read_by_plan(&mut uses, &resources, definition, &planned.plan);
    let decision = authorize_query(&access.snapshot, &principal, &uses.list, &context);
    if !decision.allowed {
        return denied(&decision, definition, &uses, audit(&decision, true));
    }
    let (Ok(query), Ok(plan)) = (bound.fingerprint(), planned.plan.fingerprint()) else {
        return Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "explain_unavailable",
            "No se pudo calcular la huella de la consulta.",
        );
    };
    let receipt = receipt(
        query.to_string(),
        model_hash.clone(),
        plan.to_string(),
        decision.fingerprint.to_string(),
        access.snapshot.version().to_owned(),
        &slots,
    );
    let obligations = validated
        .blocks
        .iter()
        .map(|block| block.obligations.len())
        .sum::<usize>();
    Outcome::Explained {
        explanation: SemanticExplanation {
            decision: ExplainDecision {
                reason: decision.reason.code().to_owned(),
                masked: masked(&decision, block),
                roles,
            },
            plan: ExplainPlan {
                explanation: sef_planner::explain(model, &planned.plan),
                obligations: u32::try_from(obligations).unwrap_or(u32::MAX),
            },
            receipt,
        },
        audit: audit(&decision, true),
    }
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
    slots: &SemanticContext,
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
    let values: Vec<String> = slots
        .slots
        .iter()
        .flat_map(|(name, value)| [name.clone(), format!("{value:?}")])
        .collect();
    let values: Vec<&str> = values.iter().map(String::as_str).collect();
    let context = format!("sha256:{}", digest(&values));
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

/// A correctness or planning message is shown only when every object it is
/// about was named by the request; otherwise only its stable code.
fn semantic_diagnostic(
    diagnostic: sef_model_ir::diagnostic::Diagnostic,
    uses: &access::Uses,
) -> SemanticDiagnostic {
    use sef_model_ir::diagnostic::Phase;
    let refs: Vec<&StableId> = diagnostic
        .primary_ref
        .iter()
        .chain(&diagnostic.related_refs)
        .collect();
    let disclosed = !refs.is_empty() && refs.iter().all(|id| uses.is_named(id.as_str()));
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

/// Appends the explanation's audit event (#71): who, which model, what
/// was decided and the redacted shape of the query.
async fn record(
    state: &AppState,
    principal: &Principal,
    id: Option<&RequestId>,
    audit: &Audit,
) -> Result<(), ()> {
    let event_id = crate::auth::random_token().await?;
    let raw = id
        .and_then(|id| id.header_value().to_str().ok())
        .unwrap_or_default();
    let request_id = if !raw.is_empty()
        && raw.len() <= 128
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
    {
        raw.to_owned()
    } else {
        format!("request:{}", hex(&Sha256::digest(raw.as_bytes())))
    };
    // Correlation reference until distributed tracing supplies a trace id.
    let trace_id = hex(&Sha256::digest(request_id.as_bytes()))[..32].to_owned();
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
    let mut tx = state.db.pool().begin().await.map_err(|_| ())?;
    crate::audit::append(&mut tx, &event)
        .await
        .map_err(|_| ())?;
    tx.commit().await.map_err(|_| ())
}
