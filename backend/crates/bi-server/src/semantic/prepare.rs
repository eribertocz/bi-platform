//! The governed preparation of one semantic query (#135): read once,
//! decided twice on that read, validated and planned. Explain renders it;
//! execution will run it. Nothing here executes the query.
//!
//! A [`Prepared`] exists only when both authorizations allowed the query
//! and validation and planning succeeded; anything else is a [`Refusal`],
//! which can be answered but never consumed as a success.

use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::http::StatusCode;
use sef_authz::{QueryDecision, authorize_query};
use sef_correctness::{SemanticContext, ValidatedQuery};
use sef_exec_ir::SemanticPlan;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{CompiledSemanticModel, Lifecycle};
use sef_planner::Certificate;
use sef_query_ir::query::{SelectionItem, SemanticQuerySet};
use sef_query_ir::unbound::UnboundQuerySet;
use tokio::sync::OwnedSemaphorePermit;

use super::access::{self, Access, AccessError, Resources};
use super::request::ExplainRequest;
use super::{
    Audit, Outcome, binding_diagnostic, context_slots, denied, model_diagnostic,
    request_is_disclosable, semantic_diagnostic, shown,
};
use crate::AppState;
use crate::auth::Principal;
use crate::models::{self, Compiled, ModelDiagnostic, ProjectFile};

/// Who asks, copied from the verified session: its roles come from the
/// project, its attributes from the verified identity, never from the
/// request.
pub struct Session {
    pub(super) user: String,
    pub(super) groups: Vec<String>,
    pub(super) attributes: BTreeMap<String, serde_json::Value>,
}

impl Session {
    pub(super) fn of(principal: &Principal) -> Self {
        let identity = principal.federated_identity.as_ref();
        Self::verified(
            principal.session.user.id.clone(),
            principal.verified_groups().to_vec(),
            identity.map(|i| i.attributes.clone()).unwrap_or_default(),
        )
    }

    /// A session whose identity, groups and attributes were already
    /// verified by its caller (the HTTP edge, or a test standing for it).
    #[must_use]
    pub const fn verified(
        user: String,
        groups: Vec<String>,
        attributes: BTreeMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            user,
            groups,
            attributes,
        }
    }
}

/// How much a failure may say to this session.
pub(super) struct Disclosure {
    /// The fingerprint of an allowed `view` on the project's catalog
    /// (#124): what Modelos already shows. Releasing anything of the
    /// catalog under it is recorded like the Modelos route records it.
    inspection: Option<String>,
}

impl Disclosure {
    const fn inspects(&self) -> bool {
        self.inspection.is_some()
    }

    /// `outcome`, released under inspection: recorded before it leaves.
    fn inspected(&self, outcome: Outcome) -> Outcome {
        match &self.inspection {
            Some(fingerprint) => Outcome::Inspected {
                outcome: Box::new(outcome),
                fingerprint: fingerprint.clone(),
            },
            None => outcome,
        }
    }
}

/// Why a query was not prepared: an answer for the client, never a plan.
pub struct Refusal(pub(super) Box<Outcome>);

/// A metric the query selects that its model marks deprecated. The
/// replacement is named only when this session may see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleNotice {
    pub metric: StableId,
    pub sunset: Option<String>,
    pub replacement: Option<StableId>,
}

/// A query decided, validated and planned on one read of the project.
pub struct Prepared {
    compiled: Compiled,
    bound: SemanticQuerySet,
    context: SemanticContext,
    validated: ValidatedQuery,
    plan: SemanticPlan,
    certificate: Certificate,
    decision: QueryDecision,
    roles: Vec<String>,
    policies: String,
    query_fingerprint: String,
    plan_fingerprint: String,
    notices: Vec<LifecycleNotice>,
    audit: Audit,
}

impl Prepared {
    #[must_use]
    pub const fn model(&self) -> &CompiledSemanticModel {
        &self.compiled.document.semantic
    }

    /// The engine's description of the sources, captured with the model.
    #[must_use]
    pub const fn sources(&self) -> &sef_ossie::catalog::Catalog {
        &self.compiled.catalog
    }

    #[must_use]
    pub const fn query(&self) -> &SemanticQuerySet {
        &self.bound
    }

    /// The verified context the row policies bind.
    #[must_use]
    pub const fn context(&self) -> &SemanticContext {
        &self.context
    }

    #[must_use]
    pub const fn validated(&self) -> &ValidatedQuery {
        &self.validated
    }

    #[must_use]
    pub const fn plan(&self) -> &SemanticPlan {
        &self.plan
    }

    #[must_use]
    pub const fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    /// The final decision: everything the plan reads, authorized.
    #[must_use]
    pub const fn decision(&self) -> &QueryDecision {
        &self.decision
    }

