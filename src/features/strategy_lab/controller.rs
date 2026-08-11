use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::domain::backtest::config::PortfolioBacktestConfig;
use crate::domain::backtest::validation::{
    RobustnessConfig, RobustnessReport, SealedTestResult, consume_sealed_test, evaluate_robustness,
};
use crate::domain::dataset::{DateInterval, FrozenDataset, FrozenSeries};
use crate::domain::experiment::{
    CandidateSource, ExperimentCandidate, ExperimentDefinition, ExperimentRecord, ExperimentStatus,
    GenerationAudit, RiskLimits,
};
use crate::domain::paper::{
    PaperBehaviorComparison, PaperCandidate, PaperCandidateStatus, PaperRunResult,
    compare_with_backtest,
};
use crate::domain::strategy::{CompiledStrategy, local_templates};
use crate::infrastructure::datasets::ingest::ingest_instruments;
use crate::infrastructure::datasets::sqlite::SqliteLabStore;
use crate::infrastructure::market::eastmoney::EastmoneyProvider;
use crate::services::backtest_repository::{
    BacktestRepository, StoredBacktestRun, StoredRunStatus,
};
use crate::services::backtest_runner::{
    BacktestProgressSnapshot, BatchBacktestRunner, BatchRunResult, BatchRunStatus,
    CancellationToken,
};
use crate::services::dataset_repository::{DatasetRepository, FreezeDatasetRequest};
use crate::services::experiment_repository::ExperimentRepository;
use crate::services::paper_trading::{PaperTradingRepository, PaperTradingService};
use crate::services::strategy_generator::{
    DraftSource, StrategyBatchDraft, StrategyGenerationInput,
};
use crate::services::validation_repository::ValidationRepository;

use super::state::{StrategyDraftView, StrategyLabPage, StrategyLabState};

pub struct StrategyLabFeature {
    pub state: StrategyLabState,
    store: Arc<SqliteLabStore>,
    runner: Arc<BatchBacktestRunner>,
    cancellation: Option<CancellationToken>,
}

#[derive(Debug, Clone)]
pub struct StrategyLabRunWork {
    pub experiment_id: String,
    pub dataset: FrozenDataset,
    pub strategies: Vec<CompiledStrategy>,
    pub config: PortfolioBacktestConfig,
    pub cancellation: CancellationToken,
}

#[derive(Debug, Clone)]
pub struct StrategyLabWorkerResult {
    pub experiment_id: String,
    pub batch: BatchRunResult,
    pub robustness: Vec<RobustnessReport>,
    pub robustness_errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SealedTestWork {
    pub experiment_id: String,
    pub dataset: FrozenDataset,
    pub strategy: CompiledStrategy,
    pub config: PortfolioBacktestConfig,
    pub robustness: RobustnessReport,
    pub consumed_at: String,
}

impl Default for StrategyLabFeature {
    fn default() -> Self {
        Self::new()
    }
}

impl StrategyLabFeature {
    pub fn new() -> Self {
        let (store, warning) = match SqliteLabStore::open(
            &crate::infrastructure::storage::paths::strategy_lab_database(),
        ) {
            Ok(store) => (store, None),
            Err(error) => (
                SqliteLabStore::open_in_memory()
                    .expect("in-memory Strategy Lab SQLite store must initialize"),
                Some(format!("实验数据库不可用，当前使用临时内存库：{error:#}")),
            ),
        };
        let mut feature = Self {
            state: StrategyLabState::default(),
            store: Arc::new(store),
            runner: Arc::new(BatchBacktestRunner::default()),
            cancellation: None,
        };
        if let Err(error) = feature.restore() {
            feature.state.status = format!("恢复历史实验失败：{error:#}");
        }
        if let Some(warning) = warning {
            feature.state.status = warning;
        }
        feature
    }

    pub fn restore(&mut self) -> Result<()> {
        self.state.datasets = self.store.list_manifests()?;
        self.state.experiments = self.store.list_experiments()?;
        self.restore_paper()?;
        if self.state.selected_experiment_id.is_none() {
            self.state.selected_experiment_id = self
                .state
                .experiments
                .first()
                .map(|item| item.definition.id.clone());
        }
        self.restore_selected()
    }

    fn restore_paper(&mut self) -> Result<()> {
        self.state.paper_candidates = self.store.list_candidates()?;
        self.state.paper_runs = self
            .state
            .paper_candidates
            .iter()
            .filter_map(|candidate| self.store.load_latest_run(&candidate.id).transpose())
            .collect::<Result<Vec<_>>>()?;
        self.rebuild_paper_comparisons();
        Ok(())
    }

    fn rebuild_paper_comparisons(&mut self) {
        self.state.paper_comparisons = self
            .state
            .paper_runs
            .iter()
            .filter_map(|paper| {
                let backtest = self
                    .state
                    .reports
                    .iter()
                    .find(|report| report.strategy_id == paper.strategy_id)?;
                Some((
                    paper.candidate_id.clone(),
                    compare_with_backtest(paper, backtest),
                ))
            })
            .collect();
    }

