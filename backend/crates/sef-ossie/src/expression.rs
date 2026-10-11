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
    BinaryOperator, CastKind, CeilFloorKind, DataType, DateTimeField, DuplicateTreatment, Expr,
    Function, FunctionArg, FunctionArgExpr, FunctionArguments, UnaryOperator, Value,
    WindowFrameBound, WindowFrameUnits, WindowSpec, WindowType,
};
use sqlparser::dialect::{Dialect, GenericDialect};
use sqlparser::keywords::Keyword;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Token, Tokenizer};

use crate::diagnostic::{Code, Diagnostic};
use crate::validate::MAX_IDENTIFIER_CHARS;

/// Maximum significant tokens, including the width of CASE and IN lists.
pub const MAX_EXPRESSION_TOKENS: usize = 4096;

/// Conservative depth budget, checked before constructing any AST. Only
/// certified shallow mappings/literal lists receive a width discount.
pub const MAX_EXPRESSION_STRUCTURAL_TOKENS: usize = 256;

/// Parser call depth, independent of the depth of left-deep operator trees.
pub const MAX_EXPRESSION_RECURSION: usize = 16;

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

/// The SQL evaluation phase of an aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregatePhase {
    /// Reduces rows to the query grain.
    Group,
    /// Operates over rows after grouping; never a grain reduction.
    Window,
}

/// One aggregate inside an expression, with its evaluation phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aggregate {
    /// Upper-cased function name.
    pub function: String,
    pub class: AggregateClass,
    pub phase: AggregatePhase,
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
/// expression; [`Code::ExpressionTooLarge`] when a resource budget is exceeded;
/// otherwise the first construct the language does not allow.
pub fn analyze(text: &str, context: Context) -> Result<Analysis, Diagnostic> {
    let expr = parse(text)?;
    let mut walker = Walker {
        context,
        analysis: Analysis::default(),
        aggregates: Vec::new(),
        inside_window: false,
    };
    walker.expr(&expr)?;
    Ok(walker.analysis)
}

fn parse(text: &str) -> Result<Expr, Diagnostic> {
    let syntax = |message: String| Diagnostic::error(Code::ExpressionSyntax, "", message);
    let dialect = GenericDialect {};
    if text.len() > crate::load::MAX_DOCUMENT_BYTES {
        return Err(too_large(crate::load::MAX_DOCUMENT_BYTES, "bytes"));
    }
    let tokens = Tokenizer::new(&dialect, text)
        .tokenize()
        .map_err(|e| syntax(format!("Expresión no válida: {e}")))?;
    let tokens: Vec<_> = tokens
        .into_iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)))
        .collect();
    if tokens.len() > MAX_EXPRESSION_TOKENS {
        return Err(too_large(MAX_EXPRESSION_TOKENS, "tokens significativos"));
    }
    check_structure_budget(&tokens)?;
    for token in &tokens {
        if let Token::Word(word) = token {
            validate_identifier(&word.value, word.quote_style)?;
        }
    }
    let mut parser = Parser::new(&dialect)
        .with_recursion_limit(MAX_EXPRESSION_RECURSION)
        .with_tokens(tokens);
    let expr = parser.parse_expr().map_err(|e| match e {
        ParserError::RecursionLimitExceeded => {
            too_large(MAX_EXPRESSION_RECURSION, "niveles de recursión del parser")
        }
        other => syntax_diagnostic(text, format!("Expresión no válida: {other}")),
    })?;
    if parser.peek_token().token != Token::EOF {
        return Err(syntax_diagnostic(
            text,
            "Después de la expresión sobra texto: una expresión es un solo valor, no varias \
             sentencias."
                .to_owned(),
        ));
    }
    Ok(expr)
}

