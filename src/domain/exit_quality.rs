//! Exit diagnostics from journal MFE / MAE, used to calibrate stops and targets.

use serde::{Deserialize, Serialize};

use super::journal::PlanOutcome;
use super::strategy::spec::ExitRule;

const DEFAULT_TARGET_PCT: f64 = 8.0;
const DEFAULT_STOP_PCT: f64 = -5.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitCase {
    TargetFirst,
    StopFirst,
    TimeDecay,
    Giveback,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExitCaseAnalysis {
    pub kind: ExitCase,
    pub touched_target: bool,
    pub stopped_first: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExitDiagnostics {
    pub sample: usize,
    pub target_touch_rate_pct: Option<f64>,
    pub stop_first_rate_pct: Option<f64>,
    pub giveback_rate_pct: Option<f64>,
    pub median_mfe_pct: Option<f64>,
    pub median_mae_pct: Option<f64>,
    pub suggested_hold_days: u16,
    pub suggested_stop_loss_pct: f64,
    pub suggested_take_profit_pct: f64,
}

impl ExitDiagnostics {
    pub fn from_cases(cases: &[ExitCaseAnalysis], outcomes: &[PlanOutcome]) -> Self {
        let sample = cases.len();
        let target_hits = cases.iter().filter(|item| item.touched_target).count();
        let stop_first = cases.iter().filter(|item| item.stopped_first).count();
        let givebacks = cases
            .iter()
            .filter(|item| item.kind == ExitCase::Giveback)
            .count();
        let mut mfes: Vec<f64> = outcomes
            .iter()
            .map(|item| item.maximum_favorable_excursion_pct)
            .filter(|value| value.is_finite())
            .collect();
        let mut maes: Vec<f64> = outcomes
            .iter()
            .map(|item| item.maximum_adverse_excursion_pct)
            .filter(|value| value.is_finite())
            .collect();
        mfes.sort_by(f64::total_cmp);
        maes.sort_by(f64::total_cmp);
        let median_mfe_pct = median(&mfes);
        let median_mae_pct = median(&maes);
        let suggested = suggest_exit_rule(median_mfe_pct, median_mae_pct, sample);
        Self {
            sample,
            target_touch_rate_pct: ratio_pct(target_hits, sample),
            stop_first_rate_pct: ratio_pct(stop_first, sample),
            giveback_rate_pct: ratio_pct(givebacks, sample),
            median_mfe_pct,
            median_mae_pct,
            suggested_hold_days: suggested.hold_days,
            suggested_stop_loss_pct: suggested.stop_loss_pct,
            suggested_take_profit_pct: suggested.take_profit_pct,
        }
    }

    pub fn summary(&self) -> String {
        if self.sample == 0 {
            return "还没有足够的 MFE/MAE 样本来校准出场".into();
        }
        format!(
            "目标触及 {} · 先碰到失效 {} · 回吐 {}",
            format_opt(self.target_touch_rate_pct),
            format_opt(self.stop_first_rate_pct),
            format_opt(self.giveback_rate_pct)
        )
    }

    pub fn hint(&self) -> Option<String> {
        if self.sample < 5 {
            return None;
        }
        if self.target_touch_rate_pct.is_some_and(|value| value < 30.0)
            && self
                .median_mfe_pct
                .is_some_and(|value| value + 0.5 < self.suggested_take_profit_pct)
        {
            return Some(format!(
                "历史目标触及率偏低，10 日中位 MFE 约 {:.1}%；目标带可能过远，可收到 {:.1}%",
                self.median_mfe_pct.unwrap_or(0.0),
                self.suggested_take_profit_pct
            ));
        }
        if self.stop_first_rate_pct.is_some_and(|value| value >= 45.0) {
            return Some(format!(
                "历史常先碰到失效价；失效带可能过紧，可放到 {:.1}% 并缩短持有到 {} 日",
                self.suggested_stop_loss_pct, self.suggested_hold_days
            ));
        }
        if self.giveback_rate_pct.is_some_and(|value| value >= 35.0) {
            return Some(format!(
                "浮盈回吐偏多；固定持有会把赢面坐回去，优先用 {:.1}% 止盈或更短持有",
                self.suggested_take_profit_pct
            ));
        }
        None
    }

    pub fn as_exit_rule(&self) -> ExitRule {
        mae_aware_exit(
            self.suggested_hold_days,
            self.suggested_stop_loss_pct,
            self.suggested_take_profit_pct,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SuggestedExit {
    pub hold_days: u16,
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
}

pub fn analyze_exit(
    outcome: &PlanOutcome,
    target_pct: Option<f64>,
    stop_pct: Option<f64>,
) -> ExitCaseAnalysis {
    let target = target_pct.filter(|value| value.is_finite() && *value > 0.0);
    let stop = stop_pct.filter(|value| value.is_finite() && *value < 0.0);
    let touched_target = target
        .is_some_and(|value| outcome.maximum_favorable_excursion_pct >= value)
        || (target.is_none() && outcome.maximum_favorable_excursion_pct >= DEFAULT_TARGET_PCT);
    let stopped_first = stop.is_some_and(|value| outcome.maximum_adverse_excursion_pct <= value)
        || (stop.is_none() && outcome.maximum_adverse_excursion_pct <= DEFAULT_STOP_PCT);
    let giveback = outcome.maximum_favorable_excursion_pct >= 4.0
        && outcome.return_pct <= outcome.maximum_favorable_excursion_pct * 0.3;
    let kind = if touched_target && !stopped_first {
        ExitCase::TargetFirst
    } else if stopped_first && !touched_target {
        ExitCase::StopFirst
    } else if giveback {
        ExitCase::Giveback
    } else if stopped_first {
        ExitCase::StopFirst
    } else if touched_target {
        ExitCase::TargetFirst
    } else {
        ExitCase::TimeDecay
    };
    ExitCaseAnalysis {
        kind,
        touched_target,
        stopped_first: matches!(kind, ExitCase::StopFirst),
    }
}

pub fn suggest_exit_rule(
    median_mfe_pct: Option<f64>,
    median_mae_pct: Option<f64>,
    sample: usize,
) -> SuggestedExit {
    if sample < 5 {
        return SuggestedExit {
            hold_days: 8,
            stop_loss_pct: 5.0,
            take_profit_pct: 9.0,
        };
    }
    let take = median_mfe_pct.unwrap_or(9.0).abs().clamp(4.0, 16.0) * 0.75;
    let stop = median_mae_pct.unwrap_or(-5.0).abs().clamp(3.0, 10.0) * 0.85;
    let take = take.max(stop * 1.5);
    SuggestedExit {
        hold_days: if take >= 12.0 { 12 } else { 8 },
        stop_loss_pct: (stop * 10.0).round() / 10.0,
        take_profit_pct: (take * 10.0).round() / 10.0,
    }
}

pub fn mae_aware_exit(hold_days: u16, stop_loss_pct: f64, take_profit_pct: f64) -> ExitRule {
    ExitRule::Any {
        any: vec![
            ExitRule::HoldDays { hold_days },
            ExitRule::StopLossPct { stop_loss_pct },
            ExitRule::TakeProfitPct { take_profit_pct },
        ],
    }
}

fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}

fn ratio_pct(hits: usize, sample: usize) -> Option<f64> {
    (sample > 0).then(|| hits as f64 / sample as f64 * 100.0)
}

fn format_opt(value: Option<f64>) -> String {
    value
        .map(|number| format!("{number:.0}%"))
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(ret: f64, mfe: f64, mae: f64) -> PlanOutcome {
        PlanOutcome {
            plan_id: "p".into(),
            horizon_days: 10,
            return_pct: ret,
            maximum_favorable_excursion_pct: mfe,
            maximum_adverse_excursion_pct: mae,
        }
    }

    #[test]
    fn target_first_and_stop_first_are_distinct() {
        let target = analyze_exit(&outcome(5.0, 8.0, -1.0), Some(6.0), Some(-5.0));
        assert_eq!(target.kind, ExitCase::TargetFirst);
        assert!(target.touched_target);
        let stop = analyze_exit(&outcome(-4.0, 1.0, -6.0), Some(8.0), Some(-5.0));
        assert_eq!(stop.kind, ExitCase::StopFirst);
        assert!(stop.stopped_first);
    }

    #[test]
    fn giveback_is_called_out_when_mfe_is_given_back() {
        let analysis = analyze_exit(&outcome(0.5, 8.0, -1.0), Some(12.0), Some(-8.0));
        assert_eq!(analysis.kind, ExitCase::Giveback);
    }

    #[test]
    fn suggestion_tightens_target_toward_median_mfe() {
        let suggested = suggest_exit_rule(Some(6.0), Some(-4.0), 8);
        assert!(suggested.take_profit_pct < 8.0);
        assert!(suggested.take_profit_pct >= suggested.stop_loss_pct * 1.5);
        let fallback = suggest_exit_rule(None, None, 1);
        assert_eq!(fallback.hold_days, 8);
    }
}