    pub fn select_experiment(&mut self, experiment_id: String) -> Result<()> {
        self.state.selected_experiment_id = Some(experiment_id);
        self.restore_selected()
    }

    fn restore_selected(&mut self) -> Result<()> {
        self.state.drafts.clear();
        self.state.reports.clear();
        self.state.robustness.clear();
        self.state.sealed_tests.clear();
        self.state.failures.clear();
        self.state.selected_strategy_id = None;
        self.state.selected_trade_index = None;
        let Some(experiment_id) = self.state.selected_experiment_id.clone() else {
            return Ok(());
        };
        let generation_model = self
            .store
            .load_experiment(&experiment_id)?
            .map(|experiment| experiment.definition.generation.model)
            .unwrap_or_else(|| "unknown".into());
        for candidate in self.store.load_candidates(&experiment_id)? {
            let Some(strategy_id) = candidate.strategy_id else {
                continue;
            };
            let Some(spec) = self.store.load_strategy(&strategy_id)? else {
                continue;
            };
            self.state.drafts.push(StrategyDraftView {
                strategy_id,
                spec,
                source: match candidate.source {
                    CandidateSource::LocalTemplate => "本地模板".into(),
                    CandidateSource::AiModel => format!("AI 模型 · {generation_model}"),
                    CandidateSource::AiRepair => format!("AI 修复 · {generation_model}"),
                },
                validation_message: if candidate.validation_errors.is_empty() {
                    "本地校验通过".into()
                } else {
                    format!("{} 项校验失败", candidate.validation_errors.len())
                },
            });
        }
        self.state.reports = self
            .store
            .list_runs(&experiment_id)?
            .into_iter()
            .filter_map(|run| run.report)
            .collect();
        self.state.robustness = self.store.list_robustness_reports(&experiment_id)?;
        self.state.sealed_tests = self.store.list_sealed_tests(&experiment_id)?;
        self.state.selected_strategy_id = self
            .state
            .reports
            .first()
            .map(|report| report.strategy_id.clone())
            .or_else(|| {
                self.state
                    .drafts
                    .first()
                    .map(|draft| draft.strategy_id.clone())
            });
        if !self.state.reports.is_empty() {
            self.state.page = StrategyLabPage::Leaderboard;
        } else if !self.state.drafts.is_empty() {
            self.state.page = StrategyLabPage::Drafts;
        }
        self.rebuild_paper_comparisons();
        Ok(())
    }

    pub fn create_local_experiment(&mut self, series: FrozenSeries) -> Result<String> {
        self.create_local_experiment_from_series(vec![series])
    }

    pub fn create_local_experiment_from_series(
        &mut self,
        series: Vec<FrozenSeries>,
    ) -> Result<String> {
        if series.is_empty() {
            return Err(anyhow!("股票池为空"));
        }
        if series.iter().any(|item| item.candles.len() < 30) {
            return Err(anyhow!("每只标的至少需要 30 根日 K 才能创建实验"));
        }
        let market = series[0].instrument.market;
        let adjustment = series[0].adjustment;
        if series
            .iter()
            .any(|item| item.instrument.market != market || item.adjustment != adjustment)
        {
            return Err(anyhow!("同一冻结股票池必须使用相同市场和复权口径"));
        }
        for item in &series {
            self.store.upsert_series(item)?;
        }
        let interval = DateInterval {
            start: series
                .iter()
                .filter_map(|item| item.candles.first().map(|bar| bar.time.clone()))
                .min()
                .expect("non-empty series checked"),
            end: series
                .iter()
                .filter_map(|item| item.candles.last().map(|bar| bar.time.clone()))
                .max()
                .expect("non-empty series checked"),
        };
        let mut source_versions: Vec<_> = series.iter().map(|item| item.source.clone()).collect();
        source_versions.sort();
        source_versions.dedup();
        let manifest = self.store.freeze_dataset(&FreezeDatasetRequest {
            market,
            adjustment,
            source_versions,
            instruments: series.iter().map(|item| item.instrument.clone()).collect(),
            interval,
            known_biases: if series.len() < 20 {
                vec!["股票池少于 20 只，无法满足默认跨标的证据门槛".into()]
            } else {
                vec!["股票池来自当前自选，可能存在幸存者偏差和历史成分缺失".into()]
            },
        })?;
        let count = self.state.form.strategy_count.clamp(3, 8);
        let specs: Vec<_> = local_templates(&manifest.id)
            .into_iter()
            .take(count)
            .collect();
        let experiment_id = format!(
            "experiment:{}",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.9fZ")
        );
        let mut candidates = Vec::with_capacity(specs.len());
        let mut strategy_ids = Vec::with_capacity(specs.len());
        for (ordinal, spec) in specs.iter().enumerate() {
            let strategy_id = self.store.save_strategy(spec)?;
            candidates.push(ExperimentCandidate {
                experiment_id: experiment_id.clone(),
                ordinal,
                strategy_id: Some(strategy_id.clone()),
                parent_strategy_id: None,
                source: CandidateSource::LocalTemplate,
                normalized_hash: Some(strategy_id.clone()),
                validation_errors: vec![],
            });
            strategy_ids.push(strategy_id);
        }
        let now = chrono::Utc::now().to_rfc3339();
        let experiment = ExperimentRecord {
            definition: ExperimentDefinition {
                id: experiment_id.clone(),
                user_goal: self.state.form.goal.clone(),
                risk_limits: RiskLimits {
                    max_drawdown_pct: self.state.form.max_drawdown_pct,
                    max_turnover_pct: None,
                    max_positions: 10,
                },
                generation: GenerationAudit {
                    model: "local-template".into(),
                    transport: "local".into(),
                    prompt_version: "strategy-generator-v1".into(),
                    raw_candidate_count: specs.len(),
                    validation_failure_count: 0,
                    raw_response_sha256: None,
                },
                strategy_ids,
                dataset_id: manifest.id.clone(),
                universe_snapshot_id: manifest.id.clone(),
                benchmark_version: "equal-weighted-frozen-universe-v1".into(),
                cost_model_version: "cn-sided-costs-v1".into(),
                validation_config_version: "robustness-validation-v1".into(),
                parameter_attempts: specs.len(),
                ranking_rule_version: "hard-gates-multi-objective-v1".into(),
            },
            status: ExperimentStatus::Draft,
            created_at: now,
            started_at: None,
            completed_at: None,
            cancelled_at: None,
            failed_at: None,
            failure_message: None,
            test_consumed_at: None,
        };
        self.store.save_experiment(&experiment, &candidates)?;
        self.state.selected_experiment_id = Some(experiment_id.clone());
        self.state.status = format!(
            "已冻结 {} 只股票并生成 {} 个本地策略草案",
            series.len(),
            specs.len()
        );
        self.restore()?;
        self.state.page = StrategyLabPage::Drafts;
        Ok(experiment_id)
    }

