//! Nothing declared is dropped in silence (review of #72): a modifier, a
//! parameter or a contract the conversion cannot carry is rejected with its
//! code and path, never converted into a different, still valid, metric.
// Fixtures are trusted test inputs.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use sef_model_ir::identity::SourceHandle;
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, to_csmir};
use sef_ossie::{Code, Diagnostic};

const MEASURE: &str = r#"{"version":1,"grain":{"dimensions":["date"]}}"#;
const RUNNING: &str =
    r#"SUM(daily_revenue) OVER (ORDER BY "date" ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)"#;

/// The reviewer's minimal document: a daily sales dataset, its daily
/// revenue and a running total, with the parts under test replaceable.
fn document(dataset_extension: &str, running: &str, running_extension: &str) -> String {
    format!(
        r#"version: '0.2.0.dev0'
name: review
datasets:
  - name: sales_daily
    source: sales_daily
    primary_key: [date]
    custom_extensions:
      - vendor_name: bi-platform
        data: '{dataset_extension}'
    fields:
      - name: date
        expression: {{dialects: [{{dialect: OSSIE_SQL_2026, expression: '"date"'}}]}}
        dimension: {{}}
        custom_extensions:
          - {{vendor_name: bi-platform, data: '{{"version":1,"dimension":"date"}}'}}
      - name: amount
        expression: {{dialects: [{{dialect: OSSIE_SQL_2026, expression: amount}}]}}
metrics:
  - name: daily_revenue
    expression: {{dialects: [{{dialect: OSSIE_SQL_2026, expression: 'SUM(sales_daily.amount)'}}]}}
  - name: cumulative_revenue
    expression: {{dialects: [{{dialect: OSSIE_SQL_2026, expression: '{running}'}}]}}
{running_extension}"#
    )
}

fn convert(text: &str) -> Result<(), Vec<Diagnostic>> {
    let model = sef_ossie::load(text).expect("valid Ossie document").model;
    let column = |name: &str, value_type: ValueType| Column {
        name: name.to_owned(),
        handle: SourceHandle::new(name).expect("handle"),
        value_type,
        nullable: false,
    };
    let catalog = Catalog {
        relations: BTreeMap::from([(
            "sales_daily".to_owned(),
            Relation {
                source: SourceHandle::new("corpus").expect("handle"),
                relation: SourceHandle::new("sales_daily").expect("handle"),
                columns: vec![
                    column("date", ValueType::Date),
                    column(
                        "amount",
                        ValueType::Decimal(DecimalType::new(18, 2).expect("decimal")),
                    ),
                ],
            },
        )]),
    };
    let options = Options {
        namespace: "corpus".to_owned(),
        model: "case-021".to_owned(),
    };
    to_csmir(&model, &catalog, &options).map(|_| ())
}

fn rejects(text: &str, code: Code, path: &str) {
    let errors = convert(text).expect_err("must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.code == code && error.path == path),
        "expected {} at {path}, got:\n{}",
        code.as_str(),
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn with_measure(expression: &str) -> String {
    let extension = format!(
        r#"{{"version":1,"grain":{{"dimensions":["date"]}},"measures":[{{"name":"filtered","expression":"{expression}"}}]}}"#
    );
    document(&extension, RUNNING, "")
}

fn running_extension(data: &str) -> String {
    format!("    custom_extensions:\n      - vendor_name: bi-platform\n        data: '{data}'\n")
}

#[test]
fn the_reviewers_document_converts() {
    convert(&document(MEASURE, RUNNING, "")).expect("baseline");
    convert(&with_measure("SUM(amount)")).expect("a plain dataset measure");
    convert(&document(
        MEASURE,
        RUNNING,
        &running_extension(r#"{"version":1,"unit":{"currency":"USD"}}"#),
    ))
    .expect("a unit on a window metric applies to its result");
}

#[test]
fn a_dataset_measure_never_drops_its_filter() {
    // Over 10.00 and -2.00 the declared measure is -2.00; without the
    // filter it would silently be 8.00.
    rejects(
        &with_measure("SUM(amount) FILTER (WHERE amount < 0)"),
        Code::UnsupportedConstruct,
        "/datasets/0/custom_extensions",
    );
}

#[test]
fn a_dataset_measure_cannot_smuggle_a_parameter() {
    // Parameters exist only in policies; here it would vanish with the
    // filter that holds it.
    rejects(
        &with_measure("SUM(amount) FILTER (WHERE amount = :context_outside_policy)"),
        Code::UnsupportedConstruct,
        "/datasets/0/custom_extensions",
    );
}

#[test]
fn a_distinct_running_sum_is_not_a_running_sum() {
    // Two days of 10.00 give 10.00 on the second day, not 20.00.
    rejects(
        &document(
            MEASURE,
            r#"SUM(DISTINCT daily_revenue) OVER (ORDER BY "date" ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)"#,
            "",
        ),
        Code::NotRepresentable,
        "/metrics/1/expression/dialects/0/expression",
    );
}

#[test]
fn a_window_metric_rejects_contracts_it_would_lose() {
    let path = "/metrics/1/custom_extensions";
    for data in [
        r#"{"version":1,"aggregation":{"class":"non_additive","axes":[{"dimension":"date","behavior":"forbidden"}]}}"#,
        r#"{"version":1,"allocation_bridge":"missing"}"#,
        r#"{"version":1,"rounding":"half_even"}"#,
        r#"{"version":1,"grain":{"dimensions":["date"]}}"#,
    ] {
        rejects(
            &document(MEASURE, RUNNING, &running_extension(data)),
            Code::NotRepresentable,
            path,
        );
    }
}
