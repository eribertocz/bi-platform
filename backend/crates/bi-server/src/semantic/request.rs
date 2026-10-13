//! The wire format of a semantic query: names as the model declares them,
//! never stable IDs, and literals as text. It becomes one unbound SQIR
//! block; binding then resolves it against the model.

use sef_model_ir::expressions::ComparisonOp;
use sef_model_ir::time::TimeGrain;
use sef_query_ir::unbound::{
    UnboundBlock, UnboundDimension, UnboundFilter, UnboundItem, UnboundOrder, UnboundQuerySet,
    UnboundSelection, UnboundTarget, Value as UnboundValue,
};
use serde::Deserialize;
use utoipa::ToSchema;

/// Bounds on what one request may ask, before any semantic work.
pub const MAX_SELECTIONS: usize = 64;
pub const MAX_FILTER_NODES: usize = 256;
pub const MAX_FILTER_DEPTH: usize = 16;
pub const MAX_TEXT: usize = 256;

/// The block's identifier in SQIR: one query is one block.
pub const BLOCK: &str = "query";

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainRequest {
    /// The project file, as `/api/v1/models` lists it (`metrics/ventas.yaml`).
    #[schema(example = "metrics/ventas.yaml")]
    pub model: String,
    pub query: SemanticQuery,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticQuery {
    /// Output columns, in order. The selected dimensions are the grain.
    pub selections: Vec<SemanticSelection>,
    /// Pre-aggregate filter over dimensions.
    #[serde(default)]
    pub filter: Option<SemanticFilter>,
    /// Filter over this query's metric selections (`HAVING`).
    #[serde(default)]
    pub having: Option<SemanticFilter>,
    #[serde(default)]
    pub order_by: Vec<SemanticOrder>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub offset: Option<u64>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SemanticSelection {
    Metric {
        /// Query-local name of the column; ordering and `having` use it.
        id: String,
        /// The metric's name in the model.
        name: String,
        #[serde(default)]
        label: Option<String>,
    },
    Dimension {
        id: String,
        name: String,
        /// Relationship roles it is reached through, when ambiguous.
        #[serde(default)]
        roles: Vec<String>,
        #[serde(default)]
        label: Option<String>,
    },
    TimeDimension {
        id: String,
        name: String,
        grain: SemanticGrain,
        #[serde(default)]
        roles: Vec<String>,
        /// IANA zone overriding the governed one.
        #[serde(default)]
        timezone: Option<String>,
        #[serde(default)]
        calendar: Option<String>,
        #[serde(default)]
        label: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticGrain {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum SemanticFilter {
    Compare {
        target: SemanticTarget,
        comparison: SemanticComparison,
        value: SemanticValue,
    },
    Between {
        target: SemanticTarget,
        low: SemanticValue,
        high: SemanticValue,
    },
    In {
        target: SemanticTarget,
        values: Vec<SemanticValue>,
    },
    IsNull {
        target: SemanticTarget,
    },
    IsNotNull {
        target: SemanticTarget,
    },
    And {
        #[schema(no_recursion)]
        filters: Vec<SemanticFilter>,
    },
    Or {
        #[schema(no_recursion)]
        filters: Vec<SemanticFilter>,
    },
    Not {
        #[schema(no_recursion)]
        filter: Box<SemanticFilter>,
    },
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SemanticTarget {
    Dimension {
        name: String,
        #[serde(default)]
        roles: Vec<String>,
    },
    /// A metric selection of this query, by its `id`.
    Metric { selection: String },
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
pub enum SemanticComparison {
    #[serde(rename = "=")]
    Equal,
    #[serde(rename = "<>")]
    NotEqual,
    #[serde(rename = "<")]
    Less,
    #[serde(rename = "<=")]
    LessOrEqual,
    #[serde(rename = ">")]
    Greater,
    #[serde(rename = ">=")]
    GreaterOrEqual,
}

/// A literal as written; binding types it after its target.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SemanticValue {
    Null,
    Boolean(bool),
    /// Exact decimal text (`"12.50"`), never a binary float.
    Number(String),
    Text(String),
    /// ISO `YYYY-MM-DD`.
    Date(String),
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticOrder {
    /// A selection `id` of this query.
    pub selection: String,
    #[serde(default)]
    pub descending: bool,
    /// Nulls go last unless stated.
    #[serde(default)]
    pub nulls_first: bool,
}

/// Why a request was refused before binding, for people.
pub type Refusal = String;

impl SemanticQuery {
    /// The unbound SQIR this request asks for, within the request bounds.
    ///
    /// # Errors
    /// A request over the bounds, or with an empty or oversized text.
    pub fn unbound(&self) -> Result<UnboundQuerySet, Refusal> {
        if self.selections.is_empty() || self.selections.len() > MAX_SELECTIONS {
            return Err(format!(
                "Una consulta tiene entre 1 y {MAX_SELECTIONS} selecciones."
            ));
        }
        let mut nodes = 0;
        let block = UnboundBlock {
            block_id: BLOCK.to_owned(),
            selections: self
                .selections
                .iter()
                .map(SemanticSelection::unbound)
                .collect::<Result<_, _>>()?,
            filter: self
                .filter
                .as_ref()
                .map(|filter| filter.unbound(0, &mut nodes))
                .transpose()?,
            post_aggregate_filter: self
                .having
                .as_ref()
                .map(|filter| filter.unbound(0, &mut nodes))
                .transpose()?,
            order_by: self
                .order_by
                .iter()
                .map(|order| {
                    Ok(UnboundOrder {
                        selection_id: text(&order.selection)?,
                        descending: order.descending,
                        nulls_first: order.nulls_first,
                    })
                })
                .collect::<Result<_, Refusal>>()?,
            limit: self.limit,
            offset: self.offset,
        };
        if block.order_by.len() > MAX_SELECTIONS {
            return Err(format!(
                "Una consulta ordena por {MAX_SELECTIONS} columnas como máximo."
            ));
        }
        Ok(UnboundQuerySet {
            blocks: vec![block],
        })
    }

    /// Selections that are dimensions, metrics and filter predicates, for
    /// the audit's redacted query shape.
    #[must_use]
    pub fn shape(&self) -> (u32, u32, u32) {
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let metrics = self
            .selections
            .iter()
            .filter(|s| matches!(s, SemanticSelection::Metric { .. }))
            .count();
        let filters = self
            .filter
            .iter()
            .chain(&self.having)
            .map(SemanticFilter::size)
            .sum();
        (
            count(self.selections.len() - metrics),
            count(metrics),
            count(filters),
        )
    }
}

fn text(value: &str) -> Result<String, Refusal> {
    if value.is_empty() || value.chars().count() > MAX_TEXT {
        return Err(format!(
            "Los nombres y valores tienen entre 1 y {MAX_TEXT} caracteres."
        ));
    }
    Ok(value.to_owned())
}

fn optional(value: Option<&String>) -> Result<Option<String>, Refusal> {
    value.map(|value| text(value)).transpose()
}

fn dimension(name: &str, roles: &[String]) -> Result<UnboundDimension, Refusal> {
    Ok(UnboundDimension {
        name: text(name)?,
        roles: roles
            .iter()
            .map(|role| text(role))
            .collect::<Result<_, _>>()?,
    })
}

impl SemanticSelection {
    fn unbound(&self) -> Result<UnboundSelection, Refusal> {
        let (id, item, label) = match self {
            Self::Metric { id, name, label } => (id, UnboundItem::Metric(text(name)?), label),
            Self::Dimension {
                id,
                name,
                roles,
                label,
            } => (id, UnboundItem::Dimension(dimension(name, roles)?), label),
            Self::TimeDimension {
                id,
                name,
                grain,
                roles,
                timezone,
                calendar,
                label,
            } => (
                id,
                UnboundItem::TimeDimension {
                    dimension: dimension(name, roles)?,
                    grain: grain.time_grain(),
                    timezone: optional(timezone.as_ref())?,
                    calendar: optional(calendar.as_ref())?,
                },
                label,
            ),
        };
        Ok(UnboundSelection {
            selection_id: text(id)?,
            item,
            output_label: optional(label.as_ref())?,
        })
    }
}

impl SemanticGrain {
    const fn time_grain(self) -> TimeGrain {
        match self {
            Self::Second => TimeGrain::Second,
            Self::Minute => TimeGrain::Minute,
            Self::Hour => TimeGrain::Hour,
            Self::Day => TimeGrain::Day,
            Self::Week => TimeGrain::Week,
            Self::Month => TimeGrain::Month,
            Self::Quarter => TimeGrain::Quarter,
            Self::Year => TimeGrain::Year,
        }
    }
}

impl SemanticComparison {
    const fn op(self) -> ComparisonOp {
        match self {
            Self::Equal => ComparisonOp::Equal,
            Self::NotEqual => ComparisonOp::NotEqual,
            Self::Less => ComparisonOp::Less,
            Self::LessOrEqual => ComparisonOp::LessOrEqual,
            Self::Greater => ComparisonOp::Greater,
            Self::GreaterOrEqual => ComparisonOp::GreaterOrEqual,
        }
    }
}

impl SemanticTarget {
    fn unbound(&self) -> Result<UnboundTarget, Refusal> {
        Ok(match self {
            Self::Dimension { name, roles } => UnboundTarget::Dimension(dimension(name, roles)?),
            Self::Metric { selection } => UnboundTarget::Metric(text(selection)?),
        })
    }
}

impl SemanticValue {
    fn unbound(&self) -> Result<UnboundValue, Refusal> {
        Ok(match self {
            Self::Null => UnboundValue::Null,
            Self::Boolean(value) => UnboundValue::Boolean(*value),
            Self::Number(value) => UnboundValue::Number(text(value)?),
            Self::Text(value) => UnboundValue::Text(value_text(value)?),
            Self::Date(value) => UnboundValue::Date(text(value)?),
        })
    }
}

/// A text literal may be empty (an empty string is a value), not oversized.
fn value_text(value: &str) -> Result<String, Refusal> {
    if value.chars().count() > MAX_TEXT {
        return Err(format!(
            "Los valores tienen {MAX_TEXT} caracteres como máximo."
        ));
    }
    Ok(value.to_owned())
}

impl SemanticFilter {
    fn unbound(&self, depth: usize, nodes: &mut usize) -> Result<UnboundFilter, Refusal> {
        *nodes += 1;
        if depth > MAX_FILTER_DEPTH || *nodes > MAX_FILTER_NODES {
            return Err(format!(
                "Un filtro tiene como máximo {MAX_FILTER_NODES} condiciones y \
                 {MAX_FILTER_DEPTH} niveles de anidamiento."
            ));
        }
        let all = |filters: &[Self], nodes: &mut usize| {
            if filters.is_empty() {
                return Err("AND y OR necesitan al menos una condición.".to_owned());
            }
            filters
                .iter()
                .map(|filter| filter.unbound(depth + 1, nodes))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(match self {
            Self::Compare {
                target,
                comparison,
                value,
            } => UnboundFilter::Compare {
                target: target.unbound()?,
                op: comparison.op(),
                value: value.unbound()?,
            },
            Self::Between { target, low, high } => UnboundFilter::Between {
                target: target.unbound()?,
                low: low.unbound()?,
                high: high.unbound()?,
            },
            Self::In { target, values } => {
                if values.is_empty() || values.len() > MAX_FILTER_NODES {
                    return Err(format!("IN admite entre 1 y {MAX_FILTER_NODES} valores."));
                }
                UnboundFilter::In {
                    target: target.unbound()?,
                    values: values
                        .iter()
                        .map(SemanticValue::unbound)
                        .collect::<Result<_, _>>()?,
                }
            }
            Self::IsNull { target } => UnboundFilter::IsNull(target.unbound()?),
            Self::IsNotNull { target } => UnboundFilter::IsNotNull(target.unbound()?),
            Self::And { filters } => UnboundFilter::And(all(filters, nodes)?),
            Self::Or { filters } => UnboundFilter::Or(all(filters, nodes)?),
            Self::Not { filter } => UnboundFilter::Not(Box::new(filter.unbound(depth + 1, nodes)?)),
        })
    }

    fn size(&self) -> usize {
        match self {
            Self::And { filters } | Self::Or { filters } => {
                1 + filters.iter().map(Self::size).sum::<usize>()
            }
            Self::Not { filter } => 1 + filter.size(),
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: &serde_json::Value) -> SemanticQuery {
        serde_json::from_value(value.clone()).expect("request format")
    }

    fn metric(id: &str) -> serde_json::Value {
        serde_json::json!({ "kind": "metric", "id": id, "name": "importe" })
    }

    #[test]
    fn a_query_becomes_one_unbound_block() {
        let query = parse(&serde_json::json!({
            "selections": [
                metric("m"),
                { "kind": "time_dimension", "id": "t", "name": "fecha", "grain": "month" }
            ],
            "filter": { "op": "and", "filters": [
                { "op": "compare", "target": { "kind": "dimension", "name": "region" },
                  "comparison": "<>", "value": { "type": "text", "value": "" } },
                { "op": "not", "filter": { "op": "is_null",
                  "target": { "kind": "dimension", "name": "region" } } }
            ]},
            "order_by": [{ "selection": "m", "descending": true }],
            "limit": 10
        }));
        let unbound = query.unbound().expect("valid");
        let [block] = unbound.blocks.as_slice() else {
            panic!("one block");
        };
        assert_eq!(block.block_id, BLOCK);
        assert_eq!(block.selections.len(), 2);
        assert!(matches!(
            block.selections[1].item,
            UnboundItem::TimeDimension {
                grain: TimeGrain::Month,
                ..
            }
        ));
        let Some(UnboundFilter::And(filters)) = &block.filter else {
            panic!("and");
        };
        // An empty text is a value, not a missing one.
        assert!(matches!(
            &filters[0],
            UnboundFilter::Compare { op: ComparisonOp::NotEqual, value: UnboundValue::Text(text), .. }
                if text.is_empty()
        ));
        assert_eq!(block.limit, Some(10));
        assert_eq!(query.shape(), (1, 1, 4));
    }

    #[test]
    fn requests_over_the_bounds_are_refused_before_any_work() {
        let many: Vec<_> = (0..=MAX_SELECTIONS)
            .map(|i| metric(&format!("m{i}")))
            .collect();
        assert!(
            parse(&serde_json::json!({ "selections": many }))
                .unbound()
                .is_err()
        );
        assert!(
            parse(&serde_json::json!({ "selections": [] }))
                .unbound()
                .is_err()
        );
        let long = "x".repeat(MAX_TEXT + 1);
        assert!(
            parse(&serde_json::json!({ "selections": [metric(&long)] }))
                .unbound()
                .is_err()
        );
        assert!(
            parse(&serde_json::json!({ "selections": [metric("")] }))
                .unbound()
                .is_err()
        );
        // Nesting deeper than the bound.
        let mut deep = serde_json::json!({ "op": "is_null",
            "target": { "kind": "dimension", "name": "region" } });
        for _ in 0..=MAX_FILTER_DEPTH {
            deep = serde_json::json!({ "op": "not", "filter": deep });
        }
        assert!(
            parse(&serde_json::json!({ "selections": [metric("m")], "filter": deep }))
                .unbound()
                .is_err()
        );
        // AND, OR and IN need something to combine.
        for empty in [
            serde_json::json!({ "op": "and", "filters": [] }),
            serde_json::json!({ "op": "in", "target": { "kind": "dimension", "name": "r" }, "values": [] }),
        ] {
            assert!(
                parse(&serde_json::json!({ "selections": [metric("m")], "filter": empty }))
                    .unbound()
                    .is_err()
            );
        }
    }

    #[test]
    fn unknown_members_are_refused_by_the_format() {
        for value in [
            serde_json::json!({ "selections": [metric("m")], "roles": ["admin"] }),
            serde_json::json!({ "selections": [{ "kind": "metric", "id": "m", "name": "x", "user": "ana" }] }),
            serde_json::json!({ "selections": [{ "kind": "table", "id": "m", "name": "x" }] }),
        ] {
            assert!(
                serde_json::from_value::<SemanticQuery>(value.clone()).is_err(),
                "{value}"
            );
        }
    }
}
