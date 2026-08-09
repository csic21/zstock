use serde::{Deserialize, Serialize};

use super::market::CandleRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceGrade {
    None,
    InsufficientSample,
    InSampleExploration,
    OutOfSampleObservation,
    MultiPeriodStable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostModel {
    pub commission_bps_each_side: f64,
    pub sell_tax_bps: f64,
    pub slippage_bps_each_side: f64,
    pub other_fees_bps_each_side: f64,
    pub version: String,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            commission_bps_each_side: 3.0,
            sell_tax_bps: 5.0,
            slippage_bps_each_side: 5.0,
            other_fees_bps_each_side: 0.0,
            version: "cn-equity-costs-v1".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationMethod {
    InSample,
    Holdout { train_fraction_pct: u8 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestConfig {
    pub hold_days: usize,
    pub costs: CostModel,
    pub strategy_version: String,
    pub dataset_version: String,
    pub benchmark_name: String,
    pub minimum_trades: usize,
    pub validation: ValidationMethod,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulatedTrade {
    pub signal_index: usize,
    pub entry_index: usize,
    pub exit_index: usize,
    pub gross_return_pct: f64,
    pub net_return_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceReport {
    pub execution_rule: String,
    pub cost_model: CostModel,
    pub benchmark_name: String,
    pub interval: String,
    pub trades: Vec<SimulatedTrade>,
    pub average_net_return_pct: f64,
    pub benchmark_return_pct: f64,
    pub excess_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub confidence_interval_95_pct: Option<(f64, f64)>,
    pub return_distribution_pct: Vec<f64>,
    pub out_of_sample_trades: usize,
    pub validation_method: ValidationMethod,
    pub bias_checks: Vec<String>,
    pub strategy_version: String,
    pub dataset_version: String,
    pub evidence_grade: EvidenceGrade,
}

pub fn run_next_open<F>(
    candles: &[CandleRecord],
    benchmark: &[CandleRecord],
    config: &BacktestConfig,
    signal: F,
) -> EvidenceReport
where
    F: Fn(&[CandleRecord], usize) -> bool,
{
    let mut trades = Vec::new();
    let hold_days = config.hold_days.max(1);
    let mut signal_index = 1;
    while signal_index + hold_days + 1 < candles.len() {
        if !signal(&candles[..=signal_index], signal_index) {
            signal_index += 1;
            continue;
        }
        let entry_index = signal_index + 1;
        let exit_index = (entry_index + hold_days).min(candles.len() - 1);
        let entry = candles[entry_index].open;
        let exit = candles[exit_index].open;
        if entry.is_finite() && exit.is_finite() && entry > 0.0 && exit > 0.0 {
            let gross_return_pct = (exit / entry - 1.0) * 100.0;
            let total_cost_bps = config.costs.commission_bps_each_side * 2.0
                + config.costs.slippage_bps_each_side * 2.0
                + config.costs.other_fees_bps_each_side * 2.0
                + config.costs.sell_tax_bps;
            trades.push(SimulatedTrade {
                signal_index,
                entry_index,
                exit_index,
                gross_return_pct,
                net_return_pct: gross_return_pct - total_cost_bps / 100.0,
            });
        }
        signal_index = exit_index + 1;
    }

    let average_net_return_pct = mean(trades.iter().map(|trade| trade.net_return_pct));
    let strategy_curve = equity_curve(&trades);
    let max_drawdown_pct = max_drawdown(&strategy_curve);
    let benchmark_return_pct = period_return(benchmark);
    let strategy_return_pct = strategy_curve.last().copied().unwrap_or(1.0) * 100.0 - 100.0;
    let validation_start = match &config.validation {
        ValidationMethod::InSample => candles.len(),
        ValidationMethod::Holdout { train_fraction_pct } => {
            candles.len() * usize::from((*train_fraction_pct).clamp(10, 90)) / 100
        }
    };
    let out_of_sample_trades = trades
        .iter()
        .filter(|trade| trade.signal_index >= validation_start)
        .count();
    let out_of_sample_excess = mean(
        trades
            .iter()
            .filter(|trade| trade.signal_index >= validation_start)
            .map(|trade| trade.net_return_pct),
    ) - benchmark_return_pct;
    let evidence_grade = grade(
        &trades,
        out_of_sample_trades,
        config.minimum_trades,
        out_of_sample_excess,
    );
    let confidence_interval_95_pct = confidence_interval(&trades);
    let return_distribution_pct = trades.iter().map(|trade| trade.net_return_pct).collect();
    let interval = match (candles.first(), candles.last()) {
        (Some(first), Some(last)) => format!("{}..{}", first.time, last.time),
        _ => "empty".into(),
    };
    EvidenceReport {
        execution_rule: format!("signal day + 1 open; exit after {hold_days} sessions at open"),
        cost_model: config.costs.clone(),
        benchmark_name: config.benchmark_name.clone(),
        interval,
        trades,
        average_net_return_pct,
        benchmark_return_pct,
        excess_return_pct: strategy_return_pct - benchmark_return_pct,
        max_drawdown_pct,
        confidence_interval_95_pct,
        return_distribution_pct,
        out_of_sample_trades,
        validation_method: config.validation.clone(),
        bias_checks: vec![
            "信号函数只接收截至信号日的历史切片（前视检查）".into(),
            "持仓不重叠；退出后才寻找下一信号".into(),
            "数据集范围与版本固定；幸存者偏差仍需由数据集清单审计".into(),
        ],
        strategy_version: config.strategy_version.clone(),
        dataset_version: config.dataset_version.clone(),
        evidence_grade,
    }
}

fn period_return(candles: &[CandleRecord]) -> f64 {
    match (candles.first(), candles.last()) {
        (Some(first), Some(last)) if first.open > 0.0 => (last.close / first.open - 1.0) * 100.0,
        _ => 0.0,
    }
}

fn equity_curve(trades: &[SimulatedTrade]) -> Vec<f64> {
    let mut equity = 1.0;
    trades
        .iter()
        .map(|trade| {
            equity *= 1.0 + trade.net_return_pct / 100.0;
            equity
        })
        .collect()
}

fn max_drawdown(curve: &[f64]) -> f64 {
    let mut peak = 1.0_f64;
    let mut worst = 0.0_f64;
    for value in curve {
        peak = peak.max(*value);
        if peak > 0.0 {
            worst = worst.min((*value / peak - 1.0) * 100.0);
        }
    }
    worst
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<_> = values.collect();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn confidence_interval(trades: &[SimulatedTrade]) -> Option<(f64, f64)> {
    if trades.len() < 2 {
        return None;
    }
    let average = mean(trades.iter().map(|trade| trade.net_return_pct));
    let variance = trades
        .iter()
        .map(|trade| (trade.net_return_pct - average).powi(2))
        .sum::<f64>()
        / (trades.len() - 1) as f64;
    let margin = 1.96 * (variance / trades.len() as f64).sqrt();
    Some((average - margin, average + margin))
}

fn grade(
    trades: &[SimulatedTrade],
    out_of_sample_trades: usize,
    minimum_trades: usize,
    out_of_sample_excess_pct: f64,
) -> EvidenceGrade {
    if trades.is_empty() {
        EvidenceGrade::None
    } else if trades.len() < minimum_trades {
        EvidenceGrade::InsufficientSample
    } else if out_of_sample_trades == 0 || out_of_sample_excess_pct <= 0.0 {
        EvidenceGrade::InSampleExploration
    } else if stable_across_periods(trades) {
        EvidenceGrade::MultiPeriodStable
    } else {
        EvidenceGrade::OutOfSampleObservation
    }
}

fn stable_across_periods(trades: &[SimulatedTrade]) -> bool {
    if trades.len() < 9 {
        return false;
    }
    let chunk_size = trades.len().div_ceil(3);
    trades
        .chunks(chunk_size)
        .take(3)
        .all(|chunk| mean(chunk.iter().map(|trade| trade.net_return_pct)) > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candles(count: usize) -> Vec<CandleRecord> {
        (0..count)
            .map(|index| CandleRecord {
                time: format!("d{index:03}"),
                open: 100.0 + index as f64,
                high: 101.0 + index as f64,
                low: 99.0 + index as f64,
                close: 100.5 + index as f64,
                volume: 1_000,
            })
            .collect()
    }

    fn config() -> BacktestConfig {
        BacktestConfig {
            hold_days: 3,
            costs: CostModel::default(),
            strategy_version: "strategy-1".into(),
            dataset_version: "fixture-1".into(),
            benchmark_name: "fixture".into(),
            minimum_trades: 2,
            validation: ValidationMethod::Holdout {
                train_fraction_pct: 70,
            },
        }
    }

    #[test]
    fn executes_after_signal_and_deducts_costs() {
        let data = candles(20);
        let report = run_next_open(&data, &data, &config(), |_, index| index == 2);
        let trade = &report.trades[0];
        assert_eq!(
            (trade.signal_index, trade.entry_index, trade.exit_index),
            (2, 3, 6)
        );
        assert!(trade.net_return_pct < trade.gross_return_pct);
        assert!(report.execution_rule.contains("+ 1 open"));
    }

    #[test]
    fn changing_future_data_cannot_change_past_signal() {
        let original = candles(20);
        let mut changed = original.clone();
        for candle in &mut changed[10..] {
            candle.close *= 50.0;
        }
        let signal = |history: &[CandleRecord], index: usize| {
            index == 4 && history[index].close > history[index - 1].close
        };
        let before = run_next_open(&original, &original, &config(), signal);
        let after = run_next_open(&changed, &original, &config(), signal);
        assert_eq!(before.trades[0].signal_index, after.trades[0].signal_index);
        assert_eq!(before.trades[0].entry_index, after.trades[0].entry_index);
    }

    #[test]
    fn report_always_carries_cost_benchmark_interval_sample_and_versions() {
        let data = candles(40);
        let report = run_next_open(&data, &data, &config(), |_, index| index % 5 == 0);
        assert!(!report.execution_rule.is_empty());
        assert!(!report.cost_model.version.is_empty());
        assert!(!report.benchmark_name.is_empty());
        assert!(!report.interval.is_empty());
        assert_eq!(report.return_distribution_pct.len(), report.trades.len());
        assert_eq!(report.strategy_version, "strategy-1");
        assert_eq!(report.dataset_version, "fixture-1");
    }
}