/// `GenericDialect` may recover from a recursion error by treating CASE/NOT as
/// identifiers, then report trailing text instead. Probe only failed parses,
/// disabling that recovery. The probe never admits an expression or changes
/// the successful `GenericDialect` path, and uses the same prechecked input and
/// recursion cap. A typed resource error takes precedence over syntax.
fn syntax_diagnostic(text: &str, message: String) -> Diagnostic {
    let dialect = RecursionDiagnosticDialect;
    if let Ok(tokens) = Tokenizer::new(&GenericDialect {}, text).tokenize()
        && matches!(
            Parser::new(&dialect)
                .with_recursion_limit(MAX_EXPRESSION_RECURSION)
                .with_tokens(tokens)
                .parse_expr(),
            Err(ParserError::RecursionLimitExceeded)
        )
    {
        return too_large(MAX_EXPRESSION_RECURSION, "niveles de recursión del parser");
    }
    Diagnostic::error(Code::ExpressionSyntax, "", message)
}

#[derive(Debug)]
struct RecursionDiagnosticDialect;

impl Dialect for RecursionDiagnosticDialect {
    fn dialect(&self) -> std::any::TypeId {
        std::any::TypeId::of::<GenericDialect>()
    }

    fn is_identifier_start(&self, ch: char) -> bool {
        GenericDialect {}.is_identifier_start(ch)
    }

    fn is_identifier_part(&self, ch: char) -> bool {
        GenericDialect {}.is_identifier_part(ch)
    }

    fn is_reserved_for_identifier(&self, _keyword: Keyword) -> bool {
        true
    }
}

fn too_large(limit: usize, unit: &str) -> Diagnostic {
    Diagnostic::error(
        Code::ExpressionTooLarge,
        "",
        format!(
            "La expresión supera el límite de {limit} {unit}. Divídela en varios campos o mueve el mapeo a una tabla."
        ),
    )
}

/// Keep the original 256-token depth guarantee, discounting only complete
/// shallow forms. Replacing a flat CASE by its widest branch, or a literal IN
/// list by its widest literal, cannot decrease the longest AST path. All other
/// tokens (including malformed forms and unsupported SQL) pay their full cost.
/// No generic comma/keyword resets: those could hide deep trees in subqueries
/// or expressions using keyword identifiers. The original tokens reach the
/// SQL parser and allowlist unchanged; this is admission, not language parsing.
fn check_structure_budget(tokens: &[Token]) -> Result<(), Diagnostic> {
    let mut position = 0;
    let mut cost = 0;
    while position < tokens.len() {
        let shallow =
            shallow_mapping(tokens, position).or_else(|| literal_in_list(tokens, position));
        if let Some((end, width_cost)) = shallow {
            cost += width_cost;
            position = end;
        } else {
            cost += 1;
            position += 1;
        }
        if cost > MAX_EXPRESSION_STRUCTURAL_TOKENS {
            return Err(too_large(
                MAX_EXPRESSION_STRUCTURAL_TOKENS,
                "tokens estructurales (presupuesto de profundidad)",
            ));
        }
    }
    Ok(())
}

fn keyword(tokens: &[Token], position: usize, expected: Keyword) -> bool {
    matches!(tokens.get(position), Some(Token::Word(word))
        if word.quote_style.is_none() && word.keyword == expected)
}

/// Only leaves, signed numbers and at most two-part references. In particular,
/// no function, parenthesis, CASE, operator chain or reserved keyword identifier
/// can acquire a width discount through this recognizer.
fn atom_end(tokens: &[Token], position: usize, references: bool) -> Option<usize> {
    match tokens.get(position)? {
        Token::Number(..) | Token::SingleQuotedString(_) => Some(position + 1),
        Token::Plus | Token::Minus
            if matches!(tokens.get(position + 1), Some(Token::Number(..))) =>
        {
            Some(position + 2)
        }
        Token::Word(word)
            if word.quote_style.is_none()
                && matches!(word.keyword, Keyword::TRUE | Keyword::FALSE | Keyword::NULL) =>
        {
            Some(position + 1)
        }
        Token::Word(word)
            if references && (word.quote_style.is_some() || word.keyword == Keyword::NoKeyword) =>
        {
            if tokens.get(position + 1) == Some(&Token::Period) {
                match tokens.get(position + 2) {
                    Some(Token::Word(part))
                        if part.quote_style.is_some() || part.keyword == Keyword::NoKeyword =>
                    {
                        Some(position + 3)
                    }
                    _ => None,
                }
            } else {
                Some(position + 1)
            }
        }
        _ => None,
    }
}

