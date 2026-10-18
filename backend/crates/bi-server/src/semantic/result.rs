//! The exact result codec (#136): the Arrow blocks an execution of a
//! [`Prepared`] query produced, as JSON that loses nothing. DECIMAL and
//! INT64 travel as exact decimal text, NULL as `null`, a masked column as
//! `masked: true` with every cell `null` (its values are never read), and a
//! TIMESTAMPTZ as its instant in UTC. A time bucket names its governed zone
//! (where its days begin) as column metadata.
//!
//! The codec only presents what the adapter produced: every batch is checked
//! against the contract of the certified query, and anything that does not
//! match, or does not fit the cell and body budgets, fails as a whole: no
//! partial result.

use std::collections::BTreeSet;

use chrono::Datelike;

use bi_runtime::duckdb::arrow::array::{
    Array, BooleanArray, Date32Array, Decimal128Array, Decimal256Array, Int64Array,
    LargeStringArray, StringArray, StringViewArray, TimestampMicrosecondArray,
    TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray,
};
use bi_runtime::duckdb::arrow::datatypes::{DataType, TimeUnit};
use bi_runtime::duckdb::arrow::record_batch::RecordBatch;
use sef_adapter_duckdb::execution::BlockResult;
use sef_authz::Constraint;
use sef_model_ir::identity::StableId;
use sef_model_ir::types::ValueType;
use sef_model_ir::units::Unit;
use sef_query_ir::query::SelectionItem;
use serde::Serialize;
use utoipa::ToSchema;

use super::prepare::Prepared;

/// One cell, after escaping, at most this many bytes of JSON.
pub const MAX_CELL_BYTES: usize = crate::query::MAX_CELL_BYTES;
/// The whole result, as JSON, at most this many bytes.
pub const MAX_RESULT_BYTES: usize = crate::query::MAX_RESULT_BYTES;

/// The rows of every block of a query, with their exact column contract.
#[derive(Debug, PartialEq, Serialize, ToSchema)]
pub struct SemanticResult {
    pub blocks: Vec<SemanticBlock>,
}

#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticBlock {
    pub block_id: String,
    pub columns: Vec<SemanticColumn>,
    /// One array per row, one cell per column; `null` is a missing value,
    /// or any value of a masked column.
    pub rows: Vec<Vec<Option<SemanticCell>>>,
    pub row_count: u64,
    /// The row limit the plan applied, when the query set one.
    #[schema(required = true)]
    pub limit: Option<u64>,
}

/// One present value: exact decimal text for DECIMAL and INT64, the text
/// of a VARCHAR, `YYYY-MM-DD` for a DATE and an RFC 3339 instant in UTC for
/// a TIMESTAMPTZ; a BOOLEAN as `true` or `false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(untagged)]
pub enum SemanticCell {
    Text(String),
    Boolean(bool),
}

/// The declared unit of a column's values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticUnit {
    /// An ISO 4217 currency (`USD`).
    Currency {
        code: String,
    },
    /// A UCUM unit (`kg`).
    Physical {
        code: String,
    },
    Ratio,
    Percent,
    Count,
    /// A unit of the model's own namespace.
    Custom {
        namespace: String,
        code: String,
    },
}

impl From<&Unit> for SemanticUnit {
    fn from(unit: &Unit) -> Self {
        match unit {
            Unit::Currency { iso4217 } => Self::Currency {
                code: iso4217.clone(),
            },
            Unit::Physical { ucum } => Self::Physical { code: ucum.clone() },
            Unit::Ratio => Self::Ratio,
            Unit::Percent => Self::Percent,
            Unit::Count => Self::Count,
            Unit::Custom { namespace, code } => Self::Custom {
                namespace: namespace.clone(),
                code: code.clone(),
            },
        }
    }
}

/// What a column selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticColumnKind {
    Metric,
    Dimension,
    TimeDimension,
}

