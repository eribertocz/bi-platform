#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../../sef-planner/tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;

use duckdb::Connection;
use duckdb::arrow::array::{
    Array, Date32Array, Decimal128Array, Int64Array, StringArray, TimestampMicrosecondArray,
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
                sum.map_or(Cell::Null, |value| Cell::Decimal(value, 18, 2)),
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
                g.map_or(Cell::Null, |v| Cell::Decimal(v, 18, 2)),
                r.map_or(Cell::Null, |v| Cell::Decimal(v, 18, 2)),
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
    for case in [2, 5] {
        let model = common::model(case);
        let (valid, plan) = self::planned(case, &model, &none);
        assert!(matches!(
            lower(
                &model,
                &valid,
                &plan.plan,
                &plan.certificate,
                &bindings(case)
            ),
            Err(ExecutionError::Unsupported(_))
        ));
    }
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
fn int64_and_timestamptz_remain_native_exact_arrow_types() {
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
        vec![vec![
            Cell::Integer(9_007_199_254_740_995),
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
