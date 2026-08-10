use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::market::{Adjustment, CandleRecord, Market};

pub mod config;
pub mod metrics;
pub mod portfolio;
pub mod report;
pub mod validation;

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
    pub minimum_commission: f64,
    pub sell_tax_bps: f64,
    pub slippage_bps_each_side: f64,
    pub other_fees_bps_each_side: f64,
    pub version: String,
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            commission_bps_each_side: 3.0,
            minimum_commission: 5.0,
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
    pub entry_cost_pct: f64,
    pub exit_cost_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DailyEquityPoint {
    pub time: String,
    pub strategy_equity: f64,
    pub benchmark_equity: f64,
    pub in_position: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeriodStatistics {
    pub label: String,
    pub interval: String,
    pub start_index: usize,
    pub end_index: usize,
    pub trades: usize,
    pub average_net_return_pct: f64,
    pub strategy_return_pct: f64,
    pub benchmark_return_pct: f64,
    pub excess_return_pct: f64,
    pub max_drawdown_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetHashMetadata {
    pub market: Market,
    pub instrument_code: String,
    pub source: String,
    pub adjustment: Adjustment,
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
    pub daily_equity: Vec<DailyEquityPoint>,
    pub total_statistics: PeriodStatistics,
    pub training_statistics: Option<PeriodStatistics>,
    pub validation_statistics: Option<PeriodStatistics>,
    pub test_statistics: Option<PeriodStatistics>,
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
            let entry_cost_pct = entry_cost_rate(&config.costs) * 100.0;
            let exit_cost_pct = exit_cost_rate(&config.costs) * 100.0;
            let net_return_pct = (exit * (1.0 - exit_cost_pct / 100.0)
                / (entry * (1.0 + entry_cost_pct / 100.0))
                - 1.0)
                * 100.0;
            trades.push(SimulatedTrade {
                signal_index,
                entry_index,
                exit_index,
                gross_return_pct,
                net_return_pct,
                entry_cost_pct,
                exit_cost_pct,
            });
        }
        signal_index = exit_index + 1;
    }

    let average_net_return_pct = mean(trades.iter().map(|trade| trade.net_return_pct));
    let validation_start = match &config.validation {
        ValidationMethod::InSample => candles.len(),
        ValidationMethod::Holdout { train_fraction_pct } => {
            candles.len() * usize::from((*train_fraction_pct).clamp(10, 90)) / 100
        }
    };
    let total_statistics = statistics_for_period(
        "total",
        candles,
        benchmark,
        &trades,
        0,
        candles.len().saturating_sub(1),
    );
    let training_statistics =
        (validation_start > 0 && validation_start < candles.len()).then(|| {
            statistics_for_period(
                "training",
                candles,
                benchmark,
                &trades,
                0,
                validation_start - 1,
            )
        });
    let validation_statistics = (validation_start < candles.len()).then(|| {
        statistics_for_period(
            "validation",
            candles,
            benchmark,
            &trades,
            validation_start,
            candles.len() - 1,
        )
    });
    let out_of_sample: Vec<_> = trades
        .iter()
        .filter(|trade| trade.signal_index >= validation_start)
        .cloned()
        .collect();
    let out_of_sample_trades = out_of_sample.len();
    let out_of_sample_excess = validation_statistics
        .as_ref()
        .map_or(0.0, |stats| stats.excess_return_pct);
    let evidence_grade = grade(
        &trades,
        &out_of_sample,
        config.minimum_trades,
        out_of_sample_excess,
    );
    let daily_equity = daily_equity(candles, benchmark, &trades, 0, candles.len());
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
        benchmark_return_pct: total_statistics.benchmark_return_pct,
        excess_return_pct: total_statistics.excess_return_pct,
        max_drawdown_pct: total_statistics.max_drawdown_pct,
        confidence_interval_95_pct,
        return_distribution_pct,
        daily_equity,
        total_statistics,
        training_statistics,
        validation_statistics,
        test_statistics: None,
        out_of_sample_trades,
        validation_method: config.validation.clone(),
        bias_checks: vec![
            "信号函数只接收截至信号日的历史切片（前视检查）".into(),
            "持仓不重叠；退出后才寻找下一信号".into(),
            "策略与基准按相同日频区间计算；空仓现金收益按 0 处理".into(),
            "数据集范围与版本固定；幸存者偏差仍需由数据集清单审计".into(),
        ],
        strategy_version: config.strategy_version.clone(),
        dataset_version: config.dataset_version.clone(),
        evidence_grade,
    }
}

