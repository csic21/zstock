use anyhow::{Context, Result};

use crate::domain::backtest::report::PortfolioBacktestReport;
use crate::domain::paper::{
    PaperBehaviorComparison, PaperCandidate, PaperRunResult, compare_with_backtest,
    run_paper_history,
};
use crate::domain::strategy::CompiledStrategy;
use crate::services::dataset_repository::DatasetRepository;
use crate::services::experiment_repository::ExperimentRepository;

pub trait PaperTradingRepository: Send + Sync {
    fn save_candidate(&self, candidate: &PaperCandidate) -> Result<()>;
    fn list_candidates(&self) -> Result<Vec<PaperCandidate>>;
    fn save_paper_run(&self, result: &PaperRunResult) -> Result<()>;
    fn load_latest_run(&self, candidate_id: &str) -> Result<Option<PaperRunResult>>;
}

pub struct PaperTradingService<'a> {
    datasets: &'a dyn DatasetRepository,
    experiments: &'a dyn ExperimentRepository,
    paper: &'a dyn PaperTradingRepository,
}

impl<'a> PaperTradingService<'a> {
    pub const fn new(
        datasets: &'a dyn DatasetRepository,
        experiments: &'a dyn ExperimentRepository,
        paper: &'a dyn PaperTradingRepository,
    ) -> Self {
        Self {
            datasets,
            experiments,
            paper,
        }
    }

    pub fn run_candidate(&self, candidate: &PaperCandidate, as_of: &str) -> Result<PaperRunResult> {
        let dataset = self
            .datasets
            .load_observation_dataset(&candidate.dataset_id, as_of)?
            .context("paper candidate dataset is unavailable")?;
        let spec = self
            .experiments
            .load_strategy(&candidate.strategy_id)?
            .context("paper candidate strategy is unavailable")?;
        let strategy = CompiledStrategy::compile(spec)
            .map_err(|errors| anyhow::anyhow!("paper strategy validation failed: {errors:?}"))?;
        let result = run_paper_history(
            candidate,
            &strategy,
            &dataset,
            as_of,
            &chrono::Utc::now().to_rfc3339(),
        );
        self.paper.save_paper_run(&result)?;
        Ok(result)
    }

    pub fn compare(
        &self,
        candidate_id: &str,
        backtest: &PortfolioBacktestReport,
    ) -> Result<Option<PaperBehaviorComparison>> {
        Ok(self
            .paper
            .load_latest_run(candidate_id)?
            .as_ref()
            .map(|paper| compare_with_backtest(paper, backtest)))
    }
}
