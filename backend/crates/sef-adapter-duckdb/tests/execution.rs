#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../sef-planner/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use duckdb::Connection;
use duckdb::arrow::array::{
    Array, Date32Array, Decimal128Array, Decimal256Array, Int64Array, StringArray,
    TimestampMicrosecondArray,
};
use duckdb::arrow::datatypes::DataType;
use proptest::prelude::*;
use sef_adapter_duckdb::execution::{
    BlockResult, DuckDbPlan, ExecutionError, PhysicalRelation, SourceCatalog, lower,
};
use sef_correctness::SemanticContext;
use sef_exec_ir::SemanticPlan;
use sef_model_ir::model::CompiledSemanticModel;
use sef_model_ir::types::{Literal, ValueType};

fn context(tenant: &str) -> SemanticContext {
    SemanticContext {
        slots: BTreeMap::from([("tenant_id".into(), Literal::Utf8(tenant.into()))]),
    }
}

fn bindings(case: u8) -> SourceCatalog {
    common::catalog(case)
        .relations
        .into_values()
        .map(|relation| {
            (
                (relation.source, relation.relation.clone()),
                PhysicalRelation {
                    name: vec![relation.relation.as_str().to_owned()],
                    columns: relation
                        .columns
                        .into_iter()
                        .map(|column| (column.handle, column.name))
                        .collect(),
                },
            )
        })
        .collect()
}

fn planned(
    case: u8,
    model: &CompiledSemanticModel,
    context: &SemanticContext,
) -> (sef_correctness::ValidatedQuery, sef_planner::PlannedQuery) {
    let bound = sef_query_ir::bind(&common::request(case), model.definition()).unwrap();
    let valid = sef_correctness::validate(model, &bound, context).unwrap();
    let plan = sef_planner::plan(model, &valid).unwrap();
    (valid, plan)
}

