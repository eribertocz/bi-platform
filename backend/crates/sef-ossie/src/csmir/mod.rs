//! Ossie → CSMIR: a validated document plus the catalog of its sources
//! becomes the engine's canonical model (Issue #48, part 3).
//!
//! The mapping, and what it does not invent:
//! - **Datasets** keep their source as logical handles from the
//!   [`Catalog`]; a field whose expression is a bare column is a CSMIR field
//!   with the column's exact type and nullability. A computed field is not
//!   stored: its expression is inlined wherever it is used.
//! - **Entities and keys** come from `primary_key` and `unique_keys`; a
//!   relationship whose `to_columns` are a unique key of `to` gives `from` a
//!   foreign key of that entity, as `MetricFlow`'s Ossie converter classifies
//!   key columns. Cardinality stays *declared, unverified*.
//! - **Dimensions** are the fields with Ossie's `dimension` object. Fields
//!   of several datasets naming the same dimension in our extension form one
//!   conformed dimension; otherwise each is `<dataset>.<field>`.
//! - **Metrics**: see [`metric`]. A single aggregate is a measure plus the
//!   metric that exposes it; anything CSMIR cannot state exactly (STDDEV,
//!   MEDIAN, percentiles, string functions…) is rejected, never guessed.
//!
//! Everything Ossie cannot say (units, time zones, grains, conformance,
//! rounding) comes from our extension ([`crate::extension`]), never from
//! defaults that would change a number.

mod contracts;
mod metric;
pub(crate) mod scalar;

use std::collections::BTreeMap;

use sef_model_ir::expressions::{ScalarExpr, ScalarExprKind};
use sef_model_ir::grain::{Grain, GrainAtom};
use sef_model_ir::identity::{SemanticVersion, StableId, valid_segment};
use sef_model_ir::model::{
    CompiledSemanticModel, ConformanceContract, DatasetTimeRole, Dimension, DimensionBinding,
    Entity, EntityKey, Field, FieldBinding, KeyBinding, ModelDefinition, Namespace, ObjectMetadata,
    SemanticDataset, SourceBinding,
};
use sef_model_ir::policy::{Policy, PolicyScope};
use sef_model_ir::relationships::{
    Bridge, Cardinality, ContractEvidenceState, KeyPair, Optionality, Relationship,
};
use sef_model_ir::time::TimestampSemantics;
use sef_model_ir::time::{IanaZoneId, TimeRole, TimeSemantics};
use sef_model_ir::types::{StructField, ValueType};
use sef_model_ir::units::Unit;
use sqlparser::ast::Expr;

use self::scalar::{Division, Failure, Names, Translator, describe, normalize};
use crate::catalog::{Catalog, Column, Relation};
use crate::diagnostic::{Code, Diagnostic};
use crate::document::{self, DataType, SemanticModel};
use crate::extension::{
    self, DatasetExtension, FieldExtension, GrainDeclaration, ModelExtension,
    OptionalityDeclaration, RelationshipExtension, TimeRoleDeclaration, UnitDeclaration,
};
use crate::ids::{self, Ids};
use crate::validate::normalize_identifier;

/// Where the model lives among the platform's stable IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The project namespace (`corpus` → `corpus/dataset/sales`).
    pub namespace: String,
    /// The model's own ID segment, usually its file name (ADR 0003: one
    /// `metrics/<model>.yaml` per model). Ossie's `name` becomes its label.
    pub model: String,
}

/// Converts a loaded document (see [`crate::load`]) into a CSMIR definition.
///
/// # Errors
/// Every problem found, with the JSON pointer of the offending value.
pub fn to_csmir(
    model: &SemanticModel,
    catalog: &Catalog,
    options: &Options,
) -> Result<ModelDefinition, Vec<Diagnostic>> {
    convert(model, catalog, options).map(|(definition, _)| definition)
}

/// Converts and compiles: the definition is also checked against CSMIR's
/// own contracts (types, units, grains, references) and fingerprinted.
///
/// # Errors
/// Conversion problems, or CSMIR's diagnostics (`semantic_contract`)
/// located at the document object they concern.
pub fn compile(
    model: &SemanticModel,
    catalog: &Catalog,
    options: &Options,
) -> Result<CompiledSemanticModel, Vec<Diagnostic>> {
    let (definition, paths) = convert(model, catalog, options)?;
    definition.compile().map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| {
                let path = diagnostic
                    .primary_ref
                    .as_ref()
                    .and_then(|id| paths.get(id))
                    .cloned()
                    .unwrap_or_default();
                Diagnostic::error(
                    Code::SemanticContract,
                    path,
                    format!("{} [{}]", diagnostic.message, diagnostic.code.as_str()),
                )
            })
            .collect()
    })
}

type Paths = BTreeMap<StableId, String>;

