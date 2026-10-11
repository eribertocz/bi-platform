//! `OSSIE_SQL_2026` expressions (`core-spec/expression_language.md`): parsed
//! with a proven SQL parser, then checked against an **allowlist** of the
//! constructs and REQUIRED/RECOMMENDED functions the language defines.
//! Anything else (subqueries, `SELECT`, vendor syntax, unknown functions) is
//! rejected rather than passed through, because a semantic engine must know
//! what every expression means.
//!
//! The analysis also classifies each aggregate: Ossie's decomposability
//! class (pre-aggregation) and whether it is idempotent under row
//! duplication (fanout safety, judged per aggregate on the expanded
//! expression). The allowlist of idempotent aggregates follows Ossie's Cube
//! converter: `MIN`, `MAX`, `APPROX_COUNT_DISTINCT` and any aggregate over
//! `DISTINCT`.

use std::collections::BTreeSet;

use sqlparser::ast::{
    BinaryOperator, CastKind, DataType, DuplicateTreatment, Expr, Function, FunctionArg,
    FunctionArgExpr, FunctionArguments, UnaryOperator, Value, WindowType,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Token;

use crate::diagnostic::{Code, Diagnostic};
use crate::validate::normalize_identifier;

/// Where an expression appears: field expressions are row-level scalars;
/// metric expressions aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Field,
    Metric,
}

/// A reference to a field or metric, as normalized identifiers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Reference {
    /// The dataset of `dataset.field`; `None` for a bare name.
    pub qualifier: Option<String>,
    pub name: String,
}

/// Ossie's decomposability classes (expression language, "Decomposability
/// Reference").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateClass {
    /// `SUM`, `COUNT`, `MIN`, `MAX`: partial results combine directly.
    Distributive,
    /// `AVG`, `STDDEV`, `VARIANCE`: combine through a fixed set of partials.
    Algebraic,
    /// Medians, percentiles, `COUNT(DISTINCT)`: need all values.
    Holistic,
    /// `APPROX_*`: mergeable sketches, approximate by definition.
    Sketch,
}

/// One aggregate inside an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aggregate {
    /// Upper-cased function name.
    pub function: String,
    pub class: AggregateClass,
    pub distinct: bool,
    /// Duplicated input rows cannot change the result.
    pub fanout_safe: bool,
    /// The references the aggregate reads; empty for `COUNT(*)`.
    pub references: BTreeSet<Reference>,
}

/// What an accepted expression reads and aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Analysis {
    pub references: BTreeSet<Reference>,
    pub aggregates: Vec<Aggregate>,
    /// Uses a window function (`… OVER (…)`).
    pub windowed: bool,
}

/// Parses and checks one expression.
///
/// # Errors
/// [`Code::ExpressionSyntax`] when it does not parse as exactly one
/// expression; otherwise the first construct the language does not allow.
pub fn analyze(text: &str, context: Context) -> Result<Analysis, Diagnostic> {
    let expr = parse(text)?;
    let mut walker = Walker {
        context,
        analysis: Analysis::default(),
        aggregate: None,
    };
    walker.expr(&expr)?;
    Ok(walker.analysis)
}

fn parse(text: &str) -> Result<Expr, Diagnostic> {
    let syntax = |message: String| Diagnostic::error(Code::ExpressionSyntax, "", message);
    let dialect = GenericDialect {};
    let mut parser = Parser::new(&dialect)
        .try_with_sql(text)
        .map_err(|e| syntax(format!("Expresión no válida: {e}")))?;
    let expr = parser
        .parse_expr()
        .map_err(|e| syntax(format!("Expresión no válida: {e}")))?;
    if parser.peek_token().token != Token::EOF {
        return Err(syntax(
            "Después de la expresión sobra texto: una expresión es un solo valor, no varias \
             sentencias."
                .to_owned(),
        ));
    }
    Ok(expr)
}

struct Walker {
    context: Context,
    analysis: Analysis,
    /// The aggregate being walked, collecting its references.
    aggregate: Option<Aggregate>,
}

fn unsupported(what: &str) -> Diagnostic {
    Diagnostic::error(
        Code::UnsupportedConstruct,
        "",
        format!("{what} no forma parte de OSSIE_SQL_2026."),
    )
}

impl Walker {
    fn reference(&mut self, reference: Reference) {
        if let Some(aggregate) = &mut self.aggregate {
            aggregate.references.insert(reference.clone());
        }
        self.analysis.references.insert(reference);
    }

