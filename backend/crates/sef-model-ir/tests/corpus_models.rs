//! CSMIR representation coverage only; normative results and query errors are deferred.
// All expect calls below construct trusted, fixed test fixtures.
#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use sef_model_ir::diagnostic::ErrorCode;
use sef_model_ir::expressions::*;
use sef_model_ir::grain::{Grain, GrainAtom};
use sef_model_ir::identity::{ObjectIdentity, SemanticVersion, SourceHandle, StableId};
use sef_model_ir::model::*;
use sef_model_ir::policy::*;
use sef_model_ir::relationships::*;
use sef_model_ir::time::*;
use sef_model_ir::types::*;
use sef_model_ir::units::Unit;

fn id(kind: &str, name: &str) -> StableId {
    StableId::new(format!("corpus/{kind}/{name}")).expect("fixed fixture ID")
}

fn meta(kind: &str, name: &str) -> ObjectMetadata {
    ObjectMetadata::new(ObjectIdentity {
        stable_id: id(kind, name),
        semantic_version: SemanticVersion::new("0.1.0").expect("fixed fixture version"),
    })
}

fn field_id(dataset: &str, field: &str) -> StableId {
    id("field", &format!("{dataset}.{field}"))
}

fn integer() -> ValueType {
    ValueType::Integer {
        signed: true,
        bits: Some(64),
    }
}

fn decimal(precision: u8, scale: u8) -> ValueType {
    ValueType::Decimal(DecimalType::new(precision, scale).expect("fixed decimal type"))
}

fn amount() -> ValueType {
    decimal(18, 2)
}

fn exact(coefficient: &str, precision: u8, scale: u8) -> DecimalValue {
    DecimalValue::new(
        coefficient,
        DecimalType::new(precision, scale).expect("fixed decimal type"),
    )
    .expect("fixed exact fixture value")
}

fn dimension_atom(name: &str) -> GrainAtom {
    GrainAtom::DimensionRef {
        dimension: id("dimension", name),
        roles: Vec::new(),
    }
}

fn key_grain(name: &str) -> Grain {
    Grain::new([GrainAtom::EntityKeyRef(id("key", name))])
}

fn order(value: StableId) -> OrderTerm {
    OrderTerm {
        value,
        direction: SortDirection::Ascending,
        nulls: NullPlacement::Last,
    }
}

struct Fixture(ModelDefinition);

impl Fixture {
    fn new(case: u8) -> Self {
        let mut model = ModelDefinition::new(meta("model", &format!("case-{case:03}")));
        model.namespaces.push(Namespace {
            metadata: meta("namespace", "corpus"),
        });
        Self(model)
    }

    fn dataset(&mut self, name: &str, columns: &[(&str, ValueType)]) {
        let dataset = id("dataset", name);
        let fields: Vec<_> = columns
            .iter()
            .map(|(column, value_type)| Field {
                metadata: meta("field", &format!("{name}.{column}")),
                dataset: dataset.clone(),
                value_type: value_type.clone(),
                unit: None,
                nullable: false,
                binding: SourceHandle::new(*column).expect("fixed column handle"),
            })
            .collect();
        self.0.source_bindings.push(SourceBinding {
            metadata: meta("source", name),
            source_id: SourceHandle::new("corpus").expect("fixed source handle"),
            relation_ref: SourceHandle::new(name).expect("fixed relation handle"),
            field_bindings: fields
                .iter()
                .map(|field| FieldBinding {
                    field: field.metadata.id().clone(),
                    handle: field.binding.clone(),
                })
                .collect(),
        });
        self.0.datasets.push(SemanticDataset {
            metadata: meta("dataset", name),
            source_binding: id("source", name),
            observation_grain: Grain::global(),
            fields,
            key_bindings: Vec::new(),
            time_roles: Vec::new(),
            policy_refs: Vec::new(),
        });
    }

    fn dataset_mut(&mut self, name: &str) -> &mut SemanticDataset {
        self.0
            .datasets
            .iter_mut()
            .find(|dataset| dataset.metadata.id() == &id("dataset", name))
            .expect("fixture dataset was declared")
    }

    fn scalar(&self, dataset: &str, name: &str) -> ScalarExpr {
        let field = self
            .0
            .datasets
            .iter()
            .flat_map(|dataset| &dataset.fields)
            .find(|field| field.metadata.id() == &field_id(dataset, name))
            .expect("fixture field was declared");
        ScalarExpr {
            kind: ScalarExprKind::FieldRef(field.metadata.id().clone()),
            value_type: field.value_type.clone(),
            unit: field.unit.clone(),
        }
    }

    fn key(&mut self, dataset: &str, key: &str, columns: &[&str], unique: bool) {
        if !self
            .0
            .entities
            .iter()
            .any(|entity| entity.metadata.id() == &id("entity", key))
        {
            let components: Vec<_> = columns
                .iter()
                .map(|name| StructField {
                    name: (*name).to_owned(),
                    value_type: self.scalar(dataset, name).value_type,
                    nullable: false,
                })
                .collect();
            let value_type = if components.len() == 1 {
                components[0].value_type.clone()
            } else {
                ValueType::Struct(components)
            };
            self.0.entities.push(Entity {
                metadata: meta("entity", key),
                keys: vec![EntityKey {
                    metadata: meta("key", key),
                    value_type,
                    uniqueness_scope: Grain::global(),
                }],
            });
        }
        self.dataset_mut(dataset).key_bindings.push(KeyBinding {
            entity_key: id("key", key),
            fields: columns
                .iter()
                .map(|column| field_id(dataset, column))
                .collect(),
            unique,
            evidence: ContractEvidenceState::DeclaredUnverified,
        });
    }

    fn primary_key(&mut self, dataset: &str, key: &str, columns: &[&str]) {
        self.key(dataset, key, columns, true);
        self.dataset_mut(dataset).observation_grain = key_grain(key);
    }

