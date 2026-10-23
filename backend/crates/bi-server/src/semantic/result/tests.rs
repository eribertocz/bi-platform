//! The exact result codec, on blocks the real executor produced for real
//! preparations, and on blocks rebuilt from their schema with extreme values.

use std::collections::HashMap;
use std::sync::Arc;

use bi_runtime::duckdb::arrow::array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Decimal256Array, Float64Array,
    Int64Array, LargeStringArray, StringArray, StringViewArray, TimestampMicrosecondArray,
    TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray,
};
use bi_runtime::duckdb::arrow::datatypes::{Field, Schema, i256};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;
use crate::semantic::prepare::tests::composition::{
    analytics, dimension, execute, metric, model, project, ready, request, session, state,
    zoned_model,
};

const MEMBERS: &str = "version: 1\nusers:\n  ana: [viewer]\n";

/// The blocks of a real execution, with their prepared query.
async fn executed(query: &Value) -> (Prepared, Vec<BlockResult>, tempfile::TempDir) {
    executed_on(&model("", ""), query).await
}

async fn executed_on(
    model: &str,
    query: &Value,
) -> (Prepared, Vec<BlockResult>, tempfile::TempDir) {
    let p = project(model, MEMBERS);
    let analytics = analytics().await;
    let state = state(p.path(), analytics.clone());
    let prepared = ready(&state, session("ana", &[]), &request(query)).await;
    let blocks = execute(&analytics, &prepared).await;
    (prepared, blocks, p)
}

/// The rows as the client reads them.
fn json_rows(block: &SemanticBlock) -> Vec<Vec<Value>> {
    serde_json::from_value(serde_json::to_value(&block.rows).unwrap()).unwrap()
}

/// The same block, its columns replaced by `columns` (same schema).
fn rebuilt(block: &BlockResult, columns: Vec<ArrayRef>) -> BlockResult {
    let schema = block.batches[0].schema();
    let fields: Vec<Field> = schema
        .fields()
        .iter()
        .zip(&columns)
        .map(|(field, column)| {
            Field::new(field.name(), column.data_type().clone(), true)
                .with_metadata(field.metadata().clone())
        })
        .collect();
    BlockResult {
        block_id: block.block_id.clone(),
        batches: vec![RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()],
    }
}