    fn all(&mut self, exprs: &[Expr]) -> Result<(), Diagnostic> {
        exprs.iter().try_for_each(|expr| self.expr(expr))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm per allowed construct reads as the allowlist"
    )]
    fn expr(&mut self, expr: &Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Identifier(ident) => {
                self.reference(Reference {
                    qualifier: None,
                    name: normalize(ident),
                });
                Ok(())
            }
            Expr::CompoundIdentifier(parts) => match parts.as_slice() {
                [dataset, name] => {
                    self.reference(Reference {
                        qualifier: Some(normalize(dataset)),
                        name: normalize(name),
                    });
                    Ok(())
                }
                _ => Err(unsupported(
                    "Una referencia de más de dos partes (solo `dataset.campo`)",
                )),
            },
            Expr::Value(value) => match &value.value {
                Value::Number(..)
                | Value::SingleQuotedString(_)
                | Value::Boolean(_)
                | Value::Null => Ok(()),
                Value::Placeholder(_) => Err(unsupported("Un parámetro (`?`, `$1`)")),
                _ => Err(unsupported("Ese tipo de literal")),
            },
            Expr::Nested(inner)
            | Expr::IsNull(inner)
            | Expr::IsNotNull(inner)
            | Expr::UnaryOp {
                op: UnaryOperator::Plus | UnaryOperator::Minus | UnaryOperator::Not,
                expr: inner,
            } => self.expr(inner),
            Expr::IsDistinctFrom(left, right) | Expr::IsNotDistinctFrom(left, right) => {
                self.expr(left)?;
                self.expr(right)
            }
            Expr::BinaryOp { left, op, right } => {
                if !allowed_operator(op) {
                    return Err(unsupported(&format!("El operador `{op}`")));
                }
                self.expr(left)?;
                self.expr(right)
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.expr(expr)?;
                self.expr(low)?;
                self.expr(high)
            }
            Expr::InList { expr, list, .. } => {
                self.expr(expr)?;
                self.all(list)
            }
            Expr::Like {
                expr,
                pattern,
                any: false,
                escape_char: None,
                ..
            }
            | Expr::ILike {
                expr,
                pattern,
                any: false,
                escape_char: None,
                ..
            } => {
                self.expr(expr)?;
                self.expr(pattern)
            }
            Expr::Case {
                operand,
                conditions,
                else_result,
                ..
            } => {
                if let Some(operand) = operand {
                    self.expr(operand)?;
                }
                for when in conditions {
                    self.expr(&when.condition)?;
                    self.expr(&when.result)?;
                }
                if let Some(otherwise) = else_result {
                    self.expr(otherwise)?;
                }
                Ok(())
            }
            Expr::Cast {
                kind: CastKind::Cast | CastKind::TryCast,
                expr,
                data_type,
                format: None,
                ..
            } => {
                if !allowed_cast_target(data_type) {
                    return Err(unsupported(&format!("El tipo `{data_type}` en CAST")));
                }
                self.expr(expr)
            }
            Expr::Extract { expr, .. } | Expr::Ceil { expr, .. } | Expr::Floor { expr, .. } => {
                self.expr(expr)
            }
            Expr::Position { expr, r#in } => {
                self.expr(expr)?;
                self.expr(r#in)
            }
            Expr::Substring {
                expr,
                substring_from,
                substring_for,
                ..
            } => {
                self.expr(expr)?;
                if let Some(from) = substring_from {
                    self.expr(from)?;
                }
                if let Some(length) = substring_for {
                    self.expr(length)?;
                }
                Ok(())
            }
            Expr::Trim {
                expr, trim_what, ..
            } => {
                self.expr(expr)?;
                if let Some(what) = trim_what {
                    self.expr(what)?;
                }
                Ok(())
            }
            Expr::Function(function) => self.function(function),
            Expr::Subquery(_) | Expr::InSubquery { .. } | Expr::Exists { .. } => Err(unsupported(
                "Una subconsulta (usa referencias a campos; `EXISTS_IN` aún no está soportado)",
            )),
            Expr::Wildcard(_) | Expr::QualifiedWildcard(..) => {
                Err(unsupported("El comodín `*` fuera de `COUNT(*)`"))
            }
            other => Err(unsupported(&format!("La construcción `{other}`"))),
        }
    }

    fn function(&mut self, function: &Function) -> Result<(), Diagnostic> {
        let name = function_name(function)?;
        if function.uses_odbc_syntax
            || !matches!(function.parameters, FunctionArguments::None)
            || function.filter.is_some()
            || function.null_treatment.is_some()
        {
            return Err(unsupported(&format!("Esa forma de llamar a `{name}`")));
        }
        let (args, distinct) = match &function.args {
            FunctionArguments::None => (Vec::new(), false),
            FunctionArguments::Subquery(_) => {
                return Err(unsupported("Una subconsulta como argumento"));
            }
            FunctionArguments::List(list) => {
                if !list.clauses.is_empty() {
                    return Err(unsupported(&format!("Cláusulas dentro de `{name}(…)`")));
                }
                let distinct =
                    matches!(list.duplicate_treatment, Some(DuplicateTreatment::Distinct));
                (list.args.iter().collect(), distinct)
            }
        };
        let window = match &function.over {
            None => None,
            Some(WindowType::WindowSpec(spec)) => Some(spec),
            Some(WindowType::NamedWindow(_)) => return Err(unsupported("Una ventana con nombre")),
        };
        let class = aggregate_class(&name, distinct);
        if window.is_none() && !function.within_group.is_empty() && !is_ordered_set(&name) {
            return Err(unsupported(&format!("`WITHIN GROUP` en `{name}`")));
        }
        if let Some(spec) = window {
            if self.context == Context::Field {
                return Err(Diagnostic::error(
                    Code::AggregateInField,
                    "",
                    format!(
                        "Un campo es un valor por fila: no puede usar la ventana `{name}(…) OVER`."
                    ),
                ));
            }
            if class.is_none() && !WINDOW_FUNCTIONS.contains(&name.as_str()) {
                return Err(unknown_function(&name));
            }
            self.analysis.windowed = true;
            self.arguments(&name, &args)?;
            self.all(&spec.partition_by)?;
            for order in &spec.order_by {
                self.expr(&order.expr)?;
            }
            return Ok(());
        }
        if let Some(class) = class {
            return self.aggregate(function, &name, class, distinct, &args);
        }
        if WINDOW_FUNCTIONS.contains(&name.as_str()) {
            return Err(unsupported(&format!("`{name}` sin `OVER (…)`")));
        }
        if !SCALAR_FUNCTIONS.contains(&name.as_str()) {
            return Err(unknown_function(&name));
        }
        self.arguments(&name, &args)
    }

    fn aggregate(
        &mut self,
        function: &Function,
        name: &str,
        class: AggregateClass,
        distinct: bool,
        args: &[&FunctionArg],
    ) -> Result<(), Diagnostic> {
        if self.context == Context::Field {
            return Err(Diagnostic::error(
                Code::AggregateInField,
                "",
                format!(
                    "Un campo es un valor por fila: no puede agregar con `{name}`. Defínelo como métrica."
                ),
            ));
        }
        if self.aggregate.is_some() {
            return Err(Diagnostic::error(
                Code::NestedAggregate,
                "",
                format!("`{name}` está dentro de otro agregado."),
            ));
        }
        let fanout_safe = distinct || FANOUT_SAFE_AGGREGATES.contains(&name);
        self.aggregate = Some(Aggregate {
            function: name.to_owned(),
            class,
            distinct,
            fanout_safe,
            references: BTreeSet::new(),
        });
        let walked = self.arguments(name, args).and_then(|()| {
            function
                .within_group
                .iter()
                .try_for_each(|order| self.expr(&order.expr))
        });
        let aggregate = self.aggregate.take();
        walked?;
        self.analysis.aggregates.extend(aggregate);
        Ok(())
    }

    fn arguments(&mut self, name: &str, args: &[&FunctionArg]) -> Result<(), Diagnostic> {
        for arg in args {
            match arg {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => self.expr(expr)?,
                FunctionArg::Unnamed(FunctionArgExpr::Wildcard) if name == "COUNT" => {}
                _ => return Err(unsupported(&format!("Ese argumento de `{name}`"))),
            }
        }
        Ok(())
    }
}

