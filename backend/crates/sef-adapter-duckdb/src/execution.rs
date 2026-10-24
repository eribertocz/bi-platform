//! Synchronous SEIR lowering. Unsupported semantics fail before SQL.
//! Call execution on an analytical worker, never on an async executor.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;

use duckdb::Connection;
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::types::Value;
use sef_correctness::{SemanticContext, ValidatedQuery};
use sef_exec_ir::{Contract, NodeId, Operator, SemanticPlan};
use sef_model_ir::expressions::{
    AggregateFunction, BooleanOp, ComparisonOp, MetricExpr, MetricExprKind, ScalarExpr,
    ScalarExprKind, UnaryOp,
};
use sef_model_ir::identity::{SourceHandle, StableId};
use sef_model_ir::model::{CompiledSemanticModel, ModelDefinition};
use sef_model_ir::relationships::Optionality;
use sef_model_ir::time::SortDirection;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{ArithmeticOp, Literal, ValueType};
use sef_planner::Certificate;
use sef_query_ir::query::{FilterExpr, FilterTarget, NullOrdering, RequestedAtom, SelectionItem};

#[path = "exact.rs"]
mod exact;
#[path = "wide.rs"]
mod wide;

/// Administrator-owned catalog: opaque source handles resolve to identifiers,
/// never caller SQL, paths or URLs.
pub type SourceCatalog = BTreeMap<(SourceHandle, SourceHandle), PhysicalRelation>;

#[derive(Clone, Debug)]
pub struct PhysicalRelation {
    pub name: Vec<String>,
    pub columns: BTreeMap<SourceHandle, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionError {
    UnknownNode,
    InvalidCertificate,
    UnknownBinding,
    InvalidIdentifier,
    Unsupported(&'static str),
    MissingContext(String),
    InvalidContext(String),
    SourceContract,
    ContractViolation,
    NumericOverflow,
    /// A result outside the exact range certified for it (#261): a sum
    /// past its declared precision, never another failure of the engine.
    ResultOutOfRange,
    Budget,
    Cancelled,
    Deadline,
    Engine,
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No backend messages or parameter values: both can contain secrets.
        write!(f, "SEIR execution rejected: {self:?}")
    }
}
impl std::error::Error for ExecutionError {}

enum Parameter {
    Context(String, ValueType),
    Literal(Literal),
}

struct SourceCheck {
    name: String,
    source: String,
    relation: String,
    columns: Vec<(String, ValueType, bool)>,
}

impl SourceCheck {
    fn snapshot(
        &self,
        connection: &Connection,
        actual: &sef_ossie::catalog::Relation,
    ) -> Result<crate::provenance::SourceSnapshot, ExecutionError> {
        use sef_model_ir::canonical::{Canonical, CanonicalValue as C};
        use sha2::{Digest, Sha256};
        let schema = C::record(
            "duckdb-referenced-schema-v1",
            [
                ("source", C::text(&self.source)),
                ("relation", C::text(&self.relation)),
                (
                    "fields",
                    C::Sequence(
                        self.columns
                            .iter()
                            .map(|(name, _, _)| {
                                let column = actual
                                    .columns
                                    .iter()
                                    .find(|c| &c.name == name)
                                    .ok_or(ExecutionError::SourceContract)?;
                                Ok(C::record(
                                    "column",
                                    [
                                        ("name", C::text(name)),
                                        ("type", column.value_type.canonical_value()),
                                        ("nullable", C::Bool(column.nullable)),
                                    ],
                                ))
                            })
                            .collect::<Result<Vec<_>, ExecutionError>>()?,
                    ),
                ),
            ],
        )
        .encode()
        .map_err(|_| ExecutionError::SourceContract)?;
        let parts = self.name.split('.').collect::<Vec<_>>();
        let database = if parts.len() == 3 {
            parts[0].to_owned()
        } else {
            connection
                .query_row("SELECT current_database()", [], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|_| ExecutionError::Engine)?
        };
        let database_type: String = connection
            .query_row(
                "SELECT type FROM duckdb_databases() WHERE lower(database_name) = lower(?)",
                [&database],
                |row| row.get(0),
            )
            .map_err(|_| ExecutionError::Engine)?;
        if !matches!(database_type.as_str(), "duckdb" | "ducklake") {
            return Err(ExecutionError::Unsupported("source snapshot topology"));
        }
        let schema_name = if parts.len() >= 2 {
            parts[parts.len() - 2].to_owned()
        } else {
            connection
                .query_row("SELECT current_schema()", [], |r| r.get(0))
                .map_err(|_| ExecutionError::Engine)?
        };
        let table = parts.last().ok_or(ExecutionError::UnknownBinding)?;
        let table_type: String = connection.query_row("SELECT table_type FROM information_schema.tables WHERE lower(table_catalog) = lower(?) AND lower(table_schema) = lower(?) AND lower(table_name) = lower(?)", [database.as_str(), schema_name.as_str(), *table], |row| row.get(0)).map_err(|_| ExecutionError::SourceContract)?;
        if table_type != "BASE TABLE" {
            return Err(ExecutionError::Unsupported("view snapshot evidence"));
        }
        let is_lake = database_type == "ducklake";
        let data_snapshot = if is_lake {
            Some(
                connection
                    .query_row(
                        &format!("SELECT id FROM {}.current_snapshot()", physical(&database)?),
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|_| ExecutionError::Engine)?,
            )
        } else {
            None
        };
        Ok(crate::provenance::SourceSnapshot {
            source: self.source.clone(),
            relation: self.relation.clone(),
            schema_fingerprint: format!("duckdb-schema-sha256-v1:{}", hex(&Sha256::digest(schema))),
            data_snapshot,
        })
    }
}

/// Opaque outside this adapter. Only `lower` can create an executable plan.
/// It contains slot names and types, never context values.
pub struct DuckDbPlan {
    sql: Vec<(String, String)>,
    parameters: Vec<Parameter>,
    sources: Vec<SourceCheck>,
    certificate: Certificate,
    checks: Vec<String>,
    wide: Option<wide::Aggregation>,
    /// Whether the plan aggregates with SUM (#261).
    sums: bool,
    definitions: Vec<sef_exec_ir::ObjectRef>,
    output_grains: BTreeMap<String, Vec<RequestedAtom>>,
    metadata: BTreeMap<String, Vec<std::collections::HashMap<String, String>>>,
    binding_key: [u8; 32],
}

/// Per-request bounds in addition to the runtime's native memory limit.
/// Batches emitted by `stream` remain provisional until it returns success.
#[derive(Clone)]
pub struct ExecutionLimits {
    pub max_rows: usize,
    pub max_bytes: usize,
    pub max_intermediate_bytes: usize,
    pub deadline: Option<std::time::Instant>,
    pub cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_rows: usize::MAX,
            max_bytes: usize::MAX,
            max_intermediate_bytes: usize::MAX,
            deadline: None,
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}

impl ExecutionLimits {
    /// # Errors
    /// Cancellation or an expired monotonic deadline.
    pub fn checkpoint(&self) -> Result<(), ExecutionError> {
        if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ExecutionError::Cancelled);
        }
        if self
            .deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            return Err(ExecutionError::Deadline);
        }
        Ok(())
    }
}

impl DuckDbPlan {
    pub fn sql(&self) -> impl Iterator<Item = (&str, &str)> {
        self.sql.iter().map(|(id, sql)| (id.as_str(), sql.as_str()))
    }

    #[must_use]
    pub const fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    /// Binds every slot before touching the engine, verifies physical field
    /// types, and returns native Arrow batches (no floating-point conversion).
    /// The caller supplies a connection/snapshot exclusively for this request.
    ///
    /// # Errors
    /// Missing/mistyped context, changed source schema, or engine failure.
    pub fn execute(
        &self,
        connection: &Connection,
        context: &SemanticContext,
    ) -> Result<Vec<BlockResult>, ExecutionError> {
        let mut results: Vec<BlockResult> = Vec::new();
        self.stream(connection, context, &ExecutionLimits::default(), |result| {
            if let Some(block) = results.iter_mut().find(|r| r.block_id == result.block_id) {
                block.batches.extend(result.batches);
            } else {
                results.push(result);
            }
            Ok(())
        })?;
        Ok(results)
    }

