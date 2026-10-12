//! Project files → isolated compilations → public model metadata. File I/O and
//! semantic compilation are bounded and run outside async executors.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use opentelemetry::metrics::{Counter, Histogram, Meter};
use sef_model_ir::model::ObjectMetadata;
use sef_model_ir::types::ValueType;
use sef_ossie::catalog::Catalog;
use sef_ossie::diagnostic::{Code, Diagnostic, Severity};
use serde::Serialize;
use tokio::sync::Semaphore;
use tower_http::request_id::RequestId;
use utoipa::ToSchema;

use crate::{AppState, problem::Problem};

pub const MAX_FILES: usize = 100;
pub const MAX_PROJECT_BYTES: usize = 32 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 4096;

#[derive(Clone)]
pub struct Models {
    directory: Option<PathBuf>,
    admission: Arc<Semaphore>,
    duration: Option<Histogram<f64>>,
    requests: Option<Counter<u64>>,
}

impl Default for Models {
    fn default() -> Self {
        Self::new(None)
    }
}

impl Models {
    #[must_use]
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            directory,
            admission: Arc::new(Semaphore::new(2)),
            duration: None,
            requests: None,
        }
    }

    #[must_use]
    pub fn with_meter(mut self, meter: &Meter) -> Self {
        self.duration = Some(
            meter
                .f64_histogram("bi_models_load")
                .with_unit("s")
                .with_description("Project reading and compilation duration")
                .build(),
        );
        self.requests = Some(
            meter
                .u64_counter("bi_models_requests")
                .with_description("Model listing requests")
                .build(),
        );
        self
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    NotConfigured,
    Ready,
    Unavailable,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelsResponse {
    pub project_status: ProjectStatus,
    pub models: Vec<ModelFile>,
    pub diagnostics: Vec<ModelDiagnostic>,
}

/// An invalid file never exposes metadata from a previous successful load.
#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ModelFile {
    Valid {
        file: String,
        name: String,
        fingerprint: String,
        description: Option<String>,
        ai_context: Option<Box<ModelAiContext>>,
        datasets: Vec<ModelDataset>,
        metrics: Vec<ModelMember>,
        dimensions: Vec<ModelMember>,
        diagnostics: Vec<ModelDiagnostic>,
    },
    Invalid {
        file: String,
        name: String,
        diagnostics: Vec<ModelDiagnostic>,
    },
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelMember {
    pub id: String,
    pub name: String,
    /// Exact logical type, e.g. INT64, DECIMAL(18,2), TIMESTAMPTZ (instant).
    pub data_type: String,
    /// Direct dimension binding nullability. Null means not inferred for
    /// computed dimensions or metrics, whose result has not been executed.
    pub nullable: Option<bool>,
    /// Metadata per Ossie origin; conformed dimensions keep every binding.
    pub annotations: Vec<ModelAnnotation>,
}

/// Context is descriptive, never executable or a semantic name alias.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
pub enum ModelAiContext {
    Text(String),
    Structured(ModelAiContextObject),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelAiContextObject {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synonyms: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    /// Unknown keys are preserved without interpretation.
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

impl From<&sef_ossie::document::AiContext> for ModelAiContext {
    fn from(context: &sef_ossie::document::AiContext) -> Self {
        match context {
            sef_ossie::document::AiContext::Text(text) => Self::Text(text.clone()),
            sef_ossie::document::AiContext::Structured(context) => {
                Self::Structured(ModelAiContextObject {
                    instructions: context.instructions.clone(),
                    synonyms: context.synonyms.clone(),
                    examples: context.examples.clone(),
                    other: context.other.clone(),
                })
            }
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelAnnotation {
    /// Ossie declaration name, useful when a dimension has several origins.
    pub name: String,
    /// JSON pointer in this Ossie file, identifying the declaration.
    pub path: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub ai_context: Option<ModelAiContext>,
    /// Effective time role for fields/dimensions; null for a metric.
    pub is_time: Option<bool>,
    /// Explicit declaration; null means the datatype supplies the default.
    pub declared_is_time: Option<bool>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelDataset {
    pub name: String,
    pub description: Option<String>,
    pub ai_context: Option<ModelAiContext>,
    /// All declared fields, including computed fields that CSMIR inlines.
    pub fields: Vec<ModelField>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelField {
    pub name: String,
    pub is_dimension: bool,
    pub annotation: ModelAnnotation,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ModelDiagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    /// JSON pointer; empty string denotes the document root.
    pub path: String,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub message: String,
}

impl ModelDiagnostic {
    fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: DiagnosticSeverity::Error,
            path: String::new(),
            line: None,
            column: None,
            message: message.into(),
        }
    }
}

impl From<Diagnostic> for ModelDiagnostic {
    fn from(d: Diagnostic) -> Self {
        let message = match d.code {
            Code::Syntax => "El archivo no es un documento YAML o JSON válido: revisa la sintaxis, las propiedades obligatorias y las claves repetidas en el lugar indicado.".to_owned(),
            Code::SemanticContract => semantic_message(&d.message),
            _ => d.message,
        };
        Self {
            code: d.code.as_str().into(),
            severity: if d.severity == Severity::Error {
                DiagnosticSeverity::Error
            } else {
                DiagnosticSeverity::Warning
            },
            path: d.path,
            line: d.location.map(|l| l.0),
            column: d.location.map(|l| l.1),
            message,
        }
    }
}

fn semantic_message(message: &str) -> String {
    let code = message
        .rsplit('[')
        .next()
        .unwrap_or("semantic_contract")
        .trim_end_matches(']');
    let explanation = match code {
        "INCOMPATIBLE_TYPES" => {
            "Los tipos de los campos o de la expresión no son compatibles. Declara tipos compatibles sin perder precisión."
        }
        "INCOMPATIBLE_UNITS" | "MISSING_UNIT_CONVERSION" => {
            "Las unidades de la métrica no son compatibles o falta una conversión explícita. Revisa sus unidades."
        }
        "INVALID_GRAIN" => {
            "El nivel de detalle del modelo no coincide con sus claves o sus métricas. Revisa el grano declarado."
        }
        "INVALID_RELATIONSHIP_CONTRACT" | "INVALID_MODEL_REFERENCE" => {
            "Una relación o referencia no coincide con los campos y claves del modelo. Revisa los elementos relacionados."
        }
        "INVALID_TEMPORAL_CONTRACT" | "AMBIGUOUS_TEMPORAL_SELECTION" => {
            "La definición temporal es incompleta o ambigua. Revisa la zona horaria y los campos de fecha."
        }
        "INVALID_ALLOCATION_CONTRACT" => {
            "La regla de distribución de una métrica entre filas es incompleta. Revisa sus pesos y claves."
        }
        "METRIC_DEPENDENCY_CYCLE" => {
            "Hay métricas que dependen de sí mismas. Elimina la dependencia circular."
        }
        "INVALID_METRIC_RECOMPOSITION" => {
            "La métrica no se puede recomponer con las medidas y el grano declarados. Revisa su expresión."
        }
        "INVALID_WINDOW_SEMANTICS" => {
            "La ventana de la métrica no tiene una definición válida. Revisa su orden y su partición."
        }
        "POLICY_VIOLATION" => {
            "La política declarada no permite esta definición. Revisa las reglas del modelo."
        }
        _ => {
            "El modelo incumple un contrato semántico. Revisa la definición del elemento indicado."
        }
    };
    format!("{explanation} ({code})")
}

/// Authorization: action `models.read`, resource the configured project's
/// `metrics` metadata. The synthetic G2 deployment requires a valid session;
/// resource policies follow in the G2 security work (ADR 0010 and #64 scope).
#[utoipa::path(get, path = "/api/v1/models", tag = "models", operation_id = "listModels",
    summary = "Recarga metrics/*.yaml, *.yml y *.json del proyecto y compila cada archivo con el catálogo analítico real.",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Estado del proyecto y resultados por archivo; sin configuración, lista vacía. Directorio inaccesible o límites globales: project_status unavailable y diagnóstico; nunca resultados parciales.", body = ModelsResponse),
        (status = 401, description = "Sin sesión válida", body = Problem, content_type = "application/problem+json"),
        (status = 429, description = "Demasiadas cargas simultáneas", body = Problem, content_type = "application/problem+json"),
        (status = 503, description = "Motor no disponible o lectura fallida", body = Problem, content_type = "application/problem+json"),
        (status = 504, description = "Tiempo de compilación agotado", body = Problem, content_type = "application/problem+json")
    )
)]
pub async fn list(State(state): State<AppState>, id: Option<Extension<RequestId>>) -> Response {
    let started = Instant::now();
    let request_id = id
        .and_then(|Extension(id)| id.header_value().to_str().ok().map(str::to_owned))
        .unwrap_or_default();
    let result = listing(&state).await;
    let outcome = if result.is_ok() { "ok" } else { "error" };
    let attributes = [opentelemetry::KeyValue::new("outcome", outcome)];
    if let Some(duration) = &state.models.duration {
        duration.record(started.elapsed().as_secs_f64(), &attributes);
    }
    if let Some(requests) = &state.models.requests {
        requests.add(1, &attributes);
    }
    tracing::info!(
        request_id,
        outcome,
        elapsed_ms = started.elapsed().as_millis(),
        "project models loaded"
    );
    match result {
        Ok(body) => ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response(),
        Err((status, code, message)) => {
            Problem::new(status, code, Some(message.to_owned()), &request_id).into_response()
        }
    }
}

type Failure = (StatusCode, &'static str, &'static str);

async fn listing(state: &AppState) -> Result<ModelsResponse, Failure> {
    let Some(directory) = state.models.directory.clone() else {
        return Ok(ModelsResponse {
            project_status: ProjectStatus::NotConfigured,
            models: vec![],
            diagnostics: vec![],
        });
    };
    let permit = state
        .models
        .admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            (
                StatusCode::TOO_MANY_REQUESTS,
                "models_busy",
                "Ya hay dos cargas en curso. Reintenta en unos segundos.",
            )
        })?;
    // The permit travels with native work, including when the HTTP future drops.
    let span = tracing::Span::current();
    let loaded = tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        (read_project(&directory), permit)
    })
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "models_unavailable",
            "No se pudo leer el proyecto.",
        )
    })?;
    let (files, permit) = loaded;
    let files = match files {
        Ok(files) => files,
        Err(diagnostic) => {
            return Ok(ModelsResponse {
                project_status: ProjectStatus::Unavailable,
                models: vec![],
                diagnostics: vec![diagnostic],
            });
        }
    };
    if files.is_empty() {
        return Ok(ModelsResponse {
            project_status: ProjectStatus::Ready,
            models: vec![],
            diagnostics: vec![],
        });
    }
    let runtime = state.analytics.runtime().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "analytics_unavailable",
        "El motor analítico no está listo. Reintenta en unos segundos.",
    ))?;
    let span = tracing::Span::current();
    let compiled = tokio::time::timeout(
        state.query_timeout,
        runtime.run(move |conn| {
            let _permit = permit;
            let _entered = span.enter();
            Ok(files
                .into_iter()
                .map(|file| compile_file(file, conn))
                .collect())
        }),
    )
    .await
    .map_err(|_| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            "models_timeout",
            "La compilación superó el tiempo máximo. Reduce el proyecto y reintenta.",
        )
    })?
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "models_unavailable",
            "No se pudo consultar el catálogo analítico. Reintenta.",
        )
    })?;
    Ok(ModelsResponse {
        project_status: ProjectStatus::Ready,
        models: compiled,
        diagnostics: vec![],
    })
}