fn lower_model(case: u8, model: &CompiledSemanticModel, context: &SemanticContext) -> DuckDbPlan {
    let (valid, plan) = planned(case, model, context);
    lower(
        model,
        &valid,
        &plan.plan,
        &plan.certificate,
        &bindings(case),
    )
    .unwrap()
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Cell {
    Null,
    Decimal(i128, u8, i8),
    WideDecimal(String, u8, i8),
    Integer(i64),
    Text(String),
    Date(i32),
    Instant(i64),
}

fn rows(results: &[BlockResult]) -> Vec<Vec<Cell>> {
    let mut rows = Vec::new();
    for batch in &results[0].batches {
        for row in 0..batch.num_rows() {
            rows.push(
                batch
                    .columns()
                    .iter()
                    .map(|column| {
                        if column.is_null(row) {
                            return Cell::Null;
                        }
                        match column.data_type() {
                            DataType::Decimal256(p, s) => Cell::WideDecimal(
                                column
                                    .as_any()
                                    .downcast_ref::<Decimal256Array>()
                                    .unwrap()
                                    .value(row)
                                    .to_string(),
                                *p,
                                *s,
                            ),
                            DataType::Decimal128(p, s) => Cell::Decimal(
                                column
                                    .as_any()
                                    .downcast_ref::<Decimal128Array>()
                                    .unwrap()
                                    .value(row),
                                *p,
                                *s,
                            ),
                            DataType::Int64 => Cell::Integer(
                                column
                                    .as_any()
                                    .downcast_ref::<Int64Array>()
                                    .unwrap()
                                    .value(row),
                            ),
                            DataType::Utf8 => Cell::Text(
                                column
                                    .as_any()
                                    .downcast_ref::<StringArray>()
                                    .unwrap()
                                    .value(row)
                                    .into(),
                            ),
                            DataType::Date32 => Cell::Date(
                                column
                                    .as_any()
                                    .downcast_ref::<Date32Array>()
                                    .unwrap()
                                    .value(row),
                            ),
                            DataType::Timestamp(
                                duckdb::arrow::datatypes::TimeUnit::Microsecond,
                                Some(_),
                            ) => Cell::Instant(
                                column
                                    .as_any()
                                    .downcast_ref::<TimestampMicrosecondArray>()
                                    .unwrap()
                                    .value(row),
                            ),
                            ty => panic!("unexpected exact type: {ty:?}"),
                        }
                    })
                    .collect(),
            );
        }
    }
    rows.sort();
    rows
}

// Independent oracle: Rust sums scaled integers, never SQL or model AST.
fn oracle_sales(data: &[(&str, &str, Option<i128>)], tenant: Option<&str>) -> Vec<Vec<Cell>> {
    let mut totals: BTreeMap<&str, Option<i128>> = BTreeMap::new();
    for (owner, region, amount) in data {
        if tenant.is_some_and(|tenant| owner != &tenant) {
            continue;
        }
        let sum = totals.entry(region).or_default();
        if let Some(amount) = amount {
            *sum = Some(sum.unwrap_or(0) + amount);
        }
    }
    let mut rows: Vec<_> = totals
        .into_iter()
        .map(|(region, sum)| {
            vec![
                sum.map_or(Cell::Null, |value| Cell::Decimal(value, 38, 2)),
                Cell::Text(region.into()),
            ]
        })
        .collect();
    rows.sort();
    rows
}

fn nullable_model(case: u8) -> CompiledSemanticModel {
    let mut catalog = common::catalog(case);
    catalog
        .relations
        .get_mut("sales")
        .unwrap()
        .columns
        .iter_mut()
        .find(|c| c.name == "amount")
        .unwrap()
        .nullable = true;
    sef_ossie::csmir::compile(&common::document(case), &catalog, &common::options(case)).unwrap()
}

fn sales(connection: &Connection, protected: bool, data: &[(&str, &str, Option<i128>)]) {
    connection.execute_batch(if protected {
        "CREATE TABLE sales(sale_id BIGINT NOT NULL, tenant_id VARCHAR NOT NULL, region VARCHAR NOT NULL, amount DECIMAL(18,2))"
    } else {
        "CREATE TABLE sales(sale_id BIGINT NOT NULL, region VARCHAR NOT NULL, amount DECIMAL(18,2))"
    }).unwrap();
    for (i, (tenant, region, amount)) in data.iter().enumerate() {
        let amount = amount.map(|v| {
            format!(
                "{}{}.{:02}",
                if v < 0 { "-" } else { "" },
                v.abs() / 100,
                v.abs() % 100
            )
        });
        if protected {
            connection
                .execute(
                    "INSERT INTO sales VALUES (?, ?, ?, CAST(? AS DECIMAL(18,2)))",
                    duckdb::params![i64::try_from(i).unwrap(), tenant, region, amount],
                )
                .unwrap();
        } else {
            connection
                .execute(
                    "INSERT INTO sales VALUES (?, ?, CAST(? AS DECIMAL(18,2)))",
                    duckdb::params![i64::try_from(i).unwrap(), region, amount],
                )
                .unwrap();
        }
    }
}

#[test]
fn case_001_matches_independent_scaled_integer_oracle_including_nulls() {
    let model = nullable_model(1);
    let none = SemanticContext::default();
    let plan = lower_model(1, &model, &none);
    let connection = Connection::open_in_memory().unwrap();
    let data = [
        ("a", "north", Some(9_007_199_254_740_993)),
        ("a", "north", Some(17)),
        ("a", "north", None),
        ("b", "south", Some(-30)),
        ("b", "empty", None),
    ];
    sales(&connection, false, &data);
    assert_eq!(
        rows(&plan.execute(&connection, &none).unwrap()),
        oracle_sales(&data, None)
    );
    connection.execute("DELETE FROM sales", []).unwrap();
    assert_eq!(
        rows(&plan.execute(&connection, &none).unwrap()),
        Vec::<Vec<Cell>>::new()
    );
}

#[test]
fn case_015_binds_context_and_rejects_missing_or_wrong_type() {
    let model = nullable_model(15);
    let who = context("a");
    let plan = lower_model(15, &model, &who);
    let connection = Connection::open_in_memory().unwrap();
    let data = [
        ("a", "north", Some(9_007_199_254_740_993)),
        ("a", "north", Some(17)),
        ("b", "north", Some(800)),
        ("a", "empty", None),
    ];
    sales(&connection, true, &data);
    assert!(
        plan.sql()
            .all(|(_, sql)| sql.contains("$1") && !sql.contains("secret"))
    );
    for tenant in ["a", "b", "' OR TRUE -- secret", "absent"] {
        assert_eq!(
            rows(&plan.execute(&connection, &context(tenant)).unwrap()),
            oracle_sales(&data, Some(tenant))
        );
    }
    assert!(matches!(
        plan.execute(&connection, &SemanticContext::default()),
        Err(ExecutionError::MissingContext(_))
    ));
    let wrong = SemanticContext {
        slots: BTreeMap::from([("tenant_id".into(), Literal::Boolean(true))]),
    };
    assert!(matches!(
        plan.execute(&connection, &wrong),
        Err(ExecutionError::InvalidContext(_))
    ));
    // Missing context is rejected before the engine is accessed, even without tables.
    assert!(matches!(
        plan.execute(
            &Connection::open_in_memory().unwrap(),
            &SemanticContext::default()
        ),
        Err(ExecutionError::MissingContext(_))
    ));
}

#[test]
fn case_009_aggregates_before_alignment_and_preserves_absent_branches() {
    let none = SemanticContext::default();
    let model = common::model(9);
    let plan = lower_model(9, &model, &none);
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE sales_monthly(month DATE NOT NULL, region VARCHAR NOT NULL, amount DECIMAL(18,2) NOT NULL); CREATE TABLE refunds_monthly(month DATE NOT NULL, region VARCHAR NOT NULL, amount DECIMAL(18,2) NOT NULL)").unwrap();
    // Independent facts and unmatched domains. The oracle unions keys and
    // subtracts integer coefficients only when both contributions exist.
    let gross = BTreeMap::from([
        ((19_723_i32, "north"), 9_007_199_254_740_993_i128),
        ((19_723, "south"), 400),
    ]);
    let refunds = BTreeMap::from([((19_723_i32, "north"), 17_i128), ((19_723, "west"), 50)]);
    for (table, values) in [("sales_monthly", &gross), ("refunds_monthly", &refunds)] {
        for ((day, region), cents) in values {
            let amount = format!("{}.{:02}", cents / 100, cents % 100);
            connection.execute(&format!("INSERT INTO {table} VALUES (DATE '1970-01-01' + CAST(? AS INTEGER), ?, CAST(? AS DECIMAL(18,2)))"), duckdb::params![day, region, amount]).unwrap();
        }
    }
    let keys: std::collections::BTreeSet<_> = gross.keys().chain(refunds.keys()).copied().collect();
    let mut expected = keys
        .into_iter()
        .map(|key @ (day, region)| {
            let g = gross.get(&key).copied();
            let r = refunds.get(&key).copied();
            vec![
                g.map_or(Cell::Null, |v| Cell::Decimal(v, 38, 2)),
                r.map_or(Cell::Null, |v| Cell::Decimal(v, 38, 2)),
                g.zip(r)
                    .map_or(Cell::Null, |(g, r)| Cell::Decimal(g - r, 19, 2)),
                Cell::Date(day),
                Cell::Text(region.into()),
            ]
        })
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(rows(&plan.execute(&connection, &none).unwrap()), expected);
    assert!(plan.sql().next().unwrap().1.contains("FULL OUTER JOIN"));
}

#[test]
fn unknown_node_stale_certificate_and_unsupported_semantics_never_execute() {
    let none = SemanticContext::default();
    let model = common::model(1);
    let (valid, planned) = planned(1, &model, &none);
    let missing = SemanticPlan {
        graph: sef_exec_ir::SeirGraph::new(),
        blocks: planned.plan.blocks.clone(),
    };
    assert!(matches!(
        lower(&model, &valid, &missing, &planned.certificate, &bindings(1)),
        Err(ExecutionError::UnknownNode)
    ));
    let mut stale = planned.certificate.clone();
    stale.query_hash = stale.model_hash;
    assert!(matches!(
        lower(&model, &valid, &planned.plan, &stale, &bindings(1)),
        Err(ExecutionError::InvalidCertificate)
    ));
}

fn multihop_database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE sales(sale_id BIGINT NOT NULL, store_id BIGINT NOT NULL, amount DECIMAL(18,2) NOT NULL);
        CREATE TABLE store(store_id BIGINT NOT NULL, region_id BIGINT NOT NULL);
        CREATE TABLE region(region_id BIGINT NOT NULL, region_name VARCHAR NOT NULL);
        INSERT INTO region VALUES (1, 'norte'), (2, 'sur');
        INSERT INTO store VALUES (10,1), (11,1), (20,2);
        INSERT INTO sales VALUES (1,10,90071992547409.93), (2,11,-1.00), (3,20,2.01);").unwrap();
    conn
}

