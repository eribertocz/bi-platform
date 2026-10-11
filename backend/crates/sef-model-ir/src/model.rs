//! Engine-neutral semantic definitions and the immutable compiled snapshot.

use std::collections::BTreeMap;

use crate::canonical::{Canonical, CanonicalValue as C, Domain, Fingerprint, fingerprint};
use crate::diagnostic::{Diagnostic, ErrorCode};
use crate::expressions::{AggregateFunction, AggregateNullSemantics, MetricExpr, ScalarExpr};
use crate::grain::{Grain, GrainAtom};
use crate::identity::{ObjectIdentity, SourceHandle, StableId};
use crate::policy::Policy;
use crate::relationships::{Bridge, ContractEvidenceState, Relationship};
use crate::time::{CalendarProfile, OrderTerm, TimeRole, TimeSemantics};
use crate::types::{DecimalValue, ValueType};
use crate::units::Unit;

#[derive(Clone, Debug, PartialEq)]
pub struct ObjectMetadata {
    pub identity: ObjectIdentity,
    pub label: Option<String>,
    pub description: Option<String>,
}

impl ObjectMetadata {
    #[must_use]
    pub fn new(identity: ObjectIdentity) -> Self {
        Self {
            identity,
            label: None,
            description: None,
        }
    }
    #[must_use]
    pub fn id(&self) -> &StableId {
        &self.identity.stable_id
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Namespace {
    pub metadata: ObjectMetadata,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceBinding {
    pub metadata: ObjectMetadata,
    pub source_id: SourceHandle,
    pub relation_ref: SourceHandle,
    pub field_bindings: Vec<FieldBinding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldBinding {
    pub field: StableId,
    pub handle: SourceHandle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticDataset {
    pub metadata: ObjectMetadata,
    pub source_binding: StableId,
    pub observation_grain: Grain,
    pub fields: Vec<Field>,
    pub key_bindings: Vec<KeyBinding>,
    pub time_roles: Vec<DatasetTimeRole>,
    pub policy_refs: Vec<StableId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub metadata: ObjectMetadata,
    pub dataset: StableId,
    pub value_type: ValueType,
    pub unit: Option<Unit>,
    pub nullable: bool,
    pub binding: SourceHandle,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyBinding {
    pub entity_key: StableId,
    /// Field order corresponds to the components of a composite entity key.
    pub fields: Vec<StableId>,
    pub unique: bool,
    pub evidence: ContractEvidenceState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DatasetTimeRole {
    pub field: StableId,
    pub role: TimeRole,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub metadata: ObjectMetadata,
    pub keys: Vec<EntityKey>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntityKey {
    pub metadata: ObjectMetadata,
    pub value_type: ValueType,
    pub uniqueness_scope: Grain,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Dimension {
    pub metadata: ObjectMetadata,
    pub value_type: ValueType,
    pub semantic_role: Option<String>,
    pub unit: Option<Unit>,
    pub time_semantics: Option<TimeSemantics>,
    pub participating_datasets: Vec<StableId>,
    pub bindings: Vec<DimensionBinding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DimensionBinding {
    pub dataset: StableId,
    pub expression: ScalarExpr,
    pub conformance: ConformanceContract,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConformanceContract {
    ExactDomain,
    NormalizedDomain {
        normalization: ScalarExpr,
    },
    ExplicitMapping {
        relationship: StableId,
        canonical_value: StableId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Measure {
    pub metadata: ObjectMetadata,
    pub dataset: StableId,
    pub scalar_expression: Option<ScalarExpr>,
    pub aggregation: AggregateFunction,
    pub intrinsic_filter: Option<ScalarExpr>,
    pub value_type: ValueType,
    pub unit: Option<Unit>,
    pub input_grain: Grain,
    pub aggregation_contract: AggregationContract,
    pub null_semantics: AggregateNullSemantics,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metric {
    pub metadata: ObjectMetadata,
    pub expression: MetricExpr,
    pub intrinsic_filter: Option<ScalarExpr>,
    pub value_type: ValueType,
    pub unit: Option<Unit>,
    pub native_grain: Grain,
    pub aggregation_contract: AggregationContract,
    pub time_behavior: Option<TemporalAggregation>,
    pub window_behavior: Option<WindowBehavior>,
    pub dependencies: Vec<StableId>,
    pub policy_refs: Vec<StableId>,
    /// Attribution is explicit; a primitive measure remains dataset-local.
    pub allocation_bridge: Option<StableId>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AggregationContract {
    pub classification: AggregationClass,
    pub axis_rules: Vec<AxisRule>,
    pub decomposition: Option<Decomposition>,
}

impl AggregationContract {
    #[must_use]
    pub fn fully_additive() -> Self {
        Self {
            classification: AggregationClass::FullyAdditive,
            axis_rules: Vec::new(),
            decomposition: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AggregationClass {
    FullyAdditive,
    SemiAdditive,
    NonAdditive,
    Recomputable,
    OrderedWindow,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AxisRule {
    pub axis: GrainAtom,
    pub behavior: AxisBehavior,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisBehavior {
    Additive,
    Recompute,
    DistinctRecompute,
    PeriodEnd,
    PeriodStart,
    Forbidden,
    WindowDependent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decomposition {
    pub components: Vec<StableId>,
    pub recomposition: MetricExpr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemporalAggregation {
    pub axis: GrainAtom,
    pub rule: TemporalSelection,
    pub order_by: Vec<OrderTerm>,
    pub selection_scope: Grain,
    pub ties: TemporalTies,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporalSelection {
    PeriodEnd,
    PeriodStart,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TemporalTies {
    UniqueByContract { key: StableId },
    SecondaryKey { fields: Vec<OrderTerm> },
    Unspecified,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WindowBehavior {
    Cumulative {
        base_metric: StableId,
        prerequisite_grain: Grain,
        order_by: Vec<OrderTerm>,
        partition_by: Grain,
        frame: WindowFrame,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowFrame {
    UnboundedPrecedingToCurrent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalendarDefinition {
    pub metadata: ObjectMetadata,
    pub profile: CalendarProfile,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UnitConversion {
    pub metadata: ObjectMetadata,
    pub from: Unit,
    pub to: Unit,
    pub rule: ConversionRule,
    pub result_type: ValueType,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConversionRule {
    Linear {
        factor: DecimalValue,
        offset: DecimalValue,
    },
    Fx {
        rate_dimension: StableId,
        time_dimension: StableId,
    },
}

/// Non-semantic metadata only. Semantic extensions require a future explicit profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtensionMetadata {
    pub namespace: String,
    pub key: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelDefinition {
    pub metadata: ObjectMetadata,
    pub namespaces: Vec<Namespace>,
    pub source_bindings: Vec<SourceBinding>,
    pub datasets: Vec<SemanticDataset>,
    pub entities: Vec<Entity>,
    pub dimensions: Vec<Dimension>,
    pub measures: Vec<Measure>,
    pub metrics: Vec<Metric>,
    pub relationships: Vec<Relationship>,
    pub bridges: Vec<Bridge>,
    pub policies: Vec<Policy>,
    pub calendars: Vec<CalendarDefinition>,
    pub unit_conversions: Vec<UnitConversion>,
    pub extensions: Vec<ExtensionMetadata>,
}

impl ModelDefinition {
    #[must_use]
    pub fn new(metadata: ObjectMetadata) -> Self {
        Self {
            metadata,
            namespaces: Vec::new(),
            source_bindings: Vec::new(),
            datasets: Vec::new(),
            entities: Vec::new(),
            dimensions: Vec::new(),
            measures: Vec::new(),
            metrics: Vec::new(),
            relationships: Vec::new(),
            bridges: Vec::new(),
            policies: Vec::new(),
            calendars: Vec::new(),
            unit_conversions: Vec::new(),
            extensions: Vec::new(),
        }
    }
    /// # Errors
    ///
    /// Returns validation diagnostics for an invalid definition, or
    /// `UnsupportedSemanticConstruct` if canonical encoding fails.
    pub fn compile(self) -> Result<CompiledSemanticModel, Vec<Diagnostic>> {
        CompiledSemanticModel::compile(self)
    }
}

#[derive(Clone, Debug)]
pub struct CompiledSemanticModel {
    definition: ModelDefinition,
    content_hash: Fingerprint,
    object_hashes: BTreeMap<StableId, Fingerprint>,
}

impl CompiledSemanticModel {
    /// # Errors
    ///
    /// Returns validation diagnostics for an invalid definition, or
    /// `UnsupportedSemanticConstruct` if canonical encoding fails.
    pub fn compile(mut definition: ModelDefinition) -> Result<Self, Vec<Diagnostic>> {
        crate::validation::validate(&definition)?;
        definition.normalize();
        let error = || {
            vec![Diagnostic::error(
                ErrorCode::UnsupportedSemanticConstruct,
                Some(definition.metadata.id().clone()),
                "Canonical encoding failed",
            )]
        };
        let content_hash = fingerprint(Domain::Csmir, &definition).map_err(|_| error())?;
        let mut object_hashes = BTreeMap::new();
        for (id, value) in definition.semantic_objects() {
            object_hashes.insert(
                id.clone(),
                fingerprint(Domain::Csmir, &value).map_err(|_| error())?,
            );
        }
        Ok(Self {
            definition,
            content_hash,
            object_hashes,
        })
    }
    #[must_use]
    pub fn definition(&self) -> &ModelDefinition {
        &self.definition
    }
    #[must_use]
    pub fn content_hash(&self) -> &Fingerprint {
        &self.content_hash
    }
    #[must_use]
    pub fn object_hash(&self, id: &StableId) -> Option<&Fingerprint> {
        self.object_hashes.get(id)
    }
}

macro_rules! canonical_record {
    ($ty:ty, $tag:literal, $($field:ident),+ $(,)?) => {
        impl Canonical for $ty {
            fn canonical_value(&self) -> C {
                C::record($tag, [$( (stringify!($field), self.$field.canonical_value()) ),+])
            }
        }
    };
}
pub(crate) use canonical_record;

impl Canonical for ObjectMetadata {
    fn canonical_value(&self) -> C {
        self.identity.canonical_value()
    }
}
canonical_record!(Namespace, "namespace", metadata);
canonical_record!(FieldBinding, "field-binding", field, handle);
canonical_record!(
    Field, "field", metadata, dataset, value_type, unit, nullable, binding
);
canonical_record!(DatasetTimeRole, "dataset-time-role", field, role);
canonical_record!(
    EntityKey,
    "entity-key",
    metadata,
    value_type,
    uniqueness_scope
);
canonical_record!(
    DimensionBinding,
    "dimension-binding",
    dataset,
    expression,
    conformance
);
canonical_record!(
    Measure,
    "measure",
    metadata,
    dataset,
    scalar_expression,
    aggregation,
    intrinsic_filter,
    value_type,
    unit,
    input_grain,
    aggregation_contract,
    null_semantics
);
canonical_record!(AxisRule, "axis-rule", axis, behavior);
canonical_record!(
    TemporalAggregation,
    "temporal-aggregation",
    axis,
    rule,
    order_by,
    selection_scope,
    ties
);
canonical_record!(CalendarDefinition, "calendar-definition", metadata, profile);
canonical_record!(
    UnitConversion,
    "unit-conversion",
    metadata,
    from,
    to,
    rule,
    result_type
);

macro_rules! canonical_unit_enum {
    ($ty:ident, $prefix:literal, $($variant:ident => $tag:literal),+ $(,)?) => {
        impl Canonical for $ty {
            fn canonical_value(&self) -> C {
                C::record(concat!($prefix), [("variant", C::text(match self { $(Self::$variant => $tag),+ }))])
            }
        }
    };
}
pub(crate) use canonical_unit_enum;
canonical_unit_enum!(AggregationClass, "aggregation-class", FullyAdditive => "fully-additive", SemiAdditive => "semi-additive", NonAdditive => "non-additive", Recomputable => "recomputable", OrderedWindow => "ordered-window");
canonical_unit_enum!(AxisBehavior, "axis-behavior", Additive => "additive", Recompute => "recompute", DistinctRecompute => "distinct-recompute", PeriodEnd => "period-end", PeriodStart => "period-start", Forbidden => "forbidden", WindowDependent => "window-dependent");
canonical_unit_enum!(TemporalSelection, "temporal-selection", PeriodEnd => "period-end", PeriodStart => "period-start");
canonical_unit_enum!(WindowFrame, "window-frame", UnboundedPrecedingToCurrent => "unbounded-preceding-to-current");

impl Canonical for KeyBinding {
    fn canonical_value(&self) -> C {
        C::record(
            "key-binding",
            [
                ("entity_key", self.entity_key.canonical_value()),
                ("fields", C::sequence(&self.fields)),
                ("unique", C::Bool(self.unique)),
            ],
        )
    }
}

impl Canonical for SourceBinding {
    fn canonical_value(&self) -> C {
        C::record(
            "source-binding",
            [
                ("metadata", self.metadata.canonical_value()),
                ("source_id", self.source_id.canonical_value()),
                ("relation_ref", self.relation_ref.canonical_value()),
                ("field_bindings", C::set(&self.field_bindings)),
            ],
        )
    }
}

impl Canonical for SemanticDataset {
    fn canonical_value(&self) -> C {
        C::record(
            "semantic-dataset",
            [
                ("metadata", self.metadata.canonical_value()),
                ("source_binding", self.source_binding.canonical_value()),
                (
                    "observation_grain",
                    self.observation_grain.canonical_value(),
                ),
                ("fields", C::set(&self.fields)),
                ("key_bindings", C::set(&self.key_bindings)),
                ("time_roles", C::set(&self.time_roles)),
                ("policy_refs", C::set(&self.policy_refs)),
            ],
        )
    }
}

impl Canonical for Entity {
    fn canonical_value(&self) -> C {
        C::record(
            "entity",
            [
                ("metadata", self.metadata.canonical_value()),
                ("keys", C::set(&self.keys)),
            ],
        )
    }
}

impl Canonical for Dimension {
    fn canonical_value(&self) -> C {
        C::record(
            "dimension",
            [
                ("metadata", self.metadata.canonical_value()),
                ("value_type", self.value_type.canonical_value()),
                ("semantic_role", self.semantic_role.canonical_value()),
                ("unit", self.unit.canonical_value()),
                ("time_semantics", self.time_semantics.canonical_value()),
                (
                    "participating_datasets",
                    C::set(&self.participating_datasets),
                ),
                ("bindings", C::set(&self.bindings)),
            ],
        )
    }
}

impl Canonical for ConformanceContract {
    fn canonical_value(&self) -> C {
        match self {
            Self::ExactDomain => C::record("conformance.exact-domain", [] as [(&str, C); 0]),
            Self::NormalizedDomain { normalization } => C::record(
                "conformance.normalized-domain",
                [("normalization", normalization.canonical_value())],
            ),
            Self::ExplicitMapping {
                relationship,
                canonical_value,
            } => C::record(
                "conformance.explicit-mapping",
                [
                    ("relationship", relationship.canonical_value()),
                    ("canonical_value", canonical_value.canonical_value()),
                ],
            ),
        }
    }
}

impl Canonical for AggregationContract {
    fn canonical_value(&self) -> C {
        C::record(
            "aggregation-contract",
            [
                ("classification", self.classification.canonical_value()),
                ("axis_rules", C::set(&self.axis_rules)),
                ("decomposition", self.decomposition.canonical_value()),
            ],
        )
    }
}

impl Canonical for Decomposition {
    fn canonical_value(&self) -> C {
        C::record(
            "decomposition",
            [
                ("components", C::set(&self.components)),
                ("recomposition", self.recomposition.canonical_value()),
            ],
        )
    }
}

impl Canonical for Metric {
    fn canonical_value(&self) -> C {
        C::record(
            "metric",
            [
                ("metadata", self.metadata.canonical_value()),
                ("expression", self.expression.canonical_value()),
                ("intrinsic_filter", self.intrinsic_filter.canonical_value()),
                ("value_type", self.value_type.canonical_value()),
                ("unit", self.unit.canonical_value()),
                ("native_grain", self.native_grain.canonical_value()),
                (
                    "aggregation_contract",
                    self.aggregation_contract.canonical_value(),
                ),
                ("time_behavior", self.time_behavior.canonical_value()),
                ("window_behavior", self.window_behavior.canonical_value()),
                ("dependencies", C::set(&self.dependencies)),
                ("policy_refs", C::set(&self.policy_refs)),
                (
                    "allocation_bridge",
                    self.allocation_bridge.canonical_value(),
                ),
            ],
        )
    }
}

impl Canonical for TemporalTies {
    fn canonical_value(&self) -> C {
        match self {
            Self::UniqueByContract { key } => C::record(
                "temporal-ties.unique-by-contract",
                [("key", key.canonical_value())],
            ),
            Self::SecondaryKey { fields } => C::record(
                "temporal-ties.secondary-key",
                [("fields", C::sequence(fields))],
            ),
            Self::Unspecified => C::record("temporal-ties.unspecified", [] as [(&str, C); 0]),
        }
    }
}

impl Canonical for WindowBehavior {
    fn canonical_value(&self) -> C {
        match self {
            Self::Cumulative {
                base_metric,
                prerequisite_grain,
                order_by,
                partition_by,
                frame,
            } => C::record(
                "window.cumulative",
                [
                    ("base_metric", base_metric.canonical_value()),
                    ("prerequisite_grain", prerequisite_grain.canonical_value()),
                    ("order_by", C::sequence(order_by)),
                    ("partition_by", partition_by.canonical_value()),
                    ("frame", frame.canonical_value()),
                ],
            ),
        }
    }
}

impl Canonical for ConversionRule {
    fn canonical_value(&self) -> C {
        match self {
            Self::Linear { factor, offset } => C::record(
                "conversion.linear",
                [
                    ("factor", factor.canonical_value()),
                    ("offset", offset.canonical_value()),
                ],
            ),
            Self::Fx {
                rate_dimension,
                time_dimension,
            } => C::record(
                "conversion.fx",
                [
                    ("rate_dimension", rate_dimension.canonical_value()),
                    ("time_dimension", time_dimension.canonical_value()),
                ],
            ),
        }
    }
}

impl Canonical for ModelDefinition {
    fn canonical_value(&self) -> C {
        C::record(
            "compiled-semantic-model",
            [
                ("semantic_profile", C::text("0.1")),
                ("csmir", C::text("0.1")),
                ("metadata", self.metadata.canonical_value()),
                ("namespaces", C::set(&self.namespaces)),
                ("source_bindings", C::set(&self.source_bindings)),
                ("datasets", C::set(&self.datasets)),
                ("entities", C::set(&self.entities)),
                ("dimensions", C::set(&self.dimensions)),
                ("measures", C::set(&self.measures)),
                ("metrics", C::set(&self.metrics)),
                ("relationships", C::set(&self.relationships)),
                ("bridges", C::set(&self.bridges)),
                ("policies", C::set(&self.policies)),
                ("calendars", C::set(&self.calendars)),
                ("unit_conversions", C::set(&self.unit_conversions)),
            ],
        )
    }
}

impl ModelDefinition {
    pub(crate) fn semantic_objects(&self) -> Vec<(&StableId, C)> {
        let mut values = vec![(self.metadata.id(), self.canonical_value())];
        macro_rules! collect {
            ($items:expr) => {
                for item in $items {
                    values.push((item.metadata.id(), item.canonical_value()));
                }
            };
        }
        collect!(&self.namespaces);
        collect!(&self.source_bindings);
        collect!(&self.datasets);
        collect!(&self.entities);
        collect!(&self.dimensions);
        collect!(&self.measures);
        collect!(&self.metrics);
        collect!(&self.relationships);
        collect!(&self.bridges);
        collect!(&self.policies);
        collect!(&self.calendars);
        collect!(&self.unit_conversions);
        for dataset in &self.datasets {
            collect!(&dataset.fields);
        }
        for entity in &self.entities {
            collect!(&entity.keys);
        }
        values
    }
    fn normalize(&mut self) {
        macro_rules! sort {
            ($items:expr) => {
                $items.sort_by(|a, b| a.metadata.id().cmp(b.metadata.id()));
            };
        }
        sort!(self.namespaces);
        sort!(self.source_bindings);
        sort!(self.datasets);
        sort!(self.entities);
        sort!(self.dimensions);
        sort!(self.measures);
        sort!(self.metrics);
        sort!(self.relationships);
        sort!(self.bridges);
        sort!(self.policies);
        sort!(self.calendars);
        sort!(self.unit_conversions);
        for dataset in &mut self.datasets {
            sort!(dataset.fields);
            dataset.policy_refs.sort();
        }
        for entity in &mut self.entities {
            sort!(entity.keys);
        }
        for metric in &mut self.metrics {
            metric.dependencies.sort();
            metric.policy_refs.sort();
        }
    }
}