fn normalize(ident: &sqlparser::ast::Ident) -> String {
    match ident.quote_style {
        // A quoted identifier keeps its exact spelling.
        Some('"') => ident.value.clone(),
        _ => normalize_identifier(&ident.value),
    }
}

fn function_name(function: &Function) -> Result<String, Diagnostic> {
    match function.name.0.as_slice() {
        [part] => part
            .as_ident()
            .map(|ident| ident.value.to_uppercase())
            .ok_or_else(|| unsupported("Ese nombre de función")),
        _ => Err(unsupported("Una función calificada (`esquema.función`)")),
    }
}

fn unknown_function(name: &str) -> Diagnostic {
    Diagnostic::error(
        Code::UnknownFunction,
        "",
        format!("La función `{name}` no es REQUIRED ni RECOMMENDED en OSSIE_SQL_2026."),
    )
}

fn allowed_operator(op: &BinaryOperator) -> bool {
    use BinaryOperator as Op;
    matches!(
        op,
        Op::Plus
            | Op::Minus
            | Op::Multiply
            | Op::Divide
            | Op::Modulo
            | Op::StringConcat
            | Op::Eq
            | Op::NotEq
            | Op::Lt
            | Op::Gt
            | Op::LtEq
            | Op::GtEq
            | Op::And
            | Op::Or
    )
}

