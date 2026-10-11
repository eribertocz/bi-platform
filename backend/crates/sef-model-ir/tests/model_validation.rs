//! Definition-level regressions; no query execution or correctness certification.
#![allow(clippy::expect_used)] // Helpers construct fixed, trusted test definitions.

use sef_model_ir::diagnostic::{Diagnostic, ErrorCode, Evidence, Phase};
use sef_model_ir::expressions::*;
use sef_model_ir::grain::{Grain, GrainAtom};
use sef_model_ir::identity::{ObjectIdentity, SemanticVersion, SourceHandle, StableId};
use sef_model_ir::model::*;
use sef_model_ir::policy::*;
use sef_model_ir::relationships::*;
use sef_model_ir::types::*;
use sef_model_ir::units::Unit;

fn id(kind: &str, name: &str) -> StableId {
    StableId::new(format!("test/{kind}/{name}")).expect("valid fixture ID")
}

fn metadata(kind: &str, name: &str) -> ObjectMetadata {
    ObjectMetadata::new(ObjectIdentity {
        stable_id: id(kind, name),
        semantic_version: SemanticVersion::new("1.0.0").expect("valid fixture version"),
    })
}

fn integer() -> ValueType {
    ValueType::Integer {
        signed: true,
        bits: Some(64),
    }
}
fn decimal() -> ValueType {
    ValueType::Decimal(DecimalType::new(12, 2).expect("valid decimal"))
}
fn currency(code: &str) -> Option<Unit> {
    Some(Unit::Currency {
        iso4217: code.into(),
    })
}
fn field_expr(name: &str, value_type: ValueType, unit: Option<Unit>) -> ScalarExpr {
    ScalarExpr {
        kind: ScalarExprKind::FieldRef(id("field", name)),
        value_type,
        unit,
    }
}

fn base_model() -> ModelDefinition {
    let mut model = ModelDefinition::new(metadata("model", "validation"));
    model.entities.push(Entity {
        metadata: metadata("entity", "customer"),
        keys: vec![EntityKey {
            metadata: metadata("key", "customer"),
            value_type: integer(),
            uniqueness_scope: Grain::global(),
        }],
    });
    for name in ["orders", "customers"] {
        let customer = format!("{name}-customer");
        let amount = format!("{name}-amount");
        let fields = vec![
            Field {
                metadata: metadata("field", &customer),
                dataset: id("dataset", name),
                value_type: integer(),
                unit: None,
                nullable: false,
                binding: SourceHandle::new("customer").expect("valid handle"),
            },
            Field {
                metadata: metadata("field", &amount),
                dataset: id("dataset", name),
                value_type: decimal(),
                unit: currency("USD"),
                nullable: true,
                binding: SourceHandle::new("amount").expect("valid handle"),
            },
        ];
        model.source_bindings.push(SourceBinding {
            metadata: metadata("source", name),
            source_id: SourceHandle::new("fixture").expect("valid handle"),
            relation_ref: SourceHandle::new(name).expect("valid handle"),
            field_bindings: fields
                .iter()
                .map(|f| FieldBinding {
                    field: f.metadata.id().clone(),
                    handle: f.binding.clone(),
                })
                .collect(),
        });
        model.datasets.push(SemanticDataset {
            metadata: metadata("dataset", name),
            source_binding: id("source", name),
            observation_grain: Grain::new([GrainAtom::EntityKeyRef(id("key", "customer"))]),
            fields,
            key_bindings: vec![KeyBinding {
                entity_key: id("key", "customer"),
                fields: vec![id("field", &customer)],
                unique: name == "customers",
                evidence: ContractEvidenceState::DeclaredUnverified,
            }],
            time_roles: vec![],
            policy_refs: vec![],
        });
    }
    model.relationships.push(Relationship {
        metadata: metadata("relationship", "customer"),
        from_dataset: id("dataset", "orders"),
        to_dataset: id("dataset", "customers"),
        role: "customer".into(),
        cardinality: Cardinality::ManyToOne,
        key_pairs: vec![KeyPair {
            from_field: id("field", "orders-customer"),
            to_field: id("field", "customers-customer"),
        }],
        optionality: Optionality::Required,
        temporal_constraint: None,
        evidence: ContractEvidenceState::DeclaredUnverified,
    });
    model.measures.push(Measure {
        metadata: metadata("measure", "revenue"),
        dataset: id("dataset", "orders"),
        scalar_expression: Some(field_expr("orders-amount", decimal(), currency("USD"))),
        aggregation: AggregateFunction::Sum,
        intrinsic_filter: None,
        value_type: decimal(),
        unit: currency("USD"),
        input_grain: model.datasets[0].observation_grain.clone(),
        aggregation_contract: AggregationContract::fully_additive(),
        null_semantics: AggregateNullSemantics::IgnoreNullsNullIfEmpty,
    });
    model.metrics.push(metric("revenue", "revenue", false));
    model
}

