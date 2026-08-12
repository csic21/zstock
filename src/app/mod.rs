//! Root application: A 股 / 港股 watchlist, chart (MA + crosshair), resizable layout, persistence.
//!
//! Split across submodules by concern:
//! - [`types`] — enums / small state types
//! - [`market`] / [`portfolio`] / [`symbols`] / [`prefs`] / [`chart_ctrl`] — logic
//! - [`ui`] — render methods
//! - [`helpers`] — pure formatting helpers

mod alerts;
mod chart_ctrl;
mod helpers;
mod labels;
mod market;
mod market_analysis;
mod portfolio;
mod prefs;
mod series_cache;
mod state;
mod strategy_lab;
mod symbols;
mod today;
mod treemap;
mod types;
mod ui;
mod view_models;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    KeyBinding, KeyDownEvent, KeyUpEvent, ParentElement, Pixels, Point, Render, SharedString,
    Styled, Window, WindowBounds, WindowOptions, actions, div, hsla, point, prelude::FluentBuilder,
    px, size,
};
use gpui_component::{
    ActiveTheme, PixelsExt, Root, TITLE_BAR_HEIGHT, Theme, ThemeMode, TitleBar,
    input::{InputEvent, InputState},
    resizable::{h_resizable, resizable_panel, v_resizable},
    v_flex,
};

use crate::data::ai::AiConfig;
use crate::data::alerts::BuyAlert;
use crate::data::backtest::{BacktestReport, BacktestRule};
use crate::data::groups::{FindMode, WatchTag};
use crate::data::indicators::{BollSeries, MaSeries, MacdSeries};
use crate::data::journal::Journal;
use crate::data::levels;
use crate::data::market as market_data;
use crate::data::market_analysis as market_analysis_data;
use crate::data::portfolio::{Portfolio, TradeSide};
use crate::data::radar::{RadarHit, RadarStrategy};
use crate::data::scout::ScoutPick;
use crate::data::signals;
use crate::data::treasure::TreasureHit;
use crate::data::universe::{FinFilter, TreasurePool};
use crate::domain::money::Currency;
use crate::model::{
    Candle, IndexSnap, MinuteSeries, Symbol, TrendLine, board_for_code, normalize_code, shared,
};
use crate::storage::{
    self, ColorScheme, DockLayout, WatchlistSort, WorkDensity, clamp_quote_interval_secs,
    normalize_status_bar,
};
use crate::update::UpdateState;

use crate::features::strategy_lab::StrategyLabFeature;
#[cfg(feature = "work-mode")]
use state::WorkModeFeature;
use state::{
    AnalysisState, AppServices, ChartState, DiscoveryState, MarketState, PortfolioState,
    RuntimeState, UiState,
};
use types::{
    AiCacheEntry, AiPanelState, AiSource, ChartKind, ChartRange, DetailTab, LeftTab, MarketRegion,
    SettingsSection,
};

actions!(
    stock,
    [
        ToggleCommandPalette,
        RefreshData,
        SelectTodayTask,
        SelectResearchTask,
        SelectOpportunitiesTask,
        SelectPortfolioTask,
        ToggleTreasure,
        ToggleWorkMode,
        ToggleSettings,
        DismissOverlay,
        SelectPrevSymbol,
        SelectNextSymbol,
        RemoveSelectedSymbol,
        ResetChartZoom,
        Quit
    ]
);

/// Window title in normal vs work mode.
const TITLE_NORMAL: &str = "ZStock · A股/港股";
const TITLE_WORK: &str = "Notes";

/// Preset quote poll intervals offered in Settings (seconds).
const QUOTE_INTERVAL_PRESETS: &[u64] = &[1, 2, 3, 5, 8, 15, 30, 60];
const QUOTE_INTERVAL_ERR_MAX: Duration = Duration::from_secs(45);
/// Minimum candles visible when zoomed in.
const CHART_MIN_VISIBLE: usize = 15;
/// 寻宝扫描相邻请求间隔，降低限流概率。
/// 扩大扫描时相邻请求间隔（约 400 只 × 150ms ≈ 1 分钟级）。
const TREASURE_SCAN_GAP: Duration = Duration::from_millis(150);
/// Debounce window for config.json writes (typing / layout thrash).
pub(crate) const PERSIST_DEBOUNCE: Duration = Duration::from_millis(400);
/// Latched Map reveal auto-hides after this delay (hold-to-peek is separate).
pub(crate) const WORK_IDENTITY_AUTO_HIDE: Duration = Duration::from_secs(6);

/// GPUI lifecycle shell. Business/request state is held in explicit aggregates;
/// `legacy` remains temporarily deref-compatible while controllers are adopted incrementally.
pub struct StockApp {
    services: AppServices,
    market_state: MarketState,
    chart_state: ChartState,
    discovery_state: DiscoveryState,
    portfolio_state: PortfolioState,
    analysis_state: AnalysisState,
    ui_state: UiState,
    runtime_state: RuntimeState,
    pub(crate) strategy_lab_feature: StrategyLabFeature,
    #[cfg(feature = "work-mode")]
    work_mode_feature: WorkModeFeature,
    legacy: AppState,
}