fn convert(
    model: &SemanticModel,
    catalog: &Catalog,
    options: &Options,
) -> Result<(ModelDefinition, Paths), Vec<Diagnostic>> {
    for (what, segment) in [("namespace", &options.namespace), ("model", &options.model)] {
        if !valid_segment(segment) {
            return Err(vec![Diagnostic::error(
                Code::InvalidIdentifier,
                "",
                format!(
                    "{what} {segment:?} no sirve como identificador estable: minúsculas ASCII, \
                     dígitos, `.`, `_` y `-`."
                ),
            )]);
        }
    }
    let extension: ModelExtension =
        extension::read(&model.custom_extensions, "").map_err(|diagnostic| vec![diagnostic])?;
    let version = SemanticVersion::new(extension.semantic_version.as_deref().unwrap_or("0.0.0"))
        .map_err(|_| {
            vec![Diagnostic::error(
                Code::InvalidExtension,
                "/custom_extensions",
                "semantic_version debe ser SemVer 2.0.0 (por ejemplo 1.2.0).",
            )]
        })?;
    let ids = Ids::new(options.namespace.clone(), version);
    let mut converter = Converter {
        model,
        catalog,
        ids,
        found: Vec::new(),
        paths: Paths::new(),
        datasets: Vec::new(),
        entities: BTreeMap::new(),
        dimensions: BTreeMap::new(),
        relationships: Vec::new(),
        bridges: Vec::new(),
        policies: Vec::new(),
        measures: Vec::new(),
        metrics: Vec::new(),
        extension,
    };
    converter.run();
    if converter.found.iter().any(Diagnostic::is_error) {
        return Err(converter.found);
    }
    converter
        .finish(options)
        .map_err(|failure| vec![Diagnostic::error(failure.code, "", failure.message)])
}

struct Converter<'a> {
    model: &'a SemanticModel,
    catalog: &'a Catalog,
    ids: Ids,
    found: Vec<Diagnostic>,
    paths: Paths,
    datasets: Vec<DatasetState<'a>>,
    /// Entity segment → its keys (the first is the primary one).
    entities: BTreeMap<String, Vec<EntityKey>>,
    dimensions: BTreeMap<String, DimensionState>,
    relationships: Vec<Relationship>,
    measures: Vec<sef_model_ir::model::Measure>,
    metrics: Vec<sef_model_ir::model::Metric>,
    bridges: Vec<Bridge>,
    policies: Vec<Policy>,
    extension: ModelExtension,
}

struct DatasetState<'a> {
    normalized: String,
    segment: String,
    id: StableId,
    path: String,
    relation: Option<&'a Relation>,
    extension: DatasetExtension,
    fields: Vec<FieldState<'a>>,
    /// CSMIR fields: declared column fields, then implicit ones that
    /// computed fields read.
    columns: Vec<(Field, String)>,
    key_bindings: Vec<KeyBinding>,
    time_roles: Vec<DatasetTimeRole>,
    grain: Grain,
}

struct FieldState<'a> {
    ossie: &'a document::Field,
    normalized: String,
    segment: String,
    path: String,
    expression_path: String,
    extension: FieldExtension,
    ast: Option<Expr>,
    /// The CSMIR field, when the expression is a bare column.
    column: Option<StableId>,
}

struct DimensionState {
    metadata: ObjectMetadata,
    path: String,
    value_type: ValueType,
    unit: Option<Unit>,
    semantic_role: Option<String>,
    time_semantics: Option<TimeSemantics>,
    bindings: Vec<DimensionBinding>,
}

impl<'a> Converter<'a> {
    fn error(&mut self, code: Code, path: impl Into<String>, message: impl Into<String>) {
        self.found.push(Diagnostic::error(code, path, message));
    }

    fn fail(&mut self, path: &str, failure: Failure) {
        let path = failure.path.unwrap_or_else(|| path.to_owned());
        self.found
            .push(Diagnostic::error(failure.code, path, failure.message));
    }

    fn id(&mut self, kind: &str, name: &str, path: &str) -> Option<StableId> {
        match self.ids.id(kind, name) {
            Ok(id) => {
                self.paths
                    .entry(id.clone())
                    .or_insert_with(|| path.to_owned());
                Some(id)
            }
            Err(id) => {
                self.error(
                    Code::InvalidIdentifier,
                    path,
                    format!("{id:?} no es un identificador estable válido."),
                );
                None
            }
        }
    }

    fn metadata(&mut self, kind: &str, name: &str, path: &str) -> Option<ObjectMetadata> {
        self.id(kind, name, path)?;
        self.ids.identity(kind, name).ok().map(ObjectMetadata::new)
    }

    fn run(&mut self) {
        self.datasets();
        if self.found.iter().any(Diagnostic::is_error) {
            return;
        }
        self.keys();
        self.relationships();
        self.computed_fields();
        self.dimensions();
        self.grains();
        self.bridges();
        self.policies();
        if self.found.iter().any(Diagnostic::is_error) {
            return;
        }
        metric::metrics(self);
    }

