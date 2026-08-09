use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::services::task_metrics::{TaskMetric, TaskMetricsSink};

pub struct LocalTaskMetrics {
    path: PathBuf,
    metrics: Mutex<Option<Vec<TaskMetric>>>,
}

impl Default for LocalTaskMetrics {
    fn default() -> Self {
        Self {
            path: super::storage::paths::app_data_dir().join("task-metrics.json"),
            metrics: Mutex::new(None),
        }
    }
}

impl TaskMetricsSink for LocalTaskMetrics {
    fn record(&self, metric: TaskMetric) -> Result<()> {
        let mut guard = self
            .metrics
            .lock()
            .map_err(|_| anyhow::anyhow!("task metric lock poisoned"))?;
        if guard.is_none() {
            let existing = match std::fs::read(&self.path) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .with_context(|| format!("decode {}", self.path.display()))?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error).context("read local task metrics"),
            };
            *guard = Some(existing);
        }
        let metrics = guard.as_mut().expect("task metrics initialized");
        metrics.push(metric);
        if metrics.len() > 500 {
            metrics.drain(..metrics.len() - 500);
        }
        super::storage::json_store::save(&self.path, metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::task_metrics::TaskName;

    #[test]
    fn local_metric_contains_no_security_or_financial_payload_fields() {
        let metric = TaskMetric {
            task: TaskName::Research,
            finished_at: "2026-08-09T10:00:00+08:00".into(),
            duration_ms: 1200,
        };
        let value = serde_json::to_value(metric).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 3);
        for forbidden in ["code", "amount", "journal", "note", "position"] {
            assert!(!object.contains_key(forbidden));
        }
    }
}