    fn dimension(&mut self, name: &str, bindings: &[(&str, &str)]) {
        let first = self.scalar(bindings[0].0, bindings[0].1);
        let expressions = bindings
            .iter()
            .map(|(dataset, field)| DimensionBinding {
                dataset: id("dataset", dataset),
                expression: self.scalar(dataset, field),
                conformance: ConformanceContract::ExactDomain,
            })
            .collect();
        self.0.dimensions.push(Dimension {
            metadata: meta("dimension", name),
            value_type: first.value_type,
            unit: first.unit,
            semantic_role: None,
            time_semantics: None,
            participating_datasets: bindings
                .iter()
                .map(|(dataset, _)| id("dataset", dataset))
                .collect(),
            bindings: expressions,
        });
    }

    fn temporal_dimension(
        &mut self,
        name: &str,
        dataset: &str,
        field: &str,
        timezone: &str,
        grain: TimeGrain,
    ) {
        let timezone = IanaZoneId::new(timezone).expect("fixed zone");
        let bucket = TimeBucket {
            source: field_id(dataset, field),
            timezone: timezone.clone(),
            calendar: CalendarProfile::Gregorian,
            grain,
        };
        self.0.dimensions.push(Dimension {
            metadata: meta("dimension", name),
            value_type: ValueType::Date,
            unit: None,
            semantic_role: Some("business_time".into()),
            time_semantics: Some(TimeSemantics {
                role: TimeRole::EventTime,
                timezone,
                calendar: CalendarProfile::Gregorian,
                pinned_tzdb_version: Some("2025b".into()),
            }),
            participating_datasets: vec![id("dataset", dataset)],
            bindings: vec![DimensionBinding {
                dataset: id("dataset", dataset),
                expression: ScalarExpr {
                    kind: ScalarExprKind::TemporalDerivation {
                        operand: Box::new(self.scalar(dataset, field)),
                        bucket,
                    },
                    value_type: ValueType::Date,
                    unit: None,
                },
                conformance: ConformanceContract::ExactDomain,
            }],
        });
    }

    fn time_role(&mut self, dataset: &str, field: &str, role: TimeRole) {
        self.dataset_mut(dataset).time_roles.push(DatasetTimeRole {
            field: field_id(dataset, field),
            role,
        });
    }

    fn relationship(&mut self, name: &str, from: (&str, &str), to: (&str, &str)) {
        self.0.relationships.push(Relationship {
            metadata: meta("relationship", name),
            from_dataset: id("dataset", from.0),
            to_dataset: id("dataset", to.0),
            role: name.to_owned(),
            cardinality: Cardinality::ManyToOne,
            key_pairs: vec![KeyPair {
                from_field: field_id(from.0, from.1),
                to_field: field_id(to.0, to.1),
            }],
            optionality: Optionality::Required,
            temporal_constraint: None,
            evidence: ContractEvidenceState::DeclaredUnverified,
        });
    }

    fn measure(
        &mut self,
        name: &str,
        dataset: &str,
        expression: Option<ScalarExpr>,
        aggregation: AggregateFunction,
        contract: AggregationContract,
    ) {
        let count = matches!(
            aggregation,
            AggregateFunction::CountRows
                | AggregateFunction::CountNonNull
                | AggregateFunction::CountDistinct
        );
        let (value_type, unit) = if count {
            (integer(), Some(Unit::Count))
        } else {
            let expression = expression.as_ref().expect("sum fixture has expression");
            (expression.value_type.clone(), expression.unit.clone())
        };
        let null_semantics = match aggregation {
            AggregateFunction::CountRows => AggregateNullSemantics::CountRows,
            AggregateFunction::CountNonNull | AggregateFunction::CountDistinct => {
                AggregateNullSemantics::CountNonNull
            }
            _ => AggregateNullSemantics::IgnoreNullsNullIfEmpty,
        };
        let input_grain = self.dataset_mut(dataset).observation_grain.clone();
        self.0.measures.push(Measure {
            metadata: meta("measure", name),
            dataset: id("dataset", dataset),
            scalar_expression: expression,
            aggregation,
            intrinsic_filter: None,
            value_type,
            unit,
            input_grain,
            aggregation_contract: contract,
            null_semantics,
        });
    }

    fn metric(
        &mut self,
        name: &str,
        expression: MetricExpr,
        native_grain: Grain,
        contract: AggregationContract,
    ) {
        let dependencies = expression
            .measure_refs()
            .into_iter()
            .chain(expression.metric_refs())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.0.metrics.push(Metric {
            metadata: meta("metric", name),
            value_type: expression.value_type.clone(),
            unit: expression.unit.clone(),
            expression,
            intrinsic_filter: None,
            native_grain,
            aggregation_contract: contract,
            time_behavior: None,
            window_behavior: None,
            dependencies,
            policy_refs: Vec::new(),
            allocation_bridge: None,
        });
    }

    fn metric_ref(&self, name: &str) -> MetricExpr {
        let metric = self
            .0
            .metrics
            .iter()
            .find(|metric| metric.metadata.id() == &id("metric", name))
            .expect("fixture metric was declared");
        MetricExpr {
            kind: MetricExprKind::MetricRef(id("metric", name)),
            value_type: metric.value_type.clone(),
            unit: metric.unit.clone(),
        }
    }

    fn measured_metric(&mut self, name: &str) {
        let measure = self
            .0
            .measures
            .iter()
            .find(|measure| measure.metadata.id() == &id("measure", name))
            .expect("fixture measure was declared");
        self.metric(
            name,
            MetricExpr {
                kind: MetricExprKind::MeasureRef(id("measure", name)),
                value_type: measure.value_type.clone(),
                unit: measure.unit.clone(),
            },
            measure.input_grain.clone(),
            measure.aggregation_contract.clone(),
        );
    }

