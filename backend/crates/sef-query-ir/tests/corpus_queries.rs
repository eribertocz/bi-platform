//! The 21 normative corpus requests as SQIR (`03_MODEL_AND_QUERY_IR.md`,
//! Part II §17), bound against the corpus models and fingerprinted. The
//! frozen fingerprints change only when a query's meaning or the SQIR
//! profile changes; regenerate them deliberately with `SEF_BLESS_SQIR=1`.
#![allow(clippy::expect_used, clippy::panic)]

mod common;

use std::path::PathBuf;

use sef_model_ir::time::TimeGrain;
use sef_query_ir::query::{RequestedAtom, SemanticQuerySet};
use sef_query_ir::unbound::{
    UnboundBlock, UnboundDimension, UnboundItem, UnboundQuerySet, UnboundSelection,
};
use sef_query_ir::{QueryErrorCode, bind};

fn metric(name: &str) -> UnboundSelection {
    UnboundSelection {
        selection_id: name.to_owned(),
        item: UnboundItem::Metric(name.to_owned()),
        output_label: None,
    }
}

fn dimension(name: &str) -> UnboundSelection {
    UnboundSelection {
        selection_id: name.to_owned(),
        item: UnboundItem::Dimension(UnboundDimension {
            name: name.to_owned(),
            roles: Vec::new(),
        }),
        output_label: None,
    }
}

fn bucket(selection: &str, name: &str, grain: TimeGrain) -> UnboundSelection {
    zoned(selection, name, grain, None)
}

fn zoned(selection: &str, name: &str, grain: TimeGrain, zone: Option<&str>) -> UnboundSelection {
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

fn block(block_id: &str, selections: Vec<UnboundSelection>) -> UnboundBlock {
    UnboundBlock {
        block_id: block_id.to_owned(),
        selections,
        ..UnboundBlock::default()
    }
}

fn query(selections: Vec<UnboundSelection>) -> UnboundQuerySet {
    UnboundQuerySet {
        blocks: vec![block("result", selections)],
    }
}

/// Each normative request, in the names of its corpus model.
fn request(case: u8) -> UnboundQuerySet {
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

fn bound(case: u8) -> SemanticQuerySet {
    bind(&request(case), &common::model(case))
        .unwrap_or_else(|errors| panic!("CASE-{case:03} must bind: {errors:#?}"))
}

#[test]
fn every_valid_corpus_request_binds_with_a_frozen_fingerprint() {
    let fingerprints: Vec<String> = (1..=21)
        .filter(|&case| case != 17)
        .map(|case| {
            let fingerprint = bound(case).fingerprint().expect("canonical encoding");
            format!("CASE-{case:03} {fingerprint}")
        })
        .collect();
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/sqir-golden-hashes.txt");
    let text = fingerprints.join("\n") + "\n";
    if std::env::var_os("SEF_BLESS_SQIR").is_some() {
        std::fs::write(&golden, &text).expect("bless golden fingerprints");
    }
    let frozen = std::fs::read_to_string(&golden).expect("frozen SQIR fingerprints");
    assert_eq!(
        frozen.replace("\r\n", "\n"),
        text,
        "a corpus query changed meaning"
    );
}

#[test]
fn an_invalid_model_has_no_metric_to_bind() {
    // CASE-017: the corpus document holds only its two valid metrics; the
    // combined USD + BOB metric is rejected when the model compiles
    // (sef-ossie `csmir_rules`, sef-correctness `corpus`), so a request for
    // it cannot bind.
    let errors = bind(&request(17), &common::model(17)).expect_err("no combined metric");
    assert_eq!(errors[0].code, QueryErrorCode::UnknownMetric);
}

#[test]
fn multiple_grains_are_separate_blocks() {
    let query = bound(8);
    let grains: Vec<Vec<RequestedAtom>> = query
        .blocks
        .iter()
        .map(sef_query_ir::query::SemanticQueryBlock::requested_grain)
        .collect();
    assert_eq!(grains[0].len(), 1, "by region");
    assert!(grains[1].is_empty(), "overall is the global grain");
}

#[test]
fn a_time_bucket_is_part_of_the_requested_grain() {
    let query = bound(7);
    let grain = query.blocks[0].requested_grain();
    assert!(grain.iter().any(|atom| matches!(
        atom,
        RequestedAtom::TimeBucket(time) if time.grain == TimeGrain::Month
    )));
}

#[test]
fn time_conversion_requests_bucket_with_their_zone() {
    // §17: CASE-012 asks for calendar months in UTC; CASE-019 for days in
    // America/La_Paz. Both are time buckets with an explicit zone, so the
    // correctness engine sees the requested zone, not a prederived column.
    for (case, grain, zone) in [
        (12, TimeGrain::Month, "UTC"),
        (19, TimeGrain::Day, "America/La_Paz"),
    ] {
        let query = bound(case);
        let atoms = query.blocks[0].requested_grain();
        assert!(
            matches!(atoms.as_slice(), [RequestedAtom::TimeBucket(time)]
                if time.grain == grain
                    && time.timezone.as_ref().map(sef_model_ir::time::IanaZoneId::as_str) == Some(zone)),
            "CASE-{case:03}: {atoms:?}"
        );
    }
    // The governed dimensions still bind as plain dimensions too.
    for (case, name) in [(12, "month"), (19, "business_day")] {
        bind(
            &query(vec![metric("revenue"), dimension(name)]),
            &common::model(case),
        )
        .unwrap_or_else(|errors| panic!("CASE-{case:03}: {errors:#?}"));
    }
}

#[test]
fn binding_is_deterministic() {
    for case in (1..=21).filter(|&case| case != 17) {
        assert_eq!(
            bound(case).fingerprint().expect("encoding"),
            bound(case).fingerprint().expect("encoding"),
            "CASE-{case:03}"
        );
    }
}
