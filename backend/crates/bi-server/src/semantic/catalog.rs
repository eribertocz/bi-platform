//! The catalog one session can use (#139): the models, metrics and fields
//! a semantic query of this session would accept, each with what it may
//! do with them. Internal: the business catalog route (#140) answers it.
//!
//! Nothing here is a second authorizer. A metric is announced only when a
//! minimal query that selects it alone goes through the single path
//! ([`prepare::govern`]): allowed as asked, validated with the session's
//! verified context, planned, and allowed again on everything its plan
//! reads, dependencies included. A field is announced only when the same
//! holds for a minimal query that selects it next to one announced metric.
//! A validation or planning failure means "not usable alone", never a
//! denial. Every capability is one more decision of `authorize_query` over
//! the minimal query's uses: showing a name grants nothing.
//!
//! Inspecting the project's catalog (`project:catalog`) is never consulted:
//! it shows definitions in Modelos and grants no data here. Nothing is
//! executed against the sources and no decision is cached: the project,
//! its policies and members are read once per call, and the engine only
//! describes the sources while each file compiles.

use std::collections::BTreeSet;
use std::time::Instant;

use sef_authz::{Constraint, Usage, Use, authorize_query};
use sef_correctness::SemanticContext;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{Lifecycle, ModelDefinition, ObjectMetadata};
use sef_model_ir::types::ValueType;
use sef_model_ir::units::Unit;
use sef_query_ir::query::{
    QualifiedDimensionRef, Selection, SelectionItem, SemanticQueryBlock, SemanticQuerySet,
};
use serde::Serialize;

use super::access::{self, Access, AccessError, Resources};
use super::context_slots;
use super::prepare::{self, Decided, Governed, Session, Ungoverned};
use super::result::{SemanticUnit, type_labels};
use crate::AppState;
use crate::models::{self, Compiled, ProjectFile};

/// How many announced metrics a field is tried with, in model order, before
/// it is left out: a bound on the work, never a promise that every
/// combination is compatible.
pub const MAX_PAIRINGS_PER_FIELD: usize = 16;

/// What one session can use of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionCatalog {
    /// In the order of their files.
    pub models: Vec<CatalogModel>,
}

/// One model with at least one metric the session can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogModel {
    /// `metrics/<file>`: what a semantic query names as its `model`.
    pub model: String,
    /// The model's name as written.
    pub name: String,
    pub metrics: Vec<CatalogMember>,
    pub fields: Vec<CatalogMember>,
}

/// A metric or field, by the name a query uses for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogMember {
    /// The name as written in the model.
    pub name: String,
    /// `DECIMAL(p,s)`, `INT64`, `VARCHAR`, `BOOLEAN`, `DATE` or `TIMESTAMPTZ`,
    /// as a query result declares it.
    #[serde(rename = "type")]
    pub value_type: String,
    pub unit: Option<SemanticUnit>,
    pub lifecycle: CatalogLifecycle,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CatalogLifecycle {
    Active,
    Deprecated {
        /// ISO `YYYY-MM-DD`, when a retirement date is declared.
        sunset: Option<String>,
        /// Only when the replacement is itself in this catalog.
        replacement: Option<String>,
    },
}

/// What a query of this session may do with a member, each decided on its
/// own: selecting it never implies filtering, grouping or ordering by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    pub select: Select,
    pub filter: bool,
    /// Always `false` for a metric: a metric is aggregated, never a group.
    pub group: bool,
    pub order: bool,
}

/// How a selection of the member returns its values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Select {
    Values,
    /// As typed `NULL`, flagged as masked.
    Masked,
}

/// Why there is no catalog. Neutral: never a partial catalog, never a
/// diagnostic of a file or of what the project hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogError {
    /// This instance serves no project.
    NotConfigured,
    /// The compilations admitted at once are all taken.
    Busy,
    /// The work outlived `query_timeout`.
    Timeout,
    /// The project's policies or members are unreadable or invalid: closed.
    PoliciesInvalid,
    /// The project, one of its files or the engine could not be used.
    Unavailable,
}

/// The work one projection did, for its bounds and its tests.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Work {
    /// Files of `metrics/` read.
    pub files: usize,
    /// Files compiled (each describes its sources to the engine).
    pub compiled: usize,
    /// Minimal queries sent through the single path.
    pub governed: usize,
    /// Of those, the ones planned.
    pub planned: usize,
    /// Decisions taken beyond the single path: a field's own uses before
    /// pairing it, and each capability.
    pub decisions: usize,
}