    fn sum(&mut self, name: &str, dataset: &str, field: &str) {
        self.measure(
            name,
            dataset,
            Some(self.scalar(dataset, field)),
            AggregateFunction::Sum,
            AggregationContract::fully_additive(),
        );
        self.measured_metric(name);
    }

    fn ratio(
        &mut self,
        name: &str,
        numerator: &str,
        denominator: &str,
        grain: Grain,
        unit: Option<Unit>,
    ) {
        let numerator_expr = self.metric_ref(numerator);
        let mut denominator_expr = self.metric_ref(denominator);
        if matches!(denominator_expr.value_type, ValueType::Integer { .. }) {
            denominator_expr = MetricExpr {
                value_type: decimal(20, 0),
                unit: denominator_expr.unit.clone(),
                kind: MetricExprKind::ExplicitCast {
                    operand: Box::new(denominator_expr),
                    rounding: None,
                },
            };
        }
        let expression = MetricExpr {
            kind: MetricExprKind::Ratio {
                numerator: Box::new(numerator_expr),
                denominator: Box::new(denominator_expr),
                zero_denominator: ZeroDivisionPolicy::Null,
                rounding: Some(RoundingMode::HalfEven),
            },
            value_type: amount(),
            unit,
        };
        let contract = AggregationContract {
            classification: AggregationClass::Recomputable,
            axis_rules: Vec::new(),
            decomposition: Some(Decomposition {
                components: vec![id("metric", numerator), id("metric", denominator)],
                recomposition: expression.clone(),
            }),
        };
        self.metric(name, expression, grain, contract);
    }
}

fn case_001() -> ModelDefinition {
    let mut f = Fixture::new(1);
    f.dataset(
        "sales",
        &[
            ("sale_id", integer()),
            ("region", ValueType::Utf8),
            ("amount", amount()),
        ],
    );
    f.primary_key("sales", "sale", &["sale_id"]);
    f.dimension("region", &[("sales", "region")]);
    f.sum("revenue", "sales", "amount");
    f.0
}

fn case_002() -> ModelDefinition {
    let mut f = Fixture::new(2);
    f.dataset(
        "sales",
        &[
            ("line_id", integer()),
            ("customer_id", integer()),
            ("product_id", integer()),
            ("quantity", integer()),
            ("unit_price", amount()),
        ],
    );
    f.dataset(
        "customers",
        &[("customer_id", integer()), ("region", ValueType::Utf8)],
    );
    f.dataset(
        "products",
        &[("product_id", integer()), ("category", ValueType::Utf8)],
    );
    f.primary_key("sales", "line", &["line_id"]);
    f.primary_key("customers", "customer", &["customer_id"]);
    f.primary_key("products", "product", &["product_id"]);
    f.key("sales", "customer", &["customer_id"], false);
    f.key("sales", "product", &["product_id"], false);
    f.relationship(
        "customer",
        ("sales", "customer_id"),
        ("customers", "customer_id"),
    );
    f.relationship(
        "product",
        ("sales", "product_id"),
        ("products", "product_id"),
    );
    f.dimension("region", &[("customers", "region")]);
    f.dimension("category", &[("products", "category")]);
    let quantity = ScalarExpr {
        kind: ScalarExprKind::ExplicitCast {
            operand: Box::new(f.scalar("sales", "quantity")),
            rounding: None,
        },
        value_type: decimal(20, 0),
        unit: None,
    };
    let revenue = ScalarExpr {
        kind: ScalarExprKind::Arithmetic {
            op: ArithmeticOp::Multiply,
            left: Box::new(quantity),
            right: Box::new(f.scalar("sales", "unit_price")),
            contract: NumericContract::default(),
        },
        value_type: decimal(39, 2),
        unit: None,
    };
    f.measure(
        "revenue",
        "sales",
        Some(revenue),
        AggregateFunction::Sum,
        AggregationContract::fully_additive(),
    );
    f.measured_metric("revenue");
    f.0
}

fn case_003() -> ModelDefinition {
    let mut f = Fixture::new(3);
    f.dataset(
        "invoices",
        &[
            ("invoice_id", integer()),
            ("customer_id", integer()),
            ("amount", amount()),
        ],
    );
    f.dataset(
        "customer",
        &[("customer_id", integer()), ("segment", ValueType::Utf8)],
    );
    f.primary_key("invoices", "invoice", &["invoice_id"]);
    f.primary_key("customer", "customer", &["customer_id"]);
    f.key("invoices", "customer", &["customer_id"], false);
    f.relationship(
        "customer",
        ("invoices", "customer_id"),
        ("customer", "customer_id"),
    );
    f.dimension("segment", &[("customer", "segment")]);
    f.sum("billed_amount", "invoices", "amount");
    f.0
}

fn case_004() -> ModelDefinition {
    let mut f = Fixture::new(4);
    f.dataset(
        "orders",
        &[
            ("order_id", integer()),
            ("customer_id", integer()),
            ("order_total", amount()),
        ],
    );
    f.dataset(
        "order_items",
        &[("item_id", integer()), ("order_id", integer())],
    );
    f.dataset(
        "customers",
        &[("customer_id", integer()), ("region", ValueType::Utf8)],
    );
    f.primary_key("orders", "order", &["order_id"]);
    f.primary_key("order_items", "item", &["item_id"]);
    f.primary_key("customers", "customer", &["customer_id"]);
    f.key("order_items", "order", &["order_id"], false);
    f.key("orders", "customer", &["customer_id"], false);
    f.relationship("order", ("order_items", "order_id"), ("orders", "order_id"));
    f.relationship(
        "customer",
        ("orders", "customer_id"),
        ("customers", "customer_id"),
    );
    f.dimension("region", &[("customers", "region")]);
    f.sum("order_revenue", "orders", "order_total");
    f.measure(
        "item_count",
        "order_items",
        Some(f.scalar("order_items", "item_id")),
        AggregateFunction::CountNonNull,
        AggregationContract::fully_additive(),
    );
    f.measured_metric("item_count");
    assert_ne!(f.0.measures[0].input_grain, f.0.measures[1].input_grain);
    f.0
}