fn metric(name: &str, target: &str, metric_ref: bool) -> Metric {
    let reference = id(if metric_ref { "metric" } else { "measure" }, target);
    Metric {
        metadata: metadata("metric", name),
        expression: MetricExpr {
            kind: if metric_ref {
                MetricExprKind::MetricRef(reference.clone())
            } else {
                MetricExprKind::MeasureRef(reference.clone())
            },
            value_type: decimal(),
            unit: currency("USD"),
        },
        intrinsic_filter: None,
        value_type: decimal(),
        unit: currency("USD"),
        native_grain: Grain::global(),
        aggregation_contract: AggregationContract::fully_additive(),
        time_behavior: None,
        window_behavior: None,
        dependencies: vec![reference],
        policy_refs: vec![],
        allocation_bridge: None,
    }
}

fn errors(model: ModelDefinition) -> Vec<Diagnostic> {
    model.compile().expect_err("invalid definition must fail")
}
fn has_code(errors: &[Diagnostic], code: ErrorCode) -> bool {
    errors.iter().any(|error| error.code == code)
}

#[test]
fn declared_unverified_cardinality_compiles_without_certifying_execution() {
    let compiled = base_model()
        .compile()
        .expect("unverified declarations are valid CSMIR");
    assert_eq!(
        compiled.definition().relationships[0].evidence,
        ContractEvidenceState::DeclaredUnverified
    );
    assert!(compiled.object_hash(&id("metric", "revenue")).is_some());
}

#[test]
fn invalid_cardinality_and_uniqueness_evidence_are_rejected() {
    let invalid = ContractEvidenceState::Invalid {
        evidence_id: "failed-check".into(),
        observed_at: "2026-09-28".into(),
    };
    let mut relationship = base_model();
    relationship.relationships[0].evidence = invalid.clone();
    assert!(has_code(
        &errors(relationship),
        ErrorCode::InvalidRelationshipContract
    ));
    let mut key = base_model();
    key.datasets[1].key_bindings[0].evidence = invalid;
    assert!(has_code(
        &errors(key),
        ErrorCode::UniquenessContractViolation
    ));
}

#[test]
fn freshness_evidence_and_labels_do_not_change_semantic_hashes() {
    let original = base_model().compile().expect("valid baseline");
    let mut changed = base_model();
    let verified = ContractEvidenceState::Verified {
        evidence_id: "check".into(),
        observed_at: "2026-09-28".into(),
        source_snapshot: "snapshot".into(),
    };
    changed.relationships[0].evidence = verified.clone();
    changed.datasets[1].key_bindings[0].evidence = verified;
    changed.metrics[0].metadata.label = Some("Revenue shown to users".into());
    let compiled = changed.compile().expect("verified definition");
    assert_eq!(compiled.content_hash(), original.content_hash());
}

