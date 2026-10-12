use crate::identity::StableId;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ErrorCode {
    InvalidStableId,
    InvalidSemanticVersion,
    InvalidSourceHandle,
    DuplicateStableId,
    InvalidModelReference,
    MetricDependencyCycle,
    InvalidRelationshipContract,
    InvalidTemporalContract,
    InvalidAllocationContract,
    InvalidMetricRecomposition,
    InvalidWindowSemantics,
    IncompatibleTypes,
    IncompatibleUnits,
    MissingUnitConversion,
    PolicyViolation,
    UnsupportedSemanticConstruct,
    AmbiguousTemporalSelection,
    UniquenessContractViolation,
    TemporalMatchContractViolation,
    AllocationContractViolation,
    NumericOverflow,
    DivisionByZero,
    InvalidCanonicalValue,
    InvalidGrain,
    // Query correctness (`04_CORRECTNESS_AND_PLANNING.md` Part I §21.2).
    AmbiguousRelationshipPath,
    InvalidRelationshipRole,
    UnresolvedFanout,
    InvalidGrainTransition,
    GrainRefinementUnsupported,
    IllegalReaggregation,
    IllegalTemporalReaggregation,
    PolicyContextMissing,
    // Planner capability (`04_CORRECTNESS_AND_PLANNING.md` Part II §25, §33).
    PlannerObligationUnsatisfied,
    PlannerInternalInvariantViolation,
    PlannerIncomplete,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidStableId => "INVALID_STABLE_ID",
            Self::InvalidSemanticVersion => "INVALID_SEMANTIC_VERSION",
            Self::InvalidSourceHandle => "INVALID_SOURCE_HANDLE",
            Self::DuplicateStableId => "DUPLICATE_STABLE_ID",
            Self::InvalidModelReference => "INVALID_MODEL_REFERENCE",
            Self::MetricDependencyCycle => "METRIC_DEPENDENCY_CYCLE",
            Self::InvalidRelationshipContract => "INVALID_RELATIONSHIP_CONTRACT",
            Self::InvalidTemporalContract => "INVALID_TEMPORAL_CONTRACT",
            Self::InvalidAllocationContract => "INVALID_ALLOCATION_CONTRACT",
            Self::InvalidMetricRecomposition => "INVALID_METRIC_RECOMPOSITION",
            Self::InvalidWindowSemantics => "INVALID_WINDOW_SEMANTICS",
            Self::IncompatibleTypes => "INCOMPATIBLE_TYPES",
            Self::IncompatibleUnits => "INCOMPATIBLE_UNITS",
            Self::MissingUnitConversion => "MISSING_UNIT_CONVERSION",
            Self::PolicyViolation => "POLICY_VIOLATION",
            Self::UnsupportedSemanticConstruct => "UNSUPPORTED_SEMANTIC_CONSTRUCT",
            Self::AmbiguousTemporalSelection => "AMBIGUOUS_TEMPORAL_SELECTION",
            Self::UniquenessContractViolation => "UNIQUENESS_CONTRACT_VIOLATION",
            Self::TemporalMatchContractViolation => "TEMPORAL_MATCH_CONTRACT_VIOLATION",
            Self::AllocationContractViolation => "ALLOCATION_CONTRACT_VIOLATION",
            Self::NumericOverflow => "NUMERIC_OVERFLOW",
            Self::DivisionByZero => "DIVISION_BY_ZERO",
            Self::InvalidCanonicalValue => "INVALID_CANONICAL_VALUE",
            Self::InvalidGrain => "INVALID_GRAIN",
            Self::AmbiguousRelationshipPath => "AMBIGUOUS_RELATIONSHIP_PATH",
            Self::InvalidRelationshipRole => "INVALID_RELATIONSHIP_ROLE",
            Self::UnresolvedFanout => "UNRESOLVED_FANOUT",
            Self::InvalidGrainTransition => "INVALID_GRAIN_TRANSITION",
            Self::GrainRefinementUnsupported => "GRAIN_REFINEMENT_UNSUPPORTED",
            Self::IllegalReaggregation => "ILLEGAL_REAGGREGATION",
            Self::IllegalTemporalReaggregation => "ILLEGAL_TEMPORAL_REAGGREGATION",
            Self::PolicyContextMissing => "POLICY_CONTEXT_MISSING",
            Self::PlannerObligationUnsatisfied => "PLANNER_OBLIGATION_UNSATISFIED",
            Self::PlannerInternalInvariantViolation => "PLANNER_INTERNAL_INVARIANT_VIOLATION",
            Self::PlannerIncomplete => "PLANNER_INCOMPLETE",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// Compiling the semantic model (CSMIR).
    Model,
    /// Validating a bound query against a compiled model.
    Correctness,
    /// Building the semantic plan (SEIR) of a validated query.
    Planning,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Evidence {
    Reference { role: String, object: StableId },
    Constraint { name: String },
    TypeMismatch { expected: String, actual: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceSpan {
    pub source: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub schema_version: &'static str,
    pub code: ErrorCode,
    pub phase: Phase,
    pub severity: Severity,
    pub primary_ref: Option<StableId>,
    pub related_refs: Vec<StableId>,
    pub query_refs: Vec<String>,
    pub evidence: Vec<Evidence>,
    pub suggestion_codes: Vec<String>,
    pub message: String,
    pub source_span: Option<SourceSpan>,
}

impl Diagnostic {
    pub fn error(
        code: ErrorCode,
        primary_ref: Option<StableId>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: "0.1",
            code,
            phase: Phase::Model,
            severity: Severity::Error,
            primary_ref,
            related_refs: Vec::new(),
            query_refs: Vec::new(),
            evidence: Vec::new(),
            suggestion_codes: Vec::new(),
            message: message.into(),
            source_span: None,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}
impl std::error::Error for Diagnostic {}
