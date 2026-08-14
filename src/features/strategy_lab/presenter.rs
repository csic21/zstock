use crate::domain::backtest::report::PortfolioBacktestReport;
use crate::domain::backtest::validation::{PromotionConclusion, RobustnessReport};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyLabLayout {
    Compact,
    Regular,
}

impl StrategyLabLayout {
    pub const fn for_width(width: f32) -> Self {
        if width < 820.0 {
            Self::Compact
        } else {
            Self::Regular
        }
    }

    pub const fn actions_stacked(self) -> bool {
        matches!(self, Self::Compact)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LeaderboardRow {
    pub strategy_id: String,
    pub return_pct: f64,
    pub excess_pct: f64,
    pub drawdown_pct: f64,
    pub win_rate_pct: f64,
    pub trades: usize,
    pub evidence: String,
    pub conclusion: String,
    pub reasons: Vec<String>,
}

pub fn leaderboard(
    reports: &[PortfolioBacktestReport],
    robustness: &[RobustnessReport],
) -> Vec<LeaderboardRow> {
    let mut rows: Vec<_> = reports
        .iter()
        .map(|report| {
            let robust = robustness
                .iter()
                .find(|item| item.strategy_id == report.strategy_id);
            let (evidence, conclusion, reasons) = match robust {
                Some(item) => (
                    format!("{:?}", item.promotion.evidence_grade),
                    conclusion_label(item.promotion.conclusion).into(),
                    item.promotion
                        .gates
                        .iter()
                        .filter(|gate| !gate.passed)
                        .map(|gate| gate.explanation.clone())
                        .collect(),
                ),
                None => (
                    "样本内探索".into(),
                    "继续研究".into(),
                    vec!["稳健性报告尚未生成或数据区间不足".into()],
                ),
            };
            LeaderboardRow {
                strategy_id: report.strategy_id.clone(),
                return_pct: report.metrics.total_return_pct,
                excess_pct: report.metrics.excess_return_pct,
                drawdown_pct: report.metrics.max_drawdown_pct.abs(),
                win_rate_pct: report.metrics.win_rate_pct,
                trades: report.metrics.trade_count,
                evidence,
                conclusion,
                reasons,
            }
        })
        .collect();
    rows.sort_by(|left, right| {
        right
            .excess_pct
            .total_cmp(&left.excess_pct)
            .then_with(|| left.drawdown_pct.total_cmp(&right.drawdown_pct))
            .then_with(|| left.strategy_id.cmp(&right.strategy_id))
    });
    rows
}

const fn conclusion_label(conclusion: PromotionConclusion) -> &'static str {
    match conclusion {
        PromotionConclusion::Rejected => "淘汰",
        PromotionConclusion::ContinueResearch => "继续研究",
        PromotionConclusion::PaperCandidate => "模拟盘候选",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_layout_keeps_actions_accessible_by_stacking_them() {
        let layout = StrategyLabLayout::for_width(640.0);
        assert_eq!(layout, StrategyLabLayout::Compact);
        assert!(layout.actions_stacked());
        assert_eq!(
            StrategyLabLayout::for_width(1_200.0),
            StrategyLabLayout::Regular
        );
    }
}