    /// Datasets, their sources and their column fields.
    fn datasets(&mut self) {
        for (i, dataset) in self.model.datasets.iter().enumerate() {
            let path = format!("/datasets/{i}");
            let segment = ids::segment(&dataset.name);
            let Some(id) = self.id("dataset", &segment, &path) else {
                continue;
            };
            let relation = self.catalog.relations.get(&dataset.source);
            if relation.is_none() {
                self.error(
                    Code::UnknownSource,
                    format!("{path}/source"),
                    format!(
                        "El catálogo no conoce el origen {:?}: no se pueden saber sus columnas.",
                        dataset.source
                    ),
                );
            }
            let extension = match extension::read(&dataset.custom_extensions, &path) {
                Ok(extension) => extension,
                Err(diagnostic) => {
                    self.found.push(diagnostic);
                    DatasetExtension::default()
                }
            };
            let mut state = DatasetState {
                normalized: normalize_identifier(&dataset.name),
                segment,
                id,
                path: path.clone(),
                relation,
                extension,
                fields: Vec::new(),
                columns: Vec::new(),
                key_bindings: Vec::new(),
                time_roles: Vec::new(),
                grain: Grain::global(),
            };
            for (j, field) in dataset.fields.iter().enumerate() {
                let field_path = format!("{path}/fields/{j}");
                self.field(&mut state, field, field_path);
            }
            self.datasets.push(state);
        }
    }

    fn field(&mut self, state: &mut DatasetState<'a>, field: &'a document::Field, path: String) {
        let extension = match extension::read(&field.custom_extensions, &path) {
            Ok(extension) => extension,
            Err(diagnostic) => {
                self.found.push(diagnostic);
                FieldExtension::default()
            }
        };
        let segment = ids::segment(&field.name);
        let (ast, expression_path) =
            self.portable(&field.expression, &format!("{path}/expression"));
        let mut column = None;
        if let (Some(Expr::Identifier(_) | Expr::CompoundIdentifier(_)), Some(relation)) =
            (ast.as_ref().map(strip), state.relation)
        {
            let Some(found) =
                self.column(state, relation, ast.as_ref().map(strip), &expression_path)
            else {
                return;
            };
            let unit = extension.unit.as_ref().map(unit);
            let Some(metadata) =
                self.metadata("field", &format!("{}.{segment}", state.segment), &path)
            else {
                return;
            };
            let mut metadata = metadata;
            metadata.label.clone_from(&field.label);
            metadata.description.clone_from(&field.description);
            if let Some(datatype) = field.datatype
                && !compatible(datatype, &found.value_type)
            {
                self.error(
                    Code::DatatypeMismatch,
                    format!("{path}/datatype"),
                    format!(
                        "El campo dice {datatype:?}, pero la columna {:?} es {}.",
                        found.name,
                        describe(&found.value_type)
                    ),
                );
            }
            column = Some(metadata.id().clone());
            if let Some(role) = extension.time_role {
                state.time_roles.push(DatasetTimeRole {
                    field: metadata.id().clone(),
                    role: time_role(role),
                });
            }
            state.columns.push((
                Field {
                    dataset: state.id.clone(),
                    value_type: found.value_type.clone(),
                    unit,
                    nullable: found.nullable,
                    binding: found.handle.clone(),
                    metadata,
                },
                found.name.clone(),
            ));
        } else if extension.time_role.is_some() || extension.unit.is_some() {
            self.error(
                Code::InvalidExtension,
                format!("{path}/custom_extensions"),
                "time_role y unit describen una columna; este campo es calculado.",
            );
        }
        state.fields.push(FieldState {
            ossie: field,
            normalized: normalize_identifier(&field.name),
            segment,
            path,
            expression_path,
            extension,
            ast,
            column,
        });
    }

    /// The portable expression, parsed (validation already accepted it).
    fn portable(
        &mut self,
        expression: &document::Expression,
        path: &str,
    ) -> (Option<Expr>, String) {
        let Some(k) = expression
            .dialects
            .iter()
            .position(|candidate| candidate.dialect == document::Dialect::OssieSql2026)
        else {
            self.error(
                Code::NotRepresentable,
                format!("{path}/dialects"),
                "Falta la expresión OSSIE_SQL_2026: los otros dialectos no definen su \
                 significado y el motor no los interpreta.",
            );
            return (None, path.to_owned());
        };
        let path = format!("{path}/dialects/{k}/expression");
        match crate::expression::parse(&expression.dialects[k].expression) {
            Ok(ast) => (Some(ast), path),
            Err(mut diagnostic) => {
                diagnostic.path.clone_from(&path);
                self.found.push(diagnostic);
                (None, path)
            }
        }
    }

