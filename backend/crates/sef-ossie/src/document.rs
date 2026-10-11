//! An Apache Ossie core document, typed after `core-spec/ossie-schema.json`
//! (specification `0.2.0.dev0`, pinned commit in `docs/referentes/ossie.md`).
//!
//! The types mirror the schema one to one: unknown properties are rejected
//! wherever the schema sets `additionalProperties: false`. Constraints serde
//! cannot express (`minLength`, `minItems`, the version constant) and the
//! semantic checks of the official validator live in [`crate::validate`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// An optional property that, when present, must hold a value: the schema
/// never allows `null`, and reading one as "absent" would silently change
/// the document on export.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    // Check null before deserializing T: an untagged struct variant with
    // defaults/flatten can otherwise interpret null as an empty object.
    Option::<T>::deserialize(deserializer)?
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("an explicitly present property cannot be null"))
}

/// The specification version this crate reads and writes.
pub const SPEC_VERSION: &str = "0.2.0.dev0";

/// One semantic model: a standalone Ossie document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticModel {
    pub version: String,
    pub name: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_context: Option<AiContext>,
    pub datasets: Vec<Dataset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<Relationship>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metrics: Vec<Metric>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_extensions: Vec<CustomExtension>,
}

/// A logical dataset: a fact or dimension table, or a query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub name: String,
    /// A physical table or view (`database.schema.table`) or a query.
    pub source: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub primary_key: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub unique_keys: Option<Vec<Vec<String>>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_context: Option<AiContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_extensions: Vec<CustomExtension>,
}

/// A row-level attribute used for grouping, filtering and metric expressions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub expression: Expression,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub dimension: Option<Dimension>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub label: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub datatype: Option<DataType>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_context: Option<AiContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_extensions: Vec<CustomExtension>,
}

impl Field {
    /// Whether the field plays the time role: an explicit `is_time` wins;
    /// otherwise it follows from a temporal `datatype` (spec section on
    /// `DataType` and `is_time`: type vs. role).
    #[must_use]
    pub fn is_time(&self) -> bool {
        self.dimension
            .as_ref()
            .and_then(|dimension| dimension.is_time)
            .unwrap_or_else(|| self.datatype.is_some_and(DataType::is_temporal))
    }
}

/// Dimension metadata of a field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dimension {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_time: Option<bool>,
}

/// A relationship from the many side (`from`) to the one side (`to`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relationship {
    pub name: String,
    pub from: String,
    pub to: String,
    pub from_columns: Vec<String>,
    pub to_columns: Vec<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_context: Option<AiContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_extensions: Vec<CustomExtension>,
}

/// An aggregate measure defined at the model level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    pub name: String,
    pub expression: Expression,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub datatype: Option<DataType>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub ai_context: Option<AiContext>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_extensions: Vec<CustomExtension>,
}

/// An expression in one or more dialects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expression {
    pub dialects: Vec<DialectExpression>,
}

impl Expression {
    /// The expression this engine can compute: Ossie's defined language,
    /// then ANSI SQL restricted to the same allowlist. Document order never
    /// changes that priority. Callers validate dialect uniqueness first.
    pub(crate) fn executable(&self) -> Option<(usize, &DialectExpression)> {
        [Dialect::OssieSql2026, Dialect::AnsiSql]
            .into_iter()
            .find_map(|dialect| {
                self.dialects
                    .iter()
                    .enumerate()
                    .find(|(_, candidate)| candidate.dialect == dialect)
            })
    }

    /// The portable expression, when declared. `OSSIE_SQL_2026` is the
    /// language whose semantics the specification defines.
    #[must_use]
    pub fn portable(&self) -> Option<&str> {
        self.dialects
            .iter()
            .find(|candidate| candidate.dialect == Dialect::OssieSql2026)
            .map(|candidate| candidate.expression.as_str())
    }
}

/// One expression in one dialect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialectExpression {
    pub dialect: Dialect,
    pub expression: String,
}

/// Expression languages registered by the specification. Only
/// `OSSIE_SQL_2026` has semantics defined by Ossie; the others are opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Dialect {
    #[serde(rename = "ANSI_SQL")]
    AnsiSql,
    #[serde(rename = "SNOWFLAKE")]
    Snowflake,
    #[serde(rename = "MDX")]
    Mdx,
    #[serde(rename = "TABLEAU")]
    Tableau,
    #[serde(rename = "DATABRICKS")]
    Databricks,
    #[serde(rename = "MAQL")]
    Maql,
    #[serde(rename = "BIGQUERY")]
    BigQuery,
    #[serde(rename = "SIGMA")]
    Sigma,
    #[serde(rename = "THOUGHTSPOT")]
    ThoughtSpot,
    #[serde(rename = "DAX")]
    Dax,
    #[serde(rename = "HOLISTICS_AQL")]
    HolisticsAql,
    #[serde(rename = "OSSIE_SQL_2026")]
    OssieSql2026,
}

/// Logical data type, independent of role and physical representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataType {
    String,
    Integer,
    /// Exact base-10.
    Decimal,
    /// Approximate.
    Float,
    Boolean,
    Date,
    Time,
    /// Civil date and time, without timezone or offset.
    DateTime,
    /// An instant: date and time with offset or timezone context.
    DateTimeTz,
    /// Known, but outside the portable vocabulary.
    Opaque,
}

impl DataType {
    #[must_use]
    pub fn is_temporal(self) -> bool {
        matches!(
            self,
            Self::Date | Self::Time | Self::DateTime | Self::DateTimeTz
        )
    }
}

/// Context for AI tools: free text or a structured object. The object
/// accepts additional properties (`additionalProperties: true`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AiContext {
    Text(String),
    Structured(StructuredAiContext),
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StructuredAiContext {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub instructions: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub synonyms: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub examples: Option<Vec<String>>,
    #[serde(flatten)]
    pub other: BTreeMap<String, serde_json::Value>,
}

/// Vendor-specific attributes. `data` is a JSON string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomExtension {
    pub vendor_name: String,
    pub data: String,
}
