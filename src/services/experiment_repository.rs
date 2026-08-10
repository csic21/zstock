use crate::domain::experiment::{ExperimentCandidate, ExperimentRecord};
use crate::domain::strategy::StrategySpec;

pub trait ExperimentRepository: Send + Sync {
    fn save_strategy(&self, spec: &StrategySpec) -> anyhow::Result<String>;
    fn load_strategy(&self, strategy_id: &str) -> anyhow::Result<Option<StrategySpec>>;
    fn save_experiment(
        &self,
        experiment: &ExperimentRecord,
        candidates: &[ExperimentCandidate],
    ) -> anyhow::Result<()>;
    fn load_experiment(&self, id: &str) -> anyhow::Result<Option<ExperimentRecord>>;
    fn load_candidates(&self, experiment_id: &str) -> anyhow::Result<Vec<ExperimentCandidate>>;
    fn list_experiments(&self) -> anyhow::Result<Vec<ExperimentRecord>>;
}
