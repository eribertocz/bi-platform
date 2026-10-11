//! For any model our validator accepts: what we write validates against the
//! official Ossie JSON Schema, and reads back unchanged from YAML and JSON.
//! Texts are arbitrary (quotes, newlines, `yes`, `null`, `~`, Unicode…), the
//! inputs that break naive YAML writers.

// Tests fail loudly on purpose: a broken fixture or contract must stop the run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::OnceLock;

use proptest::prelude::*;
use sef_ossie::document::{
    AiContext, CustomExtension, DataType, Dataset, Dialect, DialectExpression, Dimension,
    Expression, Field, Metric, Relationship, SemanticModel, StructuredAiContext,
};
use sef_ossie::{SPEC_VERSION, parse, to_json, to_yaml, validate::validate};

fn schema() -> &'static jsonschema::Validator {
    static SCHEMA: OnceLock<jsonschema::Validator> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/ossie/ossie-schema.json")).unwrap();
        jsonschema::validator_for(&schema).unwrap()
    })
}

fn identifier() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,7}"
}

/// Any text, biased towards the strings YAML treats specially.
fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => any::<String>(),
        1 => prop::sample::select(vec![
            "yes", "no", "on", "null", "~", "true", "1.0", "0x1F", "- item", "a: b", "#",
            "'quoted'", "\"double\"", "  leading", "trailing  ", "multi\nline", "",
        ])
        .prop_map(str::to_owned),
    ]
}

fn optional_text() -> impl Strategy<Value = Option<String>> {
    prop::option::of(text())
}

fn ai_context() -> impl Strategy<Value = Option<AiContext>> {
    prop::option::of(prop_oneof![
        text().prop_map(AiContext::Text),
        (
            optional_text(),
            prop::option::of(prop::collection::vec(text(), 0..3)),
            prop::option::of(prop::collection::vec(text(), 0..3)),
        )
            .prop_map(|(instructions, synonyms, examples)| {
                AiContext::Structured(StructuredAiContext {
                    instructions,
                    synonyms,
                    examples,
                    other: std::collections::BTreeMap::new(),
                })
            }),
    ])
}

fn extensions() -> impl Strategy<Value = Vec<CustomExtension>> {
    prop::collection::vec(
        (text(), text()).prop_map(|(vendor_name, value)| CustomExtension {
            vendor_name,
            data: serde_json::json!({ "value": value }).to_string(),
        }),
        0..2,
    )
}

/// Portable dialects get valid `OSSIE_SQL_2026` for their context; vendor
/// dialects are opaque to Ossie, so any text goes.
fn expression(metric: bool) -> impl Strategy<Value = Expression> {
    let portable: Vec<&'static str> = if metric {
        vec![
            "SUM(orders.amount)",
            "COUNT(DISTINCT customers.id)",
            "SUM(amount) / COUNT(*)",
            "MAX(price) - MIN(price)",
            "AVG(\"Unit Price\")",
            "SUM(CASE WHEN status = 'done' THEN amount ELSE 0 END)",
        ]
    } else {
        vec![
            "order_id",
            "LOWER(email)",
            "amount * 2",
            "CASE WHEN x > 0 THEN 'a' ELSE 'b' END",
            "CAST(x AS DECIMAL(18, 2))",
            "DATE_TRUNC('month', ordered_at)",
        ]
    };
    let one = prop_oneof![
        (
            prop::sample::select(vec![Dialect::OssieSql2026, Dialect::AnsiSql]),
            prop::sample::select(portable),
        )
            .prop_map(|(dialect, expression)| DialectExpression {
                dialect,
                expression: expression.to_owned(),
            }),
        (
            prop::sample::select(vec![Dialect::Snowflake, Dialect::Dax]),
            text()
        )
            .prop_map(|(dialect, expression)| DialectExpression {
                dialect,
                expression
            }),
    ];
    prop::collection::vec(one, 1..3).prop_map(|dialects| Expression { dialects })
}

fn datatype() -> impl Strategy<Value = Option<DataType>> {
    prop::option::of(prop::sample::select(vec![
        DataType::String,
        DataType::Integer,
        DataType::Decimal,
        DataType::Float,
        DataType::Boolean,
        DataType::Date,
        DataType::Time,
        DataType::DateTime,
        DataType::DateTimeTz,
        DataType::Opaque,
    ]))
}

