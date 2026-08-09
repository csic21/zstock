//! 决策日记：到价提醒自动记一笔，也可手写观察备注。
//!
//! 只做本地复盘素材，不构成交易指令。

use serde::{Deserialize, Serialize};

use crate::domain::journal::{DecisionPlan, PlanOutcome, PlanStatus, add_outcome_idempotently};
use crate::model::Candle;

pub const JOURNAL_SCHEMA_VERSION: u32 =
    crate::infrastructure::storage::migrations::JOURNAL_SCHEMA_VERSION;

/// 日记来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind {
    /// 买入观察区触发。
    AlertBuy,
    /// 止盈/减仓触发。
    AlertSell,
    /// 止损观察触发。
    AlertStop,
    /// 用户手写。
    Manual,
    /// 从长线/短线清单顺手记下。
    FromPick,
}

impl JournalKind {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::AlertBuy, true) => "Buy alert",
            (Self::AlertBuy, false) => "买入提醒",
            (Self::AlertSell, true) => "TP alert",
            (Self::AlertSell, false) => "止盈提醒",
            (Self::AlertStop, true) => "Stop alert",
            (Self::AlertStop, false) => "止损提醒",
            (Self::Manual, true) => "Note",
            (Self::Manual, false) => "手记",
            (Self::FromPick, true) => "Pick",
            (Self::FromPick, false) => "清单",
        }
    }

    pub fn badge(self) -> &'static str {
        match self {
            Self::AlertBuy => "买",
            Self::AlertSell => "盈",
            Self::AlertStop => "损",
            Self::Manual => "记",
            Self::FromPick => "选",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    pub code: String,
    pub name: String,
    pub kind: JournalKind,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub target: Option<f64>,
    pub note: String,
    /// 本地时间 `YYYY-MM-DD HH:MM:SS`
    pub created_at: String,
    #[serde(default)]
    pub plan: Option<DecisionPlan>,
    #[serde(default)]
    pub outcomes: Vec<PlanOutcome>,
}

impl JournalEntry {
    pub fn headline(&self, work: bool) -> String {
        let px = self
            .price
            .map(|p| format!("{p:.2}"))
            .unwrap_or_else(|| "—".into());
        if work {
            format!("{} {} @{}", self.kind.label(true), self.code, px)
        } else {
            format!(
                "{} · {} {} · 现价 {}",
                self.kind.label(false),
                self.code,
                self.name,
                px
            )
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default = "current_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<JournalEntry>,
}

fn current_schema_version() -> u32 {
    JOURNAL_SCHEMA_VERSION
}

impl Default for Journal {
    fn default() -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

/// 最多保留条数，防止日记无限涨。
pub const JOURNAL_CAP: usize = 200;

impl Journal {
    pub fn push(&mut self, entry: JournalEntry) {
        self.entries.insert(0, entry);
        if self.entries.len() > JOURNAL_CAP {
            self.entries.truncate(JOURNAL_CAP);
        }
    }

    pub fn for_code<'a>(&'a self, code: &str) -> Vec<&'a JournalEntry> {
        self.entries.iter().filter(|e| e.code == code).collect()
    }

    pub fn recent(&self, n: usize) -> &[JournalEntry] {
        let n = n.min(self.entries.len());
        &self.entries[..n]
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() != before
    }

    pub fn mark_due(&mut self, today: &str) -> usize {
        let mut changed = 0;
        for entry in &mut self.entries {
            let Some(plan) = entry.plan.as_mut() else {
                continue;
            };
            if plan.status == PlanStatus::Planned && plan.review_on.as_str() <= today {
                plan.status = PlanStatus::DueForReview;
                changed += 1;
            }
        }
        changed
    }

    pub fn add_outcome(&mut self, entry_id: &str, outcome: PlanOutcome) -> bool {
        self.entries
            .iter_mut()
            .find(|entry| entry.id == entry_id)
            .is_some_and(|entry| add_outcome_idempotently(&mut entry.outcomes, outcome))
    }

    pub fn behavior_sample_size(&self) -> Option<usize> {
        let reviewed = self
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .plan
                    .as_ref()
                    .is_some_and(|plan| plan.status == PlanStatus::Reviewed)
            })
            .count();
        (reviewed >= 20).then_some(reviewed)
    }

    /// Fill deterministic 5/10/20-session outcomes once the corresponding
    /// bars exist. Existing horizons are left untouched, so refreshes are
    /// idempotent and retain the original evidence snapshot.
    pub fn update_outcomes_for_series(&mut self, code: &str, candles: &[Candle]) -> usize {
        let mut changed = 0;
        for entry in self.entries.iter_mut().filter(|entry| entry.code == code) {
            let Some(plan) = entry.plan.as_ref() else {
                continue;
            };
            let Some(entry_index) = candles
                .iter()
                .position(|candle| candle.date.as_ref() >= plan.created_on.as_str())
            else {
                continue;
            };
            let base = candles[entry_index].close;
            if !base.is_finite() || base <= 0.0 {
                continue;
            }
            for horizon in [5_u16, 10, 20] {
                let exit_index = entry_index + usize::from(horizon);
                if exit_index >= candles.len() {
                    continue;
                }
                let window = &candles[entry_index + 1..=exit_index];
                let outcome = PlanOutcome {
                    plan_id: plan.id.clone(),
                    horizon_days: horizon,
                    return_pct: (candles[exit_index].close / base - 1.0) * 100.0,
                    maximum_favorable_excursion_pct: window
                        .iter()
                        .map(|candle| (candle.high / base - 1.0) * 100.0)
                        .fold(f64::NEG_INFINITY, f64::max),
                    maximum_adverse_excursion_pct: window
                        .iter()
                        .map(|candle| (candle.low / base - 1.0) * 100.0)
                        .fold(f64::INFINITY, f64::min),
                };
                if add_outcome_idempotently(&mut entry.outcomes, outcome) {
                    changed += 1;
                }
            }
        }
        changed
    }
}