fn case_005() -> ModelDefinition {
    let mut f = Fixture::new(5);
    f.dataset(
        "sales",
        &[
            ("sale_id", integer()),
            ("store_id", integer()),
            ("amount", amount()),
        ],
    );
    f.dataset(
        "store",
        &[("store_id", integer()), ("region_id", integer())],
    );
    f.dataset(
        "region",
        &[("region_id", integer()), ("region_name", ValueType::Utf8)],
    );
    f.primary_key("sales", "sale", &["sale_id"]);
    f.primary_key("store", "store", &["store_id"]);
    f.primary_key("region", "region", &["region_id"]);
    f.key("sales", "store", &["store_id"], false);
    f.key("store", "region", &["region_id"], false);
    f.relationship("store", ("sales", "store_id"), ("store", "store_id"));
    f.relationship("region", ("store", "region_id"), ("region", "region_id"));
    f.dimension("region", &[("region", "region_name")]);
    f.sum("revenue", "sales", "amount");
    f.0
}

fn case_006() -> ModelDefinition {
    let mut f = Fixture::new(6);
    f.dataset(
        "orders",
        &[
            ("order_id", integer()),
            ("region", ValueType::Utf8),
            ("amount", amount()),
        ],
    );
    f.primary_key("orders", "order", &["order_id"]);
    f.dimension("region", &[("orders", "region")]);
    f.sum("revenue", "orders", "amount");
    f.measure(
        "order_count",
        "orders",
        Some(f.scalar("orders", "order_id")),
        AggregateFunction::CountNonNull,
        AggregationContract::fully_additive(),
    );
    f.measured_metric("order_count");
    f.ratio(
        "average_order_value",
        "revenue",
        "order_count",
        key_grain("order"),
        None,
    );
    assert_eq!(
        f.0.metrics[2].aggregation_contract.classification,
        AggregationClass::Recomputable
    );
    f.0
}

fn case_007() -> ModelDefinition {
    let mut f = Fixture::new(7);
    f.dataset(
        "balances",
        &[
            ("account_id", integer()),
            ("snapshot_date", ValueType::Date),
            ("balance", amount()),
        ],
    );
    f.dataset(
        "accounts",
        &[("account_id", integer()), ("region", ValueType::Utf8)],
    );
    f.primary_key(
        "balances",
        "account_snapshot",
        &["account_id", "snapshot_date"],
    );
    f.primary_key("accounts", "account", &["account_id"]);
    f.key("balances", "account", &["account_id"], false);
    f.relationship(
        "account",
        ("balances", "account_id"),
        ("accounts", "account_id"),
    );
    f.dimension("region", &[("accounts", "region")]);
    f.dimension("snapshot_date", &[("balances", "snapshot_date")]);
    f.time_role("balances", "snapshot_date", TimeRole::SnapshotTime);
    let axis = dimension_atom("snapshot_date");
    let contract = AggregationContract {
        classification: AggregationClass::SemiAdditive,
        axis_rules: vec![
            AxisRule {
                axis: axis.clone(),
                behavior: AxisBehavior::PeriodEnd,
            },
            AxisRule {
                axis: GrainAtom::EntityKeyRef(id("key", "account")),
                behavior: AxisBehavior::Additive,
            },
        ],
        decomposition: None,
    };
    f.measure(
        "balance",
        "balances",
        Some(f.scalar("balances", "balance")),
        AggregateFunction::Sum,
        contract,
    );
    f.measured_metric("balance");
    f.0.metrics[0].time_behavior = Some(TemporalAggregation {
        axis,
        rule: TemporalSelection::PeriodEnd,
        order_by: vec![order(field_id("balances", "snapshot_date"))],
        selection_scope: key_grain("account"),
        ties: TemporalTies::UniqueByContract {
            key: id("key", "account_snapshot"),
        },
    });
    f.0
}

fn case_008() -> ModelDefinition {
    let mut f = Fixture::new(8);
    f.dataset(
        "activity",
        &[
            ("activity_id", integer()),
            ("customer_id", integer()),
            ("region", ValueType::Utf8),
        ],
    );
    f.primary_key("activity", "activity", &["activity_id"]);
    f.key("activity", "customer", &["customer_id"], false);
    f.dimension("region", &[("activity", "region")]);
    let contract = AggregationContract {
        classification: AggregationClass::NonAdditive,
        axis_rules: vec![AxisRule {
            axis: dimension_atom("region"),
            behavior: AxisBehavior::DistinctRecompute,
        }],
        decomposition: None,
    };
    f.measure(
        "active_customers",
        "activity",
        Some(f.scalar("activity", "customer_id")),
        AggregateFunction::CountDistinct,
        contract,
    );
    f.measured_metric("active_customers");
    f.0
}

fn case_009() -> ModelDefinition {
    let mut f = Fixture::new(9);
    for dataset in ["sales_monthly", "refunds_monthly"] {
        f.dataset(
            dataset,
            &[
                ("month", ValueType::Date),
                ("region", ValueType::Utf8),
                ("amount", amount()),
            ],
        );
        f.primary_key(dataset, "month_region", &["month", "region"]);
    }
    f.dimension(
        "month",
        &[("sales_monthly", "month"), ("refunds_monthly", "month")],
    );
    f.dimension(
        "region",
        &[("sales_monthly", "region"), ("refunds_monthly", "region")],
    );
    let grain = Grain::new([dimension_atom("month"), dimension_atom("region")]);
    for dataset in &mut f.0.datasets {
        dataset.observation_grain = grain.clone();
    }
    f.sum("gross_revenue", "sales_monthly", "amount");
    f.sum("refunds", "refunds_monthly", "amount");
    let expression = MetricExpr {
        kind: MetricExprKind::Arithmetic {
            op: ArithmeticOp::Subtract,
            left: Box::new(f.metric_ref("gross_revenue")),
            right: Box::new(f.metric_ref("refunds")),
            contract: NumericContract::default(),
        },
        value_type: decimal(19, 2),
        unit: None,
    };
    f.metric(
        "net_revenue",
        expression,
        grain,
        AggregationContract::fully_additive(),
    );
    assert_eq!(f.0.measures[0].input_grain, f.0.measures[1].input_grain);
    f.0
}