/// The catalog `session` can use, read and decided once, off the executor,
/// within the admission of compilations and `query_timeout`.
///
/// # Errors
/// A neutral [`CatalogError`]: nothing of the catalog is answered then.
pub async fn for_session(
    state: &AppState,
    session: Session,
) -> Result<SessionCatalog, CatalogError> {
    project_with_work(state, session)
        .await
        .map(|(catalog, _)| catalog)
}

/// [`for_session`] with the work it did.
pub(super) async fn project_with_work(
    state: &AppState,
    session: Session,
) -> Result<(SessionCatalog, Work), CatalogError> {
    let started = Instant::now();
    let projected = projected(state, session, started).await;
    let (outcome, models, metrics, fields) = match &projected {
        Ok((catalog, _)) => (
            "projected",
            catalog.models.len(),
            catalog.models.iter().map(|m| m.metrics.len()).sum(),
            catalog.models.iter().map(|m| m.fields.len()).sum(),
        ),
        Err(error) => (error.code(), 0, 0, 0),
    };
    let work = projected
        .as_ref()
        .map(|(_, work)| *work)
        .unwrap_or_default();
    // Counts only: no name, identity or path of the project.
    tracing::info!(
        outcome,
        models,
        metrics,
        fields,
        files = work.files,
        governed = work.governed,
        elapsed_ms = started.elapsed().as_millis(),
        "session catalog projected"
    );
    projected
}

impl CatalogError {
    const fn code(self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Busy => "busy",
            Self::Timeout => "timeout",
            Self::PoliciesInvalid => "policies_invalid",
            Self::Unavailable => "unavailable",
        }
    }
}

async fn projected(
    state: &AppState,
    session: Session,
    started: Instant,
) -> Result<(SessionCatalog, Work), CatalogError> {
    let deadline = started + state.query_timeout;
    let Some(directory) = state.models.directory.clone() else {
        return Err(CatalogError::NotConfigured);
    };
    let Ok(permit) = state.models.admission.clone().try_acquire_owned() else {
        return Err(CatalogError::Busy);
    };
    // One read of the policies, the members and the project. The permit
    // travels with the native work, also past the deadline.
    let span = tracing::Span::current();
    let read = tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        (
            access::load(&directory),
            models::read_project(&directory),
            permit,
        )
    });
    let (access, project, permit) = tokio::time::timeout_at(deadline.into(), read)
        .await
        .map_err(|_| CatalogError::Timeout)?
        .map_err(|_| CatalogError::Unavailable)?;
    let access = access.map_err(|error| {
        if let AccessError::Invalid(problems) = &error {
            tracing::warn!(problems = problems.len(), "project policies are invalid");
        }
        CatalogError::PoliciesInvalid
    })?;
    let files = project.map_err(|_| CatalogError::Unavailable)?;
    if files.is_empty() {
        let work = Work::default();
        return Ok((SessionCatalog { models: Vec::new() }, work));
    }
    let Some(runtime) = state.analytics.runtime() else {
        return Err(CatalogError::Unavailable);
    };
    let span = tracing::Span::current();
    let work = runtime.run(move |conn| {
        let _permit = permit;
        let _entered = span.enter();
        Ok(project_files(files, conn, &access, &session, deadline))
    });
    tokio::time::timeout_at(deadline.into(), work)
        .await
        .map_err(|_| CatalogError::Timeout)?
        .map_err(|_| CatalogError::Unavailable)?
}

/// Compiles every file, then projects each model. A file that does not
/// compile makes the whole catalog unavailable: no partial catalog.
fn project_files(
    files: Vec<ProjectFile>,
    conn: &bi_runtime::duckdb::Connection,
    access: &Access,
    session: &Session,
    deadline: Instant,
) -> Result<(SessionCatalog, Work), CatalogError> {
    let mut work = Work {
        files: files.len(),
        ..Work::default()
    };
    let mut compiled = Vec::with_capacity(files.len());
    for file in files {
        if Instant::now() >= deadline {
            return Err(CatalogError::Timeout);
        }
        let reference = format!("metrics/{}", file.name);
        work.compiled += 1;
        let file = models::compile(file, conn).map_err(|_| CatalogError::Unavailable)?;
        compiled.push((reference, file));
    }
    let principal = access::principal(&access.members, &session.user, &session.groups);
    let mut catalog = SessionCatalog { models: Vec::new() };
    for (reference, file) in compiled {
        let scope = Scope {
            access,
            principal: &principal,
            context: context_slots(file.document.semantic.definition(), &session.attributes),
            deadline,
        };
        if let Some(model) = project_model(reference, &file, &scope, &mut work)? {
            catalog.models.push(model);
        }
    }
    Ok((catalog, work))
}

