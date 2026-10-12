//! Explicit Phase 1A semantic projections. These tags and fields are profile contracts.
//! No serialization derive, struct layout, or Debug spelling defines canonical bytes.

use crate::canonical::{Canonical, CanonicalValue as C};
use crate::expressions::{
    AggregateFunction, AggregateNullSemantics, BooleanOp, ComparisonOp, MetricExpr, MetricExprKind,
    ScalarExpr, ScalarExprKind, UnaryOp,
};
use crate::grain::{Grain, GrainAtom};
use crate::identity::{ObjectIdentity, SemanticVersion, SourceHandle, StableId};
use crate::model::{canonical_record, canonical_unit_enum};
use crate::time::{
    CalendarProfile, IanaZoneId, LocalTimeResolution, NullPlacement, OrderTerm, SortDirection,
    TimeBucket, TimeGrain, TimeRole, TimeSemantics, TimestampSemantics,
};
use crate::types::{
    ArithmeticOp, DecimalType, DecimalValue, FiniteFloat, Literal, NullProfile, NumericContract,
    RoundingMode, StringProfile, StructField, ValueType, ZeroDivisionPolicy,
};
use crate::units::Unit;

impl Canonical for StableId {
    fn canonical_value(&self) -> C {
        C::text(self.as_str())
    }
}
impl Canonical for SourceHandle {
    fn canonical_value(&self) -> C {
        C::text(self.as_str())
    }
}
impl Canonical for SemanticVersion {
    fn canonical_value(&self) -> C {
        C::text(self.as_str())
    }
}
canonical_record!(
    ObjectIdentity,
    "object-identity",
    stable_id,
    semantic_version
);

