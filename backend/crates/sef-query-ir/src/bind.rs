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
    let mut binder = Binder {
        model,
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
    errors: Vec<QueryError>,
    roles: BTreeSet<&'m str>,
}

/// A name's match against one object.
fn matches(metadata: &ObjectMetadata, name: &str) -> bool {
    let id = metadata.id().as_str();
    let lowered = name.to_lowercase();
    id == name
        || metadata
            .label
            .as_deref()
            .is_some_and(|label| label.to_lowercase() == lowered)
        || id.rsplit('/').next() == Some(lowered.as_str())
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
                    Some(name) => {
                        let found = self
                            .model
                            .calendars
                            .iter()
                            .find(|candidate| matches(&candidate.metadata, name))
                            .map(|candidate| candidate.metadata.id().clone());
                        if found.is_none() {
                            self.error(
                                Code::InvalidLiteral,
                                format!("{path}/calendar"),
                                format!("El modelo no declara el calendario {name:?}."),
                            );
                            return None;
                        }
                        found
                    }
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
        let found: BTreeSet<&StableId> = self
            .model
            .metrics
            .iter()
            .filter(|metric| matches(&metric.metadata, name))
            .map(|metric| metric.metadata.id())
            .collect();
        self.one(found, name, path, Code::UnknownMetric, "métrica")
    }

    fn dimension(
        &mut self,
        dimension: &UnboundDimension,
        path: &str,
    ) -> Option<QualifiedDimensionRef> {
        let found: BTreeSet<&StableId> = self
            .model
            .dimensions
            .iter()
            .filter(|candidate| matches(&candidate.metadata, &dimension.name))
            .map(|candidate| candidate.metadata.id())
            .collect();
        let id = self.one(
            found,
            &dimension.name,
            path,
            Code::UnknownDimension,
            "dimensión",
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
                    format!("El modelo no tiene la {what} {name:?}."),
                );
                None
            }
            (Some(first), Some(second)) => {
                self.error(
                    Code::AmbiguousName,
                    path,
                    format!(
                        "{name:?} nombra varias {what}s ({first}, {second}…); usa su \
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
        (Value::Number(text), ValueType::Float { .. }) => text
            .parse::<f64>()
            .ok()
            .and_then(|value| FiniteFloat::new(value).ok())
            .map(Literal::Float)
            .ok_or_else(|| invalid(format!("{text:?} no es un número finito."))),
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
        let limit = 1_i128 << (bits.min(64) - 1);
        if value < -limit || value >= limit {
            return Err(out_of_range());
        }
        i64::try_from(value)
            .map(Literal::SignedInteger)
            .map_err(|_| out_of_range())
    } else {
        if negative && digits.bytes().any(|b| b != b'0') {
            return Err(out_of_range());
        }
        let value: u128 = digits.parse().map_err(|_| out_of_range())?;
        if bits < 128 && value >> bits.min(64) != 0 {
            return Err(out_of_range());
        }
        u64::try_from(value)
            .map(Literal::UnsignedInteger)
            .map_err(|_| out_of_range())
    }
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
