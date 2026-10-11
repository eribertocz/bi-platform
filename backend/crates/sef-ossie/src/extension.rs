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
