//! App-level enums and small state types for the main window.

use gpui::SharedString;

use crate::data::treasure::TREASURE_KLINE_LIMIT;
use crate::model::{shared, MinutePeriod};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum ChartRange {
    M1 = 0,
    M3 = 1,
    M6 = 2,
    Y1 = 3,
    /// ~3 年，用于对照多年高低位。
    Y3 = 4,
    /// 数据源上限附近（约 4 年）。
    Max = 5,
}

impl ChartRange {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::M1 => "1M",
            Self::M3 => "3M",
            Self::M6 => "6M",
            Self::Y1 => "1Y",
            Self::Y3 => "3Y",
            Self::Max => "MAX",
        }
    }

    pub(crate) fn bars(self) -> usize {
        match self {
            Self::M1 => 22,
            Self::M3 => 66,
            Self::M6 => 130,
            Self::Y1 => 252,
            Self::Y3 => 750,
            Self::Max => TREASURE_KLINE_LIMIT,
        }
    }

    pub(crate) fn all() -> [Self; 6] {
        [Self::M1, Self::M3, Self::M6, Self::Y1, Self::Y3, Self::Max]
    }

    pub(crate) fn from_label(s: &str) -> Self {
        match s {
            "1M" => Self::M1,
            "6M" => Self::M6,
            "1Y" => Self::Y1,
            "3Y" => Self::Y3,
            "MAX" | "All" | "ALL" => Self::Max,
            _ => Self::M3,
        }
    }
}

/// 图表类型：分时 / 日 K / 分钟 K。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChartKind {
    /// 分时（当日分钟线，腾讯 minute/query）。
    Intraday,
    /// 日 K（配合 `ChartRange` 选择窗口）。
    DayK,
    /// 分钟 K（1/5/15/30/60 分）。
    MinuteK(MinutePeriod),
}

impl ChartKind {
    pub(crate) fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "intraday" | "分时" => Self::Intraday,
            "m1" => Self::MinuteK(MinutePeriod::M1),
            "m5" => Self::MinuteK(MinutePeriod::M5),
            "m15" => Self::MinuteK(MinutePeriod::M15),
            "m30" => Self::MinuteK(MinutePeriod::M30),
            "m60" => Self::MinuteK(MinutePeriod::M60),
            _ => Self::DayK,
        }
    }

    pub(crate) fn to_label(self) -> &'static str {
        match self {
            Self::Intraday => "intraday",
            Self::DayK => "day",
            Self::MinuteK(p) => p.param(),
        }
    }

}

/// 左侧栏：自选 / 持仓 / 现在找（长线寻宝 + 短线雷达）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LeftTab {
    #[default]
    Watchlist,
    Portfolio,
    Treasure,
}

impl LeftTab {
    pub(crate) fn to_label(self) -> &'static str {
        match self {
            Self::Watchlist => "watchlist",
            Self::Portfolio => "portfolio",
            Self::Treasure => "treasure",
        }
    }

    pub(crate) fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "portfolio" | "持仓" | "book" => Self::Portfolio,
            "treasure" | "寻宝" | "scan" | "find" | "现在找" => Self::Treasure,
            _ => Self::Watchlist,
        }
    }
}

/// Market region shown by the market-analysis page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MarketRegion {
    #[default]
    AShare,
    Hk,
    Us,
}

impl MarketRegion {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::AShare => "A股",
            Self::Hk => "港股",
            Self::Us => "美股",
        }
    }
}

/// Full-page settings navigation (replaces the old modal dialog).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub(crate) enum SettingsSection {
    #[default]
    General = 0,
    StatusBar = 1,
    Ai = 2,
    Update = 3,
    About = 4,
}

impl SettingsSection {
    pub(crate) fn all() -> [Self; 5] {
        [
            Self::General,
            Self::StatusBar,
            Self::Ai,
            Self::Update,
            Self::About,
        ]
    }

    pub(crate) fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::General, true) => "General",
            (Self::General, false) => "常规",
            (Self::StatusBar, true) => "Menu bar",
            (Self::StatusBar, false) => "菜单栏",
            (Self::Ai, true) => "AI",
            (Self::Ai, false) => "AI 分析",
            (Self::Update, true) => "Update",
            (Self::Update, false) => "更新",
            (Self::About, true) => "About",
            (Self::About, false) => "关于",
        }
    }
}

