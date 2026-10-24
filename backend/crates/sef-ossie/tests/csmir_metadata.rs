//! Governance is retained and validated without changing semantic fingerprints.
#![allow(clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use sef_model_ir::diagnostic::ErrorCode;
use sef_model_ir::identity::{SourceHandle, StableId};
use sef_model_ir::model::{Classification, Governance, Lifecycle, ModelDefinition};
use sef_model_ir::types::{DecimalType, ValueType};
use sef_ossie::Code;
use sef_ossie::catalog::{Catalog, Column, Relation};
use sef_ossie::csmir::{Options, compile, to_csmir};
use sef_ossie::document::{CustomExtension, SemanticModel};
use serde_json::{Value, json};

fn fixture() -> (SemanticModel, Catalog, Options) {
    let document = sef_ossie::load(include_str!("corpus/CASE-001.yaml"))
        .expect("document")
        .model;
    let handle = |name| SourceHandle::new(name).expect("handle");
    let columns = [
        (
            "sale_id",
            ValueType::Integer {
                signed: true,
                bits: Some(64),
            },
        ),
        ("region", ValueType::Utf8),
        (
            "amount",
            ValueType::Decimal(DecimalType::new(18, 2).expect("decimal")),
        ),
    ]
    .into_iter()
    .map(|(name, value_type)| Column {
        name: name.into(),
        handle: handle(name),
        value_type,
        nullable: false,
    })
    .collect();
    let catalog = Catalog {
        relations: [(
            "sales".into(),
            Relation {
                source: handle("corpus"),
                relation: handle("sales"),
                columns,
            },
        )]
        .into(),
    };
    (
        document,
        catalog,
        Options {
            namespace: "corpus".into(),
            model: "case-001".into(),
        },
    )
}

fn extend(extensions: &mut Vec<CustomExtension>, extra: &Value) {
    if !extensions.iter().any(|e| e.vendor_name == "bi-platform") {
        extensions.push(CustomExtension {
            vendor_name: "bi-platform".into(),
            data: r#"{"version":1}"#.into(),
        });
    }
    let extension = extensions
        .iter_mut()
        .find(|e| e.vendor_name == "bi-platform")
        .expect("extension");
    let mut value: Value = serde_json::from_str(&extension.data).expect("payload");
    value
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("extra").clone());
    extension.data = value.to_string();
}

fn decorate(document: &mut SemanticModel, extra: &Value) {
    for dataset in &mut document.datasets {
        extend(&mut dataset.custom_extensions, extra);
        for field in &mut dataset.fields {
            extend(&mut field.custom_extensions, extra);
        }
    }
    for metric in &mut document.metrics {
        extend(&mut metric.custom_extensions, extra);
    }
}

fn definition(document: &SemanticModel, catalog: &Catalog, options: &Options) -> ModelDefinition {
    to_csmir(document, catalog, options).expect("conversion")
}

#[test]
fn defaults_are_active_and_internal_at_every_level() {
    let (doc, catalog, options) = fixture();
    let model = definition(&doc, &catalog, &options);
    for metadata in [
        &model.datasets[0].metadata,
        &model.datasets[0].fields[0].metadata,
        &model.dimensions[0].metadata,
        &model.metrics[0].metadata,
    ] {
        assert_eq!(metadata.governance, Governance::default());
    }
}

#[test]
fn governance_reaches_datasets_fields_dimensions_and_metrics() {
    let (mut doc, catalog, options) = fixture();
    decorate(
        &mut doc,
        &json!({"owner":"Finance", "tags":["audited", "core"],
        "status":{"state":"deprecated", "sunset":"2028-02-29"}, "sensitivity":"restricted"}),
    );
    let model = definition(&doc, &catalog, &options);
    for metadata in [
        &model.datasets[0].metadata,
        &model.datasets[0].fields[1].metadata,
        &model.dimensions[0].metadata,
        &model.metrics[0].metadata,
    ] {
        assert_eq!(metadata.governance.owner.as_deref(), Some("Finance"));
        assert_eq!(metadata.governance.tags, ["audited", "core"]);
        assert_eq!(
            metadata.governance.classification,
            Classification::Restricted
        );
        assert_eq!(
            metadata.governance.lifecycle,
            Lifecycle::Deprecated {
                sunset: Some("2028-02-29".into()),
                replacement: None,
            }
        );
    }
    model.compile().expect("valid governance");
    for (text, expected) in [
        ("public", Classification::Public),
        ("internal", Classification::Internal),
        ("confidential", Classification::Confidential),
        ("restricted", Classification::Restricted),
    ] {
        extend(
            &mut doc.metrics[0].custom_extensions,
            &json!({"sensitivity":text,"status":{"state":"active"}}),
        );
        let model = definition(&doc, &catalog, &options);
        assert_eq!(
            model.metrics[0].metadata.governance.classification,
            expected
        );
        assert_eq!(
            model.metrics[0].metadata.governance.lifecycle,
            Lifecycle::Active
        );
    }
}