fn case_010() -> ModelDefinition {
    let mut f = Fixture::new(10);
    f.dataset(
        "invoices",
        &[
            ("invoice_id", integer()),
            ("date", ValueType::Date),
            ("region", ValueType::Utf8),
            ("amount", amount()),
        ],
    );
    f.dataset(
        "targets",
        &[
            ("month", ValueType::Date),
            ("region", ValueType::Utf8),
            ("target_amount", amount()),
        ],
    );
    f.primary_key("invoices", "invoice", &["invoice_id"]);
    f.primary_key("targets", "month_region", &["month", "region"]);
    f.time_role("invoices", "date", TimeRole::EventTime);
    f.temporal_dimension("month", "invoices", "date", "UTC", TimeGrain::Month);
    let target_month = f.scalar("targets", "month");
    f.0.dimensions[0]
        .participating_datasets
        .push(id("dataset", "targets"));
    f.0.dimensions[0].bindings.push(DimensionBinding {
        dataset: id("dataset", "targets"),
        expression: target_month,
        conformance: ConformanceContract::ExactDomain,
    });
    f.dimension("region", &[("invoices", "region"), ("targets", "region")]);
    let grain = Grain::new([dimension_atom("month"), dimension_atom("region")]);
    f.dataset_mut("targets").observation_grain = grain.clone();
    f.sum("actual_revenue", "invoices", "amount");
    f.sum("target_revenue", "targets", "target_amount");
    f.ratio(
        "attainment",
        "actual_revenue",
        "target_revenue",
        grain,
        Some(Unit::Ratio),
    );
    assert_ne!(f.0.measures[0].input_grain, f.0.measures[1].input_grain);
    f.0
}

fn case_011() -> ModelDefinition {
    let mut f = Fixture::new(11);
    f.dataset(
        "orders",
        &[("order_id", integer()), ("order_total", amount())],
    );
    f.dataset(
        "order_campaign",
        &[
            ("order_id", integer()),
            ("campaign_id", ValueType::Utf8),
            ("allocation_weight", decimal(3, 2)),
        ],
    );
    f.dataset(
        "campaign",
        &[("campaign_id", ValueType::Utf8), ("name", ValueType::Utf8)],
    );
    f.primary_key("orders", "order", &["order_id"]);
    f.primary_key(
        "order_campaign",
        "order_campaign",
        &["order_id", "campaign_id"],
    );
    f.primary_key("campaign", "campaign", &["campaign_id"]);
    f.key("order_campaign", "order", &["order_id"], false);
    f.key("order_campaign", "campaign", &["campaign_id"], false);
    f.relationship(
        "order",
        ("order_campaign", "order_id"),
        ("orders", "order_id"),
    );
    f.relationship(
        "campaign",
        ("order_campaign", "campaign_id"),
        ("campaign", "campaign_id"),
    );
    f.dimension("campaign", &[("campaign", "name")]);
    f.0.bridges.push(Bridge {
        metadata: meta("bridge", "order_campaign"),
        left_dataset: id("dataset", "orders"),
        bridge_dataset: id("dataset", "order_campaign"),
        right_dataset: id("dataset", "campaign"),
        left_relationship: id("relationship", "order"),
        right_relationship: id("relationship", "campaign"),
        allocation: Some(Allocation {
            weight_expression: f.scalar("order_campaign", "allocation_weight"),
            normalization_scope: key_grain("order"),
            required_total: exact("100", 3, 2),
            tolerance: exact("0", 3, 2),
            evidence: ContractEvidenceState::DeclaredUnverified,
        }),
    });
    f.sum("allocated_revenue", "orders", "order_total");
    f.0.metrics[0].allocation_bridge = Some(id("bridge", "order_campaign"));
    f.0
}

fn case_012() -> ModelDefinition {
    let mut f = Fixture::new(12);
    f.dataset(
        "events",
        &[
            ("event_id", integer()),
            (
                "event_time",
                ValueType::Timestamp {
                    timezone_semantics: TimestampSemantics::Instant,
                },
            ),
            ("amount", amount()),
        ],
    );
    f.primary_key("events", "event", &["event_id"]);
    f.time_role("events", "event_time", TimeRole::EventTime);
    f.temporal_dimension("month", "events", "event_time", "UTC", TimeGrain::Month);
    f.sum("revenue", "events", "amount");
    f.0
}

fn case_013() -> ModelDefinition {
    let mut f = Fixture::new(13);
    f.dataset(
        "sales",
        &[
            ("sale_id", integer()),
            ("customer_id", integer()),
            ("sale_date", ValueType::Date),
            ("amount", amount()),
        ],
    );
    f.dataset(
        "customer_history",
        &[
            ("customer_id", integer()),
            ("region", ValueType::Utf8),
            ("valid_from", ValueType::Date),
            ("valid_to", ValueType::Date),
        ],
    );
    f.primary_key("sales", "sale", &["sale_id"]);
    f.primary_key(
        "customer_history",
        "customer_version",
        &["customer_id", "valid_from"],
    );
    f.key("sales", "customer", &["customer_id"], false);
    f.key("customer_history", "customer", &["customer_id"], false);
    f.time_role("sales", "sale_date", TimeRole::EventTime);
    f.time_role("customer_history", "valid_from", TimeRole::ValidityStart);
    f.time_role("customer_history", "valid_to", TimeRole::ValidityEnd);
    f.relationship(
        "historical_customer",
        ("sales", "customer_id"),
        ("customer_history", "customer_id"),
    );
    f.0.relationships[0].temporal_constraint = Some(TemporalConstraint {
        event_time: field_id("sales", "sale_date"),
        valid_from: field_id("customer_history", "valid_from"),
        valid_to: Some(field_id("customer_history", "valid_to")),
        interval_boundary: IntervalBoundary::ClosedOpen,
        match_cardinality: MatchCardinality::ExactlyOne,
        evidence: ContractEvidenceState::DeclaredUnverified,
    });
    f.dimension("historical_region", &[("customer_history", "region")]);
    f.sum("revenue", "sales", "amount");
    f.0
}