impl Canonical for DecimalType {
    fn canonical_value(&self) -> C {
        C::record(
            "value-type.decimal",
            [
                ("precision", self.precision().canonical_value()),
                ("scale", self.scale().canonical_value()),
            ],
        )
    }
}
impl Canonical for DecimalValue {
    fn canonical_value(&self) -> C {
        C::record(
            "csmir.Decimal",
            [
                ("coefficient", C::text(self.coefficient())),
                (
                    "precision",
                    self.decimal_type().precision().canonical_value(),
                ),
                ("scale", self.decimal_type().scale().canonical_value()),
            ],
        )
    }
}
canonical_record!(StructField, "struct-field", name, value_type, nullable);
impl Canonical for ValueType {
    fn canonical_value(&self) -> C {
        match self {
            Self::Boolean => empty("value-type.boolean"),
            Self::Integer { signed, bits } => C::record(
                "value-type.integer",
                [
                    ("signed", signed.canonical_value()),
                    ("bits", bits.canonical_value()),
                ],
            ),
            Self::Decimal(value) => value.canonical_value(),
            Self::Float { bits } => {
                C::record("value-type.float", [("bits", bits.canonical_value())])
            }
            Self::Utf8 => empty("value-type.utf8"),
            Self::Binary => empty("value-type.binary"),
            Self::Date => empty("value-type.date"),
            Self::Time => empty("value-type.time"),
            Self::Timestamp { timezone_semantics } => C::record(
                "value-type.timestamp",
                [("timezone_semantics", timezone_semantics.canonical_value())],
            ),
            Self::Duration => empty("value-type.duration"),
            Self::Uuid => empty("value-type.uuid"),
            Self::Identifier { entity_key } => C::record(
                "value-type.identifier",
                [("entity_key", entity_key.canonical_value())],
            ),
            Self::Struct(fields) => {
                C::record("value-type.struct", [("fields", C::sequence(fields))])
            }
            Self::List(element) => {
                C::record("value-type.list", [("element", element.canonical_value())])
            }
            Self::OpaqueExtension {
                namespace,
                type_name,
            } => C::record(
                "value-type.opaque-extension",
                [
                    ("namespace", namespace.canonical_value()),
                    ("type_name", type_name.canonical_value()),
                ],
            ),
        }
    }
}
impl Canonical for FiniteFloat {
    fn canonical_value(&self) -> C {
        C::Float(self.value())
    }
}
impl Canonical for Literal {
    fn canonical_value(&self) -> C {
        match self {
            Self::Null(value_type) => C::record(
                "literal.null",
                [("value_type", value_type.canonical_value())],
            ),
            Self::Boolean(value) => {
                C::record("literal.boolean", [("value", value.canonical_value())])
            }
            // A value that fits 64 bits keeps its original encoding, so no
            // existing fingerprint changes; only wider values use the
            // `-128` records, with their exact decimal text. Every value has
            // exactly one form.
            Self::SignedInteger(value) => match i64::try_from(*value) {
                Ok(narrow) => C::record(
                    "literal.signed-integer",
                    [("value", narrow.canonical_value())],
                ),
                Err(_) => C::record(
                    "literal.signed-integer-128",
                    [("value", C::text(value.to_string()))],
                ),
            },
            Self::UnsignedInteger(value) => match u64::try_from(*value) {
                Ok(narrow) => C::record(
                    "literal.unsigned-integer",
                    [("value", narrow.canonical_value())],
                ),
                Err(_) => C::record(
                    "literal.unsigned-integer-128",
                    [("value", C::text(value.to_string()))],
                ),
            },
            Self::Decimal(value) => {
                C::record("literal.decimal", [("value", value.canonical_value())])
            }
            Self::Float(value) => C::record("literal.float", [("value", value.canonical_value())]),
            Self::Utf8(value) => C::record("literal.utf8", [("value", value.canonical_value())]),
            Self::Binary(value) => {
                C::record("literal.binary", [("value", C::Bytes(value.clone()))])
            }
            Self::Date {
                days_since_unix_epoch,
            } => C::record(
                "literal.date",
                [(
                    "days_since_unix_epoch",
                    C::Signed(i64::from(*days_since_unix_epoch)),
                )],
            ),
            Self::Time {
                nanoseconds_since_midnight,
            } => C::record(
                "literal.time",
                [(
                    "nanoseconds_since_midnight",
                    nanoseconds_since_midnight.canonical_value(),
                )],
            ),
            Self::Timestamp {
                nanoseconds,
                timezone_semantics,
            } => C::record(
                "literal.timestamp",
                [
                    ("nanoseconds", nanoseconds.canonical_value()),
                    ("timezone_semantics", timezone_semantics.canonical_value()),
                ],
            ),
            Self::Duration { nanoseconds } => C::record(
                "literal.duration",
                [("nanoseconds", nanoseconds.canonical_value())],
            ),
            Self::Uuid(bytes) => C::record("literal.uuid", [("value", C::Bytes(bytes.to_vec()))]),
        }
    }
}
canonical_unit_enum!(StringProfile, "string-profile", Utf8Exact => "utf8-exact");
canonical_unit_enum!(NullProfile, "null-profile", SqlThreeValued => "sql-three-valued");
canonical_unit_enum!(RoundingMode, "rounding-mode", HalfEven => "half-even", HalfUp => "half-up", TowardZero => "toward-zero", Floor => "floor", Ceiling => "ceiling");
canonical_unit_enum!(ZeroDivisionPolicy, "zero-division-policy", Null => "null", Error => "error");
canonical_unit_enum!(ArithmeticOp, "arithmetic-op", Add => "add", Subtract => "subtract", Multiply => "multiply", Divide => "divide");
canonical_record!(
    NumericContract,
    "numeric-contract",
    rounding,
    zero_division,
    explicit_narrowing
);

