use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::domain::backtest::config::PortfolioBacktestConfig;
use crate::domain::backtest::portfolio::{RunControl, run_portfolio_backtest_with_control};
use crate::domain::backtest::report::PortfolioBacktestReport;
use crate::domain::dataset::FrozenDataset;
use crate::domain::strategy::CompiledStrategy;

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchRunStatus {
    Completed,
    Cancelled,
    CompletedWithFailures,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestProgressSnapshot {
    pub completed_strategies: usize,
    pub total_strategies: usize,
    pub current_strategy_id: Option<String>,
    pub completed_sessions: usize,
    pub total_sessions: usize,
    pub cached_reports: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRunFailure {
    pub strategy_id: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchRunResult {
    pub status: BatchRunStatus,
    pub reports: Vec<PortfolioBacktestReport>,
    pub failures: Vec<StrategyRunFailure>,
    pub final_progress: BacktestProgressSnapshot,
}

#[derive(Default)]
pub struct BatchBacktestRunner {
    cache: Mutex<BTreeMap<String, PortfolioBacktestReport>>,
}

impl BatchBacktestRunner {
    pub fn run(
        &self,
        dataset: &FrozenDataset,
        strategies: &[CompiledStrategy],
        config: &PortfolioBacktestConfig,
        cancellation: &CancellationToken,
        mut on_progress: impl FnMut(&BacktestProgressSnapshot),
    ) -> BatchRunResult {
        let total_sessions = dataset
            .series
            .iter()
            .map(|series| series.candles.len())
            .max()
            .unwrap_or(0);
        let mut progress = BacktestProgressSnapshot {
            completed_strategies: 0,
            total_strategies: strategies.len(),
            current_strategy_id: None,
            completed_sessions: 0,
            total_sessions,
            cached_reports: 0,
        };
        let mut reports = Vec::new();
        let mut failures = Vec::new();
        for strategy in strategies {
            if cancellation.is_cancelled() {
                break;
            }
            progress.current_strategy_id = Some(strategy.strategy_id().into());
            progress.completed_sessions = 0;
            on_progress(&progress);
            let key = cache_key(dataset, strategy, config);
            let cached = self
                .cache
                .lock()
                .expect("backtest cache mutex poisoned")
                .get(&key)
                .cloned();
            if let Some(report) = cached {
                reports.push(report);
                progress.completed_strategies += 1;
                progress.completed_sessions = total_sessions;
                progress.cached_reports += 1;
                on_progress(&progress);
                continue;
            }
            let result = run_portfolio_backtest_with_control(
                dataset,
                strategy,
                config,
                |completed, total| {
                    progress.completed_sessions = completed;
                    progress.total_sessions = total;
                    on_progress(&progress);
                    if cancellation.is_cancelled() {
                        RunControl::Cancel
                    } else {
                        RunControl::Continue
                    }
                },
            );
            match result {
                Ok(report) => {
                    let was_cancelled = report.cancelled;
                    if !was_cancelled {
                        self.cache
                            .lock()
                            .expect("backtest cache mutex poisoned")
                            .insert(key, report.clone());
                    }
                    reports.push(report);
                    if was_cancelled {
                        cancellation.cancel();
                        break;
                    }
                }
                Err(error) => failures.push(StrategyRunFailure {
                    strategy_id: strategy.strategy_id().into(),
                    message: error.to_string(),
                }),
            }
            progress.completed_strategies += 1;
            on_progress(&progress);
        }
        progress.current_strategy_id = None;
        let status = if cancellation.is_cancelled() {
            BatchRunStatus::Cancelled
        } else if failures.is_empty() {
            BatchRunStatus::Completed
        } else {
            BatchRunStatus::CompletedWithFailures
        };
        BatchRunResult {
            status,
            reports,
            failures,
            final_progress: progress,
        }
    }

    pub fn clear_cache(&self) {
        self.cache
            .lock()
            .expect("backtest cache mutex poisoned")
            .clear();
    }
}

fn cache_key(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    config: &PortfolioBacktestConfig,
) -> String {
    format!(
        "{}:{}:{}",
        dataset.manifest.id,
        strategy.strategy_id(),
        config.stable_hash()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dataset::{
        DataQualityIssue, DatasetManifest, DateInterval, FrozenSeries, dataset_content_sha256,
    };
    use crate::domain::market::{Adjustment, AssetType, CandleRecord, InstrumentId, Market};
    use crate::domain::strategy::{
        CompareOperator, Comparison, ExitRule, Expression, LocalTemplate, ValueExpression,
    };

    fn fixture() -> (FrozenDataset, CompiledStrategy) {
        let series = FrozenSeries {
            instrument: InstrumentId {
                market: Market::AShare,
                asset_type: AssetType::Stock,
                code: "600000".into(),
            },
            source: "fixture-v1".into(),
            adjustment: Adjustment::Forward,
            candles: (0..8)
                .map(|index| CandleRecord {
                    time: format!("2026-01-{:02}", index + 1),
                    open: 10.0 + index as f64,
                    high: 11.0 + index as f64,
                    low: 9.0 + index as f64,
                    close: 10.0 + index as f64,
                    volume: 10_000,
                })
                .collect(),
        };
        let hash = dataset_content_sha256(Market::AShare, std::slice::from_ref(&series));
        let id = format!("dataset-sha256:{hash}");
        let dataset = FrozenDataset {
            manifest: DatasetManifest {
                id: id.clone(),
                created_at: "2026-01-10T00:00:00Z".into(),
                market: Market::AShare,
                adjustment: Adjustment::Forward,
                source_versions: vec!["fixture-v1".into()],
                instruments: vec![series.instrument.clone()],
                interval: DateInterval {
                    start: "2026-01-01".into(),
                    end: "2026-01-08".into(),
                },
                content_sha256: hash,
                known_biases: vec![],
                quality_issues: Vec::<DataQualityIssue>::new(),
            },
            series: vec![series],
        };
        let mut spec = LocalTemplate::NDayHighBreakout.build(&id);
        spec.entry = Expression::Compare {
            compare: Comparison {
                left: ValueExpression::Constant { constant: 2.0 },
                op: CompareOperator::Above,
                right: ValueExpression::Constant { constant: 1.0 },
            },
        };
        spec.exit = ExitRule::HoldDays { hold_days: 2 };
        spec.position.size_pct = 100.0;
        spec.position.max_positions = 1;
        (dataset, CompiledStrategy::compile(spec).unwrap())
    }

    #[test]
    fn completed_result_is_cached_by_all_versioned_inputs() {
        let (dataset, strategy) = fixture();
        let runner = BatchBacktestRunner::default();
        let config = PortfolioBacktestConfig::default();
        let first = runner.run(
            &dataset,
            std::slice::from_ref(&strategy),
            &config,
            &CancellationToken::default(),
            |_| {},
        );
        let second = runner.run(
            &dataset,
            &[strategy],
            &config,
            &CancellationToken::default(),
            |_| {},
        );
        assert_eq!(first.status, BatchRunStatus::Completed);
        assert_eq!(second.final_progress.cached_reports, 1);
        assert_eq!(first.reports, second.reports);
    }

    #[test]
    fn cancellation_stops_batch_with_partial_report() {
        let (dataset, strategy) = fixture();
        let runner = BatchBacktestRunner::default();
        let cancellation = CancellationToken::default();
        let result = runner.run(
            &dataset,
            &[strategy],
            &PortfolioBacktestConfig::default(),
            &cancellation,
            |progress| {
                if progress.completed_sessions >= 2 {
                    cancellation.cancel();
                }
            },
        );
        assert_eq!(result.status, BatchRunStatus::Cancelled);
        assert!(result.reports[0].cancelled);
        assert_eq!(result.reports[0].completed_sessions, 2);
    }

    #[test]
    #[ignore = "release-mode performance baseline"]
    fn performance_100_instruments_5_strategies_1000_bars() {
        let series: Vec<_> = (0..100)
            .map(|symbol| FrozenSeries {
                instrument: InstrumentId {
                    market: Market::AShare,
                    asset_type: AssetType::Stock,
                    code: format!("{symbol:06}"),
                },
                source: "performance-fixture-v1".into(),
                adjustment: Adjustment::Forward,
                candles: (0..1_000)
                    .map(|index| {
                        let close = 20.0
                            + symbol as f64 * 0.01
                            + index as f64 * 0.002
                            + (index as f64 / 13.0).sin();
                        CandleRecord {
                            time: format!("d{index:04}"),
                            open: close * 0.999,
                            high: close * 1.01,
                            low: close * 0.99,
                            close,
                            volume: 100_000 + index as u64,
                        }
                    })
                    .collect(),
            })
            .collect();
        let hash = dataset_content_sha256(Market::AShare, &series);
        let id = format!("dataset-sha256:{hash}");
        let dataset = FrozenDataset {
            manifest: DatasetManifest {
                id: id.clone(),
                created_at: "2026-01-10T00:00:00Z".into(),
                market: Market::AShare,
                adjustment: Adjustment::Forward,
                source_versions: vec!["performance-fixture-v1".into()],
                instruments: series.iter().map(|item| item.instrument.clone()).collect(),
                interval: DateInterval {
                    start: "d0000".into(),
                    end: "d0999".into(),
                },
                content_sha256: hash,
                known_biases: vec![],
                quality_issues: vec![],
            },
            series,
        };
        let strategies: Vec<_> = crate::domain::strategy::local_templates(&id)
            .into_iter()
            .map(|spec| CompiledStrategy::compile(spec).unwrap())
            .collect();
        let config = PortfolioBacktestConfig {
            initial_cash: 10_000_000.0,
            ..PortfolioBacktestConfig::default()
        };
        let started = std::time::Instant::now();
        let result = BatchBacktestRunner::default().run(
            &dataset,
            &strategies,
            &config,
            &CancellationToken::default(),
            |_| {},
        );
        eprintln!(
            "strategy-lab performance: elapsed={:?}, reports={}, fixture_bytes={}",
            started.elapsed(),
            result.reports.len(),
            serde_json::to_vec(&dataset).unwrap().len()
        );
        assert_eq!(result.status, BatchRunStatus::Completed);
        assert_eq!(result.reports.len(), 5);
    }
}
