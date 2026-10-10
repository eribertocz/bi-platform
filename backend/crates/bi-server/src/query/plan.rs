//! Validates a [`QueryRequest`] against the catalog and compiles it into
//! parameterized SQL. Identifiers and expressions come only from the
//! catalog; request values are bound as parameters.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::catalog::{self, ColumnType, Dataset, Field};

pub const DEFAULT_LIMIT: u32 = 1_000;
pub const MAX_LIMIT: u32 = 10_000;
pub const MAX_DIMENSIONS: usize = 2;
pub const MAX_MEASURES: usize = 8;
pub const MAX_FILTER_VALUES: usize = 100;

/// An aggregate query over an allowlisted dataset. No SQL is accepted.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(example = json!({
    "dataset": "sales",
    "dimensions": ["region"],
    "measures": ["row_count", "amount_sum"],
    "filters": [{"field": "region", "op": "in", "values": ["north", null]}],
    "sort": [{"field": "amount_sum", "direction": "desc"}],
    "limit": 100
}))]
pub struct QueryRequest {
    /// Dataset name, e.g. `sales`.
    pub dataset: String,
    /// Group-by fields (at most 2).
    #[serde(default)]
    pub dimensions: Vec<String>,
    /// Aggregates to compute (1 to 8).
    pub measures: Vec<String>,
    /// Conditions on dimensions, combined with AND.
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Ordering by selected fields; ties are broken by the dimensions.
    #[serde(default)]
    pub sort: Vec<SortKey>,
    /// Maximum rows returned, 1 to 10000 (default 1000).
    #[schema(minimum = 1, maximum = 10000)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    /// A dimension of the dataset.
    pub field: String,
    pub op: FilterOp,
    /// 1 to 100 values; `null` matches missing values.
    pub values: Vec<Option<String>>,
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FilterOp {
    In,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SortKey {
    /// A selected dimension or measure.
    pub field: String,
    #[serde(default)]
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

/// Describes one column of a [`super::QueryResult`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct ResultColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub column_type: ColumnType,
    /// Present for `decimal`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub precision: Option<u8>,
    /// Present for `decimal`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<u8>,
    pub nullable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Param {
    Text(String),
    BigInt(i64),
}

/// A validated, compiled query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub sql: String,
    pub params: Vec<Param>,
    pub columns: Vec<ResultColumn>,
    /// Rows to return; the SQL fetches one more to detect truncation.
    pub limit: usize,
}

/// Why a request cannot be compiled; safe to show to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidQuery(pub String);

fn invalid<T>(message: impl Into<String>) -> Result<T, InvalidQuery> {
    Err(InvalidQuery(message.into()))
}

/// Validates `request` and compiles it.
///
/// # Errors
/// [`InvalidQuery`] describing the first problem found.
pub fn compile(request: &QueryRequest) -> Result<Plan, InvalidQuery> {
    let Some(dataset) = catalog::dataset(&request.dataset) else {
        return invalid(format!("unknown dataset {:?}", request.dataset));
    };
    let dimensions = resolve(
        dataset,
        &request.dimensions,
        "dimension",
        MAX_DIMENSIONS,
        Dataset::dimension,
    )?;
    if request.measures.is_empty() {
        return invalid("at least one measure is required");
    }
    let measures = resolve(
        dataset,
        &request.measures,
        "measure",
        MAX_MEASURES,
        Dataset::measure,
    )?;
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return invalid(format!("limit must be between 1 and {MAX_LIMIT}"));
    }
    let mut params = Vec::new();
    let conditions = conditions(dataset, &request.filters, &mut params)?;

    // Aliases are positional (d0, m0…) so no name from the request reaches SQL.
    let selected: Vec<(String, &Field)> = dimensions
        .iter()
        .enumerate()
        .map(|(i, f)| (format!("d{i}"), *f))
        .chain(
            measures
                .iter()
                .enumerate()
                .map(|(i, f)| (format!("m{i}"), *f)),
        )
        .collect();
    let order = order(&selected, dimensions.len(), &request.sort)?;

    let mut sql = String::from("SELECT ");
    let outer: Vec<String> = selected
        .iter()
        .map(|(alias, _)| format!("CAST({alias} AS VARCHAR)"))
        .collect();
    sql.push_str(&outer.join(", "));
    sql.push_str(" FROM (SELECT ");
    let inner: Vec<String> = selected
        .iter()
        .map(|(alias, field)| format!("{} AS {alias}", field.sql))
        .collect();
    sql.push_str(&inner.join(", "));
    let _ = write!(sql, " FROM {}", dataset.table);
    if !conditions.is_empty() {
        let _ = write!(sql, " WHERE {}", conditions.join(" AND "));
    }
    if !dimensions.is_empty() {
        let ordinals: Vec<String> = (1..=dimensions.len()).map(|i| i.to_string()).collect();
        let _ = write!(sql, " GROUP BY {}", ordinals.join(", "));
    }
    sql.push_str(") AS q");
    if !order.is_empty() {
        let _ = write!(sql, " ORDER BY {}", order.join(", "));
    }
    sql.push_str(" LIMIT ?");
    params.push(Param::BigInt(i64::from(limit) + 1));

    Ok(Plan {
        sql,
        params,
        columns: selected
            .iter()
            .map(|(_, field)| ResultColumn {
                name: field.name.to_owned(),
                column_type: field.column_type,
                precision: field.decimal.map(|(precision, _)| precision),
                scale: field.decimal.map(|(_, scale)| scale),
                nullable: field.nullable,
            })
            .collect(),
        limit: limit as usize,
    })
}

/// SQL conditions for `filters`, appending their values to `params`.
fn conditions(
    dataset: &Dataset,
    filters: &[Filter],
    params: &mut Vec<Param>,
) -> Result<Vec<String>, InvalidQuery> {
    filters
        .iter()
        .map(|filter| {
            let Some(field) = dataset.dimension(&filter.field) else {
                return invalid(format!(
                    "filters apply to dimensions; {:?} is not a dimension of {:?}",
                    filter.field, dataset.name
                ));
            };
            let FilterOp::In = filter.op;
            if filter.values.is_empty() || filter.values.len() > MAX_FILTER_VALUES {
                return invalid(format!(
                    "filter on {:?} needs 1 to {MAX_FILTER_VALUES} values",
                    filter.field
                ));
            }
            Ok(in_condition(field, &filter.values, params))
        })
        .collect()
}

/// `ORDER BY` keys: the requested ones, then every dimension ascending, so
/// results are deterministic. Ordering uses the typed inner columns, never
/// their text form.
fn order(
    selected: &[(String, &Field)],
    dimension_count: usize,
    sort: &[SortKey],
) -> Result<Vec<String>, InvalidQuery> {
    let mut keys: Vec<(String, SortDirection)> = Vec::new();
    for key in sort {
        let Some((alias, _)) = selected.iter().find(|(_, field)| field.name == key.field) else {
            return invalid(format!(
                "sort field {:?} must be a selected dimension or measure",
                key.field
            ));
        };
        if keys.iter().any(|(a, _)| a == alias) {
            return invalid(format!("sort field {:?} is repeated", key.field));
        }
        keys.push((alias.clone(), key.direction));
    }
    for i in 0..dimension_count {
        let alias = format!("d{i}");
        if !keys.iter().any(|(a, _)| *a == alias) {
            keys.push((alias, SortDirection::Asc));
        }
    }
    Ok(keys
        .into_iter()
        .map(|(alias, direction)| {
            let direction = match direction {
                SortDirection::Asc => "ASC",
                SortDirection::Desc => "DESC",
            };
            format!("{alias} {direction} NULLS LAST")
        })
        .collect())
}

fn resolve(
    dataset: &'static Dataset,
    names: &[String],
    kind: &str,
    max: usize,
    find: fn(&'static Dataset, &str) -> Option<&'static Field>,
) -> Result<Vec<&'static Field>, InvalidQuery> {
    if names.len() > max {
        return invalid(format!("at most {max} {kind}s per query"));
    }
    let mut fields: Vec<&'static Field> = Vec::with_capacity(names.len());
    for name in names {
        let Some(field) = find(dataset, name) else {
            return invalid(format!(
                "unknown {kind} {name:?} in dataset {:?}",
                dataset.name
            ));
        };
        if fields.iter().any(|f| f.name == field.name) {
            return invalid(format!("{kind} {name:?} is repeated"));
        }
        fields.push(field);
    }
    Ok(fields)
}

fn in_condition(field: &Field, values: &[Option<String>], params: &mut Vec<Param>) -> String {
    let present: Vec<&String> = values.iter().flatten().collect();
    let includes_null = present.len() < values.len();
    let mut parts = Vec::new();
    if !present.is_empty() {
        let placeholders = vec!["?"; present.len()].join(", ");
        parts.push(format!("{} IN ({placeholders})", field.sql));
        params.extend(present.into_iter().map(|v| Param::Text(v.clone())));
    }
    if includes_null {
        parts.push(format!("{} IS NULL", field.sql));
    }
    if parts.len() == 1 {
        parts.remove(0)
    } else {
        format!("({})", parts.join(" OR "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(json: serde_json::Value) -> QueryRequest {
        serde_json::from_value(json).unwrap()
    }

    fn error(json: serde_json::Value) -> String {
        compile(&request(json)).unwrap_err().0
    }

    #[test]
    fn compiles_grouped_filtered_sorted_query() {
        let plan = compile(&request(serde_json::json!({
            "dataset": "sales",
            "dimensions": ["region"],
            "measures": ["row_count", "amount_sum"],
            "filters": [{"field": "region", "op": "in", "values": ["north", null, "south"]}],
            "sort": [{"field": "amount_sum", "direction": "desc"}],
            "limit": 50
        })))
        .unwrap();
        assert_eq!(
            plan.sql,
            "SELECT CAST(d0 AS VARCHAR), CAST(m0 AS VARCHAR), CAST(m1 AS VARCHAR) \
             FROM (SELECT region AS d0, count(*) AS m0, sum(amount) AS m1 \
             FROM lake.main.sales WHERE (region IN (?, ?) OR region IS NULL) GROUP BY 1) AS q \
             ORDER BY m1 DESC NULLS LAST, d0 ASC NULLS LAST LIMIT ?"
        );
        assert_eq!(
            plan.params,
            [
                Param::Text("north".into()),
                Param::Text("south".into()),
                Param::BigInt(51)
            ]
        );
        assert_eq!(plan.limit, 50);
        let names: Vec<_> = plan.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["region", "row_count", "amount_sum"]);
        assert_eq!(plan.columns[2].precision, Some(38));
        assert_eq!(plan.columns[2].scale, Some(2));
    }

    #[test]
    fn totals_without_dimensions_have_no_grouping_or_order() {
        let plan = compile(&request(serde_json::json!({
            "dataset": "sales",
            "measures": ["row_count", "quantity_sum"]
        })))
        .unwrap();
        assert_eq!(
            plan.sql,
            "SELECT CAST(m0 AS VARCHAR), CAST(m1 AS VARCHAR) \
             FROM (SELECT count(*) AS m0, sum(quantity) AS m1 FROM lake.main.sales) AS q LIMIT ?"
        );
        assert_eq!(plan.params, [Param::BigInt(i64::from(DEFAULT_LIMIT) + 1)]);
    }

    #[test]
    fn values_never_reach_the_sql_text() {
        let hostile = "x') OR 1=1; DROP TABLE lake.main.sales; --";
        let plan = compile(&request(serde_json::json!({
            "dataset": "sales",
            "measures": ["row_count"],
            "filters": [{"field": "region", "op": "in", "values": [hostile]}]
        })))
        .unwrap();
        assert!(!plan.sql.contains("DROP"), "{}", plan.sql);
        assert_eq!(plan.params[0], Param::Text(hostile.into()));
    }

    #[test]
    fn only_null_matches_missing_values() {
        let plan = compile(&request(serde_json::json!({
            "dataset": "sales",
            "measures": ["row_count"],
            "filters": [{"field": "region", "op": "in", "values": [null]}]
        })))
        .unwrap();
        assert!(plan.sql.contains("WHERE region IS NULL)"), "{}", plan.sql);
    }

    #[test]
    fn rejects_requests_outside_the_catalog() {
        for (json, expected) in [
            (
                serde_json::json!({"dataset": "users", "measures": ["row_count"]}),
                "unknown dataset",
            ),
            (
                serde_json::json!({"dataset": "sales", "measures": []}),
                "at least one measure",
            ),
            (
                serde_json::json!({"dataset": "sales", "measures": ["amount"]}),
                "unknown measure",
            ),
            (
                serde_json::json!({"dataset": "sales", "measures": ["row_count", "row_count"]}),
                "repeated",
            ),
            (
                serde_json::json!({"dataset": "sales", "dimensions": ["order_id"], "measures": ["row_count"]}),
                "unknown dimension",
            ),
            (
                serde_json::json!({"dataset": "sales", "measures": ["row_count"],
                    "filters": [{"field": "row_count", "op": "in", "values": ["1"]}]}),
                "not a dimension",
            ),
            (
                serde_json::json!({"dataset": "sales", "measures": ["row_count"],
                    "filters": [{"field": "region", "op": "in", "values": []}]}),
                "needs 1 to",
            ),
            (
                serde_json::json!({"dataset": "sales", "measures": ["row_count"],
                    "sort": [{"field": "region"}]}),
                "must be a selected",
            ),
            (
                serde_json::json!({"dataset": "sales", "measures": ["row_count"], "limit": 0}),
                "limit must be",
            ),
            (
                serde_json::json!({"dataset": "sales", "measures": ["row_count"], "limit": 10001}),
                "limit must be",
            ),
        ] {
            let message = error(json.clone());
            assert!(message.contains(expected), "{json} → {message}");
        }
        let too_many: Vec<String> = (0..=MAX_FILTER_VALUES).map(|i| i.to_string()).collect();
        assert!(
            error(
                serde_json::json!({"dataset": "sales", "measures": ["row_count"],
                "filters": [{"field": "region", "op": "in", "values": too_many}]})
            )
            .contains("needs 1 to")
        );
    }

    #[test]
    fn unknown_request_fields_are_rejected_at_parse_time() {
        let parsed: Result<QueryRequest, _> = serde_json::from_value(serde_json::json!({
            "dataset": "sales", "measures": ["row_count"], "sql": "SELECT 1"
        }));
        assert!(parsed.is_err());
    }
}
