use super::expression::{
    BollBand, CompareOperator, Expression, IndicatorRef, MacdComponent, ValueExpression,
};
use super::spec::{ExitRule, StrategySpec};
use super::validation::{ValidatedStrategy, ValidationError, ValidationLimits, validate};
use crate::domain::market::CandleRecord;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionContext {
    pub entry_price: f64,
    pub holding_days: usize,
}

#[derive(Debug, Clone)]
pub struct CompiledStrategy {
    spec: StrategySpec,
    strategy_id: String,
    warm_up_bars: usize,
}

impl CompiledStrategy {
    pub fn compile(spec: StrategySpec) -> Result<Self, Vec<ValidationError>> {
        let ValidatedStrategy { warm_up_bars, .. } = validate(&spec, ValidationLimits::default())?;
        let strategy_id = super::strategy_id(&spec);
        Ok(Self {
            spec,
            strategy_id,
            warm_up_bars,
        })
    }

    pub fn spec(&self) -> &StrategySpec {
        &self.spec
    }

    pub fn strategy_id(&self) -> &str {
        &self.strategy_id
    }

    pub const fn warm_up_bars(&self) -> usize {
        self.warm_up_bars
    }

    pub fn entry_signal(&self, candles: &[CandleRecord], index: usize) -> bool {
        index < candles.len()
            && index + 1 >= self.warm_up_bars
            && eval_expression(&self.spec.entry, candles, index).unwrap_or(false)
    }

    pub fn entry_signals(&self, candles: &[CandleRecord]) -> Vec<bool> {
        (0..candles.len())
            .map(|index| self.entry_signal(&candles[..=index], index))
            .collect()
    }

    pub fn should_exit(
        &self,
        candles: &[CandleRecord],
        index: usize,
        position: PositionContext,
    ) -> bool {
        index < candles.len()
            && eval_exit(&self.spec.exit, candles, index, position).unwrap_or(false)
    }
}

fn eval_expression(
    expression: &Expression,
    candles: &[CandleRecord],
    index: usize,
) -> Option<bool> {
    match expression {
        Expression::All { all } => all
            .iter()
            .map(|item| eval_expression(item, candles, index))
            .try_fold(true, |acc, value| value.map(|value| acc && value)),
        Expression::Any { any } => {
            let mut observed = false;
            for item in any {
                if let Some(value) = eval_expression(item, candles, index) {
                    observed = true;
                    if value {
                        return Some(true);
                    }
                }
            }
            observed.then_some(false)
        }
        Expression::Not { not } => eval_expression(not, candles, index).map(|value| !value),
        Expression::Compare { compare } => {
            let left = eval_value(&compare.left, candles, index)?;
            let right = eval_value(&compare.right, candles, index)?;
            Some(compare_values(left, compare.op, right))
        }
        Expression::CrossesAbove { crosses_above } => {
            let previous = index.checked_sub(1)?;
            let left_previous = eval_value(&crosses_above.left, candles, previous)?;
            let right_previous = eval_value(&crosses_above.right, candles, previous)?;
            let left = eval_value(&crosses_above.left, candles, index)?;
            let right = eval_value(&crosses_above.right, candles, index)?;
            Some(left_previous <= right_previous && left > right)
        }
        Expression::CrossesBelow { crosses_below } => {
            let previous = index.checked_sub(1)?;
            let left_previous = eval_value(&crosses_below.left, candles, previous)?;
            let right_previous = eval_value(&crosses_below.right, candles, previous)?;
            let left = eval_value(&crosses_below.left, candles, index)?;
            let right = eval_value(&crosses_below.right, candles, index)?;
            Some(left_previous >= right_previous && left < right)
        }
    }
}

