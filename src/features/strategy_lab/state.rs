use crate::domain::backtest::report::{PortfolioBacktestReport, PortfolioTrade};
use crate::domain::backtest::validation::{RobustnessReport, SealedTestResult};
use crate::domain::dataset::DatasetManifest;
use crate::domain::experiment::ExperimentRecord;
use crate::domain::paper::{PaperBehaviorComparison, PaperCandidate, PaperRunResult};
use crate::domain::strategy::StrategySpec;
use crate::services::backtest_runner::{BacktestProgressSnapshot, StrategyRunFailure};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StrategyLabPage {
    #[default]
    Configure,
    Drafts,
    Progress,
    Leaderboard,
    Report,
    PaperCandidates,
}

impl StrategyLabPage {
    pub const ALL: [Self; 6] = [
        Self::Configure,
        Self::Drafts,
        Self::Progress,
        Self::Leaderboard,
        Self::Report,
        Self::PaperCandidates,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Configure => "实验配置",
            Self::Drafts => "策略草案",
            Self::Progress => "运行进度",
            Self::Leaderboard => "排行榜",
            Self::Report => "证据报告",
            Self::PaperCandidates => "模拟候选",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TemplateFamily {
    #[default]
    Generic,
    ScanPlaybooks,
}

impl TemplateFamily {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Generic => "通用模板",
            Self::ScanPlaybooks => "扫描规则",
        }
    }

    pub const fn hint(self) -> &'static str {
        match self {
            Self::Generic => "MA / RSI / 突破等通用研究模板",
            Self::ScanPlaybooks => "雷达回踩·突破·超跌 + 寻宝低位，走同一套冻结回测",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrategyLabForm {
    pub goal: String,
    pub strategy_count: usize,
    pub max_drawdown_pct: f64,
    pub initial_cash: f64,
    pub template_family: TemplateFamily,
}

impl Default for StrategyLabForm {
    fn default() -> Self {
        Self {
            goal: "寻找成本后仍稳健、回撤可控的日线研究候选".into(),
            strategy_count: 5,
            max_drawdown_pct: 20.0,
            initial_cash: 1_000_000.0,
            template_family: TemplateFamily::Generic,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrategyDraftView {
    pub strategy_id: String,
    pub spec: StrategySpec,
    pub source: String,
    pub validation_message: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StrategyLabState {
    pub page: StrategyLabPage,
    pub form: StrategyLabForm,
    pub datasets: Vec<DatasetManifest>,
    pub experiments: Vec<ExperimentRecord>,
    pub selected_experiment_id: Option<String>,
    pub drafts: Vec<StrategyDraftView>,
    pub progress: Option<BacktestProgressSnapshot>,
    pub reports: Vec<PortfolioBacktestReport>,
    pub robustness: Vec<RobustnessReport>,
    pub sealed_tests: Vec<SealedTestResult>,
    pub failures: Vec<StrategyRunFailure>,
    pub selected_strategy_id: Option<String>,
    pub selected_trade_index: Option<usize>,
    pub status: String,
    pub ai_explanation: Option<String>,
    pub paper_candidates: Vec<PaperCandidate>,
    pub paper_runs: Vec<PaperRunResult>,
    pub paper_comparisons: Vec<(String, PaperBehaviorComparison)>,
    pub busy: bool,
}

impl StrategyLabState {
    pub fn selected_report(&self) -> Option<&PortfolioBacktestReport> {
        let selected = self.selected_strategy_id.as_deref()?;
        self.reports
            .iter()
            .find(|report| report.strategy_id == selected)
    }

    pub fn selected_trade(&self) -> Option<&PortfolioTrade> {
        self.selected_report()?
            .trades
            .get(self.selected_trade_index?)
    }
}