pub fn dataset_content_id(candles: &[CandleRecord], metadata: &DatasetHashMetadata) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"zstock-dataset-v2\0");
    hash_bytes(
        &mut hasher,
        match metadata.market {
            Market::AShare => b"a_share",
            Market::HongKong => b"hong_kong",
        },
    );
    hash_bytes(&mut hasher, metadata.instrument_code.as_bytes());
    hash_bytes(&mut hasher, metadata.source.as_bytes());
    hash_bytes(
        &mut hasher,
        match metadata.adjustment {
            Adjustment::None => b"none",
            Adjustment::Forward => b"forward",
            Adjustment::Backward => b"backward",
        },
    );
    hasher.update((candles.len() as u64).to_be_bytes());
    for candle in candles {
        hash_bytes(&mut hasher, candle.time.as_bytes());
        hasher.update(candle.open.to_bits().to_be_bytes());
        hasher.update(candle.high.to_bits().to_be_bytes());
        hasher.update(candle.low.to_bits().to_be_bytes());
        hasher.update(candle.close.to_bits().to_be_bytes());
        hasher.update(candle.volume.to_be_bytes());
    }
    let digest = hasher.finalize();
    let mut output = String::with_capacity(7 + digest.len() * 2);
    output.push_str("sha256:");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn hash_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn entry_cost_rate(costs: &CostModel) -> f64 {
    (costs.commission_bps_each_side + costs.slippage_bps_each_side + costs.other_fees_bps_each_side)
        / 10_000.0
}

fn exit_cost_rate(costs: &CostModel) -> f64 {
    (costs.commission_bps_each_side
        + costs.slippage_bps_each_side
        + costs.other_fees_bps_each_side
        + costs.sell_tax_bps)
        / 10_000.0
}

fn statistics_for_period(
    label: &str,
    candles: &[CandleRecord],
    benchmark: &[CandleRecord],
    all_trades: &[SimulatedTrade],
    start: usize,
    end: usize,
) -> PeriodStatistics {
    if candles.is_empty() || start >= candles.len() || start > end {
        return PeriodStatistics {
            label: label.into(),
            interval: "empty".into(),
            start_index: start,
            end_index: end,
            trades: 0,
            average_net_return_pct: 0.0,
            strategy_return_pct: 0.0,
            benchmark_return_pct: 0.0,
            excess_return_pct: 0.0,
            max_drawdown_pct: 0.0,
        };
    }
    let end = end.min(candles.len() - 1);
    let trades: Vec<_> = all_trades
        .iter()
        .filter(|trade| trade.signal_index >= start && trade.exit_index <= end)
        .cloned()
        .collect();
    let curve = daily_equity(candles, benchmark, &trades, start, end + 1);
    let strategy_values: Vec<_> = curve.iter().map(|point| point.strategy_equity).collect();
    let strategy_return_pct = strategy_values.last().copied().unwrap_or(1.0) * 100.0 - 100.0;
    let benchmark_return_pct = curve
        .last()
        .map_or(0.0, |point| (point.benchmark_equity - 1.0) * 100.0);
    PeriodStatistics {
        label: label.into(),
        interval: format!("{}..{}", candles[start].time, candles[end].time),
        start_index: start,
        end_index: end,
        trades: trades.len(),
        average_net_return_pct: mean(trades.iter().map(|trade| trade.net_return_pct)),
        strategy_return_pct,
        benchmark_return_pct,
        excess_return_pct: strategy_return_pct - benchmark_return_pct,
        max_drawdown_pct: max_drawdown(&strategy_values),
    }
}

fn daily_equity(
    candles: &[CandleRecord],
    benchmark: &[CandleRecord],
    trades: &[SimulatedTrade],
    start: usize,
    end_exclusive: usize,
) -> Vec<DailyEquityPoint> {
    if start >= candles.len() || start >= end_exclusive {
        return Vec::new();
    }
    let end_exclusive = end_exclusive.min(candles.len());
    let benchmark_values = benchmark_curve(candles, benchmark, start, end_exclusive);
    let mut equity = 1.0;
    let mut trade_index = 0;
    let mut points = Vec::with_capacity(end_exclusive - start);

    for (offset, index) in (start..end_exclusive).enumerate() {
        while trade_index < trades.len() && trades[trade_index].exit_index < index {
            equity *= 1.0 + trades[trade_index].net_return_pct / 100.0;
            trade_index += 1;
        }
        let mut marked_equity = equity;
        let mut in_position = false;
        if let Some(trade) = trades.get(trade_index) {
            if index >= trade.entry_index && index < trade.exit_index {
                let entry = candles[trade.entry_index].open;
                let close = candles[index].close;
                if entry.is_finite() && entry > 0.0 && close.is_finite() && close > 0.0 {
                    marked_equity = equity * close / entry / (1.0 + trade.entry_cost_pct / 100.0);
                    in_position = true;
                }
            } else if index == trade.exit_index {
                equity *= 1.0 + trade.net_return_pct / 100.0;
                marked_equity = equity;
                trade_index += 1;
            }
        }
        points.push(DailyEquityPoint {
            time: candles[index].time.clone(),
            strategy_equity: marked_equity,
            benchmark_equity: benchmark_values[offset],
            in_position,
        });
    }
    points
}