#[test]
fn replacements_resolve_by_kind_and_field_scope() {
    let (mut doc, catalog, options) = fixture();
    let mut next = doc.metrics[0].clone();
    next.name = "new_revenue".into();
    doc.metrics.push(next);
    extend(
        &mut doc.metrics[0].custom_extensions,
        &json!({"status":{"state":"deprecated","replacement":"new_revenue"}}),
    );
    // A column and its default dimension each resolve the replacement in their own kind.
    doc.datasets[0].fields[0].dimension = doc.datasets[0].fields[1].dimension.clone();
    doc.datasets[0].fields[1].custom_extensions.clear();
    extend(
        &mut doc.datasets[0].fields[0].custom_extensions,
        &json!({"status":{"state":"deprecated","replacement":"region"}}),
    );
    let model = definition(&doc, &catalog, &options);
    assert_eq!(
        model.metrics[0].metadata.governance.lifecycle,
        Lifecycle::Deprecated {
            sunset: None,
            replacement: Some(StableId::new("corpus/metric/new_revenue").expect("id")),
        }
    );
    assert_eq!(
        model.datasets[0].fields[0].metadata.governance.lifecycle,
        Lifecycle::Deprecated {
            sunset: None,
            replacement: Some(StableId::new("corpus/field/sales.region").expect("id")),
        }
    );
    assert_eq!(
        model
            .dimensions
            .iter()
            .find(|d| d.metadata.id().as_str() == "corpus/dimension/sales.sale_id")
            .expect("dimension")
            .metadata
            .governance
            .lifecycle,
        Lifecycle::Deprecated {
            sunset: None,
            replacement: Some(StableId::new("corpus/dimension/sales.region").expect("id")),
        }
    );
    model.compile().expect("resolved replacements");
}

#[test]
fn dataset_replacement_and_qualified_field_replacement_resolve() {
    let (mut doc, catalog, options) = fixture();
    let mut next = doc.datasets[0].clone();
    next.name = "new_sales".into();
    extend(&mut next.custom_extensions, &json!({"entity":"new_sale"}));
    doc.datasets.push(next);
    extend(
        &mut doc.datasets[0].custom_extensions,
        &json!({"status":{"state":"deprecated","replacement":"NEW_SALES"}}),
    );
    extend(
        &mut doc.datasets[0].fields[2].custom_extensions,
        &json!({"status":{"state":"deprecated","replacement":"new_sales.amount"}}),
    );
    let model = definition(&doc, &catalog, &options);
    assert_eq!(
        model.datasets[0].metadata.governance.lifecycle,
        Lifecycle::Deprecated {
            sunset: None,
            replacement: Some(StableId::new("corpus/dataset/new_sales").expect("id")),
        }
    );
    assert_eq!(
        model.datasets[0].fields[2].metadata.governance.lifecycle,
        Lifecycle::Deprecated {
            sunset: None,
            replacement: Some(StableId::new("corpus/field/new_sales.amount").expect("id")),
        }
    );
    model.compile().expect("resolved replacements");
}

#[test]
fn conformed_dimensions_reject_conflicting_governance() {
    let (mut doc, catalog, options) = fixture();
    let mut other = doc.datasets[0].clone();
    other.name = "other_sales".into();
    extend(
        &mut other.custom_extensions,
        &json!({"entity":"other_sale"}),
    );
    extend(
        &mut other.fields[1].custom_extensions,
        &json!({"owner":"Someone else"}),
    );
    doc.datasets.push(other);
    let errors = to_csmir(&doc, &catalog, &options).expect_err("conflicting dimension owners");
    assert!(
        errors.iter().any(|e| e.code == Code::InvalidExtension
            && e.path == "/datasets/1/fields/1/custom_extensions")
    );
}

