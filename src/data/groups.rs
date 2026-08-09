//! 自选分组标签：长线 / 短线 / 观察，便于分池盯盘与提醒模板。

use serde::{Deserialize, Serialize};

/// 自选池标签（每只代码最多一个主标签）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WatchTag {
    /// 未分组（默认自选）。
    #[default]
    None,
    /// 长线观察 / 价值低位。
    Long,
    /// 短线博弈 / 强势。
    Short,
    /// 待定观察（尚未决定方向）。
    Watch,
}

impl WatchTag {
    pub fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Long => "long",
            Self::Short => "short",
            Self::Watch => "watch",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "long" | "长线" => Self::Long,
            "short" | "短线" => Self::Short,
            "watch" | "观察" => Self::Watch,
            _ => Self::None,
        }
    }

    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::None, true) => "All",
            (Self::None, false) => "全部",
            (Self::Long, true) => "Long",
            (Self::Long, false) => "长线",
            (Self::Short, true) => "Short",
            (Self::Short, false) => "短线",
            (Self::Watch, true) => "Watch",
            (Self::Watch, false) => "观察",
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Long => "L",
            Self::Short => "S",
            Self::Watch => "W",
        }
    }

    pub fn all_filters() -> [Self; 4] {
        [Self::None, Self::Long, Self::Short, Self::Watch]
    }
}

/// 左侧「现在找」主模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FindMode {
    /// 长线：寻宝低位 + 筛可买。
    #[default]
    Long,
    /// 短线：策略雷达（回踩 / 突破 / 超跌）。
    Short,
}

impl FindMode {
    pub fn id(self) -> &'static str {
        match self {
            Self::Long => "long",
            Self::Short => "short",
        }
    }

    pub fn from_id(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "short" | "短线" => Self::Short,
            _ => Self::Long,
        }
    }

    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Long, true) => "Long",
            (Self::Long, false) => "长线",
            (Self::Short, true) => "Short",
            (Self::Short, false) => "短线",
        }
    }

    pub fn headline(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Long, true) => "Historical lows · value watch",
            (Self::Long, false) => "历史低位 · 估值观察 · 建仓带",
            (Self::Short, true) => "Momentum radar · pullback / breakout",
            (Self::Short, false) => "强势回踩 · 放量突破 · 超跌反弹",
        }
    }
}