pub struct AppState {
    symbols: Vec<Symbol>,
    selected: SharedString,
    /// Code that currently loaded `candles` belong to (may lag `selected` while loading).
    candles_code: Option<String>,
    /// Monotonic token so stale async kline responses are dropped.
    kline_gen: u64,
    candles: Vec<Candle>,
    /// Strategy / levels for current `candles` (refreshed on series apply).
    signal_cache: Option<signals::SignalSnapshot>,
    levels_cache: Option<levels::ReferenceLevels>,
    ma: MaSeries,
    range: ChartRange,
    chart_kind: ChartKind,
    /// 分时数据（仅 Intraday 模式使用）。
    minute: Option<MinuteSeries>,
    /// Code that currently loaded `minute` belong to.
    minute_code: Option<String>,
    /// Monotonic token so stale async minute responses are dropped.
    minute_gen: u64,
    show_ma5: bool,
    show_ma10: bool,
    show_ma20: bool,
    show_ma60: bool,
    show_volume: bool,
    show_macd: bool,
    show_boll: bool,
    macd: MacdSeries,
    boll: BollSeries,
    hover_ix: Option<usize>,
    /// Visible window into `candles` for zoom/pan (inclusive start).
    chart_view_start: usize,
    /// Number of candles shown; clamped to series length. `0` means “show all”.
    chart_view_count: usize,
    chart_width: f32,
    chart_origin: Point<Pixels>,
    /// Full bounds of the chart surface (for drawing line anchors).
    chart_bounds: Bounds<Pixels>,
    /// Drawing mode: drag on the chart to create trend/price lines.
    drawing_mode: bool,
    /// Anchor of the line being drawn (abs candle index, price).
    drawing_anchor: Option<(usize, f64)>,
    /// In-progress line preview while dragging.
    draft_line: Option<TrendLine>,
    /// Per-symbol persisted lines (index/price anchors).
    chart_lines: std::collections::HashMap<String, Vec<TrendLine>>,
    /// Next palette color index for newly drawn lines.
    draw_color_ix: usize,
    /// In-memory K-line / minute series cache for instant symbol switches.
    series_cache: series_cache::SeriesCache,
    status: SharedString,
    /// True when the selected series has no paintable data yet (cold load).
    loading: bool,
    /// True when a background refresh is in flight (cache already painted).
    refreshing: bool,
    data_source: SharedString,
    palette_open: bool,
    /// Keyboard highlight index into palette rows (local first, then remote).
    palette_index: usize,
    /// Full-page settings (not a modal).
    settings_open: bool,
    /// Full-page market analysis view.
    market_analysis_open: bool,
    /// Region selected in the market analysis view.
    market_analysis_region: MarketRegion,
    /// Real-time A-share industry sectors.
    market_analysis_sectors: Vec<market_data::SectorTick>,
    /// Complete Shenwan level-1 -> level-2 -> stock heatmap hierarchy.
    market_heatmap_sectors: Arc<Vec<market_data::IndustryHeatmapSector>>,
    /// Current presentation inside the industry heatmap.
    market_heatmap_list: bool,
    /// Focus mode where the heatmap fills the whole market-analysis viewport.
    market_heatmap_fullscreen: bool,
    market_heatmap_loading: bool,
    market_heatmap_error: Option<SharedString>,
    market_analysis_loading: bool,
    market_analysis_error: Option<SharedString>,
    market_analysis_source: SharedString,
    market_analysis_updated: Option<SharedString>,
    market_analysis_gen: u64,
    /// Button-triggered market AI/local analysis.
    market_ai_panel: AiPanelState,
    market_ai_picks: Vec<market_analysis_data::MarketPick>,
    market_ai_gen: u64,
    /// Active section inside the settings page.
    settings_section: SettingsSection,
    /// Auto-update state (GitHub Releases).
    update_state: UpdateState,
    /// Quote poll interval (seconds), from config.
    quote_interval_secs: u64,
    palette_query: Entity<InputState>,
    palette_focus: FocusHandle,
    /// Search results for palette (remote + local).
    palette_hits: Vec<Symbol>,
    filtered_local: Vec<usize>,
    left_width: f32,
    bottom_height: f32,
    /// Full dock panel sizes (all groups), persisted.
    dock: DockLayout,
    /// Last-known window frame (x, y, w, h); persisted on change.
    window_bounds: Option<(f32, f32, f32, f32)>,
    /// 涨跌配色：中国红涨绿跌 / 美国绿涨红跌
    color_scheme: ColorScheme,
    /// 工作模式：中性文案 + 去红绿
    work_mode: bool,
    /// Work-mode layout density (Wide / Fit / Mini); persisted.
    work_density: WorkDensity,
    /// Host-panel width in work mode (px). 0 = density default.
    work_right_width: f32,
    /// Window bounds captured before entering Fit/Mini (restore on Wide).
    work_restore_bounds: Option<(f32, f32, f32, f32)>,
    /// Temporary owner map in work mode; intentionally never persisted.
    /// True when peek-held **or** Map is latched.
    work_identity_reveal: bool,
    /// Hold-to-peek (` / Space) is currently pressed.
    work_identity_peek_held: bool,
    /// Map button latched open (auto-hides after [`WORK_IDENTITY_AUTO_HIDE`]).
    work_identity_map_latched: bool,
    /// Cancels stale auto-hide timers.
    work_identity_hide_gen: u64,
    /// User-defined service nicknames for work mode (`code` → alias).
    work_aliases: HashMap<String, String>,
    /// Inline alias editor open for the selected service.
    work_alias_editing: bool,
    work_alias_input: Entity<InputState>,
    /// Sidebar row order.
    watchlist_sort: WatchlistSort,
    quote_fail_streak: u32,
    /// 左侧：自选 / 持仓 / 寻宝鼠（写入 config）。
    left_tab: LeftTab,
    /// 底部分析台当前分区（写入 config）。
    detail_tab: DetailTab,
    /// 寻宝扫描结果（按 score 降序）。
    treasure_hits: Vec<TreasureHit>,
    /// 候选池来源。
    treasure_pool: TreasurePool,
    /// 财务分位过滤。
    treasure_fin: FinFilter,
    treasure_scanning: bool,
    /// 静默预扫：不切 Tab、不抢状态栏、保留旧榜直至完成。
    treasure_scan_silent: bool,
    treasure_done: usize,
    treasure_total: usize,
    treasure_status: SharedString,
    /// 取消过期扫描。
    treasure_gen: u64,
    /// AI/本地批量「可买观察」结果（按可买分排序）。
    scout_picks: Vec<ScoutPick>,
    /// 整榜摘要（本地规则或 LLM）。
    scout_summary: SharedString,
    scout_running: bool,
    /// 静默筛可买：完成后不自动打开第一只。
    scout_silent: bool,
    scout_done: usize,
    scout_total: usize,
    /// 取消过期筛分。
    scout_gen: u64,
    /// 摘要来源说明。
    scout_source: SharedString,
    /// 可买清单过滤：true = 只显示「可关注」，隐藏「观察」。
    scout_only_buy_watch: bool,
    /// 有可买清单时，完整寻宝榜默认折叠，减少干扰。
    treasure_list_expanded: bool,
    /// 「现在找」：长线寻宝 / 短线雷达。
    find_mode: FindMode,
    /// 长线缓存时间戳（用于新鲜度横幅）。
    treasure_updated_at: String,
    /// 短线雷达结果。
    radar_hits: Vec<RadarHit>,
    radar_updated_at: String,
    radar_scanning: bool,
    radar_done: usize,
    radar_total: usize,
    radar_status: SharedString,
    radar_summary: SharedString,
    radar_gen: u64,
    /// 策略过滤；`None` = 全部策略。
    radar_filter: Option<RadarStrategy>,
    /// 自选分组 code → tag。
    watch_tags: HashMap<String, WatchTag>,
    /// 自选列表筛选（None = 全部）。
    watch_filter: WatchTag,
    /// 市场分析：选中的行业板块代码 / 名称。
    sector_drill_code: Option<String>,
    sector_drill_name: Option<SharedString>,
    sector_drill_quotes: Vec<crate::data::eastmoney::QuoteTick>,
    sector_drill_loading: bool,
    sector_drill_error: Option<SharedString>,
    sector_drill_gen: u64,
    /// 当前标的轻量回测报告（策略 Tab）。
    backtest_report: Option<BacktestReport>,
    /// Three-rule comparison for the current immutable candle snapshot.
    backtest_comparison: Vec<BacktestReport>,
    /// Active rule stays explicit so the UI never highlights the wrong strategy.
    backtest_active_rule: BacktestRule,
    /// 决策日记（本地 journal.json）。
    journal: Journal,
    /// 日记手写输入。
    journal_note_input: Entity<InputState>,
    /// 概览只看当前标的日记。
    journal_filter_selected: bool,
    journal_delete_confirm_id: Option<String>,
    /// 上证综指（work 模式 cpu）。
    index_sh: Option<IndexSnap>,
    /// 沪深300（work 模式 mem）。
    index_hs300: Option<IndexSnap>,
    /// 创业板指（work 模式 disk）。
    index_cyb: Option<IndexSnap>,
    /// LLM 配置（设置面板可编辑，写入 config.json）。
    ai_config: AiConfig,
    /// 底部「AI 点评」状态。
    ai_panel: AiPanelState,
    /// 当前展示的点评对应的缓存键 `code@date`（用于防止串股）。
    ai_key: Option<String>,
    /// 内存缓存：`code@date` → 点评文本 + 来源。
    ai_cache: HashMap<String, AiCacheEntry>,
    /// 使过期的 LLM 响应失效。
    ai_gen: u64,
    ai_base_url_input: Entity<InputState>,
    ai_model_input: Entity<InputState>,
    ai_api_key_input: Entity<InputState>,
    /// Optional override path/name for the local AI CLI binary.
    ai_cli_bin_input: Entity<InputState>,
    /// Price field used by the selected-symbol buy alert panel.
    alert_price_input: Entity<InputState>,
    /// Session-only capital and loss budget used by the position sizing card.
    position_capital_input: Entity<InputState>,
    position_risk_pct_input: Entity<InputState>,
    /// Persisted local buy-price alerts keyed by symbol code.
    buy_alerts: HashMap<String, BuyAlert>,
    /// 本地持仓（交易流水 + 可选现金）。
    portfolio: Portfolio,
    /// 打开中的买卖表单方向；`None` = 关闭。
    trade_form_side: Option<TradeSide>,
    trade_shares_input: Entity<InputState>,
    trade_price_input: Entity<InputState>,
    trade_fee_input: Entity<InputState>,
    trade_note_input: Entity<InputState>,
    /// 现金初始化/调整输入。
    portfolio_cash_input: Entity<InputState>,
    /// 持仓 AI 建议面板状态。
    portfolio_ai_panel: AiPanelState,
    portfolio_ai_key: Option<String>,
    portfolio_ai_cache: HashMap<String, AiCacheEntry>,
    portfolio_ai_gen: u64,
    /// macOS menu bar: show live quotes for pinned watchlist codes.
    status_bar_enabled: bool,
    /// Codes pinned to the status bar menu (subset of watchlist, max 5).
    status_bar_codes: Vec<String>,
    /// Code currently shown in the status bar title.
    status_bar_active: String,
    /// Bumps on each `schedule_persist`; only the latest gen writes disk.
    persist_gen: u64,
    _subscriptions: Vec<gpui::Subscription>,
}