fn field(name: String) -> impl Strategy<Value = Field> {
    (
        expression(false),
        prop::option::of(prop::option::of(any::<bool>()).prop_map(|is_time| Dimension { is_time })),
        optional_text(),
        optional_text(),
        datatype(),
        ai_context(),
        extensions(),
    )
        .prop_map(
            move |(
                expression,
                dimension,
                label,
                description,
                datatype,
                ai_context,
                custom_extensions,
            )| Field {
                name: name.clone(),
                expression,
                dimension,
                label,
                description,
                datatype,
                ai_context,
                custom_extensions,
            },
        )
}

fn dataset(name: String) -> impl Strategy<Value = Dataset> {
    (
        text().prop_filter("a source is never empty", |s| !s.is_empty()),
        prop::collection::btree_set(identifier(), 0..4),
        optional_text(),
        ai_context(),
        extensions(),
    )
        .prop_flat_map(
            move |(source, field_names, description, ai_context, custom_extensions)| {
                let names: Vec<String> = field_names.into_iter().collect();
                let key = names.first().cloned().map(|first| vec![first]);
                let fields: Vec<_> = names.into_iter().map(field).collect();
                let name = name.clone();
                fields.prop_map(move |fields| Dataset {
                    name: name.clone(),
                    source: source.clone(),
                    primary_key: key.clone(),
                    unique_keys: None,
                    description: description.clone(),
                    ai_context: ai_context.clone(),
                    fields,
                    custom_extensions: custom_extensions.clone(),
                })
            },
        )
}