    /// Runs in one read transaction. A bounded consumer may block only this
    /// analytical worker. Dropping the consumer must cancel the request.
    /// Successful completion certifies all provisional batches together;
    /// any error invalidates the entire result, including earlier batches.
    ///
    /// # Errors
    /// Binding, contract, engine, cancellation, deadline or budget failure.
    #[allow(clippy::too_many_lines)]
    pub fn stream(
        &self,
        connection: &Connection,
        context: &SemanticContext,
        limits: &ExecutionLimits,
        mut emit: impl FnMut(BlockResult) -> Result<(), ExecutionError>,
    ) -> Result<crate::provenance::ResultProvenance, ExecutionError> {
        limits.checkpoint()?;
        let started_at = std::time::SystemTime::now();
        let context_binding_fingerprint = self.context_fingerprint(context)?;
        let values = self
            .parameters
            .iter()
            .map(|parameter| match parameter {
                Parameter::Literal(value) => bind_literal(value),
                Parameter::Context(name, ty) => {
                    let value = context
                        .slots
                        .get(name)
                        .ok_or_else(|| ExecutionError::MissingContext(name.clone()))?;
                    if &value.value_type() != ty || matches!(value, Literal::Null(_)) {
                        return Err(ExecutionError::InvalidContext(name.clone()));
                    }
                    match value {
                        Literal::Utf8(value) => Ok(Value::Text(value.clone())),
                        Literal::Boolean(value) => Ok(Value::Boolean(*value)),
                        _ => Err(ExecutionError::Unsupported("context type")),
                    }
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let transaction = connection
            .unchecked_transaction()
            .map_err(|_| ExecutionError::Engine)?;
        // Catalog entries are shared by cloned connections. Each execution
        // owns distinct temporary names, including concurrent ratio queries.
        let mut function_nonce = [0_u8; 16];
        getrandom::fill(&mut function_nonce).map_err(|_| ExecutionError::Engine)?;
        let suffix = hex(&function_nonce);
        let ratio = format!("sef_exact_ratio_{suffix}");
        let local_date = format!("sef_local_date_{suffix}");
        let combine = format!("sef_exact_combine_{suffix}");
        let sort_key = format!("sef_exact_sort_key_{suffix}");
        let uses = |name: &str| {
            self.sql.iter().any(|(_, sql)| sql.contains(name))
                || self.checks.iter().any(|sql| sql.contains(name))
        };
        if uses("sef_exact_combine(") {
            connection
                .register_scalar_function::<exact::Combine>(&combine)
                .map_err(|_| ExecutionError::Engine)?;
        }
        if uses("sef_exact_sort_key(") {
            connection
                .register_scalar_function::<exact::SortKey>(&sort_key)
                .map_err(|_| ExecutionError::Engine)?;
        }
        if self
            .sql
            .iter()
            .any(|(_, sql)| sql.contains("sef_exact_ratio("))
            || self
                .checks
                .iter()
                .any(|sql| sql.contains("sef_exact_ratio("))
        {
            connection
                .register_scalar_function::<exact::Ratio>(&ratio)
                .map_err(|_| ExecutionError::Engine)?;
        }
        if self
            .sql
            .iter()
            .any(|(_, sql)| sql.contains("sef_local_date("))
            || self
                .checks
                .iter()
                .any(|sql| sql.contains("sef_local_date("))
        {
            connection
                .register_scalar_function::<exact::LocalDate>(&local_date)
                .map_err(|_| ExecutionError::Engine)?;
        }
        let physical_sql = |sql: &str| {
            sql.replace("sef_exact_ratio(", &format!("{ratio}("))
                .replace("sef_local_date(", &format!("{local_date}("))
                .replace("sef_exact_combine(", &format!("{combine}("))
                .replace("sef_exact_sort_key(", &format!("{sort_key}("))
        };
        let engine_version = connection
            .query_row("SELECT version()", [], |row| row.get(0))
            .map_err(|_| ExecutionError::Engine)?;
        let mut snapshots = Vec::new();
        for source in &self.sources {
            let actual = crate::describe(connection, &source.name)
                .map_err(|_| ExecutionError::SourceContract)?;
            for (name, ty, nullable) in &source.columns {
                if !actual.columns.iter().any(|column| {
                    &column.name == name
                        && &column.value_type == ty
                        && (*nullable || !column.nullable)
                }) {
                    return Err(ExecutionError::SourceContract);
                }
            }
            snapshots.push(source.snapshot(connection, &actual)?);
        }
        for check in &self.checks {
            limits.checkpoint()?;
            let mut statement = transaction
                .prepare(&physical_sql(check))
                .map_err(|_| ExecutionError::Engine)?;
            let bound = statement_values(&statement, &values)?;
            let violated: bool = statement
                .query_row(duckdb::params_from_iter(bound), |row| row.get(0))
                .map_err(|_| ExecutionError::Engine)?;
            if violated {
                return Err(ExecutionError::ContractViolation);
            }
        }
        let mut rows = 0_usize;
        let mut bytes = 0_usize;
        for (block_id, sql) in &self.sql {
            limits.checkpoint()?;
            let mut statement = connection
                .prepare(&physical_sql(sql))
                .map_err(|_| ExecutionError::Engine)?;
            let bound = statement_values(&statement, &values)?;
            let _ = statement
                .stream_arrow(duckdb::params_from_iter(bound))
                .map_err(|error| self.engine_failure(&error))?;
            let mut intermediate = Vec::new();
            let mut intermediate_bytes = 0_usize;
            let mut emitted = false;
            loop {
                // Arrow's convenience iterator panics with backend details on
                // fetch failure. The public fallible step keeps errors private.
                let chunk = statement
                    .step()
                    .map_err(|error| self.engine_failure(&error))?;
                let Some(chunk) = chunk else {
                    break;
                };
                let batch = RecordBatch::from(&chunk);
                limits.checkpoint()?;
                if self.wide.is_some() {
                    intermediate_bytes = intermediate_bytes
                        .checked_add(batch.get_array_memory_size())
                        .ok_or(ExecutionError::Budget)?;
                    if intermediate_bytes > limits.max_intermediate_bytes {
                        return Err(ExecutionError::Budget);
                    }
                    intermediate.push(batch);
                } else {
                    let batch = self.conform(block_id, &batch)?;
                    charge(&batch, &mut rows, &mut bytes, limits)?;
                    let batch = self.annotate(block_id, &batch)?;
                    emit(BlockResult {
                        block_id: block_id.clone(),
                        batches: vec![batch],
                    })?;
                    emitted = true;
                }
            }
            if let Some(wide) = &self.wide {
                for batch in wide.execute(&intermediate, limits)? {
                    let batch = self.conform(block_id, &batch)?;
                    charge(&batch, &mut rows, &mut bytes, limits)?;
                    let batch = self.annotate(block_id, &batch)?;
                    emit(BlockResult {
                        block_id: block_id.clone(),
                        batches: vec![batch],
                    })?;
                    emitted = true;
                }
            }
            if !emitted {
                emit(BlockResult {
                    block_id: block_id.clone(),
                    batches: Vec::new(),
                })?;
            }
        }
        limits.checkpoint()?;
        // This request only reads. Rollback also removes its temporary UDFs,
        // so later requests do not inherit mutable connection configuration.
        transaction.rollback().map_err(|_| ExecutionError::Engine)?;
        Ok(crate::provenance::ResultProvenance {
            schema_version: "0.1",
            certificate: self.certificate.clone(),
            definitions: self.definitions.clone(),
            output_grains: self.output_grains.clone(),
            context_binding_fingerprint,
            sources: snapshots,
            checked_contracts: self.checks.len(),
            engine_version,
            adapter_version: env!("CARGO_PKG_VERSION"),
            capability_profile: crate::capabilities::PROFILE_VERSION,
            tzdb_version: chrono_tz::IANA_TZDB_VERSION,
            started_at,
            finished_at: std::time::SystemTime::now(),
            rows,
            arrow_bytes: bytes,
        })
    }

    fn context_fingerprint(&self, context: &SemanticContext) -> Result<String, ExecutionError> {
        use hmac::{Hmac, KeyInit, Mac};
        use sef_model_ir::canonical::{Canonical, CanonicalValue as C};
        let encoded = C::Map(
            context
                .slots
                .iter()
                .map(|(k, v)| (k.clone(), v.canonical_value()))
                .collect(),
        )
        .encode()
        .map_err(|_| ExecutionError::InvalidCertificate)?;
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(&self.binding_key)
            .map_err(|_| ExecutionError::Engine)?;
        mac.update(b"sef-query-context-v1");
        mac.update(&encoded);
        Ok(format!(
            "sef-context-hmac-sha256-v1:{}",
            hex(&mac.finalize().into_bytes())
        ))
    }

    /// The engine's failure, typed: its structured out-of-range error is a
    /// result out of range only when the plan sums (#261). The driver's
    /// error type is read, never its message.
    fn engine_failure(&self, error: &duckdb::Error) -> ExecutionError {
        match error {
            duckdb::Error::DuckDBFailure(failure, _)
                if self.sums
                    && i64::from(failure.extended_code)
                        == i64::from(duckdb::ffi::duckdb_error_type_DUCKDB_ERROR_OUT_OF_RANGE) =>
            {
                ExecutionError::ResultOutOfRange
            }
            _ => ExecutionError::Engine,
        }
    }

    /// Every decimal output within its certified type, exactly (#261): a
    /// carrier past 38 digits becomes Decimal256; a native decimal whose
    /// value exceeds its certified precision is a result out of range.
    fn conform(&self, block: &str, batch: &RecordBatch) -> Result<RecordBatch, ExecutionError> {
        use duckdb::arrow::array::{Array, Decimal128Array, Decimal256Array, StringArray};
        use duckdb::arrow::datatypes::{DataType, i256};
        let metadata = self
            .metadata
            .get(block)
            .ok_or(ExecutionError::InvalidCertificate)?;
        if metadata.len() != batch.num_columns() {
            return Err(ExecutionError::SourceContract);
        }
        let mut changed = false;
        let mut columns = batch.columns().to_vec();
        let mut fields = batch
            .schema()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
            .collect::<Vec<_>>();
        for (i, metadata) in metadata.iter().enumerate() {
            let Some((precision, scale)) = metadata
                .get("sef.value_type")
                .and_then(|ty| ty.strip_prefix("DECIMAL("))
                .and_then(|ty| ty.strip_suffix(')'))
                .and_then(|ty| ty.split_once(','))
                .and_then(|(p, s)| Some((p.parse::<u8>().ok()?, s.parse::<u8>().ok()?)))
            else {
                continue;
            };
            let limit = num_bigint::BigInt::from(10).pow(u32::from(precision));
            let fits = |value: &num_bigint::BigInt| value.magnitude() < limit.magnitude();
            match columns[i].data_type() {
                DataType::Utf8 if precision > 38 => {
                    let text = columns[i]
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or(ExecutionError::SourceContract)?;
                    let values = (0..text.len())
                        .map(|row| {
                            if text.is_null(row) {
                                return Ok(None);
                            }
                            let value = exact::coefficient_of(text.value(row), scale)?;
                            if !fits(&value) {
                                return Err(ExecutionError::ResultOutOfRange);
                            }
                            value
                                .to_string()
                                .parse::<i256>()
                                .map(Some)
                                .map_err(|_| ExecutionError::ResultOutOfRange)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let array = Decimal256Array::from(values)
                        .with_precision_and_scale(
                            precision,
                            i8::try_from(scale).map_err(|_| ExecutionError::SourceContract)?,
                        )
                        .map_err(|_| ExecutionError::SourceContract)?;
                    fields[i] = fields[i].clone().with_data_type(array.data_type().clone());
                    columns[i] = std::sync::Arc::new(array);
                    changed = true;
                }
                DataType::Decimal128(_, _) => {
                    let array = columns[i]
                        .as_any()
                        .downcast_ref::<Decimal128Array>()
                        .ok_or(ExecutionError::SourceContract)?;
                    for value in array.iter().flatten() {
                        if !fits(&num_bigint::BigInt::from(value)) {
                            return Err(ExecutionError::ResultOutOfRange);
                        }
                    }
                }
                _ => {}
            }
        }
        if !changed {
            return Ok(batch.clone());
        }
        RecordBatch::try_new(
            std::sync::Arc::new(duckdb::arrow::datatypes::Schema::new(fields)),
            columns,
        )
        .map_err(|_| ExecutionError::SourceContract)
    }

    fn annotate(&self, block: &str, batch: &RecordBatch) -> Result<RecordBatch, ExecutionError> {
        let metadata = self
            .metadata
            .get(block)
            .ok_or(ExecutionError::InvalidCertificate)?;
        if metadata.len() != batch.num_columns() {
            return Err(ExecutionError::SourceContract);
        }
        let fields = batch
            .schema()
            .fields()
            .iter()
            .zip(metadata)
            .map(|(field, metadata)| {
                let mut merged = field.metadata().clone();
                merged.extend(metadata.clone());
                field.as_ref().clone().with_metadata(merged)
            })
            .collect::<Vec<_>>();
        RecordBatch::try_new(
            std::sync::Arc::new(duckdb::arrow::datatypes::Schema::new(fields)),
            batch.columns().to_vec(),
        )
        .map_err(|_| ExecutionError::SourceContract)
    }
}

fn charge(
    batch: &RecordBatch,
    rows: &mut usize,
    bytes: &mut usize,
    limits: &ExecutionLimits,
) -> Result<(), ExecutionError> {
    *rows = rows
        .checked_add(batch.num_rows())
        .ok_or(ExecutionError::Budget)?;
    *bytes = bytes
        .checked_add(batch.get_array_memory_size())
        .ok_or(ExecutionError::Budget)?;
    if *rows > limits.max_rows || *bytes > limits.max_bytes {
        return Err(ExecutionError::Budget);
    }
    Ok(())
}

pub struct BlockResult {
    pub block_id: String,
    pub batches: Vec<RecordBatch>,
}

fn statement_values<'a>(
    statement: &duckdb::Statement<'_>,
    values: &'a [Value],
) -> Result<Vec<&'a Value>, ExecutionError> {
    (1..=statement.parameter_count())
        .map(|i| {
            let name = statement
                .parameter_name(i)
                .map_err(|_| ExecutionError::Engine)?;
            let index: usize = name.parse().map_err(|_| ExecutionError::Engine)?;
            values
                .get(index.checked_sub(1).ok_or(ExecutionError::Engine)?)
                .ok_or(ExecutionError::Engine)
        })
        .collect()
}

/// Certifies again against CSMIR/SQIR and lowers every reachable operator.
/// Includes governed multihop joins, contract checks, exact native aggregates
/// and wide-decimal SUM emulation. Unsupported operators fail explicitly.
///
/// # Errors
/// Unknown nodes, stale/invalid certificates, missing catalog mappings or
/// any operator/type not implemented exactly by this capability profile.
#[allow(clippy::too_many_lines)] // certification, lowering and receipt form one atomic construction
pub fn lower(
    model: &CompiledSemanticModel,
    validated: &ValidatedQuery,
    plan: &SemanticPlan,
    certificate: &Certificate,
    catalog: &SourceCatalog,
) -> Result<DuckDbPlan, ExecutionError> {
    let roots: Vec<_> = plan.blocks.iter().map(|block| block.root).collect();
    if plan
        .graph
        .reachable(&roots)
        .iter()
        .any(|id| plan.graph.operator(id).is_none())
    {
        return Err(ExecutionError::UnknownNode);
    }
    if &sef_planner::certify(model, validated, plan)
        .map_err(|_| ExecutionError::InvalidCertificate)?
        != certificate
    {
        return Err(ExecutionError::InvalidCertificate);
    }
    let mut lowering = Lowering {
        model: model.definition(),
        catalog,
        nodes: BTreeMap::new(),
        datasets: BTreeMap::new(),
        columns: BTreeMap::new(),
        parameters: Vec::new(),
        sources: Vec::new(),
        checks: Vec::new(),
        unique: std::collections::BTreeSet::new(),
        temporal: std::collections::BTreeSet::new(),
        allocations: std::collections::BTreeSet::new(),
        filter_targets: BTreeMap::new(),
        wide: None,
        carriers: BTreeMap::new(),
        sums: false,
    };
    let mut ctes = Vec::new();
    for node in plan.graph.topological(&roots) {
        lowering.filter_targets.clear();
        if matches!(node.operator, Operator::ApplyPostAggregateFilter { .. }) {
            for block in &plan.blocks {
                if !plan.graph.reachable(&[block.root]).contains(&node.id) {
                    continue;
                }
                let query = validated
                    .query
                    .blocks
                    .iter()
                    .find(|q| q.block_id == block.block_id)
                    .ok_or(ExecutionError::InvalidCertificate)?;
                for selection in &query.selections {
                    if let SelectionItem::Metric(id) = &selection.item
                        && lowering
                            .filter_targets
                            .insert(selection.selection_id.clone(), id.clone())
                            .is_some_and(|old| old != *id)
                    {
                        return Err(ExecutionError::Unsupported("ambiguous shared post-filter"));
                    }
                }
            }
        }
        let name = format!("n{}", ctes.len());
        let body = lowering.operator(&node.operator)?;
        let columns = lowering.output_columns(&node.operator)?;
        let dataset = match &node.operator {
            Operator::ReadSemanticDataset { dataset, .. } => Some(dataset.id.clone()),
            _ => node
                .operator
                .inputs()
                .first()
                .and_then(|id| lowering.datasets.get(id))
                .cloned(),
        };
        if let Some(dataset) = dataset {
            lowering.datasets.insert(node.id, dataset);
        }
        lowering.columns.insert(node.id, columns);
        ctes.push(format!("{name} AS ({body})"));
        lowering.nodes.insert(node.id, name);
    }
    let sql = plan
        .blocks
        .iter()
        .map(|block| {
            Ok((
                block.block_id.clone(),
                format!(
                    "WITH {} SELECT * FROM {}",
                    ctes.join(",\n"),
                    lowering.node(&block.root)?
                ),
            ))
        })
        .collect::<Result<_, ExecutionError>>()?;
    if lowering.wide.is_some() && plan.blocks.len() != 1 {
        return Err(ExecutionError::Unsupported("multiple emulated blocks"));
    }
    let mut binding_key = [0_u8; 32];
    getrandom::fill(&mut binding_key).map_err(|_| ExecutionError::Engine)?;
    let definitions = plan
        .graph
        .topological(&roots)
        .iter()
        .filter_map(|n| {
            if let Operator::ProjectSemanticOutput { definitions, .. } = &n.operator {
                Some(definitions.clone())
            } else {
                None
            }
        })
        .flatten()
        .collect();
    let metadata = output_metadata(model, validated)?;
    Ok(DuckDbPlan {
        sql,
        parameters: lowering.parameters,
        sources: lowering.sources,
        certificate: certificate.clone(),
        checks: lowering
            .checks
            .into_iter()
            .map(|check| format!("WITH {} {check}", ctes.join(",\n")))
            .collect(),
        wide: lowering.wide,
        sums: lowering.sums,
        definitions,
        output_grains: validated
            .query
            .blocks
            .iter()
            .map(|b| (b.block_id.clone(), b.requested_grain()))
            .collect(),
        metadata,
        binding_key,
    })
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

type OutputMetadata = BTreeMap<String, Vec<std::collections::HashMap<String, String>>>;

fn output_metadata(
    model: &CompiledSemanticModel,
    validated: &ValidatedQuery,
) -> Result<OutputMetadata, ExecutionError> {
    validated
        .query
        .blocks
        .iter()
        .map(|block| {
            let fields = block
                .selections
                .iter()
                .map(|selection| {
                    let (metadata, ty, unit, kind) = match &selection.item {
                        SelectionItem::Metric(id) => {
                            let definition = model
                                .definition()
                                .metrics
                                .iter()
                                .find(|m| m.metadata.id() == id)
                                .ok_or(ExecutionError::UnknownBinding)?;
                            (
                                &definition.metadata,
                                &definition.value_type,
                                &definition.unit,
                                "sef.metric_id",
                            )
                        }
                        SelectionItem::Dimension(d) => {
                            let definition = model
                                .definition()
                                .dimensions
                                .iter()
                                .find(|m| m.metadata.id() == &d.dimension)
                                .ok_or(ExecutionError::UnknownBinding)?;
                            (
                                &definition.metadata,
                                &definition.value_type,
                                &definition.unit,
                                "sef.dimension_id",
                            )
                        }
                        SelectionItem::TimeDimension(t) => {
                            let definition = model
                                .definition()
                                .dimensions
                                .iter()
                                .find(|m| m.metadata.id() == &t.dimension.dimension)
                                .ok_or(ExecutionError::UnknownBinding)?;
                            (
                                &definition.metadata,
                                &definition.value_type,
                                &definition.unit,
                                "sef.dimension_id",
                            )
                        }
                    };
                    let ty = if let ValueType::Decimal(d) = ty {
                        format!("DECIMAL({},{})", d.precision(), d.scale())
                    } else {
                        sql_type(ty)?
                    };
                    let mut fields = std::collections::HashMap::from([
                        ("sef.selection_id".into(), selection.selection_id.clone()),
                        (kind.into(), metadata.id().to_string()),
                        ("sef.value_type".into(), ty),
                        (
                            "sef.semantic_version".into(),
                            metadata.identity.semantic_version.to_string(),
                        ),
                    ]);
                    if let Some(unit) = unit {
                        fields.insert("sef.unit".into(), format!("{unit:?}"));
                    }
                    Ok(fields)
                })
                .collect::<Result<Vec<_>, ExecutionError>>()?;
            Ok((block.block_id.clone(), fields))
        })
        .collect()
}

fn bind_literal(value: &Literal) -> Result<Value, ExecutionError> {
    Ok(match value {
        Literal::Null(_) => Value::Null,
        Literal::Utf8(v) => Value::Text(v.clone()),
        Literal::Boolean(v) => Value::Boolean(*v),
        Literal::SignedInteger(v) => Value::Text(v.to_string()),
        Literal::UnsignedInteger(v) => Value::Text(v.to_string()),
        Literal::Decimal(v) => {
            let text = v.coefficient();
            let (sign, digits) = text.strip_prefix('-').map_or(("", text), |s| ("-", s));
            let scale = usize::from(v.decimal_type().scale());
            let digits = format!("{:0>width$}", digits, width = scale + 1);
            Value::Text(if scale == 0 {
                format!("{sign}{digits}")
            } else {
                let split = digits.len() - scale;
                format!("{sign}{}.{}", &digits[..split], &digits[split..])
            })
        }
        _ => return Err(ExecutionError::Unsupported("bound literal type")),
    })
}

fn physical(name: &str) -> Result<String, ExecutionError> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit()))
    {
        return Err(ExecutionError::InvalidIdentifier);
    }
    Ok(quote(name))
}

fn sql_type(ty: &ValueType) -> Result<String, ExecutionError> {
    Ok(match ty {
        ValueType::Decimal(decimal) if decimal.precision() <= 38 => {
            format!("DECIMAL({},{})", decimal.precision(), decimal.scale())
        }
        ValueType::Integer {
            signed: true,
            bits: Some(64),
        } => "BIGINT".into(),
        ValueType::Utf8 => "VARCHAR".into(),
        ValueType::Boolean => "BOOLEAN".into(),
        ValueType::Date => "DATE".into(),
        ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant,
        } => "TIMESTAMPTZ".into(),
        _ => return Err(ExecutionError::Unsupported("exact value type")),
    })
}

