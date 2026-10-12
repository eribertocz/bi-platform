//! Initial, synchronous SEIR lowering. Unsupported semantics fail before SQL.
//! Call execution on an analytical worker, never on an async executor.

use std::collections::BTreeMap;
use std::fmt;

use duckdb::Connection;
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::types::Value;
use sef_correctness::{SemanticContext, ValidatedQuery};
use sef_exec_ir::{NodeId, Operator, SemanticPlan};
use sef_model_ir::expressions::{
    AggregateFunction, ComparisonOp, MetricExpr, MetricExprKind, ScalarExpr, ScalarExprKind,
};
use sef_model_ir::identity::{SourceHandle, StableId};
use sef_model_ir::model::{CompiledSemanticModel, ModelDefinition};
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{ArithmeticOp, Literal, ValueType};
use sef_planner::Certificate;
use sef_query_ir::query::{RequestedAtom, SelectionItem};

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
}

struct SourceCheck {
    name: String,
    columns: Vec<(String, ValueType, bool)>,
}

/// Opaque outside this adapter. Only `lower` can create an executable plan.
/// It contains slot names and types, never context values.
pub struct DuckDbPlan {
    sql: Vec<(String, String)>,
    parameters: Vec<Parameter>,
    sources: Vec<SourceCheck>,
    certificate: Certificate,
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
        let values = self
            .parameters
            .iter()
            .map(|parameter| match parameter {
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
        }
        self.sql
            .iter()
            .map(|(block_id, sql)| {
                let mut statement = connection
                    .prepare(sql)
                    .map_err(|_| ExecutionError::Engine)?;
                let batches = statement
                    .query_arrow(duckdb::params_from_iter(values.iter()))
                    .map_err(|_| ExecutionError::Engine)?
                    .collect();
                Ok(BlockResult {
                    block_id: block_id.clone(),
                    batches,
                })
            })
            .collect()
    }
}

pub struct BlockResult {
    pub block_id: String,
    pub batches: Vec<RecordBatch>,
}

/// Certifies again against CSMIR/SQIR and lowers every reachable operator.
/// Initial subset: local dimensions, SUM/COUNT/MIN/MAX, two aggregated
/// branches, exact +/- recomposition, projection and mandatory policies.
///
/// # Errors
/// Unknown nodes, stale/invalid certificates, missing catalog mappings or
/// any operator/type not implemented exactly (including DECIMAL > 38).
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
    // One block initially: parameter numbering cannot silently drift across
    // independent outputs. Extend with per-block binding maps later.
    if plan.blocks.len() != 1 {
        return Err(ExecutionError::Unsupported("multiple blocks"));
    }
    let mut lowering = Lowering {
        model: model.definition(),
        catalog,
        nodes: BTreeMap::new(),
        datasets: BTreeMap::new(),
        columns: BTreeMap::new(),
        parameters: Vec::new(),
        sources: Vec::new(),
    };
    let mut ctes = Vec::new();
    for node in plan.graph.topological(&roots) {
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
    Ok(DuckDbPlan {
        sql,
        parameters: lowering.parameters,
        sources: lowering.sources,
        certificate: certificate.clone(),
    })
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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

struct Lowering<'a> {
    model: &'a ModelDefinition,
    catalog: &'a SourceCatalog,
    nodes: BTreeMap<NodeId, String>,
    datasets: BTreeMap<NodeId, StableId>,
    columns: BTreeMap<NodeId, Vec<String>>,
    parameters: Vec<Parameter>,
    sources: Vec<SourceCheck>,
}

impl Lowering<'_> {
    fn node(&self, id: &NodeId) -> Result<&str, ExecutionError> {
        self.nodes
            .get(id)
            .map(String::as_str)
            .ok_or(ExecutionError::UnknownNode)
    }

    fn scalar(&mut self, expr: &ScalarExpr) -> Result<String, ExecutionError> {
        sql_type(&expr.value_type)?;
        match &expr.kind {
            ScalarExprKind::FieldRef(id) => Ok(quote(id.as_str())),
            ScalarExprKind::ContextSlot(name) => {
                if !matches!(expr.value_type, ValueType::Utf8 | ValueType::Boolean) {
                    return Err(ExecutionError::Unsupported("context type"));
                }
                self.parameters
                    .push(Parameter::Context(name.clone(), expr.value_type.clone()));
                Ok(format!("${}", self.parameters.len()))
            }
            ScalarExprKind::Comparison { op, left, right } => {
                let op = match op {
                    ComparisonOp::Equal => "=",
                    ComparisonOp::NotEqual => "<>",
                    ComparisonOp::Less => "<",
                    ComparisonOp::LessOrEqual => "<=",
                    ComparisonOp::Greater => ">",
                    ComparisonOp::GreaterOrEqual => ">=",
                };
                Ok(format!(
                    "({} {op} {})",
                    self.scalar(left)?,
                    self.scalar(right)?
                ))
            }
            _ => Err(ExecutionError::Unsupported("scalar expression")),
        }
    }

    fn metric(expr: &MetricExpr) -> Result<String, ExecutionError> {
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
                    projections.push(format!("{} AS {}", physical(column)?, quote(id.as_str())));
                    checks.push((column.clone(), field.value_type.clone(), field.nullable));
                }
                self.sources.push(SourceCheck {
                    name: relation.name.join("."),
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
            Operator::AttachDimension {
                input,
                definition,
                path,
                lookups,
                dimension,
            } if path.is_empty() && lookups.is_empty() && dimension.roles.is_empty() => {
                let dimension = self
                    .model
                    .dimensions
                    .iter()
                    .find(|d| d.metadata.id() == &definition.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                // Select a binding whose fields exist in this branch: the
                // source dataset is recovered from the operator lineage.
                let input_name = self.node(input)?.to_owned();
                let dataset = self.dataset_of(*input)?;
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
                    let expr = measure
                        .scalar_expression
                        .as_ref()
                        .map_or(Ok("*".into()), |expr| self.scalar(expr))?;
                    let function = match measure.aggregation {
                        AggregateFunction::Sum => "SUM",
                        AggregateFunction::CountRows | AggregateFunction::CountNonNull => "COUNT",
                        AggregateFunction::Min => "MIN",
                        AggregateFunction::Max => "MAX",
                        AggregateFunction::CountDistinct => {
                            return Err(ExecutionError::Unsupported("aggregate"));
                        }
                    };
                    projections.push(format!(
                        "CAST({function}({expr}) AS {}) AS {}",
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
                Ok(format!(
                    "SELECT *, CAST({} AS {}) AS {} FROM {}",
                    Self::metric(&definition.expression)?,
                    sql_type(&definition.value_type)?,
                    quote(metric.id.as_str()),
                    self.node(input)?
                ))
            }
            Operator::AlignSemanticBranches { inputs, grain } if inputs.len() == 2 => {
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
                let projections = selections
                    .iter()
                    .map(|selection| {
                        let id = match &selection.item {
                            SelectionItem::Metric(id) => id,
                            SelectionItem::Dimension(d) if d.roles.is_empty() => &d.dimension,
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
                input, definition, ..
            }
            | Operator::RecomposeMetric {
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
                }
                columns.extend(measures.iter().chain(serves).map(|r| r.id.to_string()));
                Ok(columns)
            }
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
            Operator::PolicyGuard { input, .. } => inherited(input),
            _ => Err(ExecutionError::Unsupported(op.name())),
        }
    }
}
