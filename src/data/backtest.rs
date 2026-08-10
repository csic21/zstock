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
}
