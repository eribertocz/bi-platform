//! Binding (§14): an [`UnboundQuerySet`] against one CSMIR model becomes
//! SQIR, or every problem of the request is reported at once.
//!
//! - Names resolve to exactly one metric or dimension: by stable ID, by
//!   label (the name as written, case-insensitively) or by the last segment
//!   of the stable ID. No match is `unknown_*`; several are ambiguous.
//! - Roles must be declared by some relationship. Whether a role makes a
//!   path unambiguous is the Correctness Engine's question, not binding's.
//! - Literals take their target's exact type when that loses nothing
//!   (`5` → `DECIMAL(18,2)` 5.00); otherwise binding fails. `NULL` is only
//!   reachable through `IS [NOT] NULL`.

use std::collections::BTreeSet;

use sef_model_ir::identity::StableId;
use sef_model_ir::model::{ModelDefinition, ObjectMetadata};
use sef_model_ir::time::{IanaZoneId, SortDirection, iso_date_days};
use sef_model_ir::types::{DecimalType, DecimalValue, FiniteFloat, Literal, ValueType};

use crate::error::{QueryError, QueryErrorCode as Code};
use crate::query::{
    FilterExpr, FilterTarget, NullOrdering, OrderTerm, QualifiedDimensionRef, Selection,
    SelectionItem, SemanticQueryBlock, SemanticQuerySet, TimeDimensionSelection,
};
use crate::unbound::{
    UnboundBlock, UnboundDimension, UnboundFilter, UnboundItem, UnboundQuerySet, UnboundTarget,
    Value,
};

/// Binds a request against a (compiled) model definition.
///
/// # Errors
/// Every binding problem found, each with its JSON pointer.
pub fn bind(
    query: &UnboundQuerySet,
    model: &ModelDefinition,
) -> Result<SemanticQuerySet, Vec<QueryError>> {
    bind_visible(query, model, &|_| true)
}

/// Binds a request seeing only the metrics and dimensions `visible` admits:
/// names resolve among them alone, so an object the caller may not see
/// never makes a name ambiguous nor answers for it. The caller decides
/// visibility; binding stays free of any permission model.
///
/// # Errors
/// Every binding problem found, each with its JSON pointer.
pub fn bind_visible(
    query: &UnboundQuerySet,
    model: &ModelDefinition,
    visible: &dyn Fn(&StableId) -> bool,
) -> Result<SemanticQuerySet, Vec<QueryError>> {
    let mut binder = Binder {
        model,
        visible,
        errors: Vec::new(),
        roles: model
            .relationships
            .iter()
            .map(|relationship| relationship.role.as_str())
            .collect(),
    };
    if query.blocks.is_empty() {
        binder.error(Code::EmptyQuery, "/blocks", "La consulta no tiene bloques.");
    }
    let mut block_ids = BTreeSet::new();
    let blocks: Vec<SemanticQueryBlock> = query
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(b, block)| {
            let path = format!("/blocks/{b}");
            if block.block_id.is_empty() || !block_ids.insert(block.block_id.as_str()) {
                binder.error(
                    Code::DuplicateBlock,
                    format!("{path}/block_id"),
                    format!(
                        "Cada bloque necesita un block_id propio y no vacío ({:?}).",
                        block.block_id
                    ),
                );
            }
            binder.block(block, &path)
        })
        .collect();
    if binder.errors.is_empty() {
        Ok(SemanticQuerySet { blocks })
    } else {
        Err(binder.errors)
    }
}

struct Binder<'m> {
    model: &'m ModelDefinition,
    visible: &'m dyn Fn(&StableId) -> bool,
    errors: Vec<QueryError>,
    roles: BTreeSet<&'m str>,
}

/// The objects a name designates (§14.3): an exact stable ID wins outright
/// and never competes with labels; only a name that is no ID is looked up
/// by label (case-insensitive) or by the ID's final segment.
fn resolve<'m>(
    objects: impl Iterator<Item = &'m ObjectMetadata> + Clone,
    name: &str,
) -> BTreeSet<&'m StableId> {
    if let Some(exact) = objects
        .clone()
        .find(|metadata| metadata.id().as_str() == name)
    {
        return BTreeSet::from([exact.id()]);
    }
    let lowered = name.to_lowercase();
    objects
        .filter(|metadata| {
            metadata
                .label
                .as_deref()
                .is_some_and(|label| label.to_lowercase() == lowered)
                || metadata.id().as_str().rsplit('/').next() == Some(lowered.as_str())
        })
        .map(ObjectMetadata::id)
        .collect()
}