#[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
pub struct SemanticColumn {
    pub selection_id: String,
    pub kind: SemanticColumnKind,
    /// The stable identifier of the metric or dimension.
    pub id: String,
    /// `DECIMAL(p,s)`, `INT64`, `VARCHAR`, `BOOLEAN`, `DATE` or `TIMESTAMPTZ`.
    #[serde(rename = "type")]
    pub value_type: String,
    /// Always `true` today: a metric or a dimension may yield `null`.
    pub nullable: bool,
    #[schema(required = true)]
    pub unit: Option<SemanticUnit>,
    /// The governed IANA zone of a time bucket: where its days begin. An
    /// instant needs none, it travels in UTC.
    #[schema(required = true)]
    pub timezone: Option<String>,
    /// Values withheld by the decision: every cell is `null`, unlike a
    /// column whose `null` cells are missing values.
    pub masked: bool,
}

/// Why a result could not be encoded. Nothing of it is answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// A block, column, type or metadata that the certified query does not
    /// declare, or one it declares and is missing.
    SchemaMismatch(&'static str),
    /// A value type the codec has no exact representation for.
    UnsupportedType,
    /// A value outside the precision its contract declares.
    OutOfContract,
    /// More rows than the plan's limit.
    OverLimit,
    /// One cell over [`MAX_CELL_BYTES`] once encoded.
    CellTooLarge,
    /// The whole result over [`MAX_RESULT_BYTES`] once encoded.
    ResultTooLarge,
}

/// What the codec checks of every column, beside its public contract.
struct Check {
    /// The type the adapter writes into `sef.value_type`.
    adapter_type: String,
    /// The unit the adapter writes into `sef.unit`, if any.
    adapter_unit: Option<String>,
    value_type: ValueType,
}

/// Encodes the blocks an execution of `prepared` produced.
///
/// # Errors
/// A [`CodecError`] when a block does not match the certified contract, a
/// value cannot be represented exactly, or a budget is exceeded.
pub fn encode(prepared: &Prepared, results: &[BlockResult]) -> Result<SemanticResult, CodecError> {
    encode_within(prepared, results, MAX_RESULT_BYTES)
}

/// [`encode`] with a body budget of `max_bytes`.
fn encode_within(
    prepared: &Prepared,
    results: &[BlockResult],
    max_bytes: usize,
) -> Result<SemanticResult, CodecError> {
    let masked: BTreeSet<&str> = prepared
        .decision()
        .constraints
        .iter()
        .map(|Constraint::Mask { id, .. }| id.as_str())
        .collect();
    let mut result = SemanticResult { blocks: Vec::new() };
    let mut checks = Vec::new();
    for block in &prepared.validated().query.blocks {
        let (columns, block_checks): (Vec<_>, Vec<_>) = block
            .selections
            .iter()
            .map(|selection| contract(prepared, &selection.selection_id, &selection.item, &masked))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .unzip();
        result.blocks.push(SemanticBlock {
            block_id: block.block_id.clone(),
            columns,
            rows: Vec::new(),
            row_count: 0,
            limit: block.limit,
        });
        checks.push(block_checks);
    }
    // The body as it is with no rows yet; every row adds exactly its bytes.
    let mut size = encoded_len(&result)?;
    if size > max_bytes {
        return Err(CodecError::ResultTooLarge);
    }
    let mut seen = BTreeSet::new();
    for produced in results {
        let index = result
            .blocks
            .iter()
            .position(|b| b.block_id == produced.block_id)
            .ok_or(CodecError::SchemaMismatch("block"))?;
        seen.insert(index);
        let block = &mut result.blocks[index];
        let checks = &checks[index];
        for batch in &produced.batches {
            check_schema(batch, &block.columns, checks)?;
            for row in 0..batch.num_rows() {
                if block.limit.is_some_and(|limit| block.row_count >= limit) {
                    return Err(CodecError::OverLimit);
                }
                let mut cells = Vec::with_capacity(block.columns.len());
                let mut row_bytes = 2 + block.columns.len().saturating_sub(1);
                for (column, (meta, check)) in
                    batch.columns().iter().zip(block.columns.iter().zip(checks))
                {
                    // A masked column's values are never read.
                    let cell = if meta.masked {
                        None
                    } else {
                        cell(column.as_ref(), row, &check.value_type)?
                    };
                    let bytes = encoded_len(&cell)?;
                    if bytes > MAX_CELL_BYTES {
                        return Err(CodecError::CellTooLarge);
                    }
                    row_bytes += bytes;
                    cells.push(cell);
                }
                // The row, its separator, and `row_count` growing a digit.
                size +=
                    row_bytes + usize::from(!block.rows.is_empty()) + digits(block.row_count + 1)
                        - digits(block.row_count);
                if size > max_bytes {
                    return Err(CodecError::ResultTooLarge);
                }
                block.rows.push(cells);
                block.row_count += 1;
            }
        }
    }
    // The executor answers every block, an empty one too: a block never
    // seen is a lost answer, not an empty one.
    if seen.len() != result.blocks.len() {
        return Err(CodecError::SchemaMismatch("block"));
    }
    Ok(result)
}