#[test]
fn different_currencies_coexist_until_a_definition_combines_them() {
    let mut model = base_model();
    model.datasets[1].fields[1].unit = currency("BOB");
    let mut bob = model.measures[0].clone();
    bob.metadata = metadata("measure", "bob");
    bob.dataset = id("dataset", "customers");
    bob.scalar_expression = Some(field_expr("customers-amount", decimal(), currency("BOB")));
    bob.unit = currency("BOB");
    model.measures.push(bob);
    assert!(
        model.clone().compile().is_ok(),
        "query-level combinations are deferred"
    );
    model.metrics[0].expression = MetricExpr {
        kind: MetricExprKind::Arithmetic {
            op: ArithmeticOp::Add,
            left: Box::new(model.metrics[0].expression.clone()),
            right: Box::new(MetricExpr {
                kind: MetricExprKind::MeasureRef(id("measure", "bob")),
                value_type: decimal(),
                unit: currency("BOB"),
            }),
            contract: NumericContract::default(),
        },
        value_type: ValueType::Decimal(DecimalType::new(13, 2).expect("derived type")),
        unit: currency("USD"),
    };
    model.metrics[0].value_type = model.metrics[0].expression.value_type.clone();
    model.metrics[0].dependencies.push(id("measure", "bob"));
    let diagnostics = errors(model);
    assert!(has_code(&diagnostics, ErrorCode::IncompatibleUnits));
    assert!(!has_code(&diagnostics, ErrorCode::IncompatibleTypes));
}

#[test]
fn malformed_units_are_rejected_even_without_arithmetic() {
    let mut model = base_model();
    model.datasets[1].fields[1].unit = currency("usd");
    assert!(has_code(&errors(model), ErrorCode::IncompatibleUnits));
}

#[test]
fn missing_references_have_stable_structured_locations_and_evidence() {
    let mut model = base_model();
    model.metrics[0].expression.kind = MetricExprKind::MeasureRef(id("measure", "missing"));
    model.metrics[0].dependencies = vec![id("measure", "missing")];
    let diagnostics = errors(model);
    let diagnostic = diagnostics
        .iter()
        .find(|error| error.code == ErrorCode::InvalidModelReference)
        .expect("missing reference diagnostic");
    assert_eq!(diagnostic.phase, Phase::Model);
    assert_eq!(diagnostic.primary_ref, Some(id("metric", "revenue")));
    assert_eq!(diagnostic.related_refs, vec![id("measure", "missing")]);
    assert!(
        matches!(diagnostic.evidence.first(), Some(Evidence::Reference { object, .. }) if object == &id("measure", "missing"))
    );
}

#[test]
fn duplicate_ids_and_wrong_reference_kinds_are_rejected() {
    let mut duplicate = base_model();
    duplicate.metrics.push(duplicate.metrics[0].clone());
    assert!(has_code(&errors(duplicate), ErrorCode::DuplicateStableId));
    let mut wrong_kind = base_model();
    wrong_kind.metrics[0].expression.kind = MetricExprKind::MeasureRef(id("dataset", "orders"));
    assert!(has_code(
        &errors(wrong_kind),
        ErrorCode::InvalidModelReference
    ));
}

#[test]
fn primitive_measures_cannot_hide_cross_dataset_access() {
    let mut model = base_model();
    model.measures[0].scalar_expression =
        Some(field_expr("customers-amount", decimal(), currency("USD")));
    assert!(has_code(&errors(model), ErrorCode::InvalidModelReference));
}

#[test]
fn metric_cycles_use_actual_references_even_when_dependencies_are_false() {
    let mut model = base_model();
    model.metrics = vec![
        metric("a", "b", true),
        metric("b", "a", true),
        metric("dependent", "a", true),
    ];
    model.metrics[0].dependencies.clear();
    let diagnostics = errors(model);
    let cyclic: Vec<_> = diagnostics
        .iter()
        .filter(|error| error.code == ErrorCode::MetricDependencyCycle)
        .filter_map(|error| error.primary_ref.clone())
        .collect();
    assert_eq!(cyclic, vec![id("metric", "a"), id("metric", "b")]);
    assert!(has_code(
        &diagnostics,
        ErrorCode::InvalidMetricRecomposition
    ));
}

#[test]
fn normalization_dimension_cycles_are_rejected() {
    let mut model = base_model();
    model.dimensions.push(Dimension {
        metadata: metadata("dimension", "recursive"),
        value_type: integer(),
        semantic_role: None,
        unit: None,
        time_semantics: None,
        participating_datasets: vec![id("dataset", "orders")],
        bindings: vec![DimensionBinding {
            dataset: id("dataset", "orders"),
            expression: field_expr("orders-customer", integer(), None),
            conformance: ConformanceContract::NormalizedDomain {
                normalization: ScalarExpr {
                    kind: ScalarExprKind::DimensionRef(id("dimension", "recursive")),
                    value_type: integer(),
                    unit: None,
                },
            },
        }],
    });
    assert!(has_code(&errors(model), ErrorCode::InvalidModelReference));
}

