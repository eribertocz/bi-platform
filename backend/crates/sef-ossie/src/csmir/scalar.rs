//! Row-level `OSSIE_SQL_2026` → CSMIR [`ScalarExpr`], with exact types.
//!
//! Only what CSMIR can state exactly is translated; everything else the
//! language allows is rejected with `not_representable`, never approximated.
//! Some constructs are rewritten into exact equivalents under SQL's
//! three-valued logic: `x BETWEEN a AND b` is `x >= a AND x <= b`,
//! `x IN (a, b)` is `x = a OR x = b`, and `COALESCE`, `IFNULL`/`NVL`,
//! `ZEROIFNULL`, `NULLIF` and `IF`/`IFF` are `CASE` expressions.
//!
//! CSMIR refuses implicit numeric promotion. The converter adds only casts
//! that cannot lose information: a numeric literal takes the type of the
//! value it is compared with or combined with, and an integer operand of an
//! exact division becomes `DECIMAL(20,0)`, which holds every 64-bit integer.

use sef_model_ir::expressions::{BooleanOp, ComparisonOp, ScalarExpr, ScalarExprKind, UnaryOp};
use sef_model_ir::functions::ScalarFunction;
use sef_model_ir::time::{CalendarProfile, IanaZoneId, TimeBucket, TimeGrain};
use sef_model_ir::types::{
    ArithmeticOp, DecimalType, DecimalValue, FiniteFloat, Literal, NumericContract, RoundingMode,
    ValueType, ZeroDivisionPolicy, derived_decimal,
};
use sqlparser::ast::{
    BinaryOperator, CastKind, CeilFloorKind, DataType, DateTimeField, ExactNumberInfo, Expr,
    Function, FunctionArg, FunctionArgExpr, FunctionArguments, Ident, UnaryOperator, Value,
};

use crate::diagnostic::Code;

mod functions;

/// Why an expression has no CSMIR translation; the caller adds the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    pub code: Code,
    pub message: String,
    /// Where the problem is, when not at the expression being translated.
    pub path: Option<String>,
}

impl Failure {
    pub(crate) fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path: None,
        }
    }

    pub(crate) fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

pub(crate) fn not_representable(what: &str) -> Failure {
    Failure::new(
        Code::NotRepresentable,
        format!(
            "{what}: OSSIE_SQL_2026 lo admite, pero el motor semántico aún no lo calcula de forma exacta."
        ),
    )
}

fn type_mismatch(message: impl Into<String>) -> Failure {
    Failure::new(Code::TypeMismatch, message)
}

/// Resolves the names an expression reads.
pub(crate) trait Names {
    /// `name` or `qualifier.name`, normalized, as a typed scalar.
    fn reference(&mut self, qualifier: Option<&str>, name: &str) -> Result<ScalarExpr, Failure>;
    /// The zone `DATE_TRUNC` buckets in, when the expression may bucket time.
    fn timezone(&self) -> Option<&IanaZoneId>;
    /// A request context value (`:name`), where the expression admits them.
    fn context(&mut self, name: &str) -> Result<ScalarExpr, Failure> {
        Err(not_representable(&format!(
            "El parámetro `:{name}` (solo las políticas leen valores de la petición)"
        )))
    }
}

/// Exact-division policy for `CAST(a / b AS DECIMAL(p, s))`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Division {
    pub rounding: RoundingMode,
    pub zero: ZeroDivisionPolicy,
}

impl Default for Division {
    fn default() -> Self {
        Self {
            rounding: RoundingMode::HalfEven,
            zero: ZeroDivisionPolicy::Null,
        }
    }
}

pub(crate) struct Translator<'n, N: Names> {
    pub names: &'n mut N,
    pub division: Division,
}

/// A literal whose type depends on its neighbour (`amount > 100`).
enum Typed {
    Expr(ScalarExpr),
    Number(String),
    Null,
}

