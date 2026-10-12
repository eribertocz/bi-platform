//! Rejections as the stable semantic diagnostic (`04_CORRECTNESS_AND_PLANNING.md`
//! Part VIII §32), shared with model compilation: code, phase, refs and
//! evidence are the machine contract; the message is for humans only.

use sef_model_ir::diagnostic::{Diagnostic, ErrorCode, Evidence, Phase, Severity};
use sef_model_ir::identity::StableId;

/// The analysis area a code belongs to (§21.1), for grouping in reports.
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
    Model,
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
            Self::Model => "MODEL",
        }
    }

    /// The domain of a diagnostic code.
    #[must_use]
    pub const fn of(code: ErrorCode) -> Self {
        match code {
            ErrorCode::AmbiguousRelationshipPath | ErrorCode::InvalidRelationshipRole => {
                Self::Relationship
            }
            ErrorCode::UnresolvedFanout => Self::Fanout,
            ErrorCode::InvalidGrainTransition
            | ErrorCode::GrainRefinementUnsupported
            | ErrorCode::IllegalReaggregation
            | ErrorCode::InvalidGrain => Self::Grain,
            ErrorCode::IllegalTemporalReaggregation | ErrorCode::AmbiguousTemporalSelection => {
                Self::Time
            }
            ErrorCode::IncompatibleTypes
            | ErrorCode::IncompatibleUnits
            | ErrorCode::MissingUnitConversion => Self::TypeUnit,
            ErrorCode::InvalidWindowSemantics | ErrorCode::InvalidMetricRecomposition => {
                Self::Metric
            }
            ErrorCode::PolicyContextMissing | ErrorCode::PolicyViolation => Self::Policy,
            ErrorCode::UnsupportedSemanticConstruct => Self::Unsupported,
            _ => Self::Model,
        }
    }
}

/// A correctness rejection: `path` is where in the bound query it arises,
/// `refs` the model objects involved (the first one is primary).
pub(crate) fn rejection(
    code: ErrorCode,
    path: impl Into<String>,
    refs: impl IntoIterator<Item = StableId>,
    message: impl Into<String>,
) -> Diagnostic {
    let mut refs = refs.into_iter();
    Diagnostic {
        schema_version: "0.1",
        code,
        phase: Phase::Correctness,
        severity: Severity::Error,
        primary_ref: refs.next(),
        related_refs: refs.collect(),
        query_refs: vec![path.into()],
        evidence: vec![Evidence::Constraint {
            name: code.as_str().to_owned(),
        }],
        suggestion_codes: Vec::new(),
        message: message.into(),
        source_span: None,
    }
}

/// Canonical order of diagnostics: by query location, then code and refs,
/// so the same rejection always reads the same.
pub(crate) fn canonical(mut found: Vec<Diagnostic>) -> Vec<Diagnostic> {
    found.sort_by(|a, b| {
        (
            &a.query_refs,
            a.code,
            &a.primary_ref,
            &a.related_refs,
            &a.message,
        )
            .cmp(&(
                &b.query_refs,
                b.code,
                &b.primary_ref,
                &b.related_refs,
                &b.message,
            ))
    });
    found.dedup();
    found
}