#[test]
fn case_005_multihop_matches_an_independent_integer_oracle() {
    let plan = lower_model(5, &common::model(5), &SemanticContext::default());
    let conn = multihop_database();
    assert_eq!(
        rows(&plan.execute(&conn, &SemanticContext::default()).unwrap()),
        vec![
            vec![Cell::Decimal(201, 38, 2), Cell::Text("sur".into())],
            vec![
                Cell::Decimal(9_007_199_254_740_893, 38, 2),
                Cell::Text("norte".into())
            ],
        ]
    );
}

#[test]
fn uniqueness_and_required_matches_are_checked_before_returning_data() {
    let plan = lower_model(5, &common::model(5), &SemanticContext::default());
    for mutation in [
        "INSERT INTO store VALUES (10,2)",
        "INSERT INTO region VALUES (1,'duplicado')",
        "DELETE FROM store WHERE store_id=10",
        "DELETE FROM region WHERE region_id=1",
    ] {
        let conn = multihop_database();
        conn.execute_batch(mutation).unwrap();
        assert!(
            matches!(
                plan.execute(&conn, &SemanticContext::default()),
                Err(ExecutionError::ContractViolation)
            ),
            "{mutation}"
        );
        // Failure rolls back the adapter transaction, leaving the connection reusable.
        assert_eq!(
            conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        conn.execute_batch("BEGIN; ROLLBACK").unwrap();
    }
}

#[test]
fn uniqueness_checks_only_the_governed_lookup() {
    let yaml = common::read("CASE-005.yaml").replacen(
        "\"semantic_version\": \"0.1.0\"",
        "\"semantic_version\": \"0.1.0\", \"policies\": [{\"name\": \"visible_regions\", \"dataset\": \"region\", \"enforcement\": \"mandatory\", \"predicate\": \"region_name != :tenant_id\", \"context\": [{\"name\": \"tenant_id\", \"type\": \"string\", \"sensitivity\": \"sensitive\"}]}]",
        1,
    );
    let document = sef_ossie::load(&yaml).unwrap().model;
    let model =
        sef_ossie::csmir::compile(&document, &common::catalog(5), &common::options(5)).unwrap();
    let context = context("hidden");
    let plan = lower_model(5, &model, &context);
    let conn = multihop_database();
    let before = rows(&plan.execute(&conn, &context).unwrap());
    conn.execute_batch("INSERT INTO region VALUES (1,'hidden')")
        .unwrap();
    assert_eq!(rows(&plan.execute(&conn, &context).unwrap()), before);
    // The same duplicate becomes visible to another context and must be rejected.
    assert!(matches!(
        plan.execute(&conn, &crate::context("unmatched")),
        Err(ExecutionError::ContractViolation)
    ));
}

fn star_database(data: &[(i64, i128)]) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE sales(line_id BIGINT NOT NULL, customer_id BIGINT NOT NULL, product_id BIGINT NOT NULL, quantity BIGINT NOT NULL, unit_price DECIMAL(18,2) NOT NULL);
        CREATE TABLE customers(customer_id BIGINT NOT NULL, region VARCHAR NOT NULL);
        CREATE TABLE products(product_id BIGINT NOT NULL, category VARCHAR NOT NULL);
        INSERT INTO customers VALUES(1,'norte'); INSERT INTO products VALUES(1,'producto');").unwrap();
    for (i, (quantity, coefficient)) in data.iter().enumerate() {
        let price = format!(
            "{}{}.{:02}",
            if *coefficient < 0 { "-" } else { "" },
            coefficient.abs() / 100,
            coefficient.abs() % 100
        );
        conn.execute(
            "INSERT INTO sales VALUES (?,1,1,?,CAST(? AS DECIMAL(18,2)))",
            duckdb::params![i64::try_from(i).unwrap(), quantity, price],
        )
        .unwrap();
    }
    conn
}

