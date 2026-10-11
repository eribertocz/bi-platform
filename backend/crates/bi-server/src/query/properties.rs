//! Property tests of the query compiler. For any request, valid or not:
//! compiling never panics; request values never reach the SQL text, which
//! is built only from catalog fragments, keywords and positional aliases;
//! and every compiled plan runs on DuckDB and returns exactly what a
//! reference model computes from the same rows.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use bi_runtime::duckdb::{self, Connection};
use proptest::prelude::*;

use super::plan::{self, Filter, FilterOp, Param, QueryRequest, SortDirection, SortKey};
use super::{Fetched, fetch};

const MEASURES: [&str; 3] = ["row_count", "amount_sum", "quantity_sum"];
/// Regions stored in the generated rows; one is hostile on purpose.
const REGIONS: [&str; 4] = ["east", "north", "south", "x') OR 1=1; --"];
/// Largest magnitude of `amount` (DECIMAL(18,2)) in hundredths.
const MAX_CENTS: i64 = 999_999_999_999_999_999;

fn filter_value() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        3 => prop::sample::select(&REGIONS[..]).prop_map(|region| Some(region.to_owned())),
        1 => Just(None),
        2 => any::<String>().prop_map(Some),
    ]
}

/// A request the catalog accepts, built by construction: optional grouping
/// by region, distinct measures in any order, filters with 1 to 4 values,
/// and sort keys drawn from the selected fields.
fn valid_request() -> impl Strategy<Value = QueryRequest> {
    (
        any::<bool>(),
        prop::sample::subsequence(&MEASURES[..], 1..=MEASURES.len()).prop_shuffle(),
        prop::collection::vec(prop::collection::vec(filter_value(), 1..=4), 0..=2),
        prop_oneof![
            1 => Just(None),
            4 => (1..=6_u32).prop_map(Some),
            1 => (1..=plan::MAX_LIMIT).prop_map(Some),
        ],
    )
        .prop_flat_map(|(grouped, measures, filters, limit)| {
            let dimensions: Vec<String> = if grouped {
                vec!["region".to_owned()]
            } else {
                Vec::new()
            };
            let measures: Vec<String> = measures.into_iter().map(str::to_owned).collect();
            let selected: Vec<String> = dimensions.iter().chain(&measures).cloned().collect();
            let count = selected.len();
            (
                Just(dimensions),
                Just(measures),
                Just(filters),
                Just(limit),
                prop::sample::subsequence(selected, 0..=count).prop_shuffle(),
                prop::collection::vec(any::<bool>(), count),
            )
        })
        .prop_map(
            |(dimensions, measures, filters, limit, sorted, descending)| QueryRequest {
                dataset: "sales".to_owned(),
                dimensions,
                measures,
                filters: filters
                    .into_iter()
                    .map(|values| Filter {
                        field: "region".to_owned(),
                        op: FilterOp::In,
                        values,
                    })
                    .collect(),
                sort: sorted
                    .into_iter()
                    .zip(descending)
                    .map(|(field, descending)| SortKey {
                        field,
                        direction: if descending {
                            SortDirection::Desc
                        } else {
                            SortDirection::Asc
                        },
                    })
                    .collect(),
                limit,
            },
        )
}

/// Mostly valid requests; one in five breaks exactly one rule, often with
/// arbitrary text, so rejections and hostile names are exercised too.
fn request() -> impl Strategy<Value = QueryRequest> {
    let broken =
        (valid_request(), 0..8_u8, any::<String>()).prop_map(|(mut request, rule, text)| {
            match rule {
                0 => request.dataset = text,
                1 => request.measures.push(text),
                2 => request.measures.clear(),
                3 => request.measures.push(request.measures[0].clone()),
                4 => request.filters.push(Filter {
                    field: "row_count".to_owned(),
                    op: FilterOp::In,
                    values: vec![Some(text)],
                }),
                5 => request.filters.push(Filter {
                    field: "region".to_owned(),
                    op: FilterOp::In,
                    values: Vec::new(),
                }),
                6 => request.sort.push(SortKey {
                    field: text,
                    direction: SortDirection::Asc,
                }),
                _ => {
                    request.limit = Some(if text.len() % 2 == 0 {
                        0
                    } else {
                        plan::MAX_LIMIT + 1
                    });
                }
            }
            request
        });
    prop_oneof![4 => valid_request(), 1 => broken]
}

/// A `sales` row as the reference model sees it.
#[derive(Debug, Clone)]
struct Sale {
    region: Option<String>,
    quantity: Option<i64>,
    /// `amount` in hundredths.
    cents: Option<i64>,
}