/// The decimal digits of `n` in JSON.
fn digits(n: u64) -> usize {
    n.checked_ilog10().map_or(1, |log| log as usize + 1)
}

fn encoded_len(value: &impl Serialize) -> Result<usize, CodecError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|_| CodecError::UnsupportedType)
}

/// The contract of one selection: what the model declares for it.
fn contract(
    prepared: &Prepared,
    selection_id: &str,
    item: &SelectionItem,
    masked: &BTreeSet<&str>,
) -> Result<(SemanticColumn, Check), CodecError> {
    let definition = prepared.model().definition();
    let (kind, id, value_type, unit, timezone): (_, &StableId, _, _, _) = match item {
        SelectionItem::Metric(id) => {
            let metric = definition
                .metrics
                .iter()
                .find(|m| m.metadata.id() == id)
                .ok_or(CodecError::SchemaMismatch("metric"))?;
            (
                SemanticColumnKind::Metric,
                id,
                &metric.value_type,
                &metric.unit,
                None,
            )
        }
        SelectionItem::Dimension(d)
        | SelectionItem::TimeDimension(sef_query_ir::query::TimeDimensionSelection {
            dimension: d,
            ..
        }) => {
            let dimension = definition
                .dimensions
                .iter()
                .find(|m| m.metadata.id() == &d.dimension)
                .ok_or(CodecError::SchemaMismatch("dimension"))?;
            let kind = if matches!(item, SelectionItem::TimeDimension(_)) {
                SemanticColumnKind::TimeDimension
            } else {
                SemanticColumnKind::Dimension
            };
            let timezone = dimension
                .time_semantics
                .as_ref()
                .map(|time| time.timezone.as_str().to_owned());
            (
                kind,
                &d.dimension,
                &dimension.value_type,
                &dimension.unit,
                timezone,
            )
        }
    };
    let (public, adapter) = type_labels(value_type)?;
    let column = SemanticColumn {
        selection_id: selection_id.to_owned(),
        kind,
        id: id.as_str().to_owned(),
        value_type: public,
        nullable: true,
        unit: unit.as_ref().map(SemanticUnit::from),
        timezone,
        masked: masked.contains(id.as_str()),
    };
    let check = Check {
        adapter_type: adapter,
        adapter_unit: unit.as_ref().map(|unit| format!("{unit:?}")),
        value_type: value_type.clone(),
    };
    Ok((column, check))
}

