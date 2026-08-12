//! Reproducible local strategy evidence over the visible point-in-time series.

use serde::Serialize;

use crate::domain::backtest::{
    BacktestConfig, CostModel, DatasetHashMetadata, EvidenceGrade, EvidenceReport,
    ValidationMethod, dataset_content_id, run_next_open,
};
use crate::domain::market::{Adjustment, CandleRecord, Market};
use crate::domain::money::Currency;
use crate::domain::strategy::expression::{
    CompareOperator, Comparison, Crossing, Expression, IndicatorRef, ValueExpression,
};
use crate::domain::strategy::spec::{ExitRule, StrategySpec};
use crate::domain::strategy::{CompiledStrategy, LocalTemplate};
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

    pub fn playbook(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Ma20CrossUp, true) => "Trend recovery · wait for a close back above MA20",
            (Self::Ma20CrossUp, false) => "趋势修复：收盘重新站上 MA20，次日开盘验证，不追盘中脉冲",
            (Self::RsiOversoldExit, true) => "Mean reversion · only after RSI exits oversold",
            (Self::RsiOversoldExit, false) => {
                "超跌修复：RSI 离开超卖区后再观察，弱势下跌中不抢反弹"
            }
            (Self::Breakout20, true) => "Breakout · require price and volume confirmation",
            (Self::Breakout20, false) => "趋势突破：收盘突破 20 日高并观察量能，偏离过大时等待回踩",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceVerdict {
    Insufficient,
    Reject,
    Observe,
    Candidate,
}

impl EvidenceVerdict {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Insufficient, true) => "Insufficient",
            (Self::Insufficient, false) => "样本不足",
            (Self::Reject, true) => "Unsupported",
            (Self::Reject, false) => "证据不支持",
            (Self::Observe, true) => "Observe",
            (Self::Observe, false) => "继续观察",
            (Self::Candidate, true) => "Validation candidate",
            (Self::Candidate, false) => "可继续验证",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TradeQualityMetrics {
    pub trade_count: usize,
    pub out_of_sample_count: usize,
    pub win_rate_pct: Option<f64>,
    pub out_of_sample_win_rate_pct: Option<f64>,
    pub average_win_pct: Option<f64>,
    pub average_loss_pct: Option<f64>,
    pub payoff_ratio: Option<f64>,
    pub profit_factor: Option<f64>,
    pub expectancy_pct: Option<f64>,
    pub out_of_sample_expectancy_pct: Option<f64>,
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

    pub fn quality_metrics(&self) -> TradeQualityMetrics {
        let validation_start = self
            .evidence
            .validation_statistics
            .as_ref()
            .map(|statistics| statistics.start_index)
            .unwrap_or(usize::MAX);
        quality_metrics_for(&self.evidence.trades, validation_start)
    }

    /// Conservative, deterministic screening verdict. It deliberately requires
    /// sample-out evidence and never promotes a strategy from win rate alone.
    pub fn verdict(&self) -> EvidenceVerdict {
        let quality = self.quality_metrics();
        if quality.trade_count < 10 || quality.out_of_sample_count < 3 {
            return EvidenceVerdict::Insufficient;
        }
        let profit_factor = quality.profit_factor.unwrap_or_else(|| {
            if quality.expectancy_pct.is_some_and(|value| value > 0.0) {
                f64::INFINITY
            } else {
                0.0
            }
        });
        if quality
            .out_of_sample_expectancy_pct
            .is_none_or(|value| value <= 0.0)
            || profit_factor < 1.0
            || self.evidence.max_drawdown_pct <= -30.0
        {
            return EvidenceVerdict::Reject;
        }
        let interval_is_positive = self
            .evidence
            .confidence_interval_95_pct
            .is_some_and(|(low, _)| low > 0.0);
        if quality.trade_count >= 20
            && quality.out_of_sample_count >= 5
            && profit_factor >= 1.2
            && self.evidence.max_drawdown_pct > -25.0
            && interval_is_positive
        {
            EvidenceVerdict::Candidate
        } else {
            EvidenceVerdict::Observe
        }
    }
}