impl Canonical for Unit {
    fn canonical_value(&self) -> C {
        match self {
            Self::Currency { iso4217 } => {
                C::record("unit.currency", [("iso4217", iso4217.canonical_value())])
            }
            Self::Physical { ucum } => {
                C::record("unit.physical", [("ucum", ucum.canonical_value())])
            }
            Self::Ratio => empty("unit.ratio"),
            Self::Percent => empty("unit.percent"),
            Self::Count => empty("unit.count"),
            Self::Custom { namespace, code } => C::record(
                "unit.custom",
                [
                    ("namespace", namespace.canonical_value()),
                    ("code", code.canonical_value()),
                ],
            ),
        }
    }
}
impl Canonical for Grain {
    fn canonical_value(&self) -> C {
        C::record("grain", [("atoms", C::set(self.atoms()))])
    }
}
impl Canonical for GrainAtom {
    fn canonical_value(&self) -> C {
        match self {
            Self::EntityKeyRef(key) => C::record(
                "grain-atom.entity-key",
                [("entity_key", key.canonical_value())],
            ),
            Self::DimensionRef { dimension, roles } => C::record(
                "grain-atom.dimension",
                [
                    ("dimension", dimension.canonical_value()),
                    ("roles", C::sequence(roles)),
                ],
            ),
            Self::TimeBucketRef(bucket) => C::record(
                "grain-atom.time-bucket",
                [("bucket", bucket.canonical_value())],
            ),
        }
    }
}
impl Canonical for IanaZoneId {
    fn canonical_value(&self) -> C {
        C::text(self.as_str())
    }
}
canonical_unit_enum!(LocalTimeResolution, "local-time-resolution", Reject => "reject", Earlier => "earlier", Later => "later", ShiftForward => "shift-forward");
impl Canonical for TimestampSemantics {
    fn canonical_value(&self) -> C {
        match self {
            Self::Instant => empty("timestamp-semantics.instant"),
            Self::LocalDateTime {
                source_timezone,
                local_time_resolution,
            } => C::record(
                "timestamp-semantics.local-date-time",
                [
                    ("source_timezone", source_timezone.canonical_value()),
                    (
                        "local_time_resolution",
                        local_time_resolution.canonical_value(),
                    ),
                ],
            ),
        }
    }
}
canonical_unit_enum!(TimeRole, "time-role", EventTime => "event-time", SnapshotTime => "snapshot-time", ValidityStart => "validity-start", ValidityEnd => "validity-end", ProcessingTime => "processing-time");
impl Canonical for CalendarProfile {
    fn canonical_value(&self) -> C {
        match self {
            Self::Gregorian => empty("calendar.gregorian"),
            Self::IsoWeek => empty("calendar.iso-week"),
            Self::Custom { id } => C::record("calendar.custom", [("id", id.canonical_value())]),
            Self::Fiscal { id } => C::record("calendar.fiscal", [("id", id.canonical_value())]),
        }
    }
}
canonical_unit_enum!(TimeGrain, "time-grain", Second => "second", Minute => "minute", Hour => "hour", Day => "day", Week => "week", Month => "month", Quarter => "quarter", Year => "year");
canonical_record!(TimeBucket, "time-bucket", source, timezone, calendar, grain);
canonical_record!(
    TimeSemantics,
    "time-semantics",
    role,
    timezone,
    calendar,
    pinned_tzdb_version
);
canonical_unit_enum!(SortDirection, "sort-direction", Ascending => "ascending", Descending => "descending");
canonical_unit_enum!(NullPlacement, "null-placement", First => "first", Last => "last");
canonical_record!(OrderTerm, "order-term", value, direction, nulls);

canonical_unit_enum!(UnaryOp, "unary-op", Negate => "negate", Not => "not");
canonical_unit_enum!(ComparisonOp, "comparison-op", Equal => "equal", NotEqual => "not-equal", Less => "less", LessOrEqual => "less-or-equal", Greater => "greater", GreaterOrEqual => "greater-or-equal");
canonical_unit_enum!(BooleanOp, "boolean-op", And => "and", Or => "or");
canonical_unit_enum!(AggregateFunction, "aggregate-function", Sum => "sum", CountRows => "count-rows", CountNonNull => "count-non-null", CountDistinct => "count-distinct", Min => "min", Max => "max");
canonical_unit_enum!(AggregateNullSemantics, "aggregate-null-semantics", IgnoreNullsNullIfEmpty => "ignore-nulls-null-if-empty", CountNonNull => "count-non-null", CountRows => "count-rows");
canonical_record!(ScalarExpr, "scalar-expression", kind, value_type, unit);
fn canonical_case_branch((condition, value): &(ScalarExpr, ScalarExpr)) -> C {
    C::record(
        "case-branch",
        [
            ("condition", condition.canonical_value()),
            ("value", value.canonical_value()),
        ],
    )
}