fn literal_in_list(tokens: &[Token], start: usize) -> Option<(usize, usize)> {
    if !keyword(tokens, start, Keyword::IN) || tokens.get(start + 1) != Some(&Token::LParen) {
        return None;
    }
    let mut position = start + 2;
    let mut widest = 0;
    loop {
        let end = atom_end(tokens, position, false)?;
        widest = widest.max(end - position);
        position = end;
        match tokens.get(position)? {
            Token::Comma => position += 1,
            Token::RParen => return Some((position + 1, widest + 3)),
            _ => return None,
        }
    }
}

/// Certifies simple and searched mappings with atomic results and conditions
/// that are atoms or single comparisons. More complex CASE expressions remain
/// supported, but consume the full conservative token budget.
fn shallow_mapping(tokens: &[Token], start: usize) -> Option<(usize, usize)> {
    if !keyword(tokens, start, Keyword::CASE) {
        return None;
    }
    let mut position = start + 1;
    if !keyword(tokens, position, Keyword::WHEN) {
        position = atom_end(tokens, position, true)?;
    }
    let base = position - start + 1; // CASE, operand and END.
    let mut widest = 0;
    while keyword(tokens, position, Keyword::WHEN) {
        let branch = position;
        position = atom_end(tokens, position + 1, true)?;
        if matches!(
            tokens.get(position),
            Some(Token::Eq | Token::Neq | Token::Lt | Token::Gt | Token::LtEq | Token::GtEq)
        ) {
            position = atom_end(tokens, position + 1, true)?;
        }
        if !keyword(tokens, position, Keyword::THEN) {
            return None;
        }
        position = atom_end(tokens, position + 1, true)?;
        widest = widest.max(position - branch);
    }
    if widest == 0 {
        return None;
    }
    if keyword(tokens, position, Keyword::ELSE) {
        let otherwise = position;
        position = atom_end(tokens, position + 1, true)?;
        widest = widest.max(position - otherwise);
    }
    keyword(tokens, position, Keyword::END).then_some((position + 1, base + widest))
}

