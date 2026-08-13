//! Cross-symbol rule ledger built from decision-journal outcomes.
//!
//! This is a calibration table, not a ranking. Win rate is reported alongside
//! expectancy, MFE/MAE and whether the plan was followed.

use serde::{Deserialize, Serialize};

use super::exit_quality::{ExitDiagnostics, analyze_exit};
use super::journal::{DecisionPlan, PlanOutcome};

const PRIMARY_HORIZON_DAYS: u16 = 10;
const MIN_DISPLAY_SAMPLE: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerSlice {
    pub key: String,
    pub label: String,
    pub sample: usize,
    pub win_rate_pct: Option<f64>,
    pub expectancy_pct: Option<f64>,
    pub average_mfe_pct: Option<f64>,
    pub average_mae_pct: Option<f64>,
    pub target_touch_rate_pct: Option<f64>,
    pub stop_first_rate_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleLedgerReport {
    pub horizon_days: u16,
    pub sample: usize,
    pub sufficient: bool,
    pub overall: LedgerSlice,
    pub by_strategy: Vec<LedgerSlice>,
    pub by_score: Vec<LedgerSlice>,
    pub by_followed: Vec<LedgerSlice>,
    pub by_regime: Vec<LedgerSlice>,
    pub exit: ExitDiagnostics,
    pub headline: String,
    pub exit_hint: Option<String>,
}

pub trait LedgerSource {
    fn strategy_version(&self) -> &str;
    fn score(&self) -> Option<f64>;
    fn regime(&self) -> Option<&str>;
    fn followed_plan(&self) -> Option<bool>;
    fn plan(&self) -> Option<&DecisionPlan>;
    fn outcomes(&self) -> &[PlanOutcome];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ScoreBucket {
    Below50,
    Waiting,
    Matches,
    Strong,
}

impl ScoreBucket {
    fn from_score(score: f64) -> Self {
        if score >= 75.0 {
            Self::Strong
        } else if score >= 62.0 {
            Self::Matches
        } else if score >= 50.0 {
            Self::Waiting
        } else {
            Self::Below50
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Below50 => "50 分以下",
            Self::Waiting => "50–62 等待区",
            Self::Matches => "62–75 符合策略",
            Self::Strong => "75 分以上",
        }
    }
}

#[derive(Clone)]
struct SampleRow {
    strategy: String,
    score_bucket: Option<ScoreBucket>,
    followed: Option<bool>,
    regime: String,
    outcome: PlanOutcome,
    target_pct: Option<f64>,
    stop_pct: Option<f64>,
}

pub fn build_rule_ledger<T: LedgerSource>(entries: &[T]) -> RuleLedgerReport {
    let rows = collect_rows(entries);
    let overall = slice_from("all", "全部计划", &rows);
    let by_strategy = group_slices(&rows, |row| {
        (
            row.strategy.clone(),
            short_strategy_label(&row.strategy).into(),
        )
    });
    let by_score = group_slices(&rows, |row| {
        let bucket = row.score_bucket.unwrap_or(ScoreBucket::Waiting);
        (format!("{bucket:?}"), bucket.label().into())
    });
    let by_followed = group_slices(&rows, |row| match row.followed {
        Some(true) => ("followed".into(), "按计划执行".into()),
        Some(false) => ("overridden".into(), "未按计划".into()),
        None => ("unknown".into(), "未标记纪律".into()),
    });
    let by_regime = group_slices(&rows, |row| (row.regime.clone(), row.regime.clone()));
    let diagnostics: Vec<_> = rows
        .iter()
        .map(|row| analyze_exit(&row.outcome, row.target_pct, row.stop_pct))
        .collect();
    let outcomes: Vec<_> = rows.iter().map(|row| row.outcome.clone()).collect();
    let exit = ExitDiagnostics::from_cases(&diagnostics, &outcomes);
    let sufficient = rows.len() >= MIN_DISPLAY_SAMPLE;
    let headline = if rows.is_empty() {
        "还没有带结果的计划。创建计划并等 10 个交易日，或复盘到期计划后，这里会汇总真实胜率和期望。"
            .into()
    } else if !sufficient {
        format!(
            "已有 {} 笔 10 日结果，样本仍少，只作校准参考，不能用来给规则晋级。",
            rows.len()
        )
    } else {
        format!(
            "10 日样本 {} 笔 · 胜率 {} · 期望 {} · {}",
            rows.len(),
            format_opt_pct(overall.win_rate_pct),
            format_opt_signed(overall.expectancy_pct),
            exit.summary()
        )
    };
    RuleLedgerReport {
        horizon_days: PRIMARY_HORIZON_DAYS,
        sample: rows.len(),
        sufficient,
        overall,
        by_strategy,
        by_score,
        by_followed,
        by_regime,
        exit_hint: exit.hint(),
        exit,
        headline,
    }
}

fn collect_rows<T: LedgerSource>(entries: &[T]) -> Vec<SampleRow> {
    let mut rows = Vec::new();
    for entry in entries {
        let Some(plan) = entry.plan() else {
            continue;
        };
        let Some(outcome) = entry
            .outcomes()
            .iter()
            .find(|item| item.horizon_days == PRIMARY_HORIZON_DAYS)
            .cloned()
        else {
            continue;
        };
        if !outcome.return_pct.is_finite() {
            continue;
        }
        let (target_pct, stop_pct) = planned_excursions(plan);
        rows.push(SampleRow {
            strategy: entry.strategy_version().to_string(),
            score_bucket: entry.score().map(ScoreBucket::from_score),
            followed: entry.followed_plan(),
            regime: entry
                .regime()
                .filter(|value| !value.is_empty())
                .unwrap_or("未标记状态")
                .to_string(),
            outcome,
            target_pct,
            stop_pct,
        });
    }
    rows
}

fn planned_excursions(plan: &DecisionPlan) -> (Option<f64>, Option<f64>) {
    let entry = first_number(&plan.observation_range)
        .or_else(|| first_number(&plan.trigger))
        .filter(|value| *value > 0.0);
    let target = plan
        .target
        .as_deref()
        .and_then(first_number)
        .filter(|value| *value > 0.0);
    let stop = first_number(&plan.invalidation).filter(|value| *value > 0.0);
    let target_pct = match (entry, target) {
        (Some(entry), Some(target)) if target > entry => Some((target / entry - 1.0) * 100.0),
        _ => None,
    };
    let stop_pct = match (entry, stop) {
        (Some(entry), Some(stop)) if stop < entry => Some((stop / entry - 1.0) * 100.0),
        _ => None,
    };
    (target_pct, stop_pct)
}

fn group_slices(
    rows: &[SampleRow],
    key: impl Fn(&SampleRow) -> (String, String),
) -> Vec<LedgerSlice> {
    let mut groups: Vec<(String, String, Vec<&SampleRow>)> = Vec::new();
    for row in rows {
        let (id, label) = key(row);
        if let Some(existing) = groups.iter_mut().find(|(item, _, _)| item == &id) {
            existing.2.push(row);
        } else {
            groups.push((id, label, vec![row]));
        }
    }
    groups.sort_by(|left, right| right.2.len().cmp(&left.2.len()).then(left.0.cmp(&right.0)));
    groups
        .into_iter()
        .map(|(id, label, items)| {
            let owned: Vec<SampleRow> = items.into_iter().cloned().collect();
            slice_from(&id, &label, &owned)
        })
        .collect()
}

fn slice_from(key: &str, label: &str, rows: &[SampleRow]) -> LedgerSlice {
    let sample = rows.len();
    let wins = rows
        .iter()
        .filter(|row| row.outcome.return_pct > 0.0)
        .count();
    let target_hits = rows
        .iter()
        .filter(|row| analyze_exit(&row.outcome, row.target_pct, row.stop_pct).touched_target)
        .count();
    let stop_first = rows
        .iter()
        .filter(|row| analyze_exit(&row.outcome, row.target_pct, row.stop_pct).stopped_first)
        .count();
    LedgerSlice {
        key: key.into(),
        label: label.into(),
        sample,
        win_rate_pct: ratio_pct(wins, sample),
        expectancy_pct: mean(rows.iter().map(|row| row.outcome.return_pct)),
        average_mfe_pct: mean(
            rows.iter()
                .map(|row| row.outcome.maximum_favorable_excursion_pct),
        ),
        average_mae_pct: mean(
            rows.iter()
                .map(|row| row.outcome.maximum_adverse_excursion_pct),
        ),
        target_touch_rate_pct: ratio_pct(target_hits, sample),
        stop_first_rate_pct: ratio_pct(stop_first, sample),
    }
}

fn ratio_pct(hits: usize, sample: usize) -> Option<f64> {
    (sample > 0).then(|| hits as f64 / sample as f64 * 100.0)
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut sum = 0.0;
    let mut count = 0_usize;
    for value in values {
        if value.is_finite() {
            sum += value;
            count += 1;
        }
    }
    (count > 0).then(|| sum / count as f64)
}

fn first_number(text: &str) -> Option<f64> {
    let mut buf = String::new();
    for character in text.chars() {
        let start = buf.is_empty();
        if character.is_ascii_digit()
            || character == '.'
            || (character == '-' && start)
            || (character == '+' && start)
        {
            if character != '+' {
                buf.push(character);
            }
        } else if !buf.is_empty() {
            break;
        }
    }
    if buf.is_empty() || buf == "-" || buf == "." {
        None
    } else {
        buf.parse().ok()
    }
}

fn short_strategy_label(version: &str) -> &str {
    version.rsplit(':').next().unwrap_or(version)
}

fn format_opt_pct(value: Option<f64>) -> String {
    value
        .map(|number| format!("{number:.0}%"))
        .unwrap_or_else(|| "—".into())
}

fn format_opt_signed(value: Option<f64>) -> String {
    value
        .map(|number| format!("{number:+.2}%"))
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::journal::{EvidenceSnapshot, PlanStatus};

    struct Fixture {
        strategy: String,
        score: Option<f64>,
        regime: Option<String>,
        followed: Option<bool>,
        plan: DecisionPlan,
        outcomes: Vec<PlanOutcome>,
    }

    impl LedgerSource for Fixture {
        fn strategy_version(&self) -> &str {
            &self.strategy
        }
        fn score(&self) -> Option<f64> {
            self.score
        }
        fn regime(&self) -> Option<&str> {
            self.regime.as_deref()
        }
        fn followed_plan(&self) -> Option<bool> {
            self.followed
        }
        fn plan(&self) -> Option<&DecisionPlan> {
            Some(&self.plan)
        }
        fn outcomes(&self) -> &[PlanOutcome] {
            &self.outcomes
        }
    }

    fn plan(id: &str, observation: &str, invalidation: &str, target: &str) -> DecisionPlan {
        DecisionPlan {
            id: id.into(),
            code: "600519".into(),
            created_on: "2026-01-01".into(),
            review_on: "2026-01-29".into(),
            trigger: "进入观察区".into(),
            observation_range: observation.into(),
            invalidation: invalidation.into(),
            target: Some(target.into()),
            risk_amount: None,
            status: PlanStatus::Reviewed,
            evidence: EvidenceSnapshot {
                strategy_version: "technical-quality-gate-v4".into(),
                data_as_of: "2026-01-01".into(),
                source: "fixture".into(),
                payload_json: "{}".into(),
                score: Some(70.0),
                regime: Some("偏强".into()),
            },
            executed: Some(true),
            exit_reason: None,
            followed_plan: Some(true),
        }
    }

    fn outcome(id: &str, ret: f64, mfe: f64, mae: f64) -> PlanOutcome {
        PlanOutcome {
            plan_id: id.into(),
            horizon_days: 10,
            return_pct: ret,
            maximum_favorable_excursion_pct: mfe,
            maximum_adverse_excursion_pct: mae,
        }
    }

    fn row(
        id: &str,
        score: f64,
        followed: bool,
        regime: &str,
        ret: f64,
        mfe: f64,
        mae: f64,
    ) -> Fixture {
        Fixture {
            strategy: "technical-quality-gate-v4".into(),
            score: Some(score),
            regime: Some(regime.into()),
            followed: Some(followed),
            plan: plan(id, "10.00 – 10.20 元", "有效跌破 9.40 元", "11.20 元"),
            outcomes: vec![outcome(id, ret, mfe, mae)],
        }
    }

    #[test]
    fn empty_ledger_stays_honest() {
        let report = build_rule_ledger::<Fixture>(&[]);
        assert_eq!(report.sample, 0);
        assert!(!report.sufficient);
        assert!(report.headline.contains("还没有"));
    }

    #[test]
    fn slices_keep_win_rate_and_expectancy_together() {
        let report = build_rule_ledger(&[
            row("a", 80.0, true, "偏强", 6.0, 8.0, -1.0),
            row("b", 80.0, true, "偏强", 4.0, 7.0, -2.0),
            row("c", 55.0, false, "偏弱", -3.0, 1.0, -5.0),
            row("d", 55.0, false, "偏弱", -2.0, 0.5, -4.0),
            row("e", 70.0, true, "中性", 1.0, 5.0, -2.0),
        ]);
        assert_eq!(report.sample, 5);
        assert!(report.sufficient);
        assert_eq!(report.overall.win_rate_pct, Some(60.0));
        assert!(
            report
                .overall
                .expectancy_pct
                .is_some_and(|value| value > 0.0)
        );
        assert!(
            report
                .by_followed
                .iter()
                .find(|slice| slice.key == "followed")
                .is_some_and(|slice| slice.win_rate_pct == Some(100.0))
        );
        assert!(
            report
                .by_score
                .iter()
                .find(|slice| slice.label.contains("75"))
                .is_some_and(|slice| slice.sample == 2)
        );
        assert!(report.exit.target_touch_rate_pct.is_some());
    }
}
