use std::path::PathBuf;

use anyhow::{Context, Result};
use sysinfo::{ProcessesToUpdate, System, get_current_pid};

use crate::services::performance::{PerformanceMonitor, PerformanceReport};

pub struct LocalPerformanceMonitor {
    path: PathBuf,
}

impl Default for LocalPerformanceMonitor {
    fn default() -> Self {
        Self {
            path: crate::infrastructure::storage::paths::performance_report(),
        }
    }
}

impl LocalPerformanceMonitor {
    #[cfg(test)]
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl PerformanceMonitor for LocalPerformanceMonitor {
    fn current_rss_bytes(&self) -> Result<u64> {
        let pid = get_current_pid().map_err(|error| anyhow::anyhow!(error))?;
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]));
        system
            .process(pid)
            .map(|process| process.memory())
            .context("current process missing from system snapshot")
    }

    fn persist(&self, report: &PerformanceReport) -> Result<()> {
        crate::infrastructure::storage::json_store::save(&self.path, report)
            .context("save local performance report")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_atomic_and_contains_no_financial_payload() {
        let dir = std::env::temp_dir().join(format!(
            "stock-performance-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let path = dir.join("performance.json");
        let monitor = LocalPerformanceMonitor::new(path.clone());
        let report = PerformanceReport {
            generated_at: "2026-08-09T10:00:00+08:00".into(),
            cold_start_interactive_ms: Some(900.0),
            cached_navigation_p95_ms: Some(20.0),
            ui_build_p95_ms: Some(8.0),
            ui_build_p99_ms: Some(12.0),
            latest_rss_bytes: Some(100 * 1024 * 1024),
            one_hour_rss_growth_pct: Some(2.0),
            navigation_sample_count: 20,
            ui_build_sample_count: 100,
            rss_sample_count: 61,
        };
        monitor.persist(&report).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("cold_start_interactive_ms"));
        for forbidden in ["code", "position", "amount", "journal", "note"] {
            assert!(!text.contains(forbidden), "leaked field {forbidden}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn current_process_rss_is_available() {
        let monitor = LocalPerformanceMonitor::default();
        assert!(monitor.current_rss_bytes().unwrap() > 0);
    }
}