struct Walker {
    context: Context,
    analysis: Analysis,
    /// Active aggregate scopes, collecting dependencies at both phases.
    aggregates: Vec<Aggregate>,
    inside_window: bool,
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
        for aggregate in &mut self.aggregates {
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
            Expr::TypedString(literal)
                if !literal.uses_odbc_syntax
                    && matches!(literal.value.value, Value::SingleQuotedString(_))
                    && matches!(
                        literal.data_type,
                        DataType::Date
                            | DataType::Time(None, _)
                            | DataType::Timestamp(None, _)
                            | DataType::TimestampNtz(None)
                    ) =>
            {
                Ok(())
            }
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
            Expr::Extract { expr, field, .. } => {
                validate_date_part(&field.to_string(), EXTRACTION_PARTS)?;
                self.expr(expr)
            }
            Expr::Ceil {
                expr,
                field: CeilFloorKind::DateTimeField(DateTimeField::NoDateTime),
            }
            | Expr::Floor {
                expr,
                field: CeilFloorKind::DateTimeField(DateTimeField::NoDateTime),
            } => self.expr(expr),
            Expr::Position { expr, r#in } => {
                self.expr(expr)?;
                self.expr(r#in)
            }
            Expr::Substring {
                expr,
                substring_from,
                substring_for,
                shorthand: false,
                ..
            } => {
                if substring_from.is_none() || substring_for.is_none() {
                    return Err(unsupported("SUBSTRING exige origen, inicio y longitud"));
                }
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
                expr,
                trim_what,
                trim_characters: None,
                ..
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
        validate_signature(function, &name, &args, distinct)?;
        let window = match &function.over {
            None => None,
            Some(WindowType::WindowSpec(spec)) => Some(spec),
            Some(WindowType::NamedWindow(_)) => return Err(unsupported("Una ventana con nombre")),
        };
        let class = aggregate_class(&name, distinct);
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
            if self.inside_window || !self.aggregates.is_empty() {
                return Err(Diagnostic::error(
                    Code::NestedAggregate,
                    "",
                    "Una ventana no puede estar dentro de un agregado ni de otra ventana.",
                ));
            }
            self.analysis.windowed = true;
            self.inside_window = true;
            let result = if let Some(class) = class {
                self.aggregate(function, &name, class, distinct, &args)
            } else {
                self.arguments(&name, &args)
                    .and_then(|()| self.window_spec(spec))
            };
            self.inside_window = false;
            return result;
        }
        if let Some(class) = class {
            return self.aggregate(function, &name, class, distinct, &args);
        }
        if WINDOW_FUNCTIONS.contains(&name.as_str()) {
            return Err(unsupported(&format!("`{name}` sin `OVER (…)`")));
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
        let phase = if function.over.is_some() {
            AggregatePhase::Window
        } else {
            AggregatePhase::Group
        };
        if self
            .aggregates
            .last()
            .is_some_and(|aggregate| aggregate.phase == AggregatePhase::Group)
        {
            return Err(Diagnostic::error(
                Code::NestedAggregate,
                "",
                format!("`{name}` está dentro de otro agregado."),
            ));
        }
        let fanout_safe = distinct || FANOUT_SAFE_AGGREGATES.contains(&name);
        self.aggregates.push(Aggregate {
            function: name.to_owned(),
            class,
            phase,
            distinct,
            fanout_safe,
            references: BTreeSet::new(),
        });
        let walked = self
            .arguments(name, args)
            .and_then(|()| {
                function
                    .within_group
                    .iter()
                    .try_for_each(|order| self.expr(&order.expr))
            })
            .and_then(|()| match &function.over {
                Some(WindowType::WindowSpec(spec)) => self.window_spec(spec),
                _ => Ok(()),
            });
        let aggregate = self.aggregates.pop();
        walked?;
        self.analysis.aggregates.extend(aggregate);
        Ok(())
    }

    fn arguments(&mut self, name: &str, args: &[&FunctionArg]) -> Result<(), Diagnostic> {
        for (index, arg) in args.iter().enumerate() {
            if index == 0 && matches!(name, "DATEADD" | "DATEDIFF" | "DATE_PART" | "DATE_TRUNC") {
                let parts = match name {
                    "DATE_PART" => EXTRACTION_PARTS,
                    _ => ARITHMETIC_PARTS,
                };
                let part = match arg {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Identifier(ident)))
                        if matches!(name, "DATEADD" | "DATEDIFF")
                            && ident.quote_style.is_none() =>
                    {
                        &ident.value
                    }
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(Expr::Value(value))) => {
                        match &value.value {
                            Value::SingleQuotedString(part) => part,
                            _ => return Err(unsupported("La unidad temporal debe ser constante")),
                        }
                    }
                    _ => return Err(unsupported("La unidad temporal debe ser constante")),
                };
                validate_date_part(part, parts)?;
                continue;
            }
            match arg {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => self.expr(expr)?,
                FunctionArg::Unnamed(FunctionArgExpr::Wildcard) if name == "COUNT" => {}
                _ => return Err(unsupported(&format!("Ese argumento de `{name}`"))),
            }
        }
        Ok(())
    }

    fn window_spec(&mut self, spec: &WindowSpec) -> Result<(), Diagnostic> {
        if spec.window_name.is_some() {
            return Err(unsupported("Una ventana heredada con nombre"));
        }
        self.all(&spec.partition_by)?;
        for order in &spec.order_by {
            self.expr(&order.expr)?;
        }
        if let Some(frame) = &spec.window_frame {
            if !matches!(
                frame.units,
                WindowFrameUnits::Rows | WindowFrameUnits::Range
            ) {
                return Err(unsupported("La unidad de marco GROUPS"));
            }
            self.window_bound(&frame.start_bound)?;
            if let Some(end) = &frame.end_bound {
                self.window_bound(end)?;
            }
            let end = frame
                .end_bound
                .as_ref()
                .unwrap_or(&WindowFrameBound::CurrentRow);
            if matches!(frame.start_bound, WindowFrameBound::Following(None))
                || matches!(end, WindowFrameBound::Preceding(None))
                || bound_order(&frame.start_bound) > bound_order(end)
            {
                return Err(unsupported("Los límites del marco están invertidos"));
            }
        }
        Ok(())
    }

    fn window_bound(&mut self, bound: &WindowFrameBound) -> Result<(), Diagnostic> {
        if let WindowFrameBound::Preceding(Some(expr)) | WindowFrameBound::Following(Some(expr)) =
            bound
        {
            // Visit first so forbidden nodes cannot hide in a frame offset.
            self.expr(expr)?;
            if !matches!(expr.as_ref(), Expr::Value(value) if matches!(&value.value, Value::Number(n, false) if n.parse::<u64>().is_ok()))
            {
                return Err(unsupported(
                    "El desplazamiento del marco debe ser un entero no negativo",
                ));
            }
        }
        Ok(())
    }
}