/// The public label, and the one the adapter writes in `sef.value_type`.
fn type_labels(value_type: &ValueType) -> Result<(String, String), CodecError> {
    Ok(match value_type {
        ValueType::Decimal(d) => {
            let label = format!("DECIMAL({},{})", d.precision(), d.scale());
            (label.clone(), label)
        }
        ValueType::Integer {
            signed: true,
            bits: Some(64),
        } => ("INT64".into(), "BIGINT".into()),
        ValueType::Utf8 => ("VARCHAR".into(), "VARCHAR".into()),
        ValueType::Boolean => ("BOOLEAN".into(), "BOOLEAN".into()),
        ValueType::Date => ("DATE".into(), "DATE".into()),
        ValueType::Timestamp {
            timezone_semantics: sef_model_ir::time::TimestampSemantics::Instant,
        } => ("TIMESTAMPTZ".into(), "TIMESTAMPTZ".into()),
        _ => return Err(CodecError::UnsupportedType),
    })
}

/// The batch carries exactly the declared columns, in order, with the
/// metadata the adapter writes and an Arrow type that holds the value.
fn check_schema(
    batch: &RecordBatch,
    columns: &[SemanticColumn],
    checks: &[Check],
) -> Result<(), CodecError> {
    let schema = batch.schema();
    if schema.fields().len() != columns.len() {
        return Err(CodecError::SchemaMismatch("columns"));
    }
    for ((field, column), check) in schema.fields().iter().zip(columns).zip(checks) {
        let metadata = field.metadata();
        let id_key = if column.kind == SemanticColumnKind::Metric {
            "sef.metric_id"
        } else {
            "sef.dimension_id"
        };
        if metadata.get("sef.selection_id") != Some(&column.selection_id)
            || metadata.get(id_key) != Some(&column.id)
            || metadata.get("sef.value_type") != Some(&check.adapter_type)
            || metadata.get("sef.unit") != check.adapter_unit.as_ref()
        {
            return Err(CodecError::SchemaMismatch("metadata"));
        }
        if !holds(field.data_type(), &check.value_type) {
            return Err(CodecError::SchemaMismatch("type"));
        }
    }
    Ok(())
}

/// Whether Arrow `data_type` can carry `value_type` without loss.
fn holds(data_type: &DataType, value_type: &ValueType) -> bool {
    use sef_model_ir::time::TimestampSemantics;
    match (data_type, value_type) {
        (
            DataType::Decimal128(_, scale) | DataType::Decimal256(_, scale),
            ValueType::Decimal(d),
        ) => i16::from(*scale) == i16::from(d.scale()),
        (
            DataType::Int64,
            ValueType::Integer {
                signed: true,
                bits: Some(64),
            },
        )
        | (DataType::Boolean, ValueType::Boolean)
        | (DataType::Date32, ValueType::Date)
        | (DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View, ValueType::Utf8)
        | (
            DataType::Timestamp(_, Some(_)),
            ValueType::Timestamp {
                timezone_semantics: TimestampSemantics::Instant,
            },
        ) => true,
        _ => false,
    }
}