    #[must_use]
    pub fn roles(&self) -> &[String] {
        &self.roles
    }

    /// The version of the policy snapshot both decisions used.
    #[must_use]
    pub fn policies(&self) -> &str {
        &self.policies
    }

    #[must_use]
    pub fn query_fingerprint(&self) -> &str {
        &self.query_fingerprint
    }

    #[must_use]
    pub fn plan_fingerprint(&self) -> &str {
        &self.plan_fingerprint
    }

    #[must_use]
    pub fn notices(&self) -> &[LifecycleNotice] {
        &self.notices
    }

    pub(super) const fn audit(&self) -> &Audit {
        &self.audit
    }
}

/// What one request reads from the project, on a single access snapshot.
pub(super) struct Loaded {
    project: Result<Vec<ProjectFile>, ModelDiagnostic>,
    access: Access,
    disclosure: Disclosure,
    session: Session,
    permit: OwnedSemaphorePermit,
}

/// Prepares `request` for `session`: the project read once, then decided,
/// validated and planned on that read, off the executor.
///
/// # Errors
/// A [`Refusal`] for anything that is not a fully authorized, valid and
/// planned query.
pub async fn prepare(
    state: &AppState,
    session: Session,
    request: &ExplainRequest,
) -> Result<Prepared, Refusal> {
    let loaded = load_request(state, session, request).await?;
    prepare_loaded(state, loaded, request).await
}

/// The first phase: bounds, admission and one read of the project.
pub(super) async fn load_request(
    state: &AppState,
    session: Session,
    request: &ExplainRequest,
) -> Result<Loaded, Refusal> {
    // Bounds first: nothing is read for a request over them.
    request
        .query
        .unbound()
        .map_err(|m| Refusal(Box::new(Outcome::Refused(m))))?;
    let Some(directory) = state.models.directory.clone() else {
        return Err(Refusal(Box::new(Outcome::Failed(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "No hay un proyecto configurado.",
        ))));
    };
    if !request.model.starts_with("metrics/") {
        return Err(Refusal(Box::new(Outcome::Failed(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "El modelo no existe en el proyecto. Usa el archivo que lista /api/v1/models.",
        ))));
    }
    let Ok(permit) = state.models.admission.clone().try_acquire_owned() else {
        return Err(Refusal(Box::new(Outcome::Failed(
            StatusCode::TOO_MANY_REQUESTS,
            "models_busy",
            "Ya hay dos compilaciones en curso. Reintenta en unos segundos.",
        ))));
    };
    load(directory, session, permit)
        .await
        .map_err(|(status, code, message)| {
            Refusal(Box::new(Outcome::Failed(status, code, message)))
        })
}

/// Reads the access configuration, decides on it whether this session
/// inspects the catalog, and reads the project, off the executor. An
/// unusable configuration closes the path before anything says whether a
/// file exists.
async fn load(
    directory: PathBuf,
    session: Session,
    permit: OwnedSemaphorePermit,
) -> Result<Loaded, (StatusCode, &'static str, &'static str)> {
    let span = tracing::Span::current();
    let read = tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        let access = access::load(&directory);
        // On the same snapshot; it only sets how much a failure may say,
        // never what the query may use.
        let inspection = access.as_ref().ok().and_then(|access| {
            let principal = access::principal(&access.members, &session.user, &session.groups);
            match models::catalog::decide_for(&access.snapshot, &principal) {
                models::catalog::Verdict::Allowed { fingerprint } => Some(fingerprint),
                _ => None,
            }
        });
        let project = models::read_project(&directory);
        (project, access, inspection, session, permit)
    })
    .await;
    let Ok((project, access, inspection, session, permit)) = read else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "models_unavailable",
            "No se pudo leer el proyecto.",
        ));
    };
    let access = match access {
        Ok(access) => access,
        Err(error) => {
            if let AccessError::Invalid(problems) = &error {
                tracing::warn!(problems = problems.len(), "project policies are invalid");
            }
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                "policies_invalid",
                "Las políticas o los miembros del proyecto no son válidos: nadie puede consultar \
                 hasta corregirlos.",
            ));
        }
    };
    Ok(Loaded {
        project,
        access,
        disclosure: Disclosure { inspection },
        session,
        permit,
    })
}

