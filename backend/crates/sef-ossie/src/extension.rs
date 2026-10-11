//! Our `custom_extensions` entry (`vendor_name: bi-platform`): the CSMIR
//! semantics Ossie 0.2 cannot express yet, one schema per document level.
//!
//! Every payload is a JSON object with `"version": 1` and only the keys of
//! its level; anything else is an error, never ignored, because a typo in
//! a unit or a grain would silently change what a metric means. Other
//! vendors' extensions are not ours to interpret and are left alone.
//!
//! Names follow `MetricFlow` where the concept matches (entities, grain),
//! and the open items of Ossie's roadmap are tracked in
//! `docs/referentes/ossie.md` so they converge when the standard grows.

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::diagnostic::{Code, Diagnostic};
use crate::document::CustomExtension;

/// Provisional vendor name until the product has one (one migration).
pub const VENDOR: &str = "bi-platform";

/// The only payload version this crate reads.
const VERSION: u32 = 1;

/// `semantic_model.custom_extensions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelExtension {
    #[serde(default)]
    version: u32,
    /// `SemVer` of every object of the model; `0.0.0` (unversioned) if absent.
    pub semantic_version: Option<String>,
    /// Many-to-many paths through a bridge dataset, with their allocation.
    #[serde(default)]
    pub bridges: Vec<BridgeDeclaration>,
    /// Row filters enforced by the engine.
    #[serde(default)]
    pub policies: Vec<PolicyDeclaration>,
}

/// A bridge `left` <- `bridge` -> `right` (both relationships start at the
/// bridge), optionally allocating `left`'s measures across `right`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeDeclaration {
    pub name: String,
    pub left: String,
    pub bridge: String,
    pub right: String,
    pub left_relationship: String,
    pub right_relationship: String,
    pub allocation: Option<AllocationDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllocationDeclaration {
    /// Row-level expression of the bridge dataset (a weight column).
    pub weight: String,
    /// Where the weights must add up to `required_total`.
    pub normalization_scope: GrainDeclaration,
    /// Exact decimals at the weight's type (`"1.00"`, `"0.00"`).
    pub required_total: String,
    pub tolerance: String,
}