    pub fn prepare_run(&mut self) -> Result<StrategyLabRunWork> {
        if self.state.busy {
            return Err(anyhow!("已有实验正在运行"));
        }
        let experiment_id = self
            .state
            .selected_experiment_id
            .clone()
            .context("请先创建或选择实验")?;
        let mut experiment = self
            .store
            .load_experiment(&experiment_id)?
            .context("实验记录不存在")?;
        let dataset = self
            .store
            .load_dataset(&experiment.definition.dataset_id)?
            .context("冻结数据集不存在")?;
        let mut strategies = Vec::new();
        for strategy_id in &experiment.definition.strategy_ids {
            let spec = self
                .store
                .load_strategy(strategy_id)?
                .with_context(|| format!("策略版本不存在：{strategy_id}"))?;
            let compiled = CompiledStrategy::compile(spec)
                .map_err(|errors| anyhow!("策略本地校验失败：{errors:?}"))?;
            strategies.push(compiled);
        }
        let now = chrono::Utc::now().to_rfc3339();
        experiment.status = ExperimentStatus::Running;
        experiment.started_at = Some(now);
        experiment.completed_at = None;
        experiment.cancelled_at = None;
        experiment.failed_at = None;
        experiment.failure_message = None;
        let candidates = self.store.load_candidates(&experiment_id)?;
        self.store.save_experiment(&experiment, &candidates)?;
        let cancellation = CancellationToken::default();
        self.cancellation = Some(cancellation.clone());
        let config = PortfolioBacktestConfig {
            initial_cash: self.state.form.initial_cash,
            ..PortfolioBacktestConfig::default()
        };
        self.state.busy = true;
        self.state.page = StrategyLabPage::Progress;
        self.state.progress = Some(BacktestProgressSnapshot {
            completed_strategies: 0,
            total_strategies: strategies.len(),
            current_strategy_id: None,
            completed_sessions: 0,
            total_sessions: 0,
            cached_reports: 0,
        });
        self.state.status = "批量回测正在后台运行".into();
        Ok(StrategyLabRunWork {
            experiment_id,
            dataset,
            strategies,
            config,
            cancellation,
        })
    }

    pub fn prepare_ai_generation(&mut self) -> Result<(String, StrategyGenerationInput)> {
        if self.state.busy {
            return Err(anyhow!("已有任务正在运行"));
        }
        let experiment_id = self
            .state
            .selected_experiment_id
            .clone()
            .context("请先冻结当前标的或自选股票池，再生成 AI 策略草案")?;
        let experiment = self
            .store
            .load_experiment(&experiment_id)?
            .context("实验不存在")?;
        let manifest = self
            .state
            .datasets
            .iter()
            .find(|manifest| manifest.id == experiment.definition.dataset_id)
            .cloned()
            .context("实验数据集不存在")?;
        self.state.busy = true;
        self.state.status = "AI 正在提出结构化策略假设；本地模板会在失败时自动兜底…".into();
        Ok((
            experiment_id,
            StrategyGenerationInput {
                research_goal: experiment.definition.user_goal,
                market: manifest.market,
                timeframe: "1d".into(),
                universe_snapshot_id: manifest.id,
                universe_description: format!("冻结股票池，共 {} 只", manifest.instruments.len()),
                interval_description: format!(
                    "{} 至 {}",
                    manifest.interval.start, manifest.interval.end
                ),
                risk_limits: format!(
                    "最大回撤预算 {:.1}%",
                    experiment.definition.risk_limits.max_drawdown_pct
                ),
                cost_assumptions: experiment.definition.cost_model_version,
                requested_count: self.state.form.strategy_count,
            },
        ))
    }