/// The second phase, on what [`load_request`] read and nothing else: the
/// file is compiled, the query bound, decided, validated and planned.
pub(super) async fn prepare_loaded(
    state: &AppState,
    loaded: Loaded,
    request: &ExplainRequest,
) -> Result<Prepared, Refusal> {
    let Loaded {
        project,
        access,
        disclosure,
        session,
        permit,
    } = loaded;
    // An engine that is down answers before the file is looked up, the same
    // whether it exists or not.
    let Some(runtime) = state.analytics.runtime() else {
        return Err(Refusal(Box::new(Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "analytics_unavailable",
            "El motor analítico no está listo. Reintenta en unos segundos.",
        ))));
    };
    let name = request.model.strip_prefix("metrics/").unwrap_or_default();
    let Some(file) = project
        .ok()
        .and_then(|files| files.into_iter().find(|f| f.name == name))
    else {
        if !disclosure.inspects() {
            return Err(Refusal(Box::new(Outcome::Undisclosed { audit: None })));
        }
        return Err(Refusal(Box::new(disclosure.inspected(Outcome::Failed(
            StatusCode::NOT_FOUND,
            "model_not_found",
            "El modelo no existe en el proyecto. Usa el archivo que lista /api/v1/models.",
        )))));
    };
    let unbound = request
        .query
        .unbound()
        .map_err(|m| Refusal(Box::new(Outcome::Refused(m))))?;
    let shape = shape(request);
    let span = tracing::Span::current();
    let work = runtime.run(move |conn| {
        let _permit = permit;
        let _entered = span.enter();
        let input = Input {
            access: &access,
            session: &session,
            unbound: &unbound,
            shape,
            disclosure: &disclosure,
        };
        Ok(prepare_file(file, conn, &input))
    });
    match tokio::time::timeout(state.query_timeout, work).await {
        Err(_) => Err(Refusal(Box::new(Outcome::Failed(
            StatusCode::GATEWAY_TIMEOUT,
            "explain_timeout",
            "La explicación superó el tiempo máximo. Simplifica la consulta y reintenta.",
        )))),
        Ok(Err(_)) => Err(Refusal(Box::new(Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "analytics_unavailable",
            "No se pudo consultar el catálogo analítico. Reintenta.",
        )))),
        Ok(Ok(prepared)) => prepared.map_err(Refusal),
    }
}

/// The redacted shape the audit records: counts and the limit.
fn shape(request: &ExplainRequest) -> (u32, u32, u32, u32) {
    let (dimensions, metrics, filters) = request.query.shape();
    let limit = request
        .query
        .limit
        .map_or(0, |limit| u32::try_from(limit).unwrap_or(u32::MAX));
    (dimensions, metrics, filters, limit)
}

/// What the second phase works from, all captured by the first.
struct Input<'a> {
    access: &'a Access,
    session: &'a Session,
    unbound: &'a UnboundQuerySet,
    shape: (u32, u32, u32, u32),
    disclosure: &'a Disclosure,
}

fn prepare_file(
    file: ProjectFile,
    conn: &bi_runtime::duckdb::Connection,
    input: &Input<'_>,
) -> Result<Prepared, Box<Outcome>> {
    let (access, session, disclosure) = (input.access, input.session, input.disclosure);
    let compiled = match models::compile(file, conn) {
        Ok(compiled) => compiled,
        Err(_) if !disclosure.inspects() => {
            return Err(Box::new(Outcome::Undisclosed { audit: None }));
        }
        Err(rejected) => {
            return Err(Box::new(
                disclosure.inspected(Outcome::Invalid {
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
                }),
            ));
        }
    };
    let definition = compiled.document.semantic.definition();
    let resources = Resources::of(definition);
    let principal = access::principal(&access.members, &session.user, &session.groups);
    // Without inspection, names resolve among what this session may see
    // first: a hidden object never makes a visible name ambiguous. When only
    // hidden objects answer a name, the request is still decided (and its
    // denial recorded), but answered like a name the model does not have.
    let bound = if disclosure.inspects() {
        sef_query_ir::bind(input.unbound, definition)
    } else {
        let visible = |id: &StableId| {
            resources
                .get(id)
                .is_some_and(|resource| shown(access, &principal, resource))
        };
        sef_query_ir::bind_visible(input.unbound, definition, &visible)
            .or_else(|_| sef_query_ir::bind(input.unbound, definition))
    };
    let bound = match bound {
        Ok(bound) => bound,
        // Any binding failure would tell what the model declares.
        Err(_) if !disclosure.inspects() => {
            return Err(Box::new(Outcome::Undisclosed { audit: None }));
        }
        Err(errors) => {
            return Err(Box::new(disclosure.inspected(Outcome::Invalid {
                code: "query_invalid",
                message: "La consulta no corresponde al modelo.".to_owned(),
                diagnostics: errors.into_iter().map(binding_diagnostic).collect(),
                audit: None,
            })));
        }
    };
    let decided = Decided {
        resources: &resources,
        principal: &principal,
    };
    decide(compiled, bound, input, &decided)
}

/// Who asks and what the model's resources are, computed once per request.
struct Decided<'a> {
    resources: &'a Resources,
    principal: &'a sef_authz::Principal,
}