/// A model the validator accepts: distinct names, relationships between
/// declared datasets with paired columns.
fn model() -> impl Strategy<Value = SemanticModel> {
    (
        identifier(),
        prop::collection::btree_set(identifier(), 1..4),
        prop::collection::btree_set(identifier(), 0..3),
        optional_text(),
        ai_context(),
        extensions(),
    )
        .prop_flat_map(
            |(name, dataset_names, metric_names, description, model_context, ext)| {
                let names: Vec<String> = dataset_names.into_iter().collect();
                let datasets: Vec<_> = names.iter().cloned().map(dataset).collect();
                let metrics: Vec<_> = metric_names
                    .into_iter()
                    .map(|metric| {
                        (
                            expression(true),
                            optional_text(),
                            datatype(),
                            ai_context(),
                            extensions(),
                        )
                            .prop_map(
                                move |(
                                    expression,
                                    description,
                                    datatype,
                                    ai_context,
                                    custom_extensions,
                                )| Metric {
                                    name: metric.clone(),
                                    expression,
                                    description,
                                    datatype,
                                    ai_context,
                                    custom_extensions,
                                },
                            )
                    })
                    .collect();
                let relationships = prop::collection::vec(
                    (
                        prop::sample::select(names.clone()),
                        prop::sample::select(names.clone()),
                        prop::collection::vec((identifier(), identifier()), 1..3),
                    ),
                    0..3,
                )
                .prop_map(|links| {
                    links
                        .into_iter()
                        .enumerate()
                        .map(|(i, (from, to, columns))| Relationship {
                            name: format!("rel_{i}"),
                            from,
                            to,
                            from_columns: columns.iter().map(|(f, _)| f.clone()).collect(),
                            to_columns: columns.into_iter().map(|(_, t)| t).collect(),
                            ai_context: None,
                            custom_extensions: Vec::new(),
                        })
                        .collect::<Vec<_>>()
                });
                (
                    Just(name),
                    datasets,
                    metrics,
                    relationships,
                    Just(description),
                    Just(model_context),
                    Just(ext),
                )
            },
        )
        .prop_map(
            |(
                name,
                datasets,
                metrics,
                relationships,
                description,
                ai_context,
                custom_extensions,
            )| {
                SemanticModel {
                    version: SPEC_VERSION.to_owned(),
                    name,
                    description,
                    ai_context,
                    datasets,
                    relationships,
                    metrics,
                    custom_extensions,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn accepted_models_conform_to_the_official_schema_and_round_trip(model in model()) {
        let diagnostics = validate(&model);
        prop_assert!(
            diagnostics.iter().all(|d| !d.is_error()),
            "the generator must produce valid models: {diagnostics:?}"
        );

        let json = to_json(&model).unwrap();
        let instance: serde_json::Value = serde_json::from_str(&json).unwrap();
        let violations: Vec<String> = schema().iter_errors(&instance).map(|e| e.to_string()).collect();
        prop_assert!(violations.is_empty(), "{violations:?}\n{json}");

        prop_assert_eq!(&parse(&json).unwrap(), &model);
        let yaml = to_yaml(&model).unwrap();
        prop_assert_eq!(&parse(&yaml).unwrap(), &model, "{}", yaml);
    }
}

#[test]
fn extra_ai_context_properties_survive_and_still_conform() {
    let text = "version: 0.2.0.dev0\nname: m\nai_context:\n  instructions: use it\n  owner: finance\n  \
                tags: [a, b]\ndatasets:\n  - name: d\n    source: s.t\n";
    let model = parse(text).unwrap();
    let json = to_json(&model).unwrap();
    assert!(schema().is_valid(&serde_json::from_str(&json).unwrap()));
    assert_eq!(parse(&json).unwrap(), model);
    assert_eq!(parse(&to_yaml(&model).unwrap()).unwrap(), model);
}

#[test]
fn the_official_examples_conform_to_the_official_schema() {
    for text in [
        include_str!("fixtures/ossie/tpcds_semantic_model.yaml"),
        include_str!("fixtures/ossie/flights.semantic_model.yaml"),
    ] {
        let json = to_json(&parse(text).unwrap()).unwrap();
        assert!(schema().is_valid(&serde_json::from_str(&json).unwrap()));
    }
}

#[test]
fn present_optional_properties_reject_null_in_yaml_and_json() {
    let base = serde_json::json!({
        "version": SPEC_VERSION, "name": "m",
        "datasets": [{"name": "d", "source": "t", "fields": [{
            "name": "x", "expression": {"dialects": [{
                "dialect": "OSSIE_SQL_2026", "expression": "x"
            }]}
        }]}],
        "metrics": [{"name": "n", "expression": {"dialects": [{
            "dialect": "OSSIE_SQL_2026", "expression": "SUM(d.x)"
        }]}}],
        "relationships": [{"name": "r", "from": "d", "to": "d",
            "from_columns": ["x"], "to_columns": ["x"]}]
    });
    for (path, properties) in [
        ("", vec!["description", "ai_context"]),
        (
            "/datasets/0",
            vec!["description", "ai_context", "primary_key", "unique_keys"],
        ),
        (
            "/datasets/0/fields/0",
            vec![
                "description",
                "ai_context",
                "datatype",
                "dimension",
                "label",
            ],
        ),
        ("/metrics/0", vec!["description", "ai_context", "datatype"]),
        ("/relationships/0", vec!["ai_context"]),
    ] {
        for property in properties {
            let mut input = base.clone();
            input.pointer_mut(path).unwrap()[property] = serde_json::Value::Null;
            assert!(!schema().is_valid(&input), "{path}/{property}");
            for text in [input.to_string(), serde_saphyr::to_string(&input).unwrap()] {
                assert!(parse(&text).is_err(), "{path}/{property}: {text}");
            }
        }
    }
    for (property, value) in [
        ("dimension", serde_json::json!({"is_time": null})),
        ("ai_context", serde_json::json!({"instructions": null})),
        ("ai_context", serde_json::json!({"synonyms": null})),
        ("ai_context", serde_json::json!({"examples": null})),
    ] {
        let mut input = base.clone();
        input["datasets"][0]["fields"][0][property] = value;
        assert!(!schema().is_valid(&input));
        for text in [input.to_string(), serde_saphyr::to_string(&input).unwrap()] {
            assert!(parse(&text).is_err(), "{text}");
        }
    }
    // Unknown AI properties are schema-permitted arbitrary JSON, including null.
    let mut input = base;
    input["ai_context"] = serde_json::json!({"vendor": null});
    assert!(schema().is_valid(&input));
    let model = parse(&input.to_string()).unwrap();
    assert_eq!(serde_json::to_value(model).unwrap(), input);
}
