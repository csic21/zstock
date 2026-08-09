use crate::controller::chart::ChartController;
use crate::controller::discovery::DiscoveryController;
use crate::controller::market::MarketController;
use crate::controller::state::RequestSlot;
use crate::domain::decision::DecisionCard;
use crate::domain::fundamentals::FundamentalSnapshot;
use crate::domain::market::KlineSeries;
use crate::domain::money::Currency;
use crate::services::fundamentals::FundamentalsProvider;
use crate::services::performance::{PerformanceMonitor, PerformanceTracker};
use crate::services::task_metrics::{TaskMetric, TaskMetricsSink, TaskName};

use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct AppServices {
    pub market: MarketController,
    pub fundamentals: Arc<dyn FundamentalsProvider>,
    pub performance: Arc<dyn PerformanceMonitor>,
    pub task_metrics: Arc<dyn TaskMetricsSink>,
}

impl Default for AppServices {
    fn default() -> Self {
        Self {
            market: MarketController::default(),
            fundamentals: Arc::new(crate::infrastructure::market::eastmoney::EastmoneyProvider),
            performance: Arc::new(
                crate::infrastructure::performance::LocalPerformanceMonitor::default(),
            ),
            task_metrics: Arc::new(
                crate::infrastructure::task_metrics::LocalTaskMetrics::default(),
            ),
        }
    }
}

#[derive(Default)]
pub struct MarketState {
    pub last_applied_at: Option<i64>,
}

#[derive(Default)]
pub struct ChartState {
    pub controller: ChartController,
    pub visible: Option<KlineSeries>,
}

#[derive(Default)]
pub struct DiscoveryState {
    pub controller: DiscoveryController<String>,
}

#[derive(Default)]
pub struct PortfolioState {
    pub selected_currency: Option<Currency>,
}

#[derive(Default)]
pub struct AnalysisState {
    pub decision_card: Option<DecisionCard>,
    pub fundamentals: RequestSlot<FundamentalSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrimaryTask {
    Today,
    #[default]
    Research,
    Opportunities,
    Portfolio,
}

pub struct UiState {
    pub primary_task: PrimaryTask,
    task_started_at: Instant,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            primary_task: PrimaryTask::default(),
            task_started_at: Instant::now(),
        }
    }
}

#[derive(Default)]
pub struct RuntimeState {
    pub last_error: Option<String>,
    pub performance: PerformanceTracker,
}

#[cfg(feature = "work-mode")]
#[derive(Default)]
pub struct WorkModeFeature {
    pub state: crate::features::work_mode::state::WorkModeState,
}

impl super::StockApp {
    pub(crate) fn start_performance_monitor(&mut self, cx: &mut gpui::Context<Self>) {
        let monitor = Arc::clone(&self.services.performance);
        cx.spawn(async move |this, cx| {
            loop {
                let sampler = Arc::clone(&monitor);
                let rss = smol::unblock(move || sampler.current_rss_bytes()).await;
                let report = match this.update(cx, |app, _| match rss {
                    Ok(bytes) => {
                        app.runtime_state.performance.record_rss(
                            crate::services::performance::process_elapsed_ms() as u64 / 1_000,
                            bytes,
                        );
                        Some(app.runtime_state.performance.report())
                    }
                    Err(error) => {
                        app.runtime_state.last_error =
                            Some(format!("采集本地性能指标失败：{error:#}"));
                        None
                    }
                }) {
                    Ok(report) => report,
                    Err(_) => break,
                };
                if let Some(report) = report {
                    let persistence = Arc::clone(&monitor);
                    if let Err(error) = smol::unblock(move || persistence.persist(&report)).await {
                        let _ = this.update(cx, |app, _| {
                            app.runtime_state.last_error =
                                Some(format!("保存本地性能报告失败：{error:#}"));
                        });
                    }
                }
                gpui::Timer::after(Duration::from_secs(60)).await;
            }
        })
        .detach();
    }

