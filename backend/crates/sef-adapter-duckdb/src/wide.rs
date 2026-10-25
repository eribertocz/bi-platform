//! Exact coefficient aggregation when a semantic decimal exceeds DuckDB's
//! native 38 digits. DuckDB still performs the governed reads and joins;
//! this adapter evaluates arithmetic with integers and emits Decimal256.

use std::cmp::Ordering;
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
use sef_model_ir::time::SortDirection;
use sef_model_ir::types::{ArithmeticOp, Literal, ValueType};
use sef_query_ir::query::{NullOrdering, OrderTerm, RequestedAtom, Selection, SelectionItem};

use super::ExecutionError;

pub(super) struct Aggregation {
    keys: Vec<String>,
    measures: Vec<Measure>,
    metrics: BTreeMap<String, usize>,
    selections: Vec<Selection>,
    /// The semantic order of the output, by selection (#256).
    order: Vec<OrderTerm>,
    /// LIMIT and OFFSET after aggregation and order, once at most (#256).
    window: Option<(Option<u64>, Option<u64>)>,
}

/// One group of the emulation: its key values and its exact sums.
type Group<'a> = (&'a [Option<String>], &'a [Option<BigInt>]);

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
            order: Vec::new(),
            window: None,
        })
    }

    /// Orders the output by its own selections, before any window (#256).
    pub(super) fn order(&mut self, order: &[OrderTerm]) -> Result<(), ExecutionError> {
        if self.selections.is_empty() || !self.order.is_empty() || self.window.is_some() {
            return Err(ExecutionError::Unsupported("ordering emulated output"));
        }
        if order.iter().any(|term| {
            !self
                .selections
                .iter()
                .any(|s| s.selection_id == term.selection_id)
        }) {
            return Err(ExecutionError::UnknownBinding);
        }
        self.order = order.to_vec();
        Ok(())
    }

    /// Cuts the ordered output, after aggregation and order (#256).
    pub(super) fn limit(
        &mut self,
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<(), ExecutionError> {
        if self.selections.is_empty() || self.window.is_some() {
            return Err(ExecutionError::Unsupported("limiting emulated output"));
        }
        self.window = Some((limit, offset));
        Ok(())
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

    /// Aggregates `batches`, `used` bytes of the intermediate budget: every
    /// group is charged to the same budget, so no window lets groups grow
    /// without bound (#256).
    pub(super) fn execute(
        &self,
        batches: &[RecordBatch],
        used: usize,
        limits: &super::ExecutionLimits,
    ) -> Result<Vec<RecordBatch>, ExecutionError> {
        let mut used = used;
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
                    .collect::<Vec<Option<String>>>();
                if !groups.contains_key(&key) {
                    let size = key.iter().flatten().map(String::len).sum::<usize>()
                        + (key.len() + self.measures.len()) * GROUP_SLOT;
                    used = used.checked_add(size).ok_or(ExecutionError::Budget)?;
                    if used > limits.max_intermediate_bytes {
                        return Err(ExecutionError::Budget);
                    }
                }
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
        limits.checkpoint()?;
        self.range(&groups)?;
        let mut rows: Vec<Group<'_>> = groups
            .iter()
            .map(|(key, sums)| (key.as_slice(), sums.as_slice()))
            .collect();
        let keys = self.sort_keys()?;
        arrange(&mut rows, &keys);
        limits.checkpoint()?;
        let (limit, offset) = self.window.unwrap_or((None, None));
        let skip = usize::try_from(offset.unwrap_or(0)).unwrap_or(usize::MAX);
        let take = limit.map_or(usize::MAX, |n| usize::try_from(n).unwrap_or(usize::MAX));
        let rows: Vec<Group<'_>> = rows.into_iter().skip(skip).take(take).collect();
        self.output(&rows)
    }

    /// Every selected sum, in every group, within its certified precision
    /// (#261), whether or not the window keeps the group.
    fn range(
        &self,
        groups: &BTreeMap<Vec<Option<String>>, Vec<Option<BigInt>>>,
    ) -> Result<(), ExecutionError> {
        for selection in &self.selections {
            let SelectionItem::Metric(id) = &selection.item else {
                continue;
            };
            let index = *self
                .metrics
                .get(id.as_str())
                .ok_or(ExecutionError::UnknownBinding)?;
            let ValueType::Decimal(ty) = self.measures[index].value_type else {
                return Err(ExecutionError::SourceContract);
            };
            let bound = super::exact::bound(ty.precision());
            if groups
                .values()
                .filter_map(|sums| sums[index].as_ref())
                .any(|value| !super::exact::within_bound(value, &bound))
            {
                return Err(ExecutionError::ResultOutOfRange);
            }
        }
        Ok(())
    }

    /// Each order term as the column of a group it compares.
    fn sort_keys(&self) -> Result<Vec<SortKey>, ExecutionError> {
        self.order
            .iter()
            .map(|term| {
                let selection = self
                    .selections
                    .iter()
                    .find(|s| s.selection_id == term.selection_id)
                    .ok_or(ExecutionError::UnknownBinding)?;
                let column = match &selection.item {
                    SelectionItem::Dimension(d) => Column::Key(
                        self.keys
                            .iter()
                            .position(|k| k == d.dimension.as_str())
                            .ok_or(ExecutionError::UnknownBinding)?,
                    ),
                    SelectionItem::Metric(id) => Column::Sum(
                        *self
                            .metrics
                            .get(id.as_str())
                            .ok_or(ExecutionError::UnknownBinding)?,
                    ),
                    SelectionItem::TimeDimension(_) => {
                        return Err(ExecutionError::Unsupported("emulated output"));
                    }
                };
                Ok(SortKey {
                    column,
                    descending: term.direction == SortDirection::Descending,
                    nulls_first: term.nulls == NullOrdering::First,
                })
            })
            .collect()
    }

    fn output(&self, groups: &[Group<'_>]) -> Result<Vec<RecordBatch>, ExecutionError> {
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
                            .iter()
                            .map(|(k, _)| k[index].as_deref())
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
                    // Every sum is within its precision (`range`).
                    let values = groups
                        .iter()
                        .map(|(_, sums)| {
                            sums[index]
                                .as_ref()
                                .map(|value| {
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

/// What one group costs the intermediate budget per key and sum, beyond
/// the text of its keys: a conservative size of an owned value.
const GROUP_SLOT: usize = 64;

/// The column of a group an order term compares.
#[derive(Clone, Copy)]
enum Column {
    Key(usize),
    Sum(usize),
}

#[derive(Clone, Copy)]
struct SortKey {
    column: Column,
    descending: bool,
    nulls_first: bool,
}

/// `rows` in the semantic order of `keys`: keys as text by their bytes, as
/// DuckDB's binary collation orders them, sums as exact integers, NULL
/// first or last as requested in either direction. The sort is stable, so
/// groups tied on every term keep the order of their keys (#256).
fn arrange(rows: &mut [Group<'_>], keys: &[SortKey]) {
    rows.sort_by(|a, b| {
        keys.iter()
            .map(|key| match key.column {
                Column::Key(i) => compare(a.0[i].as_ref(), b.0[i].as_ref(), *key),
                Column::Sum(i) => compare(a.1[i].as_ref(), b.1[i].as_ref(), *key),
            })
            .find(|order| order.is_ne())
            .unwrap_or(Ordering::Equal)
    });
}

fn compare<T: Ord>(a: Option<&T>, b: Option<&T>, key: SortKey) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) if key.nulls_first => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) if key.nulls_first => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) if key.descending => b.cmp(a),
        (Some(a), Some(b)) => a.cmp(b),
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
    if super::exact::within(value, precision) {
        Ok(())
    } else {
        Err(ExecutionError::NumericOverflow)
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

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A group's keys and sums, owned by the test.
    type Owned = (Vec<Option<String>>, Vec<Option<BigInt>>);

    fn key(column: Column, descending: bool, nulls_first: bool) -> SortKey {
        SortKey {
            column,
            descending,
            nulls_first,
        }
    }

    proptest! {
        /// The semantic order of groups equals an independent oracle: rank
        /// of NULL, then the value as an i128 (or its negation descending),
        /// then the text of the key, NULL ties kept stable (#256).
        #[test]
        fn groups_are_arranged_as_the_oracle_orders_them(
            groups in prop::collection::btree_map(
                prop::option::of("[a-c]{0,2}"),
                prop::option::of(prop_oneof![
                    -1_000_i128..1_000,
                    // Integers past 2^53 that differ in their last digit.
                    Just(9_007_199_254_740_993_i128),
                    Just(9_007_199_254_740_992_i128),
                    Just(-9_007_199_254_740_993_i128),
                    Just(i128::MAX / 3),
                ]),
                0..12,
            ),
            descending in any::<bool>(),
            nulls_first in any::<bool>(),
            by_key in any::<bool>(),
        ) {
            let owned: Vec<Owned> = groups
                .iter()
                .map(|(k, v)| (vec![k.clone()], vec![v.map(BigInt::from)]))
                .collect();
            let mut rows: Vec<Group<'_>> =
                owned.iter().map(|(k, s)| (k.as_slice(), s.as_slice())).collect();
            let column = if by_key { Column::Key(0) } else { Column::Sum(0) };
            arrange(&mut rows, &[key(column, descending, nulls_first)]);
            let got: Vec<Option<String>> = rows.iter().map(|(k, _)| k[0].clone()).collect();

            let mut oracle: Vec<(Option<String>, Option<i128>)> =
                groups.into_iter().collect();
            let null_rank = |is_null: bool| u8::from(is_null != nulls_first);
            oracle.sort_by(|(ka, va), (kb, vb)| {
                let rank = |k: &Option<String>, v: &Option<i128>| {
                    if by_key { null_rank(k.is_none()) } else { null_rank(v.is_none()) }
                };
                rank(ka, va).cmp(&rank(kb, vb)).then_with(|| {
                    let order = if by_key {
                        ka.as_ref().map(String::as_bytes).cmp(&kb.as_ref().map(String::as_bytes))
                    } else {
                        va.cmp(vb)
                    };
                    if descending { order.reverse() } else { order }
                })
                .then_with(|| ka.cmp(kb))
            });
            let expected: Vec<Option<String>> = oracle.into_iter().map(|(k, _)| k).collect();
            prop_assert_eq!(got, expected);
        }
    }

    #[test]
    fn ties_follow_the_next_term_and_nulls_their_requested_place() {
        let owned: Vec<Owned> = vec![
            (vec![Some("Norte".into())], vec![Some(BigInt::from(5))]),
            (vec![Some("Sur".into())], vec![Some(BigInt::from(5))]),
            (vec![None], vec![None]),
            (vec![Some("Este".into())], vec![Some(BigInt::from(-7))]),
        ];
        let mut rows: Vec<Group<'_>> = owned
            .iter()
            .map(|(k, s)| (k.as_slice(), s.as_slice()))
            .collect();
        // Sum descending NULLS LAST, then key descending: the tie on 5 is
        // broken by the second term.
        arrange(
            &mut rows,
            &[
                key(Column::Sum(0), true, false),
                key(Column::Key(0), true, false),
            ],
        );
        let order: Vec<Option<&str>> = rows.iter().map(|(k, _)| k[0].as_deref()).collect();
        assert_eq!(order, [Some("Sur"), Some("Norte"), Some("Este"), None]);
    }
}
