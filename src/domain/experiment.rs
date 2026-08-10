use serde::{Deserialize, Serialize};

use super::strategy::ValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus {
    Draft,
    Running,
    Completed,
    Cancelled,
    Failed,
}

impl ExperimentStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiskLimits {
    pub max_drawdown_pct: f64,
    pub max_turnover_pct: Option<f64>,
    pub max_positions: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationAudit {
    pub model: String,
    pub transport: String,
    pub prompt_version: String,
    pub raw_candidate_count: usize,
    pub validation_failure_count: usize,
    pub raw_response_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperimentDefinition {
    pub id: String,
    pub user_goal: String,
    pub risk_limits: RiskLimits,
    pub generation: GenerationAudit,
    pub strategy_ids: Vec<String>,
    pub dataset_id: String,
    pub universe_snapshot_id: String,
    pub benchmark_version: String,
    pub cost_model_version: String,
    pub validation_config_version: String,
    pub parameter_attempts: usize,
    pub ranking_rule_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperimentRecord {
    pub definition: ExperimentDefinition,
    pub status: ExperimentStatus,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub cancelled_at: Option<String>,
    pub failed_at: Option<String>,
    pub failure_message: Option<String>,
    pub test_consumed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    LocalTemplate,
    AiModel,
    AiRepair,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperimentCandidate {
    pub experiment_id: String,
    pub ordinal: usize,
    pub strategy_id: Option<String>,
    pub parent_strategy_id: Option<String>,
    pub source: CandidateSource,
    pub normalized_hash: Option<String>,
    pub validation_errors: Vec<ValidationError>,
}
