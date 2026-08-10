use crate::domain::backtest::validation::{RobustnessReport, SealedTestResult};

pub trait ValidationRepository: Send + Sync {
    fn save_robustness_report(
        &self,
        experiment_id: &str,
        report: &RobustnessReport,
    ) -> anyhow::Result<()>;
    fn list_robustness_reports(&self, experiment_id: &str)
    -> anyhow::Result<Vec<RobustnessReport>>;
    fn save_sealed_test(
        &self,
        experiment_id: &str,
        result: &SealedTestResult,
    ) -> anyhow::Result<()>;
    fn list_sealed_tests(&self, experiment_id: &str) -> anyhow::Result<Vec<SealedTestResult>>;
}
