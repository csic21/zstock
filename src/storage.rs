//! Local persistence for watchlist, UI preferences, and treasure scan cache.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::data::ai::AiConfig;
use crate::data::alerts::BuyAlert;
use crate::data::journal::Journal;
use crate::data::portfolio::Portfolio;
use crate::data::radar::RadarCache;
use crate::data::treasure::TreasureCache;
#[cfg(test)]
use crate::infrastructure::credential_store::MemorySecretStore;
#[cfg(not(test))]
use crate::infrastructure::credential_store::NativeSecretStore;
use crate::infrastructure::storage::json_store::{self, LoadError};
use crate::infrastructure::storage::migrations::DocumentKind;
use crate::model::{TrendLine, default_watchlist_codes};
use crate::services::secrets::SecretStore;

pub const CONFIG_SCHEMA_VERSION: u32 =
    crate::infrastructure::storage::migrations::CONFIG_SCHEMA_VERSION;

const AI_API_KEY_ACCOUNT: &str = "ai-api-key";
static LAST_STORAGE_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

pub fn record_storage_error(message: impl Into<String>) {
    *LAST_STORAGE_ERROR
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(message.into());
}

pub fn take_storage_error() -> Option<String> {
    LAST_STORAGE_ERROR
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

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
        [
            Self::Manual,
            Self::ChangeDesc,
            Self::ChangeAsc,
            Self::CodeAsc,
        ]
    }
}

/// Work-mode chrome density + companion window size.
///
/// Cycles Wide → Fit → Mini → Wide. Fit/Mini shrink row heights, hide journal
/// (Mini), and resize the OS window so the monitor skin fits a smaller desk
/// footprint without looking like a full trading terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum WorkDensity {
    /// Current roomy layout; restores the pre-compact window when leaving Fit/Mini.
    #[default]
    Wide = 0,
    /// Tighter rows + medium window (~920×580).
    Fit = 1,
    /// Densest chrome + small window (~720×440); journal hidden.
    Mini = 2,
}

impl WorkDensity {
    pub fn all() -> [Self; 3] {
        [Self::Wide, Self::Fit, Self::Mini]
    }

    pub fn next(self) -> Self {
        match self {
            Self::Wide => Self::Fit,
            Self::Fit => Self::Mini,
            Self::Mini => Self::Wide,
        }
    }

    /// Title-bar label (Focus chrome).
    pub fn label(self) -> &'static str {
        match self {
            Self::Wide => "Wide",
            Self::Fit => "Fit",
            Self::Mini => "Mini",
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Wide => "Window size · Wide (roomy) · click for Fit",
            Self::Fit => "Window size · Fit (~920×580, denser) · click for Mini",
            Self::Mini => "Window size · Mini (~720×440, densest) · click for Wide",
        }
    }

    /// Target content size when entering this density. `None` = restore saved bounds.
    pub fn window_size(self) -> Option<(f32, f32)> {
        match self {
            Self::Wide => None,
            Self::Fit => Some((920.0, 580.0)),
            Self::Mini => Some((720.0, 440.0)),
        }
    }

    /// Default host-panel width for this density (px).
    pub fn default_right_width(self) -> f32 {
        match self {
            Self::Wide => 340.0,
            Self::Fit => 280.0,
            Self::Mini => 220.0,
        }
    }

    pub fn right_width_range(self) -> (f32, f32) {
        match self {
            Self::Wide => (260.0, 420.0),
            Self::Fit => (200.0, 360.0),
            Self::Mini => (160.0, 300.0),
        }
    }
}

#[cfg(test)]
mod work_density_tests {
    use super::WorkDensity;

    #[test]
    fn density_cycles_wide_fit_mini() {
        assert_eq!(WorkDensity::Wide.next(), WorkDensity::Fit);
        assert_eq!(WorkDensity::Fit.next(), WorkDensity::Mini);
        assert_eq!(WorkDensity::Mini.next(), WorkDensity::Wide);
    }