/// What every decision of one model is taken on.
struct Scope<'a> {
    access: &'a Access,
    principal: &'a sef_authz::Principal,
    context: SemanticContext,
    deadline: Instant,
}

/// A member a minimal query accepted, with its capabilities.
struct Usable<'m> {
    metadata: &'m ObjectMetadata,
    value_type: String,
    unit: Option<&'m Unit>,
    capabilities: Capabilities,
}

fn project_model(
    reference: String,
    file: &Compiled,
    scope: &Scope<'_>,
    work: &mut Work,
) -> Result<Option<CatalogModel>, CatalogError> {
    let model = &file.document.semantic;
    let definition = model.definition();
    let resources = Resources::of(definition);
    let decided = Decided {
        resources: &resources,
        principal: scope.principal,
    };
    let declared: BTreeSet<String> = file
        .loaded
        .model
        .metrics
        .iter()
        .map(|metric| sef_ossie::ids::segment(&metric.name))
        .collect();
    let mut metrics: Vec<(&StableId, Usable<'_>)> = Vec::new();
    for metric in &definition.metrics {
        let id = metric.metadata.id();
        // Only what the file declares: an aggregate the compiler split out
        // of a metric is governed as that metric, but it is no name the
        // model's author wrote.
        if !id
            .as_str()
            .rsplit('/')
            .next()
            .is_some_and(|s| declared.contains(s))
        {
            continue;
        }
        let Some(value_type) = public_type(&metric.value_type) else {
            continue;
        };
        let query = minimal(vec![SelectionItem::Metric(id.clone())]);
        let Some(governed) = govern(model, &query, &decided, scope, work)? else {
            continue;
        };
        let capabilities = capabilities(&governed, &resources, id, false, scope, work);
        metrics.push((
            id,
            Usable {
                metadata: &metric.metadata,
                value_type,
                unit: metric.unit.as_ref(),
                capabilities,
            },
        ));
    }
    if metrics.is_empty() {
        return Ok(None);
    }
    let fields = fields(model, &metrics, &decided, scope, work)?;
    // A replacement is named only when it is itself announced.
    let announced: BTreeSet<&StableId> = metrics.iter().chain(&fields).map(|(id, _)| *id).collect();
    let member = |usable: Usable<'_>| CatalogMember {
        name: name(usable.metadata),
        value_type: usable.value_type,
        unit: usable.unit.map(SemanticUnit::from),
        lifecycle: lifecycle(usable.metadata, definition, &announced),
        capabilities: usable.capabilities,
    };
    Ok(Some(CatalogModel {
        model: reference,
        name: file.loaded.model.name.clone(),
        metrics: metrics
            .into_iter()
            .map(|(_, usable)| member(usable))
            .collect(),
        fields: fields
            .into_iter()
            .map(|(_, usable)| member(usable))
            .collect(),
    }))
}