fn normalize(ident: &sqlparser::ast::Ident) -> String {
    match ident.quote_style {
        // A quoted identifier keeps its exact spelling.
        Some('"') => ident.value.clone(),
        _ => ident.value.to_uppercase(),
    }
}

fn validate_identifier(value: &str, quote: Option<char>) -> Result<(), Diagnostic> {
    let valid = match quote {
        Some('"') => !value.is_empty(),
        None => {
            let mut chars = value.chars();
            chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
                && chars.all(|c| c.is_alphanumeric() || c == '_')
        }
        _ => {
            return Err(unsupported(
                "Un identificador delimitado sin comillas dobles ANSI",
            ));
        }
    };
    if !valid || value.chars().count() > MAX_IDENTIFIER_CHARS {
        return Err(Diagnostic::error(
            Code::InvalidIdentifier,
            "",
            "Cada parte debe ser un identificador ANSI de 1 a 128 caracteres.",
        ));
    }
    Ok(())
}

fn bound_order(bound: &WindowFrameBound) -> i128 {
    match bound {
        WindowFrameBound::CurrentRow => 0,
        WindowFrameBound::Preceding(None) => i128::MIN,
        WindowFrameBound::Following(None) => i128::MAX,
        WindowFrameBound::Preceding(Some(expr)) | WindowFrameBound::Following(Some(expr)) => {
            let n = match expr.as_ref() {
                Expr::Value(value) => match &value.value {
                    Value::Number(n, _) => i128::from(n.parse::<u64>().unwrap_or_default()),
                    _ => 0,
                },
                _ => 0,
            };
            if matches!(bound, WindowFrameBound::Preceding(_)) {
                -n
            } else {
                n
            }
        }
    }
}

const ARITHMETIC_PARTS: &[&str] = &[
    "YEAR", "QUARTER", "MONTH", "WEEK", "DAY", "HOUR", "MINUTE", "SECOND",
];
const EXTRACTION_PARTS: &[&str] = &[
    "YEAR",
    "QUARTER",
    "MONTH",
    "WEEK",
    "DAY",
    "DAYOFWEEK",
    "DAYOFYEAR",
    "HOUR",
    "MINUTE",
    "SECOND",
    "MILLISECOND",
];

fn validate_date_part(part: &str, allowed: &[&str]) -> Result<(), Diagnostic> {
    if allowed.contains(&part.to_uppercase().as_str()) {
        Ok(())
    } else {
        Err(unsupported(&format!("La unidad temporal `{part}`")))
    }
}

