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
pub mod catalog;
pub mod context;
pub mod envelope;
pub mod prepare;
pub mod request;
pub mod result;
pub mod run;

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

/// Executes a semantic query and answers its exact result with its
/// receipt, once its audit event is confirmed (#138).
#[utoipa::path(
    post,
    path = "/api/v1/semantic/query",
    operation_id = "executeSemanticQuery",
    tag = "semantic",
    request_body = ExplainRequest,
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Resultado exacto completo, recibo y avisos; solo después de confirmar su evento de auditoría. Sin `limit`, una consulta con dimensiones recibe 1000 filas ordenadas por sus dimensiones; nunca más de 10000", body = envelope::SemanticRun),
        (status = 401, description = "Sin sesión", body = Problem, content_type = "application/problem+json"),
        (status = 403, description = "Un uso no permitido, con las mismas reglas de divulgación que explain", body = SemanticDenial, content_type = "application/problem+json"),
        (status = 404, description = "`model_not_found`: el modelo no existe o la consulta no se puede ejecutar para esta sesión. Sin permiso de inspección del catálogo, todo fallo que dependa del catálogo responde igual, con el detalle fijo «No se puede ejecutar la consulta solicitada.»", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "Petición o consulta inválida (`invalid_request`, `query_invalid`, `query_incorrect`, `query_unplannable`, `model_invalid` con inspección), `limit_invalid` (limit 1–10000, offset hasta 10000), `masked_unsupported`, `temporal_unsupported`, `result_too_large` o `result_out_of_range` (el resultado numérico exacto no cabe en su tipo certificado: SUM de DECIMAL(p,s) es DECIMAL(max(p,38),s) y SUM de INT64 es DECIMAL(38,0))", body = SemanticRejection, content_type = "application/problem+json"),
        (status = 429, description = "`models_busy` o `semantic_busy`: sin capacidad para preparar o ejecutar", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "Políticas no válidas, metadatos no listos, motor no disponible o fallido (`analytics_unavailable`, `analytics_failed`), resultado no verificable (`result_unverified`), `semantic_unavailable` o auditoría no confirmada (`audit_unavailable`): sin resultado ni recibo", body = Problem, content_type = "application/problem+json"),
        (status = 504, description = "`semantic_timeout`: la consulta superó el tiempo máximo, contado desde su admisión", body = Problem, content_type = "application/problem+json")
    )
)]
pub async fn query(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    id: Option<Extension<RequestId>>,
    body: Result<Json<ExplainRequest>, JsonRejection>,
) -> Response {
    // One deadline from admission: execution, then its audit event.
    let admitted = Instant::now();
    let request_id = id
        .as_ref()
        .and_then(|Extension(id)| id.header_value().to_str().ok().map(str::to_owned))
        .unwrap_or_default();
    let Ok(Json(request)) = body else {
        return Outcome::Refused(
            "La petición no es un JSON válido con el formato de la consulta semántica.".to_owned(),
        )
        .into_response(&request_id);
    };
    let shape = prepare::shape(&request);
    let session = prepare::Session::of(&principal);
    let executed = run::execute(&state, session, request, &request_id, admitted).await;
    let settled = Settled::of(executed, shape);
    let label = settled.label();
    // Execution ended within its share; the event gets what is left.
    let timeout = state.semantic_runs.profile().budgets().timeout;
    let audited = match &settled.terminal {
        Some(terminal) => {
            let left = timeout.saturating_sub(admitted.elapsed());
            terminal.record(&state, &principal, &request_id, left).await
        }
        None => Ok(()),
    };
    tracing::info!(
        request_id,
        outcome = label,
        audited = audited.is_ok(),
        elapsed_ms = admitted.elapsed().as_millis(),
        "semantic query answered"
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
    settled.reply.into_response(&request_id)
}

/// What the session can use of the project, to discover it (#140).
#[derive(Debug, Serialize, ToSchema)]
pub struct SemanticCatalog {
    pub status: CatalogStatus,
    /// In the order of their files; only models with a metric the session
    /// can use. Empty is a valid answer: it tells nothing of what is hidden.
    pub models: Vec<catalog::CatalogModel>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CatalogStatus {
    Ready,
    /// This instance serves no project.
    NotConfigured,
}

/// What a catalog event names: the project's reserved catalog resource.
const SEMANTIC_CATALOG: (&str, &str) = ("semantic_catalog", "semantic:catalog");

/// Lists what the session can use of the project: the projection of #139,
/// answered once its audit event is confirmed (#140).
#[utoipa::path(
    get,
    path = "/api/v1/semantic/catalog",
    operation_id = "listSemanticCatalog",
    tag = "semantic",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Modelos, métricas y campos que las consultas de esta sesión aceptan, cada uno con lo que puede hacer con ellos; solo después de confirmar su evento de auditoría. Vacío es válido y no indica qué se oculta. Sin proyecto, `not_configured` vacío", body = SemanticCatalog),
        (status = 401, description = "Sin sesión", body = Problem, content_type = "application/problem+json"),
        (status = 429, description = "`models_busy`: sin capacidad para compilar el proyecto", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "`policies_invalid`, `catalog_unavailable` (proyecto o uno de sus archivos ilegible, o motor no utilizable) o `audit_unavailable`: sin catálogo. Un archivo cuya definición no compila queda fuera del catálogo, sin nombrarlo ni contarlo, y los demás modelos siguen disponibles", body = Problem, content_type = "application/problem+json"),
        (status = 504, description = "`semantic_timeout`: el catálogo superó el tiempo máximo", body = Problem, content_type = "application/problem+json")
    )
)]
pub async fn list_catalog(
    State(state): State<AppState>,
    Extension(principal): Extension<Principal>,
    id: Option<Extension<RequestId>>,
) -> Response {
    // One deadline: the projection within execution's share, then its
    // audit event with what is left.
    let admitted = Instant::now();
    let request_id = id
        .as_ref()
        .and_then(|Extension(id)| id.header_value().to_str().ok().map(str::to_owned))
        .unwrap_or_default();
    let runs = &state.semantic_runs;
    let deadline = admitted + runs.profile().execution_timeout();
    let session = prepare::Session::of(&principal);
    let projected = catalog::within(&state, session, deadline).await;
    let projection = admitted.elapsed();
    let (result, decision, answer) = match projected {
        Ok(projection) => (
            bi_audit::ResultCode::Success,
            projection.decision,
            Ok(SemanticCatalog {
                status: CatalogStatus::Ready,
                models: projection.catalog.models,
            }),
        ),
        Err(catalog::CatalogFailure {
            error: catalog::CatalogError::NotConfigured,
            ..
        }) => (
            bi_audit::ResultCode::Success,
            None,
            Ok(SemanticCatalog {
                status: CatalogStatus::NotConfigured,
                models: Vec::new(),
            }),
        ),
        // The decisions taken before the failure name its event.
        Err(failure) => (
            bi_audit::ResultCode::Failure,
            failure.decision,
            Err(failure.error),
        ),
    };
    let (resource_type, resource_id) = SEMANTIC_CATALOG;
    let left = runs
        .profile()
        .budgets()
        .timeout
        .saturating_sub(admitted.elapsed());
    let audited = crate::audit::record_within(
        &state.db,
        crate::audit::HttpRecord {
            actor: &principal.session.user.id,
            request_id: &request_id,
            action: bi_audit::Action::Query,
            result,
            resource_type,
            resource_id,
            policy_decision_id: decision,
            query_shape: None,
        },
        left,
    )
    .await;
    let code = match (&audited, &answer) {
        (Err(code), _) => code,
        (Ok(()), Ok(_)) => "listed",
        (Ok(()), Err(error)) => error.code(),
    };
    runs.catalog_answered(
        code,
        projection,
        admitted.elapsed().saturating_sub(projection),
    );
    tracing::info!(
        request_id,
        outcome = code,
        elapsed_ms = admitted.elapsed().as_millis(),
        "semantic catalog answered"
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
    match answer {
        Ok(catalog) => ([(header::CACHE_CONTROL, "no-store")], Json(catalog)).into_response(),
        Err(error) => {
            let (status, code, message) = catalog_problem(error);
            Problem::new(status, code, Some(message.to_owned()), &request_id).into_response()
        }
    }
}

/// The neutral answer of a catalog that could not be projected: never a
/// diagnostic of a file, a name or a partial list.
const fn catalog_problem(error: catalog::CatalogError) -> (StatusCode, &'static str, &'static str) {
    match error {
        catalog::CatalogError::Busy => (
            StatusCode::TOO_MANY_REQUESTS,
            "models_busy",
            "Ya hay dos compilaciones en curso. Reintenta en unos segundos.",
        ),
        catalog::CatalogError::Timeout => (
            StatusCode::GATEWAY_TIMEOUT,
            "semantic_timeout",
            "El catálogo superó el tiempo máximo. Reintenta.",
        ),
        catalog::CatalogError::PoliciesInvalid => (
            StatusCode::SERVICE_UNAVAILABLE,
            "policies_invalid",
            "Las políticas o los miembros del proyecto no son válidos: nadie puede consultar \
             hasta corregirlos.",
        ),
        catalog::CatalogError::Unavailable | catalog::CatalogError::NotConfigured => (
            StatusCode::SERVICE_UNAVAILABLE,
            "catalog_unavailable",
            "El catálogo no está disponible. Reintenta.",
        ),
    }
}

/// The only detail a session without inspection gets for an execution
/// that depends on the catalog.
const UNDISCLOSED_RUN: &str = "No se puede ejecutar la consulta solicitada.";

/// How one execution ended: its single terminal event and its answer,
/// decided before anything is written.
struct Settled {
    terminal: Option<Terminal>,
    reply: Reply,
}

enum Reply {
    Run(Box<envelope::SemanticRun>),
    Outcome(Outcome),
    Problem(StatusCode, &'static str, &'static str),
}

impl Settled {
    fn of(executed: Result<run::Answer, run::Failed>, shape: (u32, u32, u32, u32)) -> Self {
        let failed = match executed {
            Ok(answer) => {
                return Self {
                    terminal: Some(Terminal::of(&answer.audit, false)),
                    reply: Reply::Run(Box::new(answer.run)),
                };
            }
            Err(failed) => failed,
        };
        // The shape as bounded, its applied limit included, when it got there.
        let shape = failed.shape.unwrap_or(shape);
        let terminal = failed.audit.as_ref().map_or_else(
            || Terminal::undecided(shape),
            |audit| Terminal::of(audit, true),
        );
        let (status, code, message) = match failed.failure {
            run::RunFailure::Refused(prepare::Refusal(outcome)) => {
                return Self::refused(*outcome, shape);
            }
            run::RunFailure::LimitInvalid => {
                return Self {
                    terminal: None,
                    reply: Reply::Problem(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "limit_invalid",
                        "limit va de 1 a 10000 y offset de 0 a 10000.",
                    ),
                };
            }
            run::RunFailure::MaskedUnsupported => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "masked_unsupported",
                "La consulta usa valores que tus roles solo permiten ver enmascarados, y la \
                 ejecución todavía no los aplica.",
            ),
            run::RunFailure::TemporalUnsupported => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "temporal_unsupported",
                "Agrupar por periodo un instante sin semántica temporal gobernada todavía no se \
                 puede ejecutar.",
            ),
            run::RunFailure::OutOfRange => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "result_out_of_range",
                "El resultado numérico está fuera del rango admitido por la consulta.",
            ),
            run::RunFailure::Budget => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "result_too_large",
                "El resultado supera el tamaño máximo. Añade filtros, menos dimensiones o un \
                 límite menor.",
            ),
            run::RunFailure::Busy => (
                StatusCode::TOO_MANY_REQUESTS,
                "semantic_busy",
                "El motor analítico está ocupado. Reintenta en unos segundos.",
            ),
            run::RunFailure::Deadline => (
                StatusCode::GATEWAY_TIMEOUT,
                "semantic_timeout",
                "La consulta superó el tiempo máximo. Simplifica la consulta y reintenta.",
            ),
            run::RunFailure::Engine | run::RunFailure::Cancelled => (
                StatusCode::SERVICE_UNAVAILABLE,
                "analytics_failed",
                "El motor analítico no pudo completar la consulta. Reintenta.",
            ),
            run::RunFailure::Incomplete | run::RunFailure::Evidence | run::RunFailure::Codec => (
                StatusCode::SERVICE_UNAVAILABLE,
                "result_unverified",
                "No se pudo verificar el resultado completo, y no se entrega ninguna parte.",
            ),
            run::RunFailure::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "semantic_unavailable",
                "La ejecución semántica no está disponible. Reintenta.",
            ),
        };
        Self {
            terminal: Some(terminal),
            reply: Reply::Problem(status, code, message),
        }
    }

    /// A refusal of the preparation, classified as explain classifies it,
    /// with its single terminal event: the decision when there was one, the
    /// reserved request resource if not. Released under inspection, it
    /// records no second event.
    fn refused(outcome: Outcome, shape: (u32, u32, u32, u32)) -> Self {
        let outcome = match outcome {
            Outcome::Inspected { outcome, .. } => *outcome,
            outcome => outcome,
        };
        let terminal = match &outcome {
            Outcome::Denied { audit, .. }
            | Outcome::Invalid {
                audit: Some(audit), ..
            }
            | Outcome::Undisclosed { audit: Some(audit) } => Some(Terminal::of(audit, false)),
            Outcome::Invalid { audit: None, .. }
            | Outcome::Undisclosed { audit: None }
            | Outcome::Failed(..) => Some(Terminal::undecided(shape)),
            // Refused within the request's own bounds, before preparing.
            Outcome::Refused(_) | Outcome::Explained { .. } | Outcome::Inspected { .. } => None,
        };
        Self {
            terminal,
            reply: Reply::Outcome(outcome),
        }
    }

    const fn label(&self) -> &'static str {
        match &self.reply {
            Reply::Run(_) => "answered",
            Reply::Outcome(outcome) => outcome.label(),
            Reply::Problem(_, code, _) => code,
        }
    }
}