/// 底部分析台分区：一次只聚焦一个任务，避免横向信息堆叠。
///
/// Dock 主 Tab 只放「当前标的分析」：概览 / 策略 / AI / 指标。
/// 持仓流水与寻宝榜以左侧为主；`Portfolio` / `Treasure` 仍可从左侧
/// 「建议 / 详情」临时打开底栏（不出现在主 Tab 条）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub(crate) enum DetailTab {
    /// 一屏概览：评分徽章 + 关键因子 + 状态
    #[default]
    Overview = 0,
    /// 策略雷达完整因子
    Strategy = 1,
    /// AI 点评
    Ai = 2,
    /// 持仓与买卖建议（侧栏入口，非主 Tab）
    Portfolio = 3,
    /// 寻宝价位（侧栏入口，非主 Tab）
    Treasure = 4,
    /// MA / MACD / BOLL 指标读数
    Indicators = 5,
}

impl DetailTab {
    /// Primary analysis-dock tabs (no list duplication with the left sidebar).
    pub(crate) fn dock_tabs() -> [Self; 4] {
        [
            Self::Overview,
            Self::Strategy,
            Self::Ai,
            Self::Indicators,
        ]
    }

    /// Whether this tab is a primary dock tab (shown in the strip always).
    pub(crate) fn is_dock_primary(self) -> bool {
        matches!(
            self,
            Self::Overview | Self::Strategy | Self::Ai | Self::Indicators
        )
    }

    pub(crate) fn label(self, work: bool) -> &'static str {
        use super::labels::L;
        match self {
            Self::Overview => L::detail_overview(work),
            Self::Strategy => L::detail_strategy(work),
            Self::Ai => L::detail_ai(work),
            Self::Portfolio => L::detail_portfolio(work),
            Self::Treasure => L::detail_treasure(work),
            Self::Indicators => L::detail_indicators(work),
        }
    }

    pub(crate) fn to_label(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Strategy => "strategy",
            Self::Ai => "ai",
            Self::Portfolio => "portfolio",
            Self::Treasure => "treasure",
            Self::Indicators => "indicators",
        }
    }

    pub(crate) fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "strategy" | "signal" | "策略" => Self::Strategy,
            "ai" => Self::Ai,
            // Legacy config: list tabs used to live in the dock; restore to overview
            // so the strip stays analysis-only. Side panel still owns lists.
            "portfolio" | "book" | "持仓" => Self::Overview,
            "treasure" | "scan" | "寻宝" => Self::Overview,
            "indicators" | "tech" | "指标" => Self::Indicators,
            _ => Self::Overview,
        }
    }
}

/// 底部「AI 点评」列的展示状态。
#[derive(Debug, Clone)]
pub(crate) enum AiPanelState {
    Idle,
    /// LLM 请求进行中；同时保留本地规则点评供展示。
    Loading {
        text: SharedString,
    },
    Ready {
        text: SharedString,
        /// 点评来源（本地规则 / LLM 模型），让用户一眼区分。
        source: AiSource,
        /// 附加说明（例如 LLM 失败时标注“已回退本地规则”）。
        note: Option<SharedString>,
    },
}

/// 点评来源。
#[derive(Debug, Clone)]
pub(crate) enum AiSource {
    Local,
    /// Optional LLM / CLI result. `label` is the full source line (e.g. `LLM · gpt-5-mini` or `CLI · Grok`).
    Llm {
        label: String,
    },
}

impl AiSource {
    pub(crate) fn label(&self, work: bool) -> SharedString {
        match self {
            Self::Local => shared(if work { "Local rules" } else { "本地规则" }),
            Self::Llm { label } => shared(label.clone()),
        }
    }

    pub(crate) fn is_llm(&self) -> bool {
        matches!(self, Self::Llm { .. })
    }
}

/// 内存缓存条目：文本 + 来源（LLM 成功后替换本地条目，来源随条目保存）。
#[derive(Debug, Clone)]
pub(crate) struct AiCacheEntry {
    pub(crate) text: String,
    pub(crate) source: AiSource,
}

/// Soft cap for in-memory AI commentary caches (prevents unbounded growth when
/// switching symbols / dates for hours).
pub(crate) const AI_CACHE_MAX: usize = 48;

/// Insert into an AI cache, evicting an arbitrary older entry when full.
pub(crate) fn insert_ai_cache(
    map: &mut std::collections::HashMap<String, AiCacheEntry>,
    key: String,
    entry: AiCacheEntry,
) {
    if map.len() >= AI_CACHE_MAX && !map.contains_key(&key) {
        if let Some(old) = map.keys().next().cloned() {
            map.remove(&old);
        }
    }
    map.insert(key, entry);
}