pub fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("j{ms:x}")
}

pub fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 由提醒腿生成默认备注。
pub fn note_for_alert(
    kind: JournalKind,
    code: &str,
    name: &str,
    target: f64,
    current: f64,
) -> String {
    let leg = kind.label(false);
    format!("{leg}触发 · {code} {name} · 目标 {target:.2} · 现价 {current:.2} · 仅记录观察，未下单")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::journal::{EvidenceSnapshot, PlanStatus};
    use crate::model::shared;

    #[test]
    fn cap_trims_oldest() {
        let mut j = Journal::default();
        for i in 0..JOURNAL_CAP + 5 {
            j.push(JournalEntry {
                id: format!("{i}"),
                code: "600519".into(),
                name: "t".into(),
                kind: JournalKind::Manual,
                price: None,
                target: None,
                note: "n".into(),
                created_at: "t".into(),
                plan: None,
                outcomes: Vec::new(),
            });
        }
        assert_eq!(j.entries.len(), JOURNAL_CAP);
        assert_eq!(j.entries[0].id, format!("{}", JOURNAL_CAP + 4));
    }

    fn planned_entry(id: &str, created_on: &str, review_on: &str) -> JournalEntry {
        JournalEntry {
            id: id.into(),
            code: "600519".into(),
            name: "fixture".into(),
            kind: JournalKind::FromPick,
            price: Some(10.0),
            target: Some(12.0),
            note: "plan".into(),
            created_at: format!("{created_on} 09:00:00"),
            plan: Some(DecisionPlan {
                id: id.into(),
                code: "600519".into(),
                created_on: created_on.into(),
                review_on: review_on.into(),
                trigger: "trigger".into(),
                observation_range: "9.8-10.2".into(),
                invalidation: "9.0".into(),
                target: Some("12.0".into()),
                risk_amount: None,
                status: PlanStatus::Planned,
                evidence: EvidenceSnapshot {
                    strategy_version: "strategy-v1".into(),
                    data_as_of: created_on.into(),
                    source: "fixture".into(),
                    payload_json: "{\"score\":61}".into(),
                },
                executed: None,
                exit_reason: None,
                followed_plan: None,
            }),
            outcomes: Vec::new(),
        }
    }

    #[test]
    fn due_transition_and_outcomes_are_idempotent_and_keep_evidence() {
        let mut journal = Journal::default();
        journal.push(planned_entry("p1", "2026-01-01", "2026-01-10"));
        assert_eq!(journal.mark_due("2026-01-10"), 1);
        assert_eq!(journal.mark_due("2026-01-10"), 0);

        let candles = (0..25)
            .map(|index| {
                let price = 10.0 + index as f64 * 0.1;
                Candle {
                    date: shared(format!("2026-01-{:02}", index + 1)),
                    open: price,
                    high: price + 0.2,
                    low: price - 0.2,
                    close: price,
                    volume: 1_000,
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(journal.update_outcomes_for_series("600519", &candles), 3);
        assert_eq!(journal.update_outcomes_for_series("600519", &candles), 0);
        assert_eq!(journal.entries[0].outcomes.len(), 3);
        assert_eq!(
            journal.entries[0]
                .plan
                .as_ref()
                .unwrap()
                .evidence
                .strategy_version,
            "strategy-v1"
        );
    }

    #[test]
    fn behavior_trends_stay_hidden_before_twenty_reviews() {
        let mut journal = Journal::default();
        for index in 0..20 {
            let mut entry = planned_entry(&format!("p{index}"), "2026-01-01", "2026-01-10");
            entry.plan.as_mut().unwrap().status = PlanStatus::Reviewed;
            journal.push(entry);
            if index < 19 {
                assert_eq!(journal.behavior_sample_size(), None);
            }
        }
        assert_eq!(journal.behavior_sample_size(), Some(20));
    }
}