struct ProjectFile {
    name: String,
    text: Result<String, ModelDiagnostic>,
}

// Walk one component at a time with nofollow, pinning each directory handle.
// No check-then-open race, including on metrics/ and the configured root.
fn project_metrics(path: &Path) -> std::io::Result<Dir> {
    let absolute = std::path::absolute(path)?;
    let mut root = PathBuf::new();
    let mut components = absolute.components().peekable();
    while matches!(
        components.peek(),
        Some(Component::Prefix(_) | Component::RootDir)
    ) {
        root.push(components.next().expect("peeked component").as_os_str());
    }
    let mut dir = Dir::open_ambient_dir(root, cap_std::ambient_authority())?;
    for component in components {
        match component {
            Component::Normal(name) => dir = dir.open_dir_nofollow(name)?,
            Component::CurDir => {}
            _ => return Err(std::io::Error::other("invalid project path")),
        }
    }
    dir.open_dir_nofollow("metrics")
}

fn read_project(path: &Path) -> Result<Vec<ProjectFile>, ModelDiagnostic> {
    let dir = project_metrics(path).map_err(|_| ModelDiagnostic::error("project_unavailable", "No se puede abrir la carpeta del proyecto o su carpeta metrics. Comprueba que existen, tienen permisos de lectura y no son enlaces simbólicos."))?;
    let mut names = vec![];
    for (i, entry) in dir
        .entries()
        .map_err(|_| {
            ModelDiagnostic::error(
                "project_unavailable",
                "No se puede listar la carpeta metrics.",
            )
        })?
        .enumerate()
    {
        if i >= MAX_DIRECTORY_ENTRIES {
            return Err(ModelDiagnostic::error(
                "too_many_entries",
                "La carpeta metrics supera 4096 entradas. Reduce su contenido.",
            ));
        }
        let entry = entry.map_err(|_| {
            ModelDiagnostic::error(
                "project_unavailable",
                "No se puede listar la carpeta metrics.",
            )
        })?;
        let name = entry.file_name();
        if matches!(
            Path::new(&name).extension().and_then(|s| s.to_str()),
            Some("yaml" | "yml" | "json")
        ) {
            names.push(name);
            if names.len() > MAX_FILES {
                return Err(ModelDiagnostic::error(
                    "too_many_files",
                    format!(
                        "El proyecto supera {MAX_FILES} archivos de modelos. Reduce la cantidad antes de cargarlo."
                    ),
                ));
            }
        }
    }
    names.sort();
    let mut total = 0;
    names
        .into_iter()
        .map(|name| {
            let label = name
                .to_str()
                .ok_or_else(|| {
                    ModelDiagnostic::error(
                        "invalid_filename",
                        "Hay un nombre de archivo que no es texto UTF-8. Renómbralo.",
                    )
                })?
                .to_owned();
            let text = read_file(&dir, &name);
            total += text.as_ref().map_or(0, String::len);
            if total > MAX_PROJECT_BYTES {
                return Err(ModelDiagnostic::error(
                    "project_too_large",
                    "Los modelos superan 32 MiB en conjunto. Reduce el tamaño del proyecto.",
                ));
            }
            Ok(ProjectFile { name: label, text })
        })
        .collect()
}

