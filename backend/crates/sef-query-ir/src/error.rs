//! What binding rejects (§16): problems of the request itself. Grain,
//! fanout, path ambiguity, units or policies are the Correctness Engine's
//! and never appear here.

use std::fmt;

/// Stable codes. Their names never change once released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryErrorCode {
    /// The request has no block.
    EmptyQuery,
    /// Two blocks share a `block_id`, or one is empty.
    DuplicateBlock,
    /// A block selects no metric.
    NoMetric,
    /// Two selections of a block share a `selection_id`, or one is empty.
    DuplicateSelection,
    UnknownMetric,
    UnknownDimension,
    /// A name that matches several metrics or dimensions.
    AmbiguousName,
    /// A role that no relationship of the model declares.
    UnknownRole,
    /// A literal that does not fit its target's type without loss, or a
    /// zone or calendar that is not valid.
    InvalidLiteral,
    /// `= NULL`, `IN (NULL)` and the like: use `IS NULL`.
    NullComparison,
    /// `IN ()`.
    EmptyList,
    /// A time bucket on a dimension that is not a date or a timestamp.
    NotTemporal,
    /// A pre-aggregate filter on a metric, a post-aggregate filter on a
    /// dimension, or a reference to a missing or non-metric selection.
    InvalidFilterTarget,
    /// An ordering that names no selection of its block.
    UnknownSelection,
    /// `offset` without `limit`, or a row subset without explicit order.
    InvalidPagination,
}

impl QueryErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmptyQuery => "empty_query",
            Self::DuplicateBlock => "duplicate_block",
            Self::NoMetric => "no_metric",
            Self::DuplicateSelection => "duplicate_selection",
            Self::UnknownMetric => "unknown_metric",
            Self::UnknownDimension => "unknown_dimension",
            Self::AmbiguousName => "ambiguous_name",
            Self::UnknownRole => "unknown_role",
            Self::InvalidLiteral => "invalid_literal",
            Self::NullComparison => "null_comparison",
            Self::EmptyList => "empty_list",
            Self::NotTemporal => "not_temporal",
            Self::InvalidFilterTarget => "invalid_filter_target",
            Self::UnknownSelection => "unknown_selection",
            Self::InvalidPagination => "invalid_pagination",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError {
    pub code: QueryErrorCode,
    /// JSON pointer into the request (`/blocks/0/selections/2`).
    pub path: String,
    /// For people, in Spanish.
    pub message: String,
}

impl QueryError {
    pub(crate) fn new(
        code: QueryErrorCode,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.code.as_str(), self.path, self.message)
    }
}

impl std::error::Error for QueryError {}
