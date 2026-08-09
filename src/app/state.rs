use crate::controller::chart::ChartController;
use crate::controller::discovery::DiscoveryController;
use crate::controller::market::MarketController;
use crate::domain::decision::DecisionCard;
use crate::domain::market::KlineSeries;
use crate::domain::money::Currency;
use crate::services::task_metrics::{TaskMetric, TaskMetricsSink, TaskName};

use std::sync::Arc;
use std::time::Instant;

pub struct AppServices {
    pub market: MarketController,
    pub task_metrics: Arc<dyn TaskMetricsSink>,
}

impl Default for AppServices {
    fn default() -> Self {
        Self {
            market: MarketController::default(),
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
}

#[cfg(feature = "work-mode")]
#[derive(Default)]
pub struct WorkModeFeature {
    pub state: crate::features::work_mode::state::WorkModeState,
}

impl super::StockApp {
    pub(crate) fn set_primary_task(&mut self, task: PrimaryTask, cx: &mut gpui::Context<Self>) {
        if self.ui_state.primary_task == task {
            return;
        }
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
}
