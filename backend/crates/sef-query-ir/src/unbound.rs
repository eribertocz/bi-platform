//! The external request before binding (§3.1): names instead of stable
//! IDs and untyped literal text. A front end (HTTP, CLI, a future SQL
//! dialect, an AI assistant) produces this; only [`crate::bind`] turns it
//! into SQIR.

use sef_model_ir::expressions::ComparisonOp;
use sef_model_ir::time::TimeGrain;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UnboundQuerySet {
    pub blocks: Vec<UnboundBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct UnboundBlock {
    pub block_id: String,
    pub selections: Vec<UnboundSelection>,
    pub filter: Option<UnboundFilter>,
    pub post_aggregate_filter: Option<UnboundFilter>,
    pub order_by: Vec<UnboundOrder>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnboundSelection {
    pub selection_id: String,
    pub item: UnboundItem,
    pub output_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnboundItem {
    /// A metric's name, label or stable ID.
    Metric(String),
    Dimension(UnboundDimension),
    TimeDimension {
        dimension: UnboundDimension,
        grain: TimeGrain,
        /// IANA zone overriding the governed one.
        timezone: Option<String>,
        /// Calendar stable ID overriding the governed one.
        calendar: Option<String>,
    },
}

/// A dimension's name, label or stable ID, optionally qualified by the
/// relationship roles it is reached through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnboundDimension {
    pub name: String,
    pub roles: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnboundFilter {
    Compare {
        target: UnboundTarget,
        op: ComparisonOp,
        value: Value,
    },
    Between {
        target: UnboundTarget,
        low: Value,
        high: Value,
    },
    In {
        target: UnboundTarget,
        values: Vec<Value>,
    },
    IsNull(UnboundTarget),
    IsNotNull(UnboundTarget),
    And(Vec<UnboundFilter>),
    Or(Vec<UnboundFilter>),
    Not(Box<UnboundFilter>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnboundTarget {
    Dimension(UnboundDimension),
    /// A metric selection of the same block, by its `selection_id`.
    Metric(String),
}

/// Literal as the caller wrote it; binding types it after its target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Null,
    Boolean(bool),
    /// Exact decimal text (`"12.50"`), never a binary float.
    Number(String),
    Text(String),
    /// ISO `YYYY-MM-DD`.
    Date(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnboundOrder {
    pub selection_id: String,
    pub descending: bool,
    /// Nulls last unless stated: an ordered result always says where nulls
    /// go instead of inheriting an engine's default.
    pub nulls_first: bool,
}
