//! Exact coefficient aggregation when a semantic decimal exceeds DuckDB's
//! native 38 digits. DuckDB still performs the governed reads and joins;
//! this adapter evaluates arithmetic with integers and emits Decimal256.

use std::collections::BTreeMap;
use std::sync::Arc;

use duckdb::arrow::array::{
    Array, ArrayRef, Decimal128Array, Decimal256Array, Int64Array, StringArray,
};
use duckdb::arrow::datatypes::{DataType, Field, Schema, i256};
use duckdb::arrow::record_batch::RecordBatch;
use num_bigint::BigInt;
use sef_exec_ir::ObjectRef;
use sef_model_ir::expressions::{AggregateFunction, MetricExprKind, ScalarExpr, ScalarExprKind};
use sef_model_ir::model::{Measure, ModelDefinition};
use sef_model_ir::types::{ArithmeticOp, Literal, ValueType};
use sef_query_ir::query::{RequestedAtom, Selection, SelectionItem};

use super::ExecutionError;

pub(super) struct Aggregation {
    keys: Vec<String>,
    measures: Vec<Measure>,
    metrics: BTreeMap<String, usize>,
    selections: Vec<Selection>,
}

impl Aggregation {
    pub(super) fn new(
        model: &ModelDefinition,
        grain: &[RequestedAtom],
        measures: &[ObjectRef],
        serves: &[ObjectRef],
    ) -> Result<Self, ExecutionError> {
        let keys = grain
            .iter()
            .map(|atom| match atom {
                RequestedAtom::Dimension(d)
                    if d.roles.is_empty()
                        && model.dimensions.iter().any(|def| {
                            def.metadata.id() == &d.dimension && def.value_type == ValueType::Utf8
                        }) =>
                {
                    Ok(d.dimension.to_string())
                }
                _ => Err(ExecutionError::Unsupported("emulated aggregate grain")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let measures = measures
            .iter()
            .map(|r| {
                let m = model
                    .measures
                    .iter()
                    .find(|m| m.metadata.id() == &r.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                if m.aggregation != AggregateFunction::Sum
                    || m.intrinsic_filter.is_some()
                    || !matches!(m.value_type, ValueType::Decimal(_))
                {
                    return Err(ExecutionError::Unsupported("emulated aggregate"));
                }
                validate(
                    m.scalar_expression
                        .as_ref()
                        .ok_or(ExecutionError::UnknownBinding)?,
                )?;
                Ok(m.clone())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let metrics = serves
            .iter()
            .map(|r| {
                let metric = model
                    .metrics
                    .iter()
                    .find(|m| m.metadata.id() == &r.id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                let MetricExprKind::MeasureRef(measure) = &metric.expression.kind else {
                    return Err(ExecutionError::Unsupported("emulated metric recomposition"));
                };
                let index = measures
                    .iter()
                    .position(|m| m.metadata.id() == measure && m.value_type == metric.value_type)
                    .ok_or(ExecutionError::UnknownBinding)?;
                Ok((r.id.to_string(), index))
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?;
        Ok(Self {
            keys,
            measures,
            metrics,
            selections: Vec::new(),
        })
    }

    pub(super) fn project(&mut self, selections: &[Selection]) -> Result<(), ExecutionError> {
        for selection in selections {
            match &selection.item {
                SelectionItem::Metric(id) if self.metrics.contains_key(id.as_str()) => {}
                SelectionItem::Dimension(d)
                    if d.roles.is_empty() && self.keys.contains(&d.dimension.to_string()) => {}
                _ => return Err(ExecutionError::Unsupported("emulated output")),
            }
        }
        self.selections = selections.to_vec();
        Ok(())
    }

    pub(super) fn execute(
        &self,
        batches: &[RecordBatch],
        limits: &super::ExecutionLimits,
    ) -> Result<Vec<RecordBatch>, ExecutionError> {
        let mut groups: BTreeMap<Vec<Option<String>>, Vec<Option<BigInt>>> = BTreeMap::new();
        if self.keys.is_empty() {
            groups.insert(Vec::new(), vec![None; self.measures.len()]);
        }
        for batch in batches {
            let keys = self
                .keys
                .iter()
                .map(|key| {
                    let column = batch.column(
                        batch
                            .schema()
                            .index_of(key)
                            .map_err(|_| ExecutionError::Engine)?,
                    );
                    column
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .ok_or(ExecutionError::SourceContract)
                })
                .collect::<Result<Vec<_>, _>>()?;
            for row in 0..batch.num_rows() {
                limits.checkpoint()?;
                let key = keys
                    .iter()
                    .map(|c| {
                        if c.is_null(row) {
                            None
                        } else {
                            Some(c.value(row).to_owned())
                        }
                    })
                    .collect();
                let sums = groups
                    .entry(key)
                    .or_insert_with(|| vec![None; self.measures.len()]);
                for (i, measure) in self.measures.iter().enumerate() {
                    let expression = measure
                        .scalar_expression
                        .as_ref()
                        .ok_or(ExecutionError::UnknownBinding)?;
                    if let Some(value) = evaluate(expression, batch, row)? {
                        let value = rescale(
                            value,
                            scale(&expression.value_type)?,
                            scale(&measure.value_type)?,
                        )?;
                        sums[i] = Some(sums[i].take().unwrap_or_else(|| BigInt::from(0)) + value);
                    }
                }
            }
        }
        self.output(&groups)
    }

    fn output(
        &self,
        groups: &BTreeMap<Vec<Option<String>>, Vec<Option<BigInt>>>,
    ) -> Result<Vec<RecordBatch>, ExecutionError> {
        let mut arrays: Vec<ArrayRef> = Vec::new();
        let mut fields = Vec::new();
        for selection in &self.selections {
            let mut metadata = std::collections::HashMap::from([(
                "sef.selection_id".to_owned(),
                selection.selection_id.clone(),
            )]);
            let array: ArrayRef = match &selection.item {
                SelectionItem::Dimension(d) => {
                    let index = self
                        .keys
                        .iter()
                        .position(|k| k == d.dimension.as_str())
                        .ok_or(ExecutionError::UnknownBinding)?;
                    metadata.insert("sef.dimension_id".into(), d.dimension.to_string());
                    Arc::new(StringArray::from(
                        groups
                            .keys()
                            .map(|k| k[index].as_deref())
                            .collect::<Vec<_>>(),
                    ))
                }
                SelectionItem::Metric(id) => {
                    let index = *self
                        .metrics
                        .get(id.as_str())
                        .ok_or(ExecutionError::UnknownBinding)?;
                    let ValueType::Decimal(ty) = self.measures[index].value_type else {
                        return Err(ExecutionError::SourceContract);
                    };
                    metadata.insert("sef.metric_id".into(), id.to_string());
                    metadata.insert(
                        "sef.value_type".into(),
                        format!("DECIMAL({},{})", ty.precision(), ty.scale()),
                    );
                    let values = groups
                        .values()
                        .map(|sums| {
                            sums[index]
                                .as_ref()
                                .map(|value| {
                                    // A sum past its certified precision is
                                    // a result out of range (#261).
                                    check_decimal(value, ty.precision())
                                        .map_err(|_| ExecutionError::ResultOutOfRange)?;
                                    value
                                        .to_string()
                                        .parse::<i256>()
                                        .map_err(|_| ExecutionError::ResultOutOfRange)
                                })
                                .transpose()
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Arc::new(
                        Decimal256Array::from(values)
                            .with_precision_and_scale(
                                ty.precision(),
                                i8::try_from(ty.scale())
                                    .map_err(|_| ExecutionError::NumericOverflow)?,
                            )
                            .map_err(|_| ExecutionError::NumericOverflow)?,
                    )
                }
                SelectionItem::TimeDimension(_) => {
                    return Err(ExecutionError::Unsupported("emulated output"));
                }
            };
            fields.push(
                Field::new(&selection.selection_id, array.data_type().clone(), true)
                    .with_metadata(metadata),
            );
            arrays.push(array);
        }
        Ok(vec![
            RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
                .map_err(|_| ExecutionError::Engine)?,
        ])
    }
}

fn scale(ty: &ValueType) -> Result<u8, ExecutionError> {
    match ty {
        ValueType::Decimal(d) => Ok(d.scale()),
        ValueType::Integer { .. } => Ok(0),
        _ => Err(ExecutionError::Unsupported("emulated numeric type")),
    }
}

fn validate(expr: &ScalarExpr) -> Result<(), ExecutionError> {
    scale(&expr.value_type)?;
    match &expr.kind {
        ScalarExprKind::FieldRef(_)
        | ScalarExprKind::Literal(
            Literal::SignedInteger(_)
            | Literal::UnsignedInteger(_)
            | Literal::Decimal(_)
            | Literal::Null(_),
        ) => Ok(()),
        ScalarExprKind::ExplicitCast {
            operand,
            rounding: None,
        } if scale(&expr.value_type)? >= scale(&operand.value_type)? => validate(operand),
        ScalarExprKind::Arithmetic {
            op: ArithmeticOp::Add | ArithmeticOp::Subtract | ArithmeticOp::Multiply,
            left,
            right,
            contract,
        } if contract.rounding.is_none() => {
            validate(left)?;
            validate(right)
        }
        _ => Err(ExecutionError::Unsupported("emulated numeric expression")),
    }
}

fn rescale(value: BigInt, from: u8, to: u8) -> Result<BigInt, ExecutionError> {
    if to >= from {
        Ok(value * BigInt::from(10).pow(u32::from(to - from)))
    } else {
        let divisor = BigInt::from(10).pow(u32::from(from - to));
        if &value % &divisor != BigInt::from(0) {
            return Err(ExecutionError::Unsupported("implicit scale reduction"));
        }
        Ok(value / divisor)
    }
}

fn check_decimal(value: &BigInt, precision: u8) -> Result<(), ExecutionError> {
    if value.to_string().trim_start_matches('-').len() > usize::from(precision) {
        Err(ExecutionError::NumericOverflow)
    } else {
        Ok(())
    }
}

fn evaluate(
    expr: &ScalarExpr,
    batch: &RecordBatch,
    row: usize,
) -> Result<Option<BigInt>, ExecutionError> {
    let value = match &expr.kind {
        ScalarExprKind::FieldRef(id) => {
            let column = batch.column(
                batch
                    .schema()
                    .index_of(id.as_str())
                    .map_err(|_| ExecutionError::Engine)?,
            );
            if column.is_null(row) {
                return Ok(None);
            }
            match column.data_type() {
                DataType::Int64 => BigInt::from(
                    column
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .ok_or(ExecutionError::SourceContract)?
                        .value(row),
                ),
                DataType::Decimal128(_, _) => BigInt::from(
                    column
                        .as_any()
                        .downcast_ref::<Decimal128Array>()
                        .ok_or(ExecutionError::SourceContract)?
                        .value(row),
                ),
                _ => return Err(ExecutionError::Unsupported("emulated source type")),
            }
        }
        ScalarExprKind::Literal(value) => match value {
            Literal::Null(_) => return Ok(None),
            Literal::SignedInteger(v) => BigInt::from(*v),
            Literal::UnsignedInteger(v) => BigInt::from(*v),
            Literal::Decimal(v) => v
                .coefficient()
                .parse()
                .map_err(|_| ExecutionError::NumericOverflow)?,
            _ => return Err(ExecutionError::Unsupported("emulated literal")),
        },
        ScalarExprKind::ExplicitCast { operand, .. } => {
            let Some(value) = evaluate(operand, batch, row)? else {
                return Ok(None);
            };
            rescale(value, scale(&operand.value_type)?, scale(&expr.value_type)?)?
        }
        ScalarExprKind::Arithmetic {
            op, left, right, ..
        } => {
            let (Some(a), Some(b)) = (evaluate(left, batch, row)?, evaluate(right, batch, row)?)
            else {
                return Ok(None);
            };
            let target = scale(&expr.value_type)?;
            match op {
                ArithmeticOp::Multiply => rescale(
                    a * b,
                    scale(&left.value_type)? + scale(&right.value_type)?,
                    target,
                )?,
                ArithmeticOp::Add | ArithmeticOp::Subtract => {
                    let a = rescale(a, scale(&left.value_type)?, target)?;
                    let b = rescale(b, scale(&right.value_type)?, target)?;
                    if *op == ArithmeticOp::Add {
                        a + b
                    } else {
                        a - b
                    }
                }
                ArithmeticOp::Divide => {
                    return Err(ExecutionError::Unsupported("emulated arithmetic"));
                }
            }
        }
        _ => return Err(ExecutionError::Unsupported("emulated numeric expression")),
    };
    match expr.value_type {
        ValueType::Decimal(ty) => check_decimal(&value, ty.precision())?,
        ValueType::Integer {
            signed,
            bits: Some(bits),
        } => {
            let upper = BigInt::from(2).pow(u32::from(bits - u16::from(signed)));
            let lower = if signed { -&upper } else { BigInt::from(0) };
            if value < lower || value >= upper {
                return Err(ExecutionError::NumericOverflow);
            }
        }
        _ => return Err(ExecutionError::Unsupported("emulated numeric type")),
    }
    Ok(Some(value))
}
