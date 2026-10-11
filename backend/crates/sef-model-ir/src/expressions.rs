use crate::identity::StableId;
use crate::time::TimeBucket;
use crate::types::{
    ArithmeticOp, Literal, NumericContract, RoundingMode, ValueType, ZeroDivisionPolicy,
};
use crate::units::Unit;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Negate,
    Not,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonOp {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BooleanOp {
    And,
    Or,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateFunction {
    Sum,
    CountRows,
    CountNonNull,
    CountDistinct,
    Min,
    Max,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AggregateNullSemantics {
    IgnoreNullsNullIfEmpty,
    CountNonNull,
    CountRows,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScalarExpr {
    pub kind: ScalarExprKind,
    pub value_type: ValueType,
    pub unit: Option<Unit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScalarExprKind {
    Literal(Literal),
    FieldRef(StableId),
    DimensionRef(StableId),
    ContextSlot(String),
    Unary {
        op: UnaryOp,
        operand: Box<ScalarExpr>,
    },
    Arithmetic {
        op: ArithmeticOp,
        left: Box<ScalarExpr>,
        right: Box<ScalarExpr>,
        contract: NumericContract,
    },
    Comparison {
        op: ComparisonOp,
        left: Box<ScalarExpr>,
        right: Box<ScalarExpr>,
    },
    BooleanLogic {
        op: BooleanOp,
        operands: Vec<ScalarExpr>,
    },
    Case {
        branches: Vec<(ScalarExpr, ScalarExpr)>,
        otherwise: Box<ScalarExpr>,
    },
    NullTest {
        operand: Box<ScalarExpr>,
        negated: bool,
    },
    ExplicitCast {
        operand: Box<ScalarExpr>,
        rounding: Option<RoundingMode>,
    },
    TemporalDerivation {
        operand: Box<ScalarExpr>,
        bucket: TimeBucket,
    },
    UnitConversion {
        operand: Box<ScalarExpr>,
        conversion: StableId,
    },
}

impl ScalarExpr {
    #[must_use]
    pub fn field_refs(&self) -> Vec<StableId> {
        self.references(|k| {
            if let ScalarExprKind::FieldRef(id) = k {
                Some(id.clone())
            } else {
                None
            }
        })
    }
    #[must_use]
    pub fn dimension_refs(&self) -> Vec<StableId> {
        self.references(|k| {
            if let ScalarExprKind::DimensionRef(id) = k {
                Some(id.clone())
            } else {
                None
            }
        })
    }
    #[must_use]
    pub fn context_slots(&self) -> Vec<String> {
        self.references(|k| {
            if let ScalarExprKind::ContextSlot(name) = k {
                Some(name.clone())
            } else {
                None
            }
        })
    }
    #[must_use]
    pub fn conversion_refs(&self) -> Vec<StableId> {
        self.references(|k| {
            if let ScalarExprKind::UnitConversion { conversion, .. } = k {
                Some(conversion.clone())
            } else {
                None
            }
        })
    }
    fn references<T>(&self, select: impl Fn(&ScalarExprKind) -> Option<T>) -> Vec<T> {
        let mut result = Vec::new();
        let mut stack = vec![self];
        while let Some(expr) = stack.pop() {
            if let Some(value) = select(&expr.kind) {
                result.push(value);
            }
            stack.extend(expr.children());
        }
        result
    }
    #[must_use]
    pub fn children(&self) -> Vec<&Self> {
        match &self.kind {
            ScalarExprKind::Unary { operand, .. }
            | ScalarExprKind::NullTest { operand, .. }
            | ScalarExprKind::ExplicitCast { operand, .. }
            | ScalarExprKind::TemporalDerivation { operand, .. }
            | ScalarExprKind::UnitConversion { operand, .. } => vec![operand],
            ScalarExprKind::Arithmetic { left, right, .. }
            | ScalarExprKind::Comparison { left, right, .. } => vec![left, right],
            ScalarExprKind::BooleanLogic { operands, .. } => operands.iter().collect(),
            ScalarExprKind::Case {
                branches,
                otherwise,
            } => branches
                .iter()
                .flat_map(|(condition, value)| [condition, value])
                .chain(std::iter::once(otherwise.as_ref()))
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricExpr {
    pub kind: MetricExprKind,
    pub value_type: ValueType,
    pub unit: Option<Unit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricExprKind {
    ExplicitCast {
        operand: Box<MetricExpr>,
        rounding: Option<RoundingMode>,
    },
    MeasureRef(StableId),
    MetricRef(StableId),
    Arithmetic {
        op: ArithmeticOp,
        left: Box<MetricExpr>,
        right: Box<MetricExpr>,
        contract: NumericContract,
    },
    Ratio {
        numerator: Box<MetricExpr>,
        denominator: Box<MetricExpr>,
        zero_denominator: ZeroDivisionPolicy,
        rounding: Option<RoundingMode>,
    },
    Window {
        base_metric: StableId,
    },
}

impl MetricExpr {
    #[must_use]
    pub fn measure_refs(&self) -> Vec<StableId> {
        self.references(false)
    }
    #[must_use]
    pub fn metric_refs(&self) -> Vec<StableId> {
        self.references(true)
    }
    fn references(&self, metrics: bool) -> Vec<StableId> {
        let mut result = Vec::new();
        let mut stack = vec![self];
        while let Some(expr) = stack.pop() {
            match &expr.kind {
                MetricExprKind::MeasureRef(id) if !metrics => result.push(id.clone()),
                MetricExprKind::MetricRef(id) | MetricExprKind::Window { base_metric: id }
                    if metrics =>
                {
                    result.push(id.clone());
                }
                _ => {}
            }
            stack.extend(expr.children());
        }
        result
    }
    #[must_use]
    pub fn children(&self) -> Vec<&Self> {
        match &self.kind {
            MetricExprKind::ExplicitCast { operand, .. } => vec![operand],
            MetricExprKind::Arithmetic { left, right, .. } => vec![left, right],
            MetricExprKind::Ratio {
                numerator,
                denominator,
                ..
            } => vec![numerator, denominator],
            _ => Vec::new(),
        }
    }
}