    /// The single catalog column a bare reference names.
    fn column<'r>(
        &mut self,
        state: &DatasetState<'_>,
        relation: &'r Relation,
        reference: Option<&Expr>,
        path: &str,
    ) -> Option<&'r Column> {
        let (qualifier, name) = match reference? {
            Expr::Identifier(ident) => (None, normalize(ident)),
            Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [qualifier, name] => (Some(normalize(qualifier)), normalize(name)),
                _ => return None,
            },
            _ => return None,
        };
        match resolve_column(state, relation, qualifier.as_deref(), &name) {
            Ok(column) => Some(column),
            Err(failure) => {
                self.fail(path, failure);
                None
            }
        }
    }

    /// Primary, unique and declared keys; entities.
    fn keys(&mut self) {
        for d in 0..self.datasets.len() {
            let (dataset, path) = (&self.model.datasets[d], self.datasets[d].path.clone());
            let entity = self.datasets[d]
                .extension
                .entity
                .as_deref()
                .map_or_else(|| self.datasets[d].segment.clone(), ids::segment);
            if let Some(primary) = &dataset.primary_key {
                let key_path = format!("{path}/primary_key");
                if let Some(key) = self.declare_key(d, &entity, &entity, primary, true, &key_path) {
                    self.datasets[d].grain = Grain::new([GrainAtom::EntityKeyRef(key)]);
                }
            }
            for (u, unique) in dataset.unique_keys.iter().flatten().enumerate() {
                let key_path = format!("{path}/unique_keys/{u}");
                let name = format!(
                    "{entity}.{}",
                    unique
                        .iter()
                        .map(|c| ids::segment(c))
                        .collect::<Vec<_>>()
                        .join(".")
                );
                self.declare_key(d, &entity, &name, unique, true, &key_path);
            }
            let declared = self.datasets[d].extension.keys.clone();
            for (k, key) in declared.iter().enumerate() {
                let key_path = format!("{path}/custom_extensions/keys/{k}");
                let entity = ids::segment(&key.entity);
                self.declare_key(d, &entity, &entity, &key.fields, key.unique, &key_path);
            }
        }
    }

    /// Binds `fields` of dataset `d` to key `key` of `entity`, registering
    /// the entity (typed after these fields) the first time it appears.
    fn declare_key(
        &mut self,
        d: usize,
        entity: &str,
        key: &str,
        fields: &[String],
        unique: bool,
        path: &str,
    ) -> Option<StableId> {
        let mut ids = Vec::with_capacity(fields.len());
        let mut components = Vec::with_capacity(fields.len());
        for (c, name) in fields.iter().enumerate() {
            let Some((id, column)) = self.column_field(d, &normalize_identifier(name)) else {
                self.error(
                    Code::UnknownColumn,
                    format!("{path}/{c}"),
                    format!(
                        "La clave usa {name:?}, que no es un campo de columna del dataset {:?}.",
                        self.model.datasets[d].name
                    ),
                );
                return None;
            };
            components.push(StructField {
                name: id
                    .as_str()
                    .rsplit('.')
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                value_type: column.value_type.clone(),
                nullable: column.nullable,
            });
            ids.push(id);
        }
        let value_type = match <[StructField; 1]>::try_from(components) {
            Ok([single]) => single.value_type,
            Err(components) => ValueType::Struct(components),
        };
        let entity_path = path.to_owned();
        self.id("entity", entity, &entity_path)?;
        let metadata = self.metadata("key", key, path)?;
        let key_id = metadata.id().clone();
        let keys = self.entities.entry(entity.to_owned()).or_default();
        match keys
            .iter()
            .find(|existing| existing.metadata.id() == &key_id)
        {
            Some(existing) if existing.value_type != value_type => {
                let message = format!(
                    "La clave {key:?} ya tiene tipo {} en otro dataset y aquí es {}.",
                    describe(&existing.value_type),
                    describe(&value_type)
                );
                self.error(Code::TypeMismatch, path, message);
                return None;
            }
            Some(_) => {}
            None => keys.push(EntityKey {
                metadata,
                value_type,
                uniqueness_scope: Grain::global(),
            }),
        }
        let binding = KeyBinding {
            entity_key: key_id.clone(),
            fields: ids,
            unique,
            evidence: ContractEvidenceState::DeclaredUnverified,
        };
        let bindings = &mut self.datasets[d].key_bindings;
        if !bindings.contains(&binding) {
            bindings.push(binding);
        }
        Some(key_id)
    }

    /// The CSMIR column field an Ossie field name denotes.
    fn column_field(&self, d: usize, normalized: &str) -> Option<(StableId, &Field)> {
        let state = &self.datasets[d];
        let field = state
            .fields
            .iter()
            .find(|field| field.normalized == normalized)?;
        let id = field.column.as_ref()?;
        let (column, _) = state
            .columns
            .iter()
            .find(|(column, _)| column.metadata.id() == id)?;
        Some((id.clone(), column))
    }

    fn dataset_index(&self, name: &str) -> Option<usize> {
        let normalized = normalize_identifier(name);
        self.datasets
            .iter()
            .position(|dataset| dataset.normalized == normalized)
    }

    fn relationships(&mut self) {
        for (r, relationship) in self.model.relationships.iter().enumerate() {
            let path = format!("/relationships/{r}");
            let extension: RelationshipExtension =
                match extension::read(&relationship.custom_extensions, &path) {
                    Ok(extension) => extension,
                    Err(diagnostic) => {
                        self.found.push(diagnostic);
                        continue;
                    }
                };
            // Validation already resolved both ends and the column arity.
            let (Some(from), Some(to)) = (
                self.dataset_index(&relationship.from),
                self.dataset_index(&relationship.to),
            ) else {
                continue;
            };
            let mut pairs = Vec::new();
            let mut from_fields = Vec::new();
            let mut to_fields = Vec::new();
            for (c, (from_column, to_column)) in relationship
                .from_columns
                .iter()
                .zip(&relationship.to_columns)
                .enumerate()
            {
                let ends = [
                    (from, from_column, "from_columns"),
                    (to, to_column, "to_columns"),
                ]
                .map(|(d, name, side)| {
                    self.column_field(d, &normalize_identifier(name))
                        .map(|(id, _)| id)
                        .ok_or((side, name, d))
                });
                match ends {
                    [Ok(from_field), Ok(to_field)] => {
                        from_fields.push(from_field.clone());
                        to_fields.push(to_field.clone());
                        pairs.push(KeyPair {
                            from_field,
                            to_field,
                        });
                    }
                    [Err((side, name, d)), _] | [_, Err((side, name, d))] => {
                        let message = format!(
                            "{name:?} no es un campo de columna del dataset {:?}.",
                            self.model.datasets[d].name
                        );
                        self.error(Code::UnknownColumn, format!("{path}/{side}/{c}"), message);
                    }
                }
            }
            if pairs.len() != relationship.from_columns.len() {
                continue;
            }
            // A foreign key of the entity whose unique key `to_columns` are.
            let derived = self.datasets[to]
                .key_bindings
                .iter()
                .find(|binding| binding.unique && binding.fields == to_fields)
                .map(|binding| KeyBinding {
                    entity_key: binding.entity_key.clone(),
                    fields: from_fields.clone(),
                    unique: false,
                    evidence: ContractEvidenceState::DeclaredUnverified,
                });
            if let Some(binding) = derived {
                let bindings = &mut self.datasets[from].key_bindings;
                if !bindings.iter().any(|existing| {
                    existing.entity_key == binding.entity_key && existing.fields == binding.fields
                }) {
                    bindings.push(binding);
                }
            }
            let temporal_constraint = match extension.temporal.as_ref().map(|t| self.temporal(t)) {
                Some(Ok(constraint)) => Some(constraint),
                Some(Err(failure)) => {
                    self.fail(&format!("{path}/custom_extensions"), failure);
                    continue;
                }
                None => None,
            };
            let segment = ids::segment(&relationship.name);
            let Some(metadata) = self.metadata("relationship", &segment, &path) else {
                continue;
            };
            self.relationships.push(Relationship {
                metadata,
                from_dataset: self.datasets[from].id.clone(),
                to_dataset: self.datasets[to].id.clone(),
                role: segment,
                cardinality: Cardinality::ManyToOne,
                key_pairs: pairs,
                optionality: match extension.optionality {
                    Some(OptionalityDeclaration::Required) => Optionality::Required,
                    // Unknown is optional: a left join never drops rows.
                    Some(OptionalityDeclaration::Optional) | None => Optionality::Optional,
                },
                temporal_constraint,
                evidence: ContractEvidenceState::DeclaredUnverified,
            });
        }
    }

    /// Computed fields only need checking here; they are inlined where used.
    fn computed_fields(&mut self) {
        for d in 0..self.datasets.len() {
            for f in 0..self.datasets[d].fields.len() {
                let field = &self.datasets[d].fields[f];
                if field.column.is_some() || field.ossie.dimension.is_some() {
                    continue;
                }
                let (Some(ast), path) = (field.ast.clone(), field.expression_path.clone()) else {
                    continue;
                };
                let timezone = self.timezone(d, f);
                match self.row_expression(d, &ast, timezone) {
                    Ok(expression) => self.check_datatype(d, f, &expression.value_type),
                    Err(failure) => self.fail(&path, failure),
                }
            }
        }
    }

    /// A declared `datatype` must describe the computed value.
    fn check_datatype(&mut self, d: usize, f: usize, value_type: &ValueType) {
        let field = &self.datasets[d].fields[f];
        if let Some(datatype) = field.ossie.datatype
            && !compatible(datatype, value_type)
        {
            let path = format!("{}/datatype", field.path);
            let message = format!(
                "El campo dice {datatype:?}, pero su expresión da {}.",
                describe(value_type)
            );
            self.error(Code::DatatypeMismatch, path, message);
        }
    }

    fn timezone(&mut self, d: usize, f: usize) -> Option<IanaZoneId> {
        let field = &self.datasets[d].fields[f];
        let declared = field.extension.time.as_ref()?;
        let path = format!("{}/custom_extensions", field.path);
        let zone = IanaZoneId::new(declared.timezone.clone()).ok();
        if zone.is_none() {
            let message = format!("{:?} no es una zona horaria IANA.", declared.timezone);
            self.error(Code::InvalidExtension, path, message);
        }
        zone
    }

    /// Translates a row-level expression of dataset `d`.
    fn row_expression(
        &mut self,
        d: usize,
        ast: &Expr,
        timezone: Option<IanaZoneId>,
    ) -> Result<ScalarExpr, Failure> {
        let mut scope = RowScope {
            converter: self,
            dataset: d,
            timezone,
        };
        Translator {
            names: &mut scope,
            division: Division::default(),
        }
        .translate(ast)
    }

    fn dimensions(&mut self) {
        for d in 0..self.datasets.len() {
            for f in 0..self.datasets[d].fields.len() {
                if self.datasets[d].fields[f].ossie.dimension.is_none() {
                    continue;
                }
                self.dimension(d, f);
            }
        }
    }

    fn dimension(&mut self, d: usize, f: usize) {
        let field = &self.datasets[d].fields[f];
        let (path, expression_path) = (field.path.clone(), field.expression_path.clone());
        let name = field.extension.dimension.as_deref().map_or_else(
            || format!("{}.{}", self.datasets[d].segment, field.segment),
            ids::segment,
        );
        let semantic_role = field.extension.semantic_role.clone();
        let tzdb = field
            .extension
            .time
            .as_ref()
            .and_then(|time| time.tzdb.clone());
        let expression = match (&field.column, field.ast.clone()) {
            (Some(id), _) => {
                let column = self.datasets[d]
                    .columns
                    .iter()
                    .find(|(column, _)| column.metadata.id() == id)
                    .map(|(column, _)| column);
                let Some(column) = column else { return };
                Ok(field_ref(column))
            }
            (None, Some(ast)) => {
                let timezone = self.timezone(d, f);
                self.row_expression(d, &ast, timezone)
            }
            (None, None) => return,
        };
        let expression = match expression {
            Ok(expression) => expression,
            Err(failure) => {
                self.fail(&expression_path, failure);
                return;
            }
        };
        self.check_datatype(d, f, &expression.value_type);
        let time_semantics = match self.time_semantics(d, f, &expression, tzdb) {
            Ok(semantics) => semantics,
            Err(failure) => {
                self.fail(&format!("{path}/custom_extensions"), failure);
                return;
            }
        };
        let Some(metadata) = self.metadata("dimension", &name, &path) else {
            return;
        };
        let binding = DimensionBinding {
            dataset: self.datasets[d].id.clone(),
            expression: expression.clone(),
            conformance: ConformanceContract::ExactDomain,
        };
        let Some(state) = self.dimensions.get_mut(&name) else {
            self.dimensions.insert(
                name,
                DimensionState {
                    metadata,
                    path,
                    value_type: expression.value_type,
                    unit: expression.unit,
                    semantic_role,
                    time_semantics,
                    bindings: vec![binding],
                },
            );
            return;
        };
        if let Err(conflict) = state.absorb(binding, semantic_role, time_semantics) {
            let first = state.path.clone();
            self.error(
                Code::TypeMismatch,
                path,
                format!(
                    "Este campo y {first} declaran la misma dimensión {name:?} de forma \
                     incompatible ({conflict})."
                ),
            );
        }
    }

    /// The time semantics of a dimension bucketed with `DATE_TRUNC`: the
    /// zone of the bucket and the role of its source column.
    fn time_semantics(
        &self,
        d: usize,
        f: usize,
        expression: &ScalarExpr,
        tzdb: Option<String>,
    ) -> Result<Option<TimeSemantics>, Failure> {
        let ScalarExprKind::TemporalDerivation { bucket, .. } = &expression.kind else {
            if self.datasets[d].fields[f].extension.time.is_some() {
                return Err(Failure::new(
                    Code::InvalidExtension,
                    "\"time\" describe un DATE_TRUNC y este campo no lo usa.",
                ));
            }
            return Ok(None);
        };
        let role = self.datasets[d]
            .time_roles
            .iter()
            .find(|role| role.field == bucket.source)
            .map(|role| role.role)
            .ok_or_else(|| {
                Failure::new(
                    Code::InvalidExtension,
                    "El campo de origen del DATE_TRUNC necesita su time_role (event, snapshot…) \
                     en la extensión.",
                )
            })?;
        Ok(Some(TimeSemantics {
            role,
            timezone: bucket.timezone.clone(),
            calendar: bucket.calendar.clone(),
            pinned_tzdb_version: tzdb,
        }))
    }

    /// Declared observation grains (after the dimensions exist).
    fn grains(&mut self) {
        for d in 0..self.datasets.len() {
            let Some(declared) = self.datasets[d].extension.grain.clone() else {
                continue;
            };
            let path = format!("{}/custom_extensions", self.datasets[d].path);
            if let Some(grain) = self.grain(&declared, &path) {
                self.datasets[d].grain = grain;
            }
        }
    }

    fn grain(&mut self, declared: &GrainDeclaration, path: &str) -> Option<Grain> {
        let atoms = declared
            .dimensions
            .iter()
            .map(|name| self.atom(Some(name), None))
            .chain(
                declared
                    .entities
                    .iter()
                    .map(|name| self.atom(None, Some(name))),
            )
            .collect::<Result<Vec<_>, _>>();
        match atoms {
            Ok(atoms) => Some(Grain::new(atoms)),
            Err(failure) => {
                self.fail(path, failure);
                None
            }
        }
    }

    /// One grain atom: exactly one of a dimension or an entity name.
    pub(crate) fn atom(
        &self,
        dimension: Option<&str>,
        entity: Option<&str>,
    ) -> Result<GrainAtom, Failure> {
        match (dimension, entity) {
            (Some(name), None) => self
                .dimensions
                .get(&ids::segment(name))
                .map(|state| GrainAtom::DimensionRef {
                    dimension: state.metadata.id().clone(),
                    roles: Vec::new(),
                })
                .ok_or_else(|| {
                    Failure::new(
                        Code::UnknownReference,
                        format!("No existe la dimensión {name:?}."),
                    )
                }),
            (None, Some(name)) => self
                .entities
                .get(&ids::segment(name))
                .and_then(|keys| keys.first())
                .map(|key| GrainAtom::EntityKeyRef(key.metadata.id().clone()))
                .ok_or_else(|| {
                    Failure::new(
                        Code::UnknownReference,
                        format!("Ninguna clave declara la entidad {name:?}."),
                    )
                }),
            _ => Err(Failure::new(
                Code::InvalidExtension,
                "Cada eje nombra exactamente una dimensión o una entidad.",
            )),
        }
    }

    fn finish(self, options: &Options) -> Result<(ModelDefinition, Paths), Failure> {
        let internal =
            |what: &str| Failure::new(Code::SemanticContract, format!("{what} inválido"));
        let model_identity = self
            .ids
            .identity("model", &options.model)
            .map_err(|_| internal("modelo"))?;
        let mut definition = ModelDefinition::new(ObjectMetadata {
            identity: model_identity,
            label: Some(self.model.name.clone()),
            description: self.model.description.clone(),
        });
        definition.namespaces.push(Namespace {
            metadata: ObjectMetadata::new(
                self.ids
                    .identity("namespace", &options.namespace)
                    .map_err(|_| internal("espacio de nombres"))?,
            ),
        });
        for state in self.datasets {
            let relation = state.relation.ok_or_else(|| internal("origen"))?;
            let source = ObjectMetadata::new(
                self.ids
                    .identity("source", &state.segment)
                    .map_err(|_| internal("origen"))?,
            );
            let fields: Vec<Field> = state.columns.into_iter().map(|(field, _)| field).collect();
            let source_binding = source.id().clone();
            definition.source_bindings.push(SourceBinding {
                metadata: source,
                source_id: relation.source.clone(),
                relation_ref: relation.relation.clone(),
                field_bindings: fields
                    .iter()
                    .map(|field| FieldBinding {
                        field: field.metadata.id().clone(),
                        handle: field.binding.clone(),
                    })
                    .collect(),
            });
            let mut metadata = ObjectMetadata::new(
                self.ids
                    .identity("dataset", &state.segment)
                    .map_err(|_| internal("dataset"))?,
            );
            metadata.description.clone_from(
                &self
                    .model
                    .datasets
                    .iter()
                    .find(|dataset| normalize_identifier(&dataset.name) == state.normalized)
                    .and_then(|dataset| dataset.description.clone()),
            );
            let policy_refs = self
                .policies
                .iter()
                .filter(|policy| policy.scope == PolicyScope::Dataset(state.id.clone()))
                .map(|policy| policy.metadata.id().clone())
                .collect();
            definition.datasets.push(SemanticDataset {
                metadata,
                source_binding,
                observation_grain: state.grain,
                fields,
                key_bindings: state.key_bindings,
                time_roles: state.time_roles,
                policy_refs,
            });
        }
        for (entity, keys) in self.entities {
            definition.entities.push(Entity {
                metadata: ObjectMetadata::new(
                    self.ids
                        .identity("entity", &entity)
                        .map_err(|_| internal("entidad"))?,
                ),
                keys,
            });
        }
        for state in self.dimensions.into_values() {
            definition.dimensions.push(Dimension {
                metadata: state.metadata,
                value_type: state.value_type,
                semantic_role: state.semantic_role,
                unit: state.unit,
                time_semantics: state.time_semantics,
                participating_datasets: state
                    .bindings
                    .iter()
                    .map(|binding| binding.dataset.clone())
                    .collect(),
                bindings: state.bindings,
            });
        }
        definition.relationships = self.relationships;
        definition.bridges = self.bridges;
        definition.policies = self.policies;
        definition.measures = self.measures;
        definition.metrics = self.metrics;
        Ok((definition, self.paths))
    }
}