fn case_014() -> ModelDefinition {
    let mut f = Fixture::new(14);
    f.dataset(
        "measurements",
        &[("measurement_id", integer()), ("amount", amount())],
    );
    f.dataset_mut("measurements").fields[1].nullable = true;
    f.primary_key("measurements", "measurement", &["measurement_id"]);
    f.sum("amount_sum", "measurements", "amount");
    f.measure(
        "amount_count",
        "measurements",
        Some(f.scalar("measurements", "amount")),
        AggregateFunction::CountNonNull,
        AggregationContract::fully_additive(),
    );
    f.measured_metric("amount_count");
    f.measure(
        "row_count",
        "measurements",
        None,
        AggregateFunction::CountRows,
        AggregationContract::fully_additive(),
    );
    f.measured_metric("row_count");
    assert_ne!(
        f.0.measures[1].null_semantics,
        f.0.measures[2].null_semantics
    );
    f.0
}

fn case_015() -> ModelDefinition {
    let mut f = Fixture::new(15);
    f.dataset(
        "sales",
        &[
            ("sale_id", integer()),
            ("tenant_id", ValueType::Utf8),
            ("region", ValueType::Utf8),
            ("amount", amount()),
        ],
    );
    f.primary_key("sales", "sale", &["sale_id"]);
    f.dimension("region", &[("sales", "region")]);
    f.sum("revenue", "sales", "amount");
    f.0.policies.push(Policy {
        metadata: meta("policy", "tenant"),
        scope: PolicyScope::Dataset(id("dataset", "sales")),
        enforcement: Enforcement::Mandatory,
        predicate: ScalarExpr {
            kind: ScalarExprKind::Comparison {
                op: ComparisonOp::Equal,
                left: Box::new(f.scalar("sales", "tenant_id")),
                right: Box::new(ScalarExpr {
                    kind: ScalarExprKind::ContextSlot("tenant_id".into()),
                    value_type: ValueType::Utf8,
                    unit: None,
                }),
            },
            value_type: ValueType::Boolean,
            unit: None,
        },
        context_slots: vec![ContextSlot {
            name: "tenant_id".into(),
            value_type: ValueType::Utf8,
            required: true,
            sensitivity: Sensitivity::Sensitive,
        }],
    });
    f.dataset_mut("sales")
        .policy_refs
        .push(id("policy", "tenant"));
    f.0
}

fn case_016() -> ModelDefinition {
    let mut f = Fixture::new(16);
    f.dataset(
        "orders",
        &[
            ("order_id", integer()),
            ("billing_customer_id", integer()),
            ("shipping_customer_id", integer()),
            ("amount", amount()),
        ],
    );
    f.dataset(
        "customer",
        &[("customer_id", integer()), ("country", ValueType::Utf8)],
    );
    f.primary_key("orders", "order", &["order_id"]);
    f.primary_key("customer", "customer", &["customer_id"]);
    for (role, field) in [
        ("billing_customer", "billing_customer_id"),
        ("shipping_customer", "shipping_customer_id"),
    ] {
        f.key("orders", "customer", &[field], false);
        f.relationship(role, ("orders", field), ("customer", "customer_id"));
    }
    f.dimension("country", &[("customer", "country")]);
    f.sum("revenue", "orders", "amount");
    assert_ne!(f.0.relationships[0].role, f.0.relationships[1].role);
    f.0
}

fn case_017() -> ModelDefinition {
    let mut f = Fixture::new(17);
    f.dataset(
        "finance",
        &[
            ("record_id", integer()),
            ("amount_usd", amount()),
            ("amount_bob", amount()),
        ],
    );
    f.primary_key("finance", "record", &["record_id"]);
    f.dataset_mut("finance").fields[1].unit = Some(Unit::Currency {
        iso4217: "USD".into(),
    });
    f.dataset_mut("finance").fields[2].unit = Some(Unit::Currency {
        iso4217: "BOB".into(),
    });
    f.sum("revenue_usd", "finance", "amount_usd");
    f.sum("revenue_bob", "finance", "amount_bob");
    assert_ne!(f.0.metrics[0].unit, f.0.metrics[1].unit);
    // The corpus requests the invalid combination in a query. Both definitions are valid.
    f.0
}

fn case_018() -> ModelDefinition {
    let mut f = Fixture::new(18);
    f.dataset(
        "sales",
        &[
            ("sale_id", integer()),
            ("date", ValueType::Date),
            ("category", ValueType::Utf8),
            ("amount", amount()),
        ],
    );
    f.dataset(
        "sales_month_category_rollup",
        &[
            ("month", ValueType::Date),
            ("category", ValueType::Utf8),
            ("revenue", amount()),
        ],
    );
    f.primary_key("sales", "sale", &["sale_id"]);
    f.primary_key(
        "sales_month_category_rollup",
        "month_category",
        &["month", "category"],
    );
    f.time_role("sales", "date", TimeRole::EventTime);
    f.temporal_dimension("month", "sales", "date", "UTC", TimeGrain::Month);
    let month = f.scalar("sales_month_category_rollup", "month");
    f.0.dimensions[0]
        .participating_datasets
        .push(id("dataset", "sales_month_category_rollup"));
    f.0.dimensions[0].bindings.push(DimensionBinding {
        dataset: id("dataset", "sales_month_category_rollup"),
        expression: month,
        conformance: ConformanceContract::ExactDomain,
    });
    f.dimension(
        "category",
        &[
            ("sales", "category"),
            ("sales_month_category_rollup", "category"),
        ],
    );
    f.dataset_mut("sales_month_category_rollup")
        .observation_grain = Grain::new([dimension_atom("month"), dimension_atom("category")]);
    f.sum("revenue", "sales", "amount");
    f.measure(
        "rollup_revenue",
        "sales_month_category_rollup",
        Some(f.scalar("sales_month_category_rollup", "revenue")),
        AggregateFunction::Sum,
        AggregationContract::fully_additive(),
    );
    // A second source's semantic schema is representable; no substitution/equivalence is claimed.
    f.0
}

