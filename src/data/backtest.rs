//! 轻量规则回测：在历史日 K 上模拟简单触发，建立对策略的信任感。
//!
//! 不是专业回测引擎：无滑点/手续费精细模型，只给可解释统计。

use serde::Serialize;

use crate::data::indicators::MaSeries;
use crate::model::Candle;

/// 可回测的本地规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacktestRule {
    /// 收盘站上 MA20 且前一日在下方 → 持有 `hold_days` 日。
    Ma20CrossUp,
    /// RSI14 从 ≤30 回升到 >30 → 持有 `hold_days` 日。
    RsiOversoldExit,
    /// 收盘创新高 20 日 → 持有 `hold_days` 日。
    Breakout20,
}

impl BacktestRule {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Ma20CrossUp, true) => "MA20 cross",
            (Self::Ma20CrossUp, false) => "站上 MA20",
            (Self::RsiOversoldExit, true) => "RSI exit OS",
            (Self::RsiOversoldExit, false) => "RSI 离超卖",
            (Self::Breakout20, true) => "20d high",
            (Self::Breakout20, false) => "突破 20 日高",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Ma20CrossUp, Self::RsiOversoldExit, Self::Breakout20]
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestReport {
    pub rule: String,
    pub hold_days: usize,
    pub trades: usize,
    /// 平均持有期收益 %。
    pub avg_return_pct: f64,
    /// 胜率 0–100。
    pub win_rate_pct: f64,
    /// 单笔最差 %。
    pub worst_pct: f64,
    /// 单笔最好 %。
    pub best_pct: f64,
    pub sample_bars: usize,
    pub notes: Vec<String>,
}

impl BacktestReport {
    pub fn summary_line(&self, work: bool) -> String {
        if self.trades == 0 {
            return if work {
                format!("{} · no trades in sample", self.rule)
            } else {
                format!("{} · 样本内无触发", self.rule)
            };
        }
        if work {
            format!(
                "{} · n={} · avg {:+.1}% · win {:.0}% · worst {:+.1}%",
                self.rule, self.trades, self.avg_return_pct, self.win_rate_pct, self.worst_pct
            )
        } else {
            format!(
                "{} · {} 次 · 均 {:+.1}% · 胜 {:.0}% · 最差 {:+.1}%",
                self.rule, self.trades, self.avg_return_pct, self.win_rate_pct, self.worst_pct
            )
        }
    }
}

/// 在 `candles` 上跑规则；`hold_days` 默认 10。
pub fn run(candles: &[Candle], rule: BacktestRule, hold_days: usize) -> Option<BacktestReport> {
    let hold_days = hold_days.clamp(3, 40);
    if candles.len() < 60 {
        return None;
    }

    let signals = match rule {
        BacktestRule::Ma20CrossUp => signals_ma20_cross(candles),
        BacktestRule::RsiOversoldExit => signals_rsi_exit(candles),
        BacktestRule::Breakout20 => signals_breakout20(candles),
    };

    let mut rets = Vec::new();
    let mut i = 0usize;
    while i < signals.len() {
        if !signals[i] {
            i += 1;
            continue;
        }
        let exit = (i + hold_days).min(candles.len() - 1);
        if exit <= i {
            break;
        }
        let entry = candles[i].close;
        let exit_px = candles[exit].close;
        if entry > 0.0 && exit_px.is_finite() {
            rets.push((exit_px / entry - 1.0) * 100.0);
        }
        // 不重叠持仓
        i = exit + 1;
    }

    let trades = rets.len();
    let (avg, win, worst, best) = if trades == 0 {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        let sum: f64 = rets.iter().sum();
        let avg = sum / trades as f64;
        let wins = rets.iter().filter(|r| **r > 0.0).count();
        let win = wins as f64 / trades as f64 * 100.0;
        let worst = rets
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let best = rets
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        (avg, win, worst, best)
    };

    let mut notes = vec![
        format!("持有 {hold_days} 个交易日平仓"),
        "未计滑点与印花税；样本外可能失效".into(),
        "仅供学习研究，不构成投资建议".into(),
    ];
    if trades < 5 {
        notes.insert(0, "触发次数偏少，统计不稳定".into());
    }

    Some(BacktestReport {
        rule: rule.label(false).into(),
        hold_days,
        trades,
        avg_return_pct: avg,
        win_rate_pct: win,
        worst_pct: if trades == 0 { 0.0 } else { worst },
        best_pct: if trades == 0 { 0.0 } else { best },
        sample_bars: candles.len(),
        notes,
    })
}

