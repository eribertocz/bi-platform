//! Ossie metrics → CSMIR measures and metrics.
//!
//! - A metric that is one aggregate (`SUM(sales.amount)`) becomes a
//!   **measure** of the dataset it reads plus the metric exposing it, both
//!   with the metric's name, as the SEF corpus models them.
//! - Any other metric combines metrics (`gross_revenue - refunds`) and
//!   inline aggregates; each inline aggregate becomes its own measure and
//!   metric, `<metric>.m1`, `<metric>.m2`… in reading order, as `MetricFlow`
//!   generates `<metric>_numerator`/`_denominator` sub-metrics.
//! - A division states its result type: `CAST(a / b AS DECIMAL(p, s))` is
//!   an exact ratio rounded half-even (or as the extension says), with
//!   `NULL` for a zero denominator unless the extension asks for an error.
//!   `AVG(x)` is the ratio of `SUM(x)` and `COUNT(x)`.
//! - Sums and differences of fully additive metrics stay fully additive;
//!   anything else is recomputed from its components, never re-aggregated.
//! - The native grain is the one all inputs share, or the declared one.

use std::collections::{BTreeMap, BTreeSet};

use sef_model_ir::expressions::{
    AggregateFunction, AggregateNullSemantics, MetricExpr, MetricExprKind, ScalarExpr,
};
use sef_model_ir::grain::Grain;
use sef_model_ir::identity::StableId;
use sef_model_ir::model::{
    AggregationClass, AggregationContract, AxisBehavior, AxisRule, Decomposition, Measure, Metric,
    ObjectMetadata,
};
use sef_model_ir::time::IanaZoneId;
use sef_model_ir::types::{ArithmeticOp, RoundingMode, ValueType, ZeroDivisionPolicy};
use sef_model_ir::units::Unit;
use sqlparser::ast::{
    BinaryOperator, CastKind, DataType, DuplicateTreatment, Expr, Function, FunctionArg,
    FunctionArgExpr, FunctionArguments,
};

use super::scalar::{
    self, Division, Failure, Names, Translator, arguments, cast_target, describe, division,
    function_name, normalize, not_representable,
};
use super::{Converter, compatible, field_ref, strip, unit};
use crate::diagnostic::Code;
use crate::extension::{
    self, AggregationDeclaration, BehaviorDeclaration, ClassDeclaration, MetricExtension,
    RoundingDeclaration, ZeroDivisionDeclaration,
};
use crate::ids;

/// What other metrics need to know about a converted one.
#[derive(Debug, Clone)]
struct Info {
    id: StableId,
    value_type: ValueType,
    unit: Option<Unit>,
    grain: Grain,
    class: AggregationClass,
}

struct MetricState {
    segment: String,
    description: Option<String>,
    path: String,
    expression_path: String,
    extension: MetricExtension,
    ast: Option<Expr>,
}

pub(super) fn metrics(converter: &mut Converter<'_>) {
    let model = converter.model;
    let mut states = Vec::with_capacity(model.metrics.len());
    let mut names = BTreeMap::new();
    for (i, metric) in model.metrics.iter().enumerate() {
        let path = format!("/metrics/{i}");
        let extension = match extension::read(&metric.custom_extensions, &path) {
            Ok(extension) => extension,
            Err(diagnostic) => {
                converter.found.push(diagnostic);
                MetricExtension::default()
            }
        };
        let (ast, expression_path) =
            converter.portable(&metric.expression, &format!("{path}/expression"));
        names.insert(crate::validate::normalize_identifier(&metric.name), i);
        states.push(MetricState {
            segment: ids::segment(&metric.name),
            description: metric.description.clone(),
            path,
            expression_path,
            extension,
            ast,
        });
    }
    let Some(order) = order(converter, &states, &names) else {
        return;
    };
    let mut by_name: BTreeMap<String, Info> = BTreeMap::new();
    let mut by_id: BTreeMap<StableId, Info> = BTreeMap::new();
    for i in order {
        let state = &states[i];
        let Some(ast) = state.ast.as_ref() else {
            continue;
        };
        let mut builder = Builder {
            converter: &mut *converter,
            metric: state,
            names: &names,
            by_name: &by_name,
            by_id: &mut by_id,
            synthesized: 0,
        };
        match builder.build(ast) {
            Ok(info) => {
                let datatype = model.metrics[i].datatype;
                if let Some(datatype) = datatype
                    && !compatible(datatype, &info.value_type)
                {
                    converter.error(
                        Code::DatatypeMismatch,
                        format!("{}/datatype", state.path),
                        format!(
                            "La métrica dice {datatype:?}, pero su valor es {}.",
                            describe(&info.value_type)
                        ),
                    );
                }
                let name = crate::validate::normalize_identifier(&model.metrics[i].name);
                by_id.insert(info.id.clone(), info.clone());
                by_name.insert(name, info);
            }
            Err(failure) => converter.fail(&state.expression_path, failure),
        }
    }
}