fn eval_exit(
    exit: &ExitRule,
    candles: &[CandleRecord],
    index: usize,
    position: PositionContext,
) -> Option<bool> {
    match exit {
        ExitRule::All { all } => all
            .iter()
            .map(|item| eval_exit(item, candles, index, position))
            .try_fold(true, |acc, value| value.map(|value| acc && value)),
        ExitRule::Any { any } => {
            let mut observed = false;
            for item in any {
                if let Some(value) = eval_exit(item, candles, index, position) {
                    observed = true;
                    if value {
                        return Some(true);
                    }
                }
            }
            observed.then_some(false)
        }
        ExitRule::HoldDays { hold_days } => Some(position.holding_days >= usize::from(*hold_days)),
        ExitRule::StopLossPct { stop_loss_pct } => {
            let close = candles.get(index)?.close;
            valid_price(position.entry_price)
                .then_some((close / position.entry_price - 1.0) * 100.0 <= -*stop_loss_pct)
        }
        ExitRule::TakeProfitPct { take_profit_pct } => {
            let close = candles.get(index)?.close;
            valid_price(position.entry_price)
                .then_some((close / position.entry_price - 1.0) * 100.0 >= *take_profit_pct)
        }
        ExitRule::Condition { condition } => eval_expression(condition, candles, index),
    }
}

fn compare_values(left: f64, operator: CompareOperator, right: f64) -> bool {
    match operator {
        CompareOperator::Above => left > right,
        CompareOperator::Below => left < right,
        CompareOperator::AtLeast => left >= right,
        CompareOperator::AtMost => left <= right,
        CompareOperator::Equal => {
            let scale = left.abs().max(right.abs()).max(1.0);
            (left - right).abs() <= scale * 1e-12
        }
    }
}

fn eval_value(expression: &ValueExpression, candles: &[CandleRecord], index: usize) -> Option<f64> {
    match expression {
        ValueExpression::Constant { constant } => constant.is_finite().then_some(*constant),
        ValueExpression::Indicator(indicator) => eval_indicator(indicator, candles, index),
    }
}

fn eval_indicator(indicator: &IndicatorRef, candles: &[CandleRecord], index: usize) -> Option<f64> {
    let lag = usize::try_from(indicator.lag()).ok()?;
    let target = index.checked_sub(lag)?;
    let bar = candles.get(target)?;
    match indicator {
        IndicatorRef::Open { .. } => valid_price(bar.open).then_some(bar.open),
        IndicatorRef::High { .. } => valid_price(bar.high).then_some(bar.high),
        IndicatorRef::Low { .. } => valid_price(bar.low).then_some(bar.low),
        IndicatorRef::Close { .. } => valid_price(bar.close).then_some(bar.close),
        IndicatorRef::Volume { .. } => Some(bar.volume as f64),
        IndicatorRef::Return { period, .. } => {
            let previous = target.checked_sub(usize::from(*period))?;
            let base = candles.get(previous)?.close;
            valid_price(base).then_some((bar.close / base - 1.0) * 100.0)
        }
        IndicatorRef::Sma { period, .. } => moving_average(candles, target, *period),
        IndicatorRef::Ema { period, .. } => ema_at(candles, target, *period),
        IndicatorRef::Rsi { period, .. } => rsi_at(candles, target, *period),
        IndicatorRef::Macd {
            fast_period,
            slow_period,
            signal_period,
            component,
            ..
        } => macd_at(
            candles,
            target,
            *fast_period,
            *slow_period,
            *signal_period,
            *component,
        ),
        IndicatorRef::Boll {
            period,
            std_dev,
            band,
            ..
        } => boll_at(candles, target, *period, *std_dev, *band),
        IndicatorRef::Atr { period, .. } => atr_at(candles, target, *period),
        IndicatorRef::NDayHigh { period, .. } => {
            let start = target.checked_sub(usize::from(*period))?;
            candles[start..target]
                .iter()
                .map(|item| item.high)
                .filter(|value| value.is_finite())
                .reduce(f64::max)
        }
        IndicatorRef::NDayLow { period, .. } => {
            let start = target.checked_sub(usize::from(*period))?;
            candles[start..target]
                .iter()
                .map(|item| item.low)
                .filter(|value| value.is_finite())
                .reduce(f64::min)
        }
    }
}

fn moving_average(candles: &[CandleRecord], target: usize, period: u16) -> Option<f64> {
    let period = usize::from(period);
    let start = target.checked_add(1)?.checked_sub(period)?;
    let values = &candles.get(start..=target)?;
    values
        .iter()
        .all(|bar| valid_price(bar.close))
        .then(|| values.iter().map(|bar| bar.close).sum::<f64>() / period as f64)
}