fn quality_metrics_for(
    trades: &[crate::domain::backtest::SimulatedTrade],
    validation_start: usize,
) -> TradeQualityMetrics {
    let out_of_sample = trades
        .iter()
        .filter(|trade| trade.signal_index >= validation_start)
        .collect::<Vec<_>>();
    let wins = trades
        .iter()
        .filter(|trade| trade.net_return_pct > 0.0)
        .collect::<Vec<_>>();
    let losses = trades
        .iter()
        .filter(|trade| trade.net_return_pct < 0.0)
        .collect::<Vec<_>>();
    let average_win_pct = mean_values(wins.iter().map(|trade| trade.net_return_pct));
    let average_loss_pct = mean_values(losses.iter().map(|trade| trade.net_return_pct));
    let gross_profit = wins.iter().map(|trade| trade.net_return_pct).sum::<f64>();
    let gross_loss = losses
        .iter()
        .map(|trade| trade.net_return_pct.abs())
        .sum::<f64>();
    TradeQualityMetrics {
        trade_count: trades.len(),
        out_of_sample_count: out_of_sample.len(),
        win_rate_pct: ratio_pct(
            trades.len(),
            trades
                .iter()
                .filter(|trade| trade.net_return_pct > 0.0)
                .count(),
        ),
        out_of_sample_win_rate_pct: ratio_pct(
            out_of_sample.len(),
            out_of_sample
                .iter()
                .filter(|trade| trade.net_return_pct > 0.0)
                .count(),
        ),
        average_win_pct,
        average_loss_pct,
        payoff_ratio: average_win_pct
            .zip(average_loss_pct)
            .and_then(|(win, loss)| (loss.abs() > f64::EPSILON).then_some(win / loss.abs())),
        profit_factor: (gross_loss > f64::EPSILON).then_some(gross_profit / gross_loss),
        expectancy_pct: mean_values(trades.iter().map(|trade| trade.net_return_pct)),
        out_of_sample_expectancy_pct: mean_values(
            out_of_sample.iter().map(|trade| trade.net_return_pct),
        ),
    }
}

fn mean_values(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, count) = values.fold((0.0, 0usize), |(sum, count), value| {
        (sum + value, count + 1)
    });
    (count > 0).then_some(sum / count as f64)
}

fn ratio_pct(total: usize, positive: usize) -> Option<f64> {
    (total > 0).then_some(positive as f64 / total as f64 * 100.0)
}