impl Binder<'_> {
    fn error(&mut self, code: Code, path: impl Into<String>, message: impl Into<String>) {
        self.errors.push(QueryError::new(code, path, message));
    }

    fn block(&mut self, block: &UnboundBlock, path: &str) -> Option<SemanticQueryBlock> {
        let before = self.errors.len();
        let mut ids = BTreeSet::new();
        let mut selections = Vec::with_capacity(block.selections.len());
        for (s, selection) in block.selections.iter().enumerate() {
            let selection_path = format!("{path}/selections/{s}");
            if selection.selection_id.is_empty() || !ids.insert(selection.selection_id.as_str()) {
                self.error(
                    Code::DuplicateSelection,
                    format!("{selection_path}/selection_id"),
                    format!(
                        "Cada selección necesita un selection_id propio y no vacío ({:?}).",
                        selection.selection_id
                    ),
                );
            }
            if let Some(item) = self.item(&selection.item, &selection_path) {
                selections.push(Selection {
                    selection_id: selection.selection_id.clone(),
                    item,
                    output_label: selection.output_label.clone(),
                });
            }
        }
        let partial = SemanticQueryBlock {
            block_id: block.block_id.clone(),
            selections,
            filter: None,
            post_aggregate_filter: None,
            order_by: Vec::new(),
            limit: block.limit,
            offset: block.offset,
        };
        if !partial
            .selections
            .iter()
            .any(|selection| matches!(selection.item, SelectionItem::Metric(_)))
            && !block
                .selections
                .iter()
                .any(|selection| matches!(selection.item, UnboundItem::Metric(_)))
        {
            self.error(
                Code::NoMetric,
                format!("{path}/selections"),
                "Cada bloque selecciona al menos una métrica; las dimensiones solas no son una \
                 consulta semántica.",
            );
        }
        let filter = block
            .filter
            .as_ref()
            .and_then(|filter| self.filter(filter, &format!("{path}/filter"), &partial, false));
        let post_aggregate_filter = block.post_aggregate_filter.as_ref().and_then(|filter| {
            self.filter(
                filter,
                &format!("{path}/post_aggregate_filter"),
                &partial,
                true,
            )
        });
        let order_by = self.ordering(block, path, &ids);
        (self.errors.len() == before).then_some(SemanticQueryBlock {
            filter,
            post_aggregate_filter,
            order_by,
            ..partial
        })
    }

    /// Ordering by selections of the block, and pagination that never
    /// depends on an engine's row order.
    fn ordering(
        &mut self,
        block: &UnboundBlock,
        path: &str,
        ids: &BTreeSet<&str>,
    ) -> Vec<OrderTerm> {
        let mut order_by = Vec::with_capacity(block.order_by.len());
        for (o, order) in block.order_by.iter().enumerate() {
            if !ids.contains(order.selection_id.as_str()) {
                self.error(
                    Code::UnknownSelection,
                    format!("{path}/order_by/{o}/selection_id"),
                    format!(
                        "El orden nombra la selección {:?}, que no está en este bloque.",
                        order.selection_id
                    ),
                );
                continue;
            }
            order_by.push(OrderTerm {
                selection_id: order.selection_id.clone(),
                direction: if order.descending {
                    SortDirection::Descending
                } else {
                    SortDirection::Ascending
                },
                nulls: if order.nulls_first {
                    NullOrdering::First
                } else {
                    NullOrdering::Last
                },
            });
        }
        if block.offset.is_some() && block.limit.is_none() {
            self.error(
                Code::InvalidPagination,
                format!("{path}/offset"),
                "offset necesita limit.",
            );
        }
        if (block.limit.is_some() || block.offset.is_some()) && block.order_by.is_empty() {
            self.error(
                Code::InvalidPagination,
                format!("{path}/limit"),
                "Elegir un subconjunto de filas exige un orden explícito: sin él, cada motor \
                 devolvería filas distintas.",
            );
        }
        order_by
    }

    fn item(&mut self, item: &UnboundItem, path: &str) -> Option<SelectionItem> {
        match item {
            UnboundItem::Metric(name) => self.metric(name, path).map(SelectionItem::Metric),
            UnboundItem::Dimension(dimension) => self
                .dimension(dimension, path)
                .map(SelectionItem::Dimension),
            UnboundItem::TimeDimension {
                dimension,
                grain,
                timezone,
                calendar,
            } => {
                let dimension = self.dimension(dimension, path)?;
                let temporal = self
                    .model
                    .dimensions
                    .iter()
                    .find(|candidate| candidate.metadata.id() == &dimension.dimension)
                    .is_some_and(|candidate| {
                        matches!(
                            candidate.value_type,
                            ValueType::Date | ValueType::Timestamp { .. }
                        )
                    });
                if !temporal {
                    self.error(
                        Code::NotTemporal,
                        path,
                        "Solo una fecha o una fecha y hora se agrupan por periodos.",
                    );
                    return None;
                }
                let timezone = match timezone {
                    Some(zone) => {
                        let valid = IanaZoneId::new(zone.clone()).ok();
                        if valid.is_none() {
                            self.error(
                                Code::InvalidLiteral,
                                format!("{path}/timezone"),
                                format!("{zone:?} no es una zona horaria IANA."),
                            );
                            return None;
                        }
                        valid
                    }
                    None => None,
                };
                let calendar = match calendar {
                    // A label two calendars share is ambiguous: their weeks
                    // differ, so picking one would change the answer.
                    Some(name) => Some(
                        self.one(
                            resolve(
                                self.model
                                    .calendars
                                    .iter()
                                    .map(|calendar| &calendar.metadata),
                                name,
                            ),
                            name,
                            &format!("{path}/calendar"),
                            Code::InvalidLiteral,
                            "el calendario",
                        )?,
                    ),
                    None => None,
                };
                Some(SelectionItem::TimeDimension(TimeDimensionSelection {
                    dimension,
                    grain: *grain,
                    timezone,
                    calendar,
                }))
            }
        }
    }

    fn metric(&mut self, name: &str, path: &str) -> Option<StableId> {
        let visible = self.visible;
        let found = resolve(
            self.model
                .metrics
                .iter()
                .map(|metric| &metric.metadata)
                .filter(|metadata| visible(metadata.id())),
            name,
        );
        self.one(found, name, path, Code::UnknownMetric, "la métrica")
    }

    fn dimension(
        &mut self,
        dimension: &UnboundDimension,
        path: &str,
    ) -> Option<QualifiedDimensionRef> {
        let visible = self.visible;
        let found = resolve(
            self.model
                .dimensions
                .iter()
                .map(|candidate| &candidate.metadata)
                .filter(|metadata| visible(metadata.id())),
            &dimension.name,
        );
        let id = self.one(
            found,
            &dimension.name,
            path,
            Code::UnknownDimension,
            "la dimensión",
        )?;
        let mut roles = Vec::with_capacity(dimension.roles.len());
        for (r, role) in dimension.roles.iter().enumerate() {
            let lowered = role.to_lowercase();
            let Some(known) = self
                .roles
                .iter()
                .find(|known| known.to_lowercase() == lowered)
                .map(|known| (*known).to_owned())
            else {
                self.error(
                    Code::UnknownRole,
                    format!("{path}/roles/{r}"),
                    format!("Ninguna relación del modelo tiene el rol {role:?}."),
                );
                return None;
            };
            roles.push(known);
        }
        roles.sort();
        roles.dedup();
        Some(QualifiedDimensionRef {
            dimension: id,
            roles,
        })
    }

    fn one(
        &mut self,
        found: BTreeSet<&StableId>,
        name: &str,
        path: &str,
        unknown: Code,
        what: &str,
    ) -> Option<StableId> {
        let mut found = found.into_iter();
        match (found.next(), found.next()) {
            (Some(id), None) => Some(id.clone()),
            (None, _) => {
                self.error(
                    unknown,
                    path,
                    format!("El modelo no tiene {what} {name:?}."),
                );
                None
            }
            (Some(first), Some(second)) => {
                self.error(
                    Code::AmbiguousName,
                    path,
                    format!(
                        "{name:?} nombra más de un objeto ({first}, {second}…); usa su \
                         identificador estable."
                    ),
                );
                None
            }
        }
    }

    fn filter(
        &mut self,
        filter: &UnboundFilter,
        path: &str,
        block: &SemanticQueryBlock,
        post_aggregate: bool,
    ) -> Option<FilterExpr> {
        let bound = self.filter_expr(filter, path, block, post_aggregate)?;
        Some(bound.normalized())
    }

    fn filter_expr(
        &mut self,
        filter: &UnboundFilter,
        path: &str,
        block: &SemanticQueryBlock,
        post_aggregate: bool,
    ) -> Option<FilterExpr> {
        let many = |binder: &mut Self, operands: &[UnboundFilter]| {
            let bound: Vec<Option<FilterExpr>> = operands
                .iter()
                .enumerate()
                .map(|(i, operand)| {
                    binder.filter_expr(operand, &format!("{path}/{i}"), block, post_aggregate)
                })
                .collect();
            bound.into_iter().collect::<Option<Vec<_>>>()
        };
        match filter {
            UnboundFilter::And(operands) => many(self, operands).map(FilterExpr::And),
            UnboundFilter::Or(operands) => many(self, operands).map(FilterExpr::Or),
            UnboundFilter::Not(operand) => self
                .filter_expr(operand, &format!("{path}/not"), block, post_aggregate)
                .map(|operand| FilterExpr::Not(Box::new(operand))),
            UnboundFilter::IsNull(target) => self
                .target(target, path, block, post_aggregate)
                .map(|(target, _)| FilterExpr::IsNull(target)),
            UnboundFilter::IsNotNull(target) => self
                .target(target, path, block, post_aggregate)
                .map(|(target, _)| FilterExpr::IsNotNull(target)),
            UnboundFilter::Compare { target, op, value } => {
                let (target, value_type) = self.target(target, path, block, post_aggregate)?;
                let value = self.literal(value, &value_type, &format!("{path}/value"))?;
                Some(FilterExpr::Compare {
                    target,
                    op: *op,
                    value,
                })
            }
            UnboundFilter::Between { target, low, high } => {
                let (target, value_type) = self.target(target, path, block, post_aggregate)?;
                let low = self.literal(low, &value_type, &format!("{path}/low"));
                let high = self.literal(high, &value_type, &format!("{path}/high"));
                Some(FilterExpr::Between {
                    target,
                    low: low?,
                    high: high?,
                })
            }
            UnboundFilter::In { target, values } => {
                let (target, value_type) = self.target(target, path, block, post_aggregate)?;
                if values.is_empty() {
                    self.error(
                        Code::EmptyList,
                        format!("{path}/values"),
                        "IN necesita al menos un valor.",
                    );
                    return None;
                }
                let bound: Vec<Option<Literal>> = values
                    .iter()
                    .enumerate()
                    .map(|(v, value)| {
                        self.literal(value, &value_type, &format!("{path}/values/{v}"))
                    })
                    .collect();
                Some(FilterExpr::In {
                    target,
                    values: bound.into_iter().collect::<Option<Vec<_>>>()?,
                })
            }
        }
    }

    /// The filter's target and the type its literals take.
    fn target(
        &mut self,
        target: &UnboundTarget,
        path: &str,
        block: &SemanticQueryBlock,
        post_aggregate: bool,
    ) -> Option<(FilterTarget, ValueType)> {
        match (target, post_aggregate) {
            (UnboundTarget::Dimension(dimension), false) => {
                let dimension = self.dimension(dimension, &format!("{path}/target"))?;
                let value_type = self
                    .model
                    .dimensions
                    .iter()
                    .find(|candidate| candidate.metadata.id() == &dimension.dimension)?
                    .value_type
                    .clone();
                Some((FilterTarget::Dimension(dimension), value_type))
            }
            (UnboundTarget::Metric(selection_id), true) => {
                let Some(metric) = block.metric_of(selection_id) else {
                    self.error(
                        Code::InvalidFilterTarget,
                        format!("{path}/target"),
                        format!("{selection_id:?} no es una métrica seleccionada en este bloque."),
                    );
                    return None;
                };
                let value_type = self
                    .model
                    .metrics
                    .iter()
                    .find(|candidate| candidate.metadata.id() == metric)?
                    .value_type
                    .clone();
                Some((FilterTarget::Metric(selection_id.clone()), value_type))
            }
            (UnboundTarget::Metric(_), false) => {
                self.error(
                    Code::InvalidFilterTarget,
                    format!("{path}/target"),
                    "Un filtro antes de agregar se aplica a dimensiones; las métricas se \
                     filtran después (post_aggregate_filter).",
                );
                None
            }
            (UnboundTarget::Dimension(_), true) => {
                self.error(
                    Code::InvalidFilterTarget,
                    format!("{path}/target"),
                    "Un filtro después de agregar se aplica a métricas seleccionadas; las \
                     dimensiones se filtran antes (filter).",
                );
                None
            }
        }
    }

    fn literal(&mut self, value: &Value, value_type: &ValueType, path: &str) -> Option<Literal> {
        match typed(value, value_type) {
            Ok(literal) => Some(literal),
            Err((code, message)) => {
                self.error(code, path, message);
                None
            }
        }
    }
}

