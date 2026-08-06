//! Root application: A 股 / 港股 watchlist, chart (MA + crosshair), resizable layout, persistence.
//!
//! Split across submodules by concern:
//! - [`types`] — enums / small state types
//! - [`market`] / [`portfolio`] / [`symbols`] / [`prefs`] / [`chart_ctrl`] — logic
//! - [`ui`] — render methods
//! - [`helpers`] — pure formatting helpers

mod chart_ctrl;
mod helpers;
mod labels;
mod market;
mod portfolio;
mod prefs;
mod series_cache;
mod symbols;
mod types;
mod ui;

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    actions, div, point, px, size, App, AppContext, Bounds, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, KeyUpEvent, ParentElement, Pixels,
    Point, Render, SharedString, Styled, Window, WindowBounds, WindowOptions,
    prelude::FluentBuilder,
};
use gpui_component::{
    input::{InputEvent, InputState},
    resizable::{h_resizable, resizable_panel, v_resizable},
    v_flex, ActiveTheme, PixelsExt, Root, Theme, ThemeMode, TitleBar, TITLE_BAR_HEIGHT,
};

use crate::data::ai::AiConfig;
use crate::data::indicators::{BollSeries, MaSeries, MacdSeries};
use crate::data::levels;
use crate::data::market as market_data;
use crate::data::portfolio::{Portfolio, TradeSide};
use crate::data::scout::ScoutPick;
use crate::data::signals;
use crate::data::treasure::TreasureHit;
use crate::data::universe::{FinFilter, TreasurePool};
use crate::model::{
    board_for_code, normalize_code, shared, Candle, IndexSnap, MinuteSeries, Symbol, TrendLine,
};
use crate::storage::{
    self, clamp_quote_interval_secs, normalize_status_bar, ColorScheme, DockLayout, WatchlistSort,
    WorkDensity,
};
use crate::update::UpdateState;

use types::{
    AiCacheEntry, AiPanelState, AiSource, ChartKind, ChartRange, DetailTab, LeftTab, SettingsSection,
};

actions!(
    stock,
    [
        ToggleCommandPalette,
        RefreshData,
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

pub struct StockApp {
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

        let palette_query = cx.new(|cx| {
            InputState::new(window, cx).placeholder("搜索代码 / 名称，回车添加自选…")
        });
        let palette_focus = cx.focus_handle();
        let filtered_local: Vec<usize> = (0..symbols.len()).collect();

        let ai_cfg = cfg.ai_api.clone();
        let ai_base_url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://api.openai.com/v1")
        });
        let ai_model_input = cx.new(|cx| InputState::new(window, cx).placeholder("gpt-5-mini"));
        let ai_api_key_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("sk-…")
                .masked(true)
        });
        let ai_cli_bin_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("可选：CLI 绝对路径，如 /opt/homebrew/bin/claude")
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
        let trade_shares_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("股数，如 100"));
        let trade_price_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("成交价"));
        let trade_fee_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("手续费，可 0"));
        let trade_note_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("备注（可选）"));
        let portfolio_cash_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("现金余额"));
        portfolio_cash_input.update(cx, |state, cx| {
            state.set_value(format!("{:.2}", portfolio.cash), window, cx);
        });

        let work_alias_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("service tag · e.g. core-db")
        });

        let _subscriptions = vec![
            cx.subscribe_in(&palette_query, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => {
                            let q = state.read(cx).value().to_string();
                            this.on_palette_query_changed(&q, cx);
                        }
                        InputEvent::PressEnter { .. } => {
                            this.palette_confirm(window, cx);
                        }
                        _ => {}
                    }
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
                    match event {
                        InputEvent::PressEnter { .. } => {
                            this.commit_work_alias(window, cx);
                        }
                        _ => {}
                    }
                }
            }),
        ];

        let treasure_cache = storage::load_treasure_cache();
        let treasure_hits = treasure_cache.hits;
        let treasure_status = if treasure_hits.is_empty() {
            shared("点「开始搜罗」扫描历史低位，再用「AI 筛可买」批量出观察清单")
        } else {
            shared(format!(
                "缓存 {} 只 · {} · 可点「AI 筛可买」",
                treasure_hits.len(),
                if treasure_cache.updated_at.is_empty() {
                    "—".into()
                } else {
                    treasure_cache.updated_at
                }
            ))
        };

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
            treasure_done: 0,
            treasure_total: 0,
            treasure_status,
            treasure_gen: 0,
            scout_picks: Vec::new(),
            scout_summary: shared(""),
            scout_running: false,
            scout_done: 0,
            scout_total: 0,
            scout_gen: 0,
            scout_source: shared(""),
            // 默认只看「可关注」；若本轮为零会自动回退到「全部」。
            scout_only_buy_watch: true,
            treasure_list_expanded: false,
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

        window.set_window_title(app.window_title());
        app.bootstrap(cx);
        app
    }
}