/// Run a versioned rule with next-session-open execution, explicit costs and
/// a 70/30 chronological holdout. The current symbol's buy-and-hold return is
/// the local benchmark when no index series is available.
pub fn run_for_instrument(
    candles: &[Candle],
    instrument_code: &str,
    rule: BacktestRule,
    hold_days: usize,
    currency: Currency,
) -> Option<BacktestReport> {
    let hold_days = hold_days.clamp(3, 40);
    if candles.len() < 60 {
        return None;
    }

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
            minimum_commission: 0.0,
            sell_tax_bps: 0.0,
            slippage_bps_each_side: 8.0,
            other_fees_bps_each_side: 10.0,
            version: "hk-equity-costs-v1".into(),
        },
    };
    let dataset_version = dataset_content_id(
        &records,
        &DatasetHashMetadata {
            market: match currency {
                Currency::Cny => Market::AShare,
                Currency::Hkd => Market::HongKong,
            },
            instrument_code: instrument_code.into(),
            source: "visible-kline-provider".into(),
            adjustment: Adjustment::Forward,
        },
    );
    let spec = legacy_strategy_spec(rule, &dataset_version, hold_days as u16);
    let compiled = CompiledStrategy::compile(spec)
        .expect("built-in legacy strategy definitions must always validate");
    let signals = compiled.entry_signals(&records);
    let config = BacktestConfig {
        hold_days,
        costs,
        strategy_version: compiled.strategy_id().into(),
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

fn legacy_strategy_spec(rule: BacktestRule, universe_id: &str, hold_days: u16) -> StrategySpec {
    let mut spec = match rule {
        BacktestRule::Ma20CrossUp => LocalTemplate::MaTrendPullback.build(universe_id),
        BacktestRule::RsiOversoldExit => LocalTemplate::RsiOversoldRecovery.build(universe_id),
        BacktestRule::Breakout20 => LocalTemplate::NDayHighBreakout.build(universe_id),
    };
    spec.name = rule.label(false).into();
    spec.hypothesis = format!("兼容既有 {} 轻量回测规则", rule.label(false));
    spec.entry = match rule {
        BacktestRule::Ma20CrossUp => Expression::CrossesAbove {
            crosses_above: Crossing {
                left: ValueExpression::Indicator(IndicatorRef::Close { lag: 0 }),
                right: ValueExpression::Indicator(IndicatorRef::Sma { period: 20, lag: 0 }),
            },
        },
        BacktestRule::RsiOversoldExit => Expression::CrossesAbove {
            crosses_above: Crossing {
                left: ValueExpression::Indicator(IndicatorRef::Rsi { period: 14, lag: 0 }),
                right: ValueExpression::Constant { constant: 30.0 },
            },
        },
        BacktestRule::Breakout20 => Expression::Compare {
            compare: Comparison {
                left: ValueExpression::Indicator(IndicatorRef::Close { lag: 0 }),
                op: CompareOperator::Above,
                right: ValueExpression::Indicator(IndicatorRef::NDayHigh { period: 20, lag: 0 }),
            },
        },
    };
    spec.exit = ExitRule::HoldDays { hold_days };
    spec.position.size_pct = 100.0;
    spec.position.max_positions = 1;
    spec
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::backtest::SimulatedTrade;
    use crate::model::shared;

    fn simulated_trade(signal_index: usize, net_return_pct: f64) -> SimulatedTrade {
        SimulatedTrade {
            signal_index,
            entry_index: signal_index + 1,
            exit_index: signal_index + 2,
            gross_return_pct: net_return_pct + 0.2,
            net_return_pct,
            entry_cost_pct: 0.1,
            exit_cost_pct: 0.1,
        }
    }

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
        let report = run_for_instrument(
            &candles,
            "600000",
            BacktestRule::Ma20CrossUp,
            10,
            Currency::Cny,
        );
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
            let report = run_for_instrument(&candles, "600000", rule, 10, Currency::Cny).unwrap();
            assert!(!report.evidence.execution_rule.is_empty());
            assert!(!report.evidence.cost_model.version.is_empty());
            assert_eq!(report.sample_bars, candles.len());
        }
    }

    #[test]
    fn quality_metrics_keep_win_rate_payoff_and_oos_expectancy_separate() {
        let trades = vec![
            simulated_trade(10, 10.0),
            simulated_trade(20, -5.0),
            simulated_trade(80, 4.0),
            simulated_trade(90, -2.0),
        ];
        let quality = quality_metrics_for(&trades, 70);
        assert_eq!(quality.trade_count, 4);
        assert_eq!(quality.out_of_sample_count, 2);
        assert_eq!(quality.win_rate_pct, Some(50.0));
        assert_eq!(quality.out_of_sample_win_rate_pct, Some(50.0));
        assert_eq!(quality.average_win_pct, Some(7.0));
        assert_eq!(quality.average_loss_pct, Some(-3.5));
        assert_eq!(quality.payoff_ratio, Some(2.0));
        assert_eq!(quality.profit_factor, Some(2.0));
        assert_eq!(quality.expectancy_pct, Some(1.75));
        assert_eq!(quality.out_of_sample_expectancy_pct, Some(1.0));
    }
}