fn sales() -> impl Strategy<Value = Vec<Sale>> {
    // Small values too, so groups sum to a real zero and measures tie.
    let quantity = prop_oneof![any::<i64>(), -3..=3_i64];
    let cents = prop_oneof![-MAX_CENTS..=MAX_CENTS, -3..=3_i64];
    let sale = (
        prop::option::weighted(0.8, prop::sample::select(&REGIONS[..])),
        prop::option::weighted(0.8, quantity),
        prop::option::weighted(0.8, cents),
    )
        .prop_map(|(region, quantity, cents)| Sale {
            region: region.map(str::to_owned),
            quantity,
            cents,
        });
    prop::collection::vec(sale, 0..40)
}

fn decimal(hundredths: i128) -> String {
    let sign = if hundredths < 0 { "-" } else { "" };
    let magnitude = hundredths.unsigned_abs();
    format!("{sign}{}.{:02}", magnitude / 100, magnitude % 100)
}

/// A typed result cell: ordering uses the value, never its text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Text(Option<String>),
    Integer(Option<i128>),
    Hundredths(Option<i128>),
}

impl Cell {
    fn is_null(&self) -> bool {
        matches!(
            self,
            Self::Text(None) | Self::Integer(None) | Self::Hundredths(None)
        )
    }

    fn render(&self) -> Option<String> {
        match self {
            Self::Text(text) => text.clone(),
            Self::Integer(value) => value.map(|v| v.to_string()),
            Self::Hundredths(value) => value.map(decimal),
        }
    }
}

/// `count(*)` counts every row; `sum` ignores nulls and is null when the
/// group has no value at all (including an empty table).
fn measure(name: &str, group: &[&Sale]) -> Cell {
    let sum = |value: fn(&Sale) -> Option<i64>| {
        let present: Vec<i128> = group
            .iter()
            .filter_map(|s| value(s))
            .map(i128::from)
            .collect();
        (!present.is_empty()).then(|| present.iter().sum())
    };
    match name {
        "row_count" => Cell::Integer(Some(group.len() as i128)),
        "amount_sum" => Cell::Hundredths(sum(|s| s.cents)),
        "quantity_sum" => Cell::Integer(sum(|s| s.quantity)),
        other => unreachable!("not a measure: {other}"),
    }
}

/// Every row the compiled query must produce, in order and before any
/// limit, computed without SQL. Only called for accepted requests.
fn model(request: &QueryRequest, sales: &[Sale]) -> Vec<Vec<Option<String>>> {
    // `IN (...)` plus `IS NULL` when the values include null; filters AND.
    let kept: Vec<&Sale> = sales
        .iter()
        .filter(|sale| {
            request
                .filters
                .iter()
                .all(|filter| filter.values.contains(&sale.region))
        })
        .collect();
    let grouped = !request.dimensions.is_empty();
    let groups: Vec<Vec<&Sale>> = if grouped {
        let mut by_region: BTreeMap<&Option<String>, Vec<&Sale>> = BTreeMap::new();
        for sale in kept {
            by_region.entry(&sale.region).or_default().push(sale);
        }
        by_region.into_values().collect()
    } else {
        vec![kept]
    };
    let mut rows: Vec<Vec<Cell>> = groups
        .iter()
        .map(|group| {
            let dimension = grouped.then(|| Cell::Text(group[0].region.clone()));
            dimension
                .into_iter()
                .chain(request.measures.iter().map(|name| measure(name, group)))
                .collect()
        })
        .collect();

    // Requested keys, then the dimensions ascending; nulls last both ways.
    let selected: Vec<&String> = request.dimensions.iter().chain(&request.measures).collect();
    let mut keys: Vec<(usize, bool)> = request
        .sort
        .iter()
        .map(|key| {
            let column = selected.iter().position(|name| **name == key.field);
            (column.unwrap(), key.direction == SortDirection::Desc)
        })
        .collect();
    for column in 0..request.dimensions.len() {
        if !keys.iter().any(|(c, _)| *c == column) {
            keys.push((column, false));
        }
    }
    rows.sort_by(|a, b| {
        keys.iter()
            .map(|&(column, descending)| {
                let (x, y) = (&a[column], &b[column]);
                match (x.is_null(), y.is_null()) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (false, false) if descending => y.cmp(x),
                    (false, false) => x.cmp(y),
                }
            })
            .find(|ordering| ordering.is_ne())
            .unwrap_or(Ordering::Equal)
    });
    rows.into_iter()
        .map(|row| row.iter().map(Cell::render).collect())
        .collect()
}

fn requested_limit(request: &QueryRequest) -> usize {
    request.limit.unwrap_or(plan::DEFAULT_LIMIT) as usize
}