fn case_019() -> ModelDefinition {
    let mut f = Fixture::new(19);
    f.dataset(
        "events",
        &[
            ("event_id", integer()),
            (
                "event_time",
                ValueType::Timestamp {
                    timezone_semantics: TimestampSemantics::Instant,
                },
            ),
            ("amount", decimal(18, 4)),
        ],
    );
    f.primary_key("events", "event", &["event_id"]);
    f.time_role("events", "event_time", TimeRole::EventTime);
    f.temporal_dimension(
        "business_day",
        "events",
        "event_time",
        "America/La_Paz",
        TimeGrain::Day,
    );
    f.sum("revenue", "events", "amount");
    assert_eq!(f.0.metrics[0].value_type, decimal(18, 4));
    f.0
}

fn case_020() -> ModelDefinition {
    let mut f = Fixture::new(20);
    f.dataset(
        "balances",
        &[
            ("account_id", integer()),
            ("month_end", ValueType::Date),
            ("balance", amount()),
        ],
    );
    f.primary_key("balances", "account_month", &["account_id", "month_end"]);
    f.key("balances", "account", &["account_id"], false);
    f.dimension("month_end", &[("balances", "month_end")]);
    f.time_role("balances", "month_end", TimeRole::SnapshotTime);
    let contract = AggregationContract {
        classification: AggregationClass::SemiAdditive,
        axis_rules: vec![
            AxisRule {
                axis: dimension_atom("month_end"),
                behavior: AxisBehavior::Forbidden,
            },
            AxisRule {
                axis: GrainAtom::EntityKeyRef(id("key", "account")),
                behavior: AxisBehavior::Additive,
            },
        ],
        decomposition: None,
    };
    f.measure(
        "balance",
        "balances",
        Some(f.scalar("balances", "balance")),
        AggregateFunction::Sum,
        contract,
    );
    f.measured_metric("balance");
    assert!(f.0.metrics[0].time_behavior.is_none());
    f.0
}

fn case_021() -> ModelDefinition {
    let mut f = Fixture::new(21);
    f.dataset(
        "sales_daily",
        &[("date", ValueType::Date), ("amount", amount())],
    );
    f.primary_key("sales_daily", "day", &["date"]);
    f.dimension("date", &[("sales_daily", "date")]);
    f.time_role("sales_daily", "date", TimeRole::EventTime);
    let grain = Grain::new([dimension_atom("date")]);
    f.dataset_mut("sales_daily").observation_grain = grain.clone();
    f.sum("daily_revenue", "sales_daily", "amount");
    let expression = MetricExpr {
        kind: MetricExprKind::Window {
            base_metric: id("metric", "daily_revenue"),
        },
        value_type: amount(),
        unit: None,
    };
    f.metric(
        "cumulative_revenue",
        expression,
        grain.clone(),
        AggregationContract {
            classification: AggregationClass::OrderedWindow,
            axis_rules: vec![AxisRule {
                axis: dimension_atom("date"),
                behavior: AxisBehavior::WindowDependent,
            }],
            decomposition: None,
        },
    );
    f.0.metrics[1].window_behavior = Some(WindowBehavior::Cumulative {
        base_metric: id("metric", "daily_revenue"),
        prerequisite_grain: grain,
        order_by: vec![order(id("dimension", "date"))],
        partition_by: Grain::global(),
        frame: WindowFrame::UnboundedPrecedingToCurrent,
    });
    f.0
}