impl DimensionState {
    /// Adds another dataset's binding of this conformed dimension.
    fn absorb(
        &mut self,
        binding: DimensionBinding,
        semantic_role: Option<String>,
        time_semantics: Option<TimeSemantics>,
    ) -> Result<(), String> {
        let expression = &binding.expression;
        if self.value_type != expression.value_type {
            return Err(format!(
                "tipo {} frente a {}",
                describe(&self.value_type),
                describe(&expression.value_type)
            ));
        }
        if self.unit != expression.unit {
            return Err("unidades distintas".to_owned());
        }
        if self
            .bindings
            .iter()
            .any(|existing| existing.dataset == binding.dataset)
        {
            return Err("dos campos del mismo dataset".to_owned());
        }
        if !merge(&mut self.semantic_role, semantic_role) {
            return Err("semantic_role distinto".to_owned());
        }
        if !merge(&mut self.time_semantics, time_semantics) {
            return Err("semántica temporal distinta".to_owned());
        }
        self.bindings.push(binding);
        Ok(())
    }
}

/// Keeps `current` or adopts `declared`; false when both differ.
fn merge<T: PartialEq>(current: &mut Option<T>, declared: Option<T>) -> bool {
    match (current.as_ref(), declared) {
        (_, None) => true,
        (None, Some(declared)) => {
            *current = Some(declared);
            true
        }
        (Some(existing), Some(declared)) => *existing == declared,
    }
}