impl std::ops::Deref for StockApp {
    type Target = AppState;

    fn deref(&self) -> &Self::Target {
        &self.legacy
    }
}

impl std::ops::DerefMut for StockApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.legacy
    }
}

impl StockApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cfg = storage::load_config();
        let range = ChartRange::from_label(&cfg.range);

        // Bootstrap symbols offline first, then hydrate from network.
        let symbols: Vec<Symbol> = cfg
            .watchlist
            .iter()
            .filter_map(|code| {
                let code = normalize_code(code).unwrap_or_else(|| code.trim().to_string());
                if code.is_empty() {
                    return None;
                }
                Some(Symbol {
                    code: code.clone(),
                    name: shared(code.clone()),
                    last: 0.0,
                    change_pct: 0.0,
                    volume: 0,
                    board: board_for_code(&code),
                })
            })
            .collect();

        let selected_norm = normalize_code(&cfg.selected).unwrap_or_else(|| cfg.selected.clone());
        let selected = if symbols.iter().any(|s| s.code == selected_norm) {
            shared(selected_norm)
        } else {
            symbols
                .first()
                .map(|s| shared(s.code.clone()))
                .unwrap_or_else(|| shared("600519"))
        };

        let palette_query =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索代码 / 名称，回车添加自选…"));
        let palette_focus = cx.focus_handle();
        let filtered_local: Vec<usize> = (0..symbols.len()).collect();

        let ai_cfg = cfg.ai_api.clone();
        let ai_base_url_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://api.openai.com/v1"));
        let ai_model_input = cx.new(|cx| InputState::new(window, cx).placeholder("gpt-5-mini"));
        let ai_api_key_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("sk-…").masked(true));
        let ai_cli_bin_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("可选：CLI 绝对路径，如 /opt/homebrew/bin/claude")
        });
        ai_base_url_input.update(cx, |state, cx| {
            state.set_value(ai_cfg.base_url.clone(), window, cx);
        });
        ai_model_input.update(cx, |state, cx| {
            state.set_value(ai_cfg.model.clone(), window, cx);
        });
        ai_api_key_input.update(cx, |state, cx| {
            state.set_value(ai_cfg.api_key.clone(), window, cx);
        });
        ai_cli_bin_input.update(cx, |state, cx| {
            state.set_value(ai_cfg.cli_bin.clone(), window, cx);
        });

        let portfolio = storage::load_portfolio();
        let mut journal = storage::load_journal();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        if journal.mark_due(&today) > 0
            && let Err(error) = storage::save_journal(&journal)
        {
            storage::record_storage_error(format!("更新待复盘计划失败：{error:#}"));
        }
        let trade_shares_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("股数，如 100"));
        let trade_price_input = cx.new(|cx| InputState::new(window, cx).placeholder("成交价"));
        let trade_fee_input = cx.new(|cx| InputState::new(window, cx).placeholder("手续费，可 0"));
        let trade_note_input = cx.new(|cx| InputState::new(window, cx).placeholder("备注（可选）"));
        let portfolio_cash_input = cx.new(|cx| InputState::new(window, cx).placeholder("现金余额"));
        let alert_price_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("目标价，如 12.30"));
        let position_capital_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("计划本金"));
        position_capital_input.update(cx, |state, cx| {
            state.set_value("100000", window, cx);
        });
        let position_risk_pct_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("单笔亏损 %"));
        position_risk_pct_input.update(cx, |state, cx| {
            state.set_value("1.0", window, cx);
        });
        let journal_note_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("写一句观察 / 计划，如：回踩 MA20 再看…")
        });
        portfolio_cash_input.update(cx, |state, cx| {
            let currency = Currency::for_code(selected.as_ref()).unwrap_or(Currency::Cny);
            state.set_value(
                format!("{:.2}", portfolio.cash(currency).major()),
                window,
                cx,
            );
        });

        let work_alias_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("service tag · e.g. core-db"));

        let _subscriptions = vec![
            cx.subscribe_in(&palette_query, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, window, cx| match event
                {
                    InputEvent::Change => {
                        let q = state.read(cx).value().to_string();
                        this.on_palette_query_changed(&q, cx);
                    }
                    InputEvent::PressEnter { .. } => {
                        this.palette_confirm(window, cx);
                    }
                    _ => {}
                }
            }),
            cx.subscribe_in(&ai_base_url_input, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.ai_config.base_url = state.read(cx).value().to_string();
                        this.schedule_persist(cx);
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&ai_model_input, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.ai_config.model = state.read(cx).value().to_string();
                        this.schedule_persist(cx);
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&ai_api_key_input, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.ai_config.api_key = state.read(cx).unmask_value().to_string();
                        this.schedule_persist(cx);
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&ai_cli_bin_input, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.ai_config.cli_bin = state.read(cx).value().to_string();
                        this.schedule_persist(cx);
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&work_alias_input, window, {
                move |this, _state: &Entity<InputState>, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.commit_work_alias(window, cx);
                    }
                }
            }),
            cx.subscribe_in(&position_capital_input, window, {
                move |_this, _state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&position_risk_pct_input, window, {
                move |_this, _state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                }
            }),
        ];

        let treasure_cache = storage::load_treasure_cache();
        let treasure_hits = treasure_cache.hits;
        let treasure_updated_at = treasure_cache.updated_at.clone();
        let treasure_status = if treasure_hits.is_empty() {
            shared("选「长线」→ 开始搜罗历史低位；或切「短线」跑策略雷达")
        } else {
            shared(format!(
                "低位策略缓存 {} 只 · {} · 可运行规则筛选或重扫",
                treasure_hits.len(),
                if treasure_updated_at.is_empty() {
                    "—".into()
                } else {
                    treasure_updated_at.clone()
                }
            ))
        };
        let radar_cache = storage::load_radar_cache();
        let radar_hits = radar_cache.hits;
        let radar_updated_at = radar_cache.updated_at.clone();
        let radar_status = if radar_hits.is_empty() {
            shared("选「短线」→ 一键扫描回踩/突破/超跌")
        } else {
            shared(format!(
                "短线缓存 {} 只 · {}",
                radar_hits.len(),
                if radar_updated_at.is_empty() {
                    "—".into()
                } else {
                    radar_updated_at.clone()
                }
            ))
        };
        let watch_tags: HashMap<String, WatchTag> = cfg
            .watch_tags
            .iter()
            .map(|(k, v)| (k.clone(), WatchTag::from_id(v)))
            .collect();

        let mut dock = cfg.dock.clone();
        // 旧配置回退：只有宽/高向量为空时才用 legacy 字段。
        if dock.main_h.is_empty() {
            dock.main_h = vec![cfg.left_width];
        }
        if dock.main_v.is_empty() {
            dock.main_v = vec![cfg.bottom_height];
        }
        let window_bounds = dock.window;

        let watchlist_codes: Vec<String> = symbols.iter().map(|s| s.code.clone()).collect();
        let (status_bar_enabled, status_bar_codes, status_bar_active) = normalize_status_bar(
            cfg.status_bar_enabled,
            &cfg.status_bar_codes,
            &cfg.status_bar_active,
            &watchlist_codes,
        );

        let mut app = Self {
            services: AppServices::default(),
            market_state: MarketState::default(),
            chart_state: ChartState::default(),
            discovery_state: DiscoveryState::default(),
            portfolio_state: PortfolioState {
                selected_currency: Currency::for_code(selected.as_ref()),
            },
            analysis_state: AnalysisState::default(),
            ui_state: UiState::default(),
            runtime_state: RuntimeState::default(),
            strategy_lab_feature: StrategyLabFeature::new(),
            #[cfg(feature = "work-mode")]
            work_mode_feature: WorkModeFeature::default(),
            legacy: AppState {
                symbols,
                selected,
                candles_code: None,
                kline_gen: 0,
                candles: Vec::new(),
                signal_cache: None,
                levels_cache: None,
                ma: MaSeries::default(),
                range,
                chart_kind: ChartKind::from_label(&cfg.chart_kind),
                minute: None,
                minute_code: None,
                minute_gen: 0,
                show_ma5: cfg.show_ma5,
                show_ma10: cfg.show_ma10,
                show_ma20: cfg.show_ma20,
                show_ma60: cfg.show_ma60,
                show_volume: cfg.show_volume,
                show_macd: cfg.show_macd,
                show_boll: cfg.show_boll,
                macd: MacdSeries::default(),
                boll: BollSeries::default(),
                hover_ix: None,
                chart_view_start: 0,
                chart_view_count: 0,
                chart_width: 800.0,
                chart_origin: Point::default(),
                chart_bounds: Bounds::default(),
                drawing_mode: false,
                drawing_anchor: None,
                draft_line: None,
                chart_lines: cfg.chart_lines.clone(),
                draw_color_ix: 0,
                series_cache: series_cache::SeriesCache::new(),
                status: shared("正在连接行情源…"),
                loading: true,
                refreshing: false,
                data_source: shared(market_data::SRC_LABEL),
                palette_open: false,
                palette_index: 0,
                settings_open: false,
                market_analysis_open: false,
                market_analysis_region: MarketRegion::AShare,
                market_analysis_sectors: Vec::new(),
                market_heatmap_sectors: Arc::new(Vec::new()),
                market_heatmap_list: false,
                market_heatmap_fullscreen: false,
                market_heatmap_loading: false,
                market_heatmap_error: None,
                market_analysis_loading: false,
                market_analysis_error: None,
                market_analysis_source: shared(market_data::SRC_EASTMONEY),
                market_analysis_updated: None,
                market_analysis_gen: 0,
                market_ai_panel: AiPanelState::Idle,
                market_ai_picks: Vec::new(),
                market_ai_gen: 0,
                settings_section: SettingsSection::General,
                update_state: UpdateState::Idle,
                quote_interval_secs: clamp_quote_interval_secs(cfg.quote_interval_secs),
                palette_query,
                palette_focus,
                palette_hits: Vec::new(),
                filtered_local,
                left_width: cfg.left_width,
                bottom_height: cfg.bottom_height,
                dock,
                window_bounds,
                color_scheme: cfg.color_scheme,
                work_mode: cfg.work_mode,
                work_density: cfg.work_density,
                work_right_width: cfg.work_right_width,
                work_restore_bounds: None,
                work_identity_reveal: false,
                work_identity_peek_held: false,
                work_identity_map_latched: false,
                work_identity_hide_gen: 0,
                work_aliases: cfg.work_aliases.clone(),
                work_alias_editing: false,
                work_alias_input,
                watchlist_sort: cfg.watchlist_sort,
                quote_fail_streak: 0,
                left_tab: LeftTab::from_label(&cfg.left_tab),
                detail_tab: DetailTab::from_label(&cfg.detail_tab),
                treasure_hits,
                treasure_pool: TreasurePool::from_id(&cfg.treasure_pool),
                treasure_fin: FinFilter::from_id(&cfg.treasure_fin),
                treasure_scanning: false,
                treasure_scan_silent: false,
                treasure_done: 0,
                treasure_total: 0,
                treasure_status,
                treasure_gen: 0,
                scout_picks: Vec::new(),
                scout_summary: shared(""),
                scout_running: false,
                scout_silent: false,
                scout_done: 0,
                scout_total: 0,
                scout_gen: 0,
                scout_source: shared(""),
                // 默认只看「可关注」；若本轮为零会自动回退到「全部」。
                scout_only_buy_watch: true,
                treasure_list_expanded: false,
                find_mode: FindMode::from_id(&cfg.find_mode),
                treasure_updated_at,
                radar_hits,
                radar_updated_at,
                radar_scanning: false,
                radar_done: 0,
                radar_total: 0,
                radar_status,
                radar_summary: shared(""),
                radar_gen: 0,
                radar_filter: None,
                watch_tags,
                watch_filter: WatchTag::from_id(&cfg.watch_filter),
                sector_drill_code: None,
                sector_drill_name: None,
                sector_drill_quotes: Vec::new(),
                sector_drill_loading: false,
                sector_drill_error: None,
                sector_drill_gen: 0,
                backtest_report: None,
                backtest_comparison: Vec::new(),
                backtest_active_rule: BacktestRule::Ma20CrossUp,
                journal,
                journal_note_input,
                journal_filter_selected: true,
                journal_delete_confirm_id: None,
                index_sh: None,
                index_hs300: None,
                index_cyb: None,
                ai_config: ai_cfg,
                ai_panel: AiPanelState::Idle,
                ai_key: None,
                ai_cache: HashMap::new(),
                ai_gen: 0,
                ai_base_url_input,
                ai_model_input,
                ai_api_key_input,
                ai_cli_bin_input,
                alert_price_input,
                position_capital_input,
                position_risk_pct_input,
                buy_alerts: cfg.buy_alerts.clone(),
                portfolio,
                trade_form_side: None,
                trade_shares_input,
                trade_price_input,
                trade_fee_input,
                trade_note_input,
                portfolio_cash_input,
                portfolio_ai_panel: AiPanelState::Idle,
                portfolio_ai_key: None,
                portfolio_ai_cache: HashMap::new(),
                portfolio_ai_gen: 0,
                status_bar_enabled,
                status_bar_codes,
                status_bar_active,
                persist_gen: 0,
                _subscriptions,
            },
        };

        // Normal launches begin with the action-oriented Today dashboard. Work
        // mode keeps the persisted sidebar identity because it renders its own
        // full-page dashboard.
        app.ui_state.primary_task = if app.work_mode {
            match app.left_tab {
                LeftTab::Portfolio => state::PrimaryTask::Portfolio,
                LeftTab::Treasure => state::PrimaryTask::Opportunities,
                LeftTab::Watchlist => state::PrimaryTask::Research,
            }
        } else {
            state::PrimaryTask::Today
        };

        // 历史持仓代码自动并入自选，便于行情轮询。
        let before = app.symbols.len();
        let open_codes: Vec<(String, String)> = app
            .portfolio
            .positions()
            .into_iter()
            .map(|p| (p.code, p.name))
            .collect();
        for (code, name) in open_codes {
            app.ensure_in_watchlist(&code, &name, 0.0);
        }
        if app.symbols.len() != before {
            app.persist();
        }

        if let Some(error) = storage::take_storage_error() {
            app.status = shared(error);
        }

        window.set_window_title(app.window_title());
        app.bootstrap(cx);
        app.strategy_lab_start_daily_observation(cx);
        // 盘后 / 缓存过期：静默预扫长线，打开就能用。
        app.maybe_background_rescan(cx);
        app
    }
}

