//! Bound SQIR: what a caller asks, with every semantic reference resolved
//! to a CSMIR stable ID and every literal typed (`03_MODEL_AND_QUERY_IR.md`,
//! Part II). It holds intent only: no SQL, no join path, no identity or
//! policy values, no cost or freshness requirements.

use sef_model_ir::canonical::{Canonical, CanonicalValue as C, Domain, Fingerprint, fingerprint};
use sef_model_ir::expressions::ComparisonOp;
use sef_model_ir::identity::StableId;
use sef_model_ir::time::{IanaZoneId, SortDirection, TimeGrain};
use sef_model_ir::types::Literal;

/// One request: independent blocks, each with one unambiguous grain
/// (§4). Several grains are several blocks, never implicit re-aggregation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticQuerySet {
    pub blocks: Vec<SemanticQueryBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticQueryBlock {
    /// Query-local; names the block's result in the response.
    pub block_id: String,
    /// Output order is part of the result contract and is preserved.
    pub selections: Vec<Selection>,
    /// Pre-aggregate predicate over dimensions.
    pub filter: Option<FilterExpr>,
    /// Semantic `HAVING` over metric selections of this block.
    pub post_aggregate_filter: Option<FilterExpr>,
    pub order_by: Vec<OrderTerm>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Query-local; referenced by ordering and metric filters.
    pub selection_id: String,
    pub item: SelectionItem,
    /// Presentation only: excluded from semantic equivalence.
    pub output_label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionItem {
    Metric(StableId),
    Dimension(QualifiedDimensionRef),
    TimeDimension(TimeDimensionSelection),
}

/// A dimension, optionally reached through named relationship roles
/// (`country` via `billing_customer`): semantic intent, not a join path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct QualifiedDimensionRef {
    pub dimension: StableId,
    /// Relationship roles, sorted and unique; empty when not qualified.
    pub roles: Vec<String>,
}

/// A time dimension bucketed at a grain, optionally overriding the
/// governed zone or calendar (whether that is legal is a correctness
/// question, not a binding one).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeDimensionSelection {
    pub dimension: QualifiedDimensionRef,
    pub grain: TimeGrain,
    pub timezone: Option<IanaZoneId>,
    pub calendar: Option<StableId>,
}

/// Typed predicate (§9). Null intent is explicit: there is no `= NULL`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterExpr {
    Compare {
        target: FilterTarget,
        op: ComparisonOp,
        value: Literal,
    },
    Between {
        target: FilterTarget,
        low: Literal,
        high: Literal,
    },
    In {
        target: FilterTarget,
        values: Vec<Literal>,
    },
    IsNull(FilterTarget),
    IsNotNull(FilterTarget),
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
    Not(Box<FilterExpr>),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FilterTarget {
    /// Pre-aggregate: a dimension value.
    Dimension(QualifiedDimensionRef),
    /// Post-aggregate: a metric selection of the same block.
    Metric(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderTerm {
    pub selection_id: String,
    pub direction: SortDirection,
    pub nulls: NullOrdering,
}

/// A certified ordered result states where nulls go (Part III, C.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NullOrdering {
    First,
    Last,
}

/// One atom of the grain a block requests (§8).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RequestedAtom {
    Dimension(QualifiedDimensionRef),
    TimeBucket(TimeDimensionSelection),
}

impl SemanticQueryBlock {
    /// The grain the block asks for: its selected dimensions and time
    /// buckets, as a set. Empty is the global (scalar) grain.
    #[must_use]
    pub fn requested_grain(&self) -> Vec<RequestedAtom> {
        let mut atoms: Vec<RequestedAtom> = self
            .selections
            .iter()
            .filter_map(|selection| match &selection.item {
                SelectionItem::Metric(_) => None,
                SelectionItem::Dimension(dimension) => {
                    Some(RequestedAtom::Dimension(dimension.clone()))
                }
                SelectionItem::TimeDimension(time) => Some(RequestedAtom::TimeBucket(time.clone())),
            })
            .collect();
        atoms.sort();
        atoms.dedup();
        atoms
    }