#[test]
fn invalid_governance_is_rejected_at_each_extension_level() {
    let payloads = [
        json!({"status":{"state":"retired"}}),
        json!({"sensitivity":"secret"}),
        json!({"status":{"state":"deprecated","surprise":true}}),
        json!({"surprise":true}),
        json!({"owner":42}),
        json!({"tags":"core"}),
    ];
    for level in 0..3 {
        for payload in payloads.iter().cloned().chain(
            [
                "2027-02-29",
                "2028-02-30",
                "2028-13-01",
                "2028-2-01",
                "2028-01-01T00:00:00Z",
            ]
            .map(|date| json!({"status":{"state":"deprecated","sunset":date}})),
        ) {
            let (mut doc, catalog, options) = fixture();
            let (extensions, path) = match level {
                0 => (
                    &mut doc.datasets[0].custom_extensions,
                    "/datasets/0/custom_extensions/0/data",
                ),
                1 => (
                    &mut doc.datasets[0].fields[1].custom_extensions,
                    "/datasets/0/fields/1/custom_extensions/0/data",
                ),
                _ => (
                    &mut doc.metrics[0].custom_extensions,
                    "/metrics/0/custom_extensions/0/data",
                ),
            };
            extend(extensions, &payload);
            let errors = to_csmir(&doc, &catalog, &options).expect_err("invalid extension");
            let expected_path = if payload["status"]["sunset"].is_string() {
                format!("{path}/status/sunset")
            } else {
                path.into()
            };
            assert!(
                errors
                    .iter()
                    .any(|e| e.code == Code::InvalidExtension && e.path == expected_path),
                "{errors:?}"
            );
        }
    }
}

#[test]
fn unresolved_replacements_are_model_reference_errors() {
    for level in 0..3 {
        let (mut doc, catalog, options) = fixture();
        let extensions = match level {
            0 => &mut doc.datasets[0].custom_extensions,
            1 => &mut doc.datasets[0].fields[1].custom_extensions,
            _ => &mut doc.metrics[0].custom_extensions,
        };
        extend(
            extensions,
            &json!({"status":{"state":"deprecated","replacement":"missing"}}),
        );
        let model = definition(&doc, &catalog, &options);
        let errors = model.compile().expect_err("unresolved replacement");
        assert!(
            errors
                .iter()
                .any(|e| e.code == ErrorCode::InvalidModelReference)
        );
    }
    // Direct CSMIR clients cannot bypass reference validation either.
    let (doc, catalog, options) = fixture();
    let mut model = definition(&doc, &catalog, &options);
    model.metrics[0].metadata.governance.lifecycle = Lifecycle::Deprecated {
        sunset: None,
        replacement: Some(model.datasets[0].metadata.id().clone()),
    };
    assert_eq!(
        model.compile().expect_err("wrong kind")[0].code,
        ErrorCode::InvalidModelReference
    );
}

#[test]
fn dimension_replacements_use_the_destination_dimension() {
    for qualified in [false, true] {
        let (mut doc, catalog, options) = fixture();
        doc.datasets[0].fields[0].dimension = doc.datasets[0].fields[1].dimension.clone();
        extend(
            &mut doc.datasets[0].fields[0].custom_extensions,
            &json!({"status":{"state":"deprecated","replacement": if qualified {"sales.region"} else {"region"}}}),
        );
        let model = definition(&doc, &catalog, &options);
        let dimension = model
            .dimensions
            .iter()
            .find(|d| d.metadata.id().as_str() == "corpus/dimension/sales.sale_id")
            .expect("dimension");
        assert_eq!(
            dimension.metadata.governance.lifecycle,
            Lifecycle::Deprecated {
                sunset: None,
                replacement: Some(StableId::new("corpus/dimension/region").expect("id")),
            }
        );
        model.compile().expect("conformed destination");
    }
}

#[test]
fn dimension_replacement_rejects_a_field_without_a_dimension() {
    let (mut doc, catalog, options) = fixture();
    extend(
        &mut doc.datasets[0].fields[1].custom_extensions,
        &json!({"status":{"state":"deprecated","replacement":"amount"}}),
    );
    let errors = to_csmir(&doc, &catalog, &options).expect_err("not a dimension");
    assert!(
        errors.iter().any(|e| e.code == Code::UnknownReference
            && e.message.contains("no alimenta ninguna dimensión"))
    );
}