impl<N: Names> Translator<'_, N> {
    pub(crate) fn translate(&mut self, expr: &Expr) -> Result<ScalarExpr, Failure> {
        match self.typed(expr)? {
            Typed::Expr(scalar) => Ok(scalar),
            Typed::Number(text) => number(&text, None, false),
            Typed::Null => Err(type_mismatch(
                "Un NULL suelto no tiene tipo: úsalo en un CASE o con CAST(NULL AS …).",
            )),
        }
    }

    /// Translates `expr` with the type of `like` for bare literals (see
    /// [`number`] for `exact`).
    fn translate_like(
        &mut self,
        expr: &Expr,
        like: &ValueType,
        exact: bool,
    ) -> Result<ScalarExpr, Failure> {
        match self.typed(expr)? {
            Typed::Expr(scalar) => Ok(scalar),
            Typed::Number(text) => number(&text, Some(like), exact),
            Typed::Null => Ok(literal(Literal::Null(like.clone()))),
        }
    }

    /// Two operands; a bare literal takes the other operand's type: its
    /// exact type when compared (`exact`), its kind in arithmetic.
    fn pair(
        &mut self,
        left: &Expr,
        right: &Expr,
        exact: bool,
    ) -> Result<(ScalarExpr, ScalarExpr), Failure> {
        match (self.typed(left)?, self.typed(right)?) {
            (Typed::Expr(l), Typed::Expr(r)) => Ok((l, r)),
            (Typed::Expr(l), _) => {
                let r = self.translate_like(right, &l.value_type, exact)?;
                Ok((l, r))
            }
            (_, Typed::Expr(r)) => {
                let l = self.translate_like(left, &r.value_type, exact)?;
                Ok((l, r))
            }
            (Typed::Number(l), Typed::Number(r)) => {
                Ok((number(&l, None, false)?, number(&r, None, false)?))
            }
            _ => Err(type_mismatch(
                "Una comparación o una operación entre literales sin tipo; añade CAST.",
            )),
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per translated construct reads as the allowlist"
    )]
    fn typed(&mut self, expr: &Expr) -> Result<Typed, Failure> {
        let scalar = match expr {
            Expr::Nested(inner) => return self.typed(inner),
            Expr::Value(value) => match &value.value {
                Value::Number(text, _) => return Ok(Typed::Number(text.clone())),
                Value::Null => return Ok(Typed::Null),
                Value::SingleQuotedString(text) => literal(Literal::Utf8(text.clone())),
                Value::Boolean(value) => literal(Literal::Boolean(*value)),
                Value::Placeholder(name) => match name.strip_prefix(':') {
                    Some(name) if !name.is_empty() => self.names.context(name)?,
                    _ => return Err(not_representable("Un parámetro posicional (`?`, `$1`)")),
                },
                _ => return Err(not_representable("Ese literal")),
            },
            Expr::TypedString(typed) => match (&typed.data_type, &typed.value.value) {
                (DataType::Date, Value::SingleQuotedString(text)) => literal(Literal::Date {
                    days_since_unix_epoch: date(text)?,
                }),
                _ => return Err(not_representable("Un literal de hora o fecha y hora")),
            },
            Expr::Identifier(ident) => self.names.reference(None, &normalize(ident))?,
            Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [qualifier, name] => self
                    .names
                    .reference(Some(&normalize(qualifier)), &normalize(name))?,
                _ => return Err(not_representable("Una referencia de más de dos partes")),
            },
            Expr::UnaryOp { op, expr: inner } => match op {
                UnaryOperator::Plus => return self.typed(inner),
                UnaryOperator::Minus => {
                    if let Typed::Number(text) = self.typed(inner)? {
                        return Ok(Typed::Number(format!("-{text}")));
                    }
                    let operand = self.translate(inner)?;
                    if !operand.value_type.is_numeric() {
                        return Err(type_mismatch("El signo menos necesita un número."));
                    }
                    unary(UnaryOp::Negate, operand)
                }
                UnaryOperator::Not => {
                    let operand = self.boolean(inner)?;
                    unary(UnaryOp::Not, operand)
                }
                _ => return Err(not_representable(&format!("El operador `{op}`"))),
            },
            Expr::IsNull(inner) | Expr::IsNotNull(inner) => {
                let operand = self.translate(inner)?;
                ScalarExpr {
                    kind: ScalarExprKind::NullTest {
                        operand: Box::new(operand),
                        negated: matches!(expr, Expr::IsNotNull(_)),
                    },
                    value_type: ValueType::Boolean,
                    unit: None,
                }
            }
            Expr::BinaryOp { left, op, right } => self.binary(left, op, right)?,
            Expr::Between {
                expr: value,
                negated,
                low,
                high,
            } => {
                let low = self.comparison(value, ComparisonOp::GreaterOrEqual, low)?;
                let high = self.comparison(value, ComparisonOp::LessOrEqual, high)?;
                let between = boolean(BooleanOp::And, vec![low, high]);
                if *negated {
                    unary(UnaryOp::Not, between)
                } else {
                    between
                }
            }
            Expr::InList {
                expr: value,
                list,
                negated,
            } => {
                let (op, join) = if *negated {
                    (ComparisonOp::NotEqual, BooleanOp::And)
                } else {
                    (ComparisonOp::Equal, BooleanOp::Or)
                };
                let terms = list
                    .iter()
                    .map(|item| self.comparison(value, op, item))
                    .collect::<Result<Vec<_>, _>>()?;
                match <[ScalarExpr; 1]>::try_from(terms) {
                    Ok([single]) => single,
                    Err(terms) => boolean(join, terms),
                }
            }
            Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                let mut branches = Vec::with_capacity(conditions.len());
                for when in conditions {
                    let condition = match operand {
                        Some(operand) => {
                            self.comparison(operand, ComparisonOp::Equal, &when.condition)?
                        }
                        None => self.boolean(&when.condition)?,
                    };
                    branches.push((condition, &when.result));
                }
                let results: Vec<&Expr> = branches
                    .iter()
                    .map(|(_, result)| *result)
                    .chain(else_result.as_deref())
                    .collect();
                self.case(
                    branches
                        .into_iter()
                        .map(|(condition, _)| condition)
                        .collect(),
                    &results,
                    else_result.is_some(),
                )?
            }
            Expr::Cast {
                kind: CastKind::Cast | CastKind::TryCast,
                expr: inner,
                data_type,
                format: None,
                ..
            } => {
                if matches!(
                    expr,
                    Expr::Cast {
                        kind: CastKind::TryCast,
                        ..
                    }
                ) {
                    return Err(not_representable("TRY_CAST"));
                }
                let target = cast_target(data_type)?;
                self.cast(inner, target)?
            }
            Expr::Like {
                negated,
                expr: value,
                pattern,
                any: false,
                escape_char: None,
            } => self.like(value, pattern, false, *negated)?,
            Expr::ILike {
                negated,
                expr: value,
                pattern,
                any: false,
                escape_char: None,
            } => self.like(value, pattern, true, *negated)?,
            Expr::Ceil {
                expr: value,
                field: CeilFloorKind::DateTimeField(DateTimeField::NoDateTime),
            } => self.call(ScalarFunction::Ceil, &[value])?,
            Expr::Floor {
                expr: value,
                field: CeilFloorKind::DateTimeField(DateTimeField::NoDateTime),
            } => self.call(ScalarFunction::Floor, &[value])?,
            Expr::Position { expr: needle, r#in } => {
                self.call(ScalarFunction::Position, &[needle, r#in])?
            }
            Expr::Substring {
                expr: value,
                substring_from: Some(start),
                substring_for: Some(length),
                shorthand: false,
                ..
            } => self.call(ScalarFunction::Substring, &[value, start, length])?,
            Expr::Trim {
                expr: value,
                trim_where,
                trim_what,
                trim_characters: None,
            } => self.trim(value, trim_where.as_ref(), trim_what.as_deref())?,
            Expr::Extract {
                field, expr: value, ..
            } => {
                let part = functions::part_named(&field.to_string().to_uppercase())?;
                self.extract(part, value)?
            }
            Expr::IsDistinctFrom(left, right) => self.distinct(left, right, true)?,
            Expr::IsNotDistinctFrom(left, right) => self.distinct(left, right, false)?,
            Expr::Function(function) => self.function(function)?,
            other => return Err(not_representable(&format!("`{other}`"))),
        };
        Ok(Typed::Expr(scalar))
    }

    fn boolean(&mut self, expr: &Expr) -> Result<ScalarExpr, Failure> {
        let scalar = self.translate(expr)?;
        if scalar.value_type == ValueType::Boolean {
            Ok(scalar)
        } else {
            Err(type_mismatch(
                "Aquí se espera una condición (verdadero o falso).",
            ))
        }
    }

    fn comparison(
        &mut self,
        left: &Expr,
        op: ComparisonOp,
        right: &Expr,
    ) -> Result<ScalarExpr, Failure> {
        let (left, right) = self.pair(left, right, true)?;
        let (left, right) = exact_literal(left, right)?;
        if left.value_type != right.value_type {
            return Err(type_mismatch(format!(
                "Se comparan valores de tipos distintos ({} y {}); añade CAST.",
                describe(&left.value_type),
                describe(&right.value_type)
            )));
        }
        Ok(ScalarExpr {
            kind: ScalarExprKind::Comparison {
                op,
                left: Box::new(left),
                right: Box::new(right),
            },
            value_type: ValueType::Boolean,
            unit: None,
        })
    }

    fn binary(
        &mut self,
        left: &Expr,
        op: &BinaryOperator,
        right: &Expr,
    ) -> Result<ScalarExpr, Failure> {
        let comparison = match op {
            BinaryOperator::Eq => Some(ComparisonOp::Equal),
            BinaryOperator::NotEq => Some(ComparisonOp::NotEqual),
            BinaryOperator::Lt => Some(ComparisonOp::Less),
            BinaryOperator::LtEq => Some(ComparisonOp::LessOrEqual),
            BinaryOperator::Gt => Some(ComparisonOp::Greater),
            BinaryOperator::GtEq => Some(ComparisonOp::GreaterOrEqual),
            _ => None,
        };
        if let Some(op) = comparison {
            return self.comparison(left, op, right);
        }
        if let BinaryOperator::And | BinaryOperator::Or = op {
            let join = if *op == BinaryOperator::And {
                BooleanOp::And
            } else {
                BooleanOp::Or
            };
            let operands = vec![self.boolean(left)?, self.boolean(right)?];
            return Ok(boolean(join, operands));
        }
        match op {
            BinaryOperator::StringConcat => {
                return self.call(ScalarFunction::Concat, &[left, right]);
            }
            BinaryOperator::Modulo => {
                let function = ScalarFunction::Mod {
                    zero: self.division.zero,
                };
                return self.call(function, &[left, right]);
            }
            _ => {}
        }
        let op = match op {
            BinaryOperator::Plus => ArithmeticOp::Add,
            BinaryOperator::Minus => ArithmeticOp::Subtract,
            BinaryOperator::Multiply => ArithmeticOp::Multiply,
            BinaryOperator::Divide => {
                return Err(type_mismatch(
                    "Una división necesita el tipo exacto de su resultado: escribe \
                     CAST(a / b AS DECIMAL(p, s)).",
                ));
            }
            other => return Err(not_representable(&format!("El operador `{other}`"))),
        };
        let (left, right) = self.pair(left, right, false)?;
        arithmetic(op, left, right)
    }

    /// `CASE`: every result shares one type; `NULL` results take it.
    fn case(
        &mut self,
        conditions: Vec<ScalarExpr>,
        results: &[&Expr],
        has_else: bool,
    ) -> Result<ScalarExpr, Failure> {
        let mut typed = Vec::with_capacity(results.len());
        let mut first = None;
        for result in results {
            let value = self.typed(result)?;
            if let (None, Typed::Expr(scalar)) = (&first, &value) {
                first = Some(scalar.value_type.clone());
            }
            typed.push(value);
        }
        let Some(value_type) = first else {
            return Err(type_mismatch(
                "Todas las ramas del CASE son literales sin tipo; añade CAST a una de ellas.",
            ));
        };
        let mut values = typed
            .into_iter()
            .map(|value| match value {
                Typed::Expr(scalar) => Ok(scalar),
                Typed::Number(text) => number(&text, Some(&value_type), true),
                Typed::Null => Ok(literal(Literal::Null(value_type.clone()))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(other) = values.iter().find(|value| value.value_type != value_type) {
            return Err(type_mismatch(format!(
                "Las ramas del CASE devuelven tipos distintos ({} y {}); añade CAST.",
                describe(&value_type),
                describe(&other.value_type)
            )));
        }
        let otherwise = if has_else { values.pop() } else { None }
            .unwrap_or_else(|| literal(Literal::Null(value_type.clone())));
        let unit = values.first().and_then(|value| value.unit.clone());
        Ok(ScalarExpr {
            kind: ScalarExprKind::Case {
                branches: conditions.into_iter().zip(values).collect(),
                otherwise: Box::new(otherwise),
            },
            value_type,
            unit,
        })
    }

    fn cast(&mut self, inner: &Expr, target: ValueType) -> Result<ScalarExpr, Failure> {
        if let Some((numerator, denominator)) = division(inner) {
            return self.divide(numerator, denominator, target);
        }
        let operand = match self.typed(inner)? {
            Typed::Expr(scalar) => scalar,
            // A numeric target types the literal exactly; any other target
            // converts the literal's own value (`CAST(123 AS VARCHAR)`).
            Typed::Number(text) if target.is_numeric() => {
                return number(&text, Some(&target), true);
            }
            Typed::Number(text) => number(&text, None, false)?,
            Typed::Null => return Ok(literal(Literal::Null(target))),
        };
        Ok(cast(operand, target))
    }

    /// `CAST(a / b AS T)`: an exact decimal division rounded to `T`, or a
    /// floating-point one.
    fn divide(
        &mut self,
        numerator: &Expr,
        denominator: &Expr,
        target: ValueType,
    ) -> Result<ScalarExpr, Failure> {
        let (left, right) = self.pair(numerator, denominator, false)?;
        let contract = match &target {
            ValueType::Decimal(_) => NumericContract {
                rounding: Some(self.division.rounding),
                zero_division: Some(self.division.zero),
                explicit_narrowing: false,
            },
            ValueType::Float { .. } => NumericContract {
                rounding: None,
                zero_division: Some(self.division.zero),
                explicit_narrowing: false,
            },
            _ => {
                return Err(type_mismatch(
                    "Una división da DECIMAL(p, s) o un número de coma flotante.",
                ));
            }
        };
        let (left, right) = (
            exact_operand(left, &target)?,
            exact_operand(right, &target)?,
        );
        Ok(ScalarExpr {
            kind: ScalarExprKind::Arithmetic {
                op: ArithmeticOp::Divide,
                left: Box::new(left),
                right: Box::new(right),
                contract,
            },
            value_type: target,
            unit: None,
        })
    }

    fn function(&mut self, function: &Function) -> Result<ScalarExpr, Failure> {
        let name = function_name(function);
        if function.over.is_some() {
            return Err(not_representable("Una ventana en un valor por fila"));
        }
        let args = arguments(function)?;
        match (name.as_str(), args.as_slice()) {
            ("DATE_TRUNC", [unit, value]) => self.date_trunc(unit, value),
            ("COALESCE", [_, _, ..]) | ("IFNULL" | "NVL", [_, _]) => self.coalesce(&args),
            ("ZEROIFNULL", [value]) => {
                let value = self.translate(value)?;
                let zero = number("0", Some(&value.value_type), true)?;
                Ok(coalesce(value, vec![zero]))
            }
            ("NULLIF", [left, right]) => {
                let (value, other) = self.pair(left, right, true)?;
                let (value, other) = exact_literal(value, other)?;
                let condition = ScalarExpr {
                    kind: ScalarExprKind::Comparison {
                        op: ComparisonOp::Equal,
                        left: Box::new(value.clone()),
                        right: Box::new(other),
                    },
                    value_type: ValueType::Boolean,
                    unit: None,
                };
                let null = literal(Literal::Null(value.value_type.clone()));
                Ok(ScalarExpr {
                    value_type: value.value_type.clone(),
                    unit: value.unit.clone(),
                    kind: ScalarExprKind::Case {
                        branches: vec![(condition, null)],
                        otherwise: Box::new(value),
                    },
                })
            }
            ("IF" | "IFF", [condition, then, otherwise]) => {
                let condition = self.boolean(condition)?;
                self.case(vec![condition], &[*then, *otherwise], true)
            }
            _ => self
                .scalar_function(&name, &args)
                .unwrap_or_else(|| Err(not_representable(&format!("La función `{name}`")))),
        }
    }

    fn coalesce(&mut self, args: &[&Expr]) -> Result<ScalarExpr, Failure> {
        let mut first = None;
        let mut typed = Vec::with_capacity(args.len());
        for arg in args {
            let value = self.typed(arg)?;
            if let (None, Typed::Expr(scalar)) = (&first, &value) {
                first = Some(scalar.value_type.clone());
            }
            typed.push(value);
        }
        let value_type = first.ok_or_else(|| {
            type_mismatch("COALESCE de literales sin tipo; añade CAST a uno de ellos.")
        })?;
        let values = typed
            .into_iter()
            .map(|value| match value {
                Typed::Expr(scalar) => Ok(scalar),
                Typed::Number(text) => number(&text, Some(&value_type), true),
                Typed::Null => Ok(literal(Literal::Null(value_type.clone()))),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if values.iter().any(|value| value.value_type != value_type) {
            return Err(type_mismatch(
                "Los argumentos de COALESCE tienen tipos distintos; añade CAST.",
            ));
        }
        let mut values = values.into_iter();
        let first = values
            .next()
            .ok_or_else(|| type_mismatch("COALESCE sin argumentos."))?;
        Ok(coalesce(first, values.collect()))
    }

    fn date_trunc(&mut self, unit: &Expr, value: &Expr) -> Result<ScalarExpr, Failure> {
        let Expr::Value(text) = unit else {
            return Err(not_representable("Una unidad de DATE_TRUNC no constante"));
        };
        let Value::SingleQuotedString(unit) = &text.value else {
            return Err(not_representable("Una unidad de DATE_TRUNC no constante"));
        };
        let (grain, calendar) = match unit.to_uppercase().as_str() {
            "SECOND" => (TimeGrain::Second, CalendarProfile::Gregorian),
            "MINUTE" => (TimeGrain::Minute, CalendarProfile::Gregorian),
            "HOUR" => (TimeGrain::Hour, CalendarProfile::Gregorian),
            "DAY" => (TimeGrain::Day, CalendarProfile::Gregorian),
            // SQL weeks start on Monday: the ISO week calendar.
            "WEEK" => (TimeGrain::Week, CalendarProfile::IsoWeek),
            "MONTH" => (TimeGrain::Month, CalendarProfile::Gregorian),
            "QUARTER" => (TimeGrain::Quarter, CalendarProfile::Gregorian),
            "YEAR" => (TimeGrain::Year, CalendarProfile::Gregorian),
            other => return Err(not_representable(&format!("DATE_TRUNC por `{other}`"))),
        };
        let timezone = self.names.timezone().cloned().ok_or_else(|| {
            Failure::new(
                Code::InvalidExtension,
                "DATE_TRUNC necesita la zona horaria de los límites de cada periodo: decláralo \
                 en la extensión del campo (\"time\": {\"timezone\": \"UTC\"}).",
            )
        })?;
        let operand = self.translate(value)?;
        let ScalarExprKind::FieldRef(source) = &operand.kind else {
            return Err(not_representable(
                "DATE_TRUNC sobre algo que no es una columna",
            ));
        };
        let value_type = match (&operand.value_type, grain) {
            (ValueType::Date | ValueType::Timestamp { .. }, _) if grain >= TimeGrain::Day => {
                ValueType::Date
            }
            (ValueType::Timestamp { .. }, _) => operand.value_type.clone(),
            _ => {
                return Err(type_mismatch(
                    "DATE_TRUNC necesita una fecha, o una fecha y hora para horas y minutos.",
                ));
            }
        };
        let bucket = TimeBucket {
            source: source.clone(),
            timezone,
            calendar,
            grain,
        };
        Ok(ScalarExpr {
            kind: ScalarExprKind::TemporalDerivation {
                operand: Box::new(operand),
                bucket,
            },
            value_type,
            unit: None,
        })
    }
}

/// `a / b` at the top of a cast, through parentheses.
pub(crate) fn division(expr: &Expr) -> Option<(&Expr, &Expr)> {
    match expr {
        Expr::Nested(inner) => division(inner),
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Divide,
            right,
        } => Some((left, right)),
        _ => None,
    }
}

/// An operand of an exact division: decimals stay, integers widen to
/// `DECIMAL(20,0)` (every 64-bit integer fits); floats only divide floats.
pub(crate) fn exact_operand(
    operand: ScalarExpr,
    target: &ValueType,
) -> Result<ScalarExpr, Failure> {
    match (&operand.value_type, target) {
        (ValueType::Decimal(_), ValueType::Decimal(_))
        | (ValueType::Float { .. }, ValueType::Float { .. }) => Ok(operand),
        (ValueType::Integer { .. }, ValueType::Decimal(_)) => {
            Ok(cast(operand, wide_integer_decimal()?))
        }
        _ => Err(type_mismatch(format!(
            "No se puede dividir {} para obtener {}; añade CAST.",
            describe(&operand.value_type),
            describe(target)
        ))),
    }
}

/// `DECIMAL(20,0)`: holds every 64-bit integer, signed or not.
pub(crate) fn wide_integer_decimal() -> Result<ValueType, Failure> {
    DecimalType::new(20, 0)
        .map(ValueType::Decimal)
        .map_err(|diagnostic| type_mismatch(diagnostic.message))
}

pub(crate) fn arithmetic(
    op: ArithmeticOp,
    left: ScalarExpr,
    right: ScalarExpr,
) -> Result<ScalarExpr, Failure> {
    let value_type = arithmetic_type(op, &left.value_type, &right.value_type)?;
    let unit = match op {
        ArithmeticOp::Add | ArithmeticOp::Subtract => left.unit.clone(),
        _ => None,
    };
    Ok(ScalarExpr {
        kind: ScalarExprKind::Arithmetic {
            op,
            left: Box::new(left),
            right: Box::new(right),
            contract: NumericContract::default(),
        },
        value_type,
        unit,
    })
}

/// The exact result type of `+`, `-` or `*` (SQL:2003 decimal rules).
pub(crate) fn arithmetic_type(
    op: ArithmeticOp,
    left: &ValueType,
    right: &ValueType,
) -> Result<ValueType, Failure> {
    match (left, right) {
        (ValueType::Decimal(l), ValueType::Decimal(r)) => {
            let (precision, scale) = derived_decimal(op, *l, *r)
                .map_err(|diagnostic| type_mismatch(diagnostic.message))?;
            u8::try_from(precision)
                .ok()
                .zip(u8::try_from(scale).ok())
                .and_then(|(precision, scale)| DecimalType::new(precision, scale).ok())
                .map(ValueType::Decimal)
                .ok_or_else(|| {
                    Failure::new(
                        Code::TypeMismatch,
                        format!(
                            "El resultado necesitaría DECIMAL({precision}, {scale}) y el máximo es \
                             76 dígitos; reduce la precisión con CAST."
                        ),
                    )
                })
        }
        _ if left == right && left.is_numeric() => Ok(left.clone()),
        _ => Err(type_mismatch(format!(
            "Operación entre {} y {}: CSMIR no convierte tipos numéricos en silencio; añade CAST.",
            describe(left),
            describe(right)
        ))),
    }
}

fn cast(operand: ScalarExpr, target: ValueType) -> ScalarExpr {
    let rounding = match (&operand.value_type, &target) {
        (ValueType::Decimal(from), ValueType::Decimal(to)) if to.scale() < from.scale() => {
            Some(RoundingMode::HalfEven)
        }
        (ValueType::Float { .. }, ValueType::Decimal(_) | ValueType::Integer { .. }) => {
            Some(RoundingMode::HalfEven)
        }
        _ => None,
    };
    ScalarExpr {
        unit: operand.unit.clone(),
        kind: ScalarExprKind::ExplicitCast {
            operand: Box::new(operand),
            rounding,
        },
        value_type: target,
    }
}

/// `COALESCE(first, rest…)` as nested `CASE WHEN x IS NOT NULL THEN x`,
/// built from the last argument outwards.
fn coalesce(first: ScalarExpr, mut rest: Vec<ScalarExpr>) -> ScalarExpr {
    let Some(last) = rest.pop() else {
        return first;
    };
    std::iter::once(first)
        .chain(rest)
        .rev()
        .fold(last, |otherwise, value| {
            let condition = ScalarExpr {
                kind: ScalarExprKind::NullTest {
                    operand: Box::new(value.clone()),
                    negated: true,
                },
                value_type: ValueType::Boolean,
                unit: None,
            };
            ScalarExpr {
                value_type: value.value_type.clone(),
                unit: value.unit.clone(),
                kind: ScalarExprKind::Case {
                    branches: vec![(condition, value)],
                    otherwise: Box::new(otherwise),
                },
            }
        })
}

/// A comparison with a decimal literal states the literal at the other
/// side's exact type when it fits (`amount = 1.5` with `DECIMAL(18,2)`).
fn exact_literal(left: ScalarExpr, right: ScalarExpr) -> Result<(ScalarExpr, ScalarExpr), Failure> {
    let rescale = |scalar: ScalarExpr, like: &ValueType| -> Result<ScalarExpr, Failure> {
        match (&scalar.kind, like) {
            (ScalarExprKind::Literal(Literal::Decimal(value)), ValueType::Decimal(target))
                if scalar.value_type != *like =>
            {
                Ok(literal(Literal::Decimal(rescale(value, *target)?)))
            }
            _ => Ok(scalar),
        }
    };
    let right_type = right.value_type.clone();
    let left = rescale(left, &right_type)?;
    let right = rescale(right, &left.value_type)?;
    Ok((left, right))
}

fn rescale(value: &DecimalValue, target: DecimalType) -> Result<DecimalValue, Failure> {
    let from = value.decimal_type().scale();
    let to = target.scale();
    if to < from {
        return Err(type_mismatch(format!(
            "El literal tiene más decimales que el tipo con el que se compara (escala {to})."
        )));
    }
    let coefficient = if value.is_zero() {
        "0".to_owned()
    } else {
        format!(
            "{}{}",
            value.coefficient(),
            "0".repeat(usize::from(to - from))
        )
    };
    DecimalValue::new(coefficient, target)
        .map_err(|_| type_mismatch("El literal no cabe en el tipo con el que se compara."))
}

/// A numeric literal of the kind of `like` (integer, decimal or float),
/// otherwise a 64-bit integer or the smallest exact decimal that holds it.
/// With `exact`, a decimal literal takes `like`'s exact precision and scale
/// (a `CASE` branch, a comparison); without it, its own minimal type, so
/// `amount * 2` does not inflate the result's precision.
fn number(text: &str, like: Option<&ValueType>, exact: bool) -> Result<ScalarExpr, Failure> {
    // Beside text or a date, a number keeps its own type and the comparison
    // reports the mismatch.
    let like = like.filter(|like| like.is_numeric());
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if digits.contains(['e', 'E']) {
        return Err(not_representable("Un número en notación científica"));
    }
    let (integer, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    if !integer
        .bytes()
        .chain(fraction.bytes())
        .all(|b| b.is_ascii_digit())
        || integer.is_empty() && fraction.is_empty()
    {
        return Err(type_mismatch(format!("{text:?} no es un número.")));
    }
    let sign = if negative { "-" } else { "" };
    let integral = fraction.bytes().all(|b| b == b'0');
    let as_float = || {
        text.parse::<f64>()
            .ok()
            .and_then(|value| FiniteFloat::new(value).ok())
            .map(|value| literal_typed(Literal::Float(value), like.cloned()))
            .ok_or_else(|| type_mismatch(format!("{text:?} no es un número finito.")))
    };
    match like {
        Some(ValueType::Float { .. }) => as_float(),
        Some(ValueType::Integer { .. }) | None if integral => {
            let value = format!("{sign}{integer}")
                .parse::<i64>()
                .map_err(|_| type_mismatch(format!("{text} no cabe en un entero de 64 bits.")))?;
            let scalar = literal(Literal::SignedInteger(value.into()));
            Ok(match like {
                Some(target) if *target != scalar.value_type => cast(scalar, target.clone()),
                _ => scalar,
            })
        }
        Some(ValueType::Integer { .. }) => Err(type_mismatch(format!(
            "{text} tiene decimales y se usa como entero."
        ))),
        _ => {
            let significant = integer.trim_start_matches('0');
            let scale = fraction.len();
            let precision = (significant.len() + scale).max(1);
            let decimal_type = u8::try_from(precision)
                .ok()
                .zip(u8::try_from(scale).ok())
                .and_then(|(precision, scale)| DecimalType::new(precision, scale).ok())
                .ok_or_else(|| type_mismatch(format!("{text} supera los 76 dígitos.")))?;
            let value = DecimalValue::new(format!("{sign}{integer}{fraction}"), decimal_type)
                .map_err(|diagnostic| type_mismatch(diagnostic.message))?;
            match like {
                Some(ValueType::Decimal(target)) if exact => {
                    Ok(literal(Literal::Decimal(rescale(&value, *target)?)))
                }
                _ => Ok(literal(Literal::Decimal(value))),
            }
        }
    }
}

fn literal(value: Literal) -> ScalarExpr {
    let value_type = value.value_type();
    ScalarExpr {
        kind: ScalarExprKind::Literal(value),
        value_type,
        unit: None,
    }
}

fn literal_typed(value: Literal, like: Option<ValueType>) -> ScalarExpr {
    let scalar = literal(value);
    match like {
        Some(target) if target != scalar.value_type => cast(scalar, target),
        _ => scalar,
    }
}

fn unary(op: UnaryOp, operand: ScalarExpr) -> ScalarExpr {
    ScalarExpr {
        value_type: operand.value_type.clone(),
        unit: operand.unit.clone(),
        kind: ScalarExprKind::Unary {
            op,
            operand: Box::new(operand),
        },
    }
}

fn boolean(op: BooleanOp, operands: Vec<ScalarExpr>) -> ScalarExpr {
    ScalarExpr {
        kind: ScalarExprKind::BooleanLogic { op, operands },
        value_type: ValueType::Boolean,
        unit: None,
    }
}

/// The CSMIR type a portable `CAST` target names.
pub(crate) fn cast_target(data_type: &DataType) -> Result<ValueType, Failure> {
    let integer = |bits| ValueType::Integer {
        signed: true,
        bits: Some(bits),
    };
    Ok(match data_type {
        DataType::Varchar(None) | DataType::String(None) | DataType::Text => ValueType::Utf8,
        DataType::Varchar(Some(_)) | DataType::String(Some(_)) => {
            return Err(not_representable(
                "CAST a VARCHAR(n) (CSMIR no conserva la longitud, y truncar o rechazar los \
                 valores largos cambiaría el dominio de la dimensión)",
            ));
        }
        DataType::Int(_) | DataType::Integer(_) => integer(32),
        DataType::BigInt(_) => integer(64),
        DataType::Decimal(info) | DataType::Numeric(info) => {
            let (precision, scale) = match info {
                ExactNumberInfo::PrecisionAndScale(precision, scale) => (*precision, *scale),
                ExactNumberInfo::Precision(precision) => (*precision, 0),
                ExactNumberInfo::None => {
                    return Err(type_mismatch(
                        "DECIMAL necesita precisión y escala: DECIMAL(p, s).",
                    ));
                }
            };
            u8::try_from(precision)
                .ok()
                .zip(u8::try_from(scale).ok())
                .and_then(|(precision, scale)| DecimalType::new(precision, scale).ok())
                .map(ValueType::Decimal)
                .ok_or_else(|| type_mismatch("DECIMAL(p, s) admite 1 ≤ p ≤ 76 y 0 ≤ s ≤ p."))?
        }
        // FLOAT(p) keeps p binary digits: up to 24 fit in 32 bits, up to 53
        // in 64. A bare FLOAT is 32 bits in DuckDB and 64 in PostgreSQL.
        DataType::Float(ExactNumberInfo::Precision(1..=24)) | DataType::Real => {
            ValueType::Float { bits: 32 }
        }
        DataType::Float(ExactNumberInfo::Precision(25..=53))
        | DataType::Double(ExactNumberInfo::None)
        | DataType::DoublePrecision
        | DataType::Float64 => ValueType::Float { bits: 64 },
        DataType::Float(ExactNumberInfo::None) => {
            return Err(type_mismatch(
                "FLOAT sin precisión es de 32 bits en unos motores y de 64 en otros: usa REAL, \
                 DOUBLE o FLOAT(p).",
            ));
        }
        DataType::Float(_) | DataType::Double(_) => {
            return Err(type_mismatch(
                "FLOAT(p) admite 1 ≤ p ≤ 53 dígitos binarios, sin escala.",
            ));
        }
        DataType::Boolean | DataType::Bool => ValueType::Boolean,
        DataType::Date => ValueType::Date,
        DataType::Time(None, _) => ValueType::Time,
        _ => {
            return Err(not_representable(&format!(
                "CAST a `{data_type}` (una fecha y hora necesita su zona horaria)"
            )));
        }
    })
}

pub(crate) fn function_name(function: &Function) -> String {
    function
        .name
        .0
        .last()
        .and_then(|part| part.as_ident())
        .map(|ident| ident.value.to_uppercase())
        .unwrap_or_default()
}

pub(crate) fn arguments(function: &Function) -> Result<Vec<&Expr>, Failure> {
    match &function.args {
        FunctionArguments::None => Ok(Vec::new()),
        FunctionArguments::List(list) => list
            .args
            .iter()
            .map(|arg| match arg {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => Ok(expr),
                _ => Err(not_representable("Ese argumento")),
            })
            .collect(),
        FunctionArguments::Subquery(_) => Err(not_representable("Una subconsulta")),
    }
}

/// Ossie identifier normalization (`expression.rs`): unquoted upper-cased,
/// double-quoted exact.
pub(crate) fn normalize(ident: &Ident) -> String {
    match ident.quote_style {
        Some('"') => ident.value.clone(),
        _ => ident.value.to_uppercase(),
    }
}

fn date(text: &str) -> Result<i32, Failure> {
    sef_model_ir::time::iso_date_days(text)
        .ok_or_else(|| type_mismatch(format!("{text:?} no es una fecha AAAA-MM-DD válida.")))
}

pub(crate) fn describe(value_type: &ValueType) -> String {
    match value_type {
        ValueType::Boolean => "BOOLEAN".to_owned(),
        ValueType::Integer { bits, .. } => format!("INTEGER({})", bits.unwrap_or(64)),
        ValueType::Decimal(decimal) => {
            format!("DECIMAL({}, {})", decimal.precision(), decimal.scale())
        }
        ValueType::Float { bits } => format!("FLOAT({bits})"),
        ValueType::Utf8 => "VARCHAR".to_owned(),
        ValueType::Date => "DATE".to_owned(),
        ValueType::Time => "TIME".to_owned(),
        ValueType::Timestamp { .. } => "TIMESTAMP".to_owned(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::date;

    #[test]
    fn civil_dates_count_days_from_the_unix_epoch() {
        assert_eq!(date("1970-01-01"), Ok(0));
        assert_eq!(date("2000-03-01"), Ok(11_017));
        assert_eq!(date("1969-12-31"), Ok(-1));
        assert_eq!(date("2024-02-29"), Ok(19_782));
        assert!(date("2023-02-29").is_err());
        assert!(date("2024-13-01").is_err());
        assert!(date("24-01-01").is_err());
    }
}
