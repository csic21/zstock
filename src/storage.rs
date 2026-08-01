//! Local persistence for watchlist, UI preferences, and treasure scan cache.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::data::ai::AiConfig;
use crate::data::treasure::TreasureCache;
use crate::model::default_watchlist_codes;

/// Candlestick / quote color convention.
///
/// - **China (`cn`)**: up = red, down = green (A-share convention)
/// - **US (`us`)**: up = green, down = red
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ColorScheme {
    /// 红涨绿跌
    #[default]
    Cn,
    /// 绿涨红跌
    Us,
}

impl ColorScheme {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cn => "中国·红涨",
            Self::Us => "美国·绿涨",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Cn => "红涨",
            Self::Us => "绿涨",
        }
    }
}

/// Watchlist row order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum WatchlistSort {
    /// Insertion / user order as stored in `watchlist`.
    #[default]
    Manual,
    /// Highest gain first.
    ChangeDesc,
    /// Deepest loss first.
    ChangeAsc,
    /// Code ascending.
    CodeAsc,
}

impl WatchlistSort {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Manual, true) => "Order",
            (Self::Manual, false) => "默认",
            (Self::ChangeDesc, true) => "Δ↓",
            (Self::ChangeDesc, false) => "涨幅↓",
            (Self::ChangeAsc, true) => "Δ↑",
            (Self::ChangeAsc, false) => "跌幅↑",
            (Self::CodeAsc, true) => "ID",
            (Self::CodeAsc, false) => "代码",
        }
    }

    pub fn all() -> [Self; 4] {
        [Self::Manual, Self::ChangeDesc, Self::ChangeAsc, Self::CodeAsc]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Pure A-share codes in watchlist order.
    pub watchlist: Vec<String>,
    pub selected: String,
    /// `1M` | `3M` | `6M` | `1Y`
    pub range: String,
    /// `intraday` | `day` | `m1` | `m5` | `m15` | `m30` | `m60`
    #[serde(default = "default_chart_kind")]
    pub chart_kind: String,
    pub show_ma5: bool,
    pub show_ma10: bool,
    pub show_ma20: bool,
    #[serde(default = "default_true")]
    pub show_ma60: bool,
    /// Draw volume bars under the price chart.
    #[serde(default = "default_true")]
    pub show_volume: bool,
    /// Left panel width hint (px).
    pub left_width: f32,
    /// Bottom panel height hint (px).
    pub bottom_height: f32,
    /// Up/down color convention: `cn` (红涨绿跌) or `us` (绿涨红跌).
    #[serde(default)]
    pub color_scheme: ColorScheme,
    /// Work mode: neutral copy + muted up/down colors (in-app toggle).
    #[serde(default)]
    pub work_mode: bool,
    /// Quote poll interval in seconds (clamped 1..=120). Default 1.
    #[serde(default = "default_quote_interval_secs")]
    pub quote_interval_secs: u64,
    /// How the watchlist is ordered in the sidebar.
    #[serde(default)]
    pub watchlist_sort: WatchlistSort,
    /// Optional LLM settings for the AI commentary feature.
    #[serde(default)]
    pub ai_api: AiConfig,
}

fn default_true() -> bool {
    true
}

fn default_chart_kind() -> String {
    "day".into()
}

fn default_quote_interval_secs() -> u64 {
    1
}

/// Clamp user-facing quote interval.
pub fn clamp_quote_interval_secs(secs: u64) -> u64 {
    secs.clamp(1, 120)
}

impl Default for AppConfig {
    fn default() -> Self {
        let watchlist = default_watchlist_codes();
        let selected = watchlist.first().cloned().unwrap_or_else(|| "600519".into());
        Self {
            watchlist,
            selected,
            range: "3M".into(),
            chart_kind: "day".into(),
            show_ma5: true,
            show_ma10: true,
            show_ma20: true,
            show_ma60: true,
            show_volume: true,
            left_width: 280.0,
            bottom_height: 200.0,
            color_scheme: ColorScheme::Cn,
            work_mode: false,
            quote_interval_secs: default_quote_interval_secs(),
            watchlist_sort: WatchlistSort::Manual,
            ai_api: AiConfig::default(),
        }
    }
}

fn app_data_dir() -> PathBuf {
    let base = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("stock-analysis")
}

pub fn config_path() -> PathBuf {
    app_data_dir().join("config.json")
}

pub fn treasure_cache_path() -> PathBuf {
    app_data_dir().join("treasure_cache.json")
}

pub fn load_config() -> AppConfig {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

pub fn save_config(cfg: &AppConfig) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let s = serde_json::to_string_pretty(cfg)?;
    fs::write(&path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn load_treasure_cache() -> TreasureCache {
    let path = treasure_cache_path();
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => TreasureCache::default(),
    }
}

pub fn save_treasure_cache(cache: &TreasureCache) -> Result<()> {
    let path = treasure_cache_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let s = serde_json::to_string_pretty(cache)?;
    fs::write(&path, s).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