#[test]
fn literal_type_spoofing_and_implicit_rounding_are_rejected() {
    let mut spoof = base_model();
    spoof.measures[0].scalar_expression = Some(ScalarExpr {
        kind: ScalarExprKind::Literal(Literal::Utf8("not a decimal".into())),
        value_type: decimal(),
        unit: currency("USD"),
    });
    assert!(has_code(&errors(spoof), ErrorCode::IncompatibleTypes));
    let mut cast = base_model();
    cast.measures[0].scalar_expression = Some(ScalarExpr {
        kind: ScalarExprKind::ExplicitCast {
            operand: Box::new(field_expr("orders-amount", decimal(), currency("USD"))),
            rounding: None,
        },
        value_type: integer(),
        unit: currency("USD"),
    });
    cast.measures[0].value_type = integer();
    cast.metrics[0].value_type = integer();
    cast.metrics[0].expression.value_type = integer();
    assert!(has_code(&errors(cast), ErrorCode::NumericOverflow));
}

#[test]
fn metric_casts_require_explicit_rounding_and_preserve_units() {
    let mut model = base_model();
    let operand = model.metrics[0].expression.clone();
    let target = ValueType::Decimal(DecimalType::new(11, 1).expect("narrow target"));
    model.metrics[0].value_type = target.clone();
    model.metrics[0].expression = MetricExpr {
        kind: MetricExprKind::ExplicitCast {
            operand: Box::new(operand),
            rounding: None,
        },
        value_type: target,
        unit: currency("USD"),
    };
    assert!(has_code(&errors(model.clone()), ErrorCode::NumericOverflow));
    if let MetricExprKind::ExplicitCast { rounding, .. } = &mut model.metrics[0].expression.kind {
        *rounding = Some(RoundingMode::HalfEven);
    }
    assert!(model.clone().compile().is_ok());
    model.metrics[0].expression.unit = currency("BOB");
    model.metrics[0].unit = currency("BOB");
    assert!(has_code(&errors(model), ErrorCode::IncompatibleUnits));
}

#[test]
fn metric_cast_dependencies_cannot_hide_cycles() {
    let mut model = base_model();
    let mut self_reference = model.metrics[0].expression.clone();
    self_reference.kind = MetricExprKind::MetricRef(id("metric", "revenue"));
    model.metrics[0].expression.kind = MetricExprKind::ExplicitCast {
        operand: Box::new(self_reference),
        rounding: None,
    };
    model.metrics[0].dependencies = vec![id("metric", "revenue")];
    assert!(has_code(&errors(model), ErrorCode::MetricDependencyCycle));
}

#[test]
fn policy_slots_are_typed_and_cannot_appear_outside_policies() {
    let mut model = base_model();
    let slot = ScalarExpr {
        kind: ScalarExprKind::ContextSlot("customer".into()),
        value_type: integer(),
        unit: None,
    };
    let predicate = ScalarExpr {
        kind: ScalarExprKind::Comparison {
            op: ComparisonOp::Equal,
            left: Box::new(field_expr("orders-customer", integer(), None)),
            right: Box::new(slot),
        },
        value_type: ValueType::Boolean,
        unit: None,
    };
    model.policies.push(Policy {
        metadata: metadata("policy", "customer"),
        scope: PolicyScope::Dataset(id("dataset", "orders")),
        enforcement: Enforcement::Mandatory,
        predicate: predicate.clone(),
        context_slots: vec![ContextSlot {
            name: "customer".into(),
            value_type: integer(),
            required: true,
            sensitivity: Sensitivity::Sensitive,
        }],
    });
    model.datasets[0].policy_refs.push(id("policy", "customer"));
    assert!(model.clone().compile().is_ok());
    model.measures[0].intrinsic_filter = Some(predicate);
    assert!(has_code(&errors(model), ErrorCode::PolicyViolation));
}

