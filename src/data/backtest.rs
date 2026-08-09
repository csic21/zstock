//! Reproducible local strategy evidence over the visible point-in-time series.

use serde::Serialize;

use crate::data::indicators::MaSeries;
use crate::domain::backtest::{
    BacktestConfig, CostModel, EvidenceGrade, EvidenceReport, ValidationMethod, run_next_open,
};
use crate::domain::market::CandleRecord;
use crate::domain::money::Currency;
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
    pub sample_bars: usize,
    pub evidence: EvidenceReport,
    pub notes: Vec<String>,
}

impl BacktestReport {
    pub fn summary_line(&self, work: bool) -> String {
        if self.evidence.trades.is_empty() {
            return if work {
                format!("{} · no trades in sample", self.rule)
            } else {
                format!("{} · 样本内无触发", self.rule)
            };
        }
        if work {
            format!(
                "{} · n={} (OOS {}) · net avg {:+.1}% · excess {:+.1}% · MDD {:+.1}%",
                self.rule,
                self.evidence.trades.len(),
                self.evidence.out_of_sample_trades,
                self.evidence.average_net_return_pct,
                self.evidence.excess_return_pct,
                self.evidence.max_drawdown_pct
            )
        } else {
            format!(
                "{} · {} 笔（样本外 {}）· 扣费均值 {:+.1}% · 超额 {:+.1}% · 最大回撤 {:+.1}%",
                self.rule,
                self.evidence.trades.len(),
                self.evidence.out_of_sample_trades,
                self.evidence.average_net_return_pct,
                self.evidence.excess_return_pct,
                self.evidence.max_drawdown_pct
            )
        }
    }

    pub fn evidence_line(&self, work: bool) -> String {
        let grade = match (self.evidence.evidence_grade, work) {
            (EvidenceGrade::None, true) => "No evidence",
            (EvidenceGrade::None, false) => "无证据",
            (EvidenceGrade::InsufficientSample, true) => "Insufficient sample",
            (EvidenceGrade::InsufficientSample, false) => "样本不足",
            (EvidenceGrade::InSampleExploration, true) => "In-sample exploration",
            (EvidenceGrade::InSampleExploration, false) => "样本内探索",
            (EvidenceGrade::OutOfSampleObservation, true) => "OOS observation",
            (EvidenceGrade::OutOfSampleObservation, false) => "样本外观察",
            (EvidenceGrade::MultiPeriodStable, true) => "Multi-period stable",
            (EvidenceGrade::MultiPeriodStable, false) => "多阶段稳定",
        };
        let interval = self
            .evidence
            .confidence_interval_95_pct
            .map(|(low, high)| format!("95% CI [{low:+.1}%, {high:+.1}%]"))
            .unwrap_or_else(|| "95% CI —".into());
        format!(
            "{grade} · {interval} · 基准 {} {:+.1}% · 成本 {} · 策略 {} · 数据 {}",
            self.evidence.benchmark_name,
            self.evidence.benchmark_return_pct,
            self.evidence.cost_model.version,
            self.evidence.strategy_version,
            self.evidence.dataset_version
        )
    }
}

/// Run a versioned rule with next-session-open execution, explicit costs and
/// a 70/30 chronological holdout. The current symbol's buy-and-hold return is
/// the local benchmark when no index series is available.
pub fn run(
    candles: &[Candle],
    rule: BacktestRule,
    hold_days: usize,
    currency: Currency,
) -> Option<BacktestReport> {
    let hold_days = hold_days.clamp(3, 40);
    if candles.len() < 60 {
        return None;
    }

    let signals = match rule {
        BacktestRule::Ma20CrossUp => signals_ma20_cross(candles),
        BacktestRule::RsiOversoldExit => signals_rsi_exit(candles),
        BacktestRule::Breakout20 => signals_breakout20(candles),
    };

    let records = candles
        .iter()
        .map(|candle| CandleRecord {
            time: candle.date.to_string(),
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            volume: candle.volume,
        })
        .collect::<Vec<_>>();
    let costs = match currency {
        Currency::Cny => CostModel::default(),
        Currency::Hkd => CostModel {
            commission_bps_each_side: 3.0,
            sell_tax_bps: 0.0,
            slippage_bps_each_side: 8.0,
            other_fees_bps_each_side: 10.0,
            version: "hk-equity-costs-v1".into(),
        },
    };
    let dataset_version = format!(
        "visible-series-v1:{}:{}:{}",
        records
            .first()
            .map(|candle| candle.time.as_str())
            .unwrap_or("empty"),
        records
            .last()
            .map(|candle| candle.time.as_str())
            .unwrap_or("empty"),
        records.len()
    );
    let config = BacktestConfig {
        hold_days,
        costs,
        strategy_version: format!("{}-v1", rule.label(true).replace(' ', "-")),
        dataset_version,
        benchmark_name: "当前标的同期买入持有".into(),
        minimum_trades: 20,
        validation: ValidationMethod::Holdout {
            train_fraction_pct: 70,
        },
    };
    let evidence = run_next_open(&records, &records, &config, |history, index| {
        debug_assert_eq!(history.len(), index + 1);
        signals.get(index).copied().unwrap_or(false)
    });
    let notes = vec![
        evidence.execution_rule.clone(),
        "报告包含双边成本、滑点、基准、时间切分、区间和版本；幸存者偏差需由数据集清单另行审计"
            .into(),
        "仅供学习研究，不构成投资建议".into(),
    ];

    Some(BacktestReport {
        rule: rule.label(false).into(),
        hold_days,
        sample_bars: candles.len(),
        evidence,
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
        let report = run(&candles, BacktestRule::Ma20CrossUp, 10, Currency::Cny);
        assert!(report.is_some());
    }

    #[test]
    fn all_rules_produce_versioned_evidence_reports() {
        let candles: Vec<_> = (0..160)
            .map(|i| {
                let px = 10.0 + (i as f64 / 4.0).sin() + i as f64 * 0.01;
                Candle {
                    date: shared(format!("d{i:03}")),
                    open: px * 0.999,
                    high: px * 1.02,
                    low: px * 0.98,
                    close: px,
                    volume: 10_000,
                }
            })
            .collect();
        for rule in BacktestRule::all() {
            let report = run(&candles, rule, 10, Currency::Cny).unwrap();
            assert!(!report.evidence.execution_rule.is_empty());
            assert!(!report.evidence.cost_model.version.is_empty());
            assert_eq!(report.sample_bars, candles.len());
        }
    }
}