fn comparison(op: ComparisonOp) -> &'static str {
    match op {
        ComparisonOp::Equal => "=",
        ComparisonOp::NotEqual => "<>",
        ComparisonOp::Less => "<",
        ComparisonOp::LessOrEqual => "<=",
        ComparisonOp::Greater => ">",
        ComparisonOp::GreaterOrEqual => ">=",
    }
}

fn time_grain(grain: sef_model_ir::time::TimeGrain) -> Result<&'static str, ExecutionError> {
    use sef_model_ir::time::TimeGrain;
    match grain {
        TimeGrain::Day => Ok("day"),
        TimeGrain::Week => Ok("week"),
        TimeGrain::Month => Ok("month"),
        TimeGrain::Quarter => Ok("quarter"),
        TimeGrain::Year => Ok("year"),
        _ => Err(ExecutionError::Unsupported("subday date bucket")),
    }
}

struct Lowering<'a> {
    model: &'a ModelDefinition,
    catalog: &'a SourceCatalog,
    nodes: BTreeMap<NodeId, String>,
    datasets: BTreeMap<NodeId, StableId>,
    columns: BTreeMap<NodeId, Vec<String>>,
    parameters: Vec<Parameter>,
    sources: Vec<SourceCheck>,
    checks: Vec<String>,
    unique: std::collections::BTreeSet<StableId>,
    temporal: std::collections::BTreeSet<StableId>,
    allocations: std::collections::BTreeSet<StableId>,
    filter_targets: BTreeMap<String, StableId>,
    wide: Option<wide::Aggregation>,
    /// Output selections carried as exact decimal text because their type
    /// exceeds DuckDB's 38 digits (#261), with their scale.
    carriers: BTreeMap<String, u8>,
    /// Whether the plan aggregates with SUM: only then may the engine's
    /// structured out-of-range error be a result out of range.
    sums: bool,
}