fn ema_at(candles: &[CandleRecord], target: usize, period: u16) -> Option<f64> {
    let period = usize::from(period);
    if target + 1 < period {
        return None;
    }
    let initial = candles.get(..period)?;
    if !initial.iter().all(|bar| valid_price(bar.close)) {
        return None;
    }
    let mut ema = initial.iter().map(|bar| bar.close).sum::<f64>() / period as f64;
    let alpha = 2.0 / (period as f64 + 1.0);
    for bar in candles.get(period..=target).unwrap_or_default() {
        if !valid_price(bar.close) {
            return None;
        }
        ema = bar.close * alpha + ema * (1.0 - alpha);
    }
    Some(ema)
}

fn rsi_at(candles: &[CandleRecord], target: usize, period: u16) -> Option<f64> {
    let period = usize::from(period);
    if target < period {
        return None;
    }
    let mut gains = 0.0;
    let mut losses = 0.0;
    for index in 1..=period {
        let change = candles.get(index)?.close - candles.get(index - 1)?.close;
        if change >= 0.0 {
            gains += change;
        } else {
            losses -= change;
        }
    }
    let mut average_gain = gains / period as f64;
    let mut average_loss = losses / period as f64;
    for index in (period + 1)..=target {
        let change = candles.get(index)?.close - candles.get(index - 1)?.close;
        let (gain, loss) = if change >= 0.0 {
            (change, 0.0)
        } else {
            (0.0, -change)
        };
        average_gain = (average_gain * (period as f64 - 1.0) + gain) / period as f64;
        average_loss = (average_loss * (period as f64 - 1.0) + loss) / period as f64;
    }
    if average_loss < 1e-12 {
        Some(100.0)
    } else {
        let relative_strength = average_gain / average_loss;
        Some(100.0 - 100.0 / (1.0 + relative_strength))
    }
}

fn macd_at(
    candles: &[CandleRecord],
    target: usize,
    fast_period: u16,
    slow_period: u16,
    signal_period: u16,
    component: MacdComponent,
) -> Option<f64> {
    let first_line = usize::from(slow_period).checked_sub(1)?;
    if target < first_line {
        return None;
    }
    let lines: Vec<_> = (first_line..=target)
        .map(|index| {
            Some(ema_at(candles, index, fast_period)? - ema_at(candles, index, slow_period)?)
        })
        .collect::<Option<_>>()?;
    let line = *lines.last()?;
    if matches!(component, MacdComponent::Line) {
        return Some(line);
    }
    let signal_period = usize::from(signal_period);
    if lines.len() < signal_period {
        return None;
    }
    let mut signal = lines[..signal_period].iter().sum::<f64>() / signal_period as f64;
    let alpha = 2.0 / (signal_period as f64 + 1.0);
    for value in &lines[signal_period..] {
        signal = *value * alpha + signal * (1.0 - alpha);
    }
    match component {
        MacdComponent::Line => Some(line),
        MacdComponent::Signal => Some(signal),
        MacdComponent::Histogram => Some(line - signal),
    }
}

fn boll_at(
    candles: &[CandleRecord],
    target: usize,
    period: u16,
    std_dev: f64,
    band: BollBand,
) -> Option<f64> {
    let period = usize::from(period);
    let start = target.checked_add(1)?.checked_sub(period)?;
    let values = candles.get(start..=target)?;
    let middle = values.iter().map(|bar| bar.close).sum::<f64>() / period as f64;
    let variance = values
        .iter()
        .map(|bar| (bar.close - middle).powi(2))
        .sum::<f64>()
        / period as f64;
    let width = variance.sqrt() * std_dev;
    match band {
        BollBand::Upper => Some(middle + width),
        BollBand::Middle => Some(middle),
        BollBand::Lower => Some(middle - width),
    }
}

fn atr_at(candles: &[CandleRecord], target: usize, period: u16) -> Option<f64> {
    let period = usize::from(period);
    let start = target.checked_add(1)?.checked_sub(period)?;
    let mut total = 0.0;
    for index in start..=target {
        let bar = candles.get(index)?;
        let previous_close = index
            .checked_sub(1)
            .and_then(|previous| candles.get(previous))
            .map_or(bar.open, |previous| previous.close);
        let true_range = (bar.high - bar.low)
            .max((bar.high - previous_close).abs())
            .max((bar.low - previous_close).abs());
        if !true_range.is_finite() {
            return None;
        }
        total += true_range;
    }
    Some(total / period as f64)
}

fn valid_price(price: f64) -> bool {
    price.is_finite() && price > 0.0
}