#[test]
fn direct_csmir_rejects_self_replacement_and_cycles() {
    let (doc, catalog, options) = fixture();
    let baseline = definition(&doc, &catalog, &options);
    let mut model = baseline.clone();
    let id = model.metrics[0].metadata.id().clone();
    model.metrics[0].metadata.governance.lifecycle = Lifecycle::Deprecated {
        sunset: None,
        replacement: Some(id.clone()),
    };
    let errors = model.compile().expect_err("self replacement");
    assert!(
        errors
            .iter()
            .any(|e| e.code == ErrorCode::InvalidModelReference
                && e.primary_ref.as_ref() == Some(&id))
    );
    let mut doc = doc;
    let mut next = doc.metrics[0].clone();
    next.name = "next_revenue".into();
    doc.metrics.push(next);
    let mut model = definition(&doc, &catalog, &options);
    let next_id = model.metrics[1].metadata.id().clone();
    model.metrics[0].metadata.governance.lifecycle = Lifecycle::Deprecated {
        sunset: None,
        replacement: Some(next_id),
    };
    model
        .clone()
        .compile()
        .expect("acyclic chain to active object");
    model.metrics[1].metadata.governance.lifecycle = Lifecycle::Deprecated {
        sunset: None,
        replacement: Some(id),
    };
    let errors = model.compile().expect_err("cycle");
    assert_eq!(
        errors
            .iter()
            .filter(|e| e.code == ErrorCode::InvalidModelReference)
            .count(),
        2
    );
}

#[test]
fn direct_csmir_rejects_invalid_sunset_and_empty_or_duplicate_governance() {
    let (doc, catalog, options) = fixture();
    let baseline = definition(&doc, &catalog, &options);
    for date in ["not-a-date", "2027-02-29", "2028-02-30", "2028-2-01"] {
        let mut model = baseline.clone();
        model.metrics[0].metadata.governance.lifecycle = Lifecycle::Deprecated {
            sunset: Some(date.into()),
            replacement: None,
        };
        assert!(
            model
                .compile()
                .expect_err("invalid sunset")
                .iter()
                .any(|e| e.code == ErrorCode::InvalidTemporalContract)
        );
    }
    for payload in [
        json!({"owner":""}),
        json!({"owner":"   "}),
        json!({"tags":[""]}),
        json!({"tags":["a","a"]}),
    ] {
        let mut doc = doc.clone();
        decorate(&mut doc, &payload);
        assert!(
            to_csmir(&doc, &catalog, &options)
                .expect_err("invalid governance")
                .iter()
                .any(|e| e.code == Code::InvalidExtension)
        );
    }
    for (owner, tags) in [(Some(""), vec![]), (None, vec![""]), (None, vec!["a", "a"])] {
        let mut model = baseline.clone();
        model.metrics[0].metadata.governance.owner = owner.map(str::to_owned);
        model.metrics[0].metadata.governance.tags = tags.into_iter().map(str::to_owned).collect();
        assert!(
            model
                .compile()
                .expect_err("invalid direct governance")
                .iter()
                .any(|e| e.code == ErrorCode::InvalidCanonicalValue)
        );
    }
}

/// `metric`'s governance and that of each of its components, by label.
fn governed_family<'m>(
    model: &'m ModelDefinition,
    metric: &str,
) -> (&'m Governance, Vec<&'m Governance>) {
    let label = |m: &&sef_model_ir::model::Metric| m.metadata.label.clone().unwrap_or_default();
    let parent = model
        .metrics
        .iter()
        .find(|m| label(m) == metric)
        .expect("parent");
    let components = model
        .metrics
        .iter()
        .filter(|m| label(m).starts_with(&format!("{metric}.m")))
        .map(|m| &m.metadata.governance)
        .collect();
    (&parent.metadata.governance, components)
}