/// A literal at its target's exact type, or why it cannot be.
fn typed(value: &Value, value_type: &ValueType) -> Result<Literal, (Code, String)> {
    let invalid = |message: String| (Code::InvalidLiteral, message);
    match (value, value_type) {
        (Value::Null, _) => Err((
            Code::NullComparison,
            "NULL no se compara: usa IS NULL o IS NOT NULL.".to_owned(),
        )),
        (Value::Boolean(value), ValueType::Boolean) => Ok(Literal::Boolean(*value)),
        (Value::Text(text), ValueType::Utf8) => Ok(Literal::Utf8(text.clone())),
        (Value::Date(text) | Value::Text(text), ValueType::Date) => iso_date_days(text)
            .map(|days| Literal::Date {
                days_since_unix_epoch: days,
            })
            .ok_or_else(|| invalid(format!("{text:?} no es una fecha AAAA-MM-DD válida."))),
        (Value::Number(text), ValueType::Integer { signed, bits }) => {
            integer(text, *signed, bits.unwrap_or(64)).map_err(invalid)
        }
        (Value::Number(text), ValueType::Decimal(decimal)) => {
            decimal_at(text, *decimal).map_err(invalid)
        }
        (Value::Number(text), ValueType::Float { bits }) => float(text, *bits).map_err(invalid),
        (_, ValueType::Timestamp { .. }) => Err(invalid(
            "Los filtros por fecha y hora llegarán con su contrato de zona horaria; filtra por \
             una dimensión de fecha."
                .to_owned(),
        )),
        (value, value_type) => Err(invalid(format!(
            "El valor {value:?} no es del tipo de su destino ({value_type:?})."
        ))),
    }
}