fn decimal_column(values: &[Option<i128>], precision: u8, scale: i8) -> ArrayRef {
    Arc::new(
        Decimal128Array::from(values.to_vec())
            .with_precision_and_scale(precision, scale)
            .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn real_blocks_are_encoded_exactly_against_the_oracle() {
    let query = json!({ "selections": [
        dimension("r", "ventas.region"), metric("t", "importe_total"), metric("f", "filas")
    ] });
    let (prepared, blocks, _p) = executed(&query).await;
    let result = encode(&prepared, &blocks).unwrap();
    let block = &result.blocks[0];
    let types: Vec<&str> = block
        .columns
        .iter()
        .map(|c| c.value_type.as_str())
        .collect();
    assert_eq!(types, ["VARCHAR", "DECIMAL(18,2)", "INT64"]);
    assert!(block.columns.iter().all(|c| !c.masked));
    // The column contract, as the client reads it.
    assert_eq!(
        serde_json::to_value(&block.columns[1]).unwrap(),
        json!({
            "selection_id": "t", "kind": "metric", "id": block.columns[1].id,
            "type": "DECIMAL(18,2)", "nullable": true, "unit": null,
            "timezone": null, "masked": false
        })
    );
    assert_eq!(block.columns[0].kind, SemanticColumnKind::Dimension);
    let mut rows = json_rows(block);
    rows.sort_by_key(|row| row[0].to_string());
    assert_eq!(
        rows,
        vec![
            vec![json!("Norte"), json!("100.30"), json!("2")],
            vec![json!("Sur"), json!("200.00"), json!("2")],
        ]
    );
    assert_eq!(block.row_count, 2);
    let totals = json!({ "selections": [metric("t", "importe_total"), metric("f", "filas")] });
    let (prepared, blocks, _p) = executed(&totals).await;
    let result = encode(&prepared, &blocks).unwrap();
    assert_eq!(
        json_rows(&result.blocks[0]),
        vec![vec![json!("300.30"), json!("4")]]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn extreme_integers_and_decimals_keep_every_digit() {
    let query = json!({ "selections": [metric("t", "importe_total"), metric("f", "filas")] });
    let (prepared, blocks, _p) = executed(&query).await;
    let schema = blocks[0].batches[0].schema();
    let Some((precision, scale)) = (match schema.field(0).data_type() {
        DataType::Decimal128(p, s) => Some((*p, *s)),
        _ => None,
    }) else {
        panic!("a DECIMAL metric");
    };
    let block = rebuilt(
        &blocks[0],
        vec![
            decimal_column(
                &[Some(-20), Some(0), Some(9_999_999_999_999_999), None],
                precision,
                scale,
            ),
            Arc::new(Int64Array::from(vec![
                Some(9_007_199_254_740_993),
                Some(i64::MIN),
                Some(i64::MAX),
                None,
            ])),
        ],
    );
    let result = encode(&prepared, &[block]).unwrap();
    assert_eq!(
        json_rows(&result.blocks[0]),
        vec![
            vec![json!("-0.20"), json!("9007199254740993")],
            vec![json!("0.00"), json!("-9223372036854775808")],
            vec![json!("99999999999999.99"), json!("9223372036854775807")],
            vec![Value::Null, Value::Null],
        ]
    );
    // Over the declared DECIMAL(18,2): refused, never rounded.
    let over = rebuilt(
        &blocks[0],
        vec![
            decimal_column(&[Some(10_i128.pow(18))], precision, scale),
            Arc::new(Int64Array::from(vec![Some(1)])),
        ],
    );
    assert_eq!(encode(&prepared, &[over]), Err(CodecError::OutOfContract));
    // Another scale is another number: refused, never rescaled.
    let rescaled = rebuilt(
        &blocks[0],
        vec![
            decimal_column(&[Some(1)], precision, scale + 1),
            Arc::new(Int64Array::from(vec![Some(1)])),
        ],
    );
    assert_eq!(
        encode(&prepared, &[rescaled]),
        Err(CodecError::SchemaMismatch("type"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_masked_column_is_never_read_and_differs_from_null() {
    let p = project(&model("", ""), MEMBERS);
    let analytics = analytics().await;
    let state = state(p.path(), analytics.clone());
    // `margen` is confidential: the viewer sees it masked.
    let both =
        request(&json!({ "selections": [metric("c", "margen"), metric("t", "importe_total")] }));
    let prepared = ready(&state, session("ana", &[]), &both).await;
    assert!(prepared.execution().is_err(), "never executed unmasked");
    let block_id = prepared.validated().query.blocks[0].block_id.clone();
    let field = |selection: &str, id: &str| {
        Field::new(selection, DataType::Decimal128(38, 2), true).with_metadata(HashMap::from([
            ("sef.selection_id".to_owned(), selection.to_owned()),
            ("sef.metric_id".to_owned(), id.to_owned()),
            ("sef.value_type".to_owned(), "DECIMAL(18,2)".to_owned()),
        ]))
    };
    let ids: Vec<String> = prepared.validated().query.blocks[0]
        .selections
        .iter()
        .map(|s| match &s.item {
            SelectionItem::Metric(id) => id.as_str().to_owned(),
            _ => panic!("metrics"),
        })
        .collect();
    // Real values in the masked column: none of them may come out, and one
    // over the declared precision would fail if it were ever read.
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![field("c", &ids[0]), field("t", &ids[1])])),
        vec![
            decimal_column(&[Some(10_i128.pow(20)), Some(777)], 38, 2),
            decimal_column(&[None, Some(1)], 38, 2),
        ],
    )
    .unwrap();
    let result = encode(
        &prepared,
        &[BlockResult {
            block_id,
            batches: vec![batch],
        }],
    )
    .unwrap();
    let block = &result.blocks[0];
    assert!(block.columns[0].masked);
    assert!(!block.columns[1].masked);
    assert_eq!(
        json_rows(block),
        vec![
            vec![Value::Null, Value::Null],
            vec![Value::Null, json!("0.01")]
        ]
    );
    let text = serde_json::to_string(&result).unwrap();
    assert!(!text.contains("1000000000000000000") && !text.contains("7.77"));
}

#[tokio::test(flavor = "multi_thread")]
async fn instants_keep_their_moment_in_utc_across_a_daylight_saving_change() {
    let query = json!({ "selections": [dimension("i", "ventas.instante"), metric("f", "filas")] });
    let (prepared, blocks, _p) = executed(&query).await;
    let result = encode(&prepared, &blocks).unwrap();
    let column = &result.blocks[0].columns[0];
    assert_eq!(column.value_type, "TIMESTAMPTZ");
    // An instant is the same moment in every zone: none is governed.
    assert_eq!(column.timezone, None);
    let mut rows = json_rows(&result.blocks[0]);
    rows.sort_by_key(|row| row[0].to_string());
    assert_eq!(
        rows,
        vec![
            vec![json!("2026-01-01T00:00:00Z"), json!("2")],
            vec![json!("2026-01-02T00:00:00Z"), json!("2")],
        ]
    );
    // Either side of the European change (2026-03-29 01:00 UTC), to the
    // microsecond: the same instants, in UTC.
    let DataType::Timestamp(_, zone) = blocks[0].batches[0].schema().field(0).data_type().clone()
    else {
        panic!("TIMESTAMPTZ");
    };
    let micros = [1_774_745_999_999_999_i64, 1_774_746_000_000_000];
    let block = rebuilt(
        &blocks[0],
        vec![
            Arc::new(TimestampMicrosecondArray::from(micros.to_vec()).with_timezone_opt(zone)),
            Arc::new(Int64Array::from(vec![Some(1), Some(1)])),
        ],
    );
    let result = encode(&prepared, &[block]).unwrap();
    assert_eq!(
        json_rows(&result.blocks[0]),
        vec![
            vec![json!("2026-03-29T00:59:59.999999Z"), json!("1")],
            vec![json!("2026-03-29T01:00:00Z"), json!("1")],
        ]
    );
    // A wall-clock time without its zone is no instant.
    let naive = rebuilt(
        &blocks[0],
        vec![
            Arc::new(TimestampMicrosecondArray::from(micros.to_vec())),
            Arc::new(Int64Array::from(vec![Some(1), Some(1)])),
        ],
    );
    assert_eq!(
        encode(&prepared, &[naive]),
        Err(CodecError::SchemaMismatch("type"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_zoned_bucket_carries_its_governed_zone() {
    let query = json!({ "selections": [dimension("m", "ventas.mes"), metric("f", "filas")] });
    let (prepared, blocks, _p) = executed_on(&zoned_model(), &query).await;
    let result = encode(&prepared, &blocks).unwrap();
    let column = &result.blocks[0].columns[0];
    assert_eq!(
        column.timezone.as_deref(),
        Some("America/La_Paz"),
        "{column:?}"
    );
    let mut rows = json_rows(&result.blocks[0]);
    rows.sort_by_key(|row| row[0].to_string());
    // 2026-01-01T00:00Z is still December in La Paz: in UTC both instants
    // would fall in January.
    assert_eq!(column.value_type, "DATE");
    assert_eq!(
        rows,
        vec![
            vec![json!("2025-12-01"), json!("2")],
            vec![json!("2026-01-01"), json!("2")],
        ]
    );
}

/// `importe` in bolivianos: its sum inherits the unit.
fn model_in_bob() -> String {
    let importe = "      - name: importe\n";
    let text = model("", "");
    assert_eq!(text.matches(importe).count(), 1);
    text.replacen(
        importe,
        &format!(
            "{importe}        custom_extensions:\n          - vendor_name: bi-platform\n            data: '{{\"version\": 1, \"unit\": {{\"currency\": \"BOB\"}}}}'\n"
        ),
        1,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_unit_is_structured_and_must_match_the_adapter() {
    let query = json!({ "selections": [metric("b", "importe_total")] });
    let (prepared, blocks, _p) = executed_on(&model_in_bob(), &query).await;
    let result = encode(&prepared, &blocks).unwrap();
    assert_eq!(
        serde_json::to_value(&result.blocks[0].columns[0].unit).unwrap(),
        json!({ "kind": "currency", "code": "BOB" })
    );
    assert_eq!(json_rows(&result.blocks[0]), vec![vec![json!("300.30")]]);
    // A block that drops or changes the unit is not this query's.
    for unit in [None, Some("Currency { iso4217: \"USD\" }")] {
        assert_eq!(
            encode(&prepared, &[relabelled(&blocks[0], 0, "sef.unit", unit)]),
            Err(CodecError::SchemaMismatch("metadata")),
            "{unit:?}"
        );
    }
}

/// The block with the metadata `key` of column `index` set to `value`, or
/// removed.
fn relabelled(block: &BlockResult, index: usize, key: &str, value: Option<&str>) -> BlockResult {
    let batch = &block.batches[0];
    let mut fields: Vec<Field> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.as_ref().clone())
        .collect();
    let mut metadata = fields[index].metadata().clone();
    match value {
        Some(value) => metadata.insert(key.to_owned(), value.to_owned()),
        None => metadata.remove(key),
    };
    fields[index] = fields[index].clone().with_metadata(metadata);
    BlockResult {
        block_id: block.block_id.clone(),
        batches: vec![
            RecordBatch::try_new(Arc::new(Schema::new(fields)), batch.columns().to_vec()).unwrap(),
        ],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn anything_outside_the_contract_is_refused_whole() {
    let query = json!({ "selections": [dimension("r", "ventas.region"), metric("f", "filas")] });
    let (prepared, blocks, _p) = executed(&query).await;
    let schema = blocks[0].batches[0].schema();
    let text: ArrayRef = Arc::new(StringArray::from(vec!["Norte"]));
    // A column missing, or one more.
    let missing = BlockResult {
        block_id: blocks[0].block_id.clone(),
        batches: vec![
            RecordBatch::try_new(
                Arc::new(Schema::new(vec![schema.field(0).clone()])),
                vec![text.clone()],
            )
            .unwrap(),
        ],
    };
    assert_eq!(
        encode(&prepared, &[missing]),
        Err(CodecError::SchemaMismatch("columns"))
    );
    // Another type than the contract's.
    let wrong = rebuilt(&blocks[0], vec![text.clone(), text.clone()]);
    assert_eq!(
        encode(&prepared, &[wrong]),
        Err(CodecError::SchemaMismatch("type"))
    );
    // Each column must say it is this selection, of this member, of this
    // type and unit: swapped, missing or foreign metadata is refused.
    let region = schema.field(0).metadata()["sef.dimension_id"].clone();
    let filas = schema.field(1).metadata()["sef.metric_id"].clone();
    for (index, key, value) in [
        (1, "sef.selection_id", Some("r")),
        (0, "sef.selection_id", None),
        (1, "sef.metric_id", Some(region.as_str())),
        (1, "sef.metric_id", None),
        (0, "sef.dimension_id", Some(filas.as_str())),
        (1, "sef.value_type", Some("DECIMAL(18,2)")),
        (0, "sef.value_type", None),
        (1, "sef.unit", Some("Ratio")),
        (1, "sef.unit", None),
        (0, "sef.unit", Some("Count")),
    ] {
        assert_eq!(
            encode(&prepared, &[relabelled(&blocks[0], index, key, value)]),
            Err(CodecError::SchemaMismatch("metadata")),
            "{index} {key} {value:?}"
        );
    }
    // A carrier with no exact form of the declared type.
    let float = rebuilt(
        &blocks[0],
        vec![text.clone(), Arc::new(Float64Array::from(vec![1.0]))],
    );
    assert_eq!(
        encode(&prepared, &[float]),
        Err(CodecError::SchemaMismatch("type"))
    );
    // A block the executor answered, lost on the way: never an empty answer.
    assert_eq!(
        encode(&prepared, &[]),
        Err(CodecError::SchemaMismatch("block"))
    );
    assert!(
        encode(
            &prepared,
            &[BlockResult {
                block_id: blocks[0].block_id.clone(),
                batches: Vec::new(),
            }]
        )
        .is_ok_and(|r| r.blocks[0].row_count == 0)
    );
    // A block the query does not have.
    let stray = BlockResult {
        block_id: "otro".to_owned(),
        batches: blocks[0].batches.clone(),
    };
    assert_eq!(
        encode(&prepared, &[stray]),
        Err(CodecError::SchemaMismatch("block"))
    );
    // A value type without an exact representation.
    assert_eq!(
        type_labels(&ValueType::Float { bits: 64 }),
        Err(CodecError::UnsupportedType)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_beyond_the_plan_limit_are_refused_not_truncated() {
    let query = json!({
        "selections": [dimension("r", "ventas.region"), metric("f", "filas")],
        "order_by": [{ "selection": "r" }],
        "limit": 1
    });
    let (prepared, blocks, _p) = executed(&query).await;
    let result = encode(&prepared, &blocks).unwrap();
    assert_eq!(result.blocks[0].limit, Some(1));
    assert_eq!(result.blocks[0].row_count, 1);
    let two = rebuilt(
        &blocks[0],
        vec![
            Arc::new(StringArray::from(vec!["Norte", "Sur"])),
            Arc::new(Int64Array::from(vec![2, 2])),
        ],
    );
    assert_eq!(encode(&prepared, &[two]), Err(CodecError::OverLimit));
}

#[tokio::test(flavor = "multi_thread")]
async fn cell_and_body_budgets_hold_to_the_byte_counting_escapes() {
    let query = json!({ "selections": [dimension("r", "ventas.region"), metric("f", "filas")] });
    let (prepared, blocks, _p) = executed(&query).await;
    // A text that escapes: each `"` and control character takes 2 or 6
    // bytes of JSON, the 'ñ' 2 bytes of UTF-8.
    let cell_of = |len: usize| -> String {
        let pattern = "\"ñ\u{1}a";
        let mut text = String::new();
        while serde_json::to_string(&text).unwrap().len() < len {
            let next = pattern.chars().nth(text.chars().count() % 4).unwrap();
            text.push(next);
        }
        text
    };
    let with = |texts: Vec<String>| {
        let count = texts.len();
        rebuilt(
            &blocks[0],
            vec![
                Arc::new(StringArray::from(texts)),
                Arc::new(Int64Array::from(vec![1; count])),
            ],
        )
    };
    // Exactly at the cell budget, and one byte more.
    let mut at = cell_of(MAX_CELL_BYTES);
    while serde_json::to_string(&at).unwrap().len() > MAX_CELL_BYTES {
        at.pop();
    }
    while serde_json::to_string(&at).unwrap().len() < MAX_CELL_BYTES {
        at.push('a');
    }
    assert_eq!(serde_json::to_string(&at).unwrap().len(), MAX_CELL_BYTES);
    assert!(encode(&prepared, &[with(vec![at.clone()])]).is_ok());
    assert_eq!(
        encode(&prepared, &[with(vec![format!("{at}a")])]),
        Err(CodecError::CellTooLarge)
    );
    // Exactly at the body budget, and one byte more: the last cell fills it.
    let mut texts = vec![at.clone(); MAX_RESULT_BYTES / (MAX_CELL_BYTES + 8)];
    texts.push(String::new());
    let size = serde_json::to_vec(&encode(&prepared, &[with(texts.clone())]).unwrap())
        .unwrap()
        .len();
    let last = texts.len() - 1;
    texts[last] = "a".repeat(MAX_RESULT_BYTES - size);
    let full = encode(&prepared, &[with(texts.clone())]).unwrap();
    assert_eq!(serde_json::to_vec(&full).unwrap().len(), MAX_RESULT_BYTES);
    texts[last].push('a');
    assert_eq!(
        encode(&prepared, &[with(texts)]),
        Err(CodecError::ResultTooLarge)
    );
}

/// The budget is checked on the body's real size, whatever the rows: a
/// budget of exactly that size is enough, and one byte less is not.
#[tokio::test(flavor = "multi_thread")]
async fn the_counted_size_is_the_encoded_size() {
    let query = json!({ "selections": [dimension("r", "ventas.region"), metric("f", "filas")] });
    let (prepared, blocks, _p) = executed(&query).await;
    let rows = proptest::collection::vec(
        (
            proptest::option::of("[a-zñ\"\\\\\u{1}\u{7f}\u{2028}😀]{0,12}"),
            proptest::option::of(any::<i64>()),
        ),
        0..130,
    );
    let mut runner = proptest::test_runner::TestRunner::new(ProptestConfig::with_cases(128));
    runner
        .run(&(rows, 1_usize..40), |(rows, chunk)| {
            // One answer per batch, as the executor emits them.
            let blocks: Vec<BlockResult> = rows
                .chunks(chunk)
                .map(|rows| {
                    let (texts, counts): (Vec<_>, Vec<_>) = rows.iter().cloned().unzip();
                    rebuilt(
                        &blocks[0],
                        vec![
                            Arc::new(StringArray::from(texts)),
                            Arc::new(Int64Array::from(counts)),
                        ],
                    )
                })
                .chain(std::iter::once(BlockResult {
                    block_id: blocks[0].block_id.clone(),
                    batches: Vec::new(),
                }))
                .collect();
            let exact = serde_json::to_vec(&encode(&prepared, &blocks).unwrap())
                .unwrap()
                .len();
            prop_assert!(encode_within(&prepared, &blocks, exact).is_ok());
            prop_assert_eq!(
                encode_within(&prepared, &blocks, exact - 1),
                Err(CodecError::ResultTooLarge)
            );
            Ok(())
        })
        .unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Every DECIMAL that fits its declared precision comes back exactly.
    #[test]
    fn decimals_round_trip_exactly(
        coefficient in -(10_i128.pow(18) - 1)..10_i128.pow(18),
        scale in 0_i8..=6,
    ) {
        let declared = ValueType::Decimal(
            sef_model_ir::types::DecimalType::new(18, u8::try_from(scale).unwrap()).unwrap(),
        );
        let SemanticCell::Text(text) = decimal(&coefficient.to_string(), scale, &declared).unwrap() else {
            panic!("text");
        };
        // Independent of the codec: integer arithmetic, not digit padding.
        let width = usize::try_from(scale).unwrap();
        let unit = 10_u128.pow(u32::try_from(scale).unwrap());
        let magnitude = coefficient.unsigned_abs();
        let sign = if coefficient < 0 { "-" } else { "" };
        let expected = if width == 0 {
            coefficient.to_string()
        } else {
            format!("{sign}{}.{:0>width$}", magnitude / unit, magnitude % unit)
        };
        prop_assert_eq!(text, expected);
    }

    /// Every INT64 comes back as its exact decimal text.
    #[test]
    fn integers_round_trip_exactly(value in any::<i64>()) {
        let array = Int64Array::from(vec![value]);
        let declared = ValueType::Integer { signed: true, bits: Some(64) };
        let Some(SemanticCell::Text(text)) = cell(&array, 0, &declared).unwrap() else {
            panic!("text");
        };
        prop_assert_eq!(text.parse::<i64>().unwrap(), value);
    }
}

fn time_dimension(id: &str, name: &str, grain: &str) -> Value {
    json!({ "kind": "time_dimension", "id": id, "name": name, "grain": grain })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_time_dimension_is_its_bucket_and_a_mismatched_one_is_refused() {
    // The La Paz months, as months: dates, with their zone.
    let query = json!({ "selections": [
        time_dimension("t", "ventas.mes", "month"), metric("f", "filas")
    ] });
    let (prepared, blocks, _p) = executed_on(&zoned_model(), &query).await;
    let result = encode(&prepared, &blocks).unwrap();
    let column = &result.blocks[0].columns[0];
    assert_eq!(column.kind, SemanticColumnKind::TimeDimension);
    assert_eq!(column.value_type, "DATE");
    assert_eq!(column.timezone.as_deref(), Some("America/La_Paz"));
    let mut rows = json_rows(&result.blocks[0]);
    rows.sort_by_key(|row| row[0].to_string());
    assert_eq!(
        rows,
        vec![
            vec![json!("2025-12-01"), json!("2")],
            vec![json!("2026-01-01"), json!("2")],
        ]
    );
    // Days of the raw instant: the adapter declares TIMESTAMPTZ and its
    // bucket delivers dates. Refused whole, never presented as what it is
    // not. The block is the one it declares, built here: running it depends
    // on the engine's ICU build (see the PR).
    let mut day = time_dimension("t", "ventas.instante", "day");
    day["timezone"] = json!("America/La_Paz");
    let p = project(&model("", ""), MEMBERS);
    let state = state(p.path(), analytics().await);
    let query = request(&json!({ "selections": [day, metric("f", "filas")] }));
    let prepared = ready(&state, session("ana", &[]), &query).await;
    let regions = json!({ "selections": [dimension("r", "ventas.region"), metric("f", "filas")] });
    let (_, real, _q) = executed(&regions).await;
    let filas = real[0].batches[0].schema().field(1).clone();
    let SelectionItem::TimeDimension(time) =
        &prepared.validated().query.blocks[0].selections[0].item
    else {
        panic!("a time dimension");
    };
    let declared = Field::new("t", DataType::Date32, true).with_metadata(HashMap::from([
        ("sef.selection_id".to_owned(), "t".to_owned()),
        (
            "sef.dimension_id".to_owned(),
            time.dimension.dimension.as_str().to_owned(),
        ),
        ("sef.value_type".to_owned(), "TIMESTAMPTZ".to_owned()),
    ]));
    let batch = RecordBatch::try_new(
        Arc::new(Schema::new(vec![declared, filas])),
        vec![
            Arc::new(Date32Array::from(vec![20_454])),
            Arc::new(Int64Array::from(vec![2])),
        ],
    )
    .unwrap();
    let block = BlockResult {
        block_id: prepared.validated().query.blocks[0].block_id.clone(),
        batches: vec![batch],
    };
    assert_eq!(
        encode(&prepared, &[block]),
        Err(CodecError::SchemaMismatch("type"))
    );
}

/// Every Arrow carrier of a value type, read directly.
#[test]
fn every_carrier_reads_its_value_exactly_or_refuses_it() {
    let text = |s: &str| Ok(Some(SemanticCell::Text(s.to_owned())));
    let utf8 = ValueType::Utf8;
    assert_eq!(
        cell(&LargeStringArray::from(vec!["ñ\"\u{1}"]), 0, &utf8),
        text("ñ\"\u{1}")
    );
    assert_eq!(cell(&StringViewArray::from(vec!["ñ"]), 0, &utf8), text("ñ"));
    // Over the cell budget before it is copied.
    let long = "a".repeat(MAX_CELL_BYTES - 1);
    assert_eq!(
        cell(&StringArray::from(vec![long.as_str()]), 0, &utf8),
        Err(CodecError::CellTooLarge)
    );
    assert_eq!(
        cell(
            &BooleanArray::from(vec![Some(true), None]),
            0,
            &ValueType::Boolean
        ),
        Ok(Some(SemanticCell::Boolean(true)))
    );
    assert_eq!(
        cell(
            &BooleanArray::from(vec![Some(true), None]),
            1,
            &ValueType::Boolean
        ),
        Ok(None)
    );
    // DECIMAL(76,2): beyond any i128.
    let wide = ValueType::Decimal(sef_model_ir::types::DecimalType::new(76, 2).unwrap());
    let digits = format!("-{}", "9".repeat(76));
    let array = Decimal256Array::from(vec![i256::from_string(&digits).unwrap()])
        .with_precision_and_scale(76, 2)
        .unwrap();
    assert_eq!(
        cell(&array, 0, &wide),
        text(&format!("-{}.99", "9".repeat(74)))
    );
    // Dates before the epoch, and the four-digit years of `YYYY-MM-DD`.
    let date = ValueType::Date;
    for (days, expected) in [
        (-1, text("1969-12-31")),
        (-719_528, text("0000-01-01")),
        (2_932_896, text("9999-12-31")),
        (-719_529, Err(CodecError::OutOfContract)),
        (2_932_897, Err(CodecError::OutOfContract)),
    ] {
        assert_eq!(
            cell(&Date32Array::from(vec![days]), 0, &date),
            expected,
            "{days}"
        );
    }
    // One instant in every unit; and none past RFC 3339's years.
    let instant = ValueType::Timestamp {
        timezone_semantics: sef_model_ir::time::TimestampSemantics::Instant,
    };
    let last = 253_402_300_799_i64;
    let zone: Option<Arc<str>> = Some("America/La_Paz".into());
    let arrays: [ArrayRef; 4] = [
        Arc::new(TimestampSecondArray::from(vec![last]).with_timezone_opt(zone.clone())),
        Arc::new(
            TimestampMillisecondArray::from(vec![last * 1_000]).with_timezone_opt(zone.clone()),
        ),
        Arc::new(
            TimestampMicrosecondArray::from(vec![last * 1_000_000]).with_timezone_opt(zone.clone()),
        ),
        Arc::new(
            TimestampNanosecondArray::from(vec![1_774_746_000_000_000_001])
                .with_timezone_opt(zone.clone()),
        ),
    ];
    let expected = [
        "9999-12-31T23:59:59Z",
        "9999-12-31T23:59:59Z",
        "9999-12-31T23:59:59Z",
        "2026-03-29T01:00:00.000000001Z",
    ];
    for (array, expected) in arrays.iter().zip(expected) {
        assert_eq!(
            cell(array.as_ref(), 0, &instant),
            text(expected),
            "{expected}"
        );
    }
    let after = TimestampSecondArray::from(vec![last + 1]).with_timezone_opt(zone);
    assert_eq!(cell(&after, 0, &instant), Err(CodecError::OutOfContract));
}