fn read_file(dir: &Dir, name: &std::ffi::OsStr) -> Result<String, ModelDiagnostic> {
    // Entry metadata is only a fast rejection; the actual open also forbids links.
    if !dir
        .symlink_metadata(name)
        .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
    {
        return Err(ModelDiagnostic::error(
            "unsafe_file",
            "Este archivo no es un archivo regular o es un enlace simbólico. Usa un archivo dentro de metrics.",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    // A raced replacement with a FIFO must not stall a blocking worker.
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = dir.open_with(name, &options).map_err(|_| ModelDiagnostic::error("unsafe_file", "No se pudo abrir el archivo de forma segura. Comprueba sus permisos y que no sea un enlace simbólico."))?;
    let metadata = file.metadata().map_err(|_| {
        ModelDiagnostic::error("file_unreadable", "No se pudo comprobar el archivo.")
    })?;
    if !metadata.is_file() {
        return Err(ModelDiagnostic::error(
            "unsafe_file",
            "Solo se admiten archivos regulares.",
        ));
    }
    if metadata.len() > sef_ossie::load::MAX_DOCUMENT_BYTES as u64 {
        return Err(ModelDiagnostic::error(
            "too_large",
            "El archivo supera 8 MiB. Reduce su tamaño.",
        ));
    }
    let mut bytes = Vec::new();
    file.take((sef_ossie::load::MAX_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            ModelDiagnostic::error(
                "file_unreadable",
                "No se pudo leer el archivo. Comprueba sus permisos.",
            )
        })?;
    if bytes.len() > sef_ossie::load::MAX_DOCUMENT_BYTES {
        return Err(ModelDiagnostic::error(
            "too_large",
            "El archivo supera 8 MiB. Reduce su tamaño.",
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| ModelDiagnostic::error("syntax", "El archivo debe usar texto UTF-8."))
}

fn compile_file(file: ProjectFile, conn: &bi_runtime::duckdb::Connection) -> ModelFile {
    let path = format!("metrics/{}", file.name);
    let invalid = |name: String, diagnostics| ModelFile::Invalid {
        file: path.clone(),
        name,
        diagnostics,
    };
    let text = match file.text {
        Ok(text) => text,
        Err(d) => return invalid(file.name, vec![d]),
    };
    let loaded = match sef_ossie::load::load(&text) {
        Ok(loaded) => loaded,
        Err(ds) => return invalid(file.name, ds.into_iter().map(Into::into).collect()),
    };
    let name = loaded.model.name.clone();
    let mut catalog = Catalog::default();
    let mut diagnostics = vec![];
    for (i, dataset) in loaded.model.datasets.iter().enumerate() {
        match sef_adapter_duckdb::describe(conn, &dataset.source) {
            Ok(relation) => {
                catalog.relations.insert(dataset.source.clone(), relation);
            }
            Err(error) => {
                let code = match error {
                    sef_adapter_duckdb::CatalogError::UnknownSource => "unknown_source",
                    sef_adapter_duckdb::CatalogError::AmbiguousSource => "ambiguous_source",
                    sef_adapter_duckdb::CatalogError::UnsupportedType { .. } => {
                        "unsupported_source_type"
                    }
                    sef_adapter_duckdb::CatalogError::Engine(_)
                    | sef_adapter_duckdb::CatalogError::InvalidHandle => "catalog_unavailable",
                };
                let mut d = ModelDiagnostic::error(code, error.to_string());
                d.path = format!("/datasets/{i}/source");
                diagnostics.push(d);
            }
        }
    }
    if !diagnostics.is_empty() {
        return invalid(name, diagnostics);
    }
    // Exact UTF-8 filename, including extension/case, encoded as a logical ID.
    let model_id = file
        .name
        .as_bytes()
        .iter()
        .fold(String::from("file-"), |mut id, byte| {
            use std::fmt::Write as _;
            write!(id, "{byte:02x}").expect("writing to String");
            id
        });
    match sef_ossie::csmir::compile_document(
        &loaded.model,
        &catalog,
        &sef_ossie::csmir::Options {
            namespace: format!("project-{model_id}"),
            model: model_id,
        },
    ) {
        Err(ds) => invalid(name, ds.into_iter().map(Into::into).collect()),
        Ok(compiled) => {
            let definition = compiled.semantic.definition();
            ModelFile::Valid {
                file: path,
                name,
                fingerprint: compiled.semantic.content_hash().to_string(),
                description: loaded.model.description.clone(),
                ai_context: loaded
                    .model
                    .ai_context
                    .as_ref()
                    .map(|context| Box::new(context.into())),
                datasets: datasets(&loaded.model),
                metrics: definition
                    .metrics
                    .iter()
                    .map(|m| member(&m.metadata, &m.value_type, None, &compiled))
                    .collect(),
                dimensions: definition
                    .dimensions
                    .iter()
                    .map(|d| {
                        member(
                            &d.metadata,
                            &d.value_type,
                            dimension_nullable(d, definition),
                            &compiled,
                        )
                    })
                    .collect(),
                diagnostics: loaded
                    .warnings
                    .into_iter()
                    .chain(compiled.warnings)
                    .map(Into::into)
                    .collect(),
            }
        }
    }
}

fn datasets(model: &sef_ossie::SemanticModel) -> Vec<ModelDataset> {
    model
        .datasets
        .iter()
        .enumerate()
        .map(|(i, dataset)| ModelDataset {
            name: dataset.name.clone(),
            description: dataset.description.clone(),
            ai_context: dataset.ai_context.as_ref().map(Into::into),
            fields: dataset
                .fields
                .iter()
                .enumerate()
                .map(|(j, field)| ModelField {
                    name: field.name.clone(),
                    is_dimension: field.dimension.is_some(),
                    annotation: field_annotation(
                        field,
                        format!("{}.{}", dataset.name, field.name),
                        format!("/datasets/{i}/fields/{j}"),
                    ),
                })
                .collect(),
        })
        .collect()
}

fn dimension_nullable(
    dimension: &sef_model_ir::model::Dimension,
    definition: &sef_model_ir::model::ModelDefinition,
) -> Option<bool> {
    let nulls = dimension
        .bindings
        .iter()
        .map(|b| {
            if let sef_model_ir::expressions::ScalarExprKind::FieldRef(id) = &b.expression.kind {
                definition
                    .datasets
                    .iter()
                    .flat_map(|d| &d.fields)
                    .find(|f| f.metadata.id() == id)
                    .map(|f| f.nullable)
            } else {
                None
            }
        })
        .collect::<Option<Vec<_>>>()?;
    Some(nulls.into_iter().any(|v| v))
}

fn member(
    metadata: &ObjectMetadata,
    data_type: &ValueType,
    nullable: Option<bool>,
    compiled: &sef_ossie::csmir::CompiledDocument,
) -> ModelMember {
    ModelMember {
        id: metadata.id().as_str().into(),
        name: metadata.label.clone().unwrap_or_else(|| {
            metadata
                .id()
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .into()
        }),
        data_type: type_label(data_type),
        nullable,
        annotations: compiled
            .origins(metadata.id())
            .into_iter()
            .filter_map(|path| annotation(compiled.document(), path))
            .collect(),
    }
}

fn field_annotation(
    field: &sef_ossie::document::Field,
    name: String,
    path: String,
) -> ModelAnnotation {
    ModelAnnotation {
        name,
        path,
        label: field.label.clone(),
        description: field.description.clone(),
        ai_context: field.ai_context.as_ref().map(Into::into),
        is_time: Some(field.is_time()),
        declared_is_time: field.dimension.as_ref().and_then(|d| d.is_time),
    }
}

fn annotation(model: &sef_ossie::SemanticModel, path: &str) -> Option<ModelAnnotation> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["", "datasets", i, "fields", j] => {
            let field = model
                .datasets
                .get(i.parse::<usize>().ok()?)?
                .fields
                .get(j.parse::<usize>().ok()?)?;
            let dataset = &model.datasets[i.parse::<usize>().ok()?];
            Some(field_annotation(
                field,
                format!("{}.{}", dataset.name, field.name),
                path.into(),
            ))
        }
        ["", "metrics", i] => {
            let metric = model.metrics.get(i.parse::<usize>().ok()?)?;
            Some(ModelAnnotation {
                name: metric.name.clone(),
                path: path.into(),
                label: None,
                description: metric.description.clone(),
                ai_context: metric.ai_context.as_ref().map(Into::into),
                is_time: None,
                declared_is_time: None,
            })
        }
        _ => None,
    }
}

fn type_label(value: &ValueType) -> String {
    match value {
        ValueType::Boolean => "BOOLEAN".into(),
        ValueType::Integer { signed, bits } => format!(
            "{}INT{}",
            if *signed { "" } else { "U" },
            bits.map_or_else(|| " (ancho no declarado)".into(), |b| b.to_string())
        ),
        ValueType::Decimal(d) => format!("DECIMAL({},{})", d.precision(), d.scale()),
        ValueType::Float { bits } => format!("FLOAT{bits}"),
        ValueType::Utf8 => "VARCHAR".into(),
        ValueType::Date => "DATE".into(),
        ValueType::Time => "TIME".into(),
        ValueType::Timestamp {
            timezone_semantics: sef_model_ir::time::TimestampSemantics::Instant,
        } => "TIMESTAMPTZ (instante)".into(),
        _ => "Tipo sin representación pública definida".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn public_types_never_show_internal_rust_metadata() {
        assert_eq!(type_label(&ValueType::Time), "TIME");
        let opaque = ValueType::OpaqueExtension {
            namespace: "private_namespace".into(),
            type_name: "internal_type".into(),
        };
        assert_eq!(
            type_label(&opaque),
            "Tipo sin representación pública definida"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn nofollow_rejects_links_for_arbitrary_filenames(name in "[a-zA-Z0-9_-]{1,40}") {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret"), "never expose this").unwrap();
            std::fs::create_dir(root.path().join("metrics")).unwrap();
            let filename = format!("{name}.yaml");
            #[cfg(unix)]
            std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("metrics").join(&filename)).unwrap();
            #[cfg(windows)]
            {
                let output = std::process::Command::new("cmd").args(["/c", "mklink", "/J"])
                    .arg(root.path().join("metrics").join(&filename)).arg(outside.path()).output().unwrap();
                prop_assert!(output.status.success());
            }
            let loaded = read_project(root.path()).unwrap();
            prop_assert_eq!(loaded.len(), 1);
            prop_assert_eq!(loaded[0].text.as_ref().unwrap_err().code.as_str(), "unsafe_file");
        }
    }

    #[test]
    fn directory_entries_budget_includes_ignored_extensions() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("metrics")).unwrap();
        for i in 0..=MAX_DIRECTORY_ENTRIES {
            std::fs::write(root.path().join(format!("metrics/{i}.txt")), "").unwrap();
        }
        assert_eq!(
            read_project(root.path()).err().unwrap().code,
            "too_many_entries"
        );
    }
}