impl Render for StockApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let bottom_h = self.dock.main_v.get(1).copied().unwrap_or(self.bottom_height);
        let work = self.work_mode;

        div()
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
                if !this.palette_open
                    && !this.settings_open
                    && this.left_tab == LeftTab::Watchlist
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
                                                        resizable_panel().child(
                                                            self.render_chart_area(cx),
                                                        ),
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
            .when(self.palette_open && !self.settings_open, |this| {
                this.child(self.render_palette(cx))
            })
            .children(Root::render_dialog_layer(window, cx))
    }
}

pub fn run() {
    let app = gpui::Application::new();

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

        // 完整 Dock 布局序列化：上次窗口位置/大小直接恢复（无记录则居中默认）。
        let cfg = storage::load_config();
        let window_bounds = match cfg.dock.window {
            // Allow Mini focus footprint (~720×440) to restore across restarts.
            Some((x, y, w, h)) if w >= 640.0 && h >= 400.0 => {
                WindowBounds::Windowed(Bounds {
                    origin: point(px(x), px(y)),
                    size: size(px(w), px(h)),
                })
            }
            _ => WindowBounds::centered(size(px(1320.), px(860.)), cx),
        };
        let window_options = WindowOptions {
            titlebar: Some(TitleBar::title_bar_options()),
            window_bounds: Some(window_bounds),
            ..Default::default()
        };

        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                window.activate_window();
                Theme::change(ThemeMode::Dark, Some(window), cx);
                // Title is set inside StockApp::new (respects persisted work_mode).
                let view = cx.new(|cx| StockApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });
}



#[cfg(test)]
mod keymap_tests {
    use gpui::{actions, KeyBinding, KeyContext, Keymap, Keystroke};

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
            bindings[0].action().as_any().downcast_ref::<InputBackspace>().is_some(),
            "input binding must win, got {}",
            bindings[0].action().name()
        );

        // 输入框聚焦：`0` 不再触发应用动作，按键落到文本输入。
        let (bindings, _) = km.bindings_for_input(&keystrokes("0"), &input_focused);
        assert!(bindings.is_empty(), "0 binding must yield while input focused");

        // 无输入框聚焦：应用快捷键照常生效。
        let (bindings, _) = km.bindings_for_input(&keystrokes("backspace"), &no_input);
        assert_eq!(bindings.len(), 1);
        assert!(
            bindings[0].action().as_any().downcast_ref::<AppBackspace>().is_some(),
            "app binding must win without input focus, got {}",
            bindings[0].action().name()
        );

        let (bindings, _) = km.bindings_for_input(&keystrokes("0"), &no_input);
        assert_eq!(bindings.len(), 1);
        assert!(
            bindings[0].action().as_any().downcast_ref::<AppZero>().is_some(),
            "app 0 binding must win without input focus"
        );
    }
}

#[cfg(test)]
mod layout_regression_tests {
    use super::StockApp;
    use gpui::{
        px, size, AnyWindowHandle, AppContext, TestAppContext, VisualContext, VisualTestContext,
    };
    use gpui_component::PixelsExt;

    /// Shared window/App setup for layout regression tests: isolated HOME and a
    /// deterministic default config (work mode off, fixed dock) so results do
    /// not depend on the developer's real `config.json`.
    fn test_window(
        cx: &mut TestAppContext,
        w: f32,
        h: f32,
    ) -> VisualTestContext {
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
        let cfg_dir = dirs::data_dir()
            .expect("data_dir")
            .join("stock-analysis");
        std::fs::create_dir_all(&cfg_dir).expect("create temp config dir");
        let cfg = crate::storage::AppConfig::default();
        let json = serde_json::to_string_pretty(&cfg).expect("serialize test config");
        std::fs::write(cfg_dir.join("config.json"), json).expect("write test config");

        cx.update(|cx| gpui_component::init(cx));
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
                |window, cx| cx.new(|cx| StockApp::new(window, cx)),
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
        window.cx.update_window(handle, |view, _window, cx| {
            view.downcast::<StockApp>()
                .expect("window root view")
                .update(cx, |this, cx| this.toggle_settings(cx));
        });
        window.run_until_parked();
        window.update(|window, cx| {
            window.draw(cx);
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
}