impl Reply {
    fn into_response(self, request_id: &str) -> Response {
        match self {
            Self::Run(answer) => {
                ([(header::CACHE_CONTROL, "no-store")], Json(*answer)).into_response()
            }
            Self::Outcome(outcome) => outcome.answer(request_id, UNDISCLOSED_RUN),
            // A 422 keeps the contract of every semantic rejection.
            Self::Problem(StatusCode::UNPROCESSABLE_ENTITY, code, message) => Outcome::Invalid {
                code,
                message: message.to_owned(),
                diagnostics: Vec::new(),
                audit: None,
            }
            .answer(request_id, UNDISCLOSED_RUN),
            Self::Problem(status, code, message) => {
                Problem::new(status, code, Some(message.to_owned()), request_id).into_response()
            }
        }
    }
}

/// The terminal audit event of one execution (#138).
struct Terminal {
    action: bi_audit::Action,
    result: bi_audit::ResultCode,
    /// The model's fingerprint; `None` before any decision.
    model: Option<String>,
    decision: Option<String>,
    shape: (u32, u32, u32, u32),
}

/// What an event names when no model was decided on.
const SEMANTIC_REQUEST: (&str, &str) = ("semantic_request", "semantic:request");

impl Terminal {
    /// The decision taken; `failed` when the execution did not complete.
    fn of(audit: &Audit, failed: bool) -> Self {
        Self {
            action: if audit.allowed {
                bi_audit::Action::Query
            } else {
                bi_audit::Action::Denial
            },
            result: match (audit.allowed, audit.valid && !failed) {
                (false, _) => bi_audit::ResultCode::Denied,
                (true, true) => bi_audit::ResultCode::Success,
                (true, false) => bi_audit::ResultCode::Failure,
            },
            model: Some(audit.model.clone()),
            decision: Some(audit.decision.clone()),
            shape: audit.shape,
        }
    }