    #[test]
    fn mini_window_is_smaller_than_fit() {
        let fit = WorkDensity::Fit.window_size().unwrap();
        let mini = WorkDensity::Mini.window_size().unwrap();
        assert!(mini.0 < fit.0 && mini.1 < fit.1);
        assert!(WorkDensity::Wide.window_size().is_none());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "current_config_schema_version")]
    pub schema_version: u32,
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
    /// Work-mode layout density + companion window size (Wide / Fit / Mini).
    #[serde(default)]
    pub work_density: WorkDensity,
    /// Right host-panel width in work mode (px). 0 = use density default.
    #[serde(default)]
    pub work_right_width: f32,
    /// Work-mode private service nicknames (`code` → alias). Never shown as stock ids.
    #[serde(default)]
    pub work_aliases: std::collections::HashMap<String, String>,
    /// Quote poll interval in seconds (clamped 1..=120). Default 1.
    #[serde(default = "default_quote_interval_secs")]
    pub quote_interval_secs: u64,
    /// How the watchlist is ordered in the sidebar.
    #[serde(default)]
    pub watchlist_sort: WatchlistSort,
    /// Optional LLM settings for the AI commentary feature.
    #[serde(default)]
    pub ai_api: AiConfig,
    /// Local buy-price alerts keyed by symbol code.
    #[serde(default)]
    pub buy_alerts: std::collections::HashMap<String, BuyAlert>,
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
    /// Watchlist codes pinned to the status bar (max 5). All pinned codes are shown together.
    #[serde(default)]
    pub status_bar_codes: Vec<String>,
    /// Preferred pin when focusing a symbol from the menu (still all codes are displayed).
    #[serde(default)]
    pub status_bar_active: String,
    /// Bottom analysis dock tab: `overview` | `strategy` | `ai` | `portfolio` | `treasure` | `indicators`.
    #[serde(default = "default_detail_tab")]
    pub detail_tab: String,
    /// Left sidebar tab: `watchlist` | `portfolio` | `treasure`.
    #[serde(default = "default_left_tab")]
    pub left_tab: String,
    /// 「现在找」主模式：`long` | `short`。
    #[serde(default = "default_find_mode")]
    pub find_mode: String,
    /// 自选分组标签：code → `long` / `short` / `watch` / `none`。
    #[serde(default)]
    pub watch_tags: std::collections::HashMap<String, String>,
    /// 自选列表筛选：`none` 表示全部。
    #[serde(default = "default_watch_filter")]
    pub watch_filter: String,
}

fn default_true() -> bool {
    true
}

fn current_config_schema_version() -> u32 {
    CONFIG_SCHEMA_VERSION
}

fn default_chart_kind() -> String {
    "day".into()
}

fn default_quote_interval_secs() -> u64 {
    // 2s balances live feel vs full-UI cost of each poll notify.
    2
}

fn default_treasure_pool() -> String {
    "mcap".into()
}

fn default_treasure_fin() -> String {
    "off".into()
}

fn default_detail_tab() -> String {
    "overview".into()
}

fn default_left_tab() -> String {
    "watchlist".into()
}

fn default_find_mode() -> String {
    "long".into()
}

fn default_watch_filter() -> String {
    "none".into()
}

/// Clamp user-facing quote interval.
pub fn clamp_quote_interval_secs(secs: u64) -> u64 {
    secs.clamp(1, 120)
}