    pub fn apply_ai_generation(
        &mut self,
        experiment_id: &str,
        batch: StrategyBatchDraft,
    ) -> Result<()> {
        let mut experiment = self
            .store
            .load_experiment(experiment_id)?
            .context("实验不存在")?;
        let mut candidates = Vec::new();
        let mut strategy_ids = Vec::new();
        for (ordinal, draft) in batch.strategies.iter().enumerate() {
            let id = self.store.save_strategy(&draft.spec)?;
            candidates.push(ExperimentCandidate {
                experiment_id: experiment_id.into(),
                ordinal,
                strategy_id: Some(id.clone()),
                parent_strategy_id: draft.spec.metadata.parent_strategy_id.clone(),
                source: match draft.source {
                    DraftSource::AiInitial => CandidateSource::AiModel,
                    DraftSource::AiRepair => CandidateSource::AiRepair,
                    DraftSource::LocalFallback => CandidateSource::LocalTemplate,
                },
                normalized_hash: Some(id.clone()),
                validation_errors: vec![],
            });
            strategy_ids.push(id);
        }
        experiment.definition.strategy_ids = strategy_ids;
        experiment.definition.parameter_attempts = batch.raw_candidate_count;
        experiment.definition.generation = GenerationAudit {
            model: batch.model.clone(),
            transport: batch.transport.clone(),
            prompt_version: batch.prompt_version.clone(),
            raw_candidate_count: batch.raw_candidate_count,
            validation_failure_count: batch.validation_failure_count,
            raw_response_sha256: batch.raw_response_sha256.clone(),
        };
        experiment.status = ExperimentStatus::Draft;
        self.store.save_experiment(&experiment, &candidates)?;
        self.state.busy = false;
        self.state.selected_experiment_id = Some(experiment_id.into());
        self.restore()?;
        self.state.page = StrategyLabPage::Drafts;
        let ai_count = candidates
            .iter()
            .filter(|candidate| candidate.source != CandidateSource::LocalTemplate)
            .count();
        self.state.status = format!(
            "已生成 {} 个策略（AI {} 个，本地兜底 {} 个）；全部通过白名单校验并去重",
            candidates.len(),
            ai_count,
            candidates.len().saturating_sub(ai_count)
        );
        Ok(())
    }

    pub fn fail_ai_generation(&mut self, message: String) {
        self.state.busy = false;
        self.state.status = format!("策略生成失败：{message}");
    }

    pub fn prepare_sealed_test(&mut self) -> Result<SealedTestWork> {
        if self.state.busy {
            return Err(anyhow!("已有任务正在运行"));
        }
        let experiment_id = self
            .state
            .selected_experiment_id
            .clone()
            .context("请先选择实验")?;
        let experiment = self
            .store
            .load_experiment(&experiment_id)?
            .context("实验不存在")?;
        if let Some(consumed_at) = experiment.test_consumed_at {
            return Err(anyhow!("封存测试集已于 {consumed_at} 消费，禁止重复查看"));
        }
        let strategy_id = self
            .state
            .selected_strategy_id
            .clone()
            .context("请先选择策略")?;
        let spec = self
            .store
            .load_strategy(&strategy_id)?
            .context("策略版本不存在")?;
        let strategy = CompiledStrategy::compile(spec)
            .map_err(|errors| anyhow!("策略校验失败：{errors:?}"))?;
        let robustness = self
            .state
            .robustness
            .iter()
            .find(|report| report.strategy_id == strategy_id)
            .cloned()
            .context("没有训练/验证稳健性报告，不能打开封存测试")?;
        let dataset = self
            .store
            .load_dataset(&experiment.definition.dataset_id)?
            .context("冻结数据集不存在")?;
        let config = self
            .state
            .reports
            .iter()
            .find(|report| report.strategy_id == strategy_id)
            .map(|report| report.config.clone())
            .unwrap_or_default();
        let consumed_at = chrono::Utc::now().to_rfc3339();
        self.state.busy = true;
        self.state.status = "正在一次性计算封存测试；结果不会再反馈给 AI 修改此策略…".into();
        Ok(SealedTestWork {
            experiment_id,
            dataset,
            strategy,
            config,
            robustness,
            consumed_at,
        })
    }

    pub fn execute_sealed_test(work: &SealedTestWork) -> Result<SealedTestResult> {
        consume_sealed_test(
            &work.dataset,
            &work.strategy,
            &work.config,
            &work.robustness,
            None,
            &work.consumed_at,
        )
    }

