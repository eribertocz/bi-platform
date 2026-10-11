use crate::diagnostic::{Diagnostic, ErrorCode};
use crate::identity::StableId;
use crate::time::TimestampSemantics;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct DecimalType {
    precision: u8,
    scale: u8,
}

impl DecimalType {
    pub fn new(precision: u8, scale: u8) -> Result<Self, Diagnostic> {
        if !(1..=76).contains(&precision) || scale > precision {
            return Err(type_error(
                "Decimal requires precision 1..76 and scale 0..precision",
            ));
        }
        Ok(Self { precision, scale })
    }
    pub const fn precision(self) -> u8 {
        self.precision
    }
    pub const fn scale(self) -> u8 {
        self.scale
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct DecimalValue {
    coefficient: String,
    decimal_type: DecimalType,
}

impl DecimalValue {
    /// The exact value is coefficient * 10^(-scale). No binary float is involved.
    pub fn new(
        coefficient: impl Into<String>,
        decimal_type: DecimalType,
    ) -> Result<Self, Diagnostic> {
        let coefficient = coefficient.into();
        let (negative, digits) = if let Some(rest) = coefficient.strip_prefix('-') {
            (true, rest)
        } else {
            (false, coefficient.strip_prefix('+').unwrap_or(&coefficient))
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(type_error(
                "Decimal coefficient must be signed ASCII integer digits",
            ));
        }
        let significant = digits.trim_start_matches('0');
        if significant.len() > usize::from(decimal_type.precision()) {
            return Err(Diagnostic::error(
                ErrorCode::NumericOverflow,
                None,
                "Decimal coefficient exceeds its declared precision",
            ));
        }
        let coefficient = if significant.is_empty() {
            "0".to_owned()
        } else if negative {
            format!("-{significant}")
        } else {
            significant.to_owned()
        };
        Ok(Self {
            coefficient,
            decimal_type,
        })
    }
    pub fn coefficient(&self) -> &str {
        &self.coefficient
    }
    pub const fn decimal_type(&self) -> DecimalType {
        self.decimal_type
    }
    pub fn is_negative(&self) -> bool {
        self.coefficient.starts_with('-')
    }
    pub fn is_zero(&self) -> bool {
        self.coefficient == "0"
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct StructField {
    pub name: String,
    pub value_type: ValueType,
    pub nullable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum ValueType {
    Boolean,
    Integer {
        signed: bool,
        bits: Option<u16>,
    },
    Decimal(DecimalType),
    Float {
        bits: u16,
    },
    Utf8,
    Binary,
    Date,
    Time,
    Timestamp {
        timezone_semantics: TimestampSemantics,
    },
    Duration,
    Uuid,
    Identifier {
        entity_key: StableId,
    },
    Struct(Vec<StructField>),
    List(Box<ValueType>),
    OpaqueExtension {
        namespace: String,
        type_name: String,
    },
}

impl ValueType {
    pub fn validate(&self) -> Result<(), Diagnostic> {
        self.validate_depth(0)
    }
    fn validate_depth(&self, depth: usize) -> Result<(), Diagnostic> {
        if depth > 64 {
            return Err(type_error(
                "Type nesting exceeds the bounded compiler depth",
            ));
        }
        match self {
            Self::Integer {
                bits: Some(bits), ..
            } if ![8, 16, 32, 64, 128].contains(bits) => {
                Err(type_error("Unsupported integer bit width"))
            }
            Self::Float { bits } if ![32, 64].contains(bits) => {
                Err(type_error("Float bit width must be 32 or 64"))
            }
            Self::Struct(fields) => {
                let mut names = BTreeSet::new();
                for field in fields {
                    if field.name.is_empty() || !names.insert(&field.name) {
                        return Err(type_error("Struct fields require distinct nonempty names"));
                    }
                    field.value_type.validate_depth(depth + 1)?;
                }
                Ok(())
            }
            Self::List(element) => element.validate_depth(depth + 1),
            Self::OpaqueExtension {
                namespace,
                type_name,
            } if namespace.is_empty() || type_name.is_empty() => {
                Err(type_error("Opaque types require namespace and type name"))
            }
            _ => Ok(()),
        }
    }
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Self::Integer { .. } | Self::Decimal(_) | Self::Float { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FiniteFloat(u64);

impl FiniteFloat {
    pub fn new(value: f64) -> Result<Self, Diagnostic> {
        if !value.is_finite() {
            return Err(type_error("Certified floating values must be finite"));
        }
        Ok(Self(value.to_bits()))
    }
    pub fn value(self) -> f64 {
        f64::from_bits(self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Literal {
    Null(ValueType),
    Boolean(bool),
    SignedInteger(i64),
    UnsignedInteger(u64),
    Decimal(DecimalValue),
    Float(FiniteFloat),
    Utf8(String),
    Binary(Vec<u8>),
    Date {
        days_since_unix_epoch: i32,
    },
    Time {
        nanoseconds_since_midnight: u64,
    },
    Timestamp {
        nanoseconds: i64,
        timezone_semantics: TimestampSemantics,
    },
    Duration {
        nanoseconds: i64,
    },
    Uuid([u8; 16]),
}

impl Literal {
    pub fn value_type(&self) -> ValueType {
        match self {
            Self::Null(t) => t.clone(),
            Self::Boolean(_) => ValueType::Boolean,
            Self::SignedInteger(_) => ValueType::Integer {
                signed: true,
                bits: Some(64),
            },
            Self::UnsignedInteger(_) => ValueType::Integer {
                signed: false,
                bits: Some(64),
            },
            Self::Decimal(value) => ValueType::Decimal(value.decimal_type()),
            Self::Float(_) => ValueType::Float { bits: 64 },
            Self::Utf8(_) => ValueType::Utf8,
            Self::Binary(_) => ValueType::Binary,
            Self::Date { .. } => ValueType::Date,
            Self::Time { .. } => ValueType::Time,
            Self::Timestamp {
                timezone_semantics, ..
            } => ValueType::Timestamp {
                timezone_semantics: timezone_semantics.clone(),
            },
            Self::Duration { .. } => ValueType::Duration,
            Self::Uuid(_) => ValueType::Uuid,
        }
    }
    pub fn validate(&self) -> Result<(), Diagnostic> {
        self.value_type().validate()?;
        if matches!(self, Self::Time {nanoseconds_since_midnight} if *nanoseconds_since_midnight >= 86_400_000_000_000)
        {
            return Err(type_error("Time literal must fall within a civil day"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringProfile {
    Utf8Exact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NullProfile {
    SqlThreeValued,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TruthValue {
    True,
    False,
    Unknown,
}

impl TruthValue {
    pub fn and(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }
    pub fn or(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
}

impl std::ops::Not for TruthValue {
    type Output = Self;
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoundingMode {
    HalfEven,
    HalfUp,
    TowardZero,
    Floor,
    Ceiling,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ZeroDivisionPolicy {
    Null,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArithmeticOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NumericContract {
    pub rounding: Option<RoundingMode>,
    pub zero_division: Option<ZeroDivisionPolicy>,
    pub explicit_narrowing: bool,
}

pub fn derived_decimal(
    op: ArithmeticOp,
    left: DecimalType,
    right: DecimalType,
) -> Result<(u16, u16), Diagnostic> {
    let (p1, s1, p2, s2) = (
        u16::from(left.precision()),
        u16::from(left.scale()),
        u16::from(right.precision()),
        u16::from(right.scale()),
    );
    match op {
        ArithmeticOp::Add | ArithmeticOp::Subtract => {
            let s = s1.max(s2);
            Ok(((p1 - s1).max(p2 - s2) + 1 + s, s))
        }
        ArithmeticOp::Multiply => Ok((p1 + p2 + 1, s1 + s2)),
        ArithmeticOp::Divide => Err(type_error(
            "Exact division requires an explicit target decimal type",
        )),
    }
}

pub fn validate_arithmetic(
    op: ArithmeticOp,
    left: &ValueType,
    right: &ValueType,
    result: &ValueType,
    contract: &NumericContract,
) -> Result<(), Diagnostic> {
    left.validate()?;
    right.validate()?;
    result.validate()?;
    if let (ValueType::Decimal(l), ValueType::Decimal(r), ValueType::Decimal(t)) =
        (left, right, result)
    {
        if op == ArithmeticOp::Divide {
            if contract.zero_division.is_none() || contract.rounding.is_none() {
                return Err(type_error(
                    "Exact division requires zero division and rounding policies",
                ));
            }
            return Ok(());
        }
        let (precision, scale) = derived_decimal(op, *l, *r)?;
        let target_precision = u16::from(t.precision());
        let target_scale = u16::from(t.scale());
        if precision == target_precision && scale == target_scale {
            return Ok(());
        }
        if !contract.explicit_narrowing {
            return Err(Diagnostic::error(
                if precision > 76 {
                    ErrorCode::NumericOverflow
                } else {
                    ErrorCode::IncompatibleTypes
                },
                None,
                "Arithmetic result requires the derived type or an explicit narrowing contract",
            ));
        }
        if target_scale < scale && contract.rounding.is_none() {
            return Err(type_error(
                "Scale reduction requires an explicit rounding mode",
            ));
        }
        return Ok(());
    }
    if left != right || left != result || !left.is_numeric() {
        return Err(type_error("Numeric promotion requires explicit casts"));
    }
    if op == ArithmeticOp::Divide && contract.zero_division.is_none() {
        return Err(type_error(
            "Division requires an explicit zero division policy",
        ));
    }
    Ok(())
}

fn type_error(message: &str) -> Diagnostic {
    Diagnostic::error(ErrorCode::IncompatibleTypes, None, message)
}