/// Authorizes, validates and plans a bound query.
fn decide(
    compiled: Compiled,
    bound: SemanticQuerySet,
    input: &Input<'_>,
    decided: &Decided<'_>,
) -> Result<Prepared, Box<Outcome>> {
    let (access, session, disclosure) = (input.access, input.session, input.disclosure);
    let model = &compiled.document.semantic;
    let definition = model.definition();
    let Some(block) = bound.blocks.first() else {
        return Err(Box::new(Outcome::Refused(
            "La consulta no tiene bloques.".to_owned(),
        )));
    };
    let (resources, principal) = (decided.resources, decided.principal);
    let context = sef_authz::Context::new();
    let mut uses = access::requested(resources, definition, block);
    let model_hash = model.content_hash().to_string();
    let audit = |decision: &QueryDecision, valid: bool| Audit {
        allowed: decision.allowed,
        valid,
        model: model_hash.clone(),
        decision: decision.fingerprint.to_string(),
        shape: input.shape,
    };
    // First what the query asks for, before validating anything.
    let asked = authorize_query(&access.snapshot, principal, &uses.list, &context);
    if !asked.allowed {
        let audit = audit(&asked, true);
        // Named only when everything the request refers to is visible to
        // it: the first refused use never decides for the others.
        if !disclosure.inspects()
            && (asked.denied.is_none() || !request_is_disclosable(access, principal, &uses))
        {
            return Err(Box::new(Outcome::Undisclosed { audit: Some(audit) }));
        }
        return Err(Box::new(denied(&asked, definition, &uses, audit)));
    }
    let slots = context_slots(definition, &session.attributes);
    let validated = match sef_correctness::validate(model, &bound, &slots) {
        Ok(validated) => validated,
        Err(diagnostics) => {
            return Err(Box::new(Outcome::Invalid {
                code: "query_incorrect",
                message: "La consulta no tiene un resultado correcto con este modelo.".to_owned(),
                diagnostics: diagnostics
                    .into_iter()
                    .map(|d| semantic_diagnostic(d, definition, &uses))
                    .collect(),
                audit: Some(audit(&asked, false)),
            }));
        }
    };
    let planned = match sef_planner::plan(model, &validated) {
        Ok(planned) => planned,
        Err(diagnostics) => {
            return Err(Box::new(Outcome::Invalid {
                code: "query_unplannable",
                message: "El planificador no puede garantizar el resultado de esta consulta."
                    .to_owned(),
                diagnostics: diagnostics
                    .into_iter()
                    .map(|d| semantic_diagnostic(d, definition, &uses))
                    .collect(),
                audit: Some(audit(&asked, false)),
            }));
        }
    };
    // Then everything the plan reads, on the same snapshot.
    access::read_by_plan(&mut uses, resources, definition, &planned.plan);
    let decision = authorize_query(&access.snapshot, principal, &uses.list, &context);
    if !decision.allowed {
        return Err(Box::new(denied(
            &decision,
            definition,
            &uses,
            audit(&decision, true),
        )));
    }
    let (Ok(query), Ok(plan)) = (bound.fingerprint(), planned.plan.fingerprint()) else {
        return Err(Box::new(Outcome::Failed(
            StatusCode::SERVICE_UNAVAILABLE,
            "explain_unavailable",
            "No se pudo calcular la huella de la consulta.",
        )));
    };
    let notices = notices(model, &bound, |id| {
        resources
            .get(id)
            .is_some_and(|resource| shown(access, principal, resource))
    });
    let audit = audit(&decision, true);
    Ok(Prepared {
        roles: principal.roles.iter().cloned().collect(),
        policies: access.snapshot.version().to_owned(),
        query_fingerprint: query.to_string(),
        plan_fingerprint: plan.to_string(),
        context: slots,
        validated,
        plan: planned.plan,
        certificate: planned.certificate,
        decision,
        notices,
        audit,
        bound,
        compiled,
    })
}

/// The deprecated metrics the query selects, in selection order, with a
/// replacement only when `visible` admits it.
fn notices(
    model: &CompiledSemanticModel,
    bound: &SemanticQuerySet,
    visible: impl Fn(&StableId) -> bool,
) -> Vec<LifecycleNotice> {
    let definition = model.definition();
    bound
        .blocks
        .iter()
        .flat_map(|block| &block.selections)
        .filter_map(|selection| match &selection.item {
            SelectionItem::Metric(id) => Some(id),
            _ => None,
        })
        .filter_map(|id| {
            let metric = definition.metrics.iter().find(|m| m.metadata.id() == id)?;
            match &metric.metadata.governance.lifecycle {
                Lifecycle::Deprecated {
                    sunset,
                    replacement,
                } => Some(LifecycleNotice {
                    metric: id.clone(),
                    sunset: sunset.clone(),
                    replacement: replacement.clone().filter(|r| visible(r)),
                }),
                Lifecycle::Active => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