impl Default for AppConfig {
    fn default() -> Self {
        let watchlist = default_watchlist_codes();
        let selected = watchlist
            .first()
            .cloned()
            .unwrap_or_else(|| "600519".into());
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
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
            // Compact by default so the overview strip doesn't sit in a tall empty dock.
            bottom_height: 168.0,
            color_scheme: ColorScheme::Cn,
            work_mode: false,
            work_density: WorkDensity::Wide,
            work_right_width: 0.0,
            work_aliases: std::collections::HashMap::new(),
            quote_interval_secs: default_quote_interval_secs(),
            watchlist_sort: WatchlistSort::Manual,
            ai_api: AiConfig::default(),
            buy_alerts: std::collections::HashMap::new(),
            chart_lines: std::collections::HashMap::new(),
            treasure_pool: default_treasure_pool(),
            treasure_fin: default_treasure_fin(),
            status_bar_enabled: false,
            status_bar_codes: Vec::new(),
            status_bar_active: String::new(),
            detail_tab: default_detail_tab(),
            left_tab: default_left_tab(),
            find_mode: default_find_mode(),
            watch_tags: std::collections::HashMap::new(),
            watch_filter: default_watch_filter(),
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
    use super::{STATUS_BAR_MAX_CODES, normalize_status_bar};

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
    if let Some(path) = std::env::var_os("ZSTOCK_DATA_DIR").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
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

pub fn radar_cache_path() -> PathBuf {
    app_data_dir().join("radar_cache.json")
}

pub fn portfolio_path() -> PathBuf {
    app_data_dir().join("portfolio.json")
}

pub fn journal_path() -> PathBuf {
    app_data_dir().join("journal.json")
}

/// Unit tests must never access the developer's native credential store. Changing
/// HOME or ZSTOCK_DATA_DIR does not isolate Keychain / Secret Service / DPAPI.
fn config_secret_store() -> &'static dyn SecretStore {
    #[cfg(not(test))]
    {
        &NativeSecretStore
    }
    #[cfg(test)]
    {
        static STORE: OnceLock<MemorySecretStore> = OnceLock::new();
        STORE.get_or_init(MemorySecretStore::default)
    }
}

pub fn load_config() -> AppConfig {
    load_config_with_store(&config_path(), config_secret_store())
}

fn load_config_with_store(path: &Path, secrets: &dyn SecretStore) -> AppConfig {
    match json_store::load::<AppConfig>(path, DocumentKind::Config) {
        Ok(mut loaded) => {
            if let Some(secret) = loaded.migration.legacy_api_key.as_ref() {
                match secrets.get(AI_API_KEY_ACCOUNT) {
                    Ok(Some(stored)) => {
                        let matches_legacy = stored == *secret;
                        loaded.value.ai_api.api_key = stored;
                        if !matches_legacy {
                            record_storage_error(
                                "配置升级暂停：旧配置的 API Key 与系统凭据库不一致；保留现有凭据和原文件，请在 AI 设置中明确更新或清除 Key",
                            );
                            return loaded.value;
                        }
                    }
                    Ok(None) => {
                        loaded.value.ai_api.api_key = secret.clone();
                        if let Err(error) = secrets.set(AI_API_KEY_ACCOUNT, secret) {
                            record_storage_error(format!(
                                "配置升级暂停：API Key 尚未写入系统凭据库（{error}）；原文件保持不变"
                            ));
                            return loaded.value;
                        }
                    }
                    Err(error) => {
                        record_storage_error(format!(
                            "配置升级暂停：系统凭据库读取失败（{error}）；现有凭据和原文件保持不变"
                        ));
                        return loaded.value;
                    }
                }
            } else {
                match secrets.get(AI_API_KEY_ACCOUNT) {
                    Ok(Some(secret)) => loaded.value.ai_api.api_key = secret,
                    Ok(None) => {}
                    Err(error) => record_storage_error(format!("系统凭据库读取失败：{error}")),
                }
            }
            if loaded.migration.migrated
                && let Err(error) = finish_config_migration(path, &loaded)
            {
                record_storage_error(format!("配置迁移未落盘，原文件保持不变：{error:#}"));
            }
            loaded.value
        }
        Err(LoadError::NotFound) => AppConfig::default(),
        Err(error) => {
            let backups = json_store::latest_backups(path).unwrap_or_default();
            record_storage_error(format!(
                "配置进入恢复模式：{error}；可用备份 {} 份",
                backups.len()
            ));
            AppConfig::default()
        }
    }
}

fn finish_config_migration(path: &Path, loaded: &json_store::Loaded<AppConfig>) -> Result<()> {
    // The migrated JSON retains the original non-secret fields. Do not create
    // another plaintext copy of a legacy key in the migration backup.
    json_store::backup_before_migration_with_value(
        path,
        loaded.migration.from_version,
        &loaded.migration.value,
    )?;
    json_store::save(path, &loaded.value)
}

/// Save only ordinary preferences. An empty in-memory key may mean that the
/// credential store could not be read; it must never imply credential deletion.
pub fn save_config(cfg: &AppConfig) -> Result<()> {
    save_config_at(&config_path(), cfg)
}

fn save_config_at(path: &Path, cfg: &AppConfig) -> Result<()> {
    match json_store::load::<AppConfig>(path, DocumentKind::Config) {
        Ok(loaded) if loaded.migration.legacy_api_key.is_some() => {
            anyhow::bail!(
                "API Key 安全迁移尚未完成；为保留原密钥，暂不覆盖旧配置，请解锁系统凭据库后重试"
            );
        }
        Ok(_) | Err(LoadError::NotFound) => json_store::save(path, cfg),
        Err(error) => anyhow::bail!("为保留原配置，暂不覆盖无法读取的文件：{error}"),
    }
}

/// Called only after an actual edit of the API-key input. Empty text explicitly
/// clears the stored credential; routine preferences never call this function.
pub fn save_ai_api_key(api_key: &str) -> Result<()> {
    save_ai_api_key_with_store(&config_path(), api_key, config_secret_store())
}

fn save_ai_api_key_with_store(path: &Path, api_key: &str, secrets: &dyn SecretStore) -> Result<()> {
    // Scrub legacy plaintext before applying an explicit edit so clearing a
    // key cannot resurrect it at the next launch. Preserve any existing native
    // key until the requested edit succeeds; never overwrite it with legacy
    // data as an intermediate migration step.
    match json_store::load::<AppConfig>(path, DocumentKind::Config) {
        Ok(loaded) => {
            if let Some(secret) = loaded.migration.legacy_api_key.as_deref() {
                let existing = secrets
                    .get(AI_API_KEY_ACCOUNT)
                    .map_err(anyhow::Error::new)
                    .context("read credential store before legacy API key edit")?;
                if existing.is_none() {
                    secrets
                        .set(AI_API_KEY_ACCOUNT, secret)
                        .map_err(anyhow::Error::new)
                        .context("migrate legacy API key to credential store")?;
                }
                finish_config_migration(path, &loaded)?;
            }
        }
        Err(LoadError::NotFound) => {}
        Err(error) => anyhow::bail!("cannot safely edit API key with unreadable config: {error}"),
    }
    let api_key = api_key.trim();
    if api_key.is_empty() {
        secrets
            .delete(AI_API_KEY_ACCOUNT)
            .map_err(anyhow::Error::new)
            .context("delete API key from credential store")
    } else {
        secrets
            .set(AI_API_KEY_ACCOUNT, api_key)
            .map_err(anyhow::Error::new)
            .context("save API key to credential store")
    }
}

pub fn load_treasure_cache() -> TreasureCache {
    let path = treasure_cache_path();
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => TreasureCache::default(),
    }
}