impl Canonical for ScalarExprKind {
    fn canonical_value(&self) -> C {
        match self {
            Self::Literal(value) => {
                C::record("scalar.literal", [("value", value.canonical_value())])
            }
            Self::FieldRef(field) => {
                C::record("scalar.field-ref", [("field", field.canonical_value())])
            }
            Self::DimensionRef(dimension) => C::record(
                "scalar.dimension-ref",
                [("dimension", dimension.canonical_value())],
            ),
            Self::ContextSlot(name) => {
                C::record("scalar.context-slot", [("name", name.canonical_value())])
            }
            Self::Unary { op, operand } => C::record(
                "scalar.unary",
                [
                    ("op", op.canonical_value()),
                    ("operand", operand.canonical_value()),
                ],
            ),
            Self::Arithmetic {
                op,
                left,
                right,
                contract,
            } => C::record(
                "scalar.arithmetic",
                [
                    ("op", op.canonical_value()),
                    ("left", left.canonical_value()),
                    ("right", right.canonical_value()),
                    ("contract", contract.canonical_value()),
                ],
            ),
            Self::Comparison { op, left, right } => C::record(
                "scalar.comparison",
                [
                    ("op", op.canonical_value()),
                    ("left", left.canonical_value()),
                    ("right", right.canonical_value()),
                ],
            ),
            Self::BooleanLogic { op, operands } => C::record(
                "scalar.boolean-logic",
                [
                    ("op", op.canonical_value()),
                    ("operands", C::sequence(operands)),
                ],
            ),
            Self::Case {
                branches,
                otherwise,
            } => C::record(
                "scalar.case",
                [
                    (
                        "branches",
                        C::Sequence(branches.iter().map(canonical_case_branch).collect()),
                    ),
                    ("otherwise", otherwise.canonical_value()),
                ],
            ),
            Self::NullTest { operand, negated } => C::record(
                "scalar.null-test",
                [
                    ("operand", operand.canonical_value()),
                    ("negated", negated.canonical_value()),
                ],
            ),
            Self::ExplicitCast { operand, rounding } => C::record(
                "scalar.explicit-cast",
                [
                    ("operand", operand.canonical_value()),
                    ("rounding", rounding.canonical_value()),
                ],
            ),
            Self::TemporalDerivation { operand, bucket } => C::record(
                "scalar.temporal-derivation",
                [
                    ("operand", operand.canonical_value()),
                    ("bucket", bucket.canonical_value()),
                ],
            ),
            Self::UnitConversion {
                operand,
                conversion,
            } => C::record(
                "scalar.unit-conversion",
                [
                    ("operand", operand.canonical_value()),
                    ("conversion", conversion.canonical_value()),
                ],
            ),
        }
    }
}
canonical_record!(MetricExpr, "metric-expression", kind, value_type, unit);
impl Canonical for MetricExprKind {
    fn canonical_value(&self) -> C {
        match self {
            Self::MeasureRef(measure) => C::record(
                "metric-expr.measure-ref",
                [("measure", measure.canonical_value())],
            ),
            Self::MetricRef(metric) => C::record(
                "metric-expr.metric-ref",
                [("metric", metric.canonical_value())],
            ),
            Self::Arithmetic {
                op,
                left,
                right,
                contract,
            } => C::record(
                "metric-expr.arithmetic",
                [
                    ("op", op.canonical_value()),
                    ("left", left.canonical_value()),
                    ("right", right.canonical_value()),
                    ("contract", contract.canonical_value()),
                ],
            ),
            Self::Ratio {
                numerator,
                denominator,
                zero_denominator,
                rounding,
            } => C::record(
                "metric-expr.ratio",
                [
                    ("numerator", numerator.canonical_value()),
                    ("denominator", denominator.canonical_value()),
                    ("zero_denominator", zero_denominator.canonical_value()),
                    ("rounding", rounding.canonical_value()),
                ],
            ),
            Self::Window { base_metric } => C::record(
                "metric-expr.window",
                [("base_metric", base_metric.canonical_value())],
            ),
            Self::ExplicitCast { operand, rounding } => C::record(
                "metric-expr.explicit-cast",
                [
                    ("operand", operand.canonical_value()),
                    ("rounding", rounding.canonical_value()),
                ],
            ),
        }
    }
}

fn empty(tag: &str) -> C {
    C::record(tag, [] as [(&str, C); 0])
}
