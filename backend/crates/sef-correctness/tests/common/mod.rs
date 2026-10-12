//! The 21 SEF corpus models, compiled from their Ossie documents, and the
//! normative request of each case bound to its model (`03_MODEL_AND_QUERY_IR.md`
//! Part II §17, as in `sef-query-ir/tests/corpus_queries.rs`).
#![allow(clippy::expect_used, clippy::panic, dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use sef_model_ir::identity::SourceHandle;
use sef_model_ir::model::CompiledSemanticModel;
use sef_model_ir::time::TimeGrain;
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, compile};
use sef_query_ir::query::SemanticQuerySet;
use sef_query_ir::unbound::{
    UnboundBlock, UnboundDimension, UnboundItem, UnboundQuerySet, UnboundSelection,
};

pub fn read(file: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../sef-ossie/tests/corpus")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn value_type(spec: &str) -> (ValueType, bool) {
    let (spec, nullable) = spec
        .strip_suffix(" null")
        .map_or((spec, false), |spec| (spec, true));
    let value_type = match spec {
        "int64" => ValueType::Integer {
            signed: true,
            bits: Some(64),
        },
        "utf8" => ValueType::Utf8,
        "date" => ValueType::Date,
        "timestamptz" => ValueType::Timestamp {
            timezone_semantics: TimestampSemantics::Instant,
        },
        decimal => {
            let (p, s) = decimal
                .strip_prefix("decimal(")
                .and_then(|rest| rest.strip_suffix(')'))
                .and_then(|rest| rest.split_once(','))
                .expect("decimal spec");
            ValueType::Decimal(
                DecimalType::new(p.parse().expect("p"), s.parse().expect("s")).expect("type"),
            )
        }
    };
    (value_type, nullable)
}

/// The compiled corpus model of `case`.
pub fn model(case: u8) -> CompiledSemanticModel {
    compiled(case).unwrap_or_else(|errors| panic!("CASE-{case:03}: {errors:#?}"))
}

/// Compiles the corpus document of `case` against its catalog.
pub fn compiled(case: u8) -> Result<CompiledSemanticModel, Vec<sef_ossie::Diagnostic>> {
    compiled_with(case, |_| {})
}

/// Compiles the corpus document of `case`, changed first by `change`.
pub fn compiled_with(
    case: u8,
    change: impl FnOnce(&mut sef_ossie::SemanticModel),
) -> Result<CompiledSemanticModel, Vec<sef_ossie::Diagnostic>> {
    let mut document = sef_ossie::load(&read(&format!("CASE-{case:03}.yaml")))
        .expect("corpus document")
        .model;
    change(&mut document);
    let relations: BTreeMap<String, BTreeMap<String, String>> =
        serde_saphyr::from_str(&read(&format!("CASE-{case:03}.catalog.yaml"))).expect("catalog");
    let catalog = Catalog {
        relations: relations
            .into_iter()
            .map(|(name, columns)| {
                let relation = Relation {
                    source: SourceHandle::new("corpus").expect("handle"),
                    relation: SourceHandle::new(name.as_str()).expect("handle"),
                    columns: columns
                        .into_iter()
                        .map(|(column, spec)| {
                            let (value_type, nullable) = value_type(&spec);
                            Column {
                                handle: SourceHandle::new(column.as_str()).expect("handle"),
                                name: column,
                                value_type,
                                nullable,
                            }
                        })
                        .collect(),
                };
                (name, relation)
            })
            .collect(),
    };
    let options = Options {
        namespace: "corpus".to_owned(),
        model: format!("case-{case:03}"),
    };
    compile(&document, &catalog, &options)
}

pub fn metric(name: &str) -> UnboundSelection {
    UnboundSelection {
        selection_id: name.to_owned(),
        item: UnboundItem::Metric(name.to_owned()),
        output_label: None,
    }
}

pub fn dimension(name: &str) -> UnboundSelection {
    UnboundSelection {
        selection_id: name.to_owned(),
        item: UnboundItem::Dimension(UnboundDimension {
            name: name.to_owned(),
            roles: Vec::new(),
        }),
        output_label: None,
    }
}