#[test]
fn case_002_preserves_39_digits_as_decimal256_without_float_or_narrowing() {
    let plan = lower_model(2, &common::model(2), &SemanticContext::default());
    let data = [(i64::MAX, 999_999_999_999_999_999_i128), (-1, 123)];
    let expected = data
        .iter()
        .map(|(q, p)| num_bigint::BigInt::from(*q) * num_bigint::BigInt::from(*p))
        .sum::<num_bigint::BigInt>();
    let result = plan
        .execute(&star_database(&data), &SemanticContext::default())
        .unwrap();
    assert_eq!(
        rows(&result),
        vec![vec![
            Cell::WideDecimal(expected.to_string(), 39, 2),
            Cell::Text("norte".into()),
            Cell::Text("producto".into())
        ]]
    );
    assert_eq!(
        result[0].batches[0].schema().field(0).metadata()["sef.metric_id"],
        "corpus/metric/revenue"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn star_products_and_sums_match_the_integer_oracle(data in prop::collection::vec((any::<i64>(), -999_999_999_999_999_999_i128..=999_999_999_999_999_999_i128),1..15)) {
        let plan = lower_model(2, &common::model(2), &SemanticContext::default());
        let expected = data.iter().map(|(q,p)| num_bigint::BigInt::from(*q) * num_bigint::BigInt::from(*p)).sum::<num_bigint::BigInt>();
        let result = rows(&plan.execute(&star_database(&data), &SemanticContext::default()).unwrap());
        prop_assert_eq!(&result[0][0], &Cell::WideDecimal(expected.to_string(),39,2));
    }
}

fn corpus_root(case: u8) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../corpus/cases/CASE-{case:03}"))
}

#[test]
fn wide_sum_checks_final_precision_and_preserves_cancellation() {
    let plan = lower_model(2, &common::model(2), &SemanticContext::default());
    let largest = (i64::MAX, 999_999_999_999_999_999_i128);
    let mut data = vec![largest; 110];
    assert!(matches!(
        plan.execute(&star_database(&data), &SemanticContext::default()),
        Err(ExecutionError::NumericOverflow)
    ));
    data.extend(vec![(-largest.0, largest.1); 110]);
    let result = plan
        .execute(&star_database(&data), &SemanticContext::default())
        .unwrap();
    assert_eq!(rows(&result)[0][0], Cell::WideDecimal("0".into(), 39, 2));
}

fn fixture_database(case: u8) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    for (name, relation) in common::catalog(case).relations {
        let columns = relation
            .columns
            .iter()
            .map(|c| {
                let ty = match c.value_type {
                    ValueType::Integer { .. } => "BIGINT".into(),
                    ValueType::Utf8 => "VARCHAR".into(),
                    ValueType::Date => "DATE".into(),
                    ValueType::Timestamp { .. } => "TIMESTAMPTZ".into(),
                    ValueType::Decimal(d) => format!("DECIMAL({},{})", d.precision(), d.scale()),
                    _ => panic!("fixture type"),
                };
                format!(
                    "\"{}\" {ty}{}",
                    c.name,
                    if c.nullable { "" } else { " NOT NULL" }
                )
            })
            .collect::<Vec<_>>();
        conn.execute_batch(&format!("CREATE TABLE \"{name}\" ({})", columns.join(",")))
            .unwrap();
        let text =
            std::fs::read_to_string(corpus_root(case).join(format!("input/{name}.csv"))).unwrap();
        let mut lines = text.lines();
        let fields = lines
            .next()
            .unwrap()
            .split(',')
            .map(|s| format!("\"{s}\""))
            .collect::<Vec<_>>();
        let placeholders = vec!["?"; fields.len()].join(",");
        for line in lines {
            let values = line
                .split(',')
                .enumerate()
                .map(|(i, s)| {
                    if s.is_empty() || s == "NULL" {
                        None
                    } else {
                        let name = fields[i].trim_matches('"');
                        let monthly_date = relation
                            .columns
                            .iter()
                            .any(|c| c.name == name && c.value_type == ValueType::Date)
                            && s.len() == 7;
                        Some(if monthly_date {
                            format!("{s}-01")
                        } else {
                            s.to_owned()
                        })
                    }
                })
                .collect::<Vec<_>>();
            conn.execute(
                &format!(
                    "INSERT INTO \"{name}\" ({}) VALUES({placeholders})",
                    fields.join(",")
                ),
                duckdb::params_from_iter(values.iter()),
            )
            .unwrap();
        }
    }
    conn
}