    pub fn finish_sealed_test(
        &mut self,
        experiment_id: &str,
        result: SealedTestResult,
    ) -> Result<()> {
        let mut experiment = self
            .store
            .load_experiment(experiment_id)?
            .context("实验不存在")?;
        if experiment.test_consumed_at.is_some() {
            self.state.busy = false;
            return Err(anyhow!("封存测试已被消费，拒绝覆盖旧结果"));
        }
        self.store.save_sealed_test(experiment_id, &result)?;
        experiment.test_consumed_at = Some(result.consumed_at.clone());
        let candidates = self.store.load_candidates(experiment_id)?;
        self.store.save_experiment(&experiment, &candidates)?;
        self.state.busy = false;
        self.state.sealed_tests = self.store.list_sealed_tests(experiment_id)?;
        self.state.status = format!(
            "封存测试已消费：收益 {:+.2}% · 超额 {:+.2}% · 回撤 {:.2}% · {} 笔；不得据此覆盖原策略",
            result.report.metrics.total_return_pct,
            result.report.metrics.excess_return_pct,
            result.report.metrics.max_drawdown_pct.abs(),
            result.report.metrics.trade_count
        );
        self.restore_experiment_list()?;
        Ok(())
    }

    pub fn fail_sealed_test(&mut self, message: String) {
        self.state.busy = false;
        self.state.status = format!("封存测试失败且未标记消费：{message}");
    }

    pub fn runner(&self) -> Arc<BatchBacktestRunner> {
        Arc::clone(&self.runner)
    }

    pub fn execute(
        runner: Arc<BatchBacktestRunner>,
        work: StrategyLabRunWork,
        mut on_progress: impl FnMut(BacktestProgressSnapshot),
    ) -> StrategyLabWorkerResult {
        let batch = runner.run(
            &work.dataset,
            &work.strategies,
            &work.config,
            &work.cancellation,
            |progress| on_progress(progress.clone()),
        );
        let mut robustness = Vec::new();
        let mut robustness_errors = Vec::new();
        if batch.status != BatchRunStatus::Cancelled {
            for strategy in &work.strategies {
                match evaluate_robustness(
                    &work.dataset,
                    strategy,
                    &work.config,
                    &RobustnessConfig::default(),
                    work.strategies.len(),
                ) {
                    Ok(report) => robustness.push(report),
                    Err(error) => {
                        robustness_errors.push(format!("{}: {error:#}", strategy.strategy_id()))
                    }
                }
            }
        }
        StrategyLabWorkerResult {
            experiment_id: work.experiment_id,
            batch,
            robustness,
            robustness_errors,
        }
    }

    pub fn apply_progress(&mut self, progress: BacktestProgressSnapshot) {
        self.state.progress = Some(progress);
    }