/// `VARCHAR`/`STRING`, `INTEGER`/`INT`/`BIGINT`, `DECIMAL`/`NUMERIC`,
/// `FLOAT`/`DOUBLE`, `BOOLEAN`, `DATE`, `TIMESTAMP` and `TIME`.
fn allowed_cast_target(data_type: &DataType) -> bool {
    let text = data_type.to_string().to_uppercase();
    let base = text.split('(').next().unwrap_or_default().trim();
    [
        "VARCHAR",
        "STRING",
        "INTEGER",
        "INT",
        "BIGINT",
        "DECIMAL",
        "NUMERIC",
        "FLOAT",
        "DOUBLE",
        "BOOLEAN",
        "DATE",
        "TIMESTAMP",
        "TIME",
    ]
    .contains(&base)
}

fn aggregate_class(name: &str, distinct: bool) -> Option<AggregateClass> {
    let class = match name {
        "SUM" | "COUNT" | "MIN" | "MAX" => AggregateClass::Distributive,
        "AVG" | "STDDEV" | "STDDEV_POP" | "STDDEV_SAMP" | "VARIANCE" | "VAR_POP" | "VAR_SAMP" => {
            AggregateClass::Algebraic
        }
        "MEDIAN" | "PERCENTILE_CONT" | "PERCENTILE_DISC" => AggregateClass::Holistic,
        "APPROX_COUNT_DISTINCT" | "APPROX_PERCENTILE" => AggregateClass::Sketch,
        _ => return None,
    };
    // A distinct count needs every value: holistic, whatever the function.
    Some(if distinct && class != AggregateClass::Sketch {
        AggregateClass::Holistic
    } else {
        class
    })
}

fn is_ordered_set(name: &str) -> bool {
    matches!(name, "PERCENTILE_CONT" | "PERCENTILE_DISC")
}

/// Idempotent under duplicated rows; any `DISTINCT` aggregate also is.
const FANOUT_SAFE_AGGREGATES: &[&str] = &["MIN", "MAX", "APPROX_COUNT_DISTINCT"];

/// Ranking and offset functions; aggregates may also be windowed.
const WINDOW_FUNCTIONS: &[&str] = &[
    "ROW_NUMBER",
    "RANK",
    "DENSE_RANK",
    "NTILE",
    "PERCENT_RANK",
    "CUME_DIST",
    "LAG",
    "LEAD",
    "FIRST_VALUE",
    "LAST_VALUE",
    "NTH_VALUE",
];

/// REQUIRED and RECOMMENDED scalar functions. `EXTRACT`, `POSITION`,
/// `SUBSTRING`, `TRIM`, `CEIL`/`FLOOR` and `CAST` arrive as their own syntax.
const SCALAR_FUNCTIONS: &[&str] = &[
    // Date and time.
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "CURRENT_TIME",
    "YEAR",
    "QUARTER",
    "MONTH",
    "DAY",
    "DAYOFYEAR",
    "HOUR",
    "MINUTE",
    "SECOND",
    "DATE_PART",
    "DATE_TRUNC",
    "DATEADD",
    "DATEDIFF",
    "TO_DATE",
    "TO_TIMESTAMP",
    // Strings.
    "CONCAT",
    "LENGTH",
    "LOWER",
    "UPPER",
    "TRIM",
    "LTRIM",
    "RTRIM",
    "LEFT",
    "RIGHT",
    "SUBSTRING",
    "REPLACE",
    "SPLIT_PART",
    "POSITION",
    "CHARINDEX",
    "CONTAINS",
    "STARTSWITH",
    "ENDSWITH",
    "REGEXP_LIKE",
    "REGEXP_EXTRACT",
    "REGEXP_REPLACE",
    "REGEXP_COUNT",
    // Mathematics.
    "ABS",
    "ROUND",
    "FLOOR",
    "CEIL",
    "CEILING",
    "TRUNC",
    "TRUNCATE",
    "MOD",
    "SIGN",
    "POWER",
    "SQRT",
    "EXP",
    "LN",
    "LOG",
    "LOG10",
    "SIN",
    "COS",
    "TAN",
    "ASIN",
    "ACOS",
    "ATAN",
    "ATAN2",
    "RADIANS",
    "DEGREES",
    "PI",
    "GREATEST",
    "LEAST",
    // Conditionals.
    "IF",
    "IFF",
    "NULLIF",
    "COALESCE",
    "IFNULL",
    "NVL",
    "NVL2",
    "ZEROIFNULL",
    "NULLIFZERO",
    // Conversion.
    "TRY_CAST",
];