pub fn save_treasure_cache(cache: &TreasureCache) -> Result<()> {
    json_store::save(&treasure_cache_path(), cache)
}

pub fn load_radar_cache() -> RadarCache {
    let path = radar_cache_path();
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => RadarCache::default(),
    }
}

pub fn save_radar_cache(cache: &RadarCache) -> Result<()> {
    json_store::save(&radar_cache_path(), cache)
}

pub fn load_portfolio() -> Portfolio {
    let path = portfolio_path();
    match json_store::load::<Portfolio>(&path, DocumentKind::Portfolio) {
        Ok(loaded) => {
            if loaded.migration.migrated {
                let result =
                    json_store::backup_before_migration(&path, loaded.migration.from_version)
                        .and_then(|_| json_store::save(&path, &loaded.value));
                if let Err(error) = result {
                    record_storage_error(format!("持仓迁移未落盘，原文件保持不变：{error:#}"));
                }
            }
            loaded.value
        }
        Err(LoadError::NotFound) => Portfolio::default(),
        Err(error) => {
            record_storage_error(format!("持仓进入恢复模式：{error}"));
            Portfolio::default()
        }
    }
}

pub fn save_portfolio(portfolio: &Portfolio) -> Result<()> {
    json_store::save(&portfolio_path(), portfolio)
}

