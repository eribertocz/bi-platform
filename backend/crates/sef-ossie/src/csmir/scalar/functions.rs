//! `OSSIE_SQL_2026` REQUIRED scalar functions (#77) to CSMIR. Each call is
//! typed by [`ScalarFunction::result`], the rule validation also applies;
//! conditional shorthands become the CASE and boolean nodes they mean.
//! Functions whose cross-engine semantics are not fixed yet are refused with
//! the reason, never passed through.

use sef_model_ir::expressions::{BooleanOp, ComparisonOp, ScalarExpr, ScalarExprKind, UnaryOp};
use sef_model_ir::functions::ScalarFunction;
use sef_model_ir::types::{Literal, ValueType};
use sqlparser::ast::{Expr, TrimWhereField};

use super::{
    Failure, Names, Translator, Typed, boolean, exact_literal, literal, not_representable, number,
    type_mismatch, unary,
};

const fn int64() -> ValueType {
    ValueType::Integer {
        signed: true,
        bits: Some(64),
    }
}

/// Functions whose semantics differ between engines in ways CSMIR does not
/// pin down yet: refused with the reason.
fn unsettled(name: &str) -> Option<Failure> {
    let reason = match name {
        "REGEXP_LIKE" => {
            "los motores usan dialectos de expresiones regulares distintos (RE2, POSIX, PCRE); \
             no se calcula hasta fijar uno"
        }
        "POWER" | "SQRT" | "EXP" | "LN" | "LOG" | "LOG10" => {
            "su resultado es de coma flotante y su dominio (raíces de negativos, logaritmos de \
             cero) aún no está fijado; llegará en su propia entrega"
        }
        _ => return None,
    };
    Some(not_representable(&format!("`{name}`: {reason}")))
}

