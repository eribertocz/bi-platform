//! Structured semantic errors (`04_CORRECTNESS_AND_PLANNING.md`, Part I
//! §21): stable codes, never backend strings. A rejection produces no plan.

use std::fmt;

use sef_model_ir::identity::StableId;

/// Where in the analysis a rejection arises (§21.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorDomain {
    Query,
    TypeUnit,
    Grain,
    Relationship,
    Fanout,
    Metric,
    Time,
    Policy,
    Unsupported,
}

impl ErrorDomain {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "QUERY",
            Self::TypeUnit => "TYPE_UNIT",
            Self::Grain => "GRAIN",
            Self::Relationship => "RELATIONSHIP",
            Self::Fanout => "FANOUT",
            Self::Metric => "METRIC",
            Self::Time => "TIME",
            Self::Policy => "POLICY",
            Self::Unsupported => "UNSUPPORTED",
        }
    }
}

/// Stable external codes (§21.2). Model-domain codes, including an
/// unproven cardinality (`INVALID_RELATIONSHIP_CONTRACT`), are reported by
/// CSMIR compilation, which is this engine's precondition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticErrorCode {
    AmbiguousRelationshipPath,
    InvalidRelationshipRole,
    UnresolvedFanout,
    InvalidGrainTransition,
    GrainRefinementUnsupported,
    IllegalReaggregation,
    IllegalTemporalReaggregation,
    IncompatibleTypes,
    InvalidWindowSemantics,
    PolicyContextMissing,
    UnsupportedSemanticConstruct,
}

impl SemanticErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmbiguousRelationshipPath => "AMBIGUOUS_RELATIONSHIP_PATH",
            Self::InvalidRelationshipRole => "INVALID_RELATIONSHIP_ROLE",
            Self::UnresolvedFanout => "UNRESOLVED_FANOUT",
            Self::InvalidGrainTransition => "INVALID_GRAIN_TRANSITION",
            Self::GrainRefinementUnsupported => "GRAIN_REFINEMENT_UNSUPPORTED",
            Self::IllegalReaggregation => "ILLEGAL_REAGGREGATION",
            Self::IllegalTemporalReaggregation => "ILLEGAL_TEMPORAL_REAGGREGATION",
            Self::IncompatibleTypes => "INCOMPATIBLE_TYPES",
            Self::InvalidWindowSemantics => "INVALID_WINDOW_SEMANTICS",
            Self::PolicyContextMissing => "POLICY_CONTEXT_MISSING",
            Self::UnsupportedSemanticConstruct => "UNSUPPORTED_SEMANTIC_CONSTRUCT",
        }
    }

    #[must_use]
    pub const fn domain(self) -> ErrorDomain {
        match self {
            Self::AmbiguousRelationshipPath | Self::InvalidRelationshipRole => {
                ErrorDomain::Relationship
            }
            Self::UnresolvedFanout => ErrorDomain::Fanout,
            Self::InvalidGrainTransition
            | Self::GrainRefinementUnsupported
            | Self::IllegalReaggregation => ErrorDomain::Grain,
            Self::IllegalTemporalReaggregation => ErrorDomain::Time,
            Self::IncompatibleTypes => ErrorDomain::TypeUnit,
            Self::InvalidWindowSemantics => ErrorDomain::Metric,
            Self::PolicyContextMissing => ErrorDomain::Policy,
            Self::UnsupportedSemanticConstruct => ErrorDomain::Unsupported,
        }
    }
}

/// One rejection: what, where in the query, which model objects, and why.
/// Suggestions help the caller disambiguate; they never mutate the query.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SemanticError {
    pub code: SemanticErrorCode,
    /// JSON path into the bound query (`/blocks/0/selections/1`).
    pub path: String,
    pub object_refs: Vec<StableId>,
    pub message: String,
    pub suggestions: Vec<String>,
}

impl SemanticError {
    pub(crate) fn new(
        code: SemanticErrorCode,
        path: impl Into<String>,
        object_refs: impl IntoIterator<Item = StableId>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: path.into(),
            object_refs: object_refs.into_iter().collect(),
            message: message.into(),
            suggestions: Vec::new(),
        }
    }

    pub(crate) fn suggest(mut self, suggestions: impl IntoIterator<Item = String>) -> Self {
        self.suggestions.extend(suggestions);
        self
    }

    #[must_use]
    pub fn domain(&self) -> ErrorDomain {
        self.code.domain()
    }
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} {}: {}",
            self.code.domain().as_str(),
            self.code.as_str(),
            self.path,
            self.message
        )
    }
}

impl std::error::Error for SemanticError {}