pub fn load_journal() -> Journal {
    let path = journal_path();
    match json_store::load::<Journal>(&path, DocumentKind::Journal) {
        Ok(loaded) => {
            if loaded.migration.migrated {
                let result =
                    json_store::backup_before_migration(&path, loaded.migration.from_version)
                        .and_then(|_| json_store::save(&path, &loaded.value));
                if let Err(error) = result {
                    record_storage_error(format!("日记迁移未落盘，原文件保持不变：{error:#}"));
                }
            }
            loaded.value
        }
        Err(LoadError::NotFound) => Journal::default(),
        Err(error) => {
            record_storage_error(format!("日记进入恢复模式：{error}"));
            Journal::default()
        }
    }
}

pub fn save_journal(journal: &Journal) -> Result<()> {
    json_store::save(&journal_path(), journal)
}

pub fn export_journal(journal: &Journal) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = app_data_dir().join(format!("journal-export-{stamp}.json"));
    json_store::save(&path, journal)?;
    Ok(path)
}

#[cfg(test)]
mod credential_persistence_tests {
    use super::*;
    use crate::services::secrets::SecretError;

    #[derive(Default)]
    struct TestSecretStore {
        memory: MemorySecretStore,
        calls: Mutex<Vec<&'static str>>,
        fail_get: bool,
        fail_set: bool,
        fail_delete: bool,
    }

    impl SecretStore for TestSecretStore {
        fn get(&self, account: &str) -> std::result::Result<Option<String>, SecretError> {
            self.calls.lock().unwrap().push("get");
            if self.fail_get {
                return Err(SecretError("fixture read unavailable".into()));
            }
            self.memory.get(account)
        }

        fn set(&self, account: &str, secret: &str) -> std::result::Result<(), SecretError> {
            self.calls.lock().unwrap().push("set");
            if self.fail_set {
                return Err(SecretError("fixture write unavailable".into()));
            }
            self.memory.set(account, secret)
        }

        fn delete(&self, account: &str) -> std::result::Result<(), SecretError> {
            self.calls.lock().unwrap().push("delete");
            if self.fail_delete {
                return Err(SecretError("fixture delete unavailable".into()));
            }
            self.memory.delete(account)
        }
    }

    struct ConfigFixture(PathBuf);

