//! Structural and definition-local semantic compilation, without query analysis.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::{Diagnostic, ErrorCode, Evidence};
use crate::expressions::{
    AggregateFunction, AggregateNullSemantics, MetricExpr, MetricExprKind, ScalarExpr,
    ScalarExprKind, UnaryOp,
};
use crate::grain::{Grain, GrainAtom};
use crate::identity::{StableId, valid_segment};
use crate::model::{
    AggregationClass, AggregationContract, ConformanceContract, ConversionRule, Dimension,
    EntityKey, Field, Lifecycle, Measure, Metric, ModelDefinition, SemanticDataset, TemporalTies,
    UnitConversion, WindowBehavior,
};
use crate::policy::{ContextSlot, PolicyScope};
use crate::relationships::{Bridge, Cardinality, Relationship};
use crate::time::{CalendarProfile, TimeBucket, TimeRole};
use crate::types::{ArithmeticOp, NumericContract, ValueType, validate_arithmetic};
use crate::units::Unit;

pub fn validate(model: &ModelDefinition) -> Result<(), Vec<Diagnostic>> {
    let mut validator = Validator {
        model,
        errors: Vec::new(),
    };
    validator.run();
    validator.errors.sort_by(|a, b| {
        (&a.primary_ref, a.code, &a.related_refs, &a.message).cmp(&(
            &b.primary_ref,
            b.code,
            &b.related_refs,
            &b.message,
        ))
    });
    validator.errors.dedup();
    if validator.errors.is_empty() {
        Ok(())
    } else {
        Err(validator.errors)
    }
}

struct Validator<'a> {
    model: &'a ModelDefinition,
    errors: Vec<Diagnostic>,
}

