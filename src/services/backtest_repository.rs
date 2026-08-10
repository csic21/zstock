use serde::{Deserialize, Serialize};

use crate::domain::backtest::config::PortfolioBacktestConfig;
use crate::domain::backtest::report::PortfolioBacktestReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredRunStatus {
    Pending,
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl StoredRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredBacktestRun {
    pub run_id: String,
    pub experiment_id: String,
    pub strategy_id: String,
    pub status: StoredRunStatus,
    pub config: PortfolioBacktestConfig,
    pub report: Option<PortfolioBacktestReport>,
    pub failure_message: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub trait BacktestRepository: Send + Sync {
    fn save_run(&self, run: &StoredBacktestRun) -> anyhow::Result<()>;
    fn load_run(&self, run_id: &str) -> anyhow::Result<Option<StoredBacktestRun>>;
    fn list_runs(&self, experiment_id: &str) -> anyhow::Result<Vec<StoredBacktestRun>>;
}