fn validate_signature(
    function: &Function,
    name: &str,
    args: &[&FunctionArg],
    distinct: bool,
) -> Result<(), Diagnostic> {
    let (min, max) = arity(name).ok_or_else(|| unknown_function(name))?;
    if !(min..=max).contains(&args.len()) {
        return Err(unsupported(&format!("La aridad de `{name}`")));
    }
    if distinct && !matches!(name, "SUM" | "COUNT") {
        return Err(unsupported(&format!("DISTINCT en `{name}`")));
    }
    for arg in args {
        match arg {
            FunctionArg::Unnamed(FunctionArgExpr::Expr(_)) => {}
            FunctionArg::Unnamed(FunctionArgExpr::Wildcard) if name == "COUNT" && !distinct => {}
            _ => return Err(unsupported(&format!("Ese argumento de `{name}`"))),
        }
    }
    if is_ordered_set(name) {
        if function.within_group.len() != 1 {
            return Err(unsupported(
                "Un percentil exige WITHIN GROUP con una expresión de orden",
            ));
        }
    } else if !function.within_group.is_empty() {
        return Err(unsupported(&format!("WITHIN GROUP en `{name}`")));
    }
    Ok(())
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
        "TIMESTAMP_NTZ",
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

/// Portable signatures. Optional SQL arguments are bounded; vendor format
/// models for `TO_DATE`/`TO_TIMESTAMP` remain outside this language.
fn arity(name: &str) -> Option<(usize, usize)> {
    Some(match name {
        "CURRENT_DATE" | "CURRENT_TIMESTAMP" | "CURRENT_TIME" | "PI" | "ROW_NUMBER" | "RANK"
        | "DENSE_RANK" | "PERCENT_RANK" | "CUME_DIST" => (0, 0),
        "SUM"
        | "COUNT"
        | "AVG"
        | "MIN"
        | "MAX"
        | "STDDEV"
        | "STDDEV_POP"
        | "STDDEV_SAMP"
        | "VARIANCE"
        | "VAR_POP"
        | "VAR_SAMP"
        | "MEDIAN"
        | "PERCENTILE_CONT"
        | "PERCENTILE_DISC"
        | "APPROX_COUNT_DISTINCT"
        | "YEAR"
        | "QUARTER"
        | "MONTH"
        | "DAY"
        | "DAYOFYEAR"
        | "HOUR"
        | "MINUTE"
        | "SECOND"
        | "TO_DATE"
        | "TO_TIMESTAMP"
        | "LENGTH"
        | "LOWER"
        | "UPPER"
        | "TRIM"
        | "LTRIM"
        | "RTRIM"
        | "ABS"
        | "FLOOR"
        | "CEIL"
        | "CEILING"
        | "SIGN"
        | "SQRT"
        | "EXP"
        | "LN"
        | "LOG10"
        | "SIN"
        | "COS"
        | "TAN"
        | "ASIN"
        | "ACOS"
        | "ATAN"
        | "RADIANS"
        | "DEGREES"
        | "ZEROIFNULL"
        | "NULLIFZERO"
        | "NTILE"
        | "FIRST_VALUE"
        | "LAST_VALUE" => (1, 1),
        "APPROX_PERCENTILE" | "DATE_PART" | "DATE_TRUNC" | "LEFT" | "RIGHT" | "POSITION"
        | "CHARINDEX" | "CONTAINS" | "STARTSWITH" | "ENDSWITH" | "REGEXP_LIKE"
        | "REGEXP_EXTRACT" | "REGEXP_COUNT" | "MOD" | "POWER" | "LOG" | "ATAN2" | "NULLIF"
        | "IFNULL" | "NVL" | "NTH_VALUE" => (2, 2),
        "DATEADD" | "DATEDIFF" | "SUBSTRING" | "REPLACE" | "SPLIT_PART" | "REGEXP_REPLACE"
        | "IF" | "IFF" | "NVL2" => (3, 3),
        "ROUND" | "TRUNC" | "TRUNCATE" => (1, 2),
        "LAG" | "LEAD" => (1, 3),
        "CONCAT" | "GREATEST" | "LEAST" | "COALESCE" => (2, usize::MAX),
        _ => return None,
    })
}