/// One value, exactly.
fn cell(
    array: &dyn Array,
    row: usize,
    value_type: &ValueType,
) -> Result<Option<SemanticCell>, CodecError> {
    if array.is_null(row) {
        return Ok(None);
    }
    let any = array.as_any();
    let text = |s: &str| {
        // Its JSON is at least the text and two quotes: refused before any
        // copy.
        if s.len() + 2 > MAX_CELL_BYTES {
            return Err(CodecError::CellTooLarge);
        }
        Ok(Some(SemanticCell::Text(s.to_owned())))
    };
    match array.data_type() {
        DataType::Decimal128(_, scale) => {
            let raw = any
                .downcast_ref::<Decimal128Array>()
                .ok_or(CodecError::UnsupportedType)?;
            decimal(&raw.value(row).to_string(), *scale, value_type).map(Some)
        }
        DataType::Decimal256(_, scale) => {
            let raw = any
                .downcast_ref::<Decimal256Array>()
                .ok_or(CodecError::UnsupportedType)?;
            decimal(&raw.value(row).to_string(), *scale, value_type).map(Some)
        }
        DataType::Int64 => {
            let raw = any
                .downcast_ref::<Int64Array>()
                .ok_or(CodecError::UnsupportedType)?;
            text(&raw.value(row).to_string())
        }
        DataType::Boolean => {
            let raw = any
                .downcast_ref::<BooleanArray>()
                .ok_or(CodecError::UnsupportedType)?;
            Ok(Some(SemanticCell::Boolean(raw.value(row))))
        }
        DataType::Utf8 => text(
            any.downcast_ref::<StringArray>()
                .ok_or(CodecError::UnsupportedType)?
                .value(row),
        ),
        DataType::LargeUtf8 => text(
            any.downcast_ref::<LargeStringArray>()
                .ok_or(CodecError::UnsupportedType)?
                .value(row),
        ),
        DataType::Utf8View => text(
            any.downcast_ref::<StringViewArray>()
                .ok_or(CodecError::UnsupportedType)?
                .value(row),
        ),
        DataType::Date32 => {
            let days = any
                .downcast_ref::<Date32Array>()
                .ok_or(CodecError::UnsupportedType)?
                .value(row);
            let date = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)
                .and_then(|epoch| {
                    epoch.checked_add_signed(chrono::TimeDelta::days(i64::from(days)))
                })
                .ok_or(CodecError::OutOfContract)?;
            // `YYYY-MM-DD` has four digits for the year, and no sign.
            if !(0..=9999).contains(&date.year()) {
                return Err(CodecError::OutOfContract);
            }
            text(&date.format("%Y-%m-%d").to_string())
        }
        DataType::Timestamp(unit, Some(_)) => {
            let value = match unit {
                TimeUnit::Second => any
                    .downcast_ref::<TimestampSecondArray>()
                    .map(|a| a.value(row)),
                TimeUnit::Millisecond => any
                    .downcast_ref::<TimestampMillisecondArray>()
                    .map(|a| a.value(row)),
                TimeUnit::Microsecond => any
                    .downcast_ref::<TimestampMicrosecondArray>()
                    .map(|a| a.value(row)),
                TimeUnit::Nanosecond => any
                    .downcast_ref::<TimestampNanosecondArray>()
                    .map(|a| a.value(row)),
            }
            .ok_or(CodecError::UnsupportedType)?;
            let instant = match unit {
                TimeUnit::Second => chrono::DateTime::from_timestamp(value, 0),
                TimeUnit::Millisecond => chrono::DateTime::from_timestamp_millis(value),
                TimeUnit::Microsecond => chrono::DateTime::from_timestamp_micros(value),
                TimeUnit::Nanosecond => Some(chrono::DateTime::from_timestamp_nanos(value)),
            }
            .ok_or(CodecError::OutOfContract)?;
            // RFC 3339 has four digits for the year, and no sign.
            if !(0..=9999).contains(&instant.year()) {
                return Err(CodecError::OutOfContract);
            }
            text(&instant.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
        }
        _ => Err(CodecError::UnsupportedType),
    }
}

/// `coefficient` (an integer, as text) scaled by `scale`, checked against
/// the precision the contract declares.
fn decimal(
    coefficient: &str,
    scale: i8,
    value_type: &ValueType,
) -> Result<SemanticCell, CodecError> {
    let ValueType::Decimal(declared) = value_type else {
        return Err(CodecError::SchemaMismatch("type"));
    };
    let (sign, digits) = coefficient
        .strip_prefix('-')
        .map_or(("", coefficient), |digits| ("-", digits));
    let significant = digits.trim_start_matches('0').len();
    if significant > usize::from(declared.precision()) {
        return Err(CodecError::OutOfContract);
    }
    let scale = usize::try_from(scale).map_err(|_| CodecError::UnsupportedType)?;
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (whole, fraction) = digits.split_at(digits.len() - scale);
    Ok(SemanticCell::Text(if scale == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }))
}

#[cfg(test)]
mod tests;