    /// The metric a metric selection names.
    #[must_use]
    pub fn metric_of(&self, selection_id: &str) -> Option<&StableId> {
        self.selections
            .iter()
            .find(|selection| selection.selection_id == selection_id)
            .and_then(|selection| match &selection.item {
                SelectionItem::Metric(metric) => Some(metric),
                _ => None,
            })
    }
}

impl SemanticQuerySet {
    /// The canonical fingerprint of the query's meaning (domain `SQIR`).
    /// Output labels do not take part; selection and block order do,
    /// because they are the result's shape.
    ///
    /// # Errors
    /// Only if canonical encoding exceeds its resource budget.
    pub fn fingerprint(&self) -> Result<Fingerprint, sef_model_ir::canonical::CanonicalError> {
        fingerprint(Domain::Sqir, self)
    }
}

// ---- Canonical encoding ------------------------------------------------------

impl Canonical for SemanticQuerySet {
    fn canonical_value(&self) -> C {
        C::record(
            "semantic-query-set",
            [
                ("sqir", C::text("0.1")),
                ("blocks", C::sequence(&self.blocks)),
            ],
        )
    }
}

impl Canonical for SemanticQueryBlock {
    fn canonical_value(&self) -> C {
        C::record(
            "semantic-query-block",
            [
                ("block_id", C::text(self.block_id.clone())),
                ("selections", C::sequence(&self.selections)),
                ("filter", C::optional(self.filter.as_ref())),
                (
                    "post_aggregate_filter",
                    C::optional(self.post_aggregate_filter.as_ref()),
                ),
                ("order_by", C::sequence(&self.order_by)),
                ("limit", C::optional(self.limit.as_ref())),
                ("offset", C::optional(self.offset.as_ref())),
            ],
        )
    }
}

impl Canonical for Selection {
    fn canonical_value(&self) -> C {
        // `output_label` is presentation: deliberately absent.
        C::record(
            "selection",
            [
                ("selection_id", C::text(self.selection_id.clone())),
                ("item", self.item.canonical_value()),
            ],
        )
    }
}

impl Canonical for SelectionItem {
    fn canonical_value(&self) -> C {
        match self {
            Self::Metric(metric) => {
                C::record("selection.metric", [("metric", metric.canonical_value())])
            }
            Self::Dimension(dimension) => C::record(
                "selection.dimension",
                [("dimension", dimension.canonical_value())],
            ),
            Self::TimeDimension(time) => C::record(
                "selection.time-dimension",
                [("time", time.canonical_value())],
            ),
        }
    }
}

impl Canonical for QualifiedDimensionRef {
    fn canonical_value(&self) -> C {
        C::record(
            "qualified-dimension",
            [
                ("dimension", self.dimension.canonical_value()),
                (
                    "roles",
                    C::Set(
                        self.roles
                            .iter()
                            .map(|role| C::text(role.clone()))
                            .collect(),
                    ),
                ),
            ],
        )
    }
}

impl Canonical for TimeDimensionSelection {
    fn canonical_value(&self) -> C {
        C::record(
            "time-dimension-selection",
            [
                ("dimension", self.dimension.canonical_value()),
                ("grain", self.grain.canonical_value()),
                ("timezone", C::optional(self.timezone.as_ref())),
                ("calendar", C::optional(self.calendar.as_ref())),
            ],
        )
    }
}

impl Canonical for RequestedAtom {
    fn canonical_value(&self) -> C {
        match self {
            Self::Dimension(dimension) => C::record(
                "requested-atom.dimension",
                [("dimension", dimension.canonical_value())],
            ),
            Self::TimeBucket(time) => C::record(
                "requested-atom.time-bucket",
                [("time", time.canonical_value())],
            ),
        }
    }
}