fn benchmark_curve(
    candles: &[CandleRecord],
    benchmark: &[CandleRecord],
    start: usize,
    end_exclusive: usize,
) -> Vec<f64> {
    let mut base_close = None;
    let mut last_equity = 1.0;
    (start..end_exclusive)
        .map(|index| {
            let matching = benchmark
                .iter()
                .find(|bar| bar.time == candles[index].time)
                .filter(|bar| bar.close.is_finite() && bar.close > 0.0);
            if let Some(bar) = matching {
                let base = *base_close.get_or_insert(bar.close);
                last_equity = bar.close / base;
            }
            last_equity
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
    out_of_sample_trades: &[SimulatedTrade],
    minimum_trades: usize,
    out_of_sample_excess_pct: f64,
) -> EvidenceGrade {
    if trades.is_empty() {
        EvidenceGrade::None
    } else if trades.len() < minimum_trades {
        EvidenceGrade::InsufficientSample
    } else if out_of_sample_trades.is_empty() || out_of_sample_excess_pct <= 0.0 {
        EvidenceGrade::InSampleExploration
    } else if out_of_sample_trades.len() >= minimum_oos_trades(minimum_trades)
        && stable_across_periods(out_of_sample_trades)
    {
        EvidenceGrade::MultiPeriodStable
    } else {
        EvidenceGrade::OutOfSampleObservation
    }
}

fn minimum_oos_trades(minimum_trades: usize) -> usize {
    minimum_trades.div_ceil(3).max(9)
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
    fn future_data_change_does_not_change_past_signal() {
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
    fn dataset_hash_changes_when_price_changes() {
        let original = candles(20);
        let mut changed = original.clone();
        changed[10].close += 0.01;
        let metadata = DatasetHashMetadata {
            market: Market::AShare,
            instrument_code: "600000".into(),
            source: "fixture-v1".into(),
            adjustment: Adjustment::Forward,
        };

        let original_id = dataset_content_id(&original, &metadata);
        let changed_id = dataset_content_id(&changed, &metadata);

        assert_ne!(original_id, changed_id);
        assert!(original_id.starts_with("sha256:"));
        assert_eq!(original_id.len(), 71);
    }

    #[test]
    fn oos_strategy_and_benchmark_use_same_interval() {
        let data = candles(20);
        let report = run_next_open(&data, &data, &config(), |_, index| index == 14);
        let validation = report.validation_statistics.unwrap();
        let expected_benchmark = (data[19].close / data[14].close - 1.0) * 100.0;

        assert_eq!(validation.interval, "d014..d019");
        assert_eq!((validation.start_index, validation.end_index), (14, 19));
        assert_eq!(validation.trades, 1);
        assert!((validation.benchmark_return_pct - expected_benchmark).abs() < 1e-10);
        assert!(
            (validation.excess_return_pct
                - (validation.strategy_return_pct - validation.benchmark_return_pct))
                .abs()
                < 1e-10
        );
    }

    #[test]
    fn daily_drawdown_includes_intratrade_loss() {
        let mut data = candles(14);
        for candle in &mut data {
            candle.open = 100.0;
            candle.high = 101.0;
            candle.low = 99.0;
            candle.close = 100.0;
        }
        data[5].close = 50.0;
        data[5].low = 49.0;
        let mut fixture_config = config();
        fixture_config.hold_days = 4;

        let report = run_next_open(&data, &data, &fixture_config, |_, index| index == 2);

        assert!(report.max_drawdown_pct < -40.0);
        assert!(report.daily_equity[5].in_position);
        assert!(report.daily_equity[5].strategy_equity < 0.6);
    }

    #[test]
    fn one_positive_oos_trade_cannot_be_multi_period_stable() {
        let data = candles(60);
        let mut benchmark = candles(60);
        for bar in &mut benchmark {
            bar.open = 100.0;
            bar.high = 100.0;
            bar.low = 100.0;
            bar.close = 100.0;
        }
        let mut fixture_config = config();
        fixture_config.hold_days = 1;
        fixture_config.minimum_trades = 10;
        fixture_config.validation = ValidationMethod::Holdout {
            train_fraction_pct: 90,
        };

        let report = run_next_open(&data, &benchmark, &fixture_config, |_, _| true);

        assert_eq!(report.out_of_sample_trades, 1);
        assert_eq!(report.evidence_grade, EvidenceGrade::OutOfSampleObservation);
    }

    #[test]
    fn costs_are_applied_on_correct_sides() {
        let mut data = candles(12);
        for bar in &mut data {
            bar.open = 100.0;
            bar.close = 100.0;
        }
        let mut fixture_config = config();
        fixture_config.costs = CostModel {
            commission_bps_each_side: 10.0,
            minimum_commission: 0.0,
            sell_tax_bps: 20.0,
            slippage_bps_each_side: 0.0,
            other_fees_bps_each_side: 0.0,
            version: "sided-cost-fixture".into(),
        };

        let report = run_next_open(&data, &data, &fixture_config, |_, index| index == 2);
        let trade = &report.trades[0];
        let expected = ((1.0 - 0.003) / (1.0 + 0.001) - 1.0) * 100.0;

        assert!((trade.entry_cost_pct - 0.1).abs() < 1e-12);
        assert!((trade.exit_cost_pct - 0.3).abs() < 1e-12);
        assert!((trade.net_return_pct - expected).abs() < 1e-12);
    }

    #[test]
    fn same_fixture_produces_identical_report() {
        let data = candles(40);
        let run = || run_next_open(&data, &data, &config(), |_, index| index % 5 == 0);

        assert_eq!(run(), run());
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