/// The decimal type of `ty` when DuckDB cannot represent it natively.
fn wide_decimal(ty: &ValueType) -> Option<sef_model_ir::types::DecimalType> {
    match ty {
        ValueType::Decimal(d) if d.precision() > 38 => Some(*d),
        _ => None,
    }
}

impl Lowering<'_> {
    fn node(&self, id: &NodeId) -> Result<&str, ExecutionError> {
        self.nodes
            .get(id)
            .map(String::as_str)
            .ok_or(ExecutionError::UnknownNode)
    }

    #[allow(clippy::too_many_lines)] // exhaustive expression lowering with explicit refusals
    fn scalar(&mut self, expr: &ScalarExpr) -> Result<String, ExecutionError> {
        sql_type(&expr.value_type)?;
        match &expr.kind {
            ScalarExprKind::FieldRef(id) | ScalarExprKind::DimensionRef(id) => {
                Ok(quote(id.as_str()))
            }
            ScalarExprKind::Literal(value) => self.literal(value),
            ScalarExprKind::Unary { op, operand } => Ok(format!(
                "({} {})",
                if *op == UnaryOp::Not { "NOT" } else { "-" },
                self.scalar(operand)?
            )),
            ScalarExprKind::NullTest { operand, negated } => Ok(format!(
                "({} IS {}NULL)",
                self.scalar(operand)?,
                if *negated { "NOT " } else { "" }
            )),
            ScalarExprKind::BooleanLogic { op, operands } => {
                let terms = operands
                    .iter()
                    .map(|e| self.scalar(e))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!(
                    "({})",
                    terms.join(if *op == BooleanOp::And {
                        " AND "
                    } else {
                        " OR "
                    })
                ))
            }
            ScalarExprKind::Case {
                branches,
                otherwise,
            } => {
                let mut sql = "CASE".to_owned();
                for (condition, value) in branches {
                    write!(
                        sql,
                        " WHEN {} THEN {}",
                        self.scalar(condition)?,
                        self.scalar(value)?
                    )
                    .map_err(|_| ExecutionError::Engine)?;
                }
                Ok(format!("{sql} ELSE {} END", self.scalar(otherwise)?))
            }
            ScalarExprKind::ExplicitCast {
                operand,
                rounding: None,
            } => self.cast(operand, &expr.value_type),
            ScalarExprKind::Arithmetic {
                op,
                left,
                right,
                contract,
            } if contract.rounding.is_none()
                && matches!(
                    op,
                    ArithmeticOp::Add | ArithmeticOp::Subtract | ArithmeticOp::Multiply
                ) =>
            {
                let op = match op {
                    ArithmeticOp::Add => "+",
                    ArithmeticOp::Subtract => "-",
                    _ => "*",
                };
                Ok(format!(
                    "CAST(({} {op} {}) AS {})",
                    self.scalar(left)?,
                    self.scalar(right)?,
                    sql_type(&expr.value_type)?
                ))
            }
            ScalarExprKind::ContextSlot(name) => {
                if !matches!(expr.value_type, ValueType::Utf8 | ValueType::Boolean) {
                    return Err(ExecutionError::Unsupported("context type"));
                }
                self.parameters
                    .push(Parameter::Context(name.clone(), expr.value_type.clone()));
                Ok(format!("${}", self.parameters.len()))
            }
            ScalarExprKind::Comparison { op, left, right } => Ok(format!(
                "({} {} {})",
                self.scalar(left)?,
                comparison(*op),
                self.scalar(right)?
            )),
            ScalarExprKind::TemporalDerivation { operand, bucket } => {
                if bucket.calendar != sef_model_ir::time::CalendarProfile::Gregorian {
                    return Err(ExecutionError::Unsupported("calendar profile"));
                }
                let source = self.scalar(operand)?;
                let local = if matches!(
                    operand.value_type,
                    ValueType::Timestamp {
                        timezone_semantics: TimestampSemantics::Instant
                    }
                ) {
                    let zone = self.literal(&Literal::Utf8(bucket.timezone.as_str().into()))?;
                    format!("sef_local_date(epoch_us({source}), {zone})")
                } else {
                    source
                };
                Ok(format!(
                    "CAST(date_trunc('{}', {local}) AS DATE)",
                    time_grain(bucket.grain)?
                ))
            }
            _ => Err(ExecutionError::Unsupported("scalar expression")),
        }
    }

    fn cast(&mut self, operand: &ScalarExpr, ty: &ValueType) -> Result<String, ExecutionError> {
        let numeric_scale = |ty: &ValueType| match ty {
            ValueType::Decimal(d) => Some(d.scale()),
            ValueType::Integer { .. } => Some(0),
            _ => None,
        };
        if operand.value_type != *ty
            && !matches!((numeric_scale(&operand.value_type), numeric_scale(ty)), (Some(from), Some(to)) if to >= from)
        {
            return Err(ExecutionError::Unsupported("exact cast"));
        }
        Ok(format!(
            "CAST({} AS {})",
            self.scalar(operand)?,
            sql_type(ty)?
        ))
    }

    fn literal(&mut self, value: &Literal) -> Result<String, ExecutionError> {
        bind_literal(value)?;
        let ty = sql_type(&value.value_type())?;
        self.parameters.push(Parameter::Literal(value.clone()));
        Ok(format!("CAST(${} AS {ty})", self.parameters.len()))
    }

    fn filter(&mut self, predicate: &FilterExpr) -> Result<String, ExecutionError> {
        let filter_targets = self.filter_targets.clone();
        let target = |t: &FilterTarget| match t {
            FilterTarget::Dimension(d) if d.roles.is_empty() => Ok(quote(d.dimension.as_str())),
            FilterTarget::Metric(id) => filter_targets
                .get(id)
                .map(|id| quote(id.as_str()))
                .ok_or(ExecutionError::UnknownBinding),
            FilterTarget::Dimension(_) => Err(ExecutionError::Unsupported("filter target")),
        };
        Ok(match predicate {
            FilterExpr::Compare {
                target: t,
                op,
                value,
            } => {
                let op = match op {
                    ComparisonOp::Equal => "=",
                    ComparisonOp::NotEqual => "<>",
                    ComparisonOp::Less => "<",
                    ComparisonOp::LessOrEqual => "<=",
                    ComparisonOp::Greater => ">",
                    ComparisonOp::GreaterOrEqual => ">=",
                };
                format!("({} {op} {})", target(t)?, self.literal(value)?)
            }
            FilterExpr::Between {
                target: t,
                low,
                high,
            } => format!(
                "({} BETWEEN {} AND {})",
                target(t)?,
                self.literal(low)?,
                self.literal(high)?
            ),
            FilterExpr::In { target: t, values } => {
                let values = values
                    .iter()
                    .map(|v| self.literal(v))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("({} IN ({}))", target(t)?, values.join(","))
            }
            FilterExpr::IsNull(t) => format!("({} IS NULL)", target(t)?),
            FilterExpr::IsNotNull(t) => format!("({} IS NOT NULL)", target(t)?),
            FilterExpr::And(terms) | FilterExpr::Or(terms) => {
                let terms = terms
                    .iter()
                    .map(|t| self.filter(t))
                    .collect::<Result<Vec<_>, _>>()?;
                format!(
                    "({})",
                    terms.join(if matches!(predicate, FilterExpr::And(_)) {
                        " AND "
                    } else {
                        " OR "
                    })
                )
            }
            FilterExpr::Not(term) => format!("(NOT {})", self.filter(term)?),
        })
    }

    /// A metric past 38 digits as an exact decimal carrier (#261): a sum or
    /// difference evaluated by `sef_exact_combine` over its operands'
    /// carriers, never narrowed to a native type.
    fn wide_metric(
        expr: &MetricExpr,
        ty: sef_model_ir::types::DecimalType,
    ) -> Result<String, ExecutionError> {
        match &expr.kind {
            MetricExprKind::MeasureRef(id) | MetricExprKind::MetricRef(id) => {
                Ok(quote(id.as_str()))
            }
            MetricExprKind::Arithmetic {
                op,
                left,
                right,
                contract,
            } if contract.rounding.is_none()
                && matches!(op, ArithmeticOp::Add | ArithmeticOp::Subtract) =>
            {
                let carrier = |side: &MetricExpr| -> Result<String, ExecutionError> {
                    let sql = Self::metric(side)?;
                    Ok(if wide_decimal(&side.value_type).is_some() {
                        sql
                    } else {
                        format!("CAST({sql} AS VARCHAR)")
                    })
                };
                Ok(format!(
                    "sef_exact_combine({}, {}, '{}:{}:{}')",
                    carrier(left)?,
                    carrier(right)?,
                    ty.precision(),
                    ty.scale(),
                    if *op == ArithmeticOp::Add { "+" } else { "-" }
                ))
            }
            _ => Err(ExecutionError::Unsupported("wide metric expression")),
        }
    }

    fn metric(expr: &MetricExpr) -> Result<String, ExecutionError> {
        if let Some(ty) = wide_decimal(&expr.value_type) {
            return Self::wide_metric(expr, ty);
        }
        sql_type(&expr.value_type)?;
        match &expr.kind {
            MetricExprKind::MeasureRef(id) | MetricExprKind::MetricRef(id) => {
                Ok(quote(id.as_str()))
            }
            MetricExprKind::Arithmetic {
                op,
                left,
                right,
                contract,
            } if contract.rounding.is_none()
                && matches!(op, ArithmeticOp::Add | ArithmeticOp::Subtract) =>
            {
                let op = if *op == ArithmeticOp::Add { "+" } else { "-" };
                Ok(format!(
                    "({} {op} {})",
                    Self::metric(left)?,
                    Self::metric(right)?
                ))
            }
            MetricExprKind::Ratio {
                numerator,
                denominator,
                zero_denominator,
                rounding,
            } => {
                let ValueType::Decimal(ty) = expr.value_type else {
                    return Err(ExecutionError::Unsupported("ratio type"));
                };
                let mode = match rounding.ok_or(ExecutionError::Unsupported("ratio rounding"))? {
                    sef_model_ir::types::RoundingMode::HalfEven => "half-even",
                    sef_model_ir::types::RoundingMode::HalfUp => "half-up",
                    sef_model_ir::types::RoundingMode::Floor => "floor",
                    sef_model_ir::types::RoundingMode::Ceiling => "ceiling",
                    sef_model_ir::types::RoundingMode::TowardZero => "toward-zero",
                };
                let a = Self::metric(numerator)?;
                let b = Self::metric(denominator)?;
                let zero = match zero_denominator {
                    sef_model_ir::types::ZeroDivisionPolicy::Null => "NULL",
                    sef_model_ir::types::ZeroDivisionPolicy::Error => {
                        "error('SEF zero denominator')"
                    }
                };
                Ok(format!(
                    "CAST(CASE WHEN {b} = 0 THEN {zero} ELSE sef_exact_ratio(CAST({a} AS VARCHAR), CAST({b} AS VARCHAR), '{}:{mode}') END AS {})",
                    ty.scale(),
                    sql_type(&expr.value_type)?
                ))
            }
            MetricExprKind::ExplicitCast {
                operand,
                rounding: None,
            } => {
                let scale = |ty: &ValueType| match ty {
                    ValueType::Decimal(d) => Some(d.scale()),
                    ValueType::Integer { .. } => Some(0),
                    _ => None,
                };
                if operand.value_type != expr.value_type
                    && !matches!((scale(&operand.value_type), scale(&expr.value_type)), (Some(a), Some(b)) if b >= a)
                {
                    return Err(ExecutionError::Unsupported("exact metric cast"));
                }
                Ok(format!(
                    "CAST({} AS {})",
                    Self::metric(operand)?,
                    sql_type(&expr.value_type)?
                ))
            }
            _ => Err(ExecutionError::Unsupported("metric expression")),
        }
    }

    fn grain(atoms: &[RequestedAtom]) -> Result<Vec<String>, ExecutionError> {
        atoms
            .iter()
            .map(|atom| match atom {
                RequestedAtom::Dimension(d) if d.roles.is_empty() => {
                    Ok(quote(d.dimension.as_str()))
                }
                RequestedAtom::TimeBucket(time) if time.dimension.roles.is_empty() => {
                    Ok(quote(time.dimension.dimension.as_str()))
                }
                _ => Err(ExecutionError::Unsupported("qualified/time grain")),
            })
            .collect()
    }

    #[allow(clippy::too_many_lines)]
    fn operator(&mut self, op: &Operator) -> Result<String, ExecutionError> {
        match op {
            Operator::ReadSemanticDataset { dataset, fields } => {
                let dataset = self
                    .model
                    .datasets
                    .iter()
                    .find(|d| d.metadata.id() == &dataset.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                let binding = self
                    .model
                    .source_bindings
                    .iter()
                    .find(|b| b.metadata.id() == &dataset.source_binding)
                    .ok_or(ExecutionError::UnknownBinding)?;
                let relation = self
                    .catalog
                    .get(&(binding.source_id.clone(), binding.relation_ref.clone()))
                    .ok_or(ExecutionError::UnknownBinding)?;
                if !(1..=3).contains(&relation.name.len()) {
                    return Err(ExecutionError::InvalidIdentifier);
                }
                let table = relation
                    .name
                    .iter()
                    .map(|part| physical(part))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(".");
                let mut projections = Vec::new();
                let mut checks = Vec::new();
                for id in fields {
                    let field = dataset
                        .fields
                        .iter()
                        .find(|f| f.metadata.id() == id)
                        .ok_or(ExecutionError::UnknownBinding)?;
                    sql_type(&field.value_type)?;
                    let column = relation
                        .columns
                        .get(&field.binding)
                        .ok_or(ExecutionError::UnknownBinding)?;
                    let value = physical(column)?;
                    let value = if field.value_type == ValueType::Utf8 {
                        format!("({value} COLLATE \"binary\")")
                    } else {
                        value
                    };
                    projections.push(format!("{value} AS {}", quote(id.as_str())));
                    checks.push((column.clone(), field.value_type.clone(), field.nullable));
                }
                self.sources.push(SourceCheck {
                    name: relation.name.join("."),
                    source: binding.source_id.as_str().to_owned(),
                    relation: binding.relation_ref.as_str().to_owned(),
                    columns: checks,
                });
                Ok(format!("SELECT {} FROM {table}", projections.join(", ")))
            }
            Operator::PolicyGuard { input, policy, .. } => {
                let policy = self
                    .model
                    .policies
                    .iter()
                    .find(|p| p.metadata.id() == &policy.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                let predicate = self.scalar(&policy.predicate)?;
                Ok(format!(
                    "SELECT * FROM {} WHERE {predicate}",
                    self.node(input)?
                ))
            }
            Operator::ApplyRowFilter { input, predicate } => {
                let predicate = self.filter(predicate)?;
                Ok(format!(
                    "SELECT * FROM {} WHERE {predicate}",
                    self.node(input)?
                ))
            }
            Operator::ApplyPostAggregateFilter { input, predicate } => {
                if self.wide.is_some() {
                    return Err(ExecutionError::Unsupported("emulated post-filter"));
                }
                let predicate = self.filter(predicate)?;
                Ok(format!(
                    "SELECT * FROM {} WHERE {predicate}",
                    self.node(input)?
                ))
            }
            Operator::NormalizeTime {
                input,
                dimension,
                timezone,
                calendar,
            } => {
                if *calendar != sef_model_ir::time::CalendarProfile::Gregorian {
                    return Err(ExecutionError::Unsupported("calendar profile"));
                }
                let definition = self
                    .model
                    .dimensions
                    .iter()
                    .find(|d| d.metadata.id() == &dimension.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                if definition.time_semantics.as_ref().is_some_and(|t| {
                    t.timezone != *timezone
                        || t.pinned_tzdb_version
                            .as_deref()
                            .is_some_and(|v| v != chrono_tz::IANA_TZDB_VERSION)
                }) {
                    return Err(ExecutionError::Unsupported("pinned timezone database"));
                }
                // AttachDimension evaluates the certified TemporalDerivation
                // using pinned TZDB, independently of the connection zone.
                Ok(format!("SELECT * FROM {}", self.node(input)?))
            }
            Operator::TimeBucket {
                input,
                dimension,
                grain,
                calendar,
            } => {
                if *calendar != sef_model_ir::time::CalendarProfile::Gregorian {
                    return Err(ExecutionError::Unsupported("calendar profile"));
                }
                let name = quote(dimension.id.as_str());
                Ok(format!(
                    "SELECT * REPLACE(CAST(date_trunc('{}', {name}) AS DATE) AS {name}) FROM {}",
                    time_grain(*grain)?,
                    self.node(input)?
                ))
            }
            Operator::ComputeWindowMetric {
                input,
                metric,
                base,
                order_by,
                partition_by,
            } => {
                if self.wide.is_some() {
                    return Err(ExecutionError::Unsupported("emulated window"));
                }
                let definition = self
                    .model
                    .metrics
                    .iter()
                    .find(|m| m.metadata.id() == &metric.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                let partition = self.scope(partition_by, &self.dataset_of(*input)?)?;
                let partition = if partition.is_empty() {
                    String::new()
                } else {
                    format!("PARTITION BY {} ", partition.join(","))
                };
                let Some(sef_model_ir::model::WindowBehavior::Cumulative {
                    order_by: declared_order,
                    ..
                }) = &definition.window_behavior
                else {
                    return Err(ExecutionError::InvalidCertificate);
                };
                if declared_order.iter().map(|o| &o.value).collect::<Vec<_>>()
                    != order_by.iter().collect::<Vec<_>>()
                {
                    return Err(ExecutionError::InvalidCertificate);
                }
                let order = declared_order
                    .iter()
                    .map(|o| {
                        format!(
                            "{} {} NULLS {}",
                            quote(o.value.as_str()),
                            if o.direction == SortDirection::Ascending {
                                "ASC"
                            } else {
                                "DESC"
                            },
                            if o.nulls == sef_model_ir::time::NullPlacement::First {
                                "FIRST"
                            } else {
                                "LAST"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                Ok(format!(
                    "SELECT *, CAST(SUM({}) OVER ({partition}ORDER BY {order} ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS {}) AS {} FROM {}",
                    quote(base.as_str()),
                    sql_type(&definition.value_type)?,
                    quote(metric.id.as_str()),
                    self.node(input)?
                ))
            }
            Operator::SelectPeriodEnd {
                input,
                metric,
                axis,
                rule,
                period,
                scope,
            } => {
                let definition = self
                    .model
                    .metrics
                    .iter()
                    .find(|m| m.metadata.id() == &metric.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                let behavior = definition
                    .time_behavior
                    .as_ref()
                    .ok_or(ExecutionError::UnknownBinding)?;
                let dataset = self.dataset_of(*input)?;
                let mut partition = self.scope(scope, &dataset)?;
                if period.is_some() {
                    partition.push(quote(axis.as_str()));
                }
                let partition = if partition.is_empty() {
                    String::new()
                } else {
                    format!("PARTITION BY {} ", partition.join(","))
                };
                let mut order = behavior.order_by.clone();
                match &behavior.ties {
                    sef_model_ir::model::TemporalTies::UniqueByContract { key } => {
                        let binding = self
                            .model
                            .datasets
                            .iter()
                            .find(|d| d.metadata.id() == &dataset)
                            .and_then(|d| d.key_bindings.iter().find(|b| &b.entity_key == key))
                            .ok_or(ExecutionError::UnknownBinding)?;
                        let keys = binding
                            .fields
                            .iter()
                            .map(|id| quote(id.as_str()))
                            .collect::<Vec<_>>()
                            .join(",");
                        self.checks.push(format!(
                            "SELECT EXISTS(SELECT 1 FROM {} GROUP BY {keys} HAVING COUNT(*) > 1)",
                            self.node(input)?
                        ));
                    }
                    sef_model_ir::model::TemporalTies::SecondaryKey { fields } => {
                        order.extend(fields.clone());
                    }
                    sef_model_ir::model::TemporalTies::Unspecified => {
                        return Err(ExecutionError::Unsupported("period selection ties"));
                    }
                }
                let reverse = *rule == sef_model_ir::model::TemporalSelection::PeriodEnd;
                let order = order
                    .iter()
                    .map(|o| {
                        let descending = (o.direction == SortDirection::Descending) ^ reverse;
                        let null_first =
                            (o.nulls == sef_model_ir::time::NullPlacement::First) ^ reverse;
                        format!(
                            "{} {} NULLS {}",
                            quote(o.value.as_str()),
                            if descending { "DESC" } else { "ASC" },
                            if null_first { "FIRST" } else { "LAST" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                Ok(format!(
                    "SELECT * FROM {} QUALIFY ROW_NUMBER() OVER ({partition}ORDER BY {order}) = 1",
                    self.node(input)?
                ))
            }
            Operator::ApplyIntrinsicFilter { input, owner } => {
                let predicate = self
                    .model
                    .metrics
                    .iter()
                    .find(|m| m.metadata.id() == &owner.id)
                    .and_then(|m| m.intrinsic_filter.as_ref())
                    .or_else(|| {
                        self.model
                            .measures
                            .iter()
                            .find(|m| m.metadata.id() == &owner.id)
                            .and_then(|m| m.intrinsic_filter.as_ref())
                    })
                    .ok_or(ExecutionError::UnknownBinding)?;
                let predicate = self.scalar(predicate)?;
                Ok(format!(
                    "SELECT * FROM {} WHERE {predicate}",
                    self.node(input)?
                ))
            }
            Operator::DistinctAggregate {
                input,
                grain,
                measure,
                serves,
            } => self.operator(&Operator::AggregateAtGrain {
                input: *input,
                grain: grain.clone(),
                measures: vec![measure.clone()],
                serves: serves.clone(),
            }),
            Operator::OrderSemanticOutput { input, order } => {
                if self.wide.is_some() {
                    return Err(ExecutionError::Unsupported("ordering emulated output"));
                }
                let order = order
                    .iter()
                    .map(|o| {
                        let key = self.carriers.get(&o.selection_id).map_or_else(
                            || quote(&o.selection_id),
                            |scale| {
                                format!("sef_exact_sort_key({}, '{scale}')", quote(&o.selection_id))
                            },
                        );
                        format!(
                            "{key} {} NULLS {}",
                            if o.direction == SortDirection::Ascending {
                                "ASC"
                            } else {
                                "DESC"
                            },
                            if o.nulls == NullOrdering::First {
                                "FIRST"
                            } else {
                                "LAST"
                            }
                        )
                    })
                    .collect::<Vec<_>>();
                Ok(format!(
                    "SELECT * FROM {} ORDER BY {}",
                    self.node(input)?,
                    order.join(",")
                ))
            }
            Operator::LimitSemanticOutput {
                input,
                limit,
                offset,
            } => {
                if self.wide.is_some() {
                    return Err(ExecutionError::Unsupported("limiting emulated output"));
                }
                Ok(format!(
                    "SELECT * FROM {}{}{}",
                    self.node(input)?,
                    limit.map_or(String::new(), |n| format!(" LIMIT {n}")),
                    offset.map_or(String::new(), |n| format!(" OFFSET {n}"))
                ))
            }
            Operator::ContractGuard {
                input,
                contract: Contract::UniqueTarget { relationship },
            } => {
                self.unique.insert(relationship.id.clone());
                Ok(format!("SELECT * FROM {}", self.node(input)?))
            }
            Operator::TemporalAsOf {
                input,
                relationship,
            } => {
                self.temporal.insert(relationship.id.clone());
                Ok(format!("SELECT * FROM {}", self.node(input)?))
            }
            Operator::ContractGuard {
                input,
                contract: Contract::TemporalSingleMatch { relationship },
            } => {
                if !self.temporal.contains(&relationship.id) {
                    return Err(ExecutionError::InvalidCertificate);
                }
                Ok(format!("SELECT * FROM {}", self.node(input)?))
            }
            Operator::BridgeAllocate { input, bridge } => {
                self.allocations.insert(bridge.id.clone());
                Ok(format!("SELECT * FROM {}", self.node(input)?))
            }
            Operator::ContractGuard {
                input,
                contract: Contract::AllocationNormalization { bridge },
            } => {
                if !self.allocations.contains(&bridge.id) {
                    return Err(ExecutionError::InvalidCertificate);
                }
                Ok(format!("SELECT * FROM {}", self.node(input)?))
            }
            Operator::AttachDimension {
                input,
                definition,
                path,
                lookups,
                dimension,
            } if dimension.roles.is_empty() => {
                let dimension = self
                    .model
                    .dimensions
                    .iter()
                    .find(|d| d.metadata.id() == &definition.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                if dimension.time_semantics.as_ref().is_some_and(|t| {
                    t.pinned_tzdb_version
                        .as_deref()
                        .is_some_and(|v| v != chrono_tz::IANA_TZDB_VERSION)
                }) {
                    return Err(ExecutionError::Unsupported("pinned timezone database"));
                }
                // Select a binding whose fields exist in this branch: the
                // source dataset is recovered from the operator lineage.
                if path.len() != lookups.len() {
                    return Err(ExecutionError::UnknownBinding);
                }
                let mut input_name = self.node(input)?.to_owned();
                let mut dataset = self.dataset_of(*input)?;
                for (reference, lookup) in path.iter().zip(lookups) {
                    let relationship = self
                        .model
                        .relationships
                        .iter()
                        .find(|r| r.metadata.id() == &reference.id)
                        .ok_or(ExecutionError::UnknownBinding)?;
                    let reverse = relationship.to_dataset == dataset
                        && self.model.bridges.iter().any(|b| {
                            self.allocations.contains(b.metadata.id())
                                && b.left_relationship == reference.id
                                && b.left_dataset == dataset
                        });
                    if relationship.from_dataset != dataset && !reverse {
                        return Err(ExecutionError::Unsupported(
                            "relationship direction/temporal",
                        ));
                    }
                    if !self.unique.contains(&reference.id)
                        && !self.temporal.contains(&reference.id)
                        && !matches!(
                            relationship.evidence,
                            sef_model_ir::relationships::ContractEvidenceState::Verified { .. }
                        )
                    {
                        return Err(ExecutionError::InvalidCertificate);
                    }
                    let lookup_name = self.node(lookup)?.to_owned();
                    let keys = relationship
                        .key_pairs
                        .iter()
                        .map(|pair| quote(pair.to_field.as_str()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    if !reverse && relationship.temporal_constraint.is_none() {
                        self.checks.push(format!(
                            "SELECT EXISTS(SELECT 1 FROM {lookup_name} GROUP BY {keys} HAVING COUNT(*) > 1)"
                        ));
                    }
                    let mut on = relationship
                        .key_pairs
                        .iter()
                        .map(|p| {
                            format!(
                                "a.{} = b.{}",
                                quote(if reverse {
                                    p.to_field.as_str()
                                } else {
                                    p.from_field.as_str()
                                }),
                                quote(if reverse {
                                    p.from_field.as_str()
                                } else {
                                    p.to_field.as_str()
                                })
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    if let Some(temporal) = &relationship.temporal_constraint {
                        use sef_model_ir::relationships::{IntervalBoundary, MatchCardinality};
                        if !self.temporal.contains(&reference.id) {
                            return Err(ExecutionError::InvalidCertificate);
                        }
                        let start = if matches!(
                            temporal.interval_boundary,
                            IntervalBoundary::ClosedOpen | IntervalBoundary::ClosedClosed
                        ) {
                            ">="
                        } else {
                            ">"
                        };
                        write!(
                            on,
                            " AND a.{} {start} b.{}",
                            quote(temporal.event_time.as_str()),
                            quote(temporal.valid_from.as_str())
                        )
                        .map_err(|_| ExecutionError::Engine)?;
                        if let Some(end) = &temporal.valid_to {
                            let end_op = if matches!(
                                temporal.interval_boundary,
                                IntervalBoundary::ClosedClosed | IntervalBoundary::OpenClosed
                            ) {
                                "<="
                            } else {
                                "<"
                            };
                            write!(
                                on,
                                " AND (b.{} IS NULL OR a.{} {end_op} b.{})",
                                quote(end.as_str()),
                                quote(temporal.event_time.as_str()),
                                quote(end.as_str())
                            )
                            .map_err(|_| ExecutionError::Engine)?;
                        }
                        let test = if temporal.match_cardinality == MatchCardinality::ExactlyOne {
                            "<> 1"
                        } else {
                            "> 1"
                        };
                        self.checks.push(format!("SELECT EXISTS(SELECT 1 FROM {input_name} a WHERE (SELECT COUNT(*) FROM {lookup_name} b WHERE {on}) {test})"));
                    }
                    if reverse {
                        let bridge = self
                            .model
                            .bridges
                            .iter()
                            .find(|b| {
                                self.allocations.contains(b.metadata.id())
                                    && b.left_relationship == reference.id
                            })
                            .ok_or(ExecutionError::UnknownBinding)?;
                        let allocation = bridge
                            .allocation
                            .as_ref()
                            .ok_or(ExecutionError::UnknownBinding)?;
                        let weight = self.scalar(&allocation.weight_expression)?;
                        let required =
                            self.literal(&Literal::Decimal(allocation.required_total.clone()))?;
                        let tolerance =
                            self.literal(&Literal::Decimal(allocation.tolerance.clone()))?;
                        let bridge_keys = relationship
                            .key_pairs
                            .iter()
                            .map(|p| quote(p.from_field.as_str()))
                            .collect::<Vec<_>>()
                            .join(",");
                        self.checks.push(format!("SELECT EXISTS(SELECT 1 FROM {lookup_name} GROUP BY {bridge_keys} HAVING COUNT({weight}) <> COUNT(*) OR MIN({weight}) < 0 OR ABS(SUM({weight}) - {required}) > {tolerance})"));
                        let unique_keys = self
                            .model
                            .datasets
                            .iter()
                            .find(|d| d.metadata.id() == &bridge.bridge_dataset)
                            .and_then(|d| d.key_bindings.iter().find(|k| k.unique))
                            .ok_or(ExecutionError::Unsupported("bridge unique key"))?;
                        let unique_keys = unique_keys
                            .fields
                            .iter()
                            .map(|id| quote(id.as_str()))
                            .collect::<Vec<_>>()
                            .join(",");
                        self.checks.push(format!("SELECT EXISTS(SELECT 1 FROM {lookup_name} GROUP BY {unique_keys} HAVING COUNT(*) > 1)"));
                        self.checks.push(format!("SELECT EXISTS(SELECT 1 FROM {lookup_name} b WHERE NOT EXISTS(SELECT 1 FROM {input_name} a WHERE {on}))"));
                    }
                    if reverse || relationship.optionality == Optionality::Required {
                        self.checks.push(format!("SELECT EXISTS(SELECT 1 FROM {input_name} a WHERE NOT EXISTS(SELECT 1 FROM {lookup_name} b WHERE {on}))"));
                    }
                    let join = if relationship.optionality == Optionality::Required {
                        "INNER"
                    } else {
                        "LEFT"
                    };
                    input_name = format!(
                        "(SELECT a.*, b.* FROM {input_name} a {join} JOIN {lookup_name} b ON {on})"
                    );
                    dataset = if reverse {
                        relationship.from_dataset.clone()
                    } else {
                        relationship.to_dataset.clone()
                    };
                }
                let binding = dimension
                    .bindings
                    .iter()
                    .find(|b| b.dataset == dataset)
                    .ok_or(ExecutionError::UnknownBinding)?;
                let expression = self.scalar(&binding.expression)?;
                Ok(format!(
                    "SELECT *, {expression} AS {} FROM {input_name}",
                    quote(definition.id.as_str())
                ))
            }
            Operator::AggregateAtGrain {
                input,
                grain,
                measures,
                serves,
            } => {
                if measures.iter().any(|r| {
                    self.model.measures.iter().any(|m| {
                        m.metadata.id() == &r.id
                            && matches!(m.value_type, ValueType::Decimal(d) if d.precision() > 38)
                    })
                }) {
                    if self.wide.is_some() {
                        return Err(ExecutionError::Unsupported(
                            "multiple emulated aggregations",
                        ));
                    }
                    self.wide = Some(wide::Aggregation::new(self.model, grain, measures, serves)?);
                    return Ok(format!("SELECT * FROM {}", self.node(input)?));
                }
                let keys = Self::grain(grain)?;
                let mut projections = keys.clone();
                for reference in measures {
                    let measure = self
                        .model
                        .measures
                        .iter()
                        .find(|m| m.metadata.id() == &reference.id)
                        .ok_or(ExecutionError::UnknownBinding)?;
                    if measure.intrinsic_filter.is_some() {
                        return Err(ExecutionError::Unsupported("intrinsic filter"));
                    }
                    let mut expr = measure
                        .scalar_expression
                        .as_ref()
                        .map_or(Ok("*".into()), |expr| self.scalar(expr))?;
                    let bridges = serves
                        .iter()
                        .filter_map(|r| {
                            self.model
                                .metrics
                                .iter()
                                .find(|m| m.metadata.id() == &r.id)
                                .and_then(|m| m.allocation_bridge.as_ref())
                        })
                        .collect::<std::collections::BTreeSet<_>>();
                    if !bridges.is_empty() {
                        if bridges.len() != 1 || measure.aggregation != AggregateFunction::Sum {
                            return Err(ExecutionError::Unsupported("allocated aggregation"));
                        }
                        let bridge = self
                            .model
                            .bridges
                            .iter()
                            .find(|b| bridges.contains(b.metadata.id()))
                            .ok_or(ExecutionError::UnknownBinding)?;
                        if !self.allocations.contains(bridge.metadata.id()) {
                            return Err(ExecutionError::InvalidCertificate);
                        }
                        let allocation = bridge
                            .allocation
                            .as_ref()
                            .ok_or(ExecutionError::UnknownBinding)?;
                        let weight = self.scalar(&allocation.weight_expression)?;
                        // An allocation may not silently round a fractional
                        // contribution to the measure's canonical scale.
                        self.checks.push(format!("SELECT EXISTS(SELECT 1 FROM {} WHERE ({expr} * {weight}) <> CAST(({expr} * {weight}) AS {}))", self.node(input)?, sql_type(&measure.value_type)?));
                        expr = format!(
                            "CAST(({expr} * {weight}) AS {})",
                            sql_type(&measure.value_type)?
                        );
                    }
                    if measure.aggregation == AggregateFunction::Sum {
                        self.sums = true;
                    }
                    let function = match measure.aggregation {
                        AggregateFunction::Sum => "SUM",
                        AggregateFunction::CountRows
                        | AggregateFunction::CountNonNull
                        | AggregateFunction::CountDistinct => "COUNT",
                        AggregateFunction::Min => "MIN",
                        AggregateFunction::Max => "MAX",
                    };
                    projections.push(format!(
                        "CAST({function}({}{expr}) AS {}) AS {}",
                        if measure.aggregation == AggregateFunction::CountDistinct {
                            "DISTINCT "
                        } else {
                            ""
                        },
                        sql_type(&measure.value_type)?,
                        quote(reference.id.as_str())
                    ));
                }
                let grouped = if keys.is_empty() {
                    String::new()
                } else {
                    format!(" GROUP BY {}", keys.join(", "))
                };
                let aggregated = format!(
                    "SELECT {} FROM {}{grouped}",
                    projections.join(", "),
                    self.node(input)?
                );
                let metrics = serves
                    .iter()
                    .map(|reference| {
                        let metric = self
                            .model
                            .metrics
                            .iter()
                            .find(|m| m.metadata.id() == &reference.id)
                            .ok_or(ExecutionError::UnknownBinding)?;
                        Ok(format!(
                            "CAST({} AS {}) AS {}",
                            Self::metric(&metric.expression)?,
                            sql_type(&metric.value_type)?,
                            quote(reference.id.as_str())
                        ))
                    })
                    .collect::<Result<Vec<_>, ExecutionError>>()?;
                Ok(format!(
                    "SELECT *, {} FROM ({aggregated})",
                    metrics.join(", ")
                ))
            }
            Operator::RecomposeMetric { input, metric } => {
                let definition = self
                    .model
                    .metrics
                    .iter()
                    .find(|m| m.metadata.id() == &metric.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                if wide_decimal(&definition.value_type).is_some() {
                    return Ok(format!(
                        "SELECT *, {} AS {} FROM {}",
                        Self::metric(&definition.expression)?,
                        quote(metric.id.as_str()),
                        self.node(input)?
                    ));
                }
                Ok(format!(
                    "SELECT *, CAST({} AS {}) AS {} FROM {}",
                    Self::metric(&definition.expression)?,
                    sql_type(&definition.value_type)?,
                    quote(metric.id.as_str()),
                    self.node(input)?
                ))
            }
            Operator::AlignSemanticBranches { inputs, grain } if inputs.len() == 2 => {
                if self.wide.is_some() {
                    return Err(ExecutionError::Unsupported(
                        "alignment of emulated aggregates",
                    ));
                }
                let keys = Self::grain(grain)?;
                if keys.is_empty() {
                    return Err(ExecutionError::Unsupported("scalar alignment"));
                }
                let mut projections: Vec<String> = keys
                    .iter()
                    .map(|key| format!("COALESCE(a.{key}, b.{key}) AS {key}"))
                    .collect();
                let mut names = std::collections::BTreeSet::new();
                for (alias, input) in ["a", "b"].iter().zip(inputs) {
                    for column in self.columns.get(input).ok_or(ExecutionError::UnknownNode)? {
                        let quoted = quote(column);
                        if keys.contains(&quoted) {
                            continue;
                        }
                        if !names.insert(column) {
                            return Err(ExecutionError::Unsupported("overlapping branches"));
                        }
                        projections.push(format!("{alias}.{quoted} AS {quoted}"));
                    }
                }
                let on = keys
                    .iter()
                    .map(|key| format!("a.{key} IS NOT DISTINCT FROM b.{key}"))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                Ok(format!(
                    "SELECT {} FROM {} a FULL OUTER JOIN {} b ON {on}",
                    projections.join(", "),
                    self.node(&inputs[0])?,
                    self.node(&inputs[1])?
                ))
            }
            Operator::ProjectSemanticOutput {
                input, selections, ..
            } => {
                if let Some(wide) = &mut self.wide {
                    wide.project(selections)?;
                    return Ok(format!("SELECT * FROM {}", self.node(input)?));
                }
                for selection in selections {
                    if let SelectionItem::Metric(id) = &selection.item {
                        let definition = self
                            .model
                            .metrics
                            .iter()
                            .find(|m| m.metadata.id() == id)
                            .ok_or(ExecutionError::UnknownBinding)?;
                        if let Some(ty) = wide_decimal(&definition.value_type) {
                            self.carriers
                                .insert(selection.selection_id.clone(), ty.scale());
                        }
                    }
                }
                let projections = selections
                    .iter()
                    .map(|selection| {
                        let id = match &selection.item {
                            SelectionItem::Metric(id) => id,
                            SelectionItem::Dimension(d) if d.roles.is_empty() => &d.dimension,
                            SelectionItem::TimeDimension(time)
                                if time.dimension.roles.is_empty() =>
                            {
                                &time.dimension.dimension
                            }
                            _ => return Err(ExecutionError::Unsupported("output")),
                        };
                        Ok(format!(
                            "{} AS {}",
                            quote(id.as_str()),
                            quote(&selection.selection_id)
                        ))
                    })
                    .collect::<Result<Vec<_>, ExecutionError>>()?;
                Ok(format!(
                    "SELECT {} FROM {}",
                    projections.join(", "),
                    self.node(input)?
                ))
            }
            _ => Err(ExecutionError::Unsupported(op.name())),
        }
    }

    fn dataset_of(&self, id: NodeId) -> Result<StableId, ExecutionError> {
        // Per-node dataset provenance is filled when a read or unary operator
        // is lowered, rather than inferring it from physical SQL.
        self.datasets
            .get(&id)
            .cloned()
            .ok_or(ExecutionError::UnknownBinding)
    }

    fn scope(
        &self,
        grain: &sef_model_ir::grain::Grain,
        dataset: &StableId,
    ) -> Result<Vec<String>, ExecutionError> {
        use sef_model_ir::grain::GrainAtom;
        let mut columns = Vec::new();
        for atom in grain.atoms() {
            match atom {
                GrainAtom::DimensionRef { dimension, roles } if roles.is_empty() => {
                    columns.push(quote(dimension.as_str()));
                }
                GrainAtom::EntityKeyRef(key) => {
                    let binding = self
                        .model
                        .datasets
                        .iter()
                        .find(|d| d.metadata.id() == dataset)
                        .and_then(|d| d.key_bindings.iter().find(|b| &b.entity_key == key))
                        .ok_or(ExecutionError::UnknownBinding)?;
                    columns.extend(binding.fields.iter().map(|id| quote(id.as_str())));
                }
                _ => return Err(ExecutionError::Unsupported("scope atom")),
            }
        }
        Ok(columns)
    }

    fn output_columns(&self, op: &Operator) -> Result<Vec<String>, ExecutionError> {
        let inherited = |input: &NodeId| {
            self.columns
                .get(input)
                .cloned()
                .ok_or(ExecutionError::UnknownNode)
        };
        match op {
            Operator::ReadSemanticDataset { fields, .. } => {
                Ok(fields.iter().map(ToString::to_string).collect())
            }
            Operator::AttachDimension {
                input,
                definition,
                lookups,
                ..
            } => {
                let mut columns = inherited(input)?;
                for lookup in lookups {
                    columns.extend(inherited(lookup)?);
                }
                columns.push(definition.id.to_string());
                Ok(columns)
            }
            Operator::RecomposeMetric {
                input,
                metric: definition,
            } => {
                let mut columns = inherited(input)?;
                columns.push(definition.id.to_string());
                Ok(columns)
            }
            Operator::AggregateAtGrain {
                grain,
                measures,
                serves,
                ..
            } => {
                let mut columns = Vec::new();
                for atom in grain {
                    if let RequestedAtom::Dimension(d) = atom {
                        columns.push(d.dimension.to_string());
                    }
                    if let RequestedAtom::TimeBucket(t) = atom {
                        columns.push(t.dimension.dimension.to_string());
                    }
                }
                columns.extend(measures.iter().chain(serves).map(|r| r.id.to_string()));
                Ok(columns)
            }
            Operator::DistinctAggregate {
                input,
                grain,
                measure,
                serves,
            } => self.output_columns(&Operator::AggregateAtGrain {
                input: *input,
                grain: grain.clone(),
                measures: vec![measure.clone()],
                serves: serves.clone(),
            }),
            Operator::AlignSemanticBranches { inputs, .. } => {
                let mut columns = Vec::new();
                for input in inputs {
                    columns.extend(inherited(input)?);
                }
                columns.sort();
                columns.dedup();
                Ok(columns)
            }
            Operator::ProjectSemanticOutput { selections, .. } => {
                Ok(selections.iter().map(|s| s.selection_id.clone()).collect())
            }
            Operator::PolicyGuard { input, .. }
            | Operator::ContractGuard { input, .. }
            | Operator::ApplyRowFilter { input, .. }
            | Operator::ApplyPostAggregateFilter { input, .. }
            | Operator::ApplyIntrinsicFilter { input, .. }
            | Operator::OrderSemanticOutput { input, .. }
            | Operator::LimitSemanticOutput { input, .. }
            | Operator::NormalizeTime { input, .. }
            | Operator::TimeBucket { input, .. }
            | Operator::TemporalAsOf { input, .. }
            | Operator::BridgeAllocate { input, .. }
            | Operator::SelectPeriodEnd { input, .. } => inherited(input),
            Operator::ComputeWindowMetric { input, metric, .. } => {
                let mut columns = inherited(input)?;
                columns.push(metric.id.to_string());
                Ok(columns)
            }
        }
    }
}