impl<'a> Validator<'a> {
    fn error(&mut self, code: ErrorCode, owner: &StableId, message: &str) {
        let mut error = Diagnostic::error(code, Some(owner.clone()), message);
        error.evidence.push(Evidence::Constraint {
            name: code.as_str().to_owned(),
        });
        self.errors.push(error);
    }
    fn missing(&mut self, owner: &StableId, target: &StableId) {
        let mut error = Diagnostic::error(
            ErrorCode::InvalidModelReference,
            Some(owner.clone()),
            "Reference does not resolve to the required semantic object kind",
        );
        error.related_refs.push(target.clone());
        error.evidence.push(Evidence::Reference {
            role: "unresolved-definition-reference".into(),
            object: target.clone(),
        });
        self.errors.push(error);
    }
    fn field(&self, id: &StableId) -> Option<&'a Field> {
        self.model
            .datasets
            .iter()
            .flat_map(|d| &d.fields)
            .find(|f| f.metadata.id() == id)
    }
    fn dataset(&self, id: &StableId) -> Option<&'a SemanticDataset> {
        self.model.datasets.iter().find(|d| d.metadata.id() == id)
    }
    fn dimension(&self, id: &StableId) -> Option<&'a Dimension> {
        self.model.dimensions.iter().find(|d| d.metadata.id() == id)
    }
    fn key(&self, id: &StableId) -> Option<&'a EntityKey> {
        self.model
            .entities
            .iter()
            .flat_map(|e| &e.keys)
            .find(|k| k.metadata.id() == id)
    }
    fn metric(&self, id: &StableId) -> Option<&'a Metric> {
        self.model.metrics.iter().find(|m| m.metadata.id() == id)
    }
    fn measure(&self, id: &StableId) -> Option<&'a Measure> {
        self.model.measures.iter().find(|m| m.metadata.id() == id)
    }
    fn relationship(&self, id: &StableId) -> Option<&'a Relationship> {
        self.model
            .relationships
            .iter()
            .find(|r| r.metadata.id() == id)
    }

    fn run(&mut self) {
        self.validate_identities();
        self.validate_definitions();
    }

    fn validate_identities(&mut self) {
        let mut ids = BTreeSet::new();
        let mut objects = vec![&self.model.metadata];
        macro_rules! collect_metadata {
            ($items:expr) => {
                objects.extend($items.iter().map(|item| &item.metadata));
            };
        }
        collect_metadata!(self.model.namespaces);
        collect_metadata!(self.model.source_bindings);
        collect_metadata!(self.model.datasets);
        collect_metadata!(self.model.entities);
        collect_metadata!(self.model.dimensions);
        collect_metadata!(self.model.measures);
        collect_metadata!(self.model.metrics);
        collect_metadata!(self.model.relationships);
        collect_metadata!(self.model.bridges);
        collect_metadata!(self.model.policies);
        collect_metadata!(self.model.calendars);
        collect_metadata!(self.model.unit_conversions);
        for dataset in &self.model.datasets {
            collect_metadata!(dataset.fields);
        }
        for entity in &self.model.entities {
            collect_metadata!(entity.keys);
        }
        for metadata in &objects {
            let id = metadata.id();
            if !ids.insert(id) {
                self.error(
                    ErrorCode::DuplicateStableId,
                    id,
                    "Semantic stable IDs must be unique",
                );
            }
        }
        for metadata in objects {
            if let Lifecycle::Deprecated {
                replacement: Some(target),
                ..
            } = &metadata.governance.lifecycle
                && (!ids.contains(target) || target.kind() != metadata.id().kind())
            {
                self.missing(metadata.id(), target);
            }
        }
    }

    fn validate_definitions(&mut self) {
        for entity in &self.model.entities {
            if entity.keys.is_empty() {
                self.error(
                    ErrorCode::InvalidRelationshipContract,
                    entity.metadata.id(),
                    "An entity requires at least one key",
                );
            }
            for key in &entity.keys {
                self.value_type(&key.value_type, key.metadata.id());
                self.grain(&key.uniqueness_scope, key.metadata.id());
            }
        }
        for source in &self.model.source_bindings {
            let mut bound = BTreeSet::new();
            for binding in &source.field_bindings {
                if !bound.insert(&binding.field) {
                    self.error(
                        ErrorCode::InvalidModelReference,
                        source.metadata.id(),
                        "Duplicate source field binding",
                    );
                }
                if self.field(&binding.field).is_none() {
                    self.missing(source.metadata.id(), &binding.field);
                }
            }
        }
        for dataset in &self.model.datasets {
            self.validate_dataset(dataset);
        }
        for dimension in &self.model.dimensions {
            self.validate_dimension(dimension);
        }
        self.validate_dimension_cycles();
        for measure in &self.model.measures {
            self.validate_measure(measure);
        }
        for relationship in &self.model.relationships {
            self.validate_relationship(relationship);
        }
        for bridge in &self.model.bridges {
            self.validate_bridge(bridge);
        }
        for conversion in &self.model.unit_conversions {
            self.validate_conversion(conversion);
        }
        for metric in &self.model.metrics {
            self.validate_metric(metric);
        }
        self.validate_metric_cycles();
        self.validate_policies();
        for calendar in &self.model.calendars {
            self.calendar(&calendar.profile, calendar.metadata.id());
        }
    }

    fn validate_policies(&mut self) {
        for policy in &self.model.policies {
            let owner = policy.metadata.id();
            let local = match &policy.scope {
                PolicyScope::Dataset(id) => {
                    if self.dataset(id).is_none() {
                        self.missing(owner, id);
                    }
                    Some(id)
                }
                PolicyScope::Metric(id) => {
                    if self.metric(id).is_none() {
                        self.missing(owner, id);
                    }
                    None
                }
            };
            let mut names = BTreeSet::new();
            for slot in &policy.context_slots {
                if !valid_segment(&slot.name) || !names.insert(&slot.name) {
                    self.error(
                        ErrorCode::PolicyViolation,
                        owner,
                        "Policy slot names must be valid and unique",
                    );
                }
                self.value_type(&slot.value_type, owner);
            }
            self.scalar(
                &policy.predicate,
                owner,
                local,
                Some(&policy.context_slots),
                0,
            );
            self.boolean(&policy.predicate, owner);
        }
    }

    fn value_type(&mut self, ty: &ValueType, owner: &StableId) {
        if let Err(mut error) = ty.validate() {
            error.primary_ref = Some(owner.clone());
            self.errors.push(error);
            return;
        }
        match ty {
            ValueType::Identifier { entity_key } => {
                if self.key(entity_key).is_none() {
                    self.missing(owner, entity_key);
                }
            }
            ValueType::List(inner) => self.value_type(inner, owner),
            ValueType::Struct(fields) => {
                for field in fields {
                    self.value_type(&field.value_type, owner);
                }
            }
            _ => {}
        }
    }
    fn unit(&mut self, unit: Option<&Unit>, owner: &StableId) {
        if let Some(unit) = unit
            && let Err(mut error) = unit.validate()
        {
            error.primary_ref = Some(owner.clone());
            self.errors.push(error);
        }
    }
    fn grain(&mut self, grain: &Grain, owner: &StableId) {
        for atom in grain.atoms() {
            self.atom(atom, owner);
        }
    }
    fn atom(&mut self, atom: &GrainAtom, owner: &StableId) {
        match atom {
            GrainAtom::EntityKeyRef(id) => {
                if self.key(id).is_none() {
                    self.missing(owner, id);
                }
            }
            GrainAtom::DimensionRef { dimension, roles } => {
                if self.dimension(dimension).is_none() {
                    self.missing(owner, dimension);
                }
                for role in roles {
                    if !self.model.relationships.iter().any(|r| &r.role == role) {
                        self.error(
                            ErrorCode::InvalidRelationshipContract,
                            owner,
                            "Grain role does not resolve",
                        );
                    }
                }
            }
            GrainAtom::TimeBucketRef(bucket) => self.bucket(bucket, owner),
        }
    }
    fn calendar(&mut self, calendar: &CalendarProfile, owner: &StableId) {
        if let CalendarProfile::Custom { id } | CalendarProfile::Fiscal { id } = calendar
            && !self.model.calendars.iter().any(|c| c.metadata.id() == id)
        {
            self.missing(owner, id);
        }
    }
    fn bucket(&mut self, bucket: &TimeBucket, owner: &StableId) {
        self.calendar(&bucket.calendar, owner);
        let ty = self
            .field(&bucket.source)
            .map(|f| &f.value_type)
            .or_else(|| self.dimension(&bucket.source).map(|d| &d.value_type));
        match ty {
            Some(ty) if is_temporal(ty) => {}
            Some(_) => self.error(
                ErrorCode::InvalidTemporalContract,
                owner,
                "Time bucket source is not temporal",
            ),
            None => self.missing(owner, &bucket.source),
        }
    }
    fn same(
        &mut self,
        actual: &ValueType,
        unit: Option<&Unit>,
        expected: &ValueType,
        expected_unit: Option<&Unit>,
        owner: &StableId,
    ) {
        if actual != expected {
            self.error(
                ErrorCode::IncompatibleTypes,
                owner,
                "Declared expression type does not match resolved semantic type",
            );
        }
        if unit != expected_unit {
            self.error(
                ErrorCode::IncompatibleUnits,
                owner,
                "Declared expression unit does not match resolved semantic unit",
            );
        }
    }
    fn boolean(&mut self, expr: &ScalarExpr, owner: &StableId) {
        if expr.value_type != ValueType::Boolean || expr.unit.is_some() {
            self.error(
                ErrorCode::IncompatibleTypes,
                owner,
                "Predicate must be a unitless boolean",
            );
        }
    }

    fn validate_dataset(&mut self, dataset: &SemanticDataset) {
        let owner = dataset.metadata.id();
        self.grain(&dataset.observation_grain, owner);
        let source = self
            .model
            .source_bindings
            .iter()
            .find(|s| s.metadata.id() == &dataset.source_binding);
        if source.is_none() {
            self.missing(owner, &dataset.source_binding);
        }
        for field in &dataset.fields {
            if &field.dataset != owner {
                self.error(
                    ErrorCode::InvalidModelReference,
                    field.metadata.id(),
                    "Field owner does not match its dataset",
                );
            }
            self.value_type(&field.value_type, field.metadata.id());
            self.unit(field.unit.as_ref(), field.metadata.id());
            if let Some(source) = source
                && !source
                    .field_bindings
                    .iter()
                    .any(|b| b.field == *field.metadata.id() && b.handle == field.binding)
            {
                self.error(
                    ErrorCode::InvalidModelReference,
                    field.metadata.id(),
                    "Field has no matching logical source binding",
                );
            }
        }
        let mut keys = BTreeSet::new();
        for binding in &dataset.key_bindings {
            if !keys.insert((&binding.entity_key, &binding.fields)) || binding.fields.is_empty() {
                self.error(
                    ErrorCode::InvalidRelationshipContract,
                    owner,
                    "Key binding components must be nonempty and each domain/field binding unique",
                );
            }
            let fields: Vec<_> = binding
                .fields
                .iter()
                .filter_map(|id| self.field(id))
                .collect();
            for id in &binding.fields {
                match self.field(id) {
                    Some(f) if f.dataset == *owner => {}
                    _ => self.missing(owner, id),
                }
            }
            if fields.len() == binding.fields.len() {
                if let Some(key) = self.key(&binding.entity_key) {
                    let compatible = if fields.len() == 1 {
                        fields[0].value_type == key.value_type
                            || matches!(&fields[0].value_type,ValueType::Identifier{entity_key} if entity_key == &binding.entity_key)
                    } else {
                        matches!(&key.value_type,ValueType::Struct(parts) if parts.len()==fields.len() && parts.iter().zip(&fields).all(|(p,f)|p.value_type==f.value_type))
                    };
                    if !compatible {
                        self.error(
                            ErrorCode::IncompatibleTypes,
                            owner,
                            "Entity key components and bound fields have incompatible types",
                        );
                    }
                } else {
                    self.missing(owner, &binding.entity_key);
                }
            }
            if binding.unique && binding.evidence.is_invalid() {
                self.error(
                    ErrorCode::UniquenessContractViolation,
                    owner,
                    "Declared unique key has invalid evidence",
                );
            }
            if binding.fields.iter().collect::<BTreeSet<_>>().len() != binding.fields.len() {
                self.error(
                    ErrorCode::InvalidRelationshipContract,
                    owner,
                    "Key components may not repeat",
                );
            }
        }
        for role in &dataset.time_roles {
            match self.field(&role.field) {
                Some(f) if f.dataset == *owner && is_temporal(&f.value_type) => {}
                _ => self.error(
                    ErrorCode::InvalidTemporalContract,
                    owner,
                    "Time role requires a local temporal field",
                ),
            }
        }
        self.policy_refs(&dataset.policy_refs, owner, Some(owner));
    }

    fn validate_dimension(&mut self, dimension: &Dimension) {
        let owner = dimension.metadata.id();
        self.value_type(&dimension.value_type, owner);
        self.unit(dimension.unit.as_ref(), owner);
        let declared: BTreeSet<_> = dimension.participating_datasets.iter().collect();
        let bound: BTreeSet<_> = dimension.bindings.iter().map(|b| &b.dataset).collect();
        if declared.is_empty()
            || declared != bound
            || bound.len() != dimension.bindings.len()
            || declared.len() != dimension.participating_datasets.len()
        {
            self.error(ErrorCode::InvalidModelReference,owner,"Conformed dimension requires exactly one binding for each declared participating dataset");
        }
        for binding in &dimension.bindings {
            if self.dataset(&binding.dataset).is_none() {
                self.missing(owner, &binding.dataset);
            }
            self.scalar(&binding.expression, owner, Some(&binding.dataset), None, 0);
            self.same(
                &binding.expression.value_type,
                binding.expression.unit.as_ref(),
                &dimension.value_type,
                dimension.unit.as_ref(),
                owner,
            );
            match &binding.conformance {
                ConformanceContract::ExactDomain => {}
                ConformanceContract::NormalizedDomain { normalization } => {
                    self.scalar(normalization, owner, Some(&binding.dataset), None, 0);
                    self.same(
                        &normalization.value_type,
                        normalization.unit.as_ref(),
                        &dimension.value_type,
                        dimension.unit.as_ref(),
                        owner,
                    );
                }
                ConformanceContract::ExplicitMapping {
                    relationship,
                    canonical_value,
                } => {
                    let mapping = self.relationship(relationship);
                    if mapping.is_none() {
                        self.missing(owner, relationship);
                    }
                    if let Some(field) = self.field(canonical_value) {
                        self.same(
                            &field.value_type,
                            field.unit.as_ref(),
                            &dimension.value_type,
                            dimension.unit.as_ref(),
                            owner,
                        );
                        if mapping.is_some_and(|r| {
                            !((r.from_dataset == binding.dataset && r.to_dataset == field.dataset)
                                || (r.to_dataset == binding.dataset
                                    && r.from_dataset == field.dataset))
                        }) {
                            self.error(ErrorCode::InvalidModelReference,owner,"Explicit conformance mapping must connect the binding dataset to the canonical field dataset");
                        }
                    } else {
                        self.missing(owner, canonical_value);
                    }
                }
            }
        }
        if let Some(time) = &dimension.time_semantics {
            if !is_temporal(&dimension.value_type) {
                self.error(
                    ErrorCode::InvalidTemporalContract,
                    owner,
                    "Time semantics require a temporal dimension",
                );
            }
            self.calendar(&time.calendar, owner);
        }
    }

    fn aggregation(&mut self, contract: &AggregationContract, owner: &StableId) {
        let mut axes = BTreeSet::new();
        for rule in &contract.axis_rules {
            self.atom(&rule.axis, owner);
            if !axes.insert(&rule.axis) {
                self.error(
                    ErrorCode::InvalidMetricRecomposition,
                    owner,
                    "Axis aggregation rules must be unique",
                );
            }
        }
        if contract.classification == AggregationClass::Recomputable
            && contract.decomposition.is_none()
        {
            self.error(
                ErrorCode::InvalidMetricRecomposition,
                owner,
                "Recomputable metric requires sufficient components and recomposition",
            );
        }
        if let Some(decomposition) = &contract.decomposition {
            if decomposition.components.is_empty() {
                self.error(
                    ErrorCode::InvalidMetricRecomposition,
                    owner,
                    "Recomposition requires components",
                );
            }
            for id in &decomposition.components {
                if self.measure(id).is_none() && self.metric(id).is_none() {
                    self.missing(owner, id);
                }
            }
            self.metric_expression(&decomposition.recomposition, owner, 0);
            let refs: BTreeSet<_> = decomposition
                .recomposition
                .measure_refs()
                .into_iter()
                .chain(decomposition.recomposition.metric_refs())
                .collect();
            if refs != decomposition.components.iter().cloned().collect() {
                self.error(
                    ErrorCode::InvalidMetricRecomposition,
                    owner,
                    "Recomposition references must equal declared components",
                );
            }
        }
    }

    fn validate_measure(&mut self, measure: &Measure) {
        let owner = measure.metadata.id();
        self.value_type(&measure.value_type, owner);
        self.unit(measure.unit.as_ref(), owner);
        self.grain(&measure.input_grain, owner);
        self.aggregation(&measure.aggregation_contract, owner);
        if let Some(decomposition) = &measure.aggregation_contract.decomposition {
            self.same(
                &decomposition.recomposition.value_type,
                decomposition.recomposition.unit.as_ref(),
                &measure.value_type,
                measure.unit.as_ref(),
                owner,
            );
        }
        if let Some(dataset) = self.dataset(&measure.dataset) {
            if dataset.observation_grain != measure.input_grain {
                self.error(
                    ErrorCode::InvalidGrain,
                    owner,
                    "Primitive measure input grain must equal its dataset observation grain",
                );
            }
        } else {
            self.missing(owner, &measure.dataset);
        }
        if let Some(expr) = &measure.scalar_expression {
            self.scalar(expr, owner, Some(&measure.dataset), None, 0);
        }
        if let Some(filter) = &measure.intrinsic_filter {
            self.scalar(filter, owner, Some(&measure.dataset), None, 0);
            self.boolean(filter, owner);
        }
        self.validate_measure_aggregation(measure);
    }

    fn validate_measure_aggregation(&mut self, measure: &Measure) {
        let owner = measure.metadata.id();
        match measure.aggregation {
            AggregateFunction::CountRows => {
                if measure.scalar_expression.is_some()
                    || measure.null_semantics != AggregateNullSemantics::CountRows
                {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "COUNT rows has no scalar input and counts rows including nulls",
                    );
                }
                if !matches!(measure.value_type, ValueType::Integer { .. }) {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "Count result must be integer",
                    );
                }
            }
            AggregateFunction::CountNonNull | AggregateFunction::CountDistinct => {
                if measure.scalar_expression.is_none()
                    || measure.null_semantics != AggregateNullSemantics::CountNonNull
                    || !matches!(measure.value_type, ValueType::Integer { .. })
                {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "Value count requires an input and non-null count semantics",
                    );
                }
                if measure.aggregation == AggregateFunction::CountDistinct
                    && measure.aggregation_contract.classification
                        == AggregationClass::FullyAdditive
                {
                    self.error(
                        ErrorCode::InvalidMetricRecomposition,
                        owner,
                        "Exact distinct counts cannot be fully additive",
                    );
                }
            }
            AggregateFunction::Sum | AggregateFunction::Min | AggregateFunction::Max => {
                if let Some(expr) = &measure.scalar_expression {
                    self.same(
                        &expr.value_type,
                        expr.unit.as_ref(),
                        &measure.value_type,
                        measure.unit.as_ref(),
                        owner,
                    );
                    if measure.aggregation == AggregateFunction::Sum && !numeric(&expr.value_type) {
                        self.error(
                            ErrorCode::IncompatibleTypes,
                            owner,
                            "SUM requires a numeric expression",
                        );
                    }
                } else {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "Aggregate requires a scalar expression",
                    );
                }
                if measure.null_semantics != AggregateNullSemantics::IgnoreNullsNullIfEmpty {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "Aggregate must preserve declared null elision and empty-set null",
                    );
                }
            }
        }
    }

    fn validate_relationship(&mut self, relationship: &Relationship) {
        let owner = relationship.metadata.id();
        let from = self.dataset(&relationship.from_dataset);
        let to = self.dataset(&relationship.to_dataset);
        if from.is_none() {
            self.missing(owner, &relationship.from_dataset);
        }
        if to.is_none() {
            self.missing(owner, &relationship.to_dataset);
        }
        if relationship.role.is_empty()
            || !valid_segment(&relationship.role)
            || relationship.key_pairs.is_empty()
        {
            self.error(
                ErrorCode::InvalidRelationshipContract,
                owner,
                "Relationship requires a semantic role and nonempty key pairs",
            );
        }
        if relationship.evidence.is_invalid() {
            self.error(
                ErrorCode::InvalidRelationshipContract,
                owner,
                "Relationship contract evidence is invalid",
            );
        }
        let from_fields: BTreeSet<_> = relationship
            .key_pairs
            .iter()
            .map(|p| &p.from_field)
            .collect();
        let to_fields: BTreeSet<_> = relationship.key_pairs.iter().map(|p| &p.to_field).collect();
        if from_fields.len() != relationship.key_pairs.len()
            || to_fields.len() != relationship.key_pairs.len()
        {
            self.error(
                ErrorCode::InvalidRelationshipContract,
                owner,
                "Relationship key pairs must not repeat fields",
            );
        }
        self.validate_relationship_pairs(relationship);
        if let (Some(from), Some(to)) = (from, to) {
            let compatible_domain = from.key_bindings.iter().any(|left| {
                left.fields.iter().collect::<BTreeSet<_>>() == from_fields
                    && to.key_bindings.iter().any(|right| {
                        left.entity_key == right.entity_key
                            && right.fields.iter().collect::<BTreeSet<_>>() == to_fields
                            && left.fields.iter().zip(&right.fields).all(|(l, r)| {
                                relationship
                                    .key_pairs
                                    .iter()
                                    .any(|p| &p.from_field == l && &p.to_field == r)
                            })
                    })
            });
            if !compatible_domain {
                self.error(
                    ErrorCode::InvalidRelationshipContract,
                    owner,
                    "Relationship fields must bind the same semantic entity-key domain",
                );
            }
            if matches!(
                relationship.cardinality,
                Cardinality::OneToOne | Cardinality::OneToMany
            ) && !unique_cover(from, &from_fields)
            {
                self.error(
                    ErrorCode::InvalidRelationshipContract,
                    owner,
                    "Declared source one-side lacks a uniqueness contract",
                );
            }
            if matches!(
                relationship.cardinality,
                Cardinality::OneToOne | Cardinality::ManyToOne
            ) {
                let mut target_key = to_fields.clone();
                if let Some(temporal) = &relationship.temporal_constraint {
                    target_key.insert(&temporal.valid_from);
                }
                if !unique_cover(to, &target_key) {
                    self.error(
                        ErrorCode::InvalidRelationshipContract,
                        owner,
                        "Declared target one-side lacks a uniqueness contract",
                    );
                }
            }
        }
        self.validate_relationship_time(relationship);
    }

    fn validate_relationship_pairs(&mut self, relationship: &Relationship) {
        let owner = relationship.metadata.id();
        for pair in &relationship.key_pairs {
            match (self.field(&pair.from_field), self.field(&pair.to_field)) {
                (Some(left), Some(right))
                    if left.dataset == relationship.from_dataset
                        && right.dataset == relationship.to_dataset =>
                {
                    if left.value_type != right.value_type || left.unit != right.unit {
                        self.error(
                            ErrorCode::InvalidRelationshipContract,
                            owner,
                            "Relationship keys have incompatible types or units",
                        );
                    }
                }
                _ => self.error(
                    ErrorCode::InvalidRelationshipContract,
                    owner,
                    "Relationship field does not belong to the declared endpoint",
                ),
            }
        }
    }

    fn validate_relationship_time(&mut self, relationship: &Relationship) {
        let owner = relationship.metadata.id();
        if let Some(temporal) = &relationship.temporal_constraint {
            let event = self.field(&temporal.event_time);
            let start = self.field(&temporal.valid_from);
            if !matches!((event,start),(Some(e),Some(s)) if e.dataset == relationship.from_dataset && s.dataset == relationship.to_dataset && is_temporal(&e.value_type) && e.value_type == s.value_type)
            {
                self.error(
                    ErrorCode::InvalidTemporalContract,
                    owner,
                    "As-of fields require compatible temporal types at their declared endpoints",
                );
            }
            for (dataset, field, role) in [
                (
                    &relationship.from_dataset,
                    &temporal.event_time,
                    TimeRole::EventTime,
                ),
                (
                    &relationship.to_dataset,
                    &temporal.valid_from,
                    TimeRole::ValidityStart,
                ),
            ] {
                if !self.dataset(dataset).is_some_and(|d| {
                    d.time_roles
                        .iter()
                        .any(|r| r.field == *field && r.role == role)
                }) {
                    self.error(
                        ErrorCode::InvalidTemporalContract,
                        owner,
                        "As-of relationship requires explicit event and validity roles",
                    );
                }
            }
            if let Some(end) = &temporal.valid_to
                && (!matches!((self.field(end),start),(Some(e),Some(s)) if e.dataset==relationship.to_dataset && e.value_type==s.value_type)
                    || !self.dataset(&relationship.to_dataset).is_some_and(|d| {
                        d.time_roles
                            .iter()
                            .any(|r| r.field == *end && r.role == TimeRole::ValidityEnd)
                    }))
            {
                self.error(
                    ErrorCode::InvalidTemporalContract,
                    owner,
                    "Validity end requires compatible target field and validity-end role",
                );
            }
            if temporal.evidence.is_invalid() {
                self.error(
                    ErrorCode::TemporalMatchContractViolation,
                    owner,
                    "Temporal contract has invalid evidence",
                );
            }
        }
    }

    fn validate_bridge(&mut self, bridge: &Bridge) {
        let owner = bridge.metadata.id();
        for dataset in [
            &bridge.left_dataset,
            &bridge.bridge_dataset,
            &bridge.right_dataset,
        ] {
            if self.dataset(dataset).is_none() {
                self.missing(owner, dataset);
            }
        }
        for (relationship, endpoint) in [
            (&bridge.left_relationship, &bridge.left_dataset),
            (&bridge.right_relationship, &bridge.right_dataset),
        ] {
            match self.relationship(relationship) {
                Some(r)
                    if (r.from_dataset == bridge.bridge_dataset && r.to_dataset == *endpoint)
                        || (r.to_dataset == bridge.bridge_dataset
                            && r.from_dataset == *endpoint) => {}
                _ => self.error(
                    ErrorCode::InvalidAllocationContract,
                    owner,
                    "Bridge relationships must connect its declared endpoints",
                ),
            }
        }
        if let Some(allocation) = &bridge.allocation {
            self.scalar(
                &allocation.weight_expression,
                owner,
                Some(&bridge.bridge_dataset),
                None,
                0,
            );
            self.grain(&allocation.normalization_scope, owner);
            if !matches!(
                allocation.weight_expression.value_type,
                ValueType::Decimal(_)
            ) || !matches!(allocation.weight_expression.unit, None | Some(Unit::Ratio))
                || allocation.required_total.is_zero()
                || allocation.required_total.is_negative()
                || allocation.tolerance.is_negative()
            {
                self.error(ErrorCode::InvalidAllocationContract,owner,"Allocation requires exact weights, a normalization scope, positive total and nonnegative tolerance");
            }
            if allocation.evidence.is_invalid() {
                self.error(
                    ErrorCode::AllocationContractViolation,
                    owner,
                    "Allocation contract has invalid evidence",
                );
            }
        }
    }

    fn validate_conversion(&mut self, conversion: &UnitConversion) {
        let owner = conversion.metadata.id();
        self.value_type(&conversion.result_type, owner);
        self.unit(Some(&conversion.from), owner);
        self.unit(Some(&conversion.to), owner);
        if !numeric(&conversion.result_type) {
            self.error(
                ErrorCode::IncompatibleTypes,
                owner,
                "Unit conversion requires a numeric result",
            );
        }
        match &conversion.rule {
            ConversionRule::Linear { factor, .. } => {
                if factor.is_zero()
                    || matches!((&conversion.from,&conversion.to),(Unit::Currency{..},Unit::Currency{..}) if conversion.from!=conversion.to)
                {
                    self.error(ErrorCode::MissingUnitConversion,owner,"Currency exchange requires rate and time context; a linear factor must be nonzero");
                }
            }
            ConversionRule::Fx {
                rate_dimension,
                time_dimension,
            } => {
                if !self
                    .dimension(rate_dimension)
                    .is_some_and(|d| numeric(&d.value_type))
                {
                    self.missing(owner, rate_dimension);
                }
                if !self
                    .dimension(time_dimension)
                    .is_some_and(|d| is_temporal(&d.value_type))
                {
                    self.missing(owner, time_dimension);
                }
                if !matches!(
                    (&conversion.from, &conversion.to),
                    (Unit::Currency { .. }, Unit::Currency { .. })
                ) {
                    self.error(
                        ErrorCode::IncompatibleUnits,
                        owner,
                        "FX conversion requires currencies",
                    );
                }
            }
        }
    }

    fn validate_metric(&mut self, metric: &Metric) {
        let owner = metric.metadata.id();
        self.value_type(&metric.value_type, owner);
        self.unit(metric.unit.as_ref(), owner);
        self.grain(&metric.native_grain, owner);
        self.aggregation(&metric.aggregation_contract, owner);
        self.metric_expression(&metric.expression, owner, 0);
        if let Some(decomposition) = &metric.aggregation_contract.decomposition {
            self.same(
                &decomposition.recomposition.value_type,
                decomposition.recomposition.unit.as_ref(),
                &metric.value_type,
                metric.unit.as_ref(),
                owner,
            );
        }
        self.same(
            &metric.expression.value_type,
            metric.expression.unit.as_ref(),
            &metric.value_type,
            metric.unit.as_ref(),
            owner,
        );
        let mut refs: BTreeSet<_> = metric
            .expression
            .measure_refs()
            .into_iter()
            .chain(metric.expression.metric_refs())
            .collect();
        if let Some(decomposition) = &metric.aggregation_contract.decomposition {
            refs.extend(decomposition.components.iter().cloned());
        }
        if let Some(WindowBehavior::Cumulative {
            base_metric,
            prerequisite_grain,
            order_by,
            partition_by,
            ..
        }) = &metric.window_behavior
        {
            refs.insert(base_metric.clone());
            self.grain(prerequisite_grain, owner);
            self.grain(partition_by, owner);
            if self.metric(base_metric).is_none() {
                self.missing(owner, base_metric);
            }
            if order_by.is_empty()
                || metric.aggregation_contract.classification != AggregationClass::OrderedWindow
                || !matches!(&metric.expression.kind,MetricExprKind::Window{base_metric:id} if id==base_metric)
            {
                self.error(ErrorCode::InvalidWindowSemantics,owner,"Cumulative window requires matching base metric, prerequisite grain and ordered-window classification");
            }
            self.order(order_by, owner);
        } else if matches!(metric.expression.kind, MetricExprKind::Window { .. })
            || metric.aggregation_contract.classification == AggregationClass::OrderedWindow
        {
            self.error(
                ErrorCode::InvalidWindowSemantics,
                owner,
                "Window expression requires an explicit cumulative contract",
            );
        }
        if refs != metric.dependencies.iter().cloned().collect()
            || metric.dependencies.len() != refs.len()
        {
            self.error(
                ErrorCode::InvalidMetricRecomposition,
                owner,
                "Metric dependency declaration must match definition references exactly",
            );
        }
        for id in &metric.dependencies {
            if self.metric(id).is_none() && self.measure(id).is_none() {
                self.missing(owner, id);
            }
        }
        if let Some(filter) = &metric.intrinsic_filter {
            self.scalar(filter, owner, None, None, 0);
            self.boolean(filter, owner);
        }
        self.policy_refs(&metric.policy_refs, owner, None);
        self.validate_metric_time(metric);
        if let Some(bridge_id) = &metric.allocation_bridge
            && !self
                .model
                .bridges
                .iter()
                .any(|b| b.metadata.id() == bridge_id && b.allocation.is_some())
        {
            self.error(
                ErrorCode::InvalidAllocationContract,
                owner,
                "Allocated metric requires an explicit bridge allocation",
            );
        }
    }
    fn validate_metric_time(&mut self, metric: &Metric) {
        let owner = metric.metadata.id();
        if let Some(temporal) = &metric.time_behavior {
            self.atom(&temporal.axis, owner);
            self.grain(&temporal.selection_scope, owner);
            self.order(&temporal.order_by, owner);
            if temporal.order_by.is_empty() {
                self.error(
                    ErrorCode::InvalidTemporalContract,
                    owner,
                    "Temporal selection requires an ordering",
                );
            }
            match &temporal.ties {
                TemporalTies::Unspecified => self.error(
                    ErrorCode::AmbiguousTemporalSelection,
                    owner,
                    "Temporal selection must be single-valued or declare a secondary key",
                ),
                TemporalTies::SecondaryKey { fields } => {
                    if fields.is_empty() {
                        self.error(
                            ErrorCode::AmbiguousTemporalSelection,
                            owner,
                            "Secondary temporal key cannot be empty",
                        );
                    }
                    self.order(fields, owner);
                }
                TemporalTies::UniqueByContract { key } => {
                    if self.key(key).is_none()
                        || !self.model.datasets.iter().any(|d| {
                            d.key_bindings
                                .iter()
                                .any(|b| b.entity_key == *key && b.unique)
                        })
                    {
                        self.error(
                            ErrorCode::AmbiguousTemporalSelection,
                            owner,
                            "Temporal uniqueness requires an existing unique key contract",
                        );
                    }
                }
            }
        }
    }

    fn order(&mut self, terms: &[crate::time::OrderTerm], owner: &StableId) {
        let mut refs = BTreeSet::new();
        for term in terms {
            if self.field(&term.value).is_none() && self.dimension(&term.value).is_none() {
                self.missing(owner, &term.value);
            }
            if !refs.insert(&term.value) {
                self.error(
                    ErrorCode::InvalidTemporalContract,
                    owner,
                    "Order terms must be unique",
                );
            }
        }
    }
    fn policy_refs(&mut self, refs: &[StableId], owner: &StableId, dataset: Option<&StableId>) {
        for id in refs {
            match self.model.policies.iter().find(|p| p.metadata.id() == id) {
                Some(policy) => {
                    let applicable = match (&policy.scope, dataset) {
                        (PolicyScope::Dataset(target), Some(dataset)) => target == dataset,
                        (PolicyScope::Metric(target), None) => target == owner,
                        (PolicyScope::Dataset(_), None) => true,
                        _ => false,
                    };
                    if !applicable {
                        self.error(
                            ErrorCode::PolicyViolation,
                            owner,
                            "Policy reference does not match its declared scope",
                        );
                    }
                }
                None => self.missing(owner, id),
            }
        }
    }

    fn validate_metric_cycles(&mut self) {
        let graph: BTreeMap<_, _> = self
            .model
            .metrics
            .iter()
            .map(|m| {
                let mut refs = m.expression.metric_refs();
                if let Some(decomposition) = &m.aggregation_contract.decomposition {
                    refs.extend(decomposition.recomposition.metric_refs());
                }
                if let Some(WindowBehavior::Cumulative { base_metric, .. }) = &m.window_behavior {
                    refs.push(base_metric.clone());
                }
                (m.metadata.id().clone(), refs)
            })
            .collect();
        for id in cycle_members(&graph) {
            self.error(
                ErrorCode::MetricDependencyCycle,
                &id,
                "Metric definitions must have an acyclic dependency graph",
            );
        }
    }
    fn validate_dimension_cycles(&mut self) {
        let graph: BTreeMap<_, _> = self
            .model
            .dimensions
            .iter()
            .map(|d| {
                (
                    d.metadata.id().clone(),
                    d.bindings
                        .iter()
                        .flat_map(|b| {
                            let mut refs = b.expression.dimension_refs();
                            if let ConformanceContract::NormalizedDomain { normalization } =
                                &b.conformance
                            {
                                refs.extend(normalization.dimension_refs());
                            }
                            refs
                        })
                        .collect(),
                )
            })
            .collect();
        for id in cycle_members(&graph) {
            self.error(
                ErrorCode::InvalidModelReference,
                &id,
                "Dimension definitions cannot contain recursive references",
            );
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep the exhaustive scalar-expression validation table together so every variant and its recursive checks can be audited in one place."
    )]
    fn scalar(
        &mut self,
        expr: &ScalarExpr,
        owner: &StableId,
        dataset: Option<&StableId>,
        slots: Option<&[ContextSlot]>,
        depth: usize,
    ) {
        if depth > 128 {
            self.error(
                ErrorCode::UnsupportedSemanticConstruct,
                owner,
                "Expression nesting exceeds the bounded compilation limit",
            );
            return;
        }
        self.value_type(&expr.value_type, owner);
        self.unit(expr.unit.as_ref(), owner);
        match &expr.kind {
            ScalarExprKind::Literal(literal) => {
                if let Err(mut error) = literal.validate() {
                    error.primary_ref = Some(owner.clone());
                    self.errors.push(error);
                }
                if literal.value_type() != expr.value_type {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "Literal type differs from its declared expression type",
                    );
                }
            }
            ScalarExprKind::FieldRef(id) => match self.field(id) {
                Some(field) => {
                    self.same(
                        &expr.value_type,
                        expr.unit.as_ref(),
                        &field.value_type,
                        field.unit.as_ref(),
                        owner,
                    );
                    if dataset.is_some_and(|id| id != &field.dataset) {
                        self.error(
                            ErrorCode::InvalidModelReference,
                            owner,
                            "Scalar definition violates dataset locality",
                        );
                    }
                }
                None => self.missing(owner, id),
            },
            ScalarExprKind::DimensionRef(id) => match self.dimension(id) {
                Some(d) => {
                    self.same(
                        &expr.value_type,
                        expr.unit.as_ref(),
                        &d.value_type,
                        d.unit.as_ref(),
                        owner,
                    );
                    if dataset.is_some_and(|id| !d.bindings.iter().any(|b| &b.dataset == id)) {
                        self.error(
                            ErrorCode::InvalidModelReference,
                            owner,
                            "Local expression references a dimension with no local binding",
                        );
                    }
                }
                None => self.missing(owner, id),
            },
            ScalarExprKind::ContextSlot(name) => {
                if let Some(slot) = slots.and_then(|s| s.iter().find(|s| &s.name == name)) {
                    self.same(
                        &expr.value_type,
                        expr.unit.as_ref(),
                        &slot.value_type,
                        None,
                        owner,
                    );
                } else {
                    self.error(ErrorCode::PolicyViolation,owner,"Context references are permitted only in policies with declared typed slots");
                }
            }
            ScalarExprKind::Unary { op, operand } => {
                self.scalar(operand, owner, dataset, slots, depth + 1);
                match op {
                    UnaryOp::Not => {
                        self.boolean(operand, owner);
                        self.boolean(expr, owner);
                    }
                    UnaryOp::Negate => {
                        if !numeric(&operand.value_type) {
                            self.error(
                                ErrorCode::IncompatibleTypes,
                                owner,
                                "Unary arithmetic requires numeric input",
                            );
                        }
                        self.same(
                            &expr.value_type,
                            expr.unit.as_ref(),
                            &operand.value_type,
                            operand.unit.as_ref(),
                            owner,
                        );
                    }
                }
            }
            ScalarExprKind::Arithmetic {
                op,
                left,
                right,
                contract,
            } => {
                self.scalar(left, owner, dataset, slots, depth + 1);
                self.scalar(right, owner, dataset, slots, depth + 1);
                if let Err(mut error) = validate_arithmetic(
                    *op,
                    &left.value_type,
                    &right.value_type,
                    &expr.value_type,
                    contract,
                ) {
                    error.primary_ref = Some(owner.clone());
                    self.errors.push(error);
                }
                self.arithmetic_units(
                    *op,
                    left.unit.as_ref(),
                    right.unit.as_ref(),
                    expr.unit.as_ref(),
                    owner,
                );
            }
            ScalarExprKind::Comparison { left, right, .. } => {
                self.scalar(left, owner, dataset, slots, depth + 1);
                self.scalar(right, owner, dataset, slots, depth + 1);
                self.same(
                    &left.value_type,
                    left.unit.as_ref(),
                    &right.value_type,
                    right.unit.as_ref(),
                    owner,
                );
                self.boolean(expr, owner);
            }
            ScalarExprKind::BooleanLogic { operands, .. } => {
                if operands.is_empty() {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "Boolean combination cannot be empty",
                    );
                }
                for child in operands {
                    self.scalar(child, owner, dataset, slots, depth + 1);
                    self.boolean(child, owner);
                }
                self.boolean(expr, owner);
            }
            ScalarExprKind::Case {
                branches,
                otherwise,
            } => {
                if branches.is_empty() {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "CASE requires at least one branch",
                    );
                }
                for (condition, value) in branches {
                    self.scalar(condition, owner, dataset, slots, depth + 1);
                    self.boolean(condition, owner);
                    self.scalar(value, owner, dataset, slots, depth + 1);
                    self.same(
                        &value.value_type,
                        value.unit.as_ref(),
                        &expr.value_type,
                        expr.unit.as_ref(),
                        owner,
                    );
                }
                self.scalar(otherwise, owner, dataset, slots, depth + 1);
                self.same(
                    &otherwise.value_type,
                    otherwise.unit.as_ref(),
                    &expr.value_type,
                    expr.unit.as_ref(),
                    owner,
                );
            }
            ScalarExprKind::NullTest { operand, .. } => {
                self.scalar(operand, owner, dataset, slots, depth + 1);
                self.boolean(expr, owner);
            }
            ScalarExprKind::ExplicitCast { operand, rounding } => {
                self.scalar(operand, owner, dataset, slots, depth + 1);
                if operand.unit != expr.unit {
                    self.error(
                        ErrorCode::IncompatibleUnits,
                        owner,
                        "A cast cannot change semantic units",
                    );
                }
                if let (ValueType::Decimal(from), ValueType::Decimal(to)) =
                    (&operand.value_type, &expr.value_type)
                {
                    if to.scale() < from.scale() && rounding.is_none() {
                        self.error(
                            ErrorCode::NumericOverflow,
                            owner,
                            "Scale reduction requires an explicit rounding mode",
                        );
                    }
                } else if operand.value_type != expr.value_type
                    && !(numeric(&operand.value_type) && numeric(&expr.value_type))
                {
                    self.error(
                        ErrorCode::UnsupportedSemanticConstruct,
                        owner,
                        "Cast has no defined lossless or explicit numeric contract",
                    );
                }
                if matches!((&operand.value_type,&expr.value_type),(ValueType::Decimal(from),ValueType::Integer{..}) if from.scale()>0)
                    && rounding.is_none()
                {
                    self.error(
                        ErrorCode::NumericOverflow,
                        owner,
                        "Decimal-to-integer scale reduction requires explicit rounding",
                    );
                }
            }
            ScalarExprKind::TemporalDerivation { operand, bucket } => {
                self.scalar(operand, owner, dataset, slots, depth + 1);
                self.bucket(bucket, owner);
                if !is_temporal(&operand.value_type)
                    || !is_temporal(&expr.value_type)
                    || expr.unit.is_some()
                {
                    self.error(
                        ErrorCode::InvalidTemporalContract,
                        owner,
                        "Temporal derivation requires unitless temporal values",
                    );
                }
            }
            ScalarExprKind::UnitConversion {
                operand,
                conversion,
            } => {
                self.scalar(operand, owner, dataset, slots, depth + 1);
                if !numeric(&operand.value_type) {
                    self.error(
                        ErrorCode::IncompatibleTypes,
                        owner,
                        "Unit conversion requires numeric input",
                    );
                }
                if let Some(c) = self
                    .model
                    .unit_conversions
                    .iter()
                    .find(|c| c.metadata.id() == conversion)
                {
                    self.same(
                        &expr.value_type,
                        expr.unit.as_ref(),
                        &c.result_type,
                        Some(&c.to),
                        owner,
                    );
                    if operand.unit.as_ref() != Some(&c.from) {
                        self.error(
                            ErrorCode::IncompatibleUnits,
                            owner,
                            "Conversion input unit differs from declared source unit",
                        );
                    }
                } else {
                    self.missing(owner, conversion);
                }
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep the exhaustive metric-expression validation table together, including the numeric and unit contracts for each variant."
    )]
    fn metric_expression(&mut self, expr: &MetricExpr, owner: &StableId, depth: usize) {
        if depth > 128 {
            self.error(
                ErrorCode::UnsupportedSemanticConstruct,
                owner,
                "Metric expression nesting exceeds the bounded compilation limit",
            );
            return;
        }
        self.value_type(&expr.value_type, owner);
        self.unit(expr.unit.as_ref(), owner);
        match &expr.kind {
            MetricExprKind::MeasureRef(id) => match self.measure(id) {
                Some(m) => self.same(
                    &expr.value_type,
                    expr.unit.as_ref(),
                    &m.value_type,
                    m.unit.as_ref(),
                    owner,
                ),
                None => self.missing(owner, id),
            },
            MetricExprKind::ExplicitCast { operand, rounding } => {
                self.metric_expression(operand, owner, depth + 1);
                if operand.unit != expr.unit {
                    self.error(
                        ErrorCode::IncompatibleUnits,
                        owner,
                        "A cast cannot change semantic units",
                    );
                }
                if !numeric(&operand.value_type) || !numeric(&expr.value_type) {
                    self.error(
                        ErrorCode::UnsupportedSemanticConstruct,
                        owner,
                        "Metric casts require an explicit numeric contract",
                    );
                }
                let reduces_scale = match (&operand.value_type, &expr.value_type) {
                    (ValueType::Decimal(from), ValueType::Decimal(to)) => to.scale() < from.scale(),
                    (ValueType::Decimal(from), ValueType::Integer { .. }) => from.scale() > 0,
                    _ => false,
                };
                if reduces_scale && rounding.is_none() {
                    self.error(
                        ErrorCode::NumericOverflow,
                        owner,
                        "Scale reduction requires an explicit rounding mode",
                    );
                }
            }
            MetricExprKind::MetricRef(id) | MetricExprKind::Window { base_metric: id } => {
                match self.metric(id) {
                    Some(m) => {
                        self.same(
                            &expr.value_type,
                            expr.unit.as_ref(),
                            &m.value_type,
                            m.unit.as_ref(),
                            owner,
                        );
                    }
                    None => self.missing(owner, id),
                }
            }
            MetricExprKind::Arithmetic {
                op,
                left,
                right,
                contract,
            } => {
                self.metric_expression(left, owner, depth + 1);
                self.metric_expression(right, owner, depth + 1);
                if let Err(mut error) = validate_arithmetic(
                    *op,
                    &left.value_type,
                    &right.value_type,
                    &expr.value_type,
                    contract,
                ) {
                    error.primary_ref = Some(owner.clone());
                    self.errors.push(error);
                }
                self.arithmetic_units(
                    *op,
                    left.unit.as_ref(),
                    right.unit.as_ref(),
                    expr.unit.as_ref(),
                    owner,
                );
            }
            MetricExprKind::Ratio {
                numerator,
                denominator,
                zero_denominator,
                rounding,
            } => {
                self.metric_expression(numerator, owner, depth + 1);
                self.metric_expression(denominator, owner, depth + 1);
                let contract = NumericContract {
                    rounding: *rounding,
                    zero_division: Some(*zero_denominator),
                    explicit_narrowing: false,
                };
                if let Err(mut error) = validate_arithmetic(
                    ArithmeticOp::Divide,
                    &numerator.value_type,
                    &denominator.value_type,
                    &expr.value_type,
                    &contract,
                ) {
                    error.primary_ref = Some(owner.clone());
                    self.errors.push(error);
                }
                self.arithmetic_units(
                    ArithmeticOp::Divide,
                    numerator.unit.as_ref(),
                    denominator.unit.as_ref(),
                    expr.unit.as_ref(),
                    owner,
                );
            }
        }
    }

    fn arithmetic_units(
        &mut self,
        op: ArithmeticOp,
        left: Option<&Unit>,
        right: Option<&Unit>,
        result: Option<&Unit>,
        owner: &StableId,
    ) {
        let valid = match op {
            ArithmeticOp::Add | ArithmeticOp::Subtract => left == right && result == left,
            ArithmeticOp::Multiply => {
                if matches!(left, None | Some(Unit::Count | Unit::Ratio)) {
                    result == right
                } else if matches!(right, None | Some(Unit::Count | Unit::Ratio)) {
                    result == left
                } else {
                    false
                }
            }
            ArithmeticOp::Divide => {
                if left == right {
                    matches!(result, None | Some(Unit::Ratio))
                } else if matches!(right, None | Some(Unit::Count | Unit::Ratio)) {
                    result == left
                } else {
                    false
                }
            }
        };
        if !valid {
            self.error(
                ErrorCode::IncompatibleUnits,
                owner,
                "Arithmetic units do not satisfy an explicit canonical operation",
            );
        }
    }
}

fn numeric(ty: &ValueType) -> bool {
    matches!(
        ty,
        ValueType::Integer { .. } | ValueType::Decimal(_) | ValueType::Float { .. }
    )
}
fn is_temporal(ty: &ValueType) -> bool {
    matches!(ty, ValueType::Date | ValueType::Timestamp { .. })
}
fn unique_cover(dataset: &SemanticDataset, fields: &BTreeSet<&StableId>) -> bool {
    dataset.key_bindings.iter().any(|key| {
        key.unique
            && !key.evidence.is_invalid()
            && key.fields.iter().all(|field| fields.contains(field))
    })
}

fn cycle_members(graph: &BTreeMap<StableId, Vec<StableId>>) -> Vec<StableId> {
    graph
        .keys()
        .filter(|origin| {
            let mut pending: Vec<_> = graph.get(*origin).into_iter().flatten().collect();
            let mut visited = BTreeSet::new();
            while let Some(id) = pending.pop() {
                if id == *origin {
                    return true;
                }
                if visited.insert(id) {
                    pending.extend(graph.get(id).into_iter().flatten());
                }
            }
            false
        })
        .cloned()
        .collect()
}
