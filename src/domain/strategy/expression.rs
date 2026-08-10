use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOperator {
    Above,
    Below,
    AtLeast,
    AtMost,
    Equal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacdComponent {
    Line,
    Signal,
    Histogram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BollBand {
    Upper,
    Middle,
    Lower,
}

/// A whitelist of values that the deterministic evaluator can calculate.
/// `lag` is signed on purpose so untrusted JSON containing future (negative)
/// lags can be parsed and rejected with a useful validation error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "indicator", rename_all = "snake_case", deny_unknown_fields)]
pub enum IndicatorRef {
    Open {
        #[serde(default)]
        lag: i16,
    },
    High {
        #[serde(default)]
        lag: i16,
    },
    Low {
        #[serde(default)]
        lag: i16,
    },
    Close {
        #[serde(default)]
        lag: i16,
    },
    Volume {
        #[serde(default)]
        lag: i16,
    },
    Return {
        period: u16,
        #[serde(default)]
        lag: i16,
    },
    Sma {
        period: u16,
        #[serde(default)]
        lag: i16,
    },
    Ema {
        period: u16,
        #[serde(default)]
        lag: i16,
    },
    Rsi {
        period: u16,
        #[serde(default)]
        lag: i16,
    },
    Macd {
        fast_period: u16,
        slow_period: u16,
        signal_period: u16,
        component: MacdComponent,
        #[serde(default)]
        lag: i16,
    },
    Boll {
        period: u16,
        std_dev: f64,
        band: BollBand,
        #[serde(default)]
        lag: i16,
    },
    Atr {
        period: u16,
        #[serde(default)]
        lag: i16,
    },
    NDayHigh {
        period: u16,
        #[serde(default)]
        lag: i16,
    },
    NDayLow {
        period: u16,
        #[serde(default)]
        lag: i16,
    },
}

impl IndicatorRef {
    pub const fn lag(&self) -> i16 {
        match self {
            Self::Open { lag }
            | Self::High { lag }
            | Self::Low { lag }
            | Self::Close { lag }
            | Self::Volume { lag }
            | Self::Return { lag, .. }
            | Self::Sma { lag, .. }
            | Self::Ema { lag, .. }
            | Self::Rsi { lag, .. }
            | Self::Macd { lag, .. }
            | Self::Boll { lag, .. }
            | Self::Atr { lag, .. }
            | Self::NDayHigh { lag, .. }
            | Self::NDayLow { lag, .. } => *lag,
        }
    }

    pub fn warm_up(&self) -> usize {
        let lag = self.lag().max(0) as usize;
        let period = match self {
            Self::Open { .. }
            | Self::High { .. }
            | Self::Low { .. }
            | Self::Close { .. }
            | Self::Volume { .. } => 1,
            Self::Return { period, .. }
            | Self::Sma { period, .. }
            | Self::Ema { period, .. }
            | Self::Rsi { period, .. }
            | Self::Boll { period, .. }
            | Self::Atr { period, .. }
            | Self::NDayHigh { period, .. }
            | Self::NDayLow { period, .. } => *period as usize,
            Self::Macd {
                slow_period,
                signal_period,
                ..
            } => *slow_period as usize + *signal_period as usize - 1,
        };
        period + lag
    }

    pub fn periods(&self) -> impl Iterator<Item = u16> {
        let values = match self {
            Self::Open { .. }
            | Self::High { .. }
            | Self::Low { .. }
            | Self::Close { .. }
            | Self::Volume { .. } => [None, None, None],
            Self::Return { period, .. }
            | Self::Sma { period, .. }
            | Self::Ema { period, .. }
            | Self::Rsi { period, .. }
            | Self::Boll { period, .. }
            | Self::Atr { period, .. }
            | Self::NDayHigh { period, .. }
            | Self::NDayLow { period, .. } => [Some(*period), None, None],
            Self::Macd {
                fast_period,
                slow_period,
                signal_period,
                ..
            } => [Some(*fast_period), Some(*slow_period), Some(*signal_period)],
        };
        values.into_iter().flatten()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ValueExpression {
    Indicator(IndicatorRef),
    Constant { constant: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    pub left: ValueExpression,
    pub op: CompareOperator,
    pub right: ValueExpression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crossing {
    pub left: ValueExpression,
    pub right: ValueExpression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Expression {
    All { all: Vec<Expression> },
    Any { any: Vec<Expression> },
    Not { not: Box<Expression> },
    Compare { compare: Comparison },
    CrossesAbove { crosses_above: Crossing },
    CrossesBelow { crosses_below: Crossing },
}

impl Expression {
    pub fn node_count(&self) -> usize {
        1 + match self {
            Self::All { all } => all.iter().map(Self::node_count).sum(),
            Self::Any { any } => any.iter().map(Self::node_count).sum(),
            Self::Not { not } => not.node_count(),
            Self::Compare { .. } | Self::CrossesAbove { .. } | Self::CrossesBelow { .. } => 0,
        }
    }

    pub fn depth(&self) -> usize {
        1 + match self {
            Self::All { all } => all.iter().map(Self::depth).max().unwrap_or(0),
            Self::Any { any } => any.iter().map(Self::depth).max().unwrap_or(0),
            Self::Not { not } => not.depth(),
            Self::Compare { .. } | Self::CrossesAbove { .. } | Self::CrossesBelow { .. } => 0,
        }
    }

    pub fn visit_values(&self, visitor: &mut impl FnMut(&ValueExpression)) {
        match self {
            Self::All { all } => all.iter().for_each(|item| item.visit_values(visitor)),
            Self::Any { any } => any.iter().for_each(|item| item.visit_values(visitor)),
            Self::Not { not } => not.visit_values(visitor),
            Self::Compare { compare } => {
                visitor(&compare.left);
                visitor(&compare.right);
            }
            Self::CrossesAbove { crosses_above } => {
                visitor(&crosses_above.left);
                visitor(&crosses_above.right);
            }
            Self::CrossesBelow { crosses_below } => {
                visitor(&crosses_below.left);
                visitor(&crosses_below.right);
            }
        }
    }
}
