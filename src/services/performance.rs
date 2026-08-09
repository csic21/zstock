use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};

const SAMPLE_CAPACITY: usize = 2_000;
static PROCESS_STARTED: OnceLock<Instant> = OnceLock::new();

pub fn mark_process_started() {
    PROCESS_STARTED.get_or_init(Instant::now);
}

pub fn process_elapsed_ms() -> f64 {
    PROCESS_STARTED
        .get_or_init(Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1_000.0
}

pub trait PerformanceMonitor: Send + Sync {
    fn current_rss_bytes(&self) -> anyhow::Result<u64>;
    fn persist(&self, report: &PerformanceReport) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedRssSample {
    pub elapsed_secs: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PerformanceReport {
    pub generated_at: String,
    pub cold_start_interactive_ms: Option<f64>,
    pub cached_navigation_p95_ms: Option<f64>,
    pub ui_build_p95_ms: Option<f64>,
    pub ui_build_p99_ms: Option<f64>,
    pub latest_rss_bytes: Option<u64>,
    pub one_hour_rss_growth_pct: Option<f64>,
    pub navigation_sample_count: usize,
    pub ui_build_sample_count: usize,
    pub rss_sample_count: usize,
}

impl PerformanceReport {
    pub fn cold_start_within_budget(&self) -> Option<bool> {
        self.cold_start_interactive_ms.map(|value| value <= 1_500.0)
    }

    pub fn navigation_within_budget(&self) -> Option<bool> {
        self.cached_navigation_p95_ms.map(|value| value <= 100.0)
    }

    pub fn ui_build_within_budget(&self) -> Option<bool> {
        self.ui_build_p95_ms
            .zip(self.ui_build_p99_ms)
            .map(|(p95, p99)| p95 <= 16.7 && p99 <= 33.0)
    }

    pub fn rss_within_budget(&self) -> Option<bool> {
        self.latest_rss_bytes
            .zip(self.one_hour_rss_growth_pct)
            .map(|(rss, growth)| rss <= 250 * 1024 * 1024 && growth <= 10.0)
    }
}

#[derive(Debug, Default)]
pub struct PerformanceTracker {
    cold_start_interactive_ms: Option<f64>,
    navigation_ms: VecDeque<f64>,
    ui_build_ms: VecDeque<f64>,
    rss: VecDeque<TimedRssSample>,
}

impl PerformanceTracker {
    pub fn record_first_interactive(&mut self, elapsed_ms: f64) {
        if self.cold_start_interactive_ms.is_none() && elapsed_ms.is_finite() && elapsed_ms >= 0.0 {
            self.cold_start_interactive_ms = Some(elapsed_ms);
        }
    }

    pub fn record_navigation(&mut self, elapsed_ms: f64) {
        push_bounded(&mut self.navigation_ms, elapsed_ms);
    }

    pub fn record_ui_build(&mut self, elapsed_ms: f64) {
        push_bounded(&mut self.ui_build_ms, elapsed_ms);
    }

    pub fn record_rss(&mut self, elapsed_secs: u64, bytes: u64) {
        if self.rss.len() == SAMPLE_CAPACITY {
            self.rss.pop_front();
        }
        self.rss.push_back(TimedRssSample {
            elapsed_secs,
            bytes,
        });
    }

    pub fn report(&self) -> PerformanceReport {
        let latest_rss_bytes = self.rss.back().map(|sample| sample.bytes);
        let one_hour_rss_growth_pct = self.rss.back().and_then(|latest| {
            let baseline =
                self.rss.iter().rev().find(|sample| {
                    latest.elapsed_secs.saturating_sub(sample.elapsed_secs) >= 3_600
                })?;
            (baseline.bytes > 0).then_some(
                (latest.bytes as f64 - baseline.bytes as f64) / baseline.bytes as f64 * 100.0,
            )
        });
        PerformanceReport {
            generated_at: chrono::Local::now().to_rfc3339(),
            cold_start_interactive_ms: self.cold_start_interactive_ms,
            cached_navigation_p95_ms: percentile(&self.navigation_ms, 0.95),
            ui_build_p95_ms: percentile(&self.ui_build_ms, 0.95),
            ui_build_p99_ms: percentile(&self.ui_build_ms, 0.99),
            latest_rss_bytes,
            one_hour_rss_growth_pct,
            navigation_sample_count: self.navigation_ms.len(),
            ui_build_sample_count: self.ui_build_ms.len(),
            rss_sample_count: self.rss.len(),
        }
    }
}

fn push_bounded(values: &mut VecDeque<f64>, value: f64) {
    if !value.is_finite() || value < 0.0 {
        return;
    }
    if values.len() == SAMPLE_CAPACITY {
        values.pop_front();
    }
    values.push_back(value);
}

fn percentile(values: &VecDeque<f64>, quantile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted: Vec<_> = values.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let index = ((sorted.len() - 1) as f64 * quantile).ceil() as usize;
    sorted.get(index).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_requires_a_full_hour_before_claiming_rss_growth() {
        let mut tracker = PerformanceTracker::default();
        tracker.record_rss(0, 100);
        tracker.record_rss(3_599, 105);
        assert_eq!(tracker.report().one_hour_rss_growth_pct, None);
        tracker.record_rss(3_600, 109);
        assert_eq!(tracker.report().one_hour_rss_growth_pct, Some(9.0));
        assert_eq!(tracker.report().rss_within_budget(), Some(true));
    }

    #[test]
    fn percentile_budgets_are_computed_from_samples() {
        let mut tracker = PerformanceTracker::default();
        tracker.record_first_interactive(900.0);
        for value in 1..=100 {
            tracker.record_navigation(value as f64 / 2.0);
            tracker.record_ui_build(value as f64 / 10.0);
        }
        let report = tracker.report();
        assert_eq!(report.cold_start_within_budget(), Some(true));
        assert_eq!(report.navigation_within_budget(), Some(true));
        assert_eq!(report.ui_build_within_budget(), Some(true));
    }
}