/// Metrics in dependency order; `None` (with diagnostics) on a cycle.
fn order(
    converter: &mut Converter<'_>,
    states: &[MetricState],
    names: &BTreeMap<String, usize>,
) -> Option<Vec<usize>> {
    let dependencies: Vec<BTreeSet<usize>> = states
        .iter()
        .map(|state| {
            let mut found = BTreeSet::new();
            if let Some(ast) = &state.ast {
                metric_references(ast, names, &mut found);
            }
            found
        })
        .collect();
    let mut done = vec![false; states.len()];
    let mut order = Vec::with_capacity(states.len());
    while order.len() < states.len() {
        let ready =
            (0..states.len()).find(|&i| !done[i] && dependencies[i].iter().all(|&d| done[d]));
        let Some(next) = ready else {
            for (i, state) in states.iter().enumerate().filter(|(i, _)| !done[*i]) {
                let _ = i;
                converter.error(
                    Code::ReferenceCycle,
                    state.expression_path.clone(),
                    "Esta métrica depende de sí misma a través de otras métricas.",
                );
            }
            return None;
        };
        done[next] = true;
        order.push(next);
    }
    Some(order)
}

/// Unqualified names outside aggregates that name metrics.
fn metric_references(expr: &Expr, names: &BTreeMap<String, usize>, found: &mut BTreeSet<usize>) {
    let mut stack = vec![expr];
    while let Some(expr) = stack.pop() {
        match expr {
            Expr::Identifier(ident) => {
                if let Some(&index) = names.get(&normalize(ident)) {
                    found.insert(index);
                }
            }
            Expr::Nested(inner)
            | Expr::UnaryOp { expr: inner, .. }
            | Expr::Cast { expr: inner, .. } => stack.push(inner),
            Expr::BinaryOp { left, right, .. } => {
                stack.push(left);
                stack.push(right);
            }
            Expr::Function(function) if !is_aggregate(function) || function.over.is_some() => {
                if let FunctionArguments::List(list) = &function.args {
                    for arg in &list.args {
                        if let FunctionArg::Unnamed(FunctionArgExpr::Expr(inner)) = arg {
                            stack.push(inner);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn is_aggregate(function: &Function) -> bool {
    matches!(
        function_name(function).as_str(),
        "SUM"
            | "COUNT"
            | "MIN"
            | "MAX"
            | "AVG"
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
            | "APPROX_PERCENTILE"
    )
}

struct Builder<'b, 'a> {
    converter: &'b mut Converter<'a>,
    metric: &'b MetricState,
    names: &'b BTreeMap<String, usize>,
    by_name: &'b BTreeMap<String, Info>,
    by_id: &'b mut BTreeMap<StableId, Info>,
    synthesized: usize,
}

/// A measure just created, as the metric exposing it sees it.
struct MeasureInfo {
    id: StableId,
    value_type: ValueType,
    unit: Option<Unit>,
    grain: Grain,
    contract: AggregationContract,
}

impl Builder<'_, '_> {
    fn build(&mut self, ast: &Expr) -> Result<Info, Failure> {
        let segment = self.metric.segment.clone();
        let top = strip(ast);
        if let Expr::Function(function) = top
            && is_aggregate(function)
            && function.over.is_none()
            && function_name(function) != "AVG"
        {
            if self.metric.extension.grain.is_some() {
                return Err(Failure::new(
                    Code::InvalidExtension,
                    "\"grain\" se declara en métricas compuestas: el grano de una métrica de un \
                     solo agregado es el de su dataset.",
                )
                .at(format!("{}/custom_extensions", self.metric.path)));
            }
            let declared = self.metric.extension.aggregation.clone();
            let measure = self.measure(&segment, function, declared.as_ref())?;
            let unit = self.metric.extension.unit.as_ref().map(unit);
            return self.measured_metric(&segment, measure, unit);
        }
        let mut expression = self.expr(top)?;
        if let Some(declared) = &self.metric.extension.unit {
            expression.unit = Some(unit(declared));
        }
        let components: Vec<StableId> = expression
            .metric_refs()
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let grain = self.composite_grain(&components)?;
        let contract = self.composite_contract(&expression, &components)?;
        let dependencies = expression
            .measure_refs()
            .into_iter()
            .chain(expression.metric_refs())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let metadata = self.metadata("metric", &segment)?;
        let info = Info {
            id: metadata.id().clone(),
            value_type: expression.value_type.clone(),
            unit: expression.unit.clone(),
            grain: grain.clone(),
            class: contract.classification,
        };
        self.converter.metrics.push(Metric {
            metadata,
            value_type: expression.value_type.clone(),
            unit: expression.unit.clone(),
            expression,
            intrinsic_filter: None,
            native_grain: grain,
            aggregation_contract: contract,
            time_behavior: None,
            window_behavior: None,
            dependencies,
            policy_refs: Vec::new(),
            allocation_bridge: None,
        });
        Ok(info)
    }

    /// The grain all inputs share, or the declared one.
    fn composite_grain(&self, components: &[StableId]) -> Result<Grain, Failure> {
        let path = format!("{}/custom_extensions", self.metric.path);
        if let Some(declared) = &self.metric.extension.grain {
            let atoms = declared
                .dimensions
                .iter()
                .map(|name| self.converter.atom(Some(name), None))
                .chain(
                    declared
                        .entities
                        .iter()
                        .map(|name| self.converter.atom(None, Some(name))),
                )
                .collect::<Result<Vec<_>, _>>()
                .map_err(|failure| failure.at(path))?;
            return Ok(Grain::new(atoms));
        }
        let mut grains = components
            .iter()
            .filter_map(|id| self.by_id.get(id))
            .map(|info| &info.grain);
        let first = grains.next().cloned().unwrap_or_default();
        if grains.any(|grain| *grain != first) {
            return Err(Failure::new(
                Code::GrainRequired,
                "Las métricas que combina tienen granos distintos: declara el grano de esta en                  la extensión (\"grain\": {\"dimensions\": […]}).",
            ));
        }
        Ok(first)
    }

    /// A combination is recomputed from its components unless the author
    /// declares it fully additive, which is only accepted for sums and
    /// differences of fully additive metrics: `a + b` summed per group
    /// differs from the total when a group leaves a component NULL.
    fn composite_contract(
        &self,
        expression: &MetricExpr,
        components: &[StableId],
    ) -> Result<AggregationContract, Failure> {
        let Some(declared) = &self.metric.extension.aggregation else {
            return Ok(AggregationContract {
                classification: AggregationClass::Recomputable,
                axis_rules: Vec::new(),
                decomposition: Some(Decomposition {
                    components: components.to_vec(),
                    recomposition: expression.clone(),
                }),
            });
        };
        let summable = components.iter().all(|id| {
            self.by_id
                .get(id)
                .is_some_and(|info| info.class == AggregationClass::FullyAdditive)
        });
        if declared.class == ClassDeclaration::FullyAdditive
            && declared.axes.is_empty()
            && only_sums(expression)
            && summable
        {
            return Ok(AggregationContract::fully_additive());
        }
        Err(Failure::new(
            Code::InvalidExtension,
            "Una métrica compuesta solo puede declararse fully_additive, y solo si suma o resta              métricas totalmente aditivas.",
        )
        .at(format!("{}/custom_extensions", self.metric.path)))
    }

    fn metadata(&mut self, kind: &str, segment: &str) -> Result<ObjectMetadata, Failure> {
        let path = self.metric.path.clone();
        let mut metadata = self
            .converter
            .metadata(kind, segment, &path)
            .ok_or_else(|| {
                Failure::new(Code::InvalidIdentifier, "Nombre sin identificador estable.")
            })?;
        if segment == self.metric.segment {
            metadata.description.clone_from(&self.metric.description);
        }
        Ok(metadata)
    }

    fn measured_metric(
        &mut self,
        segment: &str,
        measure: MeasureInfo,
        unit: Option<Unit>,
    ) -> Result<Info, Failure> {
        let unit = unit.or(measure.unit);
        let metadata = self.metadata("metric", segment)?;
        let info = Info {
            id: metadata.id().clone(),
            value_type: measure.value_type.clone(),
            unit: unit.clone(),
            grain: measure.grain.clone(),
            class: measure.contract.classification,
        };
        self.converter.metrics.push(Metric {
            metadata,
            expression: MetricExpr {
                kind: MetricExprKind::MeasureRef(measure.id.clone()),
                value_type: measure.value_type.clone(),
                unit: unit.clone(),
            },
            intrinsic_filter: None,
            value_type: measure.value_type,
            unit,
            native_grain: measure.grain,
            aggregation_contract: measure.contract,
            time_behavior: None,
            window_behavior: None,
            dependencies: vec![measure.id],
            policy_refs: Vec::new(),
            allocation_bridge: None,
        });
        Ok(info)
    }

    /// An inline aggregate becomes `<metric>.m<k>`.
    fn synthesize(&mut self, function: &Function) -> Result<MetricExpr, Failure> {
        self.synthesized += 1;
        let segment = format!("{}.m{}", self.metric.segment, self.synthesized);
        let measure = self.measure(&segment, function, None)?;
        let info = self.measured_metric(&segment, measure, None)?;
        let reference = metric_ref(&info);
        self.by_id.insert(info.id.clone(), info);
        Ok(reference)
    }

    fn expr(&mut self, expr: &Expr) -> Result<MetricExpr, Failure> {
        match expr {
            Expr::Nested(inner) => self.expr(inner),
            Expr::Identifier(ident) => {
                let name = normalize(ident);
                if let Some(info) = self.by_name.get(&name) {
                    return Ok(metric_ref(info));
                }
                if self.names.contains_key(&name) {
                    return Err(Failure::new(
                        Code::UnknownReference,
                        format!(
                            "La métrica {:?} tiene errores; corrígela primero.",
                            ident.value
                        ),
                    ));
                }
                Err(Failure::new(
                    Code::UnknownReference,
                    format!(
                        "{:?} no es una métrica. Fuera de un agregado solo se nombran métricas; \
                         los campos van dentro de SUM(…), COUNT(…)…",
                        ident.value
                    ),
                ))
            }
            Expr::Function(function) if function_name(function) == "AVG" => {
                self.average(function, None)
            }
            Expr::Function(function) if is_aggregate(function) && function.over.is_none() => {
                self.synthesize(function)
            }
            Expr::BinaryOp { left, op, right } => self.binary(left, op, right),
            Expr::Cast {
                kind: CastKind::Cast,
                expr: inner,
                data_type,
                format: None,
                ..
            } => self.cast(inner, data_type),
            Expr::Value(_) => Err(not_representable(
                "Una constante entre métricas (CSMIR combina métricas, no literales)",
            )),
            other => Err(not_representable(&format!("`{other}` en una métrica"))),
        }
    }

    /// `+`, `-` and `*` between metrics; `/` only as a typed ratio.
    fn binary(
        &mut self,
        left: &Expr,
        op: &BinaryOperator,
        right: &Expr,
    ) -> Result<MetricExpr, Failure> {
        let op = match op {
            BinaryOperator::Plus => ArithmeticOp::Add,
            BinaryOperator::Minus => ArithmeticOp::Subtract,
            BinaryOperator::Multiply => ArithmeticOp::Multiply,
            BinaryOperator::Divide => {
                let numerator = self.expr(left)?;
                let denominator = self.expr(right)?;
                return self.ratio(numerator, denominator, None);
            }
            other => {
                return Err(not_representable(&format!(
                    "El operador `{other}` entre métricas"
                )));
            }
        };
        let left = self.expr(left)?;
        let right = self.expr(right)?;
        let value_type = scalar::arithmetic_type(op, &left.value_type, &right.value_type)?;
        let unit = match op {
            ArithmeticOp::Add | ArithmeticOp::Subtract => left.unit.clone(),
            _ => None,
        };
        Ok(MetricExpr {
            kind: MetricExprKind::Arithmetic {
                op,
                left: Box::new(left),
                right: Box::new(right),
                contract: sef_model_ir::types::NumericContract::default(),
            },
            value_type,
            unit,
        })
    }

    /// `CAST(a / b AS T)` and `CAST(AVG(x) AS T)` are ratios; any other
    /// cast converts a metric's value.
    fn cast(&mut self, inner: &Expr, data_type: &DataType) -> Result<MetricExpr, Failure> {
        let target = cast_target(data_type)?;
        if let Some((numerator, denominator)) = division(inner) {
            let numerator = self.expr(numerator)?;
            let denominator = self.expr(denominator)?;
            return self.ratio(numerator, denominator, Some(target));
        }
        if let Expr::Function(function) = strip(inner)
            && function_name(function) == "AVG"
        {
            return self.average(function, Some(target));
        }
        let operand = self.expr(inner)?;
        let rounding = match (&operand.value_type, &target) {
            (ValueType::Decimal(from), ValueType::Decimal(to)) if to.scale() < from.scale() => {
                Some(self.rounding())
            }
            (ValueType::Float { .. }, ValueType::Decimal(_) | ValueType::Integer { .. }) => {
                Some(self.rounding())
            }
            _ => None,
        };
        Ok(MetricExpr {
            unit: operand.unit.clone(),
            kind: MetricExprKind::ExplicitCast {
                operand: Box::new(operand),
                rounding,
            },
            value_type: target,
        })
    }

    /// The aggregation contract of a measure: declared, or the default of its
    /// aggregate. Only `SUM` and `COUNT` re-aggregate by summing partial
    /// results (SEF's additive partition property); `MIN`/`MAX` and exact
    /// distinct counts are recomputed from rows at every grain.
    fn contract(
        &self,
        aggregation: AggregateFunction,
        declared: Option<&AggregationDeclaration>,
    ) -> Result<AggregationContract, Failure> {
        let summable = matches!(
            aggregation,
            AggregateFunction::Sum | AggregateFunction::CountRows | AggregateFunction::CountNonNull
        );
        let Some(declared) = declared else {
            return Ok(if summable {
                AggregationContract::fully_additive()
            } else {
                AggregationContract {
                    classification: AggregationClass::NonAdditive,
                    axis_rules: Vec::new(),
                    decomposition: None,
                }
            });
        };
        let path = format!("{}/custom_extensions", self.metric.path);
        if declared.class != ClassDeclaration::NonAdditive && !summable {
            return Err(Failure::new(
                Code::InvalidExtension,
                "Solo SUM y COUNT se suman entre grupos; MIN, MAX y COUNT(DISTINCT) son \
                 non_additive.",
            )
            .at(path));
        }
        let axis_rules = declared
            .axes
            .iter()
            .map(|axis| {
                Ok(AxisRule {
                    axis: self
                        .converter
                        .atom(axis.dimension.as_deref(), axis.entity.as_deref())?,
                    behavior: match axis.behavior {
                        BehaviorDeclaration::Additive => AxisBehavior::Additive,
                        BehaviorDeclaration::Recompute => AxisBehavior::Recompute,
                        BehaviorDeclaration::DistinctRecompute => AxisBehavior::DistinctRecompute,
                        BehaviorDeclaration::PeriodEnd => AxisBehavior::PeriodEnd,
                        BehaviorDeclaration::PeriodStart => AxisBehavior::PeriodStart,
                        BehaviorDeclaration::Forbidden => AxisBehavior::Forbidden,
                    },
                })
            })
            .collect::<Result<Vec<_>, Failure>>()
            .map_err(|failure| failure.at(path.clone()))?;
        Ok(AggregationContract {
            classification: match declared.class {
                ClassDeclaration::FullyAdditive => AggregationClass::FullyAdditive,
                ClassDeclaration::SemiAdditive => AggregationClass::SemiAdditive,
                ClassDeclaration::NonAdditive => AggregationClass::NonAdditive,
            },
            axis_rules,
            decomposition: None,
        })
    }

    fn rounding(&self) -> RoundingMode {
        match self.metric.extension.rounding {
            None | Some(RoundingDeclaration::HalfEven) => RoundingMode::HalfEven,
            Some(RoundingDeclaration::HalfUp) => RoundingMode::HalfUp,
            Some(RoundingDeclaration::TowardZero) => RoundingMode::TowardZero,
            Some(RoundingDeclaration::Floor) => RoundingMode::Floor,
            Some(RoundingDeclaration::Ceiling) => RoundingMode::Ceiling,
        }
    }

    fn zero(&self) -> ZeroDivisionPolicy {
        match self.metric.extension.zero_division {
            None | Some(ZeroDivisionDeclaration::Null) => ZeroDivisionPolicy::Null,
            Some(ZeroDivisionDeclaration::Error) => ZeroDivisionPolicy::Error,
        }
    }

    /// `numerator / denominator` at `target`: exact decimal or float.
    fn ratio(
        &mut self,
        numerator: MetricExpr,
        denominator: MetricExpr,
        target: Option<ValueType>,
    ) -> Result<MetricExpr, Failure> {
        let target = match target {
            Some(target) => target,
            None if matches!(numerator.value_type, ValueType::Float { .. })
                && numerator.value_type == denominator.value_type =>
            {
                numerator.value_type.clone()
            }
            None => {
                return Err(Failure::new(
                    Code::TypeMismatch,
                    "Una división necesita el tipo exacto de su resultado: escribe \
                     CAST(a / b AS DECIMAL(p, s)).",
                ));
            }
        };
        let rounding = match &target {
            ValueType::Decimal(_) => Some(self.rounding()),
            ValueType::Float { .. } => None,
            _ => {
                return Err(Failure::new(
                    Code::TypeMismatch,
                    "Una división da DECIMAL(p, s) o un número de coma flotante.",
                ));
            }
        };
        let numerator = widen(numerator, &target)?;
        let denominator = widen(denominator, &target)?;
        Ok(MetricExpr {
            kind: MetricExprKind::Ratio {
                numerator: Box::new(numerator),
                denominator: Box::new(denominator),
                zero_denominator: self.zero(),
                rounding,
            },
            value_type: target,
            unit: None,
        })
    }

    /// `AVG(x)`: `SUM(x) / COUNT(x)`, two synthesized metrics.
    fn average(
        &mut self,
        function: &Function,
        target: Option<ValueType>,
    ) -> Result<MetricExpr, Failure> {
        if function.over.is_some() {
            return Err(not_representable("AVG con OVER"));
        }
        let mut sum = function.clone();
        sum.name = sqlparser::ast::ObjectName::from(vec![sqlparser::ast::Ident::new("SUM")]);
        let mut count = function.clone();
        count.name = sqlparser::ast::ObjectName::from(vec![sqlparser::ast::Ident::new("COUNT")]);
        let numerator = self.synthesize(&sum)?;
        let denominator = self.synthesize(&count)?;
        self.ratio(numerator, denominator, target)
    }

    /// The one dataset an aggregate reads, or the declared one for `COUNT(*)`.
    fn measure_dataset(&self, used: &BTreeSet<usize>, name: &str) -> Result<usize, Failure> {
        match (used.len(), used.first(), &self.metric.extension.dataset) {
            (1, Some(&d), _) => Ok(d),
            (0, _, Some(declared)) => self.converter.dataset_index(declared).ok_or_else(|| {
                Failure::new(
                    Code::UnknownReference,
                    format!("La extensión nombra el dataset {declared:?}, que no existe."),
                )
            }),
            (0, ..) => Err(Failure::new(
                Code::UnknownReference,
                format!(
                    "{name}(*) no dice qué filas cuenta: indica el dataset en la extensión                      (\"dataset\": \"…\")."
                ),
            )),
            _ => Err(Failure::new(
                Code::NotRepresentable,
                "Un agregado lee un solo dataset; combina varios con métricas separadas.",
            )),
        }
    }

    fn measure(
        &mut self,
        segment: &str,
        function: &Function,
        declared: Option<&AggregationDeclaration>,
    ) -> Result<MeasureInfo, Failure> {
        let name = function_name(function);
        let (distinct, wildcard) = match &function.args {
            FunctionArguments::List(list) => (
                matches!(list.duplicate_treatment, Some(DuplicateTreatment::Distinct)),
                matches!(
                    list.args.as_slice(),
                    [FunctionArg::Unnamed(FunctionArgExpr::Wildcard)]
                ),
            ),
            _ => (false, false),
        };
        let (aggregation, null_semantics) = aggregation(&name, distinct, wildcard)?;
        let mut scope = MetricScope {
            converter: &mut *self.converter,
            used: BTreeSet::new(),
        };
        let scalar = if wildcard {
            None
        } else {
            let args = arguments(function)?;
            let [argument] = args.as_slice() else {
                return Err(not_representable(&format!("{name} con varios argumentos")));
            };
            Some(
                Translator {
                    names: &mut scope,
                    division: Division::default(),
                }
                .translate(argument)?,
            )
        };
        let used = scope.used;
        let d = self.measure_dataset(&used, &name)?;
        let (value_type, unit) = measure_type(aggregation, scalar.as_ref(), &name)?;
        let contract = self.contract(aggregation, declared)?;
        let grain = self.converter.datasets[d].grain.clone();
        let metadata = self.metadata("measure", segment)?;
        let id = metadata.id().clone();
        self.converter.measures.push(Measure {
            metadata,
            dataset: self.converter.datasets[d].id.clone(),
            scalar_expression: scalar,
            aggregation,
            intrinsic_filter: None,
            value_type: value_type.clone(),
            unit: unit.clone(),
            input_grain: grain.clone(),
            aggregation_contract: contract.clone(),
            null_semantics,
        });
        Ok(MeasureInfo {
            id,
            value_type,
            unit,
            grain,
            contract,
        })
    }
}

/// Only `+`, `-` and metric references.
fn only_sums(expression: &MetricExpr) -> bool {
    match &expression.kind {
        MetricExprKind::MetricRef(_) | MetricExprKind::MeasureRef(_) => true,
        MetricExprKind::Arithmetic {
            op: ArithmeticOp::Add | ArithmeticOp::Subtract,
            left,
            right,
            ..
        } => only_sums(left) && only_sums(right),
        _ => false,
    }
}

fn metric_ref(info: &Info) -> MetricExpr {
    MetricExpr {
        kind: MetricExprKind::MetricRef(info.id.clone()),
        value_type: info.value_type.clone(),
        unit: info.unit.clone(),
    }
}

/// A ratio operand at the ratio's kind: integers widen to `DECIMAL(20,0)`.
fn widen(operand: MetricExpr, target: &ValueType) -> Result<MetricExpr, Failure> {
    match (&operand.value_type, target) {
        (ValueType::Decimal(_), ValueType::Decimal(_))
        | (ValueType::Float { .. }, ValueType::Float { .. }) => Ok(operand),
        (ValueType::Integer { .. }, ValueType::Decimal(_)) => Ok(MetricExpr {
            value_type: scalar::wide_integer_decimal()?,
            unit: operand.unit.clone(),
            kind: MetricExprKind::ExplicitCast {
                operand: Box::new(operand),
                rounding: None,
            },
        }),
        _ => Err(Failure::new(
            Code::TypeMismatch,
            format!(
                "No se puede dividir {} para obtener {}; añade CAST.",
                describe(&operand.value_type),
                describe(target)
            ),
        )),
    }
}

/// Names inside an aggregate: `dataset.field` (or a field name unique in
/// the model). A computed field is inlined.
struct MetricScope<'c, 'a> {
    converter: &'c mut Converter<'a>,
    used: BTreeSet<usize>,
}

impl Names for MetricScope<'_, '_> {
    fn reference(&mut self, qualifier: Option<&str>, name: &str) -> Result<ScalarExpr, Failure> {
        let converter = &mut *self.converter;
        let candidates: Vec<usize> = match qualifier {
            Some(qualifier) => converter
                .datasets
                .iter()
                .position(|dataset| dataset.normalized == qualifier)
                .into_iter()
                .collect(),
            None => (0..converter.datasets.len())
                .filter(|&d| {
                    converter.datasets[d]
                        .fields
                        .iter()
                        .any(|f| f.normalized == name)
                })
                .collect(),
        };
        let d = match candidates.as_slice() {
            [d] => *d,
            [] => {
                return Err(Failure::new(
                    Code::UnknownReference,
                    match qualifier {
                        Some(qualifier) => format!("No existe el dataset {qualifier:?}."),
                        None => format!("Ningún dataset tiene el campo {name:?}."),
                    },
                ));
            }
            _ => {
                return Err(Failure::new(
                    Code::AmbiguousReference,
                    format!(
                        "Varios datasets tienen el campo {name:?}: escríbelo como dataset.campo."
                    ),
                ));
            }
        };
        let state = &converter.datasets[d];
        let Some(f) = state
            .fields
            .iter()
            .position(|field| field.normalized == name)
        else {
            return Err(Failure::new(
                Code::UnknownReference,
                format!(
                    "El dataset {:?} no tiene el campo {name:?}.",
                    state.normalized
                ),
            ));
        };
        self.used.insert(d);
        let field = &state.fields[f];
        if let Some(id) = &field.column {
            let column = state
                .columns
                .iter()
                .find(|(column, _)| column.metadata.id() == id)
                .map(|(column, _)| field_ref(column));
            return column.ok_or_else(|| Failure::new(Code::UnknownColumn, "Campo sin columna."));
        }
        let ast = field
            .ast
            .clone()
            .ok_or_else(|| Failure::new(Code::UnknownReference, "El campo tiene errores."))?;
        let timezone = converter.timezone(d, f);
        converter.row_expression(d, &ast, timezone)
    }

    fn timezone(&self) -> Option<&IanaZoneId> {
        None
    }
}

/// CSMIR's aggregate and null contract for an Ossie aggregate.
fn aggregation(
    name: &str,
    distinct: bool,
    wildcard: bool,
) -> Result<(AggregateFunction, AggregateNullSemantics), Failure> {
    let ignore_nulls = AggregateNullSemantics::IgnoreNullsNullIfEmpty;
    match (name, distinct, wildcard) {
        ("COUNT", false, true) => Ok((
            AggregateFunction::CountRows,
            AggregateNullSemantics::CountRows,
        )),
        ("COUNT", true, false) => Ok((
            AggregateFunction::CountDistinct,
            AggregateNullSemantics::CountNonNull,
        )),
        ("COUNT", false, false) => Ok((
            AggregateFunction::CountNonNull,
            AggregateNullSemantics::CountNonNull,
        )),
        ("SUM", false, false) => Ok((AggregateFunction::Sum, ignore_nulls)),
        ("MIN", false, false) => Ok((AggregateFunction::Min, ignore_nulls)),
        ("MAX", false, false) => Ok((AggregateFunction::Max, ignore_nulls)),
        ("SUM", true, _) => Err(not_representable("SUM(DISTINCT …)")),
        (other, ..) => Err(not_representable(&format!(
            "El agregado {other} (holístico, algebraico o aproximado; llegará con su regla de              corrección)"
        ))),
    }
}

/// The value type and unit of a measure.
fn measure_type(
    aggregation: AggregateFunction,
    scalar: Option<&ScalarExpr>,
    name: &str,
) -> Result<(ValueType, Option<Unit>), Failure> {
    let counts = matches!(
        aggregation,
        AggregateFunction::CountRows
            | AggregateFunction::CountNonNull
            | AggregateFunction::CountDistinct
    );
    Ok(match (scalar, counts) {
        (_, true) => (
            ValueType::Integer {
                signed: true,
                bits: Some(64),
            },
            Some(Unit::Count),
        ),
        (Some(scalar), false) => {
            if aggregation == AggregateFunction::Sum && !scalar.value_type.is_numeric() {
                return Err(Failure::new(
                    Code::TypeMismatch,
                    format!(
                        "SUM necesita números y recibe {}.",
                        describe(&scalar.value_type)
                    ),
                ));
            }
            (scalar.value_type.clone(), scalar.unit.clone())
        }
        (None, false) => return Err(not_representable(&format!("{name}(*)"))),
    })
}