fn decimal_text(coefficient: &str, scale: i8) -> String {
    let (sign, digits) = coefficient
        .strip_prefix('-')
        .map_or(("", coefficient), |v| ("-", v));
    let scale = usize::try_from(scale).unwrap();
    let digits = format!("{:0>width$}", digits, width = scale + 1);
    if scale == 0 {
        format!("{sign}{digits}")
    } else {
        let split = digits.len() - scale;
        format!("{sign}{}.{}", &digits[..split], &digits[split..])
    }
}

#[test]
fn supported_cases_match_the_normative_csv_oracles() {
    for case in [1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15, 18, 19, 21] {
        println!("CASE-{case:03}");
        let context = if case == 15 {
            context("A")
        } else {
            SemanticContext::default()
        };
        let plan = lower_model(case, &common::model(case), &context);
        let result = plan.execute(&fixture_database(case), &context).unwrap();
        let expected =
            std::fs::read_to_string(corpus_root(case).join("expected/result.csv")).unwrap();
        let mut expected = expected.lines();
        let header = expected.next().unwrap().split(',').collect::<Vec<_>>();
        let schema = result[0].batches[0].schema();
        let indices = header
            .iter()
            .enumerate()
            .filter_map(|(expected_index, name)| {
                // The corpus names the field; the query selects it as `region`.
                let selection = match (case, *name) {
                    (5, "region_name") => "region",
                    (19, "business_day") => "day",
                    (13, "region") => "historical_region",
                    _ => *name,
                };
                schema.index_of(selection).ok().map(|i| (expected_index, i))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            indices.len(),
            schema.fields().len(),
            "CASE-{case:03} has an output outside the oracle"
        );
        let mut actual = rows(&result)
            .into_iter()
            .map(|row| {
                indices
                    .iter()
                    .map(|(_, i)| match &row[*i] {
                        Cell::Decimal(v, _, s) => decimal_text(&v.to_string(), *s),
                        Cell::WideDecimal(v, _, s) => decimal_text(v, *s),
                        Cell::Text(v) => v.clone(),
                        Cell::Integer(v) => v.to_string(),
                        Cell::Null => "NULL".into(),
                        Cell::Date(days) => {
                            let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
                            let date = epoch + chrono::Duration::days(i64::from(*days));
                            if header.contains(&"month") {
                                date.format("%Y-%m").to_string()
                            } else {
                                date.to_string()
                            }
                        }
                        Cell::Instant(_) => panic!("fixture output"),
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>();
        let mut expected = expected
            .map(|line| {
                let fields = line.split(',').collect::<Vec<_>>();
                indices
                    .iter()
                    .map(|(i, _)| fields[*i])
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "CASE-{case:03}");
    }
}

#[test]
fn distinct_is_recomputed_per_block_in_one_snapshot() {
    let plan = lower_model(8, &common::model(8), &SemanticContext::default());
    let result = plan
        .execute(&fixture_database(8), &SemanticContext::default())
        .unwrap();
    assert_eq!(result.len(), 2);
    let by_region = result.iter().find(|b| b.block_id == "by_region").unwrap();
    let overall = result.iter().find(|b| b.block_id == "overall").unwrap();
    assert_eq!(
        rows(std::slice::from_ref(by_region)),
        vec![
            vec![Cell::Integer(2), Cell::Text("North".into())],
            vec![Cell::Integer(2), Cell::Text("South".into())],
        ]
    );
    // The same customer belongs to both regions: summing counts would give 4.
    assert_eq!(
        rows(std::slice::from_ref(overall)),
        vec![vec![Cell::Integer(3)]]
    );
}

#[test]
fn normative_negative_cases_stop_before_duckdb() {
    use sef_model_ir::diagnostic::ErrorCode;
    for (case, code) in [
        (16, ErrorCode::AmbiguousRelationshipPath),
        (20, ErrorCode::IllegalTemporalReaggregation),
    ] {
        let model = common::model(case);
        let query = sef_query_ir::bind(&common::request(case), model.definition()).unwrap();
        let errors =
            sef_correctness::validate(&model, &query, &SemanticContext::default()).unwrap_err();
        assert!(
            errors.iter().any(|e| e.code == code),
            "CASE-{case:03}: {errors:?}"
        );
    }
    let mut document = common::document(17);
    let mut combined = document.metrics[0].clone();
    combined.name = "combined".into();
    combined.expression.dialects[0].expression = "revenue_usd + revenue_bob".into();
    document.metrics.push(combined);
    let definition =
        sef_ossie::csmir::to_csmir(&document, &common::catalog(17), &common::options(17)).unwrap();
    let errors = definition.compile().unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| e.code == ErrorCode::IncompatibleUnits)
    );
}

#[test]
fn capability_profile_is_inspectable_and_unknown_features_are_refused() {
    use sef_adapter_duckdb::capabilities::{CAPABILITIES, SupportLevel, capability};
    for name in CAPABILITIES {
        let (level, strategy) = capability(name);
        assert_ne!(level, SupportLevel::Unsupported, "{name}");
        assert_ne!(strategy, "");
    }
    assert_eq!(capability("arbitrary-sql").0, SupportLevel::Unsupported);
}

#[test]
fn metric_filter_order_offset_and_limit_follow_aggregation() {
    use sef_query_ir::query::{FilterExpr, FilterTarget, NullOrdering, OrderTerm};
    let model = nullable_model(1);
    let mut query = common::bound(1);
    let metric_selection = query.blocks[0].selections[0].selection_id.clone();
    query.blocks[0].post_aggregate_filter = Some(FilterExpr::Compare {
        target: FilterTarget::Metric(metric_selection.clone()),
        op: sef_model_ir::expressions::ComparisonOp::GreaterOrEqual,
        value: Literal::Decimal(
            sef_model_ir::types::DecimalValue::new(
                "200",
                sef_model_ir::types::DecimalType::new(18, 2).unwrap(),
            )
            .unwrap(),
        ),
    });
    query.blocks[0].order_by = vec![OrderTerm {
        selection_id: metric_selection,
        direction: sef_model_ir::time::SortDirection::Descending,
        nulls: NullOrdering::Last,
    }];
    query.blocks[0].limit = Some(1);
    query.blocks[0].offset = Some(1);
    let valid = sef_correctness::validate(&model, &query, &SemanticContext::default()).unwrap();
    let planned = sef_planner::plan(&model, &valid).unwrap();
    let plan = lower(
        &model,
        &valid,
        &planned.plan,
        &planned.certificate,
        &bindings(1),
    )
    .unwrap();
    let connection = Connection::open_in_memory().unwrap();
    sales(
        &connection,
        false,
        &[
            ("x", "a", Some(100)),
            ("x", "a", Some(300)),
            ("x", "b", Some(200)),
            ("x", "c", Some(300)),
            ("x", "d", Some(50)),
        ],
    );
    assert_eq!(
        rows(
            &plan
                .execute(&connection, &SemanticContext::default())
                .unwrap()
        ),
        vec![vec![Cell::Decimal(300, 38, 2), Cell::Text("c".into())]]
    );
}

#[test]
fn verified_relationship_evidence_is_rechecked_on_the_read_snapshot() {
    let mut definition = common::model(5).definition().clone();
    for relationship in &mut definition.relationships {
        relationship.evidence = sef_model_ir::relationships::ContractEvidenceState::Verified {
            evidence_id: "previous-proof".into(),
            observed_at: "2026-10-12".into(),
            source_snapshot: "previous-snapshot".into(),
        };
    }
    let plan = lower_model(
        5,
        &definition.compile().unwrap(),
        &SemanticContext::default(),
    );
    let connection = multihop_database();
    assert!(
        plan.execute(&connection, &SemanticContext::default())
            .is_ok()
    );
    connection
        .execute_batch("INSERT INTO region SELECT * FROM region LIMIT 1")
        .unwrap();
    assert!(matches!(
        plan.execute(&connection, &SemanticContext::default()),
        Err(ExecutionError::ContractViolation)
    ));
}

#[test]
fn an_unavailable_tzdb_pin_is_rejected_before_execution() {
    let mut definition = common::model(19).definition().clone();
    for dimension in &mut definition.dimensions {
        if let Some(time) = &mut dimension.time_semantics {
            time.pinned_tzdb_version = Some("2024a".into());
        }
    }
    let model = definition.compile().unwrap();
    let (valid, plan) = planned(19, &model, &SemanticContext::default());
    assert!(matches!(
        lower(&model, &valid, &plan.plan, &plan.certificate, &bindings(19)),
        Err(ExecutionError::Unsupported("pinned timezone database"))
    ));
}

#[test]
fn a_view_without_snapshot_evidence_is_refused() {
    let plan = lower_model(1, &common::model(1), &SemanticContext::default());
    let connection = fixture_database(1);
    connection.execute_batch("ALTER TABLE sales RENAME TO underlying_sales; CREATE VIEW sales AS SELECT * FROM underlying_sales").unwrap();
    assert!(matches!(
        plan.execute(&connection, &SemanticContext::default()),
        Err(ExecutionError::Unsupported("view snapshot evidence"))
    ));
}

#[test]
fn temporal_bridge_and_period_contracts_reject_real_bad_data() {
    for (case, mutation) in [
        (7, "INSERT INTO balances SELECT * FROM balances LIMIT 1"),
        (
            13,
            "INSERT INTO customer_history SELECT * FROM customer_history LIMIT 1",
        ),
        (11, "UPDATE order_campaign SET allocation_weight = 0"),
        (
            11,
            "INSERT INTO order_campaign SELECT * FROM order_campaign LIMIT 1",
        ),
        (11, "DELETE FROM orders WHERE order_id = 1"),
        (
            11,
            "UPDATE orders SET order_total = 0.01 WHERE order_id = 1",
        ),
    ] {
        let plan = lower_model(case, &common::model(case), &SemanticContext::default());
        let connection = fixture_database(case);
        connection.execute_batch(mutation).unwrap();
        assert!(
            matches!(
                plan.execute(&connection, &SemanticContext::default()),
                Err(ExecutionError::ContractViolation)
            ),
            "CASE-{case:03}, {mutation}"
        );
        assert_eq!(
            connection
                .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn a_concurrent_commit_cannot_change_the_second_block_snapshot() {
    let plan = lower_model(8, &common::model(8), &SemanticContext::default());
    let connection = fixture_database(8);
    let writer = connection.try_clone().unwrap();
    let mut wrote = false;
    let mut blocks = Vec::new();
    let receipt = plan
        .stream(
            &connection,
            &SemanticContext::default(),
            &sef_adapter_duckdb::execution::ExecutionLimits::default(),
            |block| {
                blocks.push(block);
                if !wrote {
                    writer.execute_batch("DELETE FROM activity").unwrap();
                    wrote = true;
                }
                Ok(())
            },
        )
        .unwrap();
    let overall = blocks.iter().find(|b| b.block_id == "overall").unwrap();
    assert_eq!(
        rows(std::slice::from_ref(overall)),
        vec![vec![Cell::Integer(3)]]
    );
    assert_eq!(receipt.rows, 3);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM activity", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn arrow_identity_and_context_receipt_do_not_expose_values() {
    let plan = lower_model(15, &common::model(15), &context("A"));
    let connection = fixture_database(15);
    let execute = |ctx: &SemanticContext| {
        let mut blocks = Vec::new();
        let receipt = plan
            .stream(
                &connection,
                ctx,
                &sef_adapter_duckdb::execution::ExecutionLimits::default(),
                |b| {
                    blocks.push(b);
                    Ok(())
                },
            )
            .unwrap();
        (blocks, receipt)
    };
    let (blocks, a) = execute(&context("A"));
    let (_, repeat) = execute(&context("A"));
    let (_, b) = execute(&context("B"));
    assert_eq!(
        a.context_binding_fingerprint,
        repeat.context_binding_fingerprint
    );
    assert_ne!(a.context_binding_fingerprint, b.context_binding_fingerprint);
    assert_eq!(a.certificate, *plan.certificate());
    assert_eq!(a.tzdb_version, "2025b");
    assert_eq!(a.definitions.len(), 2);
    let schema = blocks[0].batches[0].schema();
    for field in schema.fields() {
        assert!(field.metadata().contains_key("sef.selection_id"));
        assert!(field.metadata().contains_key("sef.value_type"));
        assert_eq!(field.metadata()["sef.semantic_version"], "0.1.0");
    }
    let (_, secret) = execute(&context("SECRET-TENANT-DO-NOT-EXPOSE"));
    assert!(!format!("{secret:?}").contains("SECRET-TENANT-DO-NOT-EXPOSE"));
}

#[test]
fn streaming_limits_invalidate_partial_batches_and_leave_the_connection_usable() {
    use sef_adapter_duckdb::execution::ExecutionLimits;
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE sales(region VARCHAR NOT NULL, amount DECIMAL(18,2) NOT NULL); INSERT INTO sales SELECT CAST(i AS VARCHAR), 1.00 FROM range(6000) t(i)").unwrap();
    let plan = lower_model(1, &common::model(1), &SemanticContext::default());
    for limits in [
        ExecutionLimits {
            max_rows: 2500,
            ..Default::default()
        },
        ExecutionLimits {
            max_bytes: 1,
            ..Default::default()
        },
    ] {
        let mut partial = 0;
        let result = plan.stream(&connection, &SemanticContext::default(), &limits, |b| {
            partial += b
                .batches
                .iter()
                .map(duckdb::arrow::record_batch::RecordBatch::num_rows)
                .sum::<usize>();
            Ok(())
        });
        assert!(matches!(result, Err(ExecutionError::Budget)));
        assert!(partial <= limits.max_rows);
        assert_eq!(
            connection
                .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    let limits = ExecutionLimits::default();
    let result = plan.stream(&connection, &SemanticContext::default(), &limits, |_| {
        limits
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    });
    assert!(matches!(result, Err(ExecutionError::Cancelled)));
    let deadline = ExecutionLimits {
        deadline: Some(std::time::Instant::now()),
        ..Default::default()
    };
    assert!(matches!(
        plan.stream(&connection, &SemanticContext::default(), &deadline, |_| Ok(
            ()
        )),
        Err(ExecutionError::Deadline)
    ));
    let wide = lower_model(2, &common::model(2), &SemanticContext::default());
    let bound = ExecutionLimits {
        max_intermediate_bytes: 1,
        ..Default::default()
    };
    assert!(matches!(
        wide.stream(
            &fixture_database(2),
            &SemanticContext::default(),
            &bound,
            |_| Ok(())
        ),
        Err(ExecutionError::Budget)
    ));
}

#[test]
fn physical_collation_never_merges_distinct_semantic_keys() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("SET default_collation = NOCASE; CREATE TABLE sales(region VARCHAR COLLATE NOCASE NOT NULL, amount DECIMAL(18,2) NOT NULL); INSERT INTO sales VALUES ('a', 1.00), ('A', 2.00)").unwrap();
    let plan = lower_model(1, &common::model(1), &SemanticContext::default());
    let mut result = rows(
        &plan
            .execute(&connection, &SemanticContext::default())
            .unwrap(),
    );
    result.sort();
    assert_eq!(
        result,
        vec![
            vec![Cell::Decimal(100, 38, 2), Cell::Text("a".into())],
            vec![Cell::Decimal(200, 38, 2), Cell::Text("A".into())]
        ]
    );
}

#[test]
fn decimal_precision_above_38_is_rejected_without_narrowing() {
    let mut catalog = common::catalog(1);
    catalog
        .relations
        .get_mut("sales")
        .unwrap()
        .columns
        .iter_mut()
        .find(|c| c.name == "amount")
        .unwrap()
        .value_type = ValueType::Decimal(sef_model_ir::types::DecimalType::new(39, 2).unwrap());
    let model =
        sef_ossie::csmir::compile(&common::document(1), &catalog, &common::options(1)).unwrap();
    let (valid, plan) = planned(1, &model, &SemanticContext::default());
    assert!(matches!(
        lower(&model, &valid, &plan.plan, &plan.certificate, &bindings(1)),
        Err(ExecutionError::Unsupported("exact value type"))
    ));
}

#[test]
fn physical_identifiers_are_validated_and_never_treated_as_sql() {
    let model = common::model(1);
    let (valid, plan) = planned(1, &model, &SemanticContext::default());
    let mut catalog = bindings(1);
    catalog.values_mut().next().unwrap().name = vec!["sales; DROP TABLE sales --".into()];
    assert!(matches!(
        lower(&model, &valid, &plan.plan, &plan.certificate, &catalog),
        Err(ExecutionError::InvalidIdentifier)
    ));
}

#[test]
fn changed_physical_type_never_rounds_the_semantic_value() {
    let plan = lower_model(1, &common::model(1), &SemanticContext::default());
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch("CREATE TABLE sales(region VARCHAR NOT NULL, amount DOUBLE NOT NULL)")
        .unwrap();
    assert!(matches!(
        plan.execute(&connection, &SemanticContext::default()),
        Err(ExecutionError::SourceContract)
    ));
}

#[test]
#[ignore = "local latency measurement, no performance threshold"]
fn initial_lowering_and_execution_latency() {
    let model = nullable_model(1);
    let none = SemanticContext::default();
    let (valid, semantic) = planned(1, &model, &none);
    let catalog = bindings(1);
    let plan = lower(
        &model,
        &valid,
        &semantic.plan,
        &semantic.certificate,
        &catalog,
    )
    .unwrap();
    let connection = Connection::open_in_memory().unwrap();
    let data = vec![("a", "north", Some(123)); 128];
    sales(&connection, false, &data);
    assert_eq!(
        rows(&plan.execute(&connection, &none).unwrap()),
        oracle_sales(&data, None)
    );
    let mut baseline = Vec::new();
    let mut lowering = Vec::new();
    let mut execution = Vec::new();
    for _ in 0..25 {
        let start = std::time::Instant::now();
        let mut statement = connection.prepare("SELECT CAST(SUM(amount) AS DECIMAL(18,2)) AS revenue, region FROM sales GROUP BY region").unwrap();
        let _: Vec<_> = statement.query_arrow([]).unwrap().collect();
        baseline.push(start.elapsed().as_micros());
        let start = std::time::Instant::now();
        let _ = lower(
            &model,
            &valid,
            &semantic.plan,
            &semantic.certificate,
            &catalog,
        )
        .unwrap();
        lowering.push(start.elapsed().as_micros());
        let start = std::time::Instant::now();
        let _ = plan.execute(&connection, &none).unwrap();
        execution.push(start.elapsed().as_micros());
    }
    for (name, mut samples) in [
        ("direct SQL", baseline),
        ("lower + recertify", lowering),
        ("SEIR execute + schema checks", execution),
    ] {
        samples.sort_unstable();
        println!(
            "{name}: p50={} us, p95={} us; 25 samples, 128 rows, debug profile",
            samples[12], samples[23]
        );
    }
}

#[test]
fn int64_sums_widen_to_decimal_and_timestamptz_stays_native() {
    let mut catalog = common::catalog(1);
    for column in &mut catalog.relations.get_mut("sales").unwrap().columns {
        if column.name == "amount" {
            column.value_type = ValueType::Integer {
                signed: true,
                bits: Some(64),
            };
        }
        if column.name == "region" {
            column.value_type = ValueType::Timestamp {
                timezone_semantics: sef_model_ir::time::TimestampSemantics::Instant,
            };
        }
    }
    let model =
        sef_ossie::csmir::compile(&common::document(1), &catalog, &common::options(1)).unwrap();
    let plan = lower_model(1, &model, &SemanticContext::default());
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE sales(region TIMESTAMPTZ NOT NULL, amount BIGINT NOT NULL); INSERT INTO sales VALUES ('1970-01-01 00:00:00.123456+00', 9007199254740993), ('1969-12-31 20:00:00.123456-04', 2)").unwrap();
    assert_eq!(
        rows(
            &plan
                .execute(&connection, &SemanticContext::default())
                .unwrap()
        ),
        // SUM over INT64 is certified DECIMAL(38,0) (#261): exact beyond 2^53.
        vec![vec![
            Cell::Decimal(9_007_199_254_740_995, 38, 0),
            Cell::Instant(123_456)
        ]]
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    #[test]
    fn any_context_text_is_bound_and_cannot_change_the_sql(tenant in ".{0,80}") {
        let model = nullable_model(15);
        let plan = lower_model(15, &model, &context(&tenant));
        let sql = plan.sql().next().unwrap().1.to_owned();
        let other = lower_model(15, &model, &context("another secret"));
        prop_assert_eq!(&sql, other.sql().next().unwrap().1);
        let connection = Connection::open_in_memory().unwrap();
        let data = [(tenant.as_str(), "north", Some(123)), ("different", "north", Some(456))];
        sales(&connection, true, &data);
        prop_assert_eq!(rows(&plan.execute(&connection, &context(&tenant)).unwrap()), oracle_sales(&data, Some(&tenant)));
    }
}
