//! Local persistence for watchlist, UI preferences, and treasure scan cache.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::data::ai::AiConfig;
use crate::data::treasure::TreasureCache;
use crate::model::{TrendLine, default_watchlist_codes};

/// Serializable window bounds (x, y, width, height in logical pixels).
pub type WindowBounds = (f32, f32, f32, f32);

/// Complete dock layout: all panel sizes + window bounds.
///
/// `main_h` holds the widths of the horizontal panel group (left panel,
/// center/right region) and `main_v` the heights of the vertical group
/// (chart, bottom detail). Older configs only persisted `left_width` /
/// `bottom_height`; when the vectors are empty the legacy fields are used.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DockLayout {
    #[serde(default)]
    pub main_h: Vec<f32>,
    #[serde(default)]
    pub main_v: Vec<f32>,
    /// Window frame bounds `(x, y, width, height)`.
    #[serde(default)]
    pub window: Option<WindowBounds>,
}

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
    /// Draw the MACD sub-pane (DIF/DEA + histogram).
    #[serde(default = "default_true")]
    pub show_macd: bool,
    /// Overlay Bollinger bands on the price pane.
    #[serde(default)]
    pub show_boll: bool,
    /// Full dock layout (panel sizes + window bounds). Empty vectors fall back
    /// to the legacy `left_width` / `bottom_height` fields.
    #[serde(default)]
    pub dock: DockLayout,
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
    /// User-drawn chart lines, keyed by symbol code.
    #[serde(default)]
    pub chart_lines: std::collections::HashMap<String, Vec<TrendLine>>,
    /// Treasure scan pool id (`mcap` / `hs300` / `zz500` / `sh50` / `cyb` / `kc50`).
    #[serde(default = "default_treasure_pool")]
    pub treasure_pool: String,
    /// Treasure financial-percentile filter (`off` / `pe` / `pb` / `value`).
    #[serde(default = "default_treasure_fin")]
    pub treasure_fin: String,
    /// Show live quotes in the macOS menu bar (no-op on other platforms).
    #[serde(default)]
    pub status_bar_enabled: bool,
    /// Watchlist codes pinned to the status bar menu (max 5). Title shows `status_bar_active`.
    #[serde(default)]
    pub status_bar_codes: Vec<String>,
    /// Code currently shown in the status bar title.
    #[serde(default)]
    pub status_bar_active: String,
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

fn default_treasure_pool() -> String {
    "mcap".into()
}

fn default_treasure_fin() -> String {
    "off".into()
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
            show_macd: true,
            show_boll: false,
            dock: DockLayout::default(),
            left_width: 280.0,
            bottom_height: 200.0,
            color_scheme: ColorScheme::Cn,
            work_mode: false,
            quote_interval_secs: default_quote_interval_secs(),
            watchlist_sort: WatchlistSort::Manual,
            ai_api: AiConfig::default(),
            chart_lines: std::collections::HashMap::new(),
            treasure_pool: default_treasure_pool(),
            treasure_fin: default_treasure_fin(),
            status_bar_enabled: false,
            status_bar_codes: Vec::new(),
            status_bar_active: String::new(),
        }
    }
}

/// Max codes that can be pinned to the status bar (menu bar space is limited).
pub const STATUS_BAR_MAX_CODES: usize = 5;

/// Keep only watchlist members, preserve order, cap length, and fix active.
pub fn normalize_status_bar(
    enabled: bool,
    codes: &[String],
    active: &str,
    watchlist: &[String],
) -> (bool, Vec<String>, String) {
    let mut out = Vec::new();
    for c in codes {
        if watchlist.iter().any(|w| w == c) && !out.iter().any(|x| x == c) {
            out.push(c.clone());
            if out.len() >= STATUS_BAR_MAX_CODES {
                break;
            }
        }
    }
    let active = if out.iter().any(|c| c == active) {
        active.to_string()
    } else {
        out.first().cloned().unwrap_or_default()
    };
    // Enabling with no pins is allowed; UI can auto-pin selected on toggle.
    (enabled, out, active)
}

#[cfg(test)]
mod status_bar_tests {
    use super::{normalize_status_bar, STATUS_BAR_MAX_CODES};

    #[test]
    fn drops_codes_not_in_watchlist_and_caps() {
        let watch: Vec<String> = (0..10).map(|i| format!("60000{i}")).collect();
        let codes: Vec<String> = (0..8)
            .map(|i| format!("60000{i}"))
            .chain(std::iter::once("999999".into()))
            .collect();
        let (en, out, active) = normalize_status_bar(true, &codes, "600003", &watch);
        assert!(en);
        assert_eq!(out.len(), STATUS_BAR_MAX_CODES);
        assert_eq!(active, "600003");
        assert!(!out.iter().any(|c| c == "999999"));
    }

    #[test]
    fn resets_active_when_missing() {
        let watch = vec!["600519".into(), "000001".into()];
        let codes = vec!["600519".into()];
        let (_, out, active) = normalize_status_bar(true, &codes, "000001", &watch);
        assert_eq!(out, vec!["600519".to_string()]);
        assert_eq!(active, "600519");
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