    pub fn finish_run(&mut self, result: StrategyLabWorkerResult) -> Result<()> {
        let mut experiment = self
            .store
            .load_experiment(&result.experiment_id)?
            .context("实验记录不存在")?;
        let now = chrono::Utc::now().to_rfc3339();
        let stored_status = match result.batch.status {
            BatchRunStatus::Completed => StoredRunStatus::Completed,
            BatchRunStatus::Cancelled => StoredRunStatus::Cancelled,
            BatchRunStatus::CompletedWithFailures => StoredRunStatus::Failed,
        };
        for report in &result.batch.reports {
            let run = StoredBacktestRun {
                run_id: format!(
                    "{}:{}:{}",
                    result.experiment_id, report.strategy_id, report.config_hash
                ),
                experiment_id: result.experiment_id.clone(),
                strategy_id: report.strategy_id.clone(),
                status: if report.cancelled {
                    StoredRunStatus::Cancelled
                } else {
                    stored_status
                },
                config: report.config.clone(),
                report: Some(report.clone()),
                failure_message: None,
                created_at: experiment.started_at.clone().unwrap_or_else(|| now.clone()),
                updated_at: now.clone(),
            };
            self.store.save_run(&run)?;
        }
        for report in &result.robustness {
            self.store
                .save_robustness_report(&result.experiment_id, report)?;
        }
        experiment.status = match result.batch.status {
            BatchRunStatus::Completed => ExperimentStatus::Completed,
            BatchRunStatus::Cancelled => ExperimentStatus::Cancelled,
            BatchRunStatus::CompletedWithFailures => ExperimentStatus::Failed,
        };
        match experiment.status {
            ExperimentStatus::Completed => experiment.completed_at = Some(now.clone()),
            ExperimentStatus::Cancelled => experiment.cancelled_at = Some(now.clone()),
            ExperimentStatus::Failed => {
                experiment.failed_at = Some(now.clone());
                experiment.failure_message = Some(
                    result
                        .batch
                        .failures
                        .iter()
                        .map(|failure| failure.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; "),
                );
            }
            ExperimentStatus::Draft | ExperimentStatus::Running => {}
        }
        let candidates = self.store.load_candidates(&result.experiment_id)?;
        self.store.save_experiment(&experiment, &candidates)?;
        self.state.busy = false;
        self.cancellation = None;
        self.state.progress = Some(result.batch.final_progress.clone());
        self.state.reports = result.batch.reports;
        self.state.robustness = result.robustness;
        self.state.failures = result.batch.failures;
        self.state.selected_strategy_id = self
            .state
            .reports
            .first()
            .map(|report| report.strategy_id.clone());
        self.state.page = StrategyLabPage::Leaderboard;
        self.state.status = match experiment.status {
            ExperimentStatus::Completed => {
                if result.robustness_errors.is_empty() {
                    "实验完成；确定性报告与稳健性门槛已计算".into()
                } else {
                    format!(
                        "实验完成；{} 个稳健性报告因数据区间不足未生成",
                        result.robustness_errors.len()
                    )
                }
            }
            ExperimentStatus::Cancelled => "实验已取消；部分报告已保留，可重新运行".into(),
            ExperimentStatus::Failed => "实验完成但包含失败策略；其他报告已隔离保存".into(),
            ExperimentStatus::Draft | ExperimentStatus::Running => unreachable!(),
        };
        self.restore_experiment_list()?;
        Ok(())
    }

    fn restore_experiment_list(&mut self) -> Result<()> {
        self.state.datasets = self.store.list_manifests()?;
        self.state.experiments = self.store.list_experiments()?;
        Ok(())
    }

    pub fn promote_selected_to_paper(&mut self) -> Result<String> {
        let strategy_id = self
            .state
            .selected_strategy_id
            .clone()
            .context("请先选择策略报告")?;
        let robustness = self
            .state
            .robustness
            .iter()
            .find(|report| report.strategy_id == strategy_id)
            .context("该策略没有完整稳健性报告，不能进入模拟盘")?;
        if !matches!(
            robustness.promotion.conclusion,
            crate::domain::backtest::validation::PromotionConclusion::PaperCandidate
        ) {
            return Err(anyhow!("该策略未通过全部确定性晋级门槛"));
        }
        let experiment_id = self
            .state
            .selected_experiment_id
            .clone()
            .context("实验未选择")?;
        let report = self
            .state
            .reports
            .iter()
            .find(|report| report.strategy_id == strategy_id)
            .context("回测报告不存在")?;
        let candidate = PaperCandidate {
            id: format!("paper:{strategy_id}"),
            strategy_id,
            dataset_id: report.dataset_id.clone(),
            experiment_id,
            created_at: chrono::Utc::now().to_rfc3339(),
            status: PaperCandidateStatus::Observing,
        };
        self.store.save_candidate(&candidate)?;
        self.restore_paper()?;
        self.state.page = StrategyLabPage::PaperCandidates;
        self.state.status = "策略版本已锁定并加入每日模拟观察".into();
        Ok(candidate.id)
    }

    pub fn prepare_paper_run(
        &mut self,
    ) -> Result<(Arc<SqliteLabStore>, Vec<PaperCandidate>, String)> {
        if self.state.busy {
            return Err(anyhow!("已有任务正在运行"));
        }
        if self.state.paper_candidates.is_empty() {
            return Err(anyhow!("尚无通过硬门槛的模拟候选"));
        }
        self.state.busy = true;
        self.state.status = "正在后台计算幂等的每日模拟信号…".into();
        Ok((
            Arc::clone(&self.store),
            self.state.paper_candidates.clone(),
            chrono::Local::now()
                .date_naive()
                .format("%Y-%m-%d")
                .to_string(),
        ))
    }

    pub fn execute_paper_runs(
        store: Arc<SqliteLabStore>,
        candidates: Vec<PaperCandidate>,
        as_of: String,
    ) -> Vec<(String, Result<PaperRunResult, String>)> {
        let mut instruments = std::collections::BTreeSet::new();
        for candidate in &candidates {
            if let Ok(Some(dataset)) = store.load_dataset(&candidate.dataset_id) {
                instruments.extend(dataset.manifest.instruments);
            }
        }
        let instruments: Vec<_> = instruments.into_iter().collect();
        if !instruments.is_empty() {
            let _ = ingest_instruments(&EastmoneyProvider, store.as_ref(), &instruments, 1_000);
        }
        let service = PaperTradingService::new(store.as_ref(), store.as_ref(), store.as_ref());
        candidates
            .into_iter()
            .map(|candidate| {
                let id = candidate.id.clone();
                let result = service
                    .run_candidate(&candidate, &as_of)
                    .map_err(|error| format!("{error:#}"));
                (id, result)
            })
            .collect()
    }

    pub fn finish_paper_runs(
        &mut self,
        results: Vec<(String, Result<PaperRunResult, String>)>,
    ) -> Result<()> {
        self.state.busy = false;
        let failures: Vec<_> = results
            .iter()
            .filter_map(|(id, result)| result.as_ref().err().map(|error| format!("{id}: {error}")))
            .collect();
        self.restore_paper()?;
        self.state.status = if failures.is_empty() {
            format!("每日模拟观察已更新：{} 个候选", results.len())
        } else {
            format!("模拟观察更新完成，{} 个候选失败", failures.len())
        };
        Ok(())
    }

    pub fn save_export(&self) -> Result<std::path::PathBuf> {
        let experiment_id = self
            .state
            .selected_experiment_id
            .as_deref()
            .context("请先选择实验")?;
        let experiment = self
            .store
            .load_experiment(experiment_id)?
            .context("实验不存在")?;
        let candidates = self.store.load_candidates(experiment_id)?;
        let strategies = experiment
            .definition
            .strategy_ids
            .iter()
            .filter_map(|id| self.store.load_strategy(id).transpose())
            .collect::<Result<Vec<_>>>()?;
        let runs = self.store.list_runs(experiment_id)?;
        let robustness_reports = self.store.list_robustness_reports(experiment_id)?;
        let sealed_tests = self.store.list_sealed_tests(experiment_id)?;
        let paper_candidates: Vec<_> = self
            .state
            .paper_candidates
            .iter()
            .filter(|candidate| candidate.experiment_id == experiment_id)
            .cloned()
            .collect();
        let paper_runs: Vec<_> = self
            .state
            .paper_runs
            .iter()
            .filter(|run| {
                paper_candidates
                    .iter()
                    .any(|candidate| candidate.id == run.candidate_id)
            })
            .cloned()
            .collect();
        let bundle = serde_json::json!({
            "export_version": "zstock-strategy-lab-export-v1",
            "exported_at": chrono::Utc::now().to_rfc3339(),
            "disclaimer": "研究记录，不构成投资建议，也不保证未来盈利。",
            "experiment": experiment,
            "candidates": candidates,
            "strategies": strategies,
            "backtest_runs": runs,
            "robustness_reports": robustness_reports,
            "sealed_tests": sealed_tests,
            "paper_candidates": paper_candidates,
            "paper_runs": paper_runs,
        });
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = crate::infrastructure::storage::paths::app_data_dir()
            .join(format!("strategy-lab-export-{stamp}.json"));
        crate::infrastructure::storage::json_store::save(&path, &bundle)?;
        Ok(path)
    }

    pub fn paper_comparison(&self, candidate_id: &str) -> Option<&PaperBehaviorComparison> {
        self.state
            .paper_comparisons
            .iter()
            .find(|(id, _)| id == candidate_id)
            .map(|(_, comparison)| comparison)
    }

    pub fn cancel(&mut self) {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
            self.state.status = "正在取消；当前交易日结束后保存一致状态…".into();
        }
    }