#[test]
fn components_of_a_composite_metric_inherit_its_governance() {
    let (mut doc, catalog, options) = fixture();
    let mut ratio = doc.metrics[0].clone();
    ratio.name = "ratio".into();
    ratio.expression.dialects[0].expression =
        "CAST(SUM(sales.amount) / COUNT(sales.sale_id) AS DECIMAL(18, 4))".into();
    doc.metrics.push(ratio);
    extend(
        &mut doc.metrics[1].custom_extensions,
        &json!({"sensitivity":"restricted"}),
    );
    let model = definition(&doc, &catalog, &options);
    let (parent, components) = governed_family(&model, "ratio");
    assert_eq!(parent.classification, Classification::Restricted);
    assert_eq!(components.len(), 2);
    for component in components {
        assert_eq!(component, parent);
    }
    // The undeclared metric beside it keeps the defaults.
    assert_eq!(model.metrics[0].metadata.governance, Governance::default());
}

proptest! {
    /// Whatever a composite metric declares, ratio or average, each of its
    /// components is governed exactly as it, and the model stays valid.
    #[test]
    fn any_composite_metric_governs_its_components_as_itself(
        average in any::<bool>(),
        owner in proptest::option::of("[a-zA-Z][a-zA-Z0-9 ]{0,20}"),
        tags in proptest::collection::btree_set("[a-zA-Z][a-zA-Z0-9]{0,20}", 0..8),
        deprecated in any::<bool>(), has_replacement in any::<bool>(), sensitivity in 0usize..4,
    ) {
        let (mut doc, catalog, options) = fixture();
        let mut composite = doc.metrics[0].clone();
        composite.name = "composite".into();
        composite.expression.dialects[0].expression = if average {
            "CAST(AVG(sales.amount) AS DECIMAL(18, 4))"
        } else {
            "CAST(SUM(sales.amount) / COUNT(sales.sale_id) AS DECIMAL(18, 4))"
        }
        .into();
        doc.metrics.push(composite);
        let status = if deprecated && has_replacement {
            json!({"state":"deprecated", "replacement": "revenue"})
        } else if deprecated {
            json!({"state":"deprecated"})
        } else {
            json!({"state":"active"})
        };
        extend(&mut doc.metrics[1].custom_extensions, &json!({"owner":owner, "tags":tags,
            "status":status, "sensitivity":(["public","internal","confidential","restricted"][sensitivity])}));
        let model = definition(&doc, &catalog, &options);
        let (parent, components) = governed_family(&model, "composite");
        prop_assert_eq!(components.len(), 2);
        for component in components {
            prop_assert_eq!(component, parent);
        }
        model.compile().expect("valid governance");
    }

    #[test]
    fn governance_never_changes_fingerprints(
        owner in proptest::option::of("[a-zA-Z][a-zA-Z0-9 ]{0,20}"), tags in proptest::collection::btree_set("[a-zA-Z][a-zA-Z0-9]{0,20}", 0..8),
        deprecated in any::<bool>(), sensitivity in 0usize..4, has_replacement in any::<bool>(),
        year in 1u16..10000, month in 1u8..13, day in 1u8..29, has_sunset in any::<bool>(),
    ) {
        let (mut doc, catalog, options) = fixture();
        let mut next = doc.metrics[0].clone();
        next.name = "next_revenue".into();
        doc.metrics.push(next);
        let baseline = compile(&doc, &catalog, &options).expect("baseline");
        let status = if deprecated {
            json!({"state":"deprecated", "sunset": has_sunset.then(|| format!("{year:04}-{month:02}-{day:02}"))})
        } else { json!({"state":"active"}) };
        decorate(&mut doc, &json!({"owner":owner,"tags":tags,"status":status,
            "sensitivity":(["public","internal","confidential","restricted"][sensitivity])}));
        let mut governed = definition(&doc, &catalog, &options);
        governed.metrics[1].metadata.governance.lifecycle = Lifecycle::Active;
        // Direct CSMIR clients also retain fingerprint invariance for resolved replacements.
        if deprecated && has_replacement {
            let target = governed.metrics[1].metadata.id().clone();
            if let Lifecycle::Deprecated { replacement, .. } = &mut governed.metrics[0].metadata.governance.lifecycle {
                *replacement = Some(target);
            }
        }
        let governed = governed.compile().expect("governed");
        prop_assert_eq!(baseline.content_hash(), governed.content_hash());
        let model = baseline.definition();
        for metadata in [&model.datasets[0].metadata, &model.datasets[0].fields[1].metadata,
            &model.dimensions[0].metadata, &model.metrics[0].metadata] {
            prop_assert_eq!(baseline.object_hash(metadata.id()), governed.object_hash(metadata.id()));
        }
    }
}