pub fn bucket(selection: &str, name: &str, grain: TimeGrain) -> UnboundSelection {
    zoned(selection, name, grain, None)
}

pub fn zoned(
    selection: &str,
    name: &str,
    grain: TimeGrain,
    zone: Option<&str>,
) -> UnboundSelection {
    UnboundSelection {
        selection_id: selection.to_owned(),
        item: UnboundItem::TimeDimension {
            dimension: UnboundDimension {
                name: name.to_owned(),
                roles: Vec::new(),
            },
            grain,
            timezone: zone.map(str::to_owned),
            calendar: None,
        },
        output_label: None,
    }
}

pub fn block(block_id: &str, selections: Vec<UnboundSelection>) -> UnboundBlock {
    UnboundBlock {
        block_id: block_id.to_owned(),
        selections,
        ..UnboundBlock::default()
    }
}

pub fn query(selections: Vec<UnboundSelection>) -> UnboundQuerySet {
    UnboundQuerySet {
        blocks: vec![block("result", selections)],
    }
}

/// Each normative request, in the names of its corpus model.
pub fn request(case: u8) -> UnboundQuerySet {
    match case {
        1 | 5 | 15 => query(vec![metric("revenue"), dimension("region")]),
        2 => query(vec![
            metric("revenue"),
            dimension("region"),
            dimension("category"),
        ]),
        3 => query(vec![metric("billed_amount"), dimension("segment")]),
        4 => query(vec![
            metric("order_revenue"),
            metric("item_count"),
            dimension("region"),
        ]),
        6 => query(vec![metric("average_order_value")]),
        7 => query(vec![
            metric("balance"),
            dimension("region"),
            bucket("month", "snapshot_date", TimeGrain::Month),
        ]),
        8 => UnboundQuerySet {
            blocks: vec![
                block(
                    "by_region",
                    vec![metric("active_customers"), dimension("region")],
                ),
                block("overall", vec![metric("active_customers")]),
            ],
        },
        9 => query(vec![
            metric("gross_revenue"),
            metric("refunds"),
            metric("net_revenue"),
            dimension("month"),
            dimension("region"),
        ]),
        10 => query(vec![
            metric("actual_revenue"),
            metric("target_revenue"),
            metric("attainment"),
            dimension("month"),
            dimension("region"),
        ]),
        11 => query(vec![metric("allocated_revenue"), dimension("campaign")]),
        // The month and business-day dimensions are the corpus's governed
        // time dimensions; the request buckets them, stating its zone.
        12 => query(vec![
            metric("revenue"),
            zoned("month", "month", TimeGrain::Month, Some("UTC")),
        ]),
        13 => query(vec![metric("revenue"), dimension("historical_region")]),
        14 => query(vec![
            metric("amount_sum"),
            metric("amount_count"),
            metric("row_count"),
        ]),
        // Without the role the correctness engine must reject the path
        // ambiguity; binding accepts the request.
        16 => query(vec![metric("revenue"), dimension("country")]),
        17 => query(vec![metric("combined_revenue")]),
        18 => query(vec![
            metric("revenue"),
            dimension("month"),
            dimension("category"),
        ]),
        19 => query(vec![
            metric("revenue"),
            zoned(
                "day",
                "business_day",
                TimeGrain::Day,
                Some("America/La_Paz"),
            ),
        ]),
        // Collapsing the snapshot axis is a temporal rejection of the
        // correctness engine, not of binding.
        20 => query(vec![metric("balance")]),
        21 => query(vec![
            metric("daily_revenue"),
            metric("cumulative_revenue"),
            dimension("date"),
        ]),
        other => panic!("no corpus case {other}"),
    }
}

/// The normative request of `case`, bound to its model.
pub fn bound(case: u8) -> SemanticQuerySet {
    sef_query_ir::bind(&request(case), model(case).definition())
        .unwrap_or_else(|errors| panic!("CASE-{case:03} must bind: {errors:#?}"))
}