    pub fn fail_run(&mut self, message: String) -> Result<()> {
        let Some(experiment_id) = self.state.selected_experiment_id.clone() else {
            return Ok(());
        };
        if let Some(mut experiment) = self.store.load_experiment(&experiment_id)? {
            let now = chrono::Utc::now().to_rfc3339();
            experiment.status = ExperimentStatus::Failed;
            experiment.failed_at = Some(now);
            experiment.failure_message = Some(message.clone());
            let candidates = self.store.load_candidates(&experiment_id)?;
            self.store.save_experiment(&experiment, &candidates)?;
        }
        self.state.busy = false;
        self.cancellation = None;
        self.state.status = format!("实验失败：{message}");
        self.restore_experiment_list()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{Adjustment, AssetType, CandleRecord, InstrumentId, Market};

    fn feature() -> StrategyLabFeature {
        StrategyLabFeature {
            state: StrategyLabState::default(),
            store: Arc::new(SqliteLabStore::open_in_memory().unwrap()),
            runner: Arc::new(BatchBacktestRunner::default()),
            cancellation: None,
        }
    }

    fn series() -> FrozenSeries {
        FrozenSeries {
            instrument: InstrumentId {
                market: Market::AShare,
                asset_type: AssetType::Stock,
                code: "600000".into(),
            },
            source: "ui-integration-fixture-v1".into(),
            adjustment: Adjustment::Forward,
            candles: (0..420)
                .map(|index| {
                    let close = 20.0 + index as f64 * 0.01 + (index as f64 / 6.0).sin() * 2.0;
                    CandleRecord {
                        time: (chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()
                            + chrono::Days::new(index))
                        .format("%Y-%m-%d")
                        .to_string(),
                        open: close * 0.998,
                        high: close * 1.02,
                        low: close * 0.98,
                        close,
                        volume: 100_000,
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn ui_workflow_create_cancel_rerun_restore_and_drill_is_consistent() {
        let mut feature = feature();
        let experiment_id = feature.create_local_experiment(series()).unwrap();
        assert_eq!(feature.state.page, StrategyLabPage::Drafts);
        assert_eq!(feature.state.drafts.len(), 5);

        let cancelled = feature.prepare_run().unwrap();
        cancelled.cancellation.cancel();
        let result = StrategyLabFeature::execute(feature.runner(), cancelled, |_| {});
        feature.finish_run(result).unwrap();
        let stored = feature
            .store
            .load_experiment(&experiment_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.status, ExperimentStatus::Cancelled);
        assert!(!feature.state.busy);

        let work = feature.prepare_run().unwrap();
        let result = StrategyLabFeature::execute(feature.runner(), work, |_| {});
        feature.finish_run(result).unwrap();
        assert!(!feature.state.reports.is_empty());
        let selected = feature.state.selected_strategy_id.clone().unwrap();
        assert_eq!(
            feature.state.selected_report().unwrap().strategy_id,
            selected
        );
        assert!(!feature.state.robustness.is_empty());
        let sealed_work = feature.prepare_sealed_test().unwrap();
        let sealed = StrategyLabFeature::execute_sealed_test(&sealed_work).unwrap();
        feature.finish_sealed_test(&experiment_id, sealed).unwrap();
        assert!(feature.prepare_sealed_test().is_err());
        assert_eq!(feature.state.sealed_tests.len(), 1);

        feature.state.reports.clear();
        feature.restore().unwrap();
        assert!(!feature.state.reports.is_empty());
        assert_eq!(feature.state.sealed_tests.len(), 1);
        assert_eq!(feature.state.page, StrategyLabPage::Leaderboard);
        assert!(feature.state.ai_explanation.is_none());
    }

    #[test]
    #[ignore = "release-mode Strategy Lab persistence/UI performance baseline"]
    fn performance_database_open_and_report_switch_baseline() {
        use crate::services::performance::PerformanceMonitor;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "zstock-strategy-lab-perf-{}-{nonce}.sqlite3",
            std::process::id()
        ));
        let mut feature = StrategyLabFeature {
            state: StrategyLabState::default(),
            store: Arc::new(SqliteLabStore::open(&path).unwrap()),
            runner: Arc::new(BatchBacktestRunner::default()),
            cancellation: None,
        };
        let pool: Vec<_> = (0..100)
            .map(|symbol| {
                let mut item = series();
                item.instrument.code = format!("{symbol:06}");
                item.candles = (0..1_000)
                    .map(|index| {
                        let close = 20.0
                            + symbol as f64 * 0.01
                            + index as f64 * 0.002
                            + (index as f64 / 13.0).sin();
                        CandleRecord {
                            time: (chrono::NaiveDate::from_ymd_opt(2020, 1, 1).unwrap()
                                + chrono::Days::new(index))
                            .format("%Y-%m-%d")
                            .to_string(),
                            open: close * 0.999,
                            high: close * 1.01,
                            low: close * 0.99,
                            close,
                            volume: 100_000 + index,
                        }
                    })
                    .collect();
                item
            })
            .collect();
        feature.create_local_experiment_from_series(pool).unwrap();

        let monitoring = Arc::new(AtomicBool::new(true));
        let peak_rss = Arc::new(AtomicU64::new(0));
        let monitoring_thread = Arc::clone(&monitoring);
        let peak_thread = Arc::clone(&peak_rss);
        let sampler = std::thread::spawn(move || {
            let monitor = crate::infrastructure::performance::LocalPerformanceMonitor::default();
            while monitoring_thread.load(Ordering::Acquire) {
                if let Ok(rss) = monitor.current_rss_bytes() {
                    peak_thread.fetch_max(rss, Ordering::AcqRel);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        });
        let work = feature.prepare_run().unwrap();
        let started = std::time::Instant::now();
        let runner = feature.runner();
        let batch = runner.run(
            &work.dataset,
            &work.strategies,
            &work.config,
            &work.cancellation,
            |_| {},
        );
        let backtest_elapsed = started.elapsed();
        feature
            .finish_run(StrategyLabWorkerResult {
                experiment_id: work.experiment_id,
                batch,
                robustness: vec![],
                robustness_errors: vec![],
            })
            .unwrap();
        monitoring.store(false, Ordering::Release);
        sampler.join().unwrap();

        let database_bytes = std::fs::metadata(&path).unwrap().len();
        let open_started = std::time::Instant::now();
        feature.restore().unwrap();
        let first_open_elapsed = open_started.elapsed();
        let reports = feature.state.reports.clone();
        let switch_started = std::time::Instant::now();
        for _ in 0..100 {
            for report in &reports {
                feature.state.selected_strategy_id = Some(report.strategy_id.clone());
                std::hint::black_box(feature.state.selected_report());
            }
        }
        let report_switch_elapsed = switch_started.elapsed();
        eprintln!(
            "strategy-lab release baseline: os={} arch={} backtest={backtest_elapsed:?} peak_rss_bytes={} database_bytes={database_bytes} first_open={first_open_elapsed:?} report_switch_500={report_switch_elapsed:?}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            peak_rss.load(Ordering::Acquire),
        );
        assert_eq!(reports.len(), 5);
        drop(feature);
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("sqlite3-wal")).ok();
        std::fs::remove_file(path.with_extension("sqlite3-shm")).ok();
    }
}
