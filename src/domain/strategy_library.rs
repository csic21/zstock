use serde::{Deserialize, Serialize};

use super::backtest::report::PortfolioBacktestReport;
use super::backtest::validation::{PromotionConclusion, RobustnessReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LibraryStatus {
    #[default]
    Retained,
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LibrarySort {
    #[default]
    WinRate,
    ExcessReturn,
    Drawdown,
    Trades,
    Recent,
}

impl LibrarySort {
    pub const ALL: [Self; 5] = [
        Self::WinRate,
        Self::ExcessReturn,
        Self::Drawdown,
        Self::Trades,
        Self::Recent,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::WinRate => "胜率",
            Self::ExcessReturn => "超额收益",
            Self::Drawdown => "回撤",
            Self::Trades => "交易数",
            Self::Recent => "最近入库",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LibraryFilter {
    #[default]
    All,
    PaperCandidate,
    ContinueResearch,
    Rejected,
}

impl LibraryFilter {
    pub const ALL: [Self; 4] = [
        Self::All,
        Self::PaperCandidate,
        Self::ContinueResearch,
        Self::Rejected,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::PaperCandidate => "模拟盘候选",
            Self::ContinueResearch => "继续研究",
            Self::Rejected => "已淘汰",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StrategyLibraryRecord {
    pub id: String,
    pub experiment_id: String,
    pub strategy_id: String,
    pub dataset_id: String,
    pub strategy_name: String,
    pub retained_at: String,
    pub status: LibraryStatus,
    pub conclusion: Option<PromotionConclusion>,
    pub evidence: String,
    pub win_rate_pct: f64,
    pub oos_win_rate_pct: Option<f64>,
    pub total_return_pct: f64,
    pub excess_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub trade_count: usize,
    pub payoff_ratio: f64,
    pub profit_factor: f64,
}

impl StrategyLibraryRecord {
    pub fn id_for(experiment_id: &str, strategy_id: &str) -> String {
        format!("library:{experiment_id}:{strategy_id}")
    }

    pub fn from_completed_run(
        experiment_id: &str,
        strategy_name: &str,
        report: &PortfolioBacktestReport,
        robustness: Option<&RobustnessReport>,
        retained_at: impl Into<String>,
    ) -> Self {
        let (conclusion, evidence, oos_win_rate_pct) = match robustness {
            Some(item) => (
                Some(item.promotion.conclusion),
                format!("{:?}", item.promotion.evidence_grade),
                Some(item.validation_report.metrics.win_rate_pct),
            ),
            None => (None, "样本内探索".into(), None),
        };
        Self {
            id: Self::id_for(experiment_id, &report.strategy_id),
            experiment_id: experiment_id.into(),
            strategy_id: report.strategy_id.clone(),
            dataset_id: report.dataset_id.clone(),
            strategy_name: strategy_name.trim().to_string(),
            retained_at: retained_at.into(),
            status: LibraryStatus::Retained,
            conclusion,
            evidence,
            win_rate_pct: report.metrics.win_rate_pct,
            oos_win_rate_pct,
            total_return_pct: report.metrics.total_return_pct,
            excess_return_pct: report.metrics.excess_return_pct,
            max_drawdown_pct: report.metrics.max_drawdown_pct.abs(),
            trade_count: report.metrics.trade_count,
            payoff_ratio: report.metrics.payoff_ratio,
            profit_factor: report.metrics.profit_factor,
        }
    }
}

pub fn rank_library(
    records: &[StrategyLibraryRecord],
    sort: LibrarySort,
    filter: LibraryFilter,
) -> Vec<StrategyLibraryRecord> {
    let mut rows: Vec<_> = records
        .iter()
        .filter(|record| record.status == LibraryStatus::Retained)
        .filter(|record| match filter {
            LibraryFilter::All => true,
            LibraryFilter::PaperCandidate => {
                record.conclusion == Some(PromotionConclusion::PaperCandidate)
            }
            LibraryFilter::ContinueResearch => {
                record.conclusion == Some(PromotionConclusion::ContinueResearch)
            }
            LibraryFilter::Rejected => record.conclusion == Some(PromotionConclusion::Rejected),
        })
        .cloned()
        .collect();
    rows.sort_by(|left, right| match sort {
        LibrarySort::WinRate => right
            .win_rate_pct
            .total_cmp(&left.win_rate_pct)
            .then_with(|| right.trade_count.cmp(&left.trade_count))
            .then_with(|| right.excess_return_pct.total_cmp(&left.excess_return_pct))
            .then_with(|| left.strategy_id.cmp(&right.strategy_id)),
        LibrarySort::ExcessReturn => right
            .excess_return_pct
            .total_cmp(&left.excess_return_pct)
            .then_with(|| left.max_drawdown_pct.total_cmp(&right.max_drawdown_pct))
            .then_with(|| left.strategy_id.cmp(&right.strategy_id)),
        LibrarySort::Drawdown => left
            .max_drawdown_pct
            .total_cmp(&right.max_drawdown_pct)
            .then_with(|| right.excess_return_pct.total_cmp(&left.excess_return_pct))
            .then_with(|| left.strategy_id.cmp(&right.strategy_id)),
        LibrarySort::Trades => right
            .trade_count
            .cmp(&left.trade_count)
            .then_with(|| right.win_rate_pct.total_cmp(&left.win_rate_pct))
            .then_with(|| left.strategy_id.cmp(&right.strategy_id)),
        LibrarySort::Recent => right
            .retained_at
            .cmp(&left.retained_at)
            .then_with(|| left.strategy_id.cmp(&right.strategy_id)),
    });
    rows
}

pub fn highest_win_rate(records: &[StrategyLibraryRecord]) -> Option<&StrategyLibraryRecord> {
    records
        .iter()
        .filter(|record| record.status == LibraryStatus::Retained)
        .max_by(|left, right| {
            left.win_rate_pct
                .total_cmp(&right.win_rate_pct)
                .then_with(|| left.trade_count.cmp(&right.trade_count))
                .then_with(|| left.strategy_id.cmp(&right.strategy_id))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        strategy_id: &str,
        win_rate: f64,
        trades: usize,
        excess: f64,
    ) -> StrategyLibraryRecord {
        StrategyLibraryRecord {
            id: format!("library:exp:{strategy_id}"),
            experiment_id: "exp".into(),
            strategy_id: strategy_id.into(),
            dataset_id: "dataset".into(),
            strategy_name: strategy_id.into(),
            retained_at: format!("2026-08-0{trades}T00:00:00Z"),
            status: LibraryStatus::Retained,
            conclusion: Some(PromotionConclusion::ContinueResearch),
            evidence: "in_sample_exploration".into(),
            win_rate_pct: win_rate,
            oos_win_rate_pct: None,
            total_return_pct: excess,
            excess_return_pct: excess,
            max_drawdown_pct: 10.0,
            trade_count: trades,
            payoff_ratio: 1.2,
            profit_factor: 1.1,
        }
    }

    #[test]
    fn default_sort_puts_the_highest_win_rate_first() {
        let records = vec![
            record("low", 40.0, 12, 3.0),
            record("high", 70.0, 8, 1.0),
            record("mid", 55.0, 20, 5.0),
        ];
        let ranked = rank_library(&records, LibrarySort::WinRate, LibraryFilter::All);
        assert_eq!(
            ranked
                .iter()
                .map(|item| item.strategy_id.as_str())
                .collect::<Vec<_>>(),
            ["high", "mid", "low"]
        );
        assert_eq!(
            highest_win_rate(&records).map(|item| item.strategy_id.as_str()),
            Some("high")
        );
    }

    #[test]
    fn dismissed_and_filtered_records_are_excluded() {
        let mut rejected = record("rejected", 90.0, 30, 8.0);
        rejected.conclusion = Some(PromotionConclusion::Rejected);
        let mut dismissed = record("gone", 99.0, 40, 9.0);
        dismissed.status = LibraryStatus::Dismissed;
        let records = vec![record("keep", 50.0, 10, 2.0), rejected, dismissed];
        let ranked = rank_library(
            &records,
            LibrarySort::WinRate,
            LibraryFilter::ContinueResearch,
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].strategy_id, "keep");
        assert_eq!(
            highest_win_rate(&records).map(|item| item.strategy_id.as_str()),
            Some("rejected")
        );
    }
}