/// `(negative, integer digits, fraction digits)` of an exact decimal text.
fn split_number(text: &str) -> Option<(bool, &str, &str)> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (integer, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let valid = !(integer.is_empty() && fraction.is_empty())
        && integer
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit());
    valid.then_some((negative, integer, fraction))
}

fn integer(text: &str, signed: bool, bits: u16) -> Result<Literal, String> {
    let (negative, integer, fraction) =
        split_number(text).ok_or_else(|| format!("{text:?} no es un número."))?;
    if fraction.bytes().any(|b| b != b'0') {
        return Err(format!("{text} tiene decimales y su destino es entero."));
    }
    let digits = if integer.is_empty() { "0" } else { integer };
    let out_of_range = || format!("{text} no cabe en un entero de {bits} bits.");
    if signed {
        let value: i128 = format!("{}{digits}", if negative { "-" } else { "" })
            .parse()
            .map_err(|_| out_of_range())?;
        if bits < 128 {
            let limit = 1_i128 << (bits - 1);
            if value < -limit || value >= limit {
                return Err(out_of_range());
            }
        }
        Ok(Literal::SignedInteger(value))
    } else {
        if negative && digits.bytes().any(|b| b != b'0') {
            return Err(out_of_range());
        }
        let value: u128 = digits.parse().map_err(|_| out_of_range())?;
        if bits < 128 && value >> bits != 0 {
            return Err(out_of_range());
        }
        Ok(Literal::UnsignedInteger(value))
    }
}