/// Names inside a row-level expression of one dataset: its columns. A
/// column without a declared field gets an implicit one.
struct RowScope<'c, 'a> {
    converter: &'c mut Converter<'a>,
    dataset: usize,
    timezone: Option<IanaZoneId>,
}

impl Names for RowScope<'_, '_> {
    fn reference(&mut self, qualifier: Option<&str>, name: &str) -> Result<ScalarExpr, Failure> {
        let converter = &mut *self.converter;
        let state = &converter.datasets[self.dataset];
        let relation = state
            .relation
            .ok_or_else(|| Failure::new(Code::UnknownSource, "El dataset no tiene origen."))?;
        let column = resolve_column(state, relation, qualifier, name)?;
        if let Some((field, _)) = state
            .columns
            .iter()
            .find(|(_, physical)| *physical == column.name)
        {
            return Ok(field_ref(field));
        }
        // An implicit field named after the column. Computed fields have no
        // CSMIR ID (`region: TRIM(region)` is fine); a column field with this
        // name is bound to another column (the same one was found above).
        let segment = ids::encode(&column.name.to_uppercase());
        if state
            .fields
            .iter()
            .any(|field| field.segment == segment && field.column.is_some())
        {
            return Err(Failure::new(
                Code::AmbiguousReference,
                format!(
                    "La columna {:?} tiene el nombre de un campo que lee otra columna; renombra \
                     ese campo o declara uno para esta columna.",
                    column.name
                ),
            ));
        }
        let path = state.path.clone();
        let (dataset_segment, dataset_id) = (state.segment.clone(), state.id.clone());
        let metadata = converter
            .metadata("field", &format!("{dataset_segment}.{segment}"), &path)
            .ok_or_else(|| Failure::new(Code::InvalidIdentifier, "Columna sin identificador."))?;
        let field = Field {
            metadata,
            dataset: dataset_id,
            value_type: column.value_type.clone(),
            unit: None,
            nullable: column.nullable,
            binding: column.handle.clone(),
        };
        let scalar = field_ref(&field);
        converter.datasets[self.dataset]
            .columns
            .push((field, column.name.clone()));
        Ok(scalar)
    }

