//! 决策日记：到价提醒自动记一笔，也可手写观察备注。
//!
//! 只做本地复盘素材，不构成交易指令。

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub entries: Vec<JournalEntry>,
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
    format!(
        "{leg}触发 · {code} {name} · 目标 {target:.2} · 现价 {current:.2} · 仅记录观察，未下单"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
            });
        }
        assert_eq!(j.entries.len(), JOURNAL_CAP);
        assert_eq!(j.entries[0].id, format!("{}", JOURNAL_CAP + 4));
    }
}