    pub(crate) fn set_primary_task(&mut self, task: PrimaryTask, cx: &mut gpui::Context<Self>) {
        if self.ui_state.primary_task == task {
            return;
        }
        self.runtime_state.performance.begin_navigation();
        let previous = match self.ui_state.primary_task {
            PrimaryTask::Today => TaskName::Today,
            PrimaryTask::Research => TaskName::Research,
            PrimaryTask::Opportunities => TaskName::Opportunities,
            PrimaryTask::Portfolio => TaskName::Portfolio,
        };
        let metric = TaskMetric {
            task: previous,
            finished_at: chrono::Local::now().to_rfc3339(),
            duration_ms: u64::try_from(self.ui_state.task_started_at.elapsed().as_millis())
                .unwrap_or(u64::MAX),
        };
        if let Err(error) = self.services.task_metrics.record(metric) {
            self.runtime_state.last_error = Some(format!("记录本地任务耗时失败：{error:#}"));
        }
        self.ui_state.primary_task = task;
        self.ui_state.task_started_at = Instant::now();
        self.settings_open = false;
        match task {
            PrimaryTask::Today => {
                self.market_analysis_open = true;
            }
            PrimaryTask::Research => {
                self.market_analysis_open = false;
                self.left_tab = super::LeftTab::Watchlist;
                self.detail_tab = super::DetailTab::Overview;
            }
            PrimaryTask::Opportunities => {
                self.market_analysis_open = false;
                self.left_tab = super::LeftTab::Treasure;
            }
            PrimaryTask::Portfolio => {
                self.market_analysis_open = false;
                self.left_tab = super::LeftTab::Portfolio;
            }
        }
        self.schedule_persist(cx);
        cx.notify();
    }

    /// Deterministic, opt-in A6 exercise. It only runs when explicitly enabled
    /// and is intended to be paired with an isolated `ZSTOCK_DATA_DIR`.
    pub(crate) fn start_a6_validation(&mut self, cx: &mut gpui::Context<Self>) {
        if std::env::var("ZSTOCK_A6_VALIDATE").as_deref() != Ok("1") {
            return;
        }
        if std::env::var_os("ZSTOCK_DATA_DIR")
            .filter(|path| !path.is_empty())
            .is_none()
        {
            self.runtime_state.last_error =
                Some("A6 验证已拒绝启动：必须设置隔离的 ZSTOCK_DATA_DIR".to_string());
            return;
        }
        self.runtime_state.performance.mark_validation_run();
        cx.spawn(async move |this, cx| {
            gpui::Timer::after(Duration::from_secs(3)).await;
            let tasks = [
                PrimaryTask::Today,
                PrimaryTask::Research,
                PrimaryTask::Opportunities,
                PrimaryTask::Portfolio,
            ];
            for index in 0..24 {
                if this
                    .update(cx, |app, cx| {
                        app.set_primary_task(tasks[index % tasks.len()], cx)
                    })
                    .is_err()
                {
                    return;
                }
                gpui::Timer::after(Duration::from_millis(150)).await;
            }
            if this
                .update(cx, |app, cx| {
                    app.set_primary_task(PrimaryTask::Research, cx)
                })
                .is_err()
            {
                return;
            }
            let mut chart_ready = false;
            for _ in 0..30 {
                match this.update(cx, |app, _| app.candles.len() > super::CHART_MIN_VISIBLE) {
                    Ok(true) => {
                        chart_ready = true;
                        break;
                    }
                    Ok(false) => {
                        gpui::Timer::after(Duration::from_secs(1)).await;
                    }
                    Err(_) => return,
                }
            }
            if !chart_ready {
                let _ = this.update(cx, |app, _| {
                    app.runtime_state.last_error =
                        Some("A6 验证未采集图表帧：等待 30 秒后仍没有足够行情数据".to_string());
                });
                return;
            }
            eprintln!("A6_CHART_FRAMES_BEGIN expected_interactions=120");
            for index in 0..120 {
                if this
                    .update(cx, |app, cx| {
                        app.runtime_state
                            .performance
                            .record_validation_chart_interaction();
                        app.chart_zoom(index % 2 == 0, None);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                gpui::Timer::after(Duration::from_millis(40)).await;
            }
            eprintln!("A6_CHART_FRAMES_END completed_interactions=120");
        })
        .detach();
    }
}