    fn timezone(&self) -> Option<&IanaZoneId> {
        self.timezone.as_ref()
    }
}

fn resolve_column<'r>(
    state: &DatasetState<'_>,
    relation: &'r Relation,
    qualifier: Option<&str>,
    name: &str,
) -> Result<&'r Column, Failure> {
    if let Some(qualifier) = qualifier
        && qualifier != state.normalized
    {
        return Err(Failure::new(
            Code::UnknownReference,
            format!(
                "Un campo solo lee columnas de su propio dataset, no de {qualifier:?}; para \
                 combinar datasets usa una relación."
            ),
        ));
    }
    match relation.columns_named(name).as_slice() {
        [column] => Ok(column),
        [] => Err(Failure::new(
            Code::UnknownColumn,
            format!("El origen no tiene la columna {name:?}."),
        )),
        _ => Err(Failure::new(
            Code::AmbiguousReference,
            format!(
                "Varias columnas se llaman {name:?} sin distinguir mayúsculas; escríbela entre \
                 comillas dobles."
            ),
        )),
    }
}

fn strip(expr: &Expr) -> &Expr {
    match expr {
        Expr::Nested(inner) => strip(inner),
        other => other,
    }
}

pub(crate) fn field_ref(field: &Field) -> ScalarExpr {
    ScalarExpr {
        kind: ScalarExprKind::FieldRef(field.metadata.id().clone()),
        value_type: field.value_type.clone(),
        unit: field.unit.clone(),
    }
}