    impl ConfigFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "zstock-credential-prefs-{}-{stamp}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&directory).unwrap();
            Self(directory.join("config.json"))
        }

        fn legacy(&self) -> Vec<u8> {
            let mut value = serde_json::to_value(AppConfig::default()).unwrap();
            value.as_object_mut().unwrap().remove("schema_version");
            value["ai_api"]["api_key"] = serde_json::json!("fixture-legacy-secret");
            let bytes = serde_json::to_vec_pretty(&value).unwrap();
            fs::write(&self.0, &bytes).unwrap();
            bytes
        }

        fn assert_redacted(&self) {
            for entry in fs::read_dir(self.0.parent().unwrap()).unwrap() {
                let bytes = fs::read(entry.unwrap().path()).unwrap();
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert!(value.pointer("/ai_api/api_key").is_none());
                let text = String::from_utf8(bytes).unwrap();
                assert!(!text.contains("fixture-legacy-secret"));
                assert!(!text.contains("fixture-new-secret"));
            }
        }
    }

    impl Drop for ConfigFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn ordinary_preferences_save_without_credential_service() {
        let fixture = ConfigFixture::new();
        let mut cfg = AppConfig::default();
        cfg.watchlist.clear();
        cfg.dock.main_h = vec![280.0, 720.0];
        cfg.ai_api.api_key = "fixture-new-secret".into();
        save_config_at(&fixture.0, &cfg).unwrap();
        cfg.ai_api.api_key.clear();
        cfg.work_mode = true;
        save_config_at(&fixture.0, &cfg).unwrap();
        let loaded = json_store::load::<AppConfig>(&fixture.0, DocumentKind::Config).unwrap();
        assert!(loaded.value.watchlist.is_empty());
        assert_eq!(loaded.value.dock.main_h, vec![280.0, 720.0]);
        assert!(loaded.value.work_mode);
        fixture.assert_redacted();
    }

    #[test]
    fn failed_credential_read_never_turns_preference_save_into_delete() {
        let fixture = ConfigFixture::new();
        save_config_at(&fixture.0, &AppConfig::default()).unwrap();
        let secrets = TestSecretStore {
            fail_get: true,
            ..Default::default()
        };
        secrets
            .memory
            .set(AI_API_KEY_ACCOUNT, "fixture-existing-secret")
            .unwrap();
        let mut cfg = load_config_with_store(&fixture.0, &secrets);
        assert!(cfg.ai_api.api_key.is_empty());
        cfg.watchlist.clear();
        save_config_at(&fixture.0, &cfg).unwrap();
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get"]);
        assert_eq!(
            secrets.memory.get(AI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some("fixture-existing-secret")
        );
        assert!(
            json_store::load::<AppConfig>(&fixture.0, DocumentKind::Config)
                .unwrap()
                .value
                .watchlist
                .is_empty()
        );
        fixture.assert_redacted();
    }

    #[test]
    fn explicit_key_edits_store_trimmed_secret_and_clear_it() {
        let fixture = ConfigFixture::new();
        save_config_at(&fixture.0, &AppConfig::default()).unwrap();
        let secrets = TestSecretStore::default();
        save_ai_api_key_with_store(&fixture.0, "  fixture-new-secret  ", &secrets).unwrap();
        assert_eq!(
            load_config_with_store(&fixture.0, &secrets).ai_api.api_key,
            "fixture-new-secret"
        );
        save_ai_api_key_with_store(&fixture.0, "  ", &secrets).unwrap();
        assert!(
            load_config_with_store(&fixture.0, &secrets)
                .ai_api
                .api_key
                .is_empty()
        );
        assert_eq!(
            *secrets.calls.lock().unwrap(),
            vec!["set", "get", "delete", "get"]
        );
        fixture.assert_redacted();
    }

    #[test]
    fn failed_explicit_key_edit_does_not_block_ordinary_preferences() {
        let fixture = ConfigFixture::new();
        let mut cfg = AppConfig::default();
        save_config_at(&fixture.0, &cfg).unwrap();
        let secrets = TestSecretStore {
            fail_set: true,
            ..Default::default()
        };
        secrets
            .memory
            .set(AI_API_KEY_ACCOUNT, "fixture-existing-secret")
            .unwrap();
        cfg.ai_api.api_key = "fixture-new-secret".into();
        cfg.watchlist.clear();
        assert!(save_ai_api_key_with_store(&fixture.0, &cfg.ai_api.api_key, &secrets).is_err());
        save_config_at(&fixture.0, &cfg).unwrap();
        assert_eq!(
            secrets.memory.get(AI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some("fixture-existing-secret")
        );
        let loaded = json_store::load::<AppConfig>(&fixture.0, DocumentKind::Config).unwrap();
        assert!(loaded.value.watchlist.is_empty());
        fixture.assert_redacted();
    }

    #[test]
    fn failed_explicit_delete_preserves_stored_key() {
        let fixture = ConfigFixture::new();
        save_config_at(&fixture.0, &AppConfig::default()).unwrap();
        let secrets = TestSecretStore {
            fail_delete: true,
            ..Default::default()
        };
        secrets
            .memory
            .set(AI_API_KEY_ACCOUNT, "fixture-existing-secret")
            .unwrap();
        assert!(save_ai_api_key_with_store(&fixture.0, "", &secrets).is_err());
        assert_eq!(
            secrets.memory.get(AI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some("fixture-existing-secret")
        );
        fixture.assert_redacted();
    }

    #[test]
    fn legacy_migration_stores_secret_before_redacting_config_and_backup() {
        let fixture = ConfigFixture::new();
        fixture.legacy();
        let secrets = TestSecretStore::default();
        let cfg = load_config_with_store(&fixture.0, &secrets);
        assert_eq!(cfg.ai_api.api_key, "fixture-legacy-secret");
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "set"]);
        assert_eq!(json_store::latest_backups(&fixture.0).unwrap().len(), 1);
        fixture.assert_redacted();
        assert_eq!(
            load_config_with_store(&fixture.0, &secrets).ai_api.api_key,
            "fixture-legacy-secret"
        );
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "set", "get"]);
    }

    #[test]
    fn failed_legacy_migration_preserves_original_through_unrelated_saves() {
        let fixture = ConfigFixture::new();
        let original = fixture.legacy();
        let secrets = TestSecretStore {
            fail_set: true,
            ..Default::default()
        };
        let mut cfg = load_config_with_store(&fixture.0, &secrets);
        assert_eq!(cfg.ai_api.api_key, "fixture-legacy-secret");
        cfg.watchlist.clear();
        assert!(save_config_at(&fixture.0, &cfg).is_err());
        assert_eq!(fs::read(&fixture.0).unwrap(), original);
        assert!(json_store::latest_backups(&fixture.0).unwrap().is_empty());
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "set"]);
    }

    #[test]
    fn explicit_clear_after_legacy_migration_cannot_resurrect_secret() {
        let fixture = ConfigFixture::new();
        fixture.legacy();
        let secrets = TestSecretStore::default();
        save_ai_api_key_with_store(&fixture.0, "", &secrets).unwrap();
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "set", "delete"]);
        assert!(
            load_config_with_store(&fixture.0, &secrets)
                .ai_api
                .api_key
                .is_empty()
        );
        fixture.assert_redacted();
        // Restoring the sanitized migration backup cannot restore the removed key.
        let backup = json_store::latest_backups(&fixture.0).unwrap().remove(0);
        json_store::restore_backup(&fixture.0, &backup).unwrap();
        assert!(
            load_config_with_store(&fixture.0, &secrets)
                .ai_api
                .api_key
                .is_empty()
        );
    }

    #[test]
    fn explicit_replacement_after_legacy_migration_keeps_new_secret() {
        let fixture = ConfigFixture::new();
        fixture.legacy();
        let secrets = TestSecretStore::default();
        save_ai_api_key_with_store(&fixture.0, "fixture-new-secret", &secrets).unwrap();
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "set", "set"]);
        assert_eq!(
            load_config_with_store(&fixture.0, &secrets).ai_api.api_key,
            "fixture-new-secret"
        );
        fixture.assert_redacted();
    }

    #[test]
    fn failed_legacy_clear_keeps_the_only_copy_and_never_deletes() {
        let fixture = ConfigFixture::new();
        let original = fixture.legacy();
        let secrets = TestSecretStore {
            fail_set: true,
            ..Default::default()
        };
        assert!(save_ai_api_key_with_store(&fixture.0, "", &secrets).is_err());
        assert_eq!(fs::read(&fixture.0).unwrap(), original);
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "set"]);
    }

    #[test]
    fn legacy_migration_conflict_preserves_newer_native_key_and_original_file() {
        let fixture = ConfigFixture::new();
        let original = fixture.legacy();
        let secrets = TestSecretStore::default();
        secrets
            .memory
            .set(AI_API_KEY_ACCOUNT, "fixture-new-secret")
            .unwrap();
        let cfg = load_config_with_store(&fixture.0, &secrets);
        assert_eq!(cfg.ai_api.api_key, "fixture-new-secret");
        assert_eq!(fs::read(&fixture.0).unwrap(), original);
        assert!(save_config_at(&fixture.0, &cfg).is_err());
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get"]);
        assert_eq!(
            secrets.memory.get(AI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some("fixture-new-secret")
        );
        assert!(json_store::latest_backups(&fixture.0).unwrap().is_empty());
    }

    #[test]
    fn matching_legacy_and_native_key_migrates_without_rewriting_credential() {
        let fixture = ConfigFixture::new();
        fixture.legacy();
        let secrets = TestSecretStore::default();
        secrets
            .memory
            .set(AI_API_KEY_ACCOUNT, "fixture-legacy-secret")
            .unwrap();
        let cfg = load_config_with_store(&fixture.0, &secrets);
        assert_eq!(cfg.ai_api.api_key, "fixture-legacy-secret");
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get"]);
        fixture.assert_redacted();
    }

    #[test]
    fn legacy_store_read_failure_blocks_migration_and_explicit_edit_without_writes() {
        let fixture = ConfigFixture::new();
        let original = fixture.legacy();
        let secrets = TestSecretStore {
            fail_get: true,
            ..Default::default()
        };
        secrets
            .memory
            .set(AI_API_KEY_ACCOUNT, "fixture-new-secret")
            .unwrap();
        let cfg = load_config_with_store(&fixture.0, &secrets);
        assert!(cfg.ai_api.api_key.is_empty());
        assert!(save_config_at(&fixture.0, &cfg).is_err());
        assert!(save_ai_api_key_with_store(&fixture.0, "replacement", &secrets).is_err());
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "get"]);
        assert_eq!(fs::read(&fixture.0).unwrap(), original);
        assert_eq!(
            secrets.memory.get(AI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some("fixture-new-secret")
        );
        assert!(json_store::latest_backups(&fixture.0).unwrap().is_empty());
    }

    #[test]
    fn explicit_replacement_never_temporarily_overwrites_newer_key_with_legacy() {
        let fixture = ConfigFixture::new();
        fixture.legacy();
        let secrets = TestSecretStore {
            fail_set: true,
            ..Default::default()
        };
        secrets
            .memory
            .set(AI_API_KEY_ACCOUNT, "fixture-new-secret")
            .unwrap();
        assert!(save_ai_api_key_with_store(&fixture.0, "replacement", &secrets).is_err());
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "set"]);
        assert_eq!(
            secrets.memory.get(AI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some("fixture-new-secret")
        );
        fixture.assert_redacted();
        // The failed requested write can now retry without any legacy migration.
        let secrets = TestSecretStore {
            memory: secrets.memory,
            ..Default::default()
        };
        save_ai_api_key_with_store(&fixture.0, "replacement", &secrets).unwrap();
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["set"]);
        assert_eq!(
            secrets.memory.get(AI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some("replacement")
        );
    }

    #[test]
    fn explicit_clear_of_conflicting_legacy_config_preserves_newer_key_until_delete() {
        let fixture = ConfigFixture::new();
        fixture.legacy();
        let secrets = TestSecretStore {
            fail_delete: true,
            ..Default::default()
        };
        secrets
            .memory
            .set(AI_API_KEY_ACCOUNT, "fixture-new-secret")
            .unwrap();
        assert!(save_ai_api_key_with_store(&fixture.0, "", &secrets).is_err());
        assert_eq!(*secrets.calls.lock().unwrap(), vec!["get", "delete"]);
        assert_eq!(
            secrets.memory.get(AI_API_KEY_ACCOUNT).unwrap().as_deref(),
            Some("fixture-new-secret")
        );
        fixture.assert_redacted();
    }

    #[test]
    fn unreadable_config_is_not_overwritten_or_used_for_credential_edits() {
        let fixture = ConfigFixture::new();
        fs::write(&fixture.0, b"{broken").unwrap();
        let secrets = TestSecretStore::default();
        assert!(save_config_at(&fixture.0, &AppConfig::default()).is_err());
        assert!(save_ai_api_key_with_store(&fixture.0, "", &secrets).is_err());
        assert_eq!(fs::read(&fixture.0).unwrap(), b"{broken");
        assert!(secrets.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn ordinary_test_store_is_persistent_in_memory() {
        // The cfg(test) factory has no NativeSecretStore branch. This verifies
        // repeated app/storage calls share the fake rather than recreating it.
        let account = "credential-persistence-test-only-account";
        config_secret_store()
            .set(account, "fixture-new-secret")
            .unwrap();
        assert_eq!(
            config_secret_store().get(account).unwrap().as_deref(),
            Some("fixture-new-secret")
        );
        config_secret_store().delete(account).unwrap();
        assert!(config_secret_store().get(account).unwrap().is_none());
    }
}