#[test]
fn compilation_and_diagnostics_ignore_object_declaration_order() {
    let model = base_model();
    let mut reversed = model.clone();
    reversed.datasets.reverse();
    reversed.source_bindings.reverse();
    for dataset in &mut reversed.datasets {
        dataset.fields.reverse();
    }
    assert_eq!(
        model.clone().compile().expect("baseline").content_hash(),
        reversed.compile().expect("permuted").content_hash()
    );
    let mut invalid = model;
    invalid.metrics.push(metric("z", "missing", false));
    invalid.metrics.push(metric("a", "missing", false));
    let original_errors = errors(invalid.clone());
    invalid.metrics.reverse();
    invalid.datasets.reverse();
    assert_eq!(original_errors, errors(invalid));
}

#[test]
fn excessive_expression_depth_is_rejected_before_canonicalization() {
    let mut model = base_model();
    let mut expression = field_expr("orders-amount", decimal(), currency("USD"));
    for _ in 0..140 {
        expression = ScalarExpr {
            kind: ScalarExprKind::Unary {
                op: UnaryOp::Negate,
                operand: Box::new(expression),
            },
            value_type: decimal(),
            unit: currency("USD"),
        };
    }
    model.measures[0].scalar_expression = Some(expression);
    assert!(has_code(
        &errors(model),
        ErrorCode::UnsupportedSemanticConstruct
    ));
}

fn add_integer_field(model: &mut ModelDefinition, dataset_index: usize, name: &str) {
    let field = Field {
        metadata: metadata("field", name),
        dataset: model.datasets[dataset_index].metadata.id().clone(),
        value_type: integer(),
        unit: None,
        nullable: false,
        binding: SourceHandle::new(name).expect("valid field handle"),
    };
    model.source_bindings[dataset_index]
        .field_bindings
        .push(FieldBinding {
            field: field.metadata.id().clone(),
            handle: field.binding.clone(),
        });
    model.datasets[dataset_index].fields.push(field);
}

#[test]
fn composite_key_pairing_preserves_component_meaning() {
    let mut model = base_model();
    model.entities[0].keys[0].value_type = ValueType::Struct(vec![
        StructField {
            name: "customer".into(),
            value_type: integer(),
            nullable: false,
        },
        StructField {
            name: "region".into(),
            value_type: integer(),
            nullable: false,
        },
    ]);
    for (index, dataset) in ["orders", "customers"].iter().enumerate() {
        let field = format!("{dataset}-region");
        add_integer_field(&mut model, index, &field);
        model.datasets[index].key_bindings[0]
            .fields
            .push(id("field", &field));
    }
    model.relationships[0].key_pairs.push(KeyPair {
        from_field: id("field", "orders-region"),
        to_field: id("field", "customers-region"),
    });
    assert!(model.clone().compile().is_ok());
    model.relationships[0].key_pairs[0].to_field = id("field", "customers-region");
    model.relationships[0].key_pairs[1].to_field = id("field", "customers-customer");
    assert!(has_code(
        &errors(model),
        ErrorCode::InvalidRelationshipContract
    ));
}

#[test]
fn multiple_role_bindings_can_share_one_entity_domain() {
    let mut model = base_model();
    add_integer_field(&mut model, 0, "shipping-customer");
    model.datasets[0].key_bindings.push(KeyBinding {
        entity_key: id("key", "customer"),
        fields: vec![id("field", "shipping-customer")],
        unique: false,
        evidence: ContractEvidenceState::DeclaredUnverified,
    });
    let mut shipping = model.relationships[0].clone();
    shipping.metadata = metadata("relationship", "shipping");
    shipping.role = "shipping".into();
    shipping.key_pairs[0].from_field = id("field", "shipping-customer");
    model.relationships.push(shipping);
    assert!(
        model.clone().compile().is_ok(),
        "role ambiguity belongs to query correctness"
    );
    let duplicate = model.datasets[0].key_bindings[0].clone();
    model.datasets[0].key_bindings.push(duplicate);
    assert!(has_code(
        &errors(model),
        ErrorCode::InvalidRelationshipContract
    ));
}
