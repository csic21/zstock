//! Explainable technical snapshot for the currently loaded daily series.
//!
//! This is intentionally a compact decision aid rather than a trading instruction:
//! it combines trend, momentum, volatility, drawdown and volume confirmation so a
//! single attractive metric cannot dominate the result.

use crate::model::Candle;

const TRADING_DAYS: f64 = 252.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalRegime {
    Strong,
    Constructive,
    Neutral,
    Weak,
    Defensive,
}

impl SignalRegime {
    pub fn label(self) -> &'static str {
        match self {
            Self::Strong => "强势",
            Self::Constructive => "偏强",
            Self::Neutral => "中性",
            Self::Weak => "偏弱",
            Self::Defensive => "防守",
        }
    }

    /// Innocuous state label used by the work-mode telemetry skin.
    pub fn service_state(self) -> &'static str {
        match self {
            Self::Strong => "optimal",
            Self::Constructive => "healthy",
            Self::Neutral => "stable",
            Self::Weak => "degraded",
            Self::Defensive => "guarded",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SignalSnapshot {
    /// Composite 0–100 technical strength score.
    pub score: f64,
    pub regime: SignalRegime,
    pub rsi14: Option<f64>,
    pub momentum_20_pct: Option<f64>,
    pub volatility_20_ann_pct: Option<f64>,
    pub max_drawdown_1y_pct: Option<f64>,
    pub volume_ratio_20: Option<f64>,
    /// 0–100: only reflects sample/metric completeness, not prediction certainty.
    pub confidence: f64,
    pub reasons: Vec<&'static str>,
}

pub fn analyze(candles: &[Candle]) -> Option<SignalSnapshot> {
    if candles.len() < 20 {
        return None;
    }
    let last = candles.last()?;
    if !last.close.is_finite() || last.close <= 0.0 {
        return None;
    }

    let ma20 = mean_close(&candles[candles.len() - 20..])?;
    let ma60 = (candles.len() >= 60)
        .then(|| mean_close(&candles[candles.len() - 60..]))
        .flatten();
    let rsi14 = rsi(candles, 14);
    let momentum_20_pct = momentum(candles, 20);
    let volatility_20_ann_pct = annualized_volatility(candles, 20);
    let max_drawdown_1y_pct = max_drawdown(candles, 252).map(|v| v * 100.0);
    let volume_ratio_20 = volume_ratio(candles, 20);

    let mut score = 50.0;
    let mut reasons = Vec::with_capacity(5);

    let price_vs_ma20 = last.close / ma20 - 1.0;
    if price_vs_ma20 > 0.005 {
        score += 12.0;
        reasons.push("价格站上MA20");
    } else if price_vs_ma20 < -0.005 {
        score -= 12.0;
        reasons.push("价格位于MA20下方");
    } else {
        reasons.push("价格贴近MA20");
    }

    if let Some(ma60) = ma60 {
        let ma_spread = ma20 / ma60 - 1.0;
        if ma_spread > 0.003 {
            score += 10.0;
            reasons.push("中期均线向好");
        } else if ma_spread < -0.003 {
            score -= 10.0;
            reasons.push("中期均线承压");
        } else {
            reasons.push("中期均线走平");
        }
    }

    if let Some(mom) = momentum_20_pct {
        score += (mom * 0.8).clamp(-12.0, 12.0);
        reasons.push(if mom >= 0.0 {
            "20日动量为正"
        } else {
            "20日动量为负"
        });
    }

    if let Some(rsi) = rsi14 {
        score += ((rsi - 50.0) * 0.20).clamp(-8.0, 8.0);
        if rsi >= 78.0 {
            score -= 5.0;
            reasons.push("短线动量偏热");
        } else if rsi <= 22.0 {
            score += 3.0;
            reasons.push("短线处于超卖区");
        }
    }

    if let Some(ratio) = volume_ratio_20 {
        let day_move = candles
            .get(candles.len().saturating_sub(2))
            .filter(|c| c.close > 0.0)
            .map(|prev| last.close / prev.close - 1.0)
            .unwrap_or(0.0);
        if ratio >= 1.2 {
            score += if day_move >= 0.0 { 4.0 } else { -4.0 };
            reasons.push(if day_move >= 0.0 {
                "放量上涨确认"
            } else {
                "放量下跌警示"
            });
        }
    }

    if volatility_20_ann_pct.is_some_and(|v| v >= 60.0) {
        score -= 5.0;
        reasons.push("短期波动较高");
    }
    if max_drawdown_1y_pct.is_some_and(|v| v <= -35.0) {
        score -= 5.0;
        reasons.push("一年回撤较深");
    }

    score = score.clamp(0.0, 100.0);
    let regime = match score {
        v if v >= 72.0 => SignalRegime::Strong,
        v if v >= 58.0 => SignalRegime::Constructive,
        v if v >= 42.0 => SignalRegime::Neutral,
        v if v >= 28.0 => SignalRegime::Weak,
        _ => SignalRegime::Defensive,
    };
    let available = [
        rsi14.is_some(),
        momentum_20_pct.is_some(),
        volatility_20_ann_pct.is_some(),
        max_drawdown_1y_pct.is_some(),
        volume_ratio_20.is_some(),
        ma60.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count() as f64;
    let sample = (candles.len() as f64 / 120.0).clamp(0.35, 1.0);
    let confidence = (available / 6.0 * sample * 100.0).clamp(0.0, 100.0);

    Some(SignalSnapshot {
        score,
        regime,
        rsi14,
        momentum_20_pct,
        volatility_20_ann_pct,
        max_drawdown_1y_pct,
        volume_ratio_20,
        confidence,
        reasons,
    })
}

fn mean_close(candles: &[Candle]) -> Option<f64> {
    let (sum, count) = candles
        .iter()
        .map(|c| c.close)
        .filter(|v| v.is_finite() && *v > 0.0)
        .fold((0.0, 0usize), |(sum, count), value| {
            (sum + value, count + 1)
        });
    (count > 0).then(|| sum / count as f64)
}

fn momentum(candles: &[Candle], period: usize) -> Option<f64> {
    if period == 0 || candles.len() <= period {
        return None;
    }
    let now = candles.last()?.close;
    let then = candles.get(candles.len() - 1 - period)?.close;
    (now.is_finite() && then.is_finite() && then > 0.0).then(|| (now / then - 1.0) * 100.0)
}

fn rsi(candles: &[Candle], period: usize) -> Option<f64> {
    if period == 0 || candles.len() <= period {
        return None;
    }
    let start = candles.len() - period - 1;
    let mut gains = 0.0;
    let mut losses = 0.0;
    for pair in candles[start..].windows(2) {
        let delta = pair[1].close - pair[0].close;
        if delta >= 0.0 {
            gains += delta;
        } else {
            losses -= delta;
        }
    }
    if gains == 0.0 && losses == 0.0 {
        return Some(50.0);
    }
    if losses <= f64::EPSILON {
        return Some(100.0);
    }
    let rs = gains / losses;
    Some((100.0 - 100.0 / (1.0 + rs)).clamp(0.0, 100.0))
}

fn annualized_volatility(candles: &[Candle], period: usize) -> Option<f64> {
    if period < 2 || candles.len() <= period {
        return None;
    }
    let start = candles.len() - period - 1;
    let returns: Vec<f64> = candles[start..]
        .windows(2)
        .filter_map(|pair| {
            let a = pair[0].close;
            let b = pair[1].close;
            (a > 0.0 && b > 0.0).then(|| (b / a).ln())
        })
        .filter(|v| v.is_finite())
        .collect();
    if returns.len() < 2 {
        return None;
    }
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance =
        returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (returns.len() - 1) as f64;
    Some(variance.sqrt() * TRADING_DAYS.sqrt() * 100.0)
}

fn max_drawdown(candles: &[Candle], period: usize) -> Option<f64> {
    let start = candles.len().saturating_sub(period.max(1));
    let mut peak = 0.0_f64;
    let mut worst = 0.0_f64;
    let mut seen = false;
    for candle in &candles[start..] {
        if !candle.close.is_finite() || candle.close <= 0.0 {
            continue;
        }
        seen = true;
        peak = peak.max(candle.close);
        worst = worst.min(candle.close / peak - 1.0);
    }
    seen.then_some(worst)
}

fn volume_ratio(candles: &[Candle], period: usize) -> Option<f64> {
    if period == 0 || candles.len() <= period {
        return None;
    }
    let last = candles.last()?.volume as f64;
    let prior = &candles[candles.len() - 1 - period..candles.len() - 1];
    let avg = prior.iter().map(|c| c.volume as f64).sum::<f64>() / prior.len() as f64;
    (avg > 0.0).then(|| last / avg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::shared;

    fn series(start: f64, daily: f64, n: usize) -> Vec<Candle> {
        (0..n)
            .map(|i| {
                let close = start * (1.0 + daily).powi(i as i32);
                Candle {
                    date: shared(format!("d{i}")),
                    open: close,
                    high: close * 1.01,
                    low: close * 0.99,
                    close,
                    volume: 100_000 + i as u64 * 100,
                }
            })
            .collect()
    }

    #[test]
    fn rising_series_scores_above_falling_series() {
        let rising = analyze(&series(10.0, 0.006, 120)).unwrap();
        let falling = analyze(&series(100.0, -0.006, 120)).unwrap();
        assert!(rising.score > falling.score + 30.0);
        assert!(matches!(
            rising.regime,
            SignalRegime::Strong | SignalRegime::Constructive
        ));
        assert!(matches!(
            falling.regime,
            SignalRegime::Weak | SignalRegime::Defensive
        ));
    }

    #[test]
    fn flat_series_is_finite_and_neutral() {
        let flat = analyze(&series(10.0, 0.0, 80)).unwrap();
        assert!(flat.score.is_finite());
        assert_eq!(flat.rsi14, Some(50.0));
        assert!(matches!(
            flat.regime,
            SignalRegime::Neutral | SignalRegime::Constructive
        ));
    }
}