/// The fields of `model` a minimal query accepts next to one of the
/// announced `metrics`, tried in model order up to
/// [`MAX_PAIRINGS_PER_FIELD`] of them.
fn fields<'m>(
    model: &'m sef_model_ir::model::CompiledSemanticModel,
    metrics: &[(&StableId, Usable<'_>)],
    decided: &Decided<'_>,
    scope: &Scope<'_>,
    work: &mut Work,
) -> Result<Vec<(&'m StableId, Usable<'m>)>, CatalogError> {
    let mut fields = Vec::new();
    for dimension in &model.definition().dimensions {
        let id = dimension.metadata.id();
        let Some(value_type) = public_type(&dimension.value_type) else {
            continue;
        };
        // Its own uses first, decided like any query decides them: a field
        // this session may not select and group is never paired.
        let Some(resource) = decided.resources.get(id) else {
            continue;
        };
        let own = [Usage::Project, Usage::Group].map(|usage| Use {
            resource: resource.clone(),
            usage,
        });
        work.decisions += 1;
        let request = sef_authz::Context::new();
        if !authorize_query(&scope.access.snapshot, scope.principal, &own, &request).allowed {
            continue;
        }
        for (metric, _) in metrics.iter().take(MAX_PAIRINGS_PER_FIELD) {
            let query = minimal(vec![
                SelectionItem::Metric((*metric).clone()),
                SelectionItem::Dimension(QualifiedDimensionRef {
                    dimension: id.clone(),
                    roles: Vec::new(),
                }),
            ]);
            if let Some(governed) = govern(model, &query, decided, scope, work)? {
                let capabilities =
                    capabilities(&governed, decided.resources, id, true, scope, work);
                fields.push((
                    id,
                    Usable {
                        metadata: &dimension.metadata,
                        value_type,
                        unit: dimension.unit.as_ref(),
                        capabilities,
                    },
                ));
                break;
            }
        }
    }
    Ok(fields)
}

/// A query with one block that selects `items`, and nothing else.
fn minimal(items: Vec<SelectionItem>) -> SemanticQuerySet {
    SemanticQuerySet {
        blocks: vec![SemanticQueryBlock {
            block_id: "catalog".to_owned(),
            selections: items
                .into_iter()
                .enumerate()
                .map(|(i, item)| Selection {
                    selection_id: format!("s{i}"),
                    item,
                    output_label: None,
                })
                .collect(),
            filter: None,
            post_aggregate_filter: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
        }],
    }
}

/// `query` through the single path, within the deadline: `None` when it is
/// refused, whether by a decision or because it has no correct, planned
/// result.
fn govern(
    model: &sef_model_ir::model::CompiledSemanticModel,
    query: &SemanticQuerySet,
    decided: &Decided<'_>,
    scope: &Scope<'_>,
    work: &mut Work,
) -> Result<Option<Governed>, CatalogError> {
    if Instant::now() >= scope.deadline {
        return Err(CatalogError::Timeout);
    }
    work.governed += 1;
    match prepare::govern(model, query, scope.access, decided, &scope.context) {
        Ok(governed) => {
            work.planned += 1;
            Ok(Some(governed))
        }
        Err(stopped) => {
            if matches!(*stopped, Ungoverned::Read { .. }) {
                work.planned += 1;
            }
            Ok(None)
        }
    }
}

/// What a query may do with `id`, on the minimal query `governed` accepted:
/// the selection as the final decision returns it, and each other usage as
/// one more decision over everything that query reads plus that use.
fn capabilities(
    governed: &Governed,
    resources: &Resources,
    id: &StableId,
    field: bool,
    scope: &Scope<'_>,
    work: &mut Work,
) -> Capabilities {
    let masked = governed
        .decision
        .constraints
        .iter()
        .any(|Constraint::Mask { id: masked, .. }| masked == id.as_str());
    let mut allows = |usage: Usage| {
        let Some(resource) = resources.get(id) else {
            return false;
        };
        let mut uses = governed.uses.list.clone();
        uses.push(Use {
            resource: resource.clone(),
            usage,
        });
        work.decisions += 1;
        let request = sef_authz::Context::new();
        authorize_query(&scope.access.snapshot, scope.principal, &uses, &request).allowed
    };
    Capabilities {
        select: if masked {
            Select::Masked
        } else {
            Select::Values
        },
        filter: allows(Usage::Filter),
        group: field && allows(Usage::Group),
        order: allows(Usage::Order),
    }
}

/// The type label a query result declares for the member; `None` when a
/// result could not carry its values, so the member is not announced.
fn public_type(value_type: &ValueType) -> Option<String> {
    type_labels(value_type).ok().map(|(public, _)| public)
}

/// The name as written: the label the compiler keeps for every member.
fn name(metadata: &ObjectMetadata) -> String {
    metadata.label.clone().unwrap_or_else(|| {
        let id = metadata.id().as_str();
        id.rsplit('/').next().unwrap_or(id).to_owned()
    })
}

fn lifecycle(
    metadata: &ObjectMetadata,
    definition: &ModelDefinition,
    announced: &BTreeSet<&StableId>,
) -> CatalogLifecycle {
    match &metadata.governance.lifecycle {
        Lifecycle::Active => CatalogLifecycle::Active,
        Lifecycle::Deprecated {
            sunset,
            replacement,
        } => CatalogLifecycle::Deprecated {
            sunset: sunset.clone(),
            replacement: replacement
                .as_ref()
                .filter(|id| announced.contains(id))
                .and_then(|id| {
                    definition
                        .metrics
                        .iter()
                        .map(|m| &m.metadata)
                        .chain(definition.dimensions.iter().map(|d| &d.metadata))
                        .find(|metadata| metadata.id() == id)
                })
                .map(name),
        },
    }
}

#[cfg(test)]
mod tests;