/// A mandatory or governed row filter on a dataset or a metric.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDeclaration {
    pub name: String,
    /// Exactly one of `dataset` and `metric`.
    pub dataset: Option<String>,
    pub metric: Option<String>,
    pub enforcement: EnforcementDeclaration,
    /// Row-level `OSSIE_SQL_2026` condition. `:name` is a context value of
    /// the request, always bound as a parameter, never spliced into SQL.
    pub predicate: String,
    #[serde(default)]
    pub context: Vec<SlotDeclaration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementDeclaration {
    Mandatory,
    OptionalGoverned,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotDeclaration {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: SlotType,
    #[serde(default = "required")]
    pub required: bool,
    pub sensitivity: SensitivityDeclaration,
}

fn required() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotType {
    String,
    Integer,
    Boolean,
    Date,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensitivityDeclaration {
    Public,
    Sensitive,
    Secret,
}

/// `datasets[].custom_extensions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetExtension {
    #[serde(default)]
    version: u32,
    /// The entity the primary key identifies; the dataset name if absent.
    pub entity: Option<String>,
    /// Further keys of other entities, when no relationship implies them
    /// (an identifier without a dataset of its own, an as-of history).
    #[serde(default)]
    pub keys: Vec<KeyDeclaration>,
    /// The observation grain when it is not the primary key, e.g. a
    /// monthly table keyed by month and region whose rows are cells of
    /// those conformed dimensions.
    pub grain: Option<GrainDeclaration>,
    /// Measures of this dataset that no metric exposes (a rollup's own
    /// totals, for instance).
    #[serde(default)]
    pub measures: Vec<MeasureDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasureDeclaration {
    pub name: String,
    /// One `OSSIE_SQL_2026` aggregate over this dataset's fields.
    pub expression: String,
    pub aggregation: Option<AggregationDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyDeclaration {
    pub entity: String,
    /// Field names, in the order of the entity's key components.
    pub fields: Vec<String>,
    #[serde(default)]
    pub unique: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrainDeclaration {
    /// Dimension names (see [`FieldExtension::dimension`]).
    #[serde(default)]
    pub dimensions: Vec<String>,
    /// Entity names.
    #[serde(default)]
    pub entities: Vec<String>,
}

/// `datasets[].fields[].custom_extensions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldExtension {
    #[serde(default)]
    version: u32,
    /// The model-wide dimension this field is a binding of. Fields of
    /// several datasets naming the same dimension are conformed (same
    /// domain); without it the dimension is `<dataset>.<field>`.
    pub dimension: Option<String>,
    pub unit: Option<UnitDeclaration>,
    /// The role of this column on its dataset's timeline.
    pub time_role: Option<TimeRoleDeclaration>,
    /// Required by `DATE_TRUNC`: where the day boundaries are.
    pub time: Option<TimeDeclaration>,
    /// Free semantic role of the dimension (e.g. `business_time`).
    pub semantic_role: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeRoleDeclaration {
    Event,
    Snapshot,
    ValidityStart,
    ValidityEnd,
    Processing,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeDeclaration {
    /// IANA zone (`UTC`, `America/La_Paz`).
    pub timezone: String,
    /// The tz database release the zone rules come from (`2025b`).
    pub tzdb: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum UnitDeclaration {
    Count,
    Ratio,
    Percent,
    /// ISO 4217 code (`USD`).
    Currency(String),
    /// UCUM code (`kg`).
    Ucum(String),
    Custom {
        namespace: String,
        code: String,
    },
}

/// `relationships[].custom_extensions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelationshipExtension {
    #[serde(default)]
    version: u32,
    /// Whether every row of `from` has a match in `to`. Unknown (and
    /// treated as optional, which never drops rows) unless declared.
    pub optionality: Option<OptionalityDeclaration>,
    /// An as-of match: each `from` row joins the `to` version valid at its
    /// event time.
    pub temporal: Option<TemporalDeclaration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalDeclaration {
    /// `dataset.field` of `from`.
    pub event_time: String,
    /// `dataset.field` of `to`.
    pub valid_from: String,
    pub valid_to: Option<String>,
    pub boundary: BoundaryDeclaration,
    #[serde(rename = "match")]
    pub match_cardinality: MatchDeclaration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryDeclaration {
    ClosedOpen,
    ClosedClosed,
    OpenClosed,
    OpenOpen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchDeclaration {
    ExactlyOne,
    AtMostOne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionalityDeclaration {
    Required,
    Optional,
}

/// `metrics[].custom_extensions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricExtension {
    #[serde(default)]
    version: u32,
    /// The dataset whose rows `COUNT(*)` counts.
    pub dataset: Option<String>,
    /// The grain the metric is natively defined at, when its inputs do not
    /// share one.
    pub grain: Option<GrainDeclaration>,
    pub unit: Option<UnitDeclaration>,
    /// Rounding of an exact division to its declared decimal type
    /// (half-even if absent).
    pub rounding: Option<RoundingDeclaration>,
    /// What a zero denominator yields (`NULL` if absent).
    pub zero_division: Option<ZeroDivisionDeclaration>,
    /// How the metric re-aggregates along each axis. If absent, `SUM` and
    /// `COUNT` are fully additive; `MIN`, `MAX` and `COUNT(DISTINCT)` are
    /// recomputed from rows; a composite metric is recomputed from its
    /// components. Declaring a composite `fully_additive` is the author's
    /// claim that summing its partial results is exact: no group may leave
    /// one of its components `NULL`.
    pub aggregation: Option<AggregationDeclaration>,
    /// Which row of each period a semi-additive metric keeps.
    pub time_behavior: Option<TimeBehaviorDeclaration>,
    /// The bridge (see [`BridgeDeclaration`]) that allocates this metric.
    pub allocation_bridge: Option<String>,
}

/// One grain atom: exactly one of `dimension` and `entity`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtomDeclaration {
    pub dimension: Option<String>,
    pub entity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeBehaviorDeclaration {
    pub axis: AtomDeclaration,
    pub rule: SelectionDeclaration,
    #[serde(default)]
    pub order_by: Vec<OrderDeclaration>,
    pub selection_scope: GrainDeclaration,
    pub ties: TiesDeclaration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionDeclaration {
    PeriodEnd,
    PeriodStart,
}

/// Ascending with nulls last unless stated (SQL's default for ascending).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderDeclaration {
    /// `dataset.field` of a column field.
    pub field: Option<String>,
    pub dimension: Option<String>,
    #[serde(default)]
    pub descending: bool,
    #[serde(default)]
    pub nulls_first: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum TiesDeclaration {
    /// The rows are unique by this entity key, so there are no ties.
    UniqueByKey(String),
    /// Ties are broken by these fields.
    Secondary(Vec<OrderDeclaration>),
    Unspecified,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregationDeclaration {
    pub class: ClassDeclaration,
    #[serde(default)]
    pub axes: Vec<AxisDeclaration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassDeclaration {
    FullyAdditive,
    SemiAdditive,
    NonAdditive,
}

/// How the metric behaves along one axis: exactly one of `dimension` and
/// `entity`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AxisDeclaration {
    pub dimension: Option<String>,
    pub entity: Option<String>,
    pub behavior: BehaviorDeclaration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorDeclaration {
    Additive,
    Recompute,
    DistinctRecompute,
    PeriodEnd,
    PeriodStart,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundingDeclaration {
    HalfEven,
    HalfUp,
    TowardZero,
    Floor,
    Ceiling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroDivisionDeclaration {
    Null,
    Error,
}

/// Our payload among `extensions`, if any.
///
/// # Errors
/// [`Code::InvalidExtension`] for a repeated entry, a payload that is not
/// the JSON object of this level, or a version other than 1.
pub(crate) fn read<T: DeserializeOwned + Default + Versioned>(
    extensions: &[CustomExtension],
    path: &str,
) -> Result<T, Diagnostic> {
    let mut ours = extensions
        .iter()
        .enumerate()
        .filter(|(_, extension)| extension.vendor_name == VENDOR);
    let Some((index, extension)) = ours.next() else {
        return Ok(T::default());
    };
    let base = format!("{path}/custom_extensions");
    if let Some((repeated, _)) = ours.next() {
        return Err(Diagnostic::error(
            Code::InvalidExtension,
            format!("{base}/{repeated}"),
            format!("La extensión {VENDOR:?} aparece más de una vez en este objeto."),
        ));
    }
    let path = format!("{base}/{index}/data");
    let payload: T = serde_json::from_str(&extension.data).map_err(|error| {
        Diagnostic::error(
            Code::InvalidExtension,
            path.clone(),
            format!("La extensión {VENDOR:?} no es válida aquí: {error}."),
        )
    })?;
    if payload.version() != VERSION {
        return Err(Diagnostic::error(
            Code::InvalidExtension,
            path,
            format!("La extensión {VENDOR:?} necesita \"version\": {VERSION}."),
        ));
    }
    Ok(payload)
}

pub(crate) trait Versioned {
    fn version(&self) -> u32;
}

macro_rules! versioned {
    ($($ty:ty),+) => {$(
        impl Versioned for $ty {
            fn version(&self) -> u32 {
                self.version
            }
        }
    )+};
}
versioned!(
    ModelExtension,
    DatasetExtension,
    FieldExtension,
    RelationshipExtension,
    MetricExtension
);