/// A literal compared with a float column means the value of the column's
/// width nearest to it, as `CAST(literal AS FLOAT)` would: rounded to the
/// nearest of that width. A value that overflows to infinity, or a non-zero
/// value that underflows to zero, would change the comparison and is
/// refused; NaN and infinities are never literals.
fn float(text: &str, bits: u16) -> Result<Literal, String> {
    let value = text
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("{text:?} no es un número finito."))?;
    let value = if bits == 32 {
        #[allow(clippy::cast_possible_truncation)] // the narrowing is the point
        let narrowed = value as f32;
        if narrowed.is_infinite() {
            return Err(format!(
                "{text} desborda un número de coma flotante de 32 bits."
            ));
        }
        if narrowed == 0.0 && value != 0.0 {
            return Err(format!(
                "{text} es demasiado pequeño para un número de 32 bits: se volvería 0."
            ));
        }
        f64::from(narrowed)
    } else {
        value
    };
    FiniteFloat::new(value)
        .map(Literal::Float)
        .map_err(|_| format!("{text:?} no es un número finito."))
}

fn decimal_at(text: &str, decimal: DecimalType) -> Result<Literal, String> {
    let (negative, integer, fraction) =
        split_number(text).ok_or_else(|| format!("{text:?} no es un número."))?;
    let scale = usize::from(decimal.scale());
    if fraction.trim_end_matches('0').len() > scale {
        return Err(format!(
            "{text} tiene más decimales que su destino (escala {scale}): se perdería precisión."
        ));
    }
    let fraction = &fraction[..fraction.len().min(scale)];
    let coefficient = format!(
        "{}{integer}{fraction:0<scale$}",
        if negative { "-" } else { "" }
    );
    DecimalValue::new(coefficient, decimal)
        .map(Literal::Decimal)
        .map_err(|_| {
            format!(
                "{text} no cabe en DECIMAL({}, {}).",
                decimal.precision(),
                decimal.scale()
            )
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::{Literal, float, integer};

    fn widths() -> impl Iterator<Item = u16> {
        [8_u16, 16, 32, 64, 128].into_iter()
    }

    /// Every width and sign accepts exactly its range, 128 bits included:
    /// the extremes bind to the same value, one step beyond is refused.
    #[test]
    fn integers_bind_exactly_the_declared_range() {
        for bits in widths() {
            let (min, max) = if bits == 128 {
                (i128::MIN, i128::MAX)
            } else {
                (-(1_i128 << (bits - 1)), (1_i128 << (bits - 1)) - 1)
            };
            for value in [min, -1, 0, max] {
                assert_eq!(
                    integer(&value.to_string(), true, bits),
                    Ok(Literal::SignedInteger(value)),
                    "INT{bits} {value}"
                );
            }
            let umax = if bits == 128 {
                u128::MAX
            } else {
                (1_u128 << bits) - 1
            };
            for value in [0, umax] {
                assert_eq!(
                    integer(&value.to_string(), false, bits),
                    Ok(Literal::UnsignedInteger(value)),
                    "UINT{bits} {value}"
                );
            }
            if bits < 128 {
                for outside in [min - 1, max + 1] {
                    assert!(
                        integer(&outside.to_string(), true, bits).is_err(),
                        "INT{bits} {outside}"
                    );
                }
                assert!(
                    integer(&(umax + 1).to_string(), false, bits).is_err(),
                    "UINT{bits}"
                );
            }
            assert!(integer("-1", false, bits).is_err(), "UINT{bits} -1");
        }
        // The reviewer's values fit their 128-bit destinations exactly.
        assert_eq!(
            integer("9223372036854775808", true, 128),
            Ok(Literal::SignedInteger(1_i128 << 63))
        );
        assert_eq!(
            integer("18446744073709551616", false, 128),
            Ok(Literal::UnsignedInteger(1_u128 << 64))
        );
        // One beyond each 128-bit extreme does not parse into the range.
        assert!(integer("170141183460469231731687563168715884728", true, 128).is_err());
        assert!(integer("-170141183460469231731687563168715884729", true, 128).is_err());
        assert!(integer("340282366920938463463374607431768211456", false, 128).is_err());
    }

    #[test]
    fn a_float32_literal_is_its_nearest_float32_or_refused() {
        // Rounded as `CAST(0.1 AS FLOAT)`: the column's own value.
        let Ok(Literal::Float(value)) = float("0.1", 32) else {
            panic!("0.1 is a float32");
        };
        assert_eq!(value.value(), f64::from(0.1_f32));
        let Ok(Literal::Float(value)) = float("0.1", 64) else {
            panic!("0.1 is a float64");
        };
        assert!((value.value() - 0.1).abs() < f64::EPSILON);
        let largest = f64::from(f32::MAX).to_string();
        assert!(float(&largest, 32).is_ok(), "FLOAT32 max");
        for refused in ["1e100", "-1e100", "1e-50", "NaN", "inf", "-inf"] {
            assert!(float(refused, 32).is_err(), "FLOAT32 {refused}");
        }
        assert!(float("1e100", 64).is_ok(), "FLOAT64 holds 1e100");
        assert!(float("0", 32).is_ok() && float("-0.0", 32).is_ok());
        for refused in ["NaN", "inf", "1e400"] {
            assert!(float(refused, 64).is_err(), "FLOAT64 {refused}");
        }
    }
}
