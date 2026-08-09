use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskName {
    Today,
    Research,
    Opportunities,
    Portfolio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMetric {
    pub task: TaskName,
    pub finished_at: String,
    pub duration_ms: u64,
}

pub trait TaskMetricsSink: Send + Sync {
    fn record(&self, metric: TaskMetric) -> anyhow::Result<()>;
}