fn signals_ma20_cross(candles: &[Candle]) -> Vec<bool> {
    let ma = MaSeries::from_candles(candles);
    let mut out = vec![false; candles.len()];
    for i in 1..candles.len() {
        let (_, _, m_prev, _) = ma.value_at(i - 1);
        let (_, _, m_now, _) = ma.value_at(i);
        let (Some(mp), Some(mn)) = (m_prev, m_now) else {
            continue;
        };
        let prev_below = candles[i - 1].close < mp;
        let now_above = candles[i].close >= mn;
        out[i] = prev_below && now_above;
    }
    out
}

fn signals_rsi_exit(candles: &[Candle]) -> Vec<bool> {
    let rsi = rsi_series(candles, 14);
    let mut out = vec![false; candles.len()];
    for i in 1..candles.len() {
        let (Some(prev), Some(now)) = (rsi[i - 1], rsi[i]) else {
            continue;
        };
        out[i] = prev <= 30.0 && now > 30.0;
    }
    out
}

fn signals_breakout20(candles: &[Candle]) -> Vec<bool> {
    let mut out = vec![false; candles.len()];
    for i in 20..candles.len() {
        let window = &candles[i - 20..i];
        let prior_high = window
            .iter()
            .map(|c| c.high)
            .fold(f64::NEG_INFINITY, f64::max);
        if prior_high.is_finite() && candles[i].close > prior_high {
            out[i] = true;
        }
    }
    out
}

fn rsi_series(candles: &[Candle], period: usize) -> Vec<Option<f64>> {
    let n = candles.len();
    let mut out = vec![None; n];
    if n < period + 1 {
        return out;
    }
    let mut gains = 0.0;
    let mut losses = 0.0;
    for i in 1..=period {
        let d = candles[i].close - candles[i - 1].close;
        if d >= 0.0 {
            gains += d;
        } else {
            losses -= d;
        }
    }
    let mut avg_gain = gains / period as f64;
    let mut avg_loss = losses / period as f64;
    out[period] = Some(rsi_from_avg(avg_gain, avg_loss));
    for i in (period + 1)..n {
        let d = candles[i].close - candles[i - 1].close;
        let (g, l) = if d >= 0.0 { (d, 0.0) } else { (0.0, -d) };
        avg_gain = (avg_gain * (period as f64 - 1.0) + g) / period as f64;
        avg_loss = (avg_loss * (period as f64 - 1.0) + l) / period as f64;
        out[i] = Some(rsi_from_avg(avg_gain, avg_loss));
    }
    out
}

fn rsi_from_avg(avg_gain: f64, avg_loss: f64) -> f64 {
    if avg_loss < 1e-12 {
        return 100.0;
    }
    let rs = avg_gain / avg_loss;
    100.0 - 100.0 / (1.0 + rs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::shared;

    #[test]
    fn runs_on_synthetic_uptrend() {
        let candles: Vec<_> = (0..120)
            .map(|i| {
                let px = 10.0 + i as f64 * 0.05;
                Candle {
                    date: shared(format!("2024-{:02}", (i % 28) + 1)),
                    open: px,
                    high: px * 1.01,
                    low: px * 0.99,
                    close: px,
                    volume: 10_000,
                }
            })
            .collect();
        let report = run(&candles, BacktestRule::Ma20CrossUp, 10);
        assert!(report.is_some());
    }
}