fn assert_fixture_sources(case: u8, model: &ModelDefinition) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/cases")
        .join(format!("CASE-{case:03}"));
    let source_files: BTreeSet<_> = std::fs::read_dir(fixture.join("input"))
        .expect("normative input directory")
        .map(|entry| {
            entry
                .expect("normative input entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let represented_files: BTreeSet<_> = model
        .source_bindings
        .iter()
        .map(|source| format!("{}.csv", source.relation_ref.as_str()))
        .collect();
    assert_eq!(
        represented_files, source_files,
        "every fixture source has a CSMIR schema"
    );
    for source in &model.source_bindings {
        let csv = std::fs::read_to_string(
            fixture
                .join("input")
                .join(format!("{}.csv", source.relation_ref.as_str())),
        )
        .expect("normative CSV source");
        let columns: BTreeSet<_> = csv.lines().next().expect("CSV header").split(',').collect();
        let fields: BTreeSet<_> = source
            .field_bindings
            .iter()
            .map(|field| field.handle.as_str())
            .collect();
        assert_eq!(
            fields, columns,
            "all columns must be explicitly represented"
        );
    }
    let expected = match case {
        16 | 17 | 20 => "error.json",
        _ => "result.csv",
    };
    assert!(fixture.join("expected").join(expected).is_file());
}

fn assert_representation(case: u8, model: ModelDefinition) {
    assert_fixture_sources(case, &model);
    let semantic_ids: Vec<_> = model
        .datasets
        .iter()
        .map(|x| x.metadata.id())
        .chain(
            model
                .datasets
                .iter()
                .flat_map(|x| &x.fields)
                .map(|x| x.metadata.id()),
        )
        .chain(
            model
                .entities
                .iter()
                .flat_map(|x| &x.keys)
                .map(|x| x.metadata.id()),
        )
        .chain(model.dimensions.iter().map(|x| x.metadata.id()))
        .chain(model.measures.iter().map(|x| x.metadata.id()))
        .chain(model.metrics.iter().map(|x| x.metadata.id()))
        .chain(model.relationships.iter().map(|x| x.metadata.id()))
        .chain(model.bridges.iter().map(|x| x.metadata.id()))
        .chain(model.policies.iter().map(|x| x.metadata.id()))
        .cloned()
        .collect();
    let compiled = model
        .clone()
        .compile()
        .expect("valid CSMIR fixture definition must compile");
    let golden = include_str!("csmir-golden-hashes.txt")
        .lines()
        .nth(usize::from(case - 1))
        .expect("frozen model fingerprint")
        .split_once(' ')
        .expect("case and fingerprint");
    assert_eq!(golden.0, format!("CASE-{case:03}"));
    assert_eq!(compiled.content_hash().to_string(), golden.1);
    for object in semantic_ids {
        assert!(
            compiled.object_hash(&object).is_some(),
            "missing object hash: {object}"
        );
    }
    assert_ne!(compiled.definition().metrics, [] as [Metric; 0]);
    let mut reordered = model;
    reordered.source_bindings.reverse();
    for source in &mut reordered.source_bindings {
        source.field_bindings.reverse();
    }
    reordered.datasets.reverse();
    for dataset in &mut reordered.datasets {
        dataset.fields.reverse();
        dataset.key_bindings.reverse();
        dataset.time_roles.reverse();
    }
    reordered.entities.reverse();
    reordered.dimensions.reverse();
    reordered.measures.reverse();
    reordered.metrics.reverse();
    reordered.relationships.reverse();
    reordered.bridges.reverse();
    reordered.policies.reverse();
    let reordered = reordered
        .compile()
        .expect("nonsemantic order must preserve compilation");
    assert_eq!(compiled.content_hash(), reordered.content_hash());
}

macro_rules! corpus_test {
    ($name:ident, $number:literal, $builder:ident) => {
        #[test]
        fn $name() {
            assert_representation($number, $builder());
        }
    };
}

corpus_test!(case_001_additive_definition, 1, case_001);
corpus_test!(case_002_star_dimensions, 2, case_002);
corpus_test!(
    case_003_declared_unverified_cardinality_is_valid,
    3,
    case_003
);
corpus_test!(case_004_independent_measure_grains, 4, case_004);
corpus_test!(case_005_multihop_relationship_graph, 5, case_005);
corpus_test!(case_006_ratio_decomposition, 6, case_006);
corpus_test!(case_007_period_end_contract, 7, case_007);
corpus_test!(case_008_distinct_recompute_contract, 8, case_008);
corpus_test!(case_009_same_conformed_grain, 9, case_009);
corpus_test!(case_010_different_native_grains, 10, case_010);
corpus_test!(case_011_bridge_allocation_contract, 11, case_011);
corpus_test!(case_012_time_bucket_definition, 12, case_012);
corpus_test!(case_013_as_of_temporal_contract, 13, case_013);
corpus_test!(case_014_aggregate_null_contracts, 14, case_014);
corpus_test!(case_015_mandatory_typed_policy, 15, case_015);
corpus_test!(
    case_016_role_ambiguity_does_not_invalidate_model,
    16,
    case_016
);
corpus_test!(case_017_separate_currency_metrics_are_valid, 17, case_017);
corpus_test!(case_018_base_and_rollup_semantic_schemas, 18, case_018);
corpus_test!(case_019_exact_decimal_and_timezone_contracts, 19, case_019);
corpus_test!(
    case_020_forbidden_temporal_axis_is_valid_metadata,
    20,
    case_020
);
corpus_test!(case_021_cumulative_window_contract, 21, case_021);

#[test]
fn invalid_as_of_evidence_rejects_the_model_definition() {
    let mut model = case_013();
    model.relationships[0]
        .temporal_constraint
        .as_mut()
        .expect("as-of fixture")
        .evidence = ContractEvidenceState::Invalid {
        evidence_id: "overlapping-history".into(),
        observed_at: "2026-09-28".into(),
    };
    let errors = model
        .compile()
        .expect_err("invalid evidence must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.code == ErrorCode::TemporalMatchContractViolation)
    );
}

#[test]
fn invalid_allocation_evidence_rejects_the_model_definition() {
    let mut model = case_011();
    model.bridges[0]
        .allocation
        .as_mut()
        .expect("allocation fixture")
        .evidence = ContractEvidenceState::Invalid {
        evidence_id: "weights-not-normalized".into(),
        observed_at: "2026-09-28".into(),
    };
    let errors = model
        .compile()
        .expect_err("invalid evidence must be rejected");
    assert!(
        errors
            .iter()
            .any(|error| error.code == ErrorCode::AllocationContractViolation)
    );
}

#[test]
fn incompatible_units_in_a_metric_definition_are_rejected_locally() {
    let mut fixture = Fixture(case_017());
    let expression = MetricExpr {
        kind: MetricExprKind::Arithmetic {
            op: ArithmeticOp::Add,
            left: Box::new(fixture.metric_ref("revenue_usd")),
            right: Box::new(fixture.metric_ref("revenue_bob")),
            contract: NumericContract::default(),
        },
        value_type: decimal(19, 2),
        unit: Some(Unit::Currency {
            iso4217: "USD".into(),
        }),
    };
    fixture.metric(
        "combined",
        expression,
        key_grain("record"),
        AggregationContract::fully_additive(),
    );
    let errors = fixture
        .0
        .compile()
        .expect_err("an invalid CSMIR definition differs from valid separate metrics");
    assert!(
        errors
            .iter()
            .any(|error| error.code == ErrorCode::IncompatibleUnits
                && error.primary_ref.as_ref() == Some(&id("metric", "combined")))
    );
}