impl Render for StockApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let render_started = Instant::now();
        // Persist the window frame whenever it changes (complete dock serialization).
        if !window.is_fullscreen() {
            let b = window.bounds();
            let cur = (
                b.origin.x.as_f32(),
                b.origin.y.as_f32(),
                b.size.width.as_f32(),
                b.size.height.as_f32(),
            );
            if self.window_bounds != Some(cur) {
                self.window_bounds = Some(cur);
                self.schedule_persist(cx);
            }
        }

        let left_w = self.dock.main_h.first().copied().unwrap_or(self.left_width);
        let center_w = self.dock.main_h.get(1).copied().unwrap_or(0.0);
        let bottom_h = self
            .dock
            .main_v
            .get(1)
            .copied()
            .unwrap_or(self.bottom_height);
        let work = self.work_mode;

        let view = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .track_focus(&self.palette_focus)
            .key_context("stock")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _w, cx| {
                if this.handle_work_peek_key_down(event, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _w, cx| {
                if this.handle_work_peek_key_up(event, cx) {
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, window, cx| {
                this.toggle_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RefreshData, _w, cx| {
                this.refresh_all(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectTodayTask, _w, cx| {
                this.set_primary_task(state::PrimaryTask::Today, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectResearchTask, _w, cx| {
                this.set_primary_task(state::PrimaryTask::Research, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectOpportunitiesTask, _w, cx| {
                this.set_primary_task(state::PrimaryTask::Opportunities, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectPortfolioTask, _w, cx| {
                this.set_primary_task(state::PrimaryTask::Portfolio, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleTreasure, _w, cx| {
                // Treasure UI is hidden in work mode; ignore hotkey there.
                if !this.work_mode {
                    this.toggle_treasure_tab(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleWorkMode, window, cx| {
                this.toggle_work_mode(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSettings, _w, cx| {
                this.toggle_settings(cx);
            }))
            .on_action(cx.listener(|this, _: &DismissOverlay, _w, cx| {
                this.dismiss_overlay(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectPrevSymbol, _w, cx| {
                if this.settings_open {
                    return;
                }
                if this.palette_open {
                    this.palette_move(-1, cx);
                } else {
                    this.select_adjacent_symbol(-1, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectNextSymbol, _w, cx| {
                if this.settings_open {
                    return;
                }
                if this.palette_open {
                    this.palette_move(1, cx);
                } else {
                    this.select_adjacent_symbol(1, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RemoveSelectedSymbol, _w, cx| {
                if !this.palette_open && !this.settings_open && this.left_tab == LeftTab::Watchlist
                {
                    this.remove_selected_from_watchlist(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ResetChartZoom, _w, cx| {
                this.reset_chart_view();
                this.hover_ix = None;
                cx.notify();
            }))
            // Explicit height so the bar always reserves space (avoids list under traffic lights).
            .child(
                div()
                    .w_full()
                    .h(TITLE_BAR_HEIGHT)
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(self.render_title_bar(cx)),
            )
            .child(if self.settings_open {
                // Full-page settings: no modal overlay.
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_settings(window, cx))
                    .into_any_element()
            } else if self.market_analysis_open {
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_market_analysis(window, cx))
                    .into_any_element()
            } else if self.ui_state.primary_task == state::PrimaryTask::Today && !work {
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_today_dashboard(cx))
                    .into_any_element()
            } else if self.ui_state.primary_task == state::PrimaryTask::StrategyLab {
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_strategy_lab(window, cx))
                    .into_any_element()
            } else if work {
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.render_work_dashboard(cx))
                    .into_any_element()
            } else {
                let entity_h = cx.entity().clone();
                let entity_v = cx.entity().clone();
                // Same definite-height trick as the left sidebar: resizable panels
                // center their children when height is unresolved, which left a
                // black band above the chart quote header.
                let avail_h = (window.bounds().size.height - TITLE_BAR_HEIGHT).max(px(0.));
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(
                        h_resizable("main-h")
                            .on_resize(move |state, _window, cx| {
                                entity_h.update(cx, |this, cx| {
                                    this.on_main_h_resize(state, cx);
                                });
                            })
                            .child(
                                resizable_panel()
                                    .size(px(left_w))
                                    .size_range(px(200.)..px(440.))
                                    .child(self.render_left_panel(window, cx)),
                            )
                            .child(
                                resizable_panel()
                                    .when(center_w > 0.0, |p| p.size(px(center_w)))
                                    .child(
                                        // Pin the right column to the full content height so
                                        // the chart header aligns with the sidebar top.
                                        v_flex()
                                            .size_full()
                                            .h(avail_h)
                                            .min_h_0()
                                            .overflow_hidden()
                                            .debug_selector(|| "right-panel-root".into())
                                            .child(
                                                v_resizable("main-v")
                                                    .on_resize(move |state, _window, cx| {
                                                        entity_v.update(cx, |this, cx| {
                                                            this.on_main_v_resize(state, cx);
                                                        });
                                                    })
                                                    .child(
                                                        resizable_panel()
                                                            .child(self.render_chart_area(cx)),
                                                    )
                                                    .child(
                                                        resizable_panel()
                                                            // Slightly shorter default so overview
                                                            // isn't floating in empty space.
                                                            .size(px(bottom_h.max(148.0)))
                                                            .size_range(px(128.)..px(420.))
                                                            .child(self.render_detail_panel(cx)),
                                                    ),
                                            ),
                                    ),
                            ),
                    )
                    .into_any_element()
            })
            .when(
                self.palette_open && !self.settings_open && !self.market_analysis_open,
                |this| this.child(self.render_palette(cx)),
            )
            .children(Root::render_dialog_layer(window, cx));
        self.runtime_state
            .performance
            .record_ui_build(render_started.elapsed().as_secs_f64() * 1_000.0);
        if let Some(started) = self.runtime_state.performance.take_navigation_started() {
            let entity = cx.entity().clone();
            window.on_next_frame(move |_, cx| {
                entity.update(cx, |app, _| {
                    app.runtime_state
                        .performance
                        .record_navigation(started.elapsed().as_secs_f64() * 1_000.0);
                });
            });
            window.request_animation_frame();
        }
        self.runtime_state
            .performance
            .record_first_interactive(crate::services::performance::process_elapsed_ms());
        view
    }
}

/// Open the one main application window.
///
/// macOS keeps the application process alive after the last window is closed.
/// Keeping this in one function lets both the initial launch and the Dock
/// "reopen" event create the same window instead of leaving the app with only
/// a running Dock icon.
fn open_main_window(cx: &mut App) {
    let cfg = storage::load_config();
    let window_bounds = match cfg.dock.window {
        // Allow Mini focus footprint (~720×440) to restore across restarts.
        Some((x, y, w, h)) if w >= 640.0 && h >= 400.0 => WindowBounds::Windowed(Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }),
        _ => WindowBounds::centered(size(px(1320.), px(860.)), cx),
    };
    let window_options = WindowOptions {
        titlebar: Some(TitleBar::title_bar_options()),
        window_bounds: Some(window_bounds),
        ..Default::default()
    };

    cx.open_window(window_options, |window, cx| {
        window.activate_window();
        Theme::change(ThemeMode::Dark, Some(window), cx);
        apply_zstock_theme(cx);
        window.refresh();
        // Title is set inside StockApp::new (respects persisted work_mode).
        let view = cx.new(|cx| StockApp::new(window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .expect("Failed to open window");
}

/// A calmer, higher-contrast visual system for dense market information.
///
/// The upstream dark theme is nearly black, which flattens cards, navigation,
/// and charts into one surface. ZStock uses a blue-slate base so users can
/// distinguish application chrome, working surfaces, and selected content at
/// a glance without introducing decorative gradients or extra chrome.
fn apply_zstock_theme(cx: &mut App) {
    let theme = Theme::global_mut(cx);

    theme.font_size = px(15.);
    theme.radius = px(8.);
    theme.radius_lg = px(12.);
    theme.shadow = false;

    theme.background = hsla(0.61, 0.30, 0.075, 1.0);
    theme.sidebar = hsla(0.60, 0.28, 0.105, 1.0);
    theme.title_bar = hsla(0.61, 0.32, 0.070, 1.0);
    theme.title_bar_border = hsla(0.59, 0.22, 0.19, 1.0);
    theme.popover = hsla(0.60, 0.30, 0.12, 1.0);
    theme.popover_foreground = hsla(0.58, 0.24, 0.93, 1.0);
    theme.foreground = hsla(0.58, 0.24, 0.93, 1.0);
    theme.muted = hsla(0.60, 0.22, 0.16, 1.0);
    theme.muted_foreground = hsla(0.58, 0.14, 0.64, 1.0);
    theme.border = hsla(0.59, 0.22, 0.20, 1.0);
    theme.input = hsla(0.59, 0.22, 0.24, 1.0);

    theme.accent = hsla(0.57, 0.84, 0.62, 1.0);
    theme.accent_foreground = hsla(0.61, 0.34, 0.09, 1.0);
    theme.primary = hsla(0.57, 0.78, 0.57, 1.0);
    theme.primary_hover = hsla(0.57, 0.82, 0.63, 1.0);
    theme.primary_active = hsla(0.57, 0.72, 0.52, 1.0);
    theme.primary_foreground = hsla(0.61, 0.34, 0.08, 1.0);
    theme.ring = theme.accent.opacity(0.72);
    theme.selection = theme.accent.opacity(0.24);

    theme.list = theme.sidebar;
    theme.list_head = hsla(0.60, 0.25, 0.13, 1.0);
    theme.list_even = hsla(0.60, 0.24, 0.115, 1.0);
    theme.list_hover = theme.accent.opacity(0.09);
    theme.list_active = theme.accent.opacity(0.15);
    theme.list_active_border = theme.accent.opacity(0.72);
    theme.tab_bar = theme.sidebar;
    theme.tab = theme.sidebar;
    theme.tab_active = theme.accent.opacity(0.14);
    theme.tab_active_foreground = theme.foreground;
    theme.tab_foreground = theme.muted_foreground;

    theme.success = hsla(0.45, 0.62, 0.55, 1.0);
    theme.warning = hsla(0.11, 0.86, 0.64, 1.0);
    theme.danger = hsla(0.985, 0.78, 0.66, 1.0);
    theme.red = hsla(0.985, 0.78, 0.66, 1.0);
    theme.green = hsla(0.45, 0.62, 0.55, 1.0);
    theme.blue = hsla(0.57, 0.84, 0.62, 1.0);
    theme.yellow = hsla(0.11, 0.86, 0.64, 1.0);
    theme.cyan = hsla(0.51, 0.68, 0.59, 1.0);
    theme.magenta = hsla(0.83, 0.64, 0.68, 1.0);

    theme.scrollbar = theme.background.opacity(0.45);
    theme.scrollbar_thumb = theme.muted_foreground.opacity(0.28);
    theme.scrollbar_thumb_hover = theme.muted_foreground.opacity(0.46);
}

pub fn run() {
    let app = gpui::Application::new();

    // macOS calls this when the running app's Dock icon is clicked after
    // its last window was closed. Without it, the process stays alive but
    // Mission Control correctly reports that there are no windows.
    app.on_reopen(|cx| {
        cx.activate(true);
        let windows = cx.windows();
        if windows.is_empty() {
            open_main_window(cx);
        } else {
            // Also recover gracefully if the only window is minimized or
            // hidden rather than creating a duplicate window.
            for handle in windows {
                let _ = handle.update(cx, |_root, window, _cx| {
                    window.activate_window();
                });
            }
        }
    });

    app.run(move |cx| {
        gpui_component::init(cx);

        cx.bind_keys([
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-k", ToggleCommandPalette, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-k", ToggleCommandPalette, None),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-p", ToggleCommandPalette, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-p", ToggleCommandPalette, None),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-r", RefreshData, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-r", RefreshData, None),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-1", SelectTodayTask, Some("stock && !Input")),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-1", SelectTodayTask, Some("stock && !Input")),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-2", SelectResearchTask, Some("stock && !Input")),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-2", SelectResearchTask, Some("stock && !Input")),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-3", SelectOpportunitiesTask, Some("stock && !Input")),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-3", SelectOpportunitiesTask, Some("stock && !Input")),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-4", SelectPortfolioTask, Some("stock && !Input")),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-4", SelectPortfolioTask, Some("stock && !Input")),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-t", ToggleTreasure, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-t", ToggleTreasure, None),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-shift-w", ToggleWorkMode, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-shift-w", ToggleWorkMode, None),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-,", ToggleSettings, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("ctrl-,", ToggleSettings, None),
            KeyBinding::new("escape", DismissOverlay, None),
            // `stock && !Input`：根节点始终带 `stock` 上下文；输入框聚焦时叠加
            // `Input` 上下文并自动禁用这些纯按键绑定，避免吞掉输入框的
            // 退格/删除/数字/字母/方向键。输入框有独立的 Input 上下文绑定。
            KeyBinding::new("up", SelectPrevSymbol, Some("stock && !Input")),
            KeyBinding::new("down", SelectNextSymbol, Some("stock && !Input")),
            KeyBinding::new("k", SelectPrevSymbol, Some("stock && !Input")),
            KeyBinding::new("j", SelectNextSymbol, Some("stock && !Input")),
            KeyBinding::new("backspace", RemoveSelectedSymbol, Some("stock && !Input")),
            KeyBinding::new("delete", RemoveSelectedSymbol, Some("stock && !Input")),
            KeyBinding::new("0", ResetChartZoom, Some("stock && !Input")),
            #[cfg(target_os = "macos")]
            KeyBinding::new("cmd-q", Quit, None),
            #[cfg(not(target_os = "macos"))]
            KeyBinding::new("alt-f4", Quit, None),
        ]);

        cx.on_action(|_: &Quit, cx: &mut App| {
            cx.quit();
        });

        open_main_window(cx);
    });
}

#[cfg(test)]
mod keymap_tests {
    use gpui::{KeyBinding, KeyContext, Keymap, Keystroke, actions};

    actions!(keymap_test, [InputBackspace, AppBackspace, AppZero]);

    fn keystrokes(s: &str) -> Vec<Keystroke> {
        vec![Keystroke::parse(s).unwrap()]
    }

    /// 应用内的纯按键快捷键必须让位于聚焦的输入框（`!Input` 上下文）。
    /// 注册顺序与运行时一致：组件输入框绑定在前，应用快捷键在后。
    #[test]
    fn plain_key_bindings_yield_to_focused_input() {
        let km = Keymap::new(vec![
            KeyBinding::new("backspace", InputBackspace, Some("Input")),
            KeyBinding::new("backspace", AppBackspace, Some("stock && !Input")),
            KeyBinding::new("0", AppZero, Some("stock && !Input")),
        ]);
        // 根节点带 `stock` 上下文；输入框聚焦时叠加 `Input`。
        let input_focused = [
            KeyContext::parse("Input").unwrap(),
            KeyContext::parse("stock").unwrap(),
        ];
        let no_input = [KeyContext::parse("stock").unwrap()];

        // 输入框聚焦：退格走输入框自己的绑定，应用绑定让位。
        let (bindings, _) = km.bindings_for_input(&keystrokes("backspace"), &input_focused);
        assert_eq!(bindings.len(), 1, "backspace while input focused");
        assert!(
            bindings[0]
                .action()
                .as_any()
                .downcast_ref::<InputBackspace>()
                .is_some(),
            "input binding must win, got {}",
            bindings[0].action().name()
        );

        // 输入框聚焦：`0` 不再触发应用动作，按键落到文本输入。
        let (bindings, _) = km.bindings_for_input(&keystrokes("0"), &input_focused);
        assert!(
            bindings.is_empty(),
            "0 binding must yield while input focused"
        );

        // 无输入框聚焦：应用快捷键照常生效。
        let (bindings, _) = km.bindings_for_input(&keystrokes("backspace"), &no_input);
        assert_eq!(bindings.len(), 1);
        assert!(
            bindings[0]
                .action()
                .as_any()
                .downcast_ref::<AppBackspace>()
                .is_some(),
            "app binding must win without input focus, got {}",
            bindings[0].action().name()
        );

        let (bindings, _) = km.bindings_for_input(&keystrokes("0"), &no_input);
        assert_eq!(bindings.len(), 1);
        assert!(
            bindings[0]
                .action()
                .as_any()
                .downcast_ref::<AppZero>()
                .is_some(),
            "app 0 binding must win without input focus"
        );
    }
}

#[cfg(test)]
mod layout_regression_tests {
    use std::sync::Arc;

    use super::StockApp;
    use gpui::{
        AnyWindowHandle, AppContext, TestAppContext, VisualContext, VisualTestContext, px, size,
    };
    use gpui_component::PixelsExt;

    use crate::data::eastmoney::{
        IndustryHeatmapSector, IndustryStockGroup, QuoteTick, SectorTick,
    };
    use crate::domain::market::{Availability, Freshness};
    use crate::domain::money::Currency;

    fn full_heatmap_fixture() -> Arc<Vec<IndustryHeatmapSector>> {
        let mut next_code = 600_000u32;
        let mut sectors = Vec::with_capacity(31);
        for sector_index in 0..31 {
            let mut industries = Vec::with_capacity(4);
            for industry_index in 0..4 {
                let mut stocks = Vec::with_capacity(44);
                for stock_index in 0..44 {
                    let code = format!("{next_code:06}");
                    next_code += 1;
                    let amount = f64::from(45 - stock_index) * 1_000_000.0;
                    stocks.push(QuoteTick {
                        code,
                        name: format!("个股{sector_index:02}{industry_index}{stock_index:02}"),
                        last: 10.0 + f64::from(stock_index),
                        change_pct: f64::from(stock_index % 9 - 4) * 0.8,
                        volume: 10_000,
                        amount,
                        currency: Currency::Cny,
                        source: "fixture".into(),
                        fetched_at: 0,
                        market_time: None,
                        availability: Availability::Available,
                        freshness: Freshness::Live,
                    });
                }
                industries.push(IndustryStockGroup {
                    name: format!("二级行业{sector_index:02}-{industry_index}"),
                    amount: stocks.iter().map(|stock| stock.amount).sum(),
                    stocks,
                });
            }
            sectors.push(IndustryHeatmapSector {
                sector: SectorTick {
                    code: format!("BK{sector_index:04}"),
                    name: format!("一级行业{sector_index:02}"),
                    change_pct: 0.0,
                    amount: industries.iter().map(|industry| industry.amount).sum(),
                    advances: 0,
                    declines: 0,
                    unchanged: 0,
                },
                industries,
            });
        }
        Arc::new(sectors)
    }

    /// Shared window/App setup for layout regression tests: isolated HOME and a
    /// deterministic default config (work mode off, fixed dock) so results do
    /// not depend on the developer's real `config.json`.
    fn test_window(cx: &mut TestAppContext, w: f32, h: f32) -> VisualTestContext {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = std::env::temp_dir().join(format!(
            "stock-analysis-test-{}-{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&tmp).expect("create temp home");
        unsafe {
            std::env::set_var("HOME", &tmp);
        }
        // Resolve config dir after HOME is set (macOS/Linux paths differ).
        let cfg_dir = dirs::data_dir().expect("data_dir").join("stock-analysis");
        std::fs::create_dir_all(&cfg_dir).expect("create temp config dir");
        let cfg = crate::storage::AppConfig::default();
        let json = serde_json::to_string_pretty(&cfg).expect("serialize test config");
        std::fs::write(cfg_dir.join("config.json"), json).expect("write test config");

        cx.update(gpui_component::init);
        let window = cx.update(|cx| {
            cx.open_window(
                gpui::WindowOptions {
                    titlebar: Some(gpui_component::TitleBar::title_bar_options()),
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                        origin: gpui::point(px(75.), px(47.)),
                        size: size(px(w), px(h)),
                    })),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let mut app = StockApp::new(window, cx);
                        // These regression cases exercise the research split
                        // layout explicitly. Normal launches now begin on the
                        // task-oriented Today dashboard.
                        app.ui_state.primary_task = super::state::PrimaryTask::Research;
                        app
                    })
                },
            )
            .expect("open window")
        });
        let window: AnyWindowHandle = window.into();
        VisualTestContext::from_window(window, cx)
    }

    /// Regression test: opening the window directly at its persisted size must
    /// not leave the left sidebar collapsed to content height and centered
    /// (gpui-component's resizable group centers its panels, and percentage
    /// heights are not resolved on the very first layout).
    #[gpui::test]
    fn sidebar_fills_height_on_first_layout(cx: &mut TestAppContext) {
        let mut window = test_window(cx, 1320.0, 860.0);
        window.run_until_parked();

        let root = window
            .debug_bounds("left-panel-root")
            .expect("left-panel-root bounds");
        eprintln!("left-panel-root bounds: {root:?}");
        assert!(
            (root.origin.y.as_f32() - 34.0).abs() < 1.0,
            "sidebar should start right below the 34px title bar, got y={}",
            root.origin.y.as_f32()
        );
        assert!(
            (root.size.height.as_f32() - 826.0).abs() < 1.0,
            "sidebar should span the full content height (826px), got {}",
            root.size.height.as_f32()
        );
    }

    #[gpui::test]
    fn today_dashboard_fills_content_area(cx: &mut TestAppContext) {
        let mut window = test_window(cx, 1320.0, 860.0);
        let handle = window.window_handle();
        let update_result = window.cx.update_window(handle, |view, _window, cx| {
            view.downcast::<StockApp>()
                .expect("window root view")
                .update(cx, |this, cx| {
                    this.ui_state.primary_task = super::state::PrimaryTask::Today;
                    this.market_analysis_open = false;
                    cx.notify();
                });
        });
        assert!(update_result.is_ok(), "today update should succeed");
        window.run_until_parked();
        window.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let root = window
            .debug_bounds("today-dashboard-root")
            .expect("today-dashboard-root bounds");
        assert!(
            (root.origin.y.as_f32() - 34.0).abs() < 1.0,
            "today dashboard should start below the title bar, got y={}",
            root.origin.y.as_f32()
        );
        assert!(
            (root.size.height.as_f32() - 826.0).abs() < 1.0,
            "today dashboard should fill the content area, got {}",
            root.size.height.as_f32()
        );
    }

    /// Regression test: chart quote header must sit flush under the title bar
    /// (same y as the left sidebar), not centered with a black band above it.
    #[gpui::test]
    fn chart_header_aligns_with_sidebar_top(cx: &mut TestAppContext) {
        let mut window = test_window(cx, 1320.0, 860.0);
        window.run_until_parked();
        window.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let left = window
            .debug_bounds("left-panel-root")
            .expect("left-panel-root bounds");
        let right = window
            .debug_bounds("right-panel-root")
            .expect("right-panel-root bounds");
        let header = window
            .debug_bounds("chart-quote-header")
            .expect("chart-quote-header bounds");
        eprintln!("left={left:?} right={right:?} header={header:?}");

        assert!(
            (right.origin.y.as_f32() - 34.0).abs() < 1.0,
            "right column should start under title bar, got y={}",
            right.origin.y.as_f32()
        );
        assert!(
            (header.origin.y.as_f32() - 34.0).abs() < 1.0,
            "chart quote header should be flush under title bar (no top band), got y={}",
            header.origin.y.as_f32()
        );
        assert!(
            (left.origin.y.as_f32() - header.origin.y.as_f32()).abs() < 1.0,
            "chart header and sidebar tops should align, left y={} header y={}",
            left.origin.y.as_f32(),
            header.origin.y.as_f32()
        );
    }

    /// Regression test: settings is a full page under the title bar (not a modal).
    #[gpui::test]
    fn settings_page_fills_content_area(cx: &mut TestAppContext) {
        let mut window = test_window(cx, 1320.0, 860.0);
        window.run_until_parked();
        let handle = window.window_handle();
        let update_result = window.cx.update_window(handle, |view, _window, cx| {
            view.downcast::<StockApp>()
                .expect("window root view")
                .update(cx, |this, cx| this.toggle_settings(cx));
        });
        assert!(update_result.is_ok(), "settings update should succeed");
        window.run_until_parked();
        window.update(|window, cx| {
            let _arena_clear_needed = window.draw(cx);
        });

        let panel = window
            .debug_bounds("settings-panel-root")
            .expect("settings-panel-root bounds");
        eprintln!("settings page bounds (1320x860): {panel:?}");
        // Below 34px title bar, full remaining height.
        assert!(
            (panel.origin.y.as_f32() - 34.0).abs() < 1.0,
            "settings page should start below title bar, got y={}",
            panel.origin.y.as_f32()
        );
        assert!(
            (panel.size.height.as_f32() - 826.0).abs() < 1.0,
            "settings page should fill content height 826, got {}",
            panel.size.height.as_f32()
        );
        assert!(
            (panel.size.width.as_f32() - 1320.0).abs() < 1.0,
            "settings page should be full window width, got {}",
            panel.size.width.as_f32()
        );

        window.simulate_resize(size(px(640.), px(600.)));
        window.run_until_parked();
        let panel = window
            .debug_bounds("settings-panel-root")
            .expect("settings-panel-root bounds");
        eprintln!("settings page bounds (640x600): {panel:?}");
        assert!(
            (panel.size.width.as_f32() - 640.0).abs() < 1.0,
            "settings page should match window width 640, got {}",
            panel.size.width.as_f32()
        );
        assert!(
            (panel.size.height.as_f32() - 566.0).abs() < 1.0,
            "settings page should fill 600-34 height, got {}",
            panel.size.height.as_f32()
        );
    }

    /// The normal heatmap uses the full content width and preserves its useful
    /// reference aspect ratio on wide windows. Focus mode then consumes the
    /// available market-analysis viewport instead of leaving the dashboard
    /// visible around it.
    #[gpui::test]
    fn market_heatmap_expands_in_fullscreen_mode(cx: &mut TestAppContext) {
        let mut window = test_window(cx, 1600.0, 900.0);
        window.run_until_parked();
        let handle = window.window_handle();
        let update_result = window.cx.update_window(handle, |view, _window, cx| {
            view.downcast::<StockApp>()
                .expect("window root view")
                .update(cx, |this, cx| {
                    this.market_analysis_open = true;
                    this.market_heatmap_sectors = full_heatmap_fixture();
                    this.market_heatmap_loading = false;
                    cx.notify();
                });
        });
        assert!(
            update_result.is_ok(),
            "market analysis update should succeed"
        );
        window.run_until_parked();
        window.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let normal = window
            .debug_bounds("market-sector-heatmap")
            .expect("normal heatmap bounds");
        let page = window
            .debug_bounds("market-analysis-page-root")
            .expect("market analysis page bounds");
        assert!(
            normal.size.width.as_f32() > 1_500.0,
            "normal heatmap should use the wide viewport, got {}",
            normal.size.width.as_f32()
        );
        let right_inset =
            page.size.width.as_f32() - normal.origin.x.as_f32() - normal.size.width.as_f32();
        assert!(
            right_inset < 50.0,
            "normal heatmap should not leave a wide blank gutter, got {right_inset}"
        );
        let normal_aspect = normal.size.width.as_f32() / normal.size.height.as_f32();
        let reference_aspect = 1248.0 / 440.0;
        assert!(
            (normal_aspect - reference_aspect).abs() < 0.02,
            "normal heatmap should preserve its readable aspect ratio, got {normal_aspect}"
        );
        let first_stock = window
            .debug_bounds("market-heatmap-first-stock")
            .expect("first stock tile bounds");
        assert!(first_stock.size.width.as_f32() > 0.0);
        assert!(first_stock.size.height.as_f32() > 0.0);

        let update_result = window.cx.update_window(handle, |view, _window, cx| {
            view.downcast::<StockApp>()
                .expect("window root view")
                .update(cx, |this, cx| {
                    this.toggle_market_heatmap_fullscreen(cx);
                });
        });
        assert!(update_result.is_ok(), "fullscreen update should succeed");
        window.run_until_parked();
        window.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let focus_page = window
            .debug_bounds("market-analysis-heatmap-fullscreen")
            .expect("fullscreen heatmap page bounds");
        let expanded = window
            .debug_bounds("market-sector-heatmap")
            .expect("fullscreen heatmap bounds");
        assert!(
            (focus_page.size.height.as_f32() - 866.0).abs() < 1.0,
            "fullscreen page should fill the content area, got {}",
            focus_page.size.height.as_f32()
        );
        assert!(
            expanded.size.height.as_f32() > normal.size.height.as_f32() + 180.0,
            "fullscreen heatmap should grow vertically: normal={}, expanded={}",
            normal.size.height.as_f32(),
            expanded.size.height.as_f32()
        );
        assert!(
            expanded.size.width.as_f32() >= normal.size.width.as_f32(),
            "fullscreen heatmap should retain the full viewport width: normal={}, expanded={}",
            normal.size.width.as_f32(),
            expanded.size.width.as_f32()
        );
    }

    /// Regression test: market analysis is commonly opened from Today. Picking
    /// a stock from its heatmap must reveal that stock in Research instead of
    /// closing the overlay back onto the Today dashboard.
    #[gpui::test]
    fn heatmap_stock_selection_opens_research(cx: &mut TestAppContext) {
        let mut window = test_window(cx, 1320.0, 860.0);
        let handle = window.window_handle();
        let update_result = window.cx.update_window(handle, |view, _window, cx| {
            view.downcast::<StockApp>()
                .expect("window root view")
                .update(cx, |this, cx| {
                    this.ui_state.primary_task = super::state::PrimaryTask::Today;
                    this.market_analysis_open = true;
                    this.market_heatmap_fullscreen = true;

                    this.select_sector_constituent("600000".into(), "浦发银行".into(), 10.25, cx);

                    assert_eq!(
                        this.ui_state.primary_task,
                        super::state::PrimaryTask::Research
                    );
                    assert!(!this.market_analysis_open);
                    assert!(!this.market_heatmap_fullscreen);
                    assert_eq!(this.selected.as_ref(), "600000");
                });
        });
        assert!(update_result.is_ok(), "heatmap selection should succeed");
    }
}
