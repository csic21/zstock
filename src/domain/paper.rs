use serde::{Deserialize, Serialize};

use super::backtest::report::PortfolioBacktestReport;
use super::dataset::FrozenDataset;
use super::market::InstrumentId;
use super::strategy::{CompiledStrategy, PositionContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperCandidateStatus {
    Observing,
    Paused,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaperCandidate {
    pub id: String,
    pub strategy_id: String,
    pub dataset_id: String,
    pub experiment_id: String,
    pub created_at: String,
    pub status: PaperCandidateStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperSignalKind {
    Entry,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperSignalOutcome {
    PendingNextOpen,
    Filled,
    MissedNoNextSession,
    MissedSuspended,
    MissedInvalidOpen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaperSignal {
    pub id: String,
    pub candidate_id: String,
    pub strategy_id: String,
    pub instrument: InstrumentId,
    pub signal_date: String,
    pub kind: PaperSignalKind,
    pub outcome: PaperSignalOutcome,
    pub signal_close: f64,
    pub execution_date: Option<String>,
    pub execution_price: Option<f64>,
    pub miss_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaperTrade {
    pub id: String,
    pub candidate_id: String,
    pub instrument: InstrumentId,
    pub entry_signal_date: String,
    pub entry_date: String,
    pub entry_price: f64,
    pub exit_signal_date: String,
    pub exit_date: String,
    pub exit_price: f64,
    pub holding_sessions: usize,
    pub gross_return_pct: f64,
    pub entry_gap_bps: f64,
    pub exit_gap_bps: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaperOpenPosition {
    pub instrument: InstrumentId,
    pub entry_signal_date: String,
    pub entry_date: String,
    pub entry_price: f64,
    pub holding_sessions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaperRunResult {
    pub candidate_id: String,
    pub strategy_id: String,
    pub observation_dataset_id: String,
    pub observation_content_sha256: String,
    pub as_of: String,
    pub generated_at: String,
    pub signals: Vec<PaperSignal>,
    pub trades: Vec<PaperTrade>,
    pub open_positions: Vec<PaperOpenPosition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaperBehaviorComparison {
    pub paper_signal_count: usize,
    pub paper_trade_count: usize,
    pub missed_signal_pct: f64,
    pub average_holding_sessions: f64,
    pub backtest_average_holding_sessions: f64,
    pub average_execution_gap_bps: f64,
    pub backtest_trade_count: usize,
    pub observation_days: i64,
    pub minimum_observation_met: bool,
    pub warnings: Vec<String>,
}

pub fn run_paper_history(
    candidate: &PaperCandidate,
    strategy: &CompiledStrategy,
    dataset: &FrozenDataset,
    as_of: &str,
    generated_at: &str,
) -> PaperRunResult {
    let mut signals = Vec::new();
    let mut trades = Vec::new();
    let mut open_positions = Vec::new();
    for series in &dataset.series {
        let bars: Vec<_> = series
            .candles
            .iter()
            .filter(|bar| bar.time.as_str() <= as_of)
            .cloned()
            .collect();
        let mut position: Option<(usize, String, String, f64)> = None;
        for index in 0..bars.len() {
            if let Some((entry_index, entry_signal_date, entry_date, entry_price)) = &position {
                if strategy.should_exit(
                    &bars[..=index],
                    index,
                    PositionContext {
                        entry_price: *entry_price,
                        holding_days: index.saturating_sub(*entry_index) + 1,
                    },
                ) {
                    let signal = execution_signal(
                        candidate,
                        &series.instrument,
                        PaperSignalKind::Exit,
                        &bars,
                        index,
                    );
                    if signal.outcome == PaperSignalOutcome::Filled {
                        let exit_price = signal.execution_price.expect("filled signal has price");
                        let exit_date = signal
                            .execution_date
                            .clone()
                            .expect("filled signal has date");
                        trades.push(PaperTrade {
                            id: stable_event_id(
                                candidate.id.as_str(),
                                &series.instrument,
                                &signal.signal_date,
                                "trade",
                            ),
                            candidate_id: candidate.id.clone(),
                            instrument: series.instrument.clone(),
                            entry_signal_date: entry_signal_date.clone(),
                            entry_date: entry_date.clone(),
                            entry_price: *entry_price,
                            exit_signal_date: signal.signal_date.clone(),
                            exit_date,
                            exit_price,
                            holding_sessions: index.saturating_sub(*entry_index) + 1,
                            gross_return_pct: (exit_price / *entry_price - 1.0) * 100.0,
                            entry_gap_bps: execution_gap_bps(
                                &bars,
                                entry_signal_date,
                                *entry_price,
                                true,
                            ),
                            exit_gap_bps: (bars[index].close - exit_price) / bars[index].close
                                * 10_000.0,
                        });
                        position = None;
                    }
                    signals.push(signal);
                }
            } else if strategy.entry_signal(&bars[..=index], index) {
                let signal = execution_signal(
                    candidate,
                    &series.instrument,
                    PaperSignalKind::Entry,
                    &bars,
                    index,
                );
                if signal.outcome == PaperSignalOutcome::Filled {
                    position = Some((
                        index + 1,
                        signal.signal_date.clone(),
                        signal
                            .execution_date
                            .clone()
                            .expect("filled signal has date"),
                        signal.execution_price.expect("filled signal has price"),
                    ));
                }
                signals.push(signal);
            }
        }
        if let Some((entry_index, entry_signal_date, entry_date, entry_price)) = position {
            open_positions.push(PaperOpenPosition {
                instrument: series.instrument.clone(),
                entry_signal_date,
                entry_date,
                entry_price,
                holding_sessions: bars.len().saturating_sub(entry_index),
            });
        }
    }
    signals.sort_by(|left, right| {
        left.signal_date
            .cmp(&right.signal_date)
            .then_with(|| left.instrument.cmp(&right.instrument))
            .then_with(|| signal_order(left.kind).cmp(&signal_order(right.kind)))
    });
    trades.sort_by(|left, right| {
        left.exit_date
            .cmp(&right.exit_date)
            .then_with(|| left.instrument.cmp(&right.instrument))
    });
    PaperRunResult {
        candidate_id: candidate.id.clone(),
        strategy_id: candidate.strategy_id.clone(),
        observation_dataset_id: dataset.manifest.id.clone(),
        observation_content_sha256: dataset.manifest.content_sha256.clone(),
        as_of: as_of.into(),
        generated_at: generated_at.into(),
        signals,
        trades,
        open_positions,
    }
}

pub fn compare_with_backtest(
    paper: &PaperRunResult,
    backtest: &PortfolioBacktestReport,
) -> PaperBehaviorComparison {
    let missed = paper
        .signals
        .iter()
        .filter(|signal| !matches!(signal.outcome, PaperSignalOutcome::Filled))
        .count();
    let missed_signal_pct = percent(missed, paper.signals.len());
    let average_holding_sessions = mean(
        paper
            .trades
            .iter()
            .map(|trade| trade.holding_sessions as f64),
    );
    let average_execution_gap_bps = mean(
        paper
            .trades
            .iter()
            .flat_map(|trade| [trade.entry_gap_bps.abs(), trade.exit_gap_bps.abs()]),
    );
    let observation_days = paper
        .signals
        .first()
        .and_then(|first| {
            let start = chrono::NaiveDate::parse_from_str(&first.signal_date, "%Y-%m-%d").ok()?;
            let end = chrono::NaiveDate::parse_from_str(&paper.as_of, "%Y-%m-%d").ok()?;
            Some((end - start).num_days().max(0))
        })
        .unwrap_or(0);
    let minimum_observation_met = observation_days >= 28 || paper.signals.len() >= 20;
    let mut warnings = Vec::new();
    if !minimum_observation_met {
        warnings.push("尚未达到连续 4 周或 20 个新信号的最低观察条件".into());
    }
    if average_execution_gap_bps > backtest.config.costs.slippage_bps_each_side * 2.0 {
        warnings.push("模拟成交跳空明显超过回测滑点压力假设".into());
    }
    if missed_signal_pct > 20.0 {
        warnings.push("超过 20% 理论信号无法成交或仍待下一交易日".into());
    }
    PaperBehaviorComparison {
        paper_signal_count: paper.signals.len(),
        paper_trade_count: paper.trades.len(),
        missed_signal_pct,
        average_holding_sessions,
        backtest_average_holding_sessions: backtest.metrics.average_holding_sessions,
        average_execution_gap_bps,
        backtest_trade_count: backtest.metrics.trade_count,
        observation_days,
        minimum_observation_met,
        warnings,
    }
}

fn execution_signal(
    candidate: &PaperCandidate,
    instrument: &InstrumentId,
    kind: PaperSignalKind,
    bars: &[super::market::CandleRecord],
    signal_index: usize,
) -> PaperSignal {
    let signal_bar = &bars[signal_index];
    let next = bars.get(signal_index + 1);
    let (outcome, execution_date, execution_price, miss_reason) = match next {
        None => (
            PaperSignalOutcome::PendingNextOpen,
            None,
            None,
            Some("等待下一有效交易日开盘".into()),
        ),
        Some(bar) if bar.volume == 0 => (
            PaperSignalOutcome::MissedSuspended,
            Some(bar.time.clone()),
            None,
            Some("下一交易日停牌或成交量为零".into()),
        ),
        Some(bar) if !bar.open.is_finite() || bar.open <= 0.0 => (
            PaperSignalOutcome::MissedInvalidOpen,
            Some(bar.time.clone()),
            None,
            Some("下一交易日开盘价无效".into()),
        ),
        Some(bar) => (
            PaperSignalOutcome::Filled,
            Some(bar.time.clone()),
            Some(bar.open),
            None,
        ),
    };
    let kind_name = match kind {
        PaperSignalKind::Entry => "entry",
        PaperSignalKind::Exit => "exit",
    };
    PaperSignal {
        id: stable_event_id(&candidate.id, instrument, &signal_bar.time, kind_name),
        candidate_id: candidate.id.clone(),
        strategy_id: candidate.strategy_id.clone(),
        instrument: instrument.clone(),
        signal_date: signal_bar.time.clone(),
        kind,
        outcome,
        signal_close: signal_bar.close,
        execution_date,
        execution_price,
        miss_reason,
    }
}

fn stable_event_id(
    candidate_id: &str,
    instrument: &InstrumentId,
    date: &str,
    kind: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!(
        "paper-event-v1\0{candidate_id}\0{}\0{date}\0{kind}",
        instrument.storage_key()
    ));
    let suffix: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("paper-sha256:{suffix}")
}

fn execution_gap_bps(
    bars: &[super::market::CandleRecord],
    signal_date: &str,
    execution_price: f64,
    is_entry: bool,
) -> f64 {
    let Some(signal) = bars.iter().find(|bar| bar.time == signal_date) else {
        return 0.0;
    };
    let raw = (execution_price - signal.close) / signal.close * 10_000.0;
    if is_entry { raw } else { -raw }
}

const fn signal_order(kind: PaperSignalKind) -> u8 {
    match kind {
        PaperSignalKind::Exit => 0,
        PaperSignalKind::Entry => 1,
    }
}

fn percent(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64 * 100.0
    }
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, count) = values.fold((0.0, 0_usize), |(sum, count), value| {
        (sum + value, count + 1)
    });
    if count == 0 { 0.0 } else { sum / count as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dataset::{DataQualityIssue, DatasetManifest, DateInterval, FrozenSeries};
    use crate::domain::market::{Adjustment, AssetType, CandleRecord, Market};
    use crate::domain::strategy::{
        CompareOperator, Comparison, ExitRule, Expression, LocalTemplate, ValueExpression,
    };

    fn fixture() -> (PaperCandidate, CompiledStrategy, FrozenDataset) {
        let instrument = InstrumentId {
            market: Market::AShare,
            asset_type: AssetType::Stock,
            code: "600000".into(),
        };
        let candles = (0..8)
            .map(|index| CandleRecord {
                time: format!("2026-01-{:02}", index + 1),
                open: 10.0 + index as f64,
                high: 11.0 + index as f64,
                low: 9.0 + index as f64,
                close: 10.5 + index as f64,
                volume: 1_000,
            })
            .collect();
        let series = FrozenSeries {
            instrument: instrument.clone(),
            source: "fixture".into(),
            adjustment: Adjustment::Forward,
            candles,
        };
        let dataset = FrozenDataset {
            manifest: DatasetManifest {
                id: "dataset".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                market: Market::AShare,
                adjustment: Adjustment::Forward,
                source_versions: vec!["fixture".into()],
                instruments: vec![instrument],
                interval: DateInterval {
                    start: "2026-01-01".into(),
                    end: "2026-01-08".into(),
                },
                content_sha256: "hash".into(),
                known_biases: vec![],
                quality_issues: Vec::<DataQualityIssue>::new(),
            },
            series: vec![series],
        };
        let mut spec = LocalTemplate::NDayHighBreakout.build("dataset");
        spec.entry = Expression::Compare {
            compare: Comparison {
                left: ValueExpression::Constant { constant: 2.0 },
                op: CompareOperator::Above,
                right: ValueExpression::Constant { constant: 1.0 },
            },
        };
        spec.exit = ExitRule::HoldDays { hold_days: 2 };
        let strategy = CompiledStrategy::compile(spec).unwrap();
        let candidate = PaperCandidate {
            id: strategy.strategy_id().into(),
            strategy_id: strategy.strategy_id().into(),
            dataset_id: "dataset".into(),
            experiment_id: "experiment".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            status: PaperCandidateStatus::Observing,
        };
        (candidate, strategy, dataset)
    }

    #[test]
    fn repeated_daily_run_is_byte_for_byte_idempotent() {
        let (candidate, strategy, dataset) = fixture();
        let first = run_paper_history(
            &candidate,
            &strategy,
            &dataset,
            "2026-01-08",
            "2026-01-08T10:00:00Z",
        );
        let second = run_paper_history(
            &candidate,
            &strategy,
            &dataset,
            "2026-01-08",
            "2026-01-08T10:00:00Z",
        );
        assert_eq!(first, second);
        let ids: std::collections::BTreeSet<_> =
            first.signals.iter().map(|signal| &signal.id).collect();
        assert_eq!(ids.len(), first.signals.len());
        assert!(!first.trades.is_empty());
    }

    #[test]
    fn no_next_session_is_recorded_instead_of_fabricating_a_fill() {
        let (candidate, strategy, dataset) = fixture();
        let result = run_paper_history(
            &candidate,
            &strategy,
            &dataset,
            "2026-01-01",
            "2026-01-01T10:00:00Z",
        );
        assert!(result.signals.iter().any(|signal| {
            signal.outcome == PaperSignalOutcome::PendingNextOpen
                && signal.execution_price.is_none()
        }));
    }
}