fn lake(sales: &[Sale]) -> duckdb::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(
        "ATTACH ':memory:' AS lake;
         CREATE TABLE lake.main.sales (order_id BIGINT, ordered_at TIMESTAMPTZ,
             region VARCHAR, quantity BIGINT, amount DECIMAL(18,2));",
    )?;
    let mut insert = conn
        .prepare("INSERT INTO lake.main.sales VALUES (1, NULL, ?, ?, CAST(? AS DECIMAL(18,2)))")?;
    for sale in sales {
        insert.execute(duckdb::params![
            sale.region,
            sale.quantity,
            sale.cents.map(|cents| decimal(i128::from(cents)))
        ])?;
    }
    Ok(conn)
}

/// Words the compiled SQL may contain: keywords, catalog fragments and the
/// positional aliases. Any request text would add a word outside this set.
fn sql_vocabulary(word: &str) -> bool {
    const WORDS: &[&str] = &[
        "SELECT", "CAST", "AS", "VARCHAR", "FROM", "WHERE", "IN", "OR", "AND", "IS", "NULL",
        "GROUP", "BY", "ORDER", "ASC", "DESC", "NULLS", "LAST", "LIMIT", "q", "lake", "main",
        "sales", "region", "count", "sum", "amount", "quantity",
    ];
    let alias = |prefix: char| {
        word.strip_prefix(prefix)
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    };
    WORDS.contains(&word) || word.bytes().all(|b| b.is_ascii_digit()) || alias('d') || alias('m')
}

/// The properties only say something if most generated requests compile:
/// guards the generator against drifting into testing rejections alone.
#[test]
fn the_generator_mostly_produces_valid_requests() {
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;

    let mut runner = TestRunner::deterministic();
    let strategy = request();
    let valid = (0..1_000)
        .filter(|_| {
            let request = strategy.new_tree(&mut runner).unwrap().current();
            plan::compile(&request).is_ok()
        })
        .count();
    println!("{valid} of 1000 generated requests compile");
    assert!(
        valid >= 700,
        "only {valid} of 1000 generated requests compile"
    );
}

proptest! {
    #[test]
    fn request_values_never_reach_the_sql(
        request in request(),
        other in any::<String>(),
        other_limit in 1..=plan::MAX_LIMIT,
    ) {
        let Ok(compiled) = plan::compile(&request) else {
            return Ok(());
        };
        for word in compiled
            .sql
            .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .filter(|word| !word.is_empty())
        {
            prop_assert!(sql_vocabulary(word), "{word:?} in {}", compiled.sql);
        }
        // The SQL is a function of the request's shape alone: neither the
        // filter values nor the limit change it.
        let mut same_shape = request.clone();
        for filter in &mut same_shape.filters {
            for value in filter.values.iter_mut().flatten() {
                value.clone_from(&other);
            }
        }
        same_shape.limit = Some(other_limit);
        prop_assert_eq!(&plan::compile(&same_shape).unwrap().sql, &compiled.sql);
        // Every value is bound in order, then the limit plus the one row that
        // detects truncation.
        let limit = requested_limit(&request);
        prop_assert_eq!(compiled.limit, limit);
        let mut expected: Vec<Param> = request
            .filters
            .iter()
            .flat_map(|filter| filter.values.iter().flatten())
            .map(|value| Param::Text(value.clone()))
            .collect();
        expected.push(Param::BigInt(i64::try_from(limit + 1).unwrap()));
        prop_assert_eq!(compiled.sql.matches('?').count(), expected.len());
        prop_assert_eq!(&compiled.params, &expected);
    }

    #[test]
    fn compiled_plans_run_on_duckdb_and_match_the_model(request in request(), sales in sales()) {
        let Ok(compiled) = plan::compile(&request) else {
            return Ok(());
        };
        let conn = lake(&sales).map_err(|e| TestCaseError::fail(e.to_string()))?;
        // As the endpoint does: read one row past the limit to detect truncation.
        let fetched = fetch(
            &conn,
            &compiled.sql,
            &compiled.params,
            compiled.columns.len(),
            compiled.limit + 1,
        )
        .map_err(|e| TestCaseError::fail(format!("{e} in {}", compiled.sql)))?;
        let Fetched::Rows(rows) = fetched else {
            return Err(TestCaseError::fail("a tiny result exceeded the byte budget"));
        };
        let expected = model(&request, &sales);
        let limit = requested_limit(&request);
        prop_assert_eq!(rows.len() > limit, expected.len() > limit, "truncation flag");
        prop_assert_eq!(rows, expected.into_iter().take(limit + 1).collect::<Vec<_>>());
    }
}
