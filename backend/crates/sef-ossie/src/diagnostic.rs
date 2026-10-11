//! Structured diagnostics: a stable code for programs, a JSON-pointer path
//! into the document, a Spanish message for people and, when the parser
//! knows it, the line and column.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    /// The document is accepted; the author should look at it.
    Warning,
}

/// Stable diagnostic codes. Their names never change once released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    /// Not YAML or JSON, a duplicate key, a resource budget exceeded, or a
    /// shape the schema rejects (unknown property, wrong type, missing field).
    Syntax,
    /// Larger than [`crate::load::MAX_DOCUMENT_BYTES`].
    TooLarge,
    /// The pre-0.2 `semantic_model: [...]` wrapper.
    LegacyWrapper,
    /// `version` is not the supported specification version.
    UnsupportedVersion,
    /// A name, source or relationship endpoint is empty.
    EmptyValue,
    /// A list that must not be empty is empty.
    EmptyList,
    /// A dataset, field, metric, relationship or column name that is not a
    /// valid Ossie identifier (syntax or more than 128 characters).
    InvalidIdentifier,
    /// Two datasets, fields of one dataset, metrics or relationships share a
    /// name, compared as normalized identifiers.
    DuplicateName,
    /// A relationship names a dataset the model does not declare.
    UnknownDataset,
    /// `from_columns` and `to_columns` differ in length.
    ColumnArity,
    /// `to_columns` covers none of the declared keys of the `to` dataset.
    KeyNotCovered,
    /// A portable expression does not parse as exactly one expression.
    ExpressionSyntax,
    /// An expression exceeds its byte, token, structural or parser recursion budget.
    ExpressionTooLarge,
    /// A construct outside `OSSIE_SQL_2026` (subquery, vendor syntax…).
    UnsupportedConstruct,
    /// A function that is neither REQUIRED nor RECOMMENDED.
    UnknownFunction,
    /// An aggregate or window in a field, which is a row-level value.
    AggregateInField,
    /// An aggregate inside another aggregate.
    NestedAggregate,
    /// A `custom_extensions` entry of ours that is malformed, repeated, of
    /// another version, or incoherent with the object it extends.
    InvalidExtension,
    /// The catalog does not know a dataset's `source`.
    UnknownSource,
    /// A field, key or relationship names a column the source lacks, or a
    /// field that is not a column.
    UnknownColumn,
    /// An expression or declaration names something the model lacks.
    UnknownReference,
    /// A name that matches several columns or fields.
    AmbiguousReference,
    /// `datatype` contradicts the type the source or expression has.
    DatatypeMismatch,
    /// An explicit time role on a text datatype, which needs author review.
    NonTemporalTimeRole,
    /// Operand or result types that CSMIR does not combine implicitly.
    TypeMismatch,
    /// Valid `OSSIE_SQL_2026` the engine cannot yet compute exactly.
    NotRepresentable,
    /// A metric combining inputs of different grains without declaring its own.
    GrainRequired,
    /// Metrics that depend on themselves.
    ReferenceCycle,
    /// CSMIR rejected the converted model (types, units, grains…); the
    /// message carries CSMIR's own code.
    SemanticContract,
}

impl Code {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Syntax => "syntax",
            Self::TooLarge => "too_large",
            Self::LegacyWrapper => "legacy_wrapper",
            Self::UnsupportedVersion => "unsupported_version",
            Self::EmptyValue => "empty_value",
            Self::EmptyList => "empty_list",
            Self::InvalidIdentifier => "invalid_identifier",
            Self::DuplicateName => "duplicate_name",
            Self::UnknownDataset => "unknown_dataset",
            Self::ColumnArity => "column_arity",
            Self::KeyNotCovered => "key_not_covered",
            Self::ExpressionSyntax => "expression_syntax",
            Self::ExpressionTooLarge => "expression_too_large",
            Self::UnsupportedConstruct => "unsupported_construct",
            Self::UnknownFunction => "unknown_function",
            Self::AggregateInField => "aggregate_in_field",
            Self::NestedAggregate => "nested_aggregate",
            Self::InvalidExtension => "invalid_extension",
            Self::UnknownSource => "unknown_source",
            Self::UnknownColumn => "unknown_column",
            Self::UnknownReference => "unknown_reference",
            Self::AmbiguousReference => "ambiguous_reference",
            Self::DatatypeMismatch => "datatype_mismatch",
            Self::NonTemporalTimeRole => "non_temporal_time_role",
            Self::TypeMismatch => "type_mismatch",
            Self::NotRepresentable => "not_representable",
            Self::GrainRequired => "grain_required",
            Self::ReferenceCycle => "reference_cycle",
            Self::SemanticContract => "semantic_contract",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Code,
    /// JSON pointer (RFC 6901) to the offending value; empty for the root.
    pub path: String,
    pub message: String,
    /// 1-based line and column, when the parser reports them.
    pub location: Option<(u64, u64)>,
}

impl Diagnostic {
    pub(crate) fn error(code: Code, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code,
            path: path.into(),
            message: message.into(),
            location: None,
        }
    }

    pub(crate) fn warning(code: Code, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, path, message)
        }
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "aviso",
        };
        write!(f, "{severity}[{}]", self.code.as_str())?;
        if let Some((line, column)) = self.location {
            write!(f, " {line}:{column}")?;
        }
        if !self.path.is_empty() {
            write!(f, " {}", self.path)?;
        }
        write!(f, ": {}", self.message)
    }
}