impl Canonical for FilterTarget {
    fn canonical_value(&self) -> C {
        match self {
            Self::Dimension(dimension) => C::record(
                "filter-target.dimension",
                [("dimension", dimension.canonical_value())],
            ),
            Self::Metric(selection_id) => C::record(
                "filter-target.metric",
                [("selection_id", C::text(selection_id.clone()))],
            ),
        }
    }
}

impl Canonical for FilterExpr {
    fn canonical_value(&self) -> C {
        match self {
            Self::Compare { target, op, value } => C::record(
                "filter.compare",
                [
                    ("target", target.canonical_value()),
                    ("op", op.canonical_value()),
                    ("value", value.canonical_value()),
                ],
            ),
            Self::Between { target, low, high } => C::record(
                "filter.between",
                [
                    ("target", target.canonical_value()),
                    ("low", low.canonical_value()),
                    ("high", high.canonical_value()),
                ],
            ),
            Self::In { target, values } => C::record(
                "filter.in",
                [
                    ("target", target.canonical_value()),
                    ("values", C::set(values)),
                ],
            ),
            Self::IsNull(target) => {
                C::record("filter.is-null", [("target", target.canonical_value())])
            }
            Self::IsNotNull(target) => {
                C::record("filter.is-not-null", [("target", target.canonical_value())])
            }
            Self::And(operands) => C::record("filter.and", [("operands", C::set(operands))]),
            Self::Or(operands) => C::record("filter.or", [("operands", C::set(operands))]),
            Self::Not(operand) => C::record("filter.not", [("operand", operand.canonical_value())]),
        }
    }
}

impl Canonical for OrderTerm {
    fn canonical_value(&self) -> C {
        C::record(
            "order-term",
            [
                ("selection_id", C::text(self.selection_id.clone())),
                ("direction", self.direction.canonical_value()),
                (
                    "nulls",
                    C::text(match self.nulls {
                        NullOrdering::First => "first",
                        NullOrdering::Last => "last",
                    }),
                ),
            ],
        )
    }
}

// ---- Normalization -------------------------------------------------------------

impl FilterExpr {
    /// Semantics-preserving normal form under SQL's three-valued logic:
    /// nested `AND`/`OR` flattened, operands and `IN` values deduplicated
    /// (their order is already irrelevant to the fingerprint), a single
    /// operand unwrapped and `NOT NOT x` reduced to `x`.
    #[must_use]
    pub fn normalized(self) -> Self {
        match self {
            Self::And(operands) => flatten(operands, true),
            Self::Or(operands) => flatten(operands, false),
            Self::Not(operand) => match operand.normalized() {
                Self::Not(inner) => *inner,
                other => Self::Not(Box::new(other)),
            },
            Self::In { target, mut values } => {
                values.sort_by_key(canonical_bytes);
                values.dedup();
                Self::In { target, values }
            }
            leaf => leaf,
        }
    }
}

fn flatten(operands: Vec<FilterExpr>, and: bool) -> FilterExpr {
    let mut flat = Vec::with_capacity(operands.len());
    for operand in operands {
        match (operand.normalized(), and) {
            (FilterExpr::And(inner), true) | (FilterExpr::Or(inner), false) => flat.extend(inner),
            (other, _) => flat.push(other),
        }
    }
    flat.sort_by_key(canonical_bytes);
    flat.dedup();
    match <[FilterExpr; 1]>::try_from(flat) {
        Ok([single]) => single,
        Err(flat) if and => FilterExpr::And(flat),
        Err(flat) => FilterExpr::Or(flat),
    }
}

/// A total order consistent with the canonical encoding; encoding failure
/// (resource budget) sorts last and never panics.
fn canonical_bytes<T: Canonical>(value: &T) -> Vec<u8> {
    value
        .canonical_value()
        .encode()
        .unwrap_or_else(|_| vec![u8::MAX])
}