impl<N: Names> Translator<'_, N> {
    /// The scalar function `name` applied to `args`, if it is one of ours.
    pub(super) fn scalar_function(
        &mut self,
        name: &str,
        args: &[&Expr],
    ) -> Option<Result<ScalarExpr, Failure>> {
        if let Some(failure) = unsettled(name) {
            return Some(Err(failure));
        }
        let function = match name {
            "CONCAT" => ScalarFunction::Concat,
            "LENGTH" => ScalarFunction::Length,
            "LOWER" => ScalarFunction::Lower,
            "UPPER" => ScalarFunction::Upper,
            "TRIM" => ScalarFunction::Trim,
            "LTRIM" => ScalarFunction::LTrim,
            "RTRIM" => ScalarFunction::RTrim,
            "LEFT" => ScalarFunction::Left,
            "RIGHT" => ScalarFunction::Right,
            "SUBSTRING" => ScalarFunction::Substring,
            "REPLACE" => ScalarFunction::Replace,
            "SPLIT_PART" => ScalarFunction::SplitPart,
            "POSITION" | "CHARINDEX" => ScalarFunction::Position,
            "CONTAINS" => ScalarFunction::Contains,
            "STARTSWITH" => ScalarFunction::StartsWith,
            "ENDSWITH" => ScalarFunction::EndsWith,
            "ABS" => ScalarFunction::Abs,
            "SIGN" => ScalarFunction::Sign,
            "FLOOR" => ScalarFunction::Floor,
            "CEIL" | "CEILING" => ScalarFunction::Ceil,
            "ROUND" => ScalarFunction::Round,
            "TRUNC" | "TRUNCATE" => ScalarFunction::Truncate,
            "MOD" => ScalarFunction::Mod {
                zero: self.division.zero,
            },
            "GREATEST" => ScalarFunction::Greatest,
            "LEAST" => ScalarFunction::Least,
            "NVL2" => return Some(self.nvl2(args)),
            "NULLIFZERO" => return Some(self.null_if_zero(args)),
            _ => return None,
        };
        Some(self.call(function, args))
    }

    /// Types the arguments the way the function reads them and builds the
    /// call; bare numbers become 64-bit integers where an integer is
    /// expected, and take their neighbour's type in `MOD`, `GREATEST` and
    /// `LEAST`.
    pub(super) fn call(
        &mut self,
        function: ScalarFunction,
        args: &[&Expr],
    ) -> Result<ScalarExpr, Failure> {
        let arguments = match function {
            ScalarFunction::Left
            | ScalarFunction::Right
            | ScalarFunction::Round
            | ScalarFunction::Truncate => {
                let mut typed = Vec::with_capacity(args.len());
                for (i, arg) in args.iter().enumerate() {
                    typed.push(if i == 0 {
                        self.translate(arg)?
                    } else {
                        self.translate_like(arg, &int64(), true)?
                    });
                }
                typed
            }
            ScalarFunction::Substring | ScalarFunction::SplitPart => {
                let mut typed = Vec::with_capacity(args.len());
                for (i, arg) in args.iter().enumerate() {
                    let integral = matches!((function, i), (ScalarFunction::Substring, 1 | 2))
                        || matches!((function, i), (ScalarFunction::SplitPart, 2));
                    typed.push(if integral {
                        self.translate_like(arg, &int64(), true)?
                    } else {
                        self.translate_like(arg, &ValueType::Utf8, true)?
                    });
                }
                typed
            }
            ScalarFunction::Mod { .. } => match args {
                [dividend, divisor] => {
                    let (dividend, divisor) = self.pair(dividend, divisor, false)?;
                    vec![dividend, divisor]
                }
                _ => return Err(arity(function)),
            },
            ScalarFunction::Greatest | ScalarFunction::Least => self.same_type(args)?,
            _ => args
                .iter()
                .map(|arg| self.translate_like(arg, &ValueType::Utf8, true))
                .collect::<Result<Vec<_>, _>>()?,
        };
        build(function, arguments)
    }

    /// Every argument in the type of the first typed one, as COALESCE.
    fn same_type(&mut self, args: &[&Expr]) -> Result<Vec<ScalarExpr>, Failure> {
        let mut typed = Vec::with_capacity(args.len());
        let mut first: Option<ValueType> = None;
        for arg in args {
            let value = self.typed(arg)?;
            if let (None, Typed::Expr(scalar)) = (&first, &value) {
                first = Some(scalar.value_type.clone());
            }
            typed.push(value);
        }
        let value_type = first.ok_or_else(|| {
            type_mismatch("GREATEST y LEAST de literales sin tipo; añade CAST a uno de ellos.")
        })?;
        typed
            .into_iter()
            .map(|value| match value {
                Typed::Expr(scalar) => Ok(scalar),
                Typed::Number(text) => number(&text, Some(&value_type), true),
                Typed::Null => Ok(literal(Literal::Null(value_type.clone()))),
            })
            .collect()
    }

    /// `TRIM([BOTH | LEADING | TRAILING] [c FROM] s)`.
    pub(super) fn trim(
        &mut self,
        value: &Expr,
        side: Option<&TrimWhereField>,
        character: Option<&Expr>,
    ) -> Result<ScalarExpr, Failure> {
        let function = match side {
            None | Some(TrimWhereField::Both) => ScalarFunction::Trim,
            Some(TrimWhereField::Leading) => ScalarFunction::LTrim,
            Some(TrimWhereField::Trailing) => ScalarFunction::RTrim,
        };
        let mut args = vec![value];
        args.extend(character);
        self.call(function, &args)
    }

    /// `a LIKE p`, `a ILIKE p` and their negations.
    pub(super) fn like(
        &mut self,
        value: &Expr,
        pattern: &Expr,
        insensitive: bool,
        negated: bool,
    ) -> Result<ScalarExpr, Failure> {
        let function = if insensitive {
            ScalarFunction::ILike
        } else {
            ScalarFunction::Like
        };
        let matched = self.call(function, &[value, pattern])?;
        Ok(if negated {
            unary(UnaryOp::Not, matched)
        } else {
            matched
        })
    }

    /// `a IS [NOT] DISTINCT FROM b`: equality where two NULLs are equal and
    /// the answer is never NULL.
    pub(super) fn distinct(
        &mut self,
        left: &Expr,
        right: &Expr,
        distinct: bool,
    ) -> Result<ScalarExpr, Failure> {
        let (left, right) = self.pair(left, right, true)?;
        let (left, right) = exact_literal(left, right)?;
        if left.value_type != right.value_type {
            return Err(type_mismatch(
                "IS DISTINCT FROM compara valores de tipos distintos; añade CAST.",
            ));
        }
        let null = |operand: &ScalarExpr, negated: bool| ScalarExpr {
            kind: ScalarExprKind::NullTest {
                operand: Box::new(operand.clone()),
                negated,
            },
            value_type: ValueType::Boolean,
            unit: None,
        };
        let equal = ScalarExpr {
            kind: ScalarExprKind::Comparison {
                op: ComparisonOp::Equal,
                left: Box::new(left.clone()),
                right: Box::new(right.clone()),
            },
            value_type: ValueType::Boolean,
            unit: None,
        };
        let same = boolean(
            BooleanOp::Or,
            vec![
                boolean(
                    BooleanOp::And,
                    vec![null(&left, false), null(&right, false)],
                ),
                boolean(
                    BooleanOp::And,
                    vec![null(&left, true), null(&right, true), equal],
                ),
            ],
        );
        Ok(if distinct {
            unary(UnaryOp::Not, same)
        } else {
            same
        })
    }

    /// `NVL2(a, b, c)`: `CASE WHEN a IS NOT NULL THEN b ELSE c END`.
    fn nvl2(&mut self, args: &[&Expr]) -> Result<ScalarExpr, Failure> {
        let [value, present, absent] = args else {
            return Err(type_mismatch("NVL2 toma tres argumentos."));
        };
        let value = self.translate(value)?;
        let condition = ScalarExpr {
            kind: ScalarExprKind::NullTest {
                operand: Box::new(value),
                negated: true,
            },
            value_type: ValueType::Boolean,
            unit: None,
        };
        self.case(vec![condition], &[*present, *absent], true)
    }

    /// `NULLIFZERO(x)`: `CASE WHEN x = 0 THEN NULL ELSE x END`.
    fn null_if_zero(&mut self, args: &[&Expr]) -> Result<ScalarExpr, Failure> {
        let [value] = args else {
            return Err(type_mismatch("NULLIFZERO toma un argumento."));
        };
        let value = self.translate(value)?;
        if !value.value_type.is_numeric() {
            return Err(type_mismatch("NULLIFZERO necesita un número."));
        }
        let zero = number("0", Some(&value.value_type), true)?;
        let condition = ScalarExpr {
            kind: ScalarExprKind::Comparison {
                op: ComparisonOp::Equal,
                left: Box::new(value.clone()),
                right: Box::new(zero),
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
}

fn arity(function: ScalarFunction) -> Failure {
    type_mismatch(format!(
        "`{}` no admite ese número de argumentos.",
        function.code().to_uppercase()
    ))
}

/// The typed call, or why its arguments do not fit.
fn build(function: ScalarFunction, arguments: Vec<ScalarExpr>) -> Result<ScalarExpr, Failure> {
    let (value_type, unit) = function.result(&arguments).map_err(|reason| {
        type_mismatch(format!(
            "`{}` no admite esos argumentos: {reason}.",
            function.code().to_uppercase()
        ))
    })?;
    Ok(ScalarExpr {
        kind: ScalarExprKind::Function {
            function,
            arguments,
        },
        value_type,
        unit,
    })
}
