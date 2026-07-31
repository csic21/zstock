//! Local persistence for watchlist, UI preferences, and treasure scan cache.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Pure A-share codes in watchlist order.
    pub watchlist: Vec<String>,
    pub selected: String,
    /// `1M` | `3M` | `6M` | `1Y`
    pub range: String,
    pub show_ma5: bool,
    pub show_ma10: bool,
    pub show_ma20: bool,
    /// Left panel width hint (px).
    pub left_width: f32,
    /// Bottom panel height hint (px).
    pub bottom_height: f32,
    /// Up/down color convention: `cn` (红涨绿跌) or `us` (绿涨红跌).
    #[serde(default)]
    pub color_scheme: ColorScheme,
}

impl Default for AppConfig {
    fn default() -> Self {
        let watchlist = default_watchlist_codes();
        let selected = watchlist.first().cloned().unwrap_or_else(|| "600519".into());
        Self {
            watchlist,
            selected,
            range: "3M".into(),
            show_ma5: true,
            show_ma10: true,
            show_ma20: true,
            left_width: 280.0,
            bottom_height: 200.0,
            color_scheme: ColorScheme::Cn,
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