pub(crate) fn unit(declared: &UnitDeclaration) -> Unit {
    match declared {
        UnitDeclaration::Count => Unit::Count,
        UnitDeclaration::Ratio => Unit::Ratio,
        UnitDeclaration::Percent => Unit::Percent,
        UnitDeclaration::Currency(code) => Unit::Currency {
            iso4217: code.clone(),
        },
        UnitDeclaration::Ucum(code) => Unit::Physical { ucum: code.clone() },
        UnitDeclaration::Custom { namespace, code } => Unit::Custom {
            namespace: namespace.clone(),
            code: code.clone(),
        },
    }
}

fn time_role(declared: TimeRoleDeclaration) -> TimeRole {
    match declared {
        TimeRoleDeclaration::Event => TimeRole::EventTime,
        TimeRoleDeclaration::Snapshot => TimeRole::SnapshotTime,
        TimeRoleDeclaration::ValidityStart => TimeRole::ValidityStart,
        TimeRoleDeclaration::ValidityEnd => TimeRole::ValidityEnd,
        TimeRoleDeclaration::Processing => TimeRole::ProcessingTime,
    }
}

/// Whether Ossie's logical `datatype` describes a CSMIR type.
pub(crate) fn compatible(datatype: DataType, value_type: &ValueType) -> bool {
    match datatype {
        DataType::String => *value_type == ValueType::Utf8,
        DataType::Integer => matches!(value_type, ValueType::Integer { .. }),
        DataType::Decimal => matches!(value_type, ValueType::Decimal(_)),
        DataType::Float => matches!(value_type, ValueType::Float { .. }),
        DataType::Boolean => *value_type == ValueType::Boolean,
        DataType::Date => *value_type == ValueType::Date,
        DataType::Time => *value_type == ValueType::Time,
        DataType::DateTime => matches!(
            value_type,
            ValueType::Timestamp {
                timezone_semantics: TimestampSemantics::LocalDateTime { .. }
            }
        ),
        DataType::DateTimeTz => matches!(
            value_type,
            ValueType::Timestamp {
                timezone_semantics: TimestampSemantics::Instant
            }
        ),
        DataType::Opaque => true,
    }
}