    /// A request that entered the preparation and failed before deciding.
    const fn undecided(shape: (u32, u32, u32, u32)) -> Self {
        Self {
            action: bi_audit::Action::Query,
            result: bi_audit::ResultCode::Failure,
            model: None,
            decision: None,
            shape,
        }
    }

    /// Appends and confirms the event within `budget`, what is left of the
    /// request's deadline.
    async fn record(
        &self,
        state: &AppState,
        principal: &Principal,
        request_id: &str,
        budget: std::time::Duration,
    ) -> Result<(), &'static str> {
        let (resource_type, resource_id) = match &self.model {
            Some(model) => ("semantic_model", model.as_str()),
            None => SEMANTIC_REQUEST,
        };
        let (dimensions, measures, filters, limit) = self.shape;
        crate::audit::record_within(
            &state.db,
            crate::audit::HttpRecord {
                actor: &principal.session.user.id,
                request_id,
                action: self.action,
                result: self.result,
                resource_type,
                resource_id,
                policy_decision_id: self.decision.clone(),
                query_shape: Some(bi_audit::QueryShape {
                    dimensions,
                    measures,
                    filters,
                    limit,
                }),
            },
            budget,
        )
        .await
    }
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
        self.answer(request_id, UNDISCLOSED)
    }

    /// The answer, with `undisclosed` as the one detail of a failure that
    /// depends on the catalog.
    fn answer(self, request_id: &str, undisclosed: &str) -> Response {
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
            .answer(request_id, undisclosed),
            Self::Failed(status, code, message) => {
                Problem::new(status, code, Some(message.to_owned()), request_id).into_response()
            }
            Self::Inspected { outcome, .. } => outcome.answer(request_id, undisclosed),
            Self::Undisclosed { .. } => Problem::new(
                StatusCode::NOT_FOUND,
                "model_not_found",
                Some(undisclosed.to_owned()),
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
