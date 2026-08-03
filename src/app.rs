//! Root application: A-share watchlist, chart (MA + crosshair), resizable layout, persistence.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    actions, canvas, div, point, px, size, App, AppContext, Bounds, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, Timer, Window, WindowBounds, WindowOptions,
    prelude::FluentBuilder,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    IconName,
    input::{Input, InputEvent, InputState},
    resizable::{h_resizable, resizable_panel, v_resizable, ResizableState},
    v_flex, ActiveTheme, Disableable, PixelsExt, Root, Sizable, StyledExt, Theme, ThemeMode,
    TitleBar, TITLE_BAR_HEIGHT,
};
use gpui_component::tooltip::Tooltip;

use crate::chart::{
    chart_layout, index_from_x, paint_chart, paint_sparkline, price_from_y, BollPaintData,
    ChartPaintData, ChartStyle, MacdPaintData, MinutePaintData,
};
use crate::data::ai::{self, AiCliProvider, AiConfig, AiKind, AiTransport};
use crate::data::levels;
use crate::data::scout::{self, ScoutPick, ScoutVerdict, SCOUT_CANDIDATE_N};
use crate::data::treasure::{self, fmt_dd, fmt_pos, TreasureHit, TREASURE_KLINE_LIMIT};
use crate::data::universe::{self, FinFilter, TreasurePool, TREASURE_SCAN_CAP, TREASURE_TOP_N};
use crate::data::{
    indicators::{BollSeries, MaSeries, MacdSeries},
    market, signals,
};
use crate::data::market::Sourced;
use crate::model::{
    board_for_code, disguise_index, disguise_label, format_index, format_pct, format_price,
    format_volume, shared, Candle, IndexSnap, MinutePeriod, MinuteSeries, QuoteSnapshot, Symbol,
    TrendLine,
};
use crate::storage::{
    self, clamp_quote_interval_secs, normalize_status_bar, AppConfig, ColorScheme, DockLayout,
    WatchlistSort, STATUS_BAR_MAX_CODES,
};
use crate::update::{self, UpdateState};

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
const TITLE_NORMAL: &str = "ZStock · A股";
const TITLE_WORK: &str = "Notes";

/// Preset quote poll intervals offered in Settings (seconds).
const QUOTE_INTERVAL_PRESETS: &[u64] = &[1, 2, 3, 5, 8, 15, 30, 60];
const QUOTE_INTERVAL_ERR_MAX: Duration = Duration::from_secs(45);
/// Minimum candles visible when zoomed in.
const CHART_MIN_VISIBLE: usize = 15;
/// 寻宝扫描相邻请求间隔，降低限流概率。
/// 扩大扫描时相邻请求间隔（约 400 只 × 150ms ≈ 1 分钟级）。
const TREASURE_SCAN_GAP: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ChartRange {
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
    fn label(self) -> &'static str {
        match self {
            Self::M1 => "1M",
            Self::M3 => "3M",
            Self::M6 => "6M",
            Self::Y1 => "1Y",
            Self::Y3 => "3Y",
            Self::Max => "MAX",
        }
    }

    fn bars(self) -> usize {
        match self {
            Self::M1 => 22,
            Self::M3 => 66,
            Self::M6 => 130,
            Self::Y1 => 252,
            Self::Y3 => 750,
            Self::Max => TREASURE_KLINE_LIMIT,
        }
    }

    fn all() -> [Self; 6] {
        [Self::M1, Self::M3, Self::M6, Self::Y1, Self::Y3, Self::Max]
    }

    fn from_label(s: &str) -> Self {
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
enum ChartKind {
    /// 分时（当日分钟线，腾讯 minute/query）。
    Intraday,
    /// 日 K（配合 `ChartRange` 选择窗口）。
    DayK,
    /// 分钟 K（1/5/15/30/60 分）。
    MinuteK(MinutePeriod),
}

impl ChartKind {
    fn from_label(s: &str) -> Self {
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

    fn to_label(self) -> &'static str {
        match self {
            Self::Intraday => "intraday",
            Self::DayK => "day",
            Self::MinuteK(p) => p.param(),
        }
    }

}

/// 左侧栏：自选 vs 寻宝鼠。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum LeftTab {
    #[default]
    Watchlist,
    Treasure,
}

/// Full-page settings navigation (replaces the old modal dialog).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
enum SettingsSection {
    #[default]
    General = 0,
    StatusBar = 1,
    Ai = 2,
    Update = 3,
    About = 4,
}

impl SettingsSection {
    fn all() -> [Self; 5] {
        [
            Self::General,
            Self::StatusBar,
            Self::Ai,
            Self::Update,
            Self::About,
        ]
    }

    fn label(self, work: bool) -> &'static str {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
enum DetailTab {
    /// 一屏概览：评分徽章 + 关键因子 + 状态
    #[default]
    Overview = 0,
    /// 策略雷达完整因子
    Strategy = 1,
    /// AI 点评
    Ai = 2,
    /// 寻宝多窗口位置
    Treasure = 3,
    /// MA / MACD / BOLL 指标读数
    Indicators = 4,
}

impl DetailTab {
    fn all() -> [Self; 5] {
        [
            Self::Overview,
            Self::Strategy,
            Self::Ai,
            Self::Treasure,
            Self::Indicators,
        ]
    }

    fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Overview, true) => "Overview",
            (Self::Overview, false) => "概览",
            (Self::Strategy, true) => "Signal",
            (Self::Strategy, false) => "策略",
            (Self::Ai, true) => "AI",
            (Self::Ai, false) => "AI",
            (Self::Treasure, true) => "Scan",
            (Self::Treasure, false) => "寻宝",
            (Self::Indicators, true) => "Tech",
            (Self::Indicators, false) => "指标",
        }
    }
}

/// 底部「AI 点评」列的展示状态。
#[derive(Debug, Clone)]
enum AiPanelState {
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
enum AiSource {
    Local,
    /// Optional LLM / CLI result. `label` is the full source line (e.g. `LLM · gpt-5-mini` or `CLI · Grok`).
    Llm {
        label: String,
    },
}

impl AiSource {
    fn label(&self, work: bool) -> SharedString {
        match self {
            Self::Local => shared(if work { "Local rules" } else { "本地规则" }),
            Self::Llm { label } => shared(label.clone()),
        }
    }

    fn is_llm(&self) -> bool {
        matches!(self, Self::Llm { .. })
    }
}

/// 内存缓存条目：文本 + 来源（LLM 成功后替换本地条目，来源随条目保存）。
#[derive(Debug, Clone)]
struct AiCacheEntry {
    text: String,
    source: AiSource,
}

pub struct StockApp {
    symbols: Vec<Symbol>,
    selected: SharedString,
    /// Code that currently loaded `candles` belong to (may lag `selected` while loading).
    candles_code: Option<String>,
    /// Monotonic token so stale async kline responses are dropped.
    kline_gen: u64,
    candles: Vec<Candle>,
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
    status: SharedString,
    loading: bool,
    data_source: SharedString,
    palette_open: bool,
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
    /// Temporary owner map in work mode; intentionally never persisted.
    work_identity_reveal: bool,
    /// Sidebar row order.
    watchlist_sort: WatchlistSort,
    quote_fail_streak: u32,
    /// 左侧：自选 / 寻宝鼠
    left_tab: LeftTab,
    /// 底部分析台当前分区（会话内；不写入 config）。
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
    /// macOS menu bar: show live quotes for pinned watchlist codes.
    status_bar_enabled: bool,
    /// Codes pinned to the status bar menu (subset of watchlist, max 5).
    status_bar_codes: Vec<String>,
    /// Code currently shown in the status bar title.
    status_bar_active: String,
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
            .map(|code| Symbol {
                code: code.clone(),
                name: shared(code.clone()),
                last: 0.0,
                change_pct: 0.0,
                volume: 0,
                board: board_for_code(code),
            })
            .collect();

        let selected = if symbols.iter().any(|s| s.code == cfg.selected) {
            shared(cfg.selected.clone())
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

        let _subscriptions = vec![
            cx.subscribe_in(&palette_query, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        let q = state.read(cx).value().to_string();
                        this.on_palette_query_changed(&q, cx);
                    }
                }
            }),
            cx.subscribe_in(&ai_base_url_input, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.ai_config.base_url = state.read(cx).value().to_string();
                        this.persist();
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&ai_model_input, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.ai_config.model = state.read(cx).value().to_string();
                        this.persist();
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&ai_api_key_input, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.ai_config.api_key = state.read(cx).unmask_value().to_string();
                        this.persist();
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&ai_cli_bin_input, window, {
                move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.ai_config.cli_bin = state.read(cx).value().to_string();
                        this.persist();
                        cx.notify();
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
            status: shared("正在连接行情源…"),
            loading: true,
            data_source: shared(market::SRC_LABEL),
            palette_open: false,
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
            work_identity_reveal: false,
            watchlist_sort: cfg.watchlist_sort,
            quote_fail_streak: 0,
            left_tab: LeftTab::Watchlist,
            detail_tab: DetailTab::Overview,
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
            status_bar_enabled,
            status_bar_codes,
            status_bar_active,
            _subscriptions,
        };

        window.set_window_title(app.window_title());
        app.bootstrap(cx);
        app
    }

    fn window_title(&self) -> &'static str {
        if self.work_mode {
            TITLE_WORK
        } else {
            TITLE_NORMAL
        }
    }

    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        // Initial hydrate + klines
        self.refresh_all(cx);
        // 旧缓存可能只有代码没有中文名
        self.enrich_treasure_names_if_needed(cx);
        // Auto-update: check shortly after startup, then periodically.
        self.check_for_updates(false, cx);
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(4 * 60 * 60)).await;
                let res = smol::unblock(update::check_latest).await;
                if this
                    .update(cx, |app, cx| {
                        // Never clobber an in-flight install or visible error.
                        if matches!(
                            app.update_state,
                            UpdateState::Downloading(_) | UpdateState::Error(_)
                        ) {
                            return;
                        }
                        app.update_state = match res {
                            Ok(Some(info)) => UpdateState::Available(info),
                            Ok(None) => UpdateState::UpToDate,
                            // 自动检查失败保持安静（参考 Zed：离线/清单暂缺都不打扰用户）。
                            Err(_) => UpdateState::Idle,
                        };
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // Major indices for work-mode host gauges
        cx.spawn(async move |this, cx| {
            let idx = smol::unblock(market::fetch_major_indices).await;
            this.update(cx, |app, cx| {
                if let Ok(sourced) = idx {
                    let rows: Vec<_> = sourced
                        .data
                        .iter()
                        .map(|t| (t.code.clone(), t.name.clone(), t.last, t.change_pct))
                        .collect();
                    app.apply_index_ticks(&rows);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();

        // macOS trackpad pinch (NSEvent magnify) — GPUI does not forward this itself
        #[cfg(target_os = "macos")]
        {
            let pinch_rx = crate::mac_gesture::install_pinch_receiver();
            cx.spawn(async move |this, cx| {
                loop {
                    Timer::after(Duration::from_millis(8)).await;
                    let mut acc = 0.0f32;
                    let mut any = false;
                    while let Ok(m) = pinch_rx.try_recv() {
                        acc += m;
                        any = true;
                    }
                    if any {
                        let ok = this.update(cx, |app, cx| {
                            app.on_chart_pinch(acc, cx);
                        });
                        if ok.is_err() {
                            break;
                        }
                    }
                }
            })
            .detach();
        }

        // macOS menu bar quotes — install only when enabled (avoids AppKit
        // crashes in headless / unit-test windows that never open a real bar).
        #[cfg(target_os = "macos")]
        {
            if self.status_bar_enabled {
                self.ensure_status_bar_installed(cx);
            }
        }

        // Quote polling loop with backoff on failure (interval from settings).
        cx.spawn(async move |this, cx| {
            let mut delay = Duration::from_secs(1);
            loop {
                Timer::after(delay).await;
                let codes = match this.read_with(cx, |app, _| {
                    app.symbols.iter().map(|s| s.code.clone()).collect::<Vec<_>>()
                }) {
                    Ok(c) => c,
                    Err(_) => break,
                };
                if codes.is_empty() {
                    // Still respect configured interval while empty.
                    if let Ok(secs) = this.read_with(cx, |app, _| app.quote_interval_secs) {
                        delay = Duration::from_secs(secs);
                    }
                    continue;
                }
                let need_idx = this
                    .read_with(cx, |app, _| app.work_mode)
                    .unwrap_or(false);
                let result = smol::unblock(move || market::fetch_quotes(&codes)).await;
                let idx_result = if need_idx {
                    Some(smol::unblock(market::fetch_major_indices).await)
                } else {
                    None
                };
                let ok = this.update(cx, |app, cx| {
                    match result {
                        Ok(sourced) => {
                            app.quote_fail_streak = 0;
                            let symbol_ix: std::collections::HashMap<String, usize> = app
                                .symbols
                                .iter()
                                .enumerate()
                                .map(|(ix, s)| (s.code.clone(), ix))
                                .collect();
                            let treasure_ix: std::collections::HashMap<String, usize> = app
                                .treasure_hits
                                .iter()
                                .enumerate()
                                .map(|(ix, h)| (h.code.clone(), ix))
                                .collect();
                            for t in sourced.data {
                                if let Some(&ix) = symbol_ix.get(&t.code) {
                                    let sym = &mut app.symbols[ix];
                                    if is_real_name(&t.name, &t.code) {
                                        sym.name = shared(t.name.clone());
                                    }
                                    if t.last > 0.0 {
                                        sym.last = t.last;
                                        sym.change_pct = t.change_pct;
                                        sym.volume = t.volume;
                                    }
                                }
                                // 顺带补寻宝列表中文名
                                if is_real_name(&t.name, &t.code) {
                                    if let Some(&ix) = treasure_ix.get(&t.code) {
                                        let hit = &mut app.treasure_hits[ix];
                                        if hit.name != t.name {
                                            hit.name = t.name.clone();
                                        }
                                    }
                                }
                            }
                            if let Some(Ok(idx)) = &idx_result {
                                let rows: Vec<_> = idx
                                    .data
                                    .iter()
                                    .map(|t| {
                                        (t.code.clone(), t.name.clone(), t.last, t.change_pct)
                                    })
                                    .collect();
                                app.apply_index_ticks(&rows);
                            }
                            // Don't clobber an in-flight kline status unless idle
                            if !app.loading {
                                app.status = shared(format!(
                                    "行情已更新 · {} · {}",
                                    sourced.source,
                                    chrono::Local::now().format("%H:%M:%S")
                                ));
                            }
                            app.sync_status_bar();
                            cx.notify();
                            Duration::from_secs(app.quote_interval_secs)
                        }
                        Err(e) => {
                            app.quote_fail_streak = app.quote_fail_streak.saturating_add(1);
                            let base = app.quote_interval_secs.max(1);
                            let backoff_secs = (base * 2u64.pow(app.quote_fail_streak.min(5)))
                                .min(QUOTE_INTERVAL_ERR_MAX.as_secs());
                            app.status = shared(format!(
                                "行情刷新失败: {e} · {}s 后重试",
                                backoff_secs
                            ));
                            if let Some(Ok(idx)) = &idx_result {
                                let rows: Vec<_> = idx
                                    .data
                                    .iter()
                                    .map(|t| {
                                        (t.code.clone(), t.name.clone(), t.last, t.change_pct)
                                    })
                                    .collect();
                                app.apply_index_ticks(&rows);
                            }
                            cx.notify();
                            Duration::from_secs(backoff_secs)
                        }
                    }
                });
                match ok {
                    Ok(next) => delay = next,
                    Err(_) => break,
                }
            }
        })
        .detach();

        // 分时自动刷新（仅 Intraday 模式）。
        self.spawn_minute_refresh_loop(cx);
    }

    // 分时自动刷新：仅 Intraday 模式生效，约每 5 秒补一根新分钟线。
    // 分时刷新在 quote loop 之外单独跑，避免拖慢行情轮询。
    fn spawn_minute_refresh_loop(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut delay = Duration::from_secs(5);
            loop {
                Timer::after(delay).await;
                let is_intraday = this
                    .read_with(cx, |app, _| {
                        matches!(app.chart_kind, ChartKind::Intraday)
                    })
                    .unwrap_or(false);
                if !is_intraday {
                    delay = Duration::from_secs(5);
                    continue;
                }
                let selected = match this.read_with(cx, |app, _| app.selected.to_string()) {
                    Ok(s) => s,
                    Err(_) => break,
                };
                if selected.is_empty() {
                    continue;
                }
                let fetch_code = selected.clone();
                let result =
                    smol::unblock(move || market::fetch_minute_series(&fetch_code)).await;
                let ok = this.update(cx, |app, cx| {
                    if !matches!(app.chart_kind, ChartKind::Intraday)
                        || app.selected.as_ref() != selected
                    {
                        return;
                    }
                    if let Ok(sourced) = result {
                        app.apply_minute(&selected, sourced.data);
                        cx.notify();
                    }
                });
                if ok.is_err() {
                    break;
                }
                delay = Duration::from_secs(5);
            }
        })
        .detach();
    }

    fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let codes: Vec<String> = self.symbols.iter().map(|s| s.code.clone()).collect();
        let selected = self.selected.to_string();
        let bars = self.current_bars();
        let is_intraday = matches!(self.chart_kind, ChartKind::Intraday);
        let minute_period = match self.chart_kind {
            ChartKind::MinuteK(p) => Some(p),
            ChartKind::DayK | ChartKind::Intraday => None,
        };
        let req_kind = self.chart_kind;
        self.kline_gen = self.kline_gen.wrapping_add(1);
        let req_gen = self.kline_gen;
        // Keep previous candles painted until the new series arrives (no blank flash).
        self.hover_ix = None;
        self.loading = true;
        self.status = shared(if is_intraday {
            format!("加载 {selected} 分时…")
        } else {
            "加载中…".into()
        });
        cx.notify();

        cx.spawn(async move |this, cx| {
            let codes2 = codes.clone();
            let req_code = selected.clone();
            let quotes = smol::unblock(move || market::hydrate_symbols(&codes2)).await;
            let minute = if is_intraday {
                let c = selected.clone();
                Some(smol::unblock(move || market::fetch_minute_series(&c)).await)
            } else {
                None
            };
            let kline = if is_intraday {
                None
            } else if let Some(p) = minute_period {
                let code = selected.clone();
                Some(smol::unblock(move || {
                    market::fetch_minute_klines(&code, p, bars).map(|s| Sourced {
                        data: (code.clone(), String::new(), s.data),
                        source: s.source,
                    })
                })
                .await)
            } else {
                Some(smol::unblock(move || market::fetch_klines(&selected, bars)).await)
            };

            this.update(cx, |app, cx| {
                let mut quote_src = None;
                match quotes {
                    Ok(sourced) => {
                        quote_src = Some(sourced.source);
                        let quotes: std::collections::HashMap<&str, &Symbol> = sourced
                            .data
                            .iter()
                            .map(|symbol| (symbol.code.as_str(), symbol))
                            .collect();
                        for s in &mut app.symbols {
                            if let Some(n) = quotes.get(s.code.as_str()) {
                                // Keep existing last if hydrate returned zeros
                                let keep_last = s.last;
                                let keep_chg = s.change_pct;
                                let keep_vol = s.volume;
                                *s = (*n).clone();
                                if s.last <= 0.0 && keep_last > 0.0 {
                                    s.last = keep_last;
                                    s.change_pct = keep_chg;
                                    s.volume = keep_vol;
                                }
                            }
                        }
                        app.quote_fail_streak = 0;
                    }
                    Err(e) => {
                        app.status = shared(format!("自选列表加载失败: {e}"));
                    }
                }
                // Drop stale kline if user switched while we were loading
                if req_gen != app.kline_gen
                    || app.selected.as_ref() != req_code
                    || app.chart_kind != req_kind
                {
                    return;
                }
                if is_intraday {
                    match minute {
                        Some(Ok(sourced)) => {
                            let name = sourced.data.name.clone();
                            app.apply_minute(&req_code, sourced.data);
                            app.status = shared(format!(
                                "已加载 {} · 分时 {} · 行情{} · {} · {}",
                                req_code,
                                name,
                                quote_src.unwrap_or("—"),
                                sourced.source,
                                chrono::Local::now().format("%H:%M:%S")
                            ));
                        }
                        Some(Err(e)) => {
                            app.status = shared(format!("分时加载失败: {e}"));
                        }
                        None => {}
                    }
                } else {
                    match kline {
                        Some(Ok(sourced)) => {
                            let (_resp_code, name, candles) = sourced.data;
                            app.apply_klines(&req_code, name, candles);
                            app.status = shared(format!(
                                "已加载 {} · {} 根K线 · 行情{} · K线{} · {}",
                                req_code,
                                app.candles.len(),
                                quote_src.unwrap_or("—"),
                                sourced.source,
                                chrono::Local::now().format("%H:%M:%S")
                            ));
                        }
                        Some(Err(e)) => {
                            app.status = shared(format!("K线加载失败: {e}"));
                        }
                        None => {}
                    }
                }
                app.loading = false;
                app.persist();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_klines(&mut self, code: &str, name: String, candles: Vec<Candle>) {
        if let Some(sym) = self.symbols.iter_mut().find(|s| s.code == code) {
            // 仅写入真实中文名；空名 / 代码占位不覆盖已有名称
            if is_real_name(&name, code) {
                sym.name = shared(name);
            }
            if let Some(last) = candles.last() {
                let prev = candles
                    .get(candles.len().saturating_sub(2))
                    .map(|c| c.close)
                    .unwrap_or(last.open);
                // Prefer live quote when present; only fill from kline if missing
                if sym.last <= 0.0 {
                    sym.last = last.close;
                    sym.change_pct = if prev > 0.0 {
                        (last.close - prev) / prev * 100.0
                    } else {
                        0.0
                    };
                    sym.volume = last.volume;
                }
            }
        }
        self.candles = candles;
        self.candles_code = Some(code.to_string());
        self.ma = MaSeries::from_candles(&self.candles);
        self.macd = MacdSeries::from_candles(&self.candles);
        self.boll = BollSeries::from_candles(&self.candles);
        self.hover_ix = None;
        self.reset_chart_view();
    }

    fn apply_minute(&mut self, code: &str, series: MinuteSeries) {
        // Periodic refresh of the same code keeps the user's zoom/pan window.
        let same_series = self.minute_code.as_deref() == Some(code) && self.minute.is_some();
        if let Some(sym) = self.symbols.iter_mut().find(|s| s.code == code) {
            if is_real_name(&series.name, code) {
                sym.name = shared(series.name.clone());
            }
            if sym.last <= 0.0 {
                if let Some(snap) = series.snapshot() {
                    sym.last = snap.close;
                    sym.change_pct = snap.change_pct;
                    sym.volume = snap.volume;
                }
            }
        }
        self.candles = series.as_candles();
        self.candles_code = Some(code.to_string());
        self.minute = Some(series);
        self.minute_code = Some(code.to_string());
        self.ma = MaSeries::default();
        self.macd = MacdSeries::default();
        self.boll = BollSeries::default();
        self.hover_ix = None;
        if !same_series {
            self.reset_chart_view();
        }
    }

    /// Bars requested for the current chart kind.
    fn current_bars(&self) -> usize {
        match self.chart_kind {
            ChartKind::Intraday => 0,
            ChartKind::DayK => self.range.bars(),
            ChartKind::MinuteK(p) => p.bars(),
        }
    }

    /// Human label for the current chart, e.g. `日K · 3M`, `5分K`, `分时`.
    fn chart_label(&self) -> String {
        match self.chart_kind {
            ChartKind::Intraday => "分时".into(),
            ChartKind::DayK => format!("日K · {}", self.range.label()),
            ChartKind::MinuteK(p) => format!("{}K", p.label()),
        }
    }

    fn reset_chart_view(&mut self) {
        self.chart_view_start = 0;
        self.chart_view_count = 0; // show all
    }

    /// Half-open `[start, end)` index range currently painted.
    fn chart_visible_range(&self) -> (usize, usize) {
        let n = self.candles.len();
        if n == 0 {
            return (0, 0);
        }
        let count = if self.chart_view_count == 0 {
            n
        } else {
            self.chart_view_count.clamp(CHART_MIN_VISIBLE, n)
        };
        let start = self.chart_view_start.min(n.saturating_sub(count));
        (start, start + count)
    }

    /// Continuous zoom: `factor < 1` shows fewer bars (zoom in).
    fn chart_zoom_factor(&mut self, factor: f32, anchor: Option<usize>) {
        let n = self.candles.len();
        if n <= CHART_MIN_VISIBLE {
            return;
        }
        let factor = factor.clamp(0.55, 1.8);
        if (factor - 1.0).abs() < 0.002 {
            return;
        }
        let (start, end) = self.chart_visible_range();
        let old_count = (end - start).max(1);
        let mut new_count = ((old_count as f32) * factor).round() as usize;
        new_count = new_count.clamp(CHART_MIN_VISIBLE, n);
        if new_count == old_count {
            if factor > 1.0 && old_count >= n {
                self.chart_view_count = 0;
                self.chart_view_start = 0;
            }
            return;
        }

        let anchor = anchor
            .filter(|a| *a < n)
            .unwrap_or(start + old_count / 2);
        let rel = if old_count > 1 {
            (anchor.saturating_sub(start)) as f32 / (old_count as f32)
        } else {
            0.5
        };
        let new_start = anchor.saturating_sub((rel * new_count as f32) as usize);
        let new_start = new_start.min(n.saturating_sub(new_count));
        self.chart_view_start = new_start;
        self.chart_view_count = if new_count >= n { 0 } else { new_count };
        self.clamp_hover_to_view();
    }

    /// Discrete step zoom (mouse wheel notches).
    fn chart_zoom(&mut self, zoom_in: bool, anchor: Option<usize>) {
        self.chart_zoom_factor(if zoom_in { 0.82 } else { 1.22 }, anchor);
    }

    fn clamp_hover_to_view(&mut self) {
        if let Some(h) = self.hover_ix {
            let (s, e) = self.chart_visible_range();
            if h < s || h >= e {
                self.hover_ix = None;
            }
        }
    }

    fn chart_pan(&mut self, delta_bars: i32) {
        if delta_bars == 0 {
            return;
        }
        let n = self.candles.len();
        if n == 0 {
            return;
        }
        let (start, end) = self.chart_visible_range();
        let count = end - start;
        if count >= n {
            return;
        }
        let new_start = (start as i32 + delta_bars).clamp(0, (n - count) as i32) as usize;
        self.chart_view_start = new_start;
        self.chart_view_count = count;
        self.clamp_hover_to_view();
    }

    /// Apply trackpad pinch magnification (positive ≈ fingers apart → zoom in).
    fn on_chart_pinch(&mut self, magnification: f32, cx: &mut Context<Self>) {
        if self.candles.is_empty() || magnification.abs() < 1e-5 {
            return;
        }
        // Magnify > 0 → zoom in (fewer bars): factor < 1
        let factor = (1.0 - magnification * 2.4).clamp(0.65, 1.45);
        self.chart_zoom_factor(factor, self.hover_ix);
        cx.notify();
    }

    fn on_chart_scroll(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let n = self.candles.len();
        if n == 0 {
            return;
        }
        let precise = matches!(ev.delta, ScrollDelta::Pixels(_));
        let dy = match ev.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y.as_f32() / 48.0,
        };
        let dx = match ev.delta {
            ScrollDelta::Lines(p) => p.x,
            ScrollDelta::Pixels(p) => p.x.as_f32() / 48.0,
        };

        // Ctrl/Cmd + scroll always zooms (browser-like / accessibility)
        let force_zoom = ev.modifiers.control || ev.modifiers.platform;

        // Horizontal dominant → pan (unless modifier forces zoom)
        if !force_zoom && dx.abs() > dy.abs() && dx.abs() > 0.05 {
            let bars = (dx * 4.0).round() as i32;
            if bars != 0 {
                self.chart_pan(bars);
                cx.notify();
            }
            return;
        }

        if dy.abs() < 0.04 {
            return;
        }

        let local_x = ev.position.x.as_f32() - self.chart_origin.x.as_f32();
        let (start, end) = self.chart_visible_range();
        let visible_n = end - start;
        let local_ix = index_from_x(local_x, self.chart_width, visible_n);
        let anchor = local_ix.map(|i| start + i).or(self.hover_ix);

        if precise || force_zoom {
            // Continuous zoom for trackpad pixel deltas / modifier zoom
            // dy < 0 (scroll up / natural) → zoom in
            let factor = (1.0 + dy * 0.12).clamp(0.75, 1.35);
            self.chart_zoom_factor(factor, anchor);
        } else {
            self.chart_zoom(dy < 0.0, anchor);
        }
        cx.notify();
    }

    fn reload_klines(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected.to_string();
        let bars = self.current_bars();
        let minute_period = match self.chart_kind {
            ChartKind::MinuteK(p) => Some(p),
            ChartKind::DayK | ChartKind::Intraday => None,
        };
        let req_kind = self.chart_kind;
        self.kline_gen = self.kline_gen.wrapping_add(1);
        let req_gen = self.kline_gen;
        // Keep last series visible while loading (header uses live quote + loading flag).
        self.hover_ix = None;
        self.loading = true;
        self.status = shared(format!("加载 {selected} {}…", self.chart_label()));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let req_code = selected.clone();
            let result = if let Some(p) = minute_period {
                let code = selected.clone();
                smol::unblock(move || {
                    market::fetch_minute_klines(&code, p, bars).map(|s| Sourced {
                        data: (code.clone(), String::new(), s.data),
                        source: s.source,
                    })
                })
                .await
            } else {
                smol::unblock(move || market::fetch_klines(&selected, bars)).await
            };
            this.update(cx, |app, cx| {
                if req_gen != app.kline_gen
                    || app.selected.as_ref() != req_code
                    || app.chart_kind != req_kind
                {
                    // A newer request is in flight / selection changed
                    return;
                }
                match result {
                    Ok(sourced) => {
                        let (_resp_code, name, candles) = sourced.data;
                        app.apply_klines(&req_code, name, candles);
                        app.status = shared(format!(
                            "{} · {} 根 {} · {}",
                            req_code,
                            app.candles.len(),
                            app.chart_label(),
                            sourced.source
                        ));
                    }
                    Err(e) => {
                        app.status = shared(format!("{}加载失败: {e}", app.chart_label()));
                    }
                }
                app.loading = false;
                app.persist();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reload_minute(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected.to_string();
        self.minute_gen = self.minute_gen.wrapping_add(1);
        let req_gen = self.minute_gen;
        self.hover_ix = None;
        self.loading = true;
        self.status = shared(format!("加载 {selected} 分时…"));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let req_code = selected.clone();
            let result = smol::unblock(move || market::fetch_minute_series(&selected)).await;
            this.update(cx, |app, cx| {
                if req_gen != app.minute_gen
                    || app.selected.as_ref() != req_code
                    || !matches!(app.chart_kind, ChartKind::Intraday)
                {
                    return;
                }
                match result {
                    Ok(sourced) => {
                        app.apply_minute(&req_code, sourced.data);
                        app.status = shared(format!(
                            "{} · 分时 {} 点 · {}",
                            req_code,
                            app.minute.as_ref().map(|m| m.points.len()).unwrap_or(0),
                            sourced.source
                        ));
                    }
                    Err(e) => {
                        app.status = shared(format!("分时失败: {e}"));
                    }
                }
                app.loading = false;
                app.persist();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reload_chart(&mut self, cx: &mut Context<Self>) {
        match self.chart_kind {
            ChartKind::Intraday => self.reload_minute(cx),
            ChartKind::DayK | ChartKind::MinuteK(_) => self.reload_klines(cx),
        }
    }

    fn set_chart_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        if self.chart_kind == kind {
            return;
        }
        self.chart_kind = kind;
        self.persist();
        self.reload_chart(cx);
    }

    fn persist(&self) {
        let mut dock = self.dock.clone();
        dock.window = self.window_bounds;
        let cfg = AppConfig {
            watchlist: self.symbols.iter().map(|s| s.code.clone()).collect(),
            selected: self.selected.to_string(),
            range: self.range.label().into(),
            chart_kind: self.chart_kind.to_label().into(),
            show_ma5: self.show_ma5,
            show_ma10: self.show_ma10,
            show_ma20: self.show_ma20,
            show_ma60: self.show_ma60,
            show_volume: self.show_volume,
            show_macd: self.show_macd,
            show_boll: self.show_boll,
            dock,
            left_width: self.left_width,
            bottom_height: self.bottom_height,
            color_scheme: self.color_scheme,
            work_mode: self.work_mode,
            quote_interval_secs: self.quote_interval_secs,
            watchlist_sort: self.watchlist_sort,
            ai_api: self.ai_config.clone(),
            chart_lines: self.chart_lines.clone(),
            treasure_pool: self.treasure_pool.id().into(),
            treasure_fin: self.treasure_fin.id().into(),
            status_bar_enabled: self.status_bar_enabled,
            status_bar_codes: self.status_bar_codes.clone(),
            status_bar_active: self.status_bar_active.clone(),
        };
        let _ = storage::save_config(&cfg);
    }

    fn dismiss_overlay(&mut self, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
            cx.notify();
            return;
        }
        if self.settings_open {
            self.close_settings(cx);
            return;
        }
        if self.drawing_mode {
            self.drawing_mode = false;
            self.drawing_anchor = None;
            self.draft_line = None;
            self.status = shared(if self.work_mode {
                "draw mode off"
            } else {
                "已退出画线模式"
            });
            cx.notify();
        }
    }

    fn select_adjacent_symbol(&mut self, delta: i32, cx: &mut Context<Self>) {
        let order = self.watchlist_display_order();
        if order.is_empty() {
            return;
        }
        let cur = order
            .iter()
            .position(|&ix| self.symbols[ix].code == self.selected.as_ref())
            .unwrap_or(0);
        let n = order.len() as i32;
        let next = ((cur as i32 + delta).rem_euclid(n)) as usize;
        let code = shared(self.symbols[order[next]].code.clone());
        self.select_symbol(code, cx);
    }

    /// Indices into `symbols` in the current sidebar order.
    fn watchlist_display_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.symbols.len()).collect();
        match self.watchlist_sort {
            WatchlistSort::Manual => {}
            WatchlistSort::ChangeDesc => {
                order.sort_by(|&a, &b| {
                    self.symbols[b]
                        .change_pct
                        .partial_cmp(&self.symbols[a].change_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| self.symbols[a].code.cmp(&self.symbols[b].code))
                });
            }
            WatchlistSort::ChangeAsc => {
                order.sort_by(|&a, &b| {
                    self.symbols[a]
                        .change_pct
                        .partial_cmp(&self.symbols[b].change_pct)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| self.symbols[a].code.cmp(&self.symbols[b].code))
                });
            }
            WatchlistSort::CodeAsc => {
                order.sort_by(|&a, &b| self.symbols[a].code.cmp(&self.symbols[b].code));
            }
        }
        order
    }

    fn set_watchlist_sort(&mut self, sort: WatchlistSort, cx: &mut Context<Self>) {
        if self.watchlist_sort == sort {
            return;
        }
        self.watchlist_sort = sort;
        self.persist();
        cx.notify();
    }

    fn on_main_h_resize(
        &mut self,
        state: &Entity<ResizableState>,
        cx: &mut Context<Self>,
    ) {
        let sizes = state.read(cx).sizes().clone();
        let new: Vec<f32> = sizes.iter().map(|s| s.as_f32()).collect();
        if !new.is_empty() && new != self.dock.main_h {
            self.dock.main_h = new.clone();
            if let Some(w) = new.first() {
                self.left_width = *w;
            }
            self.persist();
        }
    }

    fn on_main_v_resize(
        &mut self,
        state: &Entity<ResizableState>,
        cx: &mut Context<Self>,
    ) {
        let sizes = state.read(cx).sizes().clone();
        let new: Vec<f32> = sizes.iter().map(|s| s.as_f32()).collect();
        if !new.is_empty() && new != self.dock.main_v {
            self.dock.main_v = new.clone();
            if let Some(h) = new.get(1) {
                self.bottom_height = *h;
            }
            self.persist();
        }
    }

    fn set_color_scheme(&mut self, scheme: ColorScheme, cx: &mut Context<Self>) {
        if self.color_scheme == scheme {
            return;
        }
        self.color_scheme = scheme;
        self.persist();
        cx.notify();
    }

    fn set_work_mode(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.work_mode == on {
            return;
        }
        self.work_mode = on;
        self.work_identity_reveal = false;
        self.palette_query.update(cx, |input, cx| {
            input.set_placeholder(
                if on {
                    "Find service or id…"
                } else {
                    "搜索代码 / 名称，回车添加自选…"
                },
                window,
                cx,
            );
        });
        window.set_window_title(self.window_title());
        self.persist();
        self.sync_status_bar();
        cx.notify();
    }

    fn toggle_work_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_work_mode(!self.work_mode, window, cx);
    }

    fn toggle_work_identity(&mut self, cx: &mut Context<Self>) {
        if !self.work_mode {
            return;
        }
        self.work_identity_reveal = !self.work_identity_reveal;
        cx.notify();
    }

    fn set_quote_interval_secs(&mut self, secs: u64, cx: &mut Context<Self>) {
        let secs = clamp_quote_interval_secs(secs);
        if self.quote_interval_secs == secs {
            return;
        }
        self.quote_interval_secs = secs;
        self.persist();
        cx.notify();
    }

    fn set_status_bar_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.status_bar_enabled == on {
            return;
        }
        self.status_bar_enabled = on;
        // Auto-pin current selection when turning on with an empty list.
        if on && self.status_bar_codes.is_empty() {
            let code = self.selected.to_string();
            if !code.is_empty() {
                self.status_bar_codes.push(code.clone());
                self.status_bar_active = code;
            }
        }
        self.normalize_status_bar_state();
        if on {
            self.ensure_status_bar_installed(cx);
        }
        self.persist();
        self.sync_status_bar();
        cx.notify();
    }

    /// Install the native status item once and start polling menu actions.
    #[cfg(target_os = "macos")]
    fn ensure_status_bar_installed(&mut self, cx: &mut Context<Self>) {
        // AppKit NSStatusItem is unsafe in headless / gpui unit tests.
        if cfg!(test) {
            return;
        }
        use crate::mac_status_bar;
        if mac_status_bar::is_installed() {
            self.sync_status_bar();
            return;
        }
        let action_rx = mac_status_bar::install();
        self.sync_status_bar();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(50)).await;
                let mut actions = Vec::new();
                while let Ok(a) = action_rx.try_recv() {
                    actions.push(a);
                }
                if actions.is_empty() {
                    continue;
                }
                let ok = this.update(cx, |app, cx| {
                    for a in actions {
                        app.handle_status_bar_action(a, cx);
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    #[cfg(not(target_os = "macos"))]
    fn ensure_status_bar_installed(&mut self, _cx: &mut Context<Self>) {}

    fn toggle_status_bar_code(&mut self, code: &str, cx: &mut Context<Self>) {
        if let Some(ix) = self.status_bar_codes.iter().position(|c| c == code) {
            self.status_bar_codes.remove(ix);
            if self.status_bar_active == code {
                self.status_bar_active = self
                    .status_bar_codes
                    .first()
                    .cloned()
                    .unwrap_or_default();
            }
        } else {
            if self.status_bar_codes.len() >= STATUS_BAR_MAX_CODES {
                self.status = shared(if self.work_mode {
                    format!("Status bar max {STATUS_BAR_MAX_CODES}")
                } else {
                    format!("状态栏最多固定 {STATUS_BAR_MAX_CODES} 只")
                });
                cx.notify();
                return;
            }
            if !self.symbols.iter().any(|s| s.code == code) {
                return;
            }
            self.status_bar_codes.push(code.to_string());
            if self.status_bar_active.is_empty() {
                self.status_bar_active = code.to_string();
            }
        }
        self.normalize_status_bar_state();
        self.persist();
        self.sync_status_bar();
        cx.notify();
    }

    fn set_status_bar_active(&mut self, code: &str, cx: &mut Context<Self>) {
        if !self.status_bar_codes.iter().any(|c| c == code) {
            return;
        }
        if self.status_bar_active == code {
            return;
        }
        self.status_bar_active = code.to_string();
        self.persist();
        self.sync_status_bar();
        cx.notify();
    }

    fn normalize_status_bar_state(&mut self) {
        let watchlist: Vec<String> = self.symbols.iter().map(|s| s.code.clone()).collect();
        let (enabled, codes, active) = normalize_status_bar(
            self.status_bar_enabled,
            &self.status_bar_codes,
            &self.status_bar_active,
            &watchlist,
        );
        self.status_bar_enabled = enabled;
        self.status_bar_codes = codes;
        self.status_bar_active = active;
    }

    /// Push current status-bar state to the native menu bar item (macOS only).
    fn sync_status_bar(&self) {
        #[cfg(target_os = "macos")]
        {
            use crate::mac_status_bar::{self, MenuEntry};

            if !mac_status_bar::is_installed() {
                return;
            }
            mac_status_bar::set_visible(self.status_bar_enabled);
            if !self.status_bar_enabled {
                return;
            }

            // All pinned symbols appear together in the menu-bar title.
            // No pins → show the S logo instead of "ZStock · 未固定".
            let syms: Vec<&Symbol> = self
                .status_bar_codes
                .iter()
                .filter_map(|code| self.symbols.iter().find(|s| s.code == *code))
                .collect();
            if syms.is_empty() {
                mac_status_bar::set_logo();
            } else {
                let title = self.status_bar_multi_title_for(&syms);
                mac_status_bar::set_title(&title);
            }

            let selected = self.selected.as_ref();
            let entries: Vec<MenuEntry> = self
                .status_bar_codes
                .iter()
                .filter_map(|code| {
                    let sym = self.symbols.iter().find(|s| s.code == *code)?;
                    Some(MenuEntry {
                        code: code.clone(),
                        label: self.status_bar_menu_label_for(sym),
                        // Checkmark = currently selected in the main window.
                        active: *code == selected,
                    })
                })
                .collect();
            mac_status_bar::rebuild_menu(&entries, self.work_mode);
        }
    }

    /// Menu-bar title: one symbol full, many symbols compact side-by-side.
    /// Caller guarantees `syms` is non-empty.
    fn status_bar_multi_title_for(&self, syms: &[&Symbol]) -> String {
        if syms.len() == 1 {
            return self.status_bar_title_for(syms[0]);
        }
        // Multi: keep each segment short so 3–5 names still fit the menu bar.
        let parts: Vec<String> = syms
            .iter()
            .map(|s| self.status_bar_compact_for(s))
            .collect();
        parts.join(" · ")
    }

    fn status_bar_title_for(&self, sym: &Symbol) -> String {
        if self.work_mode {
            let alias = disguise_label(&sym.code, sym.name.as_ref());
            if sym.last > 0.0 {
                format!("{alias} {:+.2}%", sym.change_pct)
            } else {
                alias
            }
        } else {
            let name = short_status_name(sym.name.as_ref(), &sym.code);
            if sym.last > 0.0 {
                format!(
                    "{name} {} {}",
                    format_price(sym.last),
                    format_pct(sym.change_pct)
                )
            } else {
                format!("{name} …")
            }
        }
    }

    /// Compact segment for multi-symbol titles: `名+涨跌%` (no price).
    fn status_bar_compact_for(&self, sym: &Symbol) -> String {
        if self.work_mode {
            let alias = disguise_label(&sym.code, sym.name.as_ref());
            if sym.last > 0.0 {
                format!("{alias}{:+.2}%", sym.change_pct)
            } else {
                alias
            }
        } else {
            let name = short_status_name(sym.name.as_ref(), &sym.code);
            if sym.last > 0.0 {
                format!("{name}{}", format_pct(sym.change_pct))
            } else {
                format!("{name}…")
            }
        }
    }

    fn status_bar_menu_label_for(&self, sym: &Symbol) -> String {
        if self.work_mode {
            let alias = disguise_label(&sym.code, sym.name.as_ref());
            if sym.last > 0.0 {
                format!("{alias}  {:+.2}%", sym.change_pct)
            } else {
                alias
            }
        } else {
            let name = short_status_name(sym.name.as_ref(), &sym.code);
            if sym.last > 0.0 {
                format!(
                    "{}  {}  {}",
                    name,
                    format_price(sym.last),
                    format_pct(sym.change_pct)
                )
            } else {
                format!("{name}  ({})", sym.code)
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn handle_status_bar_action(
        &mut self,
        action: crate::mac_status_bar::StatusBarAction,
        cx: &mut Context<Self>,
    ) {
        use crate::mac_status_bar::StatusBarAction;
        match action {
            StatusBarAction::SelectCode(code) => {
                // Focus that symbol in the main window (title still shows all pins).
                self.status_bar_active = code.clone();
                self.settings_open = false;
                self.persist();
                self.select_symbol(shared(code), cx);
                self.activate_main_window(cx);
            }
            StatusBarAction::ShowWindow => {
                self.settings_open = false;
                cx.notify();
                self.activate_main_window(cx);
            }
            StatusBarAction::Quit => {
                cx.quit();
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn activate_main_window(&self, cx: &mut Context<Self>) {
        cx.activate(true);
        for handle in cx.windows() {
            let _ = handle.update(cx, |_root, window, _cx| {
                window.activate_window();
            });
        }
    }

    /// 当前展示的 AI 点评对应的缓存键（`code@最后一根 K 日期`）。
    /// 与 `ai_key` 不一致时，详情栏按「未生成」展示，避免串股。
    fn ai_current_key(&self) -> Option<String> {
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        if !matched {
            return None;
        }
        let date = self.candles.last()?.date.to_string();
        Some(format!("{}@{date}", self.selected))
    }

    fn request_ai_commentary(&mut self, cx: &mut Context<Self>) {
        let Some(last) = self.candles.last() else {
            self.ai_panel = AiPanelState::Idle;
            cx.notify();
            return;
        };
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        if !matched {
            self.ai_panel = AiPanelState::Idle;
            cx.notify();
            return;
        }
        let code = self.selected.to_string();
        let name = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.name.to_string())
            .unwrap_or_default();
        let Some(snap) = ai::build_snapshot(&self.candles, &code, &name) else {
            self.ai_panel = AiPanelState::Ready {
                text: shared("数据不足：策略雷达需要至少 20 根有效日 K。"),
                source: AiSource::Local,
                note: None,
            };
            self.ai_key = self.ai_current_key();
            cx.notify();
            return;
        };
        let cache_key = format!("{}@{}", code, last.date);

        // 内存缓存命中（本地或 LLM 结果）直接展示。
        if let Some(hit) = self.ai_cache.get(&cache_key).cloned() {
            self.ai_panel = AiPanelState::Ready {
                text: hit.text.into(),
                source: hit.source,
                note: None,
            };
            self.ai_key = Some(cache_key);
            cx.notify();
            return;
        }

        // 先秒出本地规则点评，LLM 结果到达后再覆盖。
        let local = ai::local_commentary(&snap);
        self.ai_cache.insert(
            cache_key.clone(),
            AiCacheEntry {
                text: local.clone(),
                source: AiSource::Local,
            },
        );
        self.ai_key = Some(cache_key.clone());

        if !self.ai_config.enabled {
            self.ai_panel = AiPanelState::Ready {
                text: local.into(),
                source: AiSource::Local,
                note: None,
            };
            cx.notify();
            return;
        }

        self.ai_panel = AiPanelState::Loading {
            text: local.clone().into(),
        };
        self.ai_gen = self.ai_gen.wrapping_add(1);
        let req_id = self.ai_gen;
        let cfg = self.ai_config.clone();
        let source_label = cfg.source_label();
        cx.spawn(async move |this, cx| {
            let res = smol::unblock(move || ai::llm_commentary(&cfg, &snap)).await;
            let _ = this.update(cx, |app, cx| {
                if app.ai_gen != req_id {
                    return;
                }
                match res {
                    Ok(text) if !text.trim().is_empty() => {
                        let source = AiSource::Llm {
                            label: source_label.clone(),
                        };
                        app.ai_cache.insert(
                            cache_key.clone(),
                            AiCacheEntry {
                                text: text.clone(),
                                source: source.clone(),
                            },
                        );
                        app.ai_panel = AiPanelState::Ready {
                            text: text.into(),
                            source,
                            note: None,
                        };
                    }
                    Ok(_) => {
                        app.ai_panel = AiPanelState::Ready {
                            text: local.clone().into(),
                            source: AiSource::Local,
                            note: Some(shared("LLM 返回了空内容")),
                        };
                    }
                    Err(e) => {
                        app.ai_panel = AiPanelState::Ready {
                            text: local.clone().into(),
                            source: AiSource::Local,
                            note: Some(shared(format!("LLM 请求失败：{e}"))),
                        };
                    }
                }
                app.ai_key = Some(cache_key.clone());
                cx.notify();
            });
        })
        .detach();
    }

    fn set_ai_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.ai_config.enabled == enabled {
            return;
        }
        self.ai_config.enabled = enabled;
        self.persist();
        cx.notify();
    }

    fn set_ai_kind(&mut self, kind: AiKind, cx: &mut Context<Self>) {
        if self.ai_config.kind == kind {
            return;
        }
        self.ai_config.kind = kind;
        self.persist();
        cx.notify();
    }

    fn set_ai_transport(&mut self, transport: AiTransport, cx: &mut Context<Self>) {
        if self.ai_config.transport == transport {
            return;
        }
        self.ai_config.transport = transport;
        self.persist();
        cx.notify();
    }

    fn set_ai_cli_provider(&mut self, provider: AiCliProvider, cx: &mut Context<Self>) {
        if self.ai_config.cli_provider == provider {
            return;
        }
        self.ai_config.cli_provider = provider;
        self.persist();
        cx.notify();
    }

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.palette_open = false;
            // Re-enter on General so the page feels fresh each open.
            self.settings_section = SettingsSection::General;
        }
        cx.notify();
    }

    fn set_settings_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        if self.settings_section == section {
            return;
        }
        self.settings_section = section;
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        cx.notify();
    }

    fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        self.update_state = UpdateState::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = smol::unblock(update::check_latest).await;
            if this
                .update(cx, |app, cx| {
                    app.update_state = match res {
                        Ok(Some(info)) => UpdateState::Available(info),
                        Ok(None) => UpdateState::UpToDate,
                        // 只有手动点“检查更新”才把错误展示出来，自动检查静默。
                        Err(e) if manual => UpdateState::Error(e),
                        Err(_) => UpdateState::Idle,
                    };
                    cx.notify();
                })
                .is_err()
            {
                return;
            }
        })
        .detach();
    }

    fn start_update(&mut self, cx: &mut Context<Self>) {
        let info = match &self.update_state {
            UpdateState::Available(info) => info.clone(),
            _ => return,
        };
        self.update_state = UpdateState::Downloading(info.version.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = smol::unblock(move || update::download_and_install(&info)).await;
            if let Err(e) = res {
                let _ = this.update(cx, |app, cx| {
                    app.update_state = UpdateState::Error(e);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn render_update_button(&self, cx: &mut Context<Self>) -> Option<Button> {
        match &self.update_state {
            UpdateState::Available(info) => {
                let version = info.version.clone();
                let notes = info.notes.clone();
                let release_url = info.release_url.clone();
                let tooltip = format!(
                    "发现新版本 v{version}，点击下载并重启应用{}\n发布页：{release_url}",
                    if notes.is_empty() {
                        String::new()
                    } else {
                        format!("\n\n{}", notes.chars().take(240).collect::<String>())
                    }
                );
                Some(
                    Button::new("update-btn")
                        .primary()
                        .xsmall()
                        .label(format!("更新 v{version}"))
                        .tooltip(tooltip)
                        .on_click(cx.listener(|this, _, _w, cx| this.start_update(cx))),
                )
            }
            UpdateState::Downloading(version) => Some(
                Button::new("update-downloading")
                    .xsmall()
                    .disabled(true)
                    .label(format!("更新中 {version}…")),
            ),
            _ => None,
        }
    }

    fn update_status_line(&self, work: bool) -> String {
        match &self.update_state {
            UpdateState::Idle | UpdateState::Checking => {
                (if work { "Checking…" } else { "正在检查更新…" }).to_string()
            }
            UpdateState::UpToDate => format!(
                "v{} · {}",
                env!("CARGO_PKG_VERSION"),
                if work { "up to date" } else { "已是最新版本" }
            ),
            UpdateState::Available(info) => format!(
                "{} v{}（{} v{}）",
                if work { "New version" } else { "发现新版本" },
                info.version,
                if work { "current" } else { "当前" },
                env!("CARGO_PKG_VERSION")
            ),
            UpdateState::Downloading(version) => format!(
                "{} v{version}…",
                if work { "Downloading" } else { "正在下载" }
            ),
            UpdateState::Error(e) => e.clone(),
        }
    }

    /// Color for a rising (`up`) or falling move under the active convention.
    /// In work mode, always use muted tones so red/green do not stand out.
    fn chg_color(&self, up: bool, cx: &App) -> gpui::Hsla {
        if self.work_mode {
            return if up {
                cx.theme().muted_foreground
            } else {
                cx.theme().muted_foreground.opacity(0.65)
            };
        }
        match self.color_scheme {
            ColorScheme::Cn => {
                if up {
                    cx.theme().red
                } else {
                    cx.theme().green
                }
            }
            ColorScheme::Us => {
                if up {
                    cx.theme().green
                } else {
                    cx.theme().red
                }
            }
        }
    }

    fn select_symbol(&mut self, code: SharedString, cx: &mut Context<Self>) {
        if self.selected == code {
            self.palette_open = false;
            cx.notify();
            return;
        }
        self.selected = code;
        self.palette_open = false;
        self.persist();
        self.reload_chart(cx);
    }

    /// 从寻宝列表点选：必要时临时加入自选，并切到 3Y 以便对照多年高低。
    fn select_treasure_hit(&mut self, hit: &TreasureHit, cx: &mut Context<Self>) {
        let code = hit.code.clone();
        let display = display_name_str(&hit.name, &code);
        if !self.symbols.iter().any(|s| s.code == code) {
            self.symbols.push(Symbol {
                code: code.clone(),
                name: shared(display.clone()),
                last: hit.close,
                change_pct: 0.0,
                volume: 0,
                board: board_for_code(&code),
            });
            self.filtered_local = (0..self.symbols.len()).collect();
        } else if let Some(sym) = self.symbols.iter_mut().find(|s| s.code == code) {
            // 自选里若还是代码占位，用 hit 里更好的名字
            if !is_real_name(sym.name.as_ref(), &code) && is_real_name(&hit.name, &code) {
                sym.name = shared(hit.name.clone());
            }
        }
        // 多年对照：自动用 3Y（若已是 Max 则保留）
        if !matches!(self.range, ChartRange::Y3 | ChartRange::Max) {
            self.range = ChartRange::Y3;
        }
        // 多年对照需要日 K 视图；分时/分钟K 自动切回日 K。
        self.chart_kind = ChartKind::DayK;
        self.left_tab = LeftTab::Treasure;
        self.persist();
        self.select_symbol(shared(code.clone()), cx);

        // 名称仍是代码时，立刻拉一笔报价补中文名
        if !is_real_name(&hit.name, &code) {
            self.fill_names_for_codes(vec![code], cx);
        }
    }

    /// 用批量行情补全自选 / 寻宝结果中的中文名称。
    fn fill_names_for_codes(&mut self, codes: Vec<String>, cx: &mut Context<Self>) {
        if codes.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let codes2 = codes.clone();
            let result = smol::unblock(move || market::fetch_quotes(&codes2)).await;
            let Ok(sourced) = result else {
                return;
            };
            let _ = this.update(cx, |app, cx| {
                let mut hit_changed = false;
                let symbol_ix: std::collections::HashMap<String, usize> = app
                    .symbols
                    .iter()
                    .enumerate()
                    .map(|(ix, s)| (s.code.clone(), ix))
                    .collect();
                let treasure_ix: std::collections::HashMap<String, usize> = app
                    .treasure_hits
                    .iter()
                    .enumerate()
                    .map(|(ix, h)| (h.code.clone(), ix))
                    .collect();
                for t in &sourced.data {
                    if !is_real_name(&t.name, &t.code) {
                        continue;
                    }
                    if let Some(&ix) = symbol_ix.get(&t.code) {
                        let sym = &mut app.symbols[ix];
                        if !is_real_name(sym.name.as_ref(), &t.code) || sym.name.as_ref() != t.name {
                            sym.name = shared(t.name.clone());
                        }
                        if t.last > 0.0 {
                            sym.last = t.last;
                            sym.change_pct = t.change_pct;
                            sym.volume = t.volume;
                        }
                    }
                    if let Some(&ix) = treasure_ix.get(&t.code) {
                        let hit = &mut app.treasure_hits[ix];
                        if hit.name != t.name {
                            hit.name = t.name.clone();
                            hit_changed = true;
                        }
                    }
                }
                if hit_changed {
                    let cache = treasure::TreasureCache {
                        updated_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
                        universe: "watchlist+extended".into(),
                        hits: app.treasure_hits.clone(),
                    };
                    let _ = storage::save_treasure_cache(&cache);
                }
                app.persist();
                cx.notify();
            });
        })
        .detach();
    }

    /// 启动时若缓存名称是代码，后台补一次名称。
    fn enrich_treasure_names_if_needed(&mut self, cx: &mut Context<Self>) {
        let need: Vec<String> = self
            .treasure_hits
            .iter()
            .filter(|h| !is_real_name(&h.name, &h.code))
            .map(|h| h.code.clone())
            .collect();
        if !need.is_empty() {
            self.fill_names_for_codes(need, cx);
        }
    }

    fn set_left_tab(&mut self, tab: LeftTab, cx: &mut Context<Self>) {
        self.left_tab = tab;
        cx.notify();
    }

    fn set_treasure_pool(&mut self, pool: TreasurePool, cx: &mut Context<Self>) {
        if self.treasure_pool == pool {
            return;
        }
        self.treasure_pool = pool;
        self.persist();
        cx.notify();
    }

    fn set_treasure_fin(&mut self, fin: FinFilter, cx: &mut Context<Self>) {
        if self.treasure_fin == fin {
            return;
        }
        self.treasure_fin = fin;
        self.persist();
        cx.notify();
    }

    fn toggle_treasure_tab(&mut self, cx: &mut Context<Self>) {
        self.left_tab = match self.left_tab {
            LeftTab::Watchlist => LeftTab::Treasure,
            LeftTab::Treasure => LeftTab::Watchlist,
        };
        cx.notify();
    }

    /// 后台扫描：自选 ∪ 东财扩大池（市值前列）→ 深评 → Top100。
    fn start_treasure_scan(&mut self, cx: &mut Context<Self>) {
        if self.treasure_scanning {
            self.status = shared("寻宝扫描进行中…");
            cx.notify();
            return;
        }
        let watchlist: Vec<String> = self.symbols.iter().map(|s| s.code.clone()).collect();
        let pool = self.treasure_pool;
        let fin = self.treasure_fin;

        self.treasure_gen = self.treasure_gen.wrapping_add(1);
        let scan_id = self.treasure_gen;
        self.treasure_scanning = true;
        self.treasure_done = 0;
        self.treasure_total = 0;
        self.treasure_hits.clear();
        // 新扫描作废旧的可买清单
        self.scout_gen = self.scout_gen.wrapping_add(1);
        self.scout_picks.clear();
        self.scout_summary = shared("");
        self.scout_source = shared("");
        self.scout_running = false;
        self.scout_done = 0;
        self.scout_total = 0;
        self.treasure_list_expanded = false;
        self.treasure_status = shared(format!(
            "① 拉取 {} 池（{}）· 入榜 Top {TREASURE_TOP_N}…",
            pool.label(),
            fin.label()
        ));
        self.status = shared(format!(
            "🐭 寻宝 · {}池 · {} · 入榜{TREASURE_TOP_N}",
            pool.label(),
            fin.label()
        ));
        self.left_tab = LeftTab::Treasure;
        cx.notify();

        cx.spawn(async move |this, cx| {
            // 网络拉候选（指数成分动态拉取 / 市值池；财务分位过滤；失败回落内置表）
            let build = smol::unblock(move || {
                universe::build_scan_universe_for_pool(&watchlist, pool, fin)
            })
            .await;
            let codes = build.codes;
            let pool_src = build.source;
            let filter_note = build.filter_note;

            if codes.is_empty() {
                let _ = this.update(cx, |app, cx| {
                    if app.treasure_gen != scan_id {
                        return;
                    }
                    app.treasure_scanning = false;
                    app.treasure_status = shared(format!("没有可扫描代码 · {filter_note}"));
                    app.status = shared("寻宝失败：候选池为空");
                    cx.notify();
                });
                return;
            }

            let total = codes.len();
            let _ = this.update(cx, |app, cx| {
                if app.treasure_gen != scan_id {
                    return;
                }
                app.treasure_total = total;
                app.treasure_status = shared(format!(
                    "池 {total} 只（{pool_src}）· {filter_note} · 深评中 0/{total} · 将取 Top {TREASURE_TOP_N}"
                ));
                app.status = shared(format!("🐭 深评 {total} 只 · 源 {pool_src} · {filter_note}"));
                cx.notify();
            });

            let mut hits: Vec<TreasureHit> = Vec::new();
            for (i, code) in codes.into_iter().enumerate() {
                // 协作式取消
                let cancelled = this
                    .read_with(cx, |app, _| app.treasure_gen != scan_id)
                    .unwrap_or(true);
                if cancelled {
                    return;
                }

                let code_fetch = code.clone();
                let result = smol::unblock(move || {
                    market::fetch_klines_adjusted(&code_fetch, TREASURE_KLINE_LIMIT)
                })
                .await;

                if let Ok(sourced) = result {
                    let (_c, name, candles) = sourced.data;
                    // 空名保持空串，结束后用行情批量补中文名（勿用 code 冒充）
                    if let Some(hit) = treasure::analyze(&code, &name, &candles, sourced.source) {
                        hits.push(hit);
                    }
                }

                let done = i + 1;
                let _ = this.update(cx, |app, cx| {
                    if app.treasure_gen != scan_id {
                        return;
                    }
                    app.treasure_done = done;
                    app.treasure_total = total;
                    // 边扫边排，便于观察
                    let mut partial = hits.clone();
                    partial.sort_by(|a, b| {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    // 扫描中只展示暂定 Top N，避免列表过长卡顿
                    if partial.len() > TREASURE_TOP_N {
                        partial.truncate(TREASURE_TOP_N);
                    }
                    app.treasure_hits = partial;
                    app.treasure_status = shared(format!(
                        "深评 {done}/{total} · 暂定 Top {} · {pool_src}",
                        app.treasure_hits.len()
                    ));
                    if done == total || done % 10 == 0 {
                        app.status = shared(format!("🐭 深评 {done}/{total}"));
                    }
                    cx.notify();
                });

                if done < total {
                    Timer::after(TREASURE_SCAN_GAP).await;
                }
            }

            hits.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            // 最终只保留 Top N
            if hits.len() > TREASURE_TOP_N {
                hits.truncate(TREASURE_TOP_N);
            }

            // 批量行情补全名称（K 线源常无中文名；分批避免单次过长）
            let name_codes: Vec<String> = hits.iter().map(|h| h.code.clone()).collect();
            for chunk in name_codes.chunks(40) {
                let chunk = chunk.to_vec();
                if let Ok(sourced) = smol::unblock(move || market::fetch_quotes(&chunk)).await {
                    for t in sourced.data {
                        if !is_real_name(&t.name, &t.code) {
                            continue;
                        }
                        if let Some(hit) = hits.iter_mut().find(|h| h.code == t.code) {
                            hit.name = t.name;
                        }
                    }
                }
            }

            let updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            let cache = treasure::TreasureCache {
                updated_at: updated_at.clone(),
                universe: format!("{pool_src}/scan{total}/top{TREASURE_TOP_N}"),
                hits: hits.clone(),
            };
            let _ = storage::save_treasure_cache(&cache);

            let _ = this.update(cx, |app, cx| {
                if app.treasure_gen != scan_id {
                    return;
                }
                app.treasure_scanning = false;
                app.treasure_hits = hits;
                app.treasure_done = total;
                // 同步名称到已在自选里的同代码
                for hit in &app.treasure_hits {
                    if !is_real_name(&hit.name, &hit.code) {
                        continue;
                    }
                    if let Some(sym) = app.symbols.iter_mut().find(|s| s.code == hit.code) {
                        if !is_real_name(sym.name.as_ref(), &hit.code) {
                            sym.name = shared(hit.name.clone());
                        }
                    }
                }
                app.treasure_status = shared(format!(
                    "完成 · 深评 {total} · 入榜 {} · {pool_src} · {filter_note} · {updated_at}",
                    app.treasure_hits.len(),
                ));
                app.status = shared(format!(
                    "🐭 寻宝完成 · Top {} / 扫描 {total} · 正在筛可买…",
                    app.treasure_hits.len(),
                ));
                app.persist();
                cx.notify();
                // 扫完自动批量筛「可买观察」，避免用户一只只点
                if !app.treasure_hits.is_empty() {
                    app.start_scout_picks(cx);
                }
            });
        })
        .detach();
    }

    /// 从当前寻宝榜批量深评，筛出「可关注 / 观察」清单（本地规则；可选 LLM 整榜摘要）。
    fn start_scout_picks(&mut self, cx: &mut Context<Self>) {
        if self.scout_running {
            self.status = shared("可买筛分进行中…");
            cx.notify();
            return;
        }
        if self.treasure_scanning {
            self.status = shared("请等寻宝扫描结束后再筛可买");
            cx.notify();
            return;
        }
        if self.treasure_hits.is_empty() {
            self.scout_summary = shared("请先「开始搜罗」生成寻宝榜，再筛可买。");
            self.status = shared("无可筛标的 · 先搜罗");
            cx.notify();
            return;
        }

        let mut candidates: Vec<TreasureHit> = self.treasure_hits.clone();
        candidates.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if candidates.len() > SCOUT_CANDIDATE_N {
            candidates.truncate(SCOUT_CANDIDATE_N);
        }

        self.scout_gen = self.scout_gen.wrapping_add(1);
        let run_id = self.scout_gen;
        let total = candidates.len();
        self.scout_running = true;
        self.scout_done = 0;
        self.scout_total = total;
        self.scout_picks.clear();
        self.scout_summary = shared(format!(
            "对寻宝 Top {total} 做可买深评（位置+雷达+价位带）…"
        ));
        self.scout_source = shared(if self.work_mode {
            "Scoring…"
        } else {
            "本地规则筛分中"
        });
        self.left_tab = LeftTab::Treasure;
        self.status = shared(format!("🎯 筛可买 0/{total}"));
        cx.notify();

        let ai_cfg = self.ai_config.clone();
        let want_llm = ai_cfg.is_configured();

        cx.spawn(async move |this, cx| {
            let mut raw: Vec<ScoutPick> = Vec::new();
            for (i, hit) in candidates.into_iter().enumerate() {
                let cancelled = this
                    .read_with(cx, |app, _| app.scout_gen != run_id)
                    .unwrap_or(true);
                if cancelled {
                    return;
                }

                let code = hit.code.clone();
                let result = smol::unblock(move || {
                    market::fetch_klines_adjusted(&code, TREASURE_KLINE_LIMIT)
                })
                .await;

                if let Ok(sourced) = result {
                    let (_c, name, candles) = sourced.data;
                    let mut hit = hit;
                    if is_real_name(&name, &hit.code) {
                        hit.name = name;
                    }
                    if let Some(pick) = scout::evaluate(&hit, &candles) {
                        raw.push(pick);
                    }
                }

                let done = i + 1;
                let _ = this.update(cx, |app, cx| {
                    if app.scout_gen != run_id {
                        return;
                    }
                    app.scout_done = done;
                    app.scout_total = total;
                    // 边评边展示中间结果（Skip 不进列表）
                    app.scout_picks = scout::finalize_results(raw.clone());
                    app.scout_summary = shared(format!(
                        "深评 {done}/{total} · 暂定可关注/观察 {} 只",
                        app.scout_picks.len()
                    ));
                    if done == total || done % 5 == 0 {
                        app.status = shared(format!("🎯 筛可买 {done}/{total}"));
                    }
                    cx.notify();
                });

                if done < total {
                    Timer::after(TREASURE_SCAN_GAP).await;
                }
            }

            let picks = scout::finalize_results(raw);
            let local = scout::local_summary(&picks);

            let _ = this.update(cx, |app, cx| {
                if app.scout_gen != run_id {
                    return;
                }
                app.scout_picks = picks.clone();
                app.scout_summary = shared(local.clone());
                app.scout_source = shared(if app.work_mode {
                    "Local rules"
                } else {
                    "本地规则"
                });
                app.scout_done = total;
                if !want_llm {
                    app.scout_running = false;
                    let buy_n = app
                        .scout_picks
                        .iter()
                        .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
                        .count();
                    app.status = shared(format!(
                        "🎯 可关注 {buy_n} / 共 {} · 本地规则",
                        app.scout_picks.len()
                    ));
                    app.treasure_status = shared(format!(
                        "② 可买清单就绪 · 可关注 {buy_n} · 观察 {} · 本地",
                        app.scout_picks.len().saturating_sub(buy_n)
                    ));
                    app.finish_scout_ux(cx);
                } else {
                    app.scout_summary = shared(format!("{local}\n\n（LLM 整榜摘要生成中…）"));
                    app.status = shared("🎯 本地清单已出 · 请求 LLM 摘要…");
                    // 先应用 UX（打开第一只），摘要回来后再刷新文案
                    app.finish_scout_ux(cx);
                }
            });

            if !want_llm {
                return;
            }

            let picks_for_llm = picks.clone();
            let cfg = ai_cfg;
            let source_label = cfg.source_label();
            let res = smol::unblock(move || scout::llm_summary(&cfg, &picks_for_llm)).await;

            let _ = this.update(cx, |app, cx| {
                if app.scout_gen != run_id {
                    return;
                }
                app.scout_running = false;
                let buy_n = app
                    .scout_picks
                    .iter()
                    .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
                    .count();
                match res {
                    Ok(text) if !text.trim().is_empty() => {
                        app.scout_summary = shared(text);
                        app.scout_source = shared(source_label.clone());
                        app.status = shared(format!(
                            "🎯 可关注 {buy_n} / 共 {} · LLM",
                            app.scout_picks.len()
                        ));
                    }
                    Ok(_) => {
                        app.scout_summary = shared(local.clone());
                        app.scout_source = shared("本地规则 · LLM 空响应");
                        app.status = shared(format!(
                            "🎯 可关注 {buy_n} / 共 {} · 本地",
                            app.scout_picks.len()
                        ));
                    }
                    Err(e) => {
                        app.scout_summary = shared(format!(
                            "{local}\n\n（LLM 摘要失败：{e} · 已保留本地清单）"
                        ));
                        app.scout_source = shared("本地规则 · LLM 失败回退");
                        app.status = shared(format!(
                            "🎯 可关注 {buy_n} / 共 {} · 本地回退",
                            app.scout_picks.len()
                        ));
                    }
                }
                app.treasure_status = shared(format!(
                    "② 可买清单就绪 · 可关注 {buy_n} · 观察 {} · {}",
                    app.scout_picks.len().saturating_sub(buy_n),
                    app.scout_source
                ));
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_scout_picks(&mut self, cx: &mut Context<Self>) {
        if !self.scout_running {
            return;
        }
        self.scout_gen = self.scout_gen.wrapping_add(1);
        self.scout_running = false;
        self.scout_summary = shared(format!(
            "已取消筛分 · 保留 {} 条中间结果",
            self.scout_picks.len()
        ));
        self.status = shared("已取消可买筛分");
        cx.notify();
    }

    fn select_scout_pick(&mut self, pick: &ScoutPick, cx: &mut Context<Self>) {
        // 若在寻宝榜中，走完整寻宝选中（含 3Y 视图）；否则直接选代码
        if let Some(hit) = self
            .treasure_hits
            .iter()
            .find(|h| h.code == pick.code)
            .cloned()
        {
            self.select_treasure_hit(&hit, cx);
        } else {
            self.left_tab = LeftTab::Treasure;
            self.detail_tab = DetailTab::Treasure;
            self.select_symbol(shared(pick.code.clone()), cx);
        }
    }

    fn set_scout_only_buy_watch(&mut self, only: bool, cx: &mut Context<Self>) {
        if self.scout_only_buy_watch == only {
            return;
        }
        self.scout_only_buy_watch = only;
        cx.notify();
    }

    fn set_treasure_list_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if self.treasure_list_expanded == expanded {
            return;
        }
        self.treasure_list_expanded = expanded;
        cx.notify();
    }

    /// 当前过滤后的可买清单（视图用；不改动 `scout_picks` 源数据）。
    fn visible_scout_picks(&self) -> Vec<&ScoutPick> {
        self.scout_picks
            .iter()
            .filter(|p| {
                if self.scout_only_buy_watch {
                    p.verdict == ScoutVerdict::BuyWatch
                } else {
                    true
                }
            })
            .collect()
    }

    /// 筛分结束后的 UX：过滤回退、默认打开第一只、切到底栏寻宝。
    fn finish_scout_ux(&mut self, cx: &mut Context<Self>) {
        let buy_n = self
            .scout_picks
            .iter()
            .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
            .count();
        // 没有「可关注」时别留空列表，自动显示观察。
        if buy_n == 0 {
            self.scout_only_buy_watch = false;
        } else {
            self.scout_only_buy_watch = true;
        }
        self.treasure_list_expanded = false;
        self.detail_tab = DetailTab::Treasure;
        self.left_tab = LeftTab::Treasure;

        let first = self
            .visible_scout_picks()
            .first()
            .map(|p| (*p).clone())
            .or_else(|| self.scout_picks.first().cloned());
        if let Some(pick) = first {
            // 自动打开第一只，图表与底栏价位立刻可读。
            self.select_scout_pick(&pick, cx);
        } else {
            cx.notify();
        }
    }

    fn cancel_treasure_scan(&mut self, cx: &mut Context<Self>) {
        if !self.treasure_scanning {
            return;
        }
        self.treasure_gen = self.treasure_gen.wrapping_add(1);
        self.treasure_scanning = false;
        self.treasure_status = shared(format!(
            "已取消 · 保留 {} 条中间结果",
            self.treasure_hits.len()
        ));
        self.status = shared("寻宝扫描已取消");
        cx.notify();
    }

    fn add_symbol(&mut self, code: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.symbols.iter().any(|s| s.code == code) {
            self.select_symbol(shared(code), cx);
            return;
        }
        self.symbols.push(Symbol {
            code: code.clone(),
            name: shared(name),
            last: 0.0,
            change_pct: 0.0,
            volume: 0,
            board: board_for_code(&code),
        });
        self.filtered_local = (0..self.symbols.len()).collect();
        self.persist();
        self.select_symbol(shared(code), cx);
        self.palette_query.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
    }

    /// 从自选里删除指定代码；删除的是当前选中标的时，自动选中相邻标的。
    fn remove_symbol(&mut self, code: &str, cx: &mut Context<Self>) {
        if self.symbols.len() <= 1 {
            self.status = shared(if self.work_mode {
                "At least one symbol"
            } else {
                "至少保留一只自选"
            });
            cx.notify();
            return;
        }
        let Some(pos) = self.symbols.iter().position(|s| s.code == code) else {
            return;
        };
        let was_selected = self.selected.as_ref() == code;
        self.symbols.remove(pos);
        self.filtered_local = (0..self.symbols.len()).collect();
        if was_selected {
            self.selected = shared(
                self.symbols
                    .get(pos)
                    .or_else(|| self.symbols.last())
                    .map(|s| s.code.clone())
                    .unwrap_or_default(),
            );
        }
        // Drop from status-bar pins if present.
        if let Some(ix) = self.status_bar_codes.iter().position(|c| c == code) {
            self.status_bar_codes.remove(ix);
            if self.status_bar_active == code {
                self.status_bar_active = self
                    .status_bar_codes
                    .first()
                    .cloned()
                    .unwrap_or_default();
            }
        }
        self.normalize_status_bar_state();
        self.persist();
        self.sync_status_bar();
        if was_selected {
            self.reload_klines(cx);
        }
        cx.notify();
    }

    fn remove_selected_from_watchlist(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        self.remove_symbol(&code, cx);
    }

    fn set_range(&mut self, range: ChartRange, cx: &mut Context<Self>) {
        if self.range == range && matches!(self.chart_kind, ChartKind::DayK) {
            return;
        }
        self.range = range;
        self.chart_kind = ChartKind::DayK;
        self.persist();
        self.reload_klines(cx);
    }

    fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = !self.palette_open;
        if self.palette_open {
            self.palette_hits.clear();
            self.filtered_local = (0..self.symbols.len()).collect();
            self.palette_query.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            window.focus(&self.palette_focus);
            self.palette_query.update(cx, |input, cx| {
                input.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn on_palette_query_changed(&mut self, q: &str, cx: &mut Context<Self>) {
        let q_l = q.trim().to_lowercase();
        if q_l.is_empty() {
            self.filtered_local = (0..self.symbols.len()).collect();
            self.palette_hits.clear();
            cx.notify();
            return;
        }
        self.filtered_local = self
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.code.to_lowercase().contains(&q_l)
                    || s.name.to_lowercase().contains(&q_l)
                    || s.board.to_lowercase().contains(&q_l)
            })
            .map(|(i, _)| i)
            .collect();

        // Async remote search
        let query = q.trim().to_string();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || market::search_symbols(&query, 12)).await;
            this.update(cx, |app, cx| {
                if let Ok(sourced) = result {
                    app.palette_hits = sourced.data;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn current_symbol(&self) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.code == self.selected.as_ref())
    }

    fn chart_paint_data(&self, cx: &App) -> ChartPaintData {
        let theme = cx.theme();
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let minute_matched = matches!(self.chart_kind, ChartKind::Intraday)
            && self
                .minute_code
                .as_ref()
                .is_some_and(|c| c == self.selected.as_ref());
        // While loading a new series, keep painting the previous candles to avoid a blank flash.
        let show_series = if matches!(self.chart_kind, ChartKind::Intraday) {
            minute_matched && matched
        } else {
            matched || (self.loading && !self.candles.is_empty())
        };
        let (start, end) = if show_series {
            self.chart_visible_range()
        } else {
            (0, 0)
        };
        let candles = if show_series && end > start {
            self.candles[start..end].to_vec()
        } else {
            Vec::new()
        };
        let ma = if show_series && end > start {
            self.ma.slice(start, end)
        } else {
            MaSeries::default()
        };
        let work = self.work_mode;
        let is_intraday = matches!(self.chart_kind, ChartKind::Intraday);
        let macd = if show_series && end > start && !is_intraday && self.show_macd && !work {
            let s = self.macd.slice(start, end);
            Some(MacdPaintData {
                dif: s.dif,
                dea: s.dea,
                hist: s.hist,
                dif_color: theme.foreground,
                dea_color: theme.blue,
                axis_color: theme.muted_foreground,
                bullish: self.chg_color(true, cx),
                bearish: self.chg_color(false, cx),
            })
        } else {
            None
        };
        let boll = if show_series && end > start && !is_intraday && self.show_boll && !work {
            let s = self.boll.slice(start, end);
            Some(BollPaintData {
                upper: s.upper,
                mid: s.mid,
                lower: s.lower,
                upper_color: theme.cyan.opacity(0.9),
                mid_color: theme.muted_foreground.opacity(0.8),
                lower_color: theme.magenta.opacity(0.9),
            })
        } else {
            None
        };
        // 画线：当前标的的线（裁剪到可见区间，保持锚点索引为切片内坐标）。
        let mut lines = Vec::new();
        if show_series && end > start && !work {
            let owned = self
                .chart_lines
                .get(self.selected.as_ref())
                .cloned()
                .unwrap_or_default();
            for line in owned {
                if line.from.0 < start || line.to.0 < start {
                    continue;
                }
                if line.from.0 >= end || line.to.0 >= end {
                    continue;
                }
                lines.push(TrendLine {
                    from: (line.from.0 - start, line.from.1),
                    to: (line.to.0 - start, line.to.1),
                    color_ix: line.color_ix,
                });
            }
            if let Some(draft) = self.draft_line {
                if draft.from.0 >= start && draft.from.0 < end && draft.to.0 >= start && draft.to.0 < end {
                    lines.push(TrendLine {
                        from: (draft.from.0 - start, draft.from.1),
                        to: (draft.to.0 - start, draft.to.1),
                        color_ix: draft.color_ix,
                    });
                }
            }
        }
        let hover_ix = if matched {
            self.hover_ix.and_then(|ix| {
                if ix >= start && ix < end {
                    Some(ix - start)
                } else {
                    None
                }
            })
        } else {
            None
        };
        let minute = if matches!(self.chart_kind, ChartKind::Intraday) {
            self.minute_paint_data(cx)
        } else {
            None
        };
        ChartPaintData {
            candles,
            ma,
            macd,
            boll,
            lines,
            line_colors: chart_line_palette(theme),
            minute,
            show_ma5: self.show_ma5,
            show_ma10: self.show_ma10,
            show_ma20: self.show_ma20,
            show_ma60: self.show_ma60,
            show_volume: self.show_volume && !work,
            hover_ix,
            style: if work {
                ChartStyle::Area
            } else {
                ChartStyle::Candles
            },
            bullish: self.chg_color(true, cx),
            bearish: self.chg_color(false, cx),
            line_color: if work {
                theme.blue
            } else {
                theme.foreground
            },
            area_fill: theme.blue.opacity(0.18),
            border: theme.border,
            ma5_color: if work {
                theme.muted_foreground.opacity(0.85)
            } else {
                theme.yellow
            },
            ma10_color: if work {
                theme.muted_foreground.opacity(0.65)
            } else {
                theme.blue
            },
            ma20_color: if work {
                theme.muted_foreground.opacity(0.45)
            } else {
                theme.magenta
            },
            ma60_color: if work {
                theme.muted_foreground.opacity(0.35)
            } else {
                theme.cyan
            },
            crosshair: theme.muted_foreground.opacity(0.7),
            axis_color: theme.muted_foreground,
        }
    }

    fn minute_paint_data(&self, cx: &App) -> Option<MinutePaintData> {
        let theme = cx.theme();
        let matched = self
            .minute_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref())
            && self
                .candles_code
                .as_ref()
                .is_some_and(|c| c == self.selected.as_ref());
        if !matched {
            return None;
        }
        let m = self.minute.as_ref()?;
        if m.is_empty() {
            return None;
        }
        let (start, end) = self.chart_visible_range();
        if start >= end || end > m.points.len() {
            return None;
        }
        let mut prices = Vec::with_capacity(end - start);
        let mut avg = Vec::with_capacity(end - start);
        let mut volumes = Vec::with_capacity(end - start);
        for i in start..end {
            let p = &m.points[i];
            prices.push(p.price);
            avg.push(p.avg_price());
            volumes.push(p.minute_volume(i.checked_sub(1).map(|j| &m.points[j])));
        }
        let hover_ix = self.hover_ix.and_then(|ix| {
            if ix >= start && ix < end {
                Some(ix - start)
            } else {
                None
            }
        });
        Some(MinutePaintData {
            prices,
            avg,
            volumes,
            prev_close: m.prev_close,
            hover_ix,
            bullish: self.chg_color(true, cx),
            bearish: self.chg_color(false, cx),
            avg_color: if self.work_mode {
                theme.muted_foreground.opacity(0.85)
            } else {
                theme.yellow
            },
            border: theme.border,
            crosshair: theme.muted_foreground.opacity(0.7),
            axis_color: theme.muted_foreground,
        })
    }

    /// Display id: real code, or stable camouflage label in work mode.
    fn display_code(&self, code: &str) -> String {
        if self.work_mode {
            let name = self
                .symbols
                .iter()
                .find(|s| s.code == code)
                .map(|s| s.name.as_ref())
                .unwrap_or("");
            if self.work_identity_reveal {
                if is_real_name(name, code) {
                    format!("{name} · {code}")
                } else {
                    code.to_string()
                }
            } else {
                disguise_label(code, name)
            }
        } else {
            code.to_string()
        }
    }

    fn apply_index_ticks(&mut self, ticks: &[(String, String, f64, f64)]) {
        for (code, name, last, change_pct) in ticks {
            let snap = IndexSnap {
                last: *last,
                change_pct: *change_pct,
            };
            let n = name.as_str();
            if n.contains("上证") || (*code == "000001" && n.contains("指数")) {
                self.index_sh = Some(snap);
            } else if n.contains("沪深300") || code == "000300" {
                self.index_hs300 = Some(snap);
            } else if n.contains("创业板") || code == "399006" {
                self.index_cyb = Some(snap);
            } else if code == "000001" && *last > 1000.0 {
                // 上证点位通常 >1000；个股平安银行不会这么高
                self.index_sh = Some(snap);
            }
        }
    }

    /// Price base for index rebased display (first visible close, else last).
    fn price_base(&self) -> f64 {
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        if matched {
            let (start, end) = self.chart_visible_range();
            if end > start {
                if let Some(c) = self.candles.get(start) {
                    if c.close > 0.0 {
                        return c.close;
                    }
                }
            }
            if let Some(c) = self.candles.last() {
                if c.close > 0.0 {
                    return c.close;
                }
            }
        }
        self.current_symbol()
            .map(|s| s.last)
            .filter(|v| *v > 0.0)
            .unwrap_or(1.0)
    }

    fn format_value(&self, price: f64) -> String {
        if self.work_mode {
            format_index(disguise_index(price, self.price_base()))
        } else {
            format_price(price)
        }
    }

    fn format_change(&self, pct: f64) -> String {
        if self.work_mode {
            // Show as index points vs 100, not a stock-style %.
            format!("{pct:+.2}")
        } else {
            format_pct(pct)
        }
    }

    /// Work-mode status never mentions quotes / vendors / Chinese stock jargon.
    fn work_status_line(&self) -> String {
        if self.loading {
            return "loading series…".into();
        }
        if self.treasure_scanning {
            return format!("job {}/{}", self.treasure_done, self.treasure_total);
        }
        let t = chrono::Local::now().format("%H:%M:%S");
        if self.quote_fail_streak > 0 {
            return format!("sync retry · {t}");
        }
        format!("sync ok · src-a · {t}")
    }

    fn max_watchlist_volume(&self) -> u64 {
        self.symbols.iter().map(|s| s.volume).max().unwrap_or(0)
    }

    /// 0..1 volume share vs busiest row (looks like load, not 手/万).
    fn load_factor(volume: u64, max_vol: u64) -> f64 {
        if max_vol == 0 {
            0.0
        } else {
            (volume as f64 / max_vol as f64).clamp(0.0, 1.0)
        }
    }

    /// |涨跌幅| → CPU%（波动大 = 更忙）。约 0%→8%，5%→48%，10%→88%。
    fn sys_cpu_pct(change_pct: f64) -> f64 {
        (8.0 + change_pct.abs() * 8.0).clamp(3.0, 96.0)
    }

    /// 成交量 → 假网速 MB/s（看起来像网络吞吐）。
    fn sys_net_mbs(volume: u64, max_vol: u64) -> f64 {
        let load = Self::load_factor(volume, max_vol);
        (0.4 + load * 24.0 + (volume % 97) as f64 * 0.02).clamp(0.2, 48.0)
    }

    /// RSS MB from volume + stable code salt.
    fn sys_rss_mb(code: &str, volume: u64, max_vol: u64) -> u32 {
        let salt = code.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        let base = 48 + (salt % 180);
        let vol = (Self::load_factor(volume, max_vol) * 720.0) as u32;
        base + vol
    }

    fn current_signal(&self) -> Option<signals::SignalSnapshot> {
        self.candles_code
            .as_ref()
            .is_some_and(|code| code == self.selected.as_ref())
            .then(|| signals::analyze(&self.candles))
            .flatten()
    }

    fn spark_closes(&self) -> Vec<f64> {
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        if !matched || self.candles.is_empty() {
            return Vec::new();
        }
        let (start, end) = self.chart_visible_range();
        let slice = if end > start {
            &self.candles[start..end]
        } else {
            self.candles.as_slice()
        };
        // Cap points so the spark stays readable.
        let n = slice.len();
        let step = (n / 80).max(1);
        slice
            .iter()
            .step_by(step)
            .map(|c| c.close)
            .collect()
    }

    // ---------- UI pieces ----------

    /// Full-page service metrics + host panel (work mode only).
    ///
    /// Right-hand CPU/mem/disk/net/process stats are **derived from real quotes**,
    /// remapped so they read as system telemetry (see `sys_*` helpers).
    fn render_work_dashboard(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let max_vol = self.max_watchlist_volume();
        let spark = self.spark_closes();
        let sel_alias = self.display_code(selected.as_ref());
        let sel_sym = self.current_symbol();
        let p50 = sel_sym
            .filter(|s| s.last > 0.0)
            .map(|s| format!("{}ms", format_price(s.last)))
            .unwrap_or_else(|| "--".into());
        let sel_identity = sel_sym
            .map(|s| format!("{} · {} · 现价 {}", s.code, s.name, format_price(s.last)))
            .unwrap_or_else(|| "identity unavailable".into());
        let delta = sel_sym
            .map(|s| format!("{:+.2}%", s.change_pct))
            .unwrap_or_else(|| "--".into());
        let load = sel_sym
            .map(|s| format!("{:.2}", Self::load_factor(s.volume, max_vol)))
            .unwrap_or_else(|| "--".into());
        let status = self.work_status_line();
        let range_label = self.range.label();
        let pts = if spark.is_empty() {
            self.candles.len()
        } else {
            spark.len()
        };
        let line = cx.theme().blue;
        let fill = cx.theme().blue.opacity(0.16);
        let border = cx.theme().border;
        let signal = self.current_signal();
        let health = signal
            .as_ref()
            .map(|s| format!("{:.0}%", s.score))
            .unwrap_or_else(|| "--".into());
        let service_state = signal
            .as_ref()
            .map(|s| s.regime.service_state())
            .unwrap_or("warming");

        v_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    // ── left: service table ──
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(360.))
                            .min_h_0()
                            .h_full()
                            .overflow_hidden()
                            .child(
                                h_flex()
                                    .h(px(32.))
                                    .flex_shrink_0()
                                    .px_3()
                                    .items_center()
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().sidebar)
                                    .child(
                                        div()
                                            .w(px(210.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("service"),
                                    )
                                    .child(
                                        div()
                                            .id("work-p50-header")
                                            .w(px(90.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("p50")
                                            .tooltip(|window, cx| {
                                                Tooltip::new(
                                                    "owner key · p50=现价 · drift=涨跌幅 · health=策略评分",
                                                )
                                                .build(window, cx)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .w(px(74.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("drift"),
                                    )
                                    .child(
                                        div()
                                            .w(px(58.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("load"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{} workers", self.symbols.len())),
                                    ),
                            )
                            // Scroll on a plain div (more reliable than v_flex + overflow).
                            .child(
                                div()
                                    .id("work-metrics-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .w_full()
                                    .overflow_y_scroll()
                                    .children(self.symbols.iter().enumerate().map(|(ix, sym)| {
                                        let is_selected = sym.code == selected.as_ref();
                                        let code = shared(sym.code.clone());
                                        let alias = self.display_code(&sym.code);
                                        let p50 = if sym.last > 0.0 {
                                            format!("{}ms", format_price(sym.last))
                                        } else {
                                            "--".into()
                                        };
                                        let delta = format!("{:+.2}%", sym.change_pct);
                                        let load =
                                            format!("{:.2}", Self::load_factor(sym.volume, max_vol));
                                        let identity = format!(
                                            "{} · {} · 现价 {}",
                                            sym.code,
                                            sym.name,
                                            format_price(sym.last)
                                        );

                                        div()
                                            .id(("work-row", ix))
                                            .h(px(34.))
                                            .w_full()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .flex_shrink_0()
                                            .cursor_pointer()
                                            .border_b_1()
                                            .border_color(cx.theme().border.opacity(0.3))
                                            .when(is_selected, |this| {
                                                this.bg(cx.theme().accent.opacity(0.14))
                                            })
                                            .hover(|this| this.bg(cx.theme().accent.opacity(0.08)))
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(identity.clone()).build(window, cx)
                                            })
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.select_symbol(code.clone(), cx);
                                            }))
                                            .child(
                                                div()
                                                    .w(px(210.))
                                                    .text_sm()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .truncate()
                                                    .child(alias),
                                            )
                                            .child(
                                                div()
                                                    .w(px(90.))
                                                    .text_sm()
                                                    .font_medium()
                                                    .text_color(cx.theme().foreground)
                                                    .child(p50),
                                            )
                                            .child(
                                                div()
                                                    .w(px(74.))
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(delta),
                                            )
                                            .child(
                                                div()
                                                    .w(px(58.))
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(load),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(if is_selected { "run" } else { "·" }),
                                            )
                                    })),
                            ),
                    )
                    // ── right: host / process panel (real data, system skin) ──
                    .child(self.render_work_system_panel(cx)),
            )
            // footer: selected + sparkline
            .child(
                v_flex()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .child(
                        h_flex()
                            .h(px(32.))
                            .flex_shrink_0()
                            .px_3()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.5))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("selected"),
                                    )
                                    .child(
                                        div()
                                            .id("work-selected-alias")
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(sel_alias)
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(sel_identity.clone()).build(window, cx)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("p50 {p50}")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("Δ {delta}")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("load {load}")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("health {health} · {service_state}")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("window {range_label} · {pts} pts")),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .children(ChartRange::all().map(|range| {
                                        let active = self.range == range;
                                        Button::new(("work-range", range as u32))
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(range.label())
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.set_range(range, cx);
                                            }))
                                    }))
                                    .child(
                                        div()
                                            .ml_2()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(status),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("work-spark")
                            .h(px(72.))
                            .w_full()
                            .px_2()
                            .pb_2()
                            .child(
                                div()
                                    .id("work-spark-surface")
                                    .size_full()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().background)
                                    .overflow_hidden()
                                    .child({
                                        let closes = spark.clone();
                                        canvas(
                                            move |bounds, _, _| bounds,
                                            move |bounds, _, window, _cx| {
                                                paint_sparkline(
                                                    bounds, &closes, line, fill, border, window,
                                                );
                                            },
                                        )
                                        .size_full()
                                    }),
                            ),
                    ),
            )
    }

    /// Right column: host gauges (major indices) + process table + journal.
    fn render_work_system_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let max_vol = self.max_watchlist_volume();
        let sel = self.current_symbol();
        let host_name = sel
            .map(|s| self.display_code(&s.code))
            .unwrap_or_else(|| "host".into());
        let (net_in, net_out) = if let Some(s) = sel {
            (
                Self::sys_net_mbs(s.volume, max_vol),
                Self::sys_net_mbs(s.volume.saturating_mul(3) / 4, max_vol),
            )
        } else {
            (1.2, 0.8)
        };

        // Major-index direction is encoded around a neutral 50% telemetry baseline.
        let sh = self.index_sh;
        let hs300 = self.index_hs300;
        let cyb = self.index_cyb;
        let telemetry = |pct: f64| (50.0 + pct * 12.0).clamp(5.0, 95.0);

        // Sort processes by abs change for top talkers.
        let mut procs: Vec<&Symbol> = self.symbols.iter().collect();
        procs.sort_by(|a, b| {
            b.change_pct
                .abs()
                .partial_cmp(&a.change_pct.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_procs: Vec<&Symbol> = procs.into_iter().take(12).collect();

        let t = chrono::Local::now().format("%H:%M:%S").to_string();
        let sh_load = sh.map(|s| telemetry(s.change_pct));
        let hs300_load = hs300.map(|s| telemetry(s.change_pct));
        let cyb_load = cyb.map(|s| telemetry(s.change_pct));
        let sh_pct = sh_load.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "--".into());
        let hs300_pct = hs300_load
            .map(|v| format!("{v:.0}%"))
            .unwrap_or_else(|| "--".into());
        let cyb_pct = cyb_load.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "--".into());
        let gauge_value = |snap: Option<IndexSnap>, unit: &str| match snap {
            Some(s) if self.work_identity_reveal => {
                format!("{} {}", s.point_label(), s.pct_label())
            }
            Some(s) => format!("{}{unit}", s.point_label()),
            None => "--".into(),
        };
        let gauge_tip = |name: &str, snap: Option<IndexSnap>| match snap {
            Some(s) => format!("{name} · {} · {}", s.point_label(), s.pct_label()),
            None => format!("{name} · unavailable"),
        };
        let market_changes: Vec<f64> = [sh, hs300, cyb]
            .into_iter()
            .flatten()
            .map(|s| s.change_pct)
            .collect();
        let market_avg = if market_changes.is_empty() {
            None
        } else {
            Some(market_changes.iter().sum::<f64>() / market_changes.len() as f64)
        };
        let journal = [
            format!("{t}  scheduler tick · node={host_name}"),
            format!("{t}  sample cpu={sh_pct} mem={hs300_pct} disk={cyb_pct}"),
            format!("{t}  net rx={net_in:.1} tx={net_out:.1} MB/s"),
            format!(
                "{t}  cluster nodes={}",
                self.symbols.len()
            ),
            format!("{t}  gc pause ok · heap stable"),
            format!("{t}  worker pool active"),
        ];

        v_flex()
            .w(px(340.))
            .min_w(px(300.))
            .max_w(px(380.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_hidden()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h(px(32.))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.work_identity_reveal { "大盘" } else { "host" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(if self.work_identity_reveal {
                                "沪深核心指数".to_string()
                            } else {
                                host_name.clone()
                            }),
                    ),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .p_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(sys_gauge(
                        1,
                        if self.work_identity_reveal { "上证" } else { "cpu" },
                        gauge_value(sh, "MHz"),
                        sh_load.unwrap_or(20.0),
                        gauge_tip("上证综指", sh),
                        cx,
                    ))
                    .child(sys_gauge(
                        2,
                        if self.work_identity_reveal { "沪深" } else { "mem" },
                        gauge_value(hs300, "MB"),
                        hs300_load.unwrap_or(20.0),
                        gauge_tip("沪深300", hs300),
                        cx,
                    ))
                    .child(sys_gauge(
                        3,
                        if self.work_identity_reveal { "创业" } else { "disk" },
                        gauge_value(cyb, "IOPS"),
                        cyb_load.unwrap_or(20.0),
                        gauge_tip("创业板指", cyb),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.work_identity_reveal { "大盘" } else { "net" }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(if self.work_identity_reveal {
                                        market_avg
                                            .map(|v| format!("平均 {v:+.2}%"))
                                            .unwrap_or_else(|| "--".into())
                                    } else {
                                        format!("↓{net_in:.1}  ↑{net_out:.1} MB/s")
                                    }),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(28.))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .w(px(160.))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("process"),
                    )
                    .child(
                        div()
                            .w(px(48.))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("cpu"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("rss"),
                    ),
            )
            .child(
                div()
                    .id("work-proc-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .children(top_procs.into_iter().enumerate().map(|(ix, sym)| {
                        let is_selected = sym.code == self.selected.as_ref();
                        let code = shared(sym.code.clone());
                        let name = self.display_code(&sym.code);
                        let proc_cpu = Self::sys_cpu_pct(sym.change_pct);
                        let rss = Self::sys_rss_mb(&sym.code, sym.volume, max_vol);

                        div()
                            .id(("work-proc", ix))
                            .h(px(28.))
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.25))
                            .when(is_selected, |this| {
                                this.bg(cx.theme().accent.opacity(0.14))
                            })
                            .hover(|this| this.bg(cx.theme().accent.opacity(0.08)))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.select_symbol(code.clone(), cx);
                            }))
                            .child(
                                div()
                                    .w(px(160.))
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .w(px(48.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{proc_cpu:.0}%")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{rss}M")),
                            )
                    })),
            )
            .child(
                v_flex()
                    .h(px(120.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .p_2()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .mb_1()
                            .child("journal"),
                    )
                    .children(journal.into_iter().map(|line| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .truncate()
                            .child(line)
                    })),
            )
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        TitleBar::new().child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .px_3()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when(work, |row| {
                            row.child(
                                Button::new("work-identity-map")
                                    .ghost()
                                    .xsmall()
                                    .label(if self.work_identity_reveal { "Hide" } else { "Map" })
                                    .tooltip(if self.work_identity_reveal {
                                        "Hide stock identity mapping"
                                    } else {
                                        "Temporarily map services to stock names and codes"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.toggle_work_identity(cx);
                                    })),
                            )
                        })
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(if work { "Workspace" } else { "Stock" }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "local" } else { "A股分析" }),
                        )
                        .when(!work, |row| {
                            row.child(
                                div()
                                    .text_xs()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.data_source.clone()),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("work-mode")
                                .xsmall()
                                .when(work, |b| b.primary())
                                .when(!work, |b| b.ghost())
                                .label(if work { "Focus" } else { "工作" })
                                .tooltip(if work {
                                    "Exit focus layout · ⌘⇧W"
                                } else {
                                    "工作模式：中性配色与文案 · ⌘⇧W"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_work_mode(window, cx);
                                })),
                        )
                        .when(!work, |row| {
                            row.child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("涨跌色"),
                                    )
                                    .children([ColorScheme::Cn, ColorScheme::Us].map(|scheme| {
                                        let active = self.color_scheme == scheme;
                                        let id = match scheme {
                                            ColorScheme::Cn => "color-scheme-cn",
                                            ColorScheme::Us => "color-scheme-us",
                                        };
                                        Button::new(id)
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(scheme.short_label())
                                            .tooltip(scheme.label())
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.set_color_scheme(scheme, cx);
                                            }))
                                    })),
                            )
                        })
                        .child(
                            Button::new("refresh")
                                .ghost()
                                .xsmall()
                                .label(if work { "Sync" } else { "刷新" })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.refresh_all(cx);
                                })),
                        )
                        .when(!work, |row| {
                            row.child(
                                Button::new("treasure-btn")
                                    .ghost()
                                    .xsmall()
                                    .label("🐭 寻宝")
                                    .tooltip("多窗口历史低位 · ⌘T")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_left_tab(LeftTab::Treasure, cx);
                                    })),
                            )
                        })
                        .child(
                            Button::new("cmd-palette-btn")
                                .ghost()
                                .xsmall()
                                .label(if work { "Find" } else { "⌘K 搜索" })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_palette(window, cx);
                                })),
                        )
                        .when(!work, |row| row.children(self.render_update_button(cx)))
                        .child(
                            Button::new("settings-btn")
                                .ghost()
                                .xsmall()
                                .when(self.settings_open, |b| b.primary())
                                .label(if self.settings_open {
                                    if work {
                                        "Back"
                                    } else {
                                        "返回"
                                    }
                                } else if work {
                                    "Prefs"
                                } else {
                                    "设置"
                                })
                                .tooltip(if work {
                                    "Preferences · ⌘,"
                                } else {
                                    "设置 · ⌘,"
                                })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.toggle_settings(cx);
                                })),
                        ),
                ),
        )
    }

    fn render_settings_status_bar(
        &self,
        work: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = self.status_bar_enabled;
        let pinned = self.status_bar_codes.clone();
        let pin_count = pinned.len();

        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "Menu bar quotes" } else { "菜单栏行情" }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.9))
                    .child(if work {
                        format!(
                            "Pin up to {STATUS_BAR_MAX_CODES} watchlist symbols. All pinned quotes show together in the macOS menu bar (e.g. A · B · C). Click a row in the dropdown to open that symbol."
                        )
                    } else {
                        format!(
                            "从自选固定最多 {STATUS_BAR_MAX_CODES} 只；菜单栏会同时显示全部固定标的（例：比亚迪-0.1% · 楚天+0.5%）。点下拉项可打开对应股票。Windows/Linux 暂不支持。"
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("set-statusbar-off")
                            .xsmall()
                            .when(!enabled, |b| b.primary())
                            .when(enabled, |b| b.ghost())
                            .label(if work { "Off" } else { "关闭" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_status_bar_enabled(false, cx);
                            })),
                    )
                    .child(
                        Button::new("set-statusbar-on")
                            .xsmall()
                            .when(enabled, |b| b.primary())
                            .when(!enabled, |b| b.ghost())
                            .label(if work { "On" } else { "开启" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_status_bar_enabled(true, cx);
                            })),
                    ),
            )
            .when(enabled, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if work {
                            format!("Pinned {pin_count}/{STATUS_BAR_MAX_CODES} · click to pin/unpin · all show in menu bar")
                        } else {
                            format!("已固定 {pin_count}/{STATUS_BAR_MAX_CODES} · 点击切换固定 · 全部同时显示在菜单栏")
                        }),
                )
                .child(
                    // Vertical list: name (left) + code (muted) + pin state.
                    // Horizontal wrap chips looked cramped and double-coded ETFs.
                    v_flex()
                        .gap_0()
                        .max_w(px(480.))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.5))
                        .rounded(px(6.))
                        .overflow_hidden()
                        .children(self.symbols.iter().enumerate().map(|(ix, sym)| {
                            let code = sym.code.clone();
                            let is_pinned = pinned.iter().any(|c| c == &code);
                            let name_raw = sym.name.as_ref();
                            let (name_show, code_show) = if work {
                                (
                                    disguise_label(&sym.code, name_raw),
                                    String::new(),
                                )
                            } else if is_real_name(name_raw, &sym.code) {
                                (
                                    short_status_name(name_raw, &sym.code),
                                    sym.code.clone(),
                                )
                            } else {
                                (sym.code.clone(), String::new())
                            };
                            let pin_hint = if work {
                                if is_pinned { "pinned" } else { "pin" }
                            } else if is_pinned {
                                "已固定"
                            } else {
                                "固定"
                            };
                            let row_id = SharedString::from(format!("sb-pin-{}", sym.code));
                            div()
                                .id(row_id)
                                .w_full()
                                .h(px(32.))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .when(ix > 0, |r| {
                                    r.border_t_1()
                                        .border_color(cx.theme().border.opacity(0.35))
                                })
                                .when(is_pinned, |r| {
                                    r.bg(cx.theme().accent.opacity(0.16))
                                })
                                .hover(|r| r.bg(cx.theme().accent.opacity(0.10)))
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.toggle_status_bar_code(&code, cx);
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child(name_show),
                                )
                                .when(!code_show.is_empty(), |r| {
                                    r.child(
                                        div()
                                            .text_xs()
                                            .font_family("Menlo")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(code_show),
                                    )
                                })
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(if is_pinned {
                                            cx.theme().accent_foreground
                                        } else {
                                            cx.theme().muted_foreground.opacity(0.8)
                                        })
                                        .child(pin_hint),
                                )
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child(if work {
                            "Highlighted = pinned (shown in menu bar) · click to toggle."
                        } else {
                            "高亮 = 已固定并显示在菜单栏 · 点击切换。可多选。"
                        }),
                )
            })
    }

    /// Full-page settings (replaces the old centered modal).
    fn render_settings(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let section = self.settings_section;
        let _ = window; // height comes from flex layout

        v_flex()
            .id("settings-panel")
            .debug_selector(|| "settings-panel-root".into())
            .size_full()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .h(px(44.))
                    .px_3()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .child(
                        Button::new("settings-back")
                            .ghost()
                            .xsmall()
                            .label(if work { "← Back" } else { "← 返回行情" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.close_settings(cx);
                            })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(if work { "Preferences" } else { "设置" }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work {
                                "Saved locally · Esc to leave"
                            } else {
                                "本地保存 · Esc 返回"
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        v_flex()
                            .w(px(200.))
                            .h_full()
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().sidebar)
                            .p_2()
                            .gap_1()
                            .children(SettingsSection::all().map(|sec| {
                                let on = section == sec;
                                Button::new(("settings-nav", sec as u32))
                                    .ghost()
                                    .when(on, |b| b.primary())
                                    .label(sec.label(work))
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_settings_section(sec, cx);
                                    }))
                            })),
                    )
                    .child(
                        div()
                            .id("settings-body")
                            .flex_1()
                            .min_h_0()
                            .h_full()
                            .overflow_y_scroll()
                            .p_5()
                            .child(match section {
                                SettingsSection::General => {
                                    self.render_settings_general(work, cx).into_any_element()
                                }
                                SettingsSection::StatusBar => {
                                    self.render_settings_status_bar(work, cx).into_any_element()
                                }
                                SettingsSection::Ai => {
                                    self.render_settings_ai(work, cx).into_any_element()
                                }
                                SettingsSection::Update => {
                                    self.render_settings_update(work, cx).into_any_element()
                                }
                                SettingsSection::About => {
                                    self.render_settings_about(work, cx).into_any_element()
                                }
                            }),
                    ),
            )
    }

    fn render_settings_general(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let interval = self.quote_interval_secs;
        let scheme = self.color_scheme;

        v_flex()
            .gap_5()
            .max_w(px(640.))
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "General" } else { "常规" }),
            )
            // Quote interval
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Poll interval" } else { "行情刷新间隔" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .child(if work {
                                "How often quotes refresh. Faster may hit rate limits."
                            } else {
                                "自选报价自动刷新频率。过快可能被数据源限流。"
                            }),
                    )
                    .child(
                        h_flex().gap_1().flex_wrap().children(
                            QUOTE_INTERVAL_PRESETS.iter().map(|&secs| {
                                let active = interval == secs;
                                Button::new(("qi", secs as u32))
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(format!("{secs}s"))
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_quote_interval_secs(secs, cx);
                                    }))
                            }),
                        ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{}: {interval}s",
                                if work { "Current" } else { "当前" }
                            )),
                    ),
            )
            // Color scheme
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Color scheme" } else { "涨跌配色" }),
                    )
                    .child(
                        h_flex().gap_1().children([ColorScheme::Cn, ColorScheme::Us].map(|s| {
                            let active = scheme == s;
                            let id = match s {
                                ColorScheme::Cn => "set-scheme-cn",
                                ColorScheme::Us => "set-scheme-us",
                            };
                            Button::new(id)
                                .xsmall()
                                .when(active, |b| b.primary())
                                .when(!active, |b| b.ghost())
                                .label(s.label())
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.set_color_scheme(s, cx);
                                }))
                        })),
                    ),
            )
            // Work mode
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Focus layout" } else { "工作模式" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .child(if work {
                                "Metrics dashboard + neutral chrome. Toggle with ⌘⇧W."
                            } else {
                                "服务指标台 + 中性文案。快捷键 ⌘⇧W。"
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("set-work-off")
                                    .xsmall()
                                    .when(!work, |b| b.primary())
                                    .when(work, |b| b.ghost())
                                    .label(if work { "Off" } else { "关闭" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_work_mode(false, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("set-work-on")
                                    .xsmall()
                                    .when(work, |b| b.primary())
                                    .when(!work, |b| b.ghost())
                                    .label(if work { "On" } else { "开启" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_work_mode(true, window, cx);
                                    })),
                            ),
                    ),
            )
    }

    fn render_settings_update(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .max_w(px(640.))
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "Update" } else { "更新" }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.9))
                    .child(self.update_status_line(work)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("check-update-btn")
                            .xsmall()
                            .ghost()
                            .label(if work { "Check" } else { "检查更新" })
                            .disabled(matches!(
                                self.update_state,
                                UpdateState::Checking | UpdateState::Downloading(_)
                            ))
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.check_for_updates(true, cx);
                            })),
                    )
                    .children(match &self.update_state {
                        UpdateState::Available(_) => Some(
                            Button::new("settings-update-now")
                                .xsmall()
                                .primary()
                                .label(if work { "Update now" } else { "立即更新" })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.start_update(cx);
                                })),
                        ),
                        _ => None,
                    }),
            )
    }

    fn render_settings_ai(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let use_cli = self.ai_config.transport == AiTransport::Cli;
        let status = if self.ai_config.enabled {
            if self.ai_config.is_configured() {
                if work {
                    format!("Enabled · {}", self.ai_config.source_label())
                } else {
                    format!("已开启 · {}", self.ai_config.source_label())
                }
            } else if work {
                "Enabled · missing base URL / model / key.".to_string()
            } else {
                "已开启 · 尚未填全 API 地址 / 模型 / Key。".to_string()
            }
        } else if work {
            "Disabled · local rules only.".to_string()
        } else {
            "未开启 · 仅使用本地点评。".to_string()
        };

        let mut col = v_flex()
            .gap_5()
            .w_full()
            .max_w(px(640.))
            // Page title
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "AI analysis" } else { "AI 分析" }),
            )
            // Enable / disable
            .child(
                v_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Enable" } else { "开关" }),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .child(if work {
                                "Optional LLM brief. Falls back to local rules when off or failed."
                            } else {
                                "可选 LLM 点评；关闭或请求失败时自动使用本地规则。"
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("ai-on")
                                    .xsmall()
                                    .when(self.ai_config.enabled, |b| b.primary())
                                    .when(!self.ai_config.enabled, |b| b.ghost())
                                    .label(if work { "On" } else { "开启" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_ai_enabled(true, cx);
                                    })),
                            )
                            .child(
                                Button::new("ai-off")
                                    .xsmall()
                                    .when(!self.ai_config.enabled, |b| b.primary())
                                    .when(self.ai_config.enabled, |b| b.ghost())
                                    .label(if work { "Off" } else { "关闭" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_ai_enabled(false, cx);
                                    })),
                            ),
                    ),
            )
            // Transport
            .child(
                v_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Transport" } else { "调用方式" }),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .child(if work {
                                "HTTP API or a local CLI already logged in on this machine."
                            } else {
                                "HTTP API，或本机已登录的 CLI（Grok / ChatGPT·Codex / OpenCode / Claude）。"
                            }),
                    )
                    .child(h_flex().gap_1().children(AiTransport::all().map(|t| {
                        let active = self.ai_config.transport == t;
                        let id = match t {
                            AiTransport::Api => "ai-transport-api",
                            AiTransport::Cli => "ai-transport-cli",
                        };
                        Button::new(id)
                            .xsmall()
                            .when(active, |b| b.primary())
                            .when(!active, |b| b.ghost())
                            .label(t.label())
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.set_ai_transport(t, cx);
                            }))
                    }))),
            );

        if use_cli {
            col = col
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "CLI tool" } else { "CLI 工具" }),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .flex_wrap()
                                .children(AiCliProvider::all().map(|p| {
                                    let active = self.ai_config.cli_provider == p;
                                    let id = match p {
                                        AiCliProvider::Grok => "ai-cli-grok",
                                        AiCliProvider::Chatgpt => "ai-cli-chatgpt",
                                        AiCliProvider::Opencode => "ai-cli-opencode",
                                        AiCliProvider::Claude => "ai-cli-claude",
                                    };
                                    Button::new(id)
                                        .xsmall()
                                        .when(active, |b| b.primary())
                                        .when(!active, |b| b.ghost())
                                        .label(p.label())
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.set_ai_cli_provider(p, cx);
                                        }))
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "Model (optional)"
                                } else {
                                    "模型（可选）"
                                }),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.9))
                                .child(if work {
                                    "Leave empty to use the CLI default model."
                                } else {
                                    "留空则使用 CLI 默认模型。"
                                }),
                        )
                        .child(Input::new(&self.ai_model_input).small()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "CLI path (optional)"
                                } else {
                                    "CLI 路径（可选）"
                                }),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.9))
                                .child(if work {
                                    "Absolute path if the binary is not on PATH."
                                } else {
                                    "不在 PATH 时填写绝对路径，例如 /opt/homebrew/bin/claude。"
                                }),
                        )
                        .child(Input::new(&self.ai_cli_bin_input).small()),
                )
                .child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child(if work {
                            "Uses your logged-in CLI. Only the metric snapshot is sent as the prompt."
                        } else {
                            "使用本机 CLI 登录态；只把指标快照作为提示词，不上传原始行情。"
                        }),
                );
        } else {
            col = col
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "Protocol" } else { "协议" }),
                        )
                        .child(
                            h_flex().gap_1().children(AiKind::all().map(|kind| {
                                let active = self.ai_config.kind == kind;
                                let id = match kind {
                                    AiKind::Responses => "ai-kind-responses",
                                    AiKind::Chat => "ai-kind-chat",
                                };
                                Button::new(id)
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(kind.label())
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_ai_kind(kind, cx);
                                    }))
                            })),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "Base URL" } else { "API 地址" }),
                        )
                        .child(Input::new(&self.ai_base_url_input).small()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "Model" } else { "模型" }),
                        )
                        .child(Input::new(&self.ai_model_input).small()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "API key" } else { "API Key" }),
                        )
                        .child(Input::new(&self.ai_api_key_input).small().mask_toggle()),
                )
                .child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child(if work {
                            "Key stays in local config.json. Only the metric snapshot is sent."
                        } else {
                            "Key 仅保存在本机 config.json；只上传指标快照，不上传原始行情。"
                        }),
                );
        }

        col.child(
            div()
                .w_full()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(status),
        )
    }

    fn render_settings_about(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .max_w(px(640.))
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "About" } else { "关于" }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.9))
                    .child(format!(
                        "{} v{}",
                        if work { "Version" } else { "版本" },
                        env!("CARGO_PKG_VERSION")
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.9))
                    .child(if work {
                        "Data: Eastmoney & Tencent public endpoints, personal study only."
                    } else {
                        "数据来源：东方财富 / 腾讯财经公开接口，仅供个人学习研究。"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                    .child(if work {
                        "For reference only. Quotes may be delayed or erroneous; no investment advice."
                    } else {
                        "行情可能有延迟或误差，所有指标与评分仅供参考，不构成任何投资建议。"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                    .child(if work {
                        "Prefs are saved locally and apply immediately."
                    } else {
                        "设置会写入本地配置，立即生效。"
                    }),
            )
    }

    fn render_left_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let avail_h = (window.bounds().size.height - TITLE_BAR_HEIGHT).max(px(0.));
        v_flex()
            .size_full()
            // Definite height: the resizable group's panels are centered
            // (align-items: center) and percentage heights are not resolved on
            // the very first layout, which otherwise leaves the sidebar
            // collapsed to its content height with empty bands above/below.
            .h(avail_h)
            .bg(cx.theme().sidebar)
            // Used by the layout regression test; no-op outside test builds.
            .debug_selector(|| "left-panel-root".into())
            .child(
                h_flex()
                    .h(px(36.))
                    .px_2()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("tab-watch")
                            .xsmall()
                            .when(self.left_tab == LeftTab::Watchlist, |b| b.primary())
                            .when(self.left_tab != LeftTab::Watchlist, |b| b.ghost())
                            .label(if work { "List" } else { "自选" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_left_tab(LeftTab::Watchlist, cx);
                            })),
                    )
                    .child(
                        Button::new("tab-treasure")
                            .xsmall()
                            .when(self.left_tab == LeftTab::Treasure, |b| b.primary())
                            .when(self.left_tab != LeftTab::Treasure, |b| b.ghost())
                            .label(if work { "Scan" } else { "🐭 寻宝" })
                            .tooltip(if work {
                                "Multi-window scan · ⌘T"
                            } else {
                                "多窗口历史低位扫描（1Y/3Y/全样本）"
                            })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_left_tab(LeftTab::Treasure, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(match self.left_tab {
                                LeftTab::Watchlist => format!("{} 只", self.symbols.len()),
                                LeftTab::Treasure => {
                                    if self.treasure_scanning {
                                        format!("{}/{}", self.treasure_done, self.treasure_total)
                                    } else {
                                        format!("{} 只", self.treasure_hits.len())
                                    }
                                }
                            }),
                    ),
            )
            .child(match self.left_tab {
                LeftTab::Watchlist => self.render_watchlist_body(cx).into_any_element(),
                LeftTab::Treasure => self.render_treasure_body(cx).into_any_element(),
            })
    }

    fn render_watchlist_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let work = self.work_mode;
        let sort = self.watchlist_sort;
        let display_order = self.watchlist_display_order();
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                h_flex()
                    .h(px(28.))
                    .px_2()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "ID / Name" } else { "代码 / 名称" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Value" } else { "最新" }),
                    ),
            )
            .child(
                h_flex()
                    .h(px(26.))
                    .px_1()
                    .items_center()
                    .gap_0p5()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.6))
                    .children(WatchlistSort::all().map(|s| {
                        let active = sort == s;
                        Button::new(("wl-sort", s as u32))
                            .ghost()
                            .xsmall()
                            .when(active, |b| b.primary())
                            .label(s.label(work))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.set_watchlist_sort(s, cx);
                            }))
                    })),
            )
            .child(
                v_flex()
                    .id("watchlist-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(display_order.into_iter().map(|ix| {
                        let sym = &self.symbols[ix];
                        let is_selected = sym.code == selected.as_ref();
                        let code = shared(sym.code.clone());
                        let code_show = self.display_code(&sym.code);
                        let name_show = if work {
                            shared("series")
                        } else {
                            sym.name.clone()
                        };
                        let code_rm = code.clone();
                        let name_rm = name_show.clone();
                        let code_tip = code_show.clone();
                        let last = format_price(sym.last);
                        let chg = self.format_change(sym.change_pct);
                        let chg_color = self.chg_color(sym.is_up(), cx);
                        let board = if work {
                            shared("svc")
                        } else {
                            sym.board.clone()
                        };

                        div()
                            .id(("watch-row", ix))
                            .h(px(48.))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.35))
                            .when(is_selected, |this| {
                                this.bg(cx.theme().accent.opacity(0.18))
                            })
                            .hover(|this| this.bg(cx.theme().accent.opacity(0.10)))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.select_symbol(code.clone(), cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .child(code_show),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(board),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .truncate()
                                            .child(name_show),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .items_end()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .text_color(cx.theme().foreground)
                                            .child(last),
                                    )
                                    .child(
                                        div().text_xs().text_color(chg_color).child(chg),
                                    ),
                            )
                            .child(
                                Button::new(("wl-rm", ix))
                                    .icon(IconName::Delete)
                                    .ghost()
                                    .xsmall()
                                    .tooltip(if work {
                                        format!("Remove {code_tip}")
                                    } else {
                                        format!("删除 {name_rm}")
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.remove_symbol(&code_rm, cx);
                                    })),
                            )
                    })),
            )
            .child(
                h_flex()
                    .h(px(32.))
                    .px_2()
                    .items_center()
                    .gap_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("add-sym")
                            .ghost()
                            .xsmall()
                            .label(if work { "+ Add" } else { "+ 添加" })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_palette(window, cx);
                            })),
                    )
                    .child(
                        Button::new("rm-sym")
                            .ghost()
                            .xsmall()
                            .label(if work { "Remove" } else { "移除" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.remove_selected_from_watchlist(cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.75))
                            .child(if work { "↑↓ navigate" } else { "↑↓ 切换" }),
                    ),
            )
    }

    fn render_treasure_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let work = self.work_mode;
        // 主按钮：空榜 → 搜罗；有榜无清单/重跑 → 筛可买；两边都不抢 primary。
        let has_hits = !self.treasure_hits.is_empty();
        let has_picks = !self.scout_picks.is_empty();
        let busy = self.treasure_scanning || self.scout_running;
        let scan_is_primary = !busy && !has_hits;
        let pick_is_primary = !busy && has_hits && !has_picks;
        let show_full_list = !has_picks || self.treasure_list_expanded;

        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                v_flex()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_wrap()
                            .child(
                                Button::new("treasure-scan")
                                    .xsmall()
                                    .when(scan_is_primary, |b| b.primary())
                                    .when(!scan_is_primary, |b| b.ghost())
                                    .label(if self.treasure_scanning {
                                        if work {
                                            "① Running…"
                                        } else {
                                            "① 扫描中…"
                                        }
                                    } else if work {
                                        "① Scout pool"
                                    } else {
                                        "① 开始搜罗"
                                    })
                                    .disabled(busy)
                                    .tooltip(if work {
                                        "Scan historical lows (then auto AI picks)"
                                    } else {
                                        "扫描历史低位；完成后自动筛可买"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_treasure_scan(cx);
                                    })),
                            )
                            .child(
                                Button::new("treasure-ai-scout")
                                    .xsmall()
                                    .when(pick_is_primary, |b| b.primary())
                                    .when(!pick_is_primary, |b| b.ghost())
                                    .label(if self.scout_running {
                                        if work {
                                            "② Picking…"
                                        } else {
                                            "② 筛可买中…"
                                        }
                                    } else if has_picks {
                                        if work {
                                            "② Re-pick"
                                        } else {
                                            "② 重新筛可买"
                                        }
                                    } else if work {
                                        "② AI picks"
                                    } else {
                                        "② 筛可买"
                                    })
                                    .disabled(busy || !has_hits)
                                    .tooltip(if work {
                                        "Batch-rank buy-watch names from the scan list"
                                    } else {
                                        "从寻宝榜批量给出可关注清单与建仓价（不必一只只点）"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_scout_picks(cx);
                                    })),
                            )
                            .when(busy, |row| {
                                row.child(
                                    Button::new("treasure-cancel")
                                        .xsmall()
                                        .ghost()
                                        .label(if work { "Cancel" } else { "取消" })
                                        .on_click(cx.listener(|this, _, _w, cx| {
                                            if this.scout_running {
                                                this.cancel_scout_picks(cx);
                                            } else {
                                                this.cancel_treasure_scan(cx);
                                            }
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.treasure_status.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.85))
                            .child(if work {
                                format!(
                                    "{} · {} · ≤{TREASURE_SCAN_CAP} · Top{TREASURE_TOP_N}",
                                    self.treasure_pool.label(),
                                    self.treasure_fin.label(),
                                )
                            } else {
                                format!(
                                    "流程：①搜罗低位 → ②筛可买（自动）→ 点可关注看图 · {}池 · {} · Top{TREASURE_TOP_N}",
                                    self.treasure_pool.label(),
                                    self.treasure_fin.label(),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(TreasurePool::all().into_iter().enumerate().map(
                                |(ix, p)| {
                                    let active = self.treasure_pool == p;
                                    Button::new(("tpool", ix as u32))
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(p.label())
                                    .tooltip(if p == TreasurePool::Mcap {
                                        "东财按总市值取沪深 A（默认）"
                                    } else {
                                        "指数成分动态拉取（新浪）"
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_treasure_pool(p, cx);
                                    }))
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(FinFilter::all().into_iter().enumerate().map(
                                |(ix, f)| {
                                    let active = self.treasure_fin == f;
                                    Button::new(("tfin", ix as u32))
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(f.label())
                                    .tooltip(match f {
                                        FinFilter::Off => "不做财务过滤",
                                        FinFilter::Pe => "保留池内 PE 分位 ≤ 50%",
                                        FinFilter::Pb => "保留池内 PB 分位 ≤ 50%",
                                        FinFilter::Value => "PE 与 PB 分位均 ≤ 50%",
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_treasure_fin(f, cx);
                                    }))
                                },
                            )),
                    ),
            )
            .child(
                v_flex()
                    .id("treasure-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    // —— 可买观察清单（批量筛结果，优先展示）——
                    .when(
                        !self.scout_picks.is_empty()
                            || self.scout_running
                            || !self.scout_summary.as_ref().is_empty(),
                        |el| {
                            let visible = self.visible_scout_picks();
                            let buy_n = self
                                .scout_picks
                                .iter()
                                .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
                                .count();
                            let watch_n = self
                                .scout_picks
                                .iter()
                                .filter(|p| p.verdict == ScoutVerdict::Watch)
                                .count();
                            let only_buy = self.scout_only_buy_watch;
                            let count_label = if self.scout_running {
                                format!("{}/{}", self.scout_done, self.scout_total)
                            } else if only_buy {
                                format!("{}/{} 可关注", visible.len(), self.scout_picks.len())
                            } else {
                                format!("{} 只", self.scout_picks.len())
                            };

                            el.child(
                                v_flex()
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .child(
                                        h_flex()
                                            .h(px(26.))
                                            .px_3()
                                            .items_center()
                                            .gap_2()
                                            .bg(cx.theme().accent.opacity(0.08))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .child(if work {
                                                        "Buy watchlist"
                                                    } else {
                                                        "🎯 可买观察"
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(count_label),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .px_3()
                                            .py_1()
                                            .gap_1()
                                            .items_center()
                                            .border_b_1()
                                            .border_color(cx.theme().border.opacity(0.5))
                                            .child(
                                                Button::new("scout-filter-all")
                                                    .xsmall()
                                                    .when(!only_buy, |b| b.primary())
                                                    .when(only_buy, |b| b.ghost())
                                                    .label(if work {
                                                        "All"
                                                    } else {
                                                        "全部"
                                                    })
                                                    .tooltip(if work {
                                                        "Show BuyWatch + Watch"
                                                    } else {
                                                        "显示可关注与观察"
                                                    })
                                                    .on_click(cx.listener(|this, _, _w, cx| {
                                                        this.set_scout_only_buy_watch(false, cx);
                                                    })),
                                            )
                                            .child(
                                                Button::new("scout-filter-buy")
                                                    .xsmall()
                                                    .when(only_buy, |b| b.primary())
                                                    .when(!only_buy, |b| b.ghost())
                                                    .label(if work {
                                                        "Buy only"
                                                    } else {
                                                        "仅可关注"
                                                    })
                                                    .tooltip(if work {
                                                        "Hide Watch rows"
                                                    } else {
                                                        "只显示可关注，隐藏观察"
                                                    })
                                                    .on_click(cx.listener(|this, _, _w, cx| {
                                                        this.set_scout_only_buy_watch(true, cx);
                                                    })),
                                            )
                                            .when(!self.scout_running && !work, |row| {
                                                row.child(
                                                    div()
                                                        .ml_1()
                                                        .text_xs()
                                                        .text_color(
                                                            cx.theme().muted_foreground.opacity(0.85),
                                                        )
                                                        .child(format!(
                                                            "可关注 {buy_n} · 观察 {watch_n}"
                                                        )),
                                                )
                                            }),
                                    )
                                    .when(!self.scout_summary.as_ref().is_empty(), |c| {
                                        c.child(
                                            div()
                                                .px_3()
                                                .py_2()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(self.scout_summary.clone()),
                                        )
                                        .when(!self.scout_source.as_ref().is_empty(), |c2| {
                                            c2.child(
                                                div()
                                                    .px_3()
                                                    .pb_1()
                                                    .text_xs()
                                                    .text_color(
                                                        cx.theme().muted_foreground.opacity(0.8),
                                                    )
                                                    .child(self.scout_source.clone()),
                                            )
                                        })
                                    })
                                    .when(
                                        visible.is_empty()
                                            && !self.scout_running
                                            && !self.scout_picks.is_empty()
                                            && only_buy,
                                        |c| {
                                            c.child(
                                                div()
                                                    .px_3()
                                                    .py_2()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(if work {
                                                        "No BuyWatch rows. Switch to All to see Watch."
                                                    } else {
                                                        "当前没有「可关注」；点「全部」可查看观察名单。"
                                                    }),
                                            )
                                        },
                                    )
                                    .children(visible.into_iter().enumerate().map(
                                        |(ix, pick)| {
                                            let is_selected =
                                                pick.code == selected.as_ref();
                                            let pick_owned = pick.clone();
                                            let code_label = if work {
                                                disguise_label(&pick.code, &pick.name)
                                            } else {
                                                pick.code.clone()
                                            };
                                            let name =
                                                display_name_str(&pick.name, &pick.code);
                                            let show_name =
                                                !work && is_real_name(&name, &pick.code);
                                            let verdict_color = match pick.verdict {
                                                ScoutVerdict::BuyWatch => {
                                                    cx.theme().success
                                                }
                                                ScoutVerdict::Watch => {
                                                    cx.theme().warning
                                                }
                                                ScoutVerdict::Skip => {
                                                    cx.theme().muted_foreground
                                                }
                                            };
                                            let band = format!(
                                                "建仓 {} · 减仓 {}",
                                                pick.buy_band_text(),
                                                pick.sell_band_text()
                                            );
                                            let why = pick
                                                .reasons
                                                .iter()
                                                .take(2)
                                                .cloned()
                                                .collect::<Vec<_>>()
                                                .join(" · ");

                                            div()
                                                .id(("scout-row", ix as u64))
                                                .px_3()
                                                .py_2()
                                                .flex()
                                                .items_start()
                                                .gap_2()
                                                .cursor_pointer()
                                                .border_b_1()
                                                .border_color(
                                                    cx.theme().border.opacity(0.35),
                                                )
                                                .when(is_selected, |this| {
                                                    this.bg(cx.theme().accent.opacity(0.18))
                                                })
                                                .hover(|this| {
                                                    this.bg(cx.theme().accent.opacity(0.10))
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, _w, cx| {
                                                        this.select_scout_pick(
                                                            &pick_owned, cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .gap_1()
                                                        .child(
                                                            h_flex()
                                                                .gap_2()
                                                                .items_center()
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .font_semibold()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .foreground,
                                                                        )
                                                                        .child(code_label),
                                                                )
                                                                .when(show_name, |row| {
                                                                    row.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(name),
                                                                    )
                                                                })
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .font_semibold()
                                                                        .text_color(
                                                                            verdict_color,
                                                                        )
                                                                        .child(
                                                                            pick.verdict
                                                                                .label(),
                                                                        ),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().foreground
                                                                        .opacity(0.9),
                                                                )
                                                                .child(if work {
                                                                    format!(
                                                                        "buy {} / sell {}",
                                                                        pick.buy_band_text(),
                                                                        pick.sell_band_text()
                                                                    )
                                                                } else {
                                                                    band
                                                                }),
                                                        )
                                                        .when(!why.is_empty() && !work, |r| {
                                                            r.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(
                                                                        cx.theme()
                                                                            .muted_foreground,
                                                                    )
                                                                    .child(why),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_semibold()
                                                        .text_color(verdict_color)
                                                        .child(format!(
                                                            "{:.0}",
                                                            pick.buy_score
                                                        )),
                                                )
                                        },
                                    )),
                            )
                        },
                    )
                    // 完整寻宝榜：有可买清单时默认折叠，避免抢注意力
                    .when(has_picks, |el| {
                        el.child(
                            h_flex()
                                .id("treasure-list-fold")
                                .h(px(28.))
                                .px_3()
                                .items_center()
                                .gap_2()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .cursor_pointer()
                                .hover(|this| this.bg(cx.theme().accent.opacity(0.06)))
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    let next = !this.treasure_list_expanded;
                                    this.set_treasure_list_expanded(next, cx);
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work {
                                            format!(
                                                "{} full list ({})",
                                                if self.treasure_list_expanded {
                                                    "▾"
                                                } else {
                                                    "▸"
                                                },
                                                self.treasure_hits.len()
                                            )
                                        } else {
                                            format!(
                                                "{} 完整寻宝榜（{} 只，参考用）",
                                                if self.treasure_list_expanded {
                                                    "▾"
                                                } else {
                                                    "▸"
                                                },
                                                self.treasure_hits.len()
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if self.treasure_list_expanded {
                                            if work {
                                                "Hide"
                                            } else {
                                                "收起"
                                            }
                                        } else if work {
                                            "Show"
                                        } else {
                                            "展开"
                                        }),
                                ),
                        )
                    })
                    .when(!has_picks, |el| {
                        el.child(
                            h_flex()
                                .h(px(26.))
                                .px_3()
                                .items_center()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work {
                                            "Scan list / position"
                                        } else {
                                            "寻宝榜 / 位置（筛可买后会出现上方清单）"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work { "Scr" } else { "分" }),
                                ),
                        )
                    })
                    .when(self.treasure_hits.is_empty() && !self.treasure_scanning, |el| {
                        el.child(
                            div()
                                .p_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "三步：①「开始搜罗」扫历史低位（≤{TREASURE_SCAN_CAP}）→ \
                                     ②自动筛可买清单与建仓价 → ③点「可关注」看图。\
                                     有缓存榜时可直接点「② 筛可买」。"
                                )),
                        )
                    })
                    .when(show_full_list, |el| {
                        el.children(self.treasure_hits.iter().enumerate().map(|(ix, hit)| {
                        let is_selected = hit.code == selected.as_ref();
                        let hit_owned = hit.clone();
                        let code_label = if work {
                            disguise_label(&hit.code, &hit.name)
                        } else {
                            hit.code.clone()
                        };
                        let name = display_name_str(&hit.name, &hit.code);
                        let show_name = !work && is_real_name(&name, &hit.code);
                        let score = format!("{:.0}", hit.score);
                        let pos_line = format!(
                            "1Y {} · 3Y {} · 全 {}",
                            fmt_pos(hit.pos_1y),
                            fmt_pos(hit.pos_3y),
                            fmt_pos(hit.pos_all)
                        );
                        let dd_line = format!("回撤全 {}", fmt_dd(hit.dd_all));
                        let tags: String = hit
                            .tags
                            .iter()
                            .take(3)
                            .map(|t| t.label())
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let is_pullback = hit
                            .tags
                            .iter()
                            .any(|t| matches!(t, treasure::TreasureTag::UptrendPullback));

                        div()
                            .id(("treasure-row", ix as u64))
                            .px_3()
                            .py_2()
                            .flex()
                            .items_start()
                            .gap_2()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.35))
                            .when(is_selected, |this| {
                                this.bg(cx.theme().accent.opacity(0.18))
                            })
                            .hover(|this| this.bg(cx.theme().accent.opacity(0.10)))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.select_treasure_hit(&hit_owned, cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .child(code_label),
                                            )
                                            .when(show_name, |row| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .truncate()
                                                        .child(name),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(pos_line),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(dd_line),
                                            )
                                            .when(!tags.is_empty(), |row| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(if is_pullback {
                                                            cx.theme().yellow
                                                        } else {
                                                            cx.theme().blue
                                                        })
                                                        .truncate()
                                                        .child(tags),
                                                )
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(if is_pullback {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().yellow
                                    })
                                    .child(score),
                            )
                        }))
                    }),
            )
    }

    fn render_chart_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sym = self.current_symbol();
        // Only use candle snapshot when it belongs to the selected symbol
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let snap = if candles_match {
            match self.chart_kind {
                ChartKind::Intraday => self.minute.as_ref().and_then(|m| m.snapshot()),
                ChartKind::DayK | ChartKind::MinuteK(_) => {
                    QuoteSnapshot::from_candles(&self.candles)
                }
            }
        } else {
            None
        };
        let up = snap
            .as_ref()
            .map(|s| s.change_pct >= 0.0)
            .or_else(|| sym.map(|s| s.is_up()))
            .unwrap_or(true);
        let chg_color = self.chg_color(up, cx);
        let paint = self.chart_paint_data(cx);
        let work = self.work_mode;

        let code_show = self.display_code(self.selected.as_ref());
        let name_raw = sym.map(|s| s.name.as_ref().to_string()).unwrap_or_default();
        let name_show = if work {
            None
        } else if is_real_name(&name_raw, self.selected.as_ref()) {
            Some(shared(name_raw))
        } else {
            None
        };
        let board = if work {
            shared("metric")
        } else {
            sym.map(|s| s.board.clone())
                .unwrap_or_else(|| shared(""))
        };
        // Prefer live quote on the watchlist; fall back to last candle only if matched
        let close = sym
            .map(|s| s.last)
            .filter(|v| *v > 0.0)
            .or_else(|| snap.as_ref().map(|s| s.close))
            .unwrap_or(0.0);
        let chg = sym
            .map(|s| s.change_pct)
            .or_else(|| snap.as_ref().map(|s| s.change_pct))
            .unwrap_or(0.0);
        let close_disp = self.format_value(close);
        let chg_disp = self.format_change(chg);

        // OHLC strip (merged into quote header to free chart vertical space)
        let ohlc_el = if candles_match {
            let o = snap.as_ref().map(|s| s.open).unwrap_or(0.0);
            let hi = snap.as_ref().map(|s| s.high).unwrap_or(0.0);
            let lo = snap.as_ref().map(|s| s.low).unwrap_or(0.0);
            let v = snap.as_ref().map(|s| s.volume).unwrap_or(0);
            if work {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("min {}", self.format_value(lo)))
                    .child(format!("max {}", self.format_value(hi)))
                    .child(format!("pts {}", format_volume(v)))
                    .into_any_element()
            } else {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("开 {}", format_price(o)))
                    .child(format!("高 {}", format_price(hi)))
                    .child(format!("低 {}", format_price(lo)))
                    .child(format!("量 {}", format_volume(v)))
                    .into_any_element()
            }
        } else {
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if self.loading {
                    if work {
                        "Loading series…"
                    } else if matches!(self.chart_kind, ChartKind::Intraday) {
                        "分时加载中…"
                    } else {
                        "K线加载中…"
                    }
                } else if work {
                    "No series data"
                } else if matches!(self.chart_kind, ChartKind::Intraday) {
                    "暂无分时数据"
                } else {
                    "暂无匹配的 K 线"
                })
                .into_any_element()
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            // Used by the layout regression test; no-op outside test builds.
            .debug_selector(|| "chart-area-root".into())
            // Quote identity + price + OHLC（原两行合并为一行）
            .child(
                h_flex()
                    .id("chart-quote-header")
                    .h(px(48.))
                    .flex_shrink_0()
                    .px_4()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .debug_selector(|| "chart-quote-header".into())
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .min_w_0()
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(code_show),
                            )
                            .when_some(name_show, |row, n| {
                                row.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(n),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(board),
                            )
                            .child(div().w(px(8.)))
                            .child(ohlc_el),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(close_disp),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(chg_color)
                                    .child(chg_disp),
                            ),
                    ),
            )
            // Toolbar：周期 / 指标 / 画线（操作与行情分离）
            .child(
                h_flex()
                    .h(px(34.))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(self.kind_button(
                                if work { "Intraday" } else { "分时" },
                                ChartKind::Intraday,
                                cx,
                            ))
                            .child(self.kind_button(
                                if work { "Daily" } else { "日K" },
                                ChartKind::DayK,
                                cx,
                            ))
                            .child(self.kind_button(
                                if work { "Minute" } else { "分钟" },
                                ChartKind::MinuteK(self.current_minute_period()),
                                cx,
                            ))
                            .when(matches!(self.chart_kind, ChartKind::DayK), |row| {
                                row.child(div().w(px(6.)))
                                    .children(ChartRange::all().map(|range| {
                                        let active = self.range == range;
                                        Button::new(("range", range as u32))
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(range.label())
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.set_range(range, cx);
                                            }))
                                    }))
                            })
                            .when(matches!(self.chart_kind, ChartKind::MinuteK(_)), |row| {
                                row.child(div().w(px(6.)))
                                    .children(MinutePeriod::all().map(|p| {
                                        let active = self.chart_kind == ChartKind::MinuteK(p);
                                        Button::new(("mperiod", p as u32))
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(p.label())
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.set_chart_kind(ChartKind::MinuteK(p), cx);
                                            }))
                                    }))
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .when(!matches!(self.chart_kind, ChartKind::Intraday), |row| {
                                row.child(self.ma_toggle(
                                    "ma5",
                                    if work { "L1" } else { "MA5" },
                                    self.show_ma5,
                                    cx,
                                ))
                                .child(self.ma_toggle(
                                    "ma10",
                                    if work { "L2" } else { "MA10" },
                                    self.show_ma10,
                                    cx,
                                ))
                                .child(self.ma_toggle(
                                    "ma20",
                                    if work { "L3" } else { "MA20" },
                                    self.show_ma20,
                                    cx,
                                ))
                                .child(self.ma_toggle(
                                    "ma60",
                                    if work { "L4" } else { "MA60" },
                                    self.show_ma60,
                                    cx,
                                ))
                                .when(!work, |row| {
                                    row.child(self.ma_toggle(
                                        "vol",
                                        "VOL",
                                        self.show_volume,
                                        cx,
                                    ))
                                    .child(self.ma_toggle(
                                        "macd",
                                        "MACD",
                                        self.show_macd,
                                        cx,
                                    ))
                                    .child(self.ma_toggle(
                                        "boll",
                                        "BOLL",
                                        self.show_boll,
                                        cx,
                                    ))
                                })
                            })
                            .when(!work, |row| {
                                row.child(div().w(px(6.)))
                                    .child(
                                        Button::new("draw-toggle")
                                            .xsmall()
                                            .when(self.drawing_mode, |b| b.primary())
                                            .when(!self.drawing_mode, |b| b.ghost())
                                            .label("画线")
                                            .tooltip(
                                                "画线模式：拖拽画趋势线，单击画水平价格线；Esc 退出",
                                            )
                                            .on_click(cx.listener(|this, _, _w, cx| {
                                                this.toggle_drawing_mode(cx);
                                            })),
                                    )
                                    .when(self.drawing_mode, |row| {
                                        row.child(
                                            Button::new("clear-lines")
                                                .xsmall()
                                                .ghost()
                                                .label("清除")
                                                .tooltip("清除当前标的的全部画线")
                                                .on_click(cx.listener(|this, _, _w, cx| {
                                                    this.clear_chart_lines(cx);
                                                })),
                                        )
                                    })
                            }),
                    ),
            )
            // hover strip
            .child(
                h_flex()
                    .h(px(26.))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar.opacity(0.4))
                    .child(self.render_hover_strip(cx)),
            )
            // chart canvas
            .child({
                let entity = cx.entity().clone();
                let paint = paint.clone();
                div()
                    .id("chart-body")
                    .flex_1()
                    .min_h_0()
                    .min_h(px(220.))
                    .p_2()
                    .child(
                        div()
                            .id("chart-surface")
                            .size_full()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(cx.theme().radius)
                            .relative()
                            .overflow_hidden()
                            .child(
                                canvas(
                                    move |bounds, _, _| bounds,
                                    move |bounds, _, window, cx| {
                                        entity.update(cx, |this, _| {
                                            this.chart_origin = bounds.origin;
                                            this.chart_bounds = bounds;
                                            this.chart_width = bounds.size.width.as_f32();
                                        });
                                        paint_chart(bounds, &paint, window);
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _w, cx| {
                                let local_x =
                                    ev.position.x.as_f32() - this.chart_origin.x.as_f32();
                                let local_y =
                                    ev.position.y.as_f32() - this.chart_origin.y.as_f32();
                                if this.drawing_mode && this.drawing_anchor.is_some() {
                                    let paint = this.chart_paint_data(cx);
                                    let bounds = this.chart_bounds;
                                    if let Some((ix, price)) =
                                        this.anchor_from_local(&paint, bounds, local_x, local_y)
                                    {
                                        if let Some((ax, ap)) = this.drawing_anchor {
                                            let color_ix = this.draw_color_ix;
                                            this.draft_line = Some(TrendLine::new(
                                                (ax, ap),
                                                (ix, price),
                                                color_ix,
                                            ));
                                            cx.notify();
                                        }
                                    }
                                    return;
                                }
                                let (start, end) = this.chart_visible_range();
                                let vn = end.saturating_sub(start);
                                let local_ix = index_from_x(local_x, this.chart_width, vn);
                                let abs_ix = local_ix.map(|i| start + i);
                                if this.hover_ix != abs_ix {
                                    this.hover_ix = abs_ix;
                                    cx.notify();
                                }
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                                    if this.drawing_mode {
                                        if ev.click_count == 1 {
                                            let local_x = ev.position.x.as_f32()
                                                - this.chart_origin.x.as_f32();
                                            let local_y = ev.position.y.as_f32()
                                                - this.chart_origin.y.as_f32();
                                            let paint = this.chart_paint_data(cx);
                                            let bounds = this.chart_bounds;
                                            if let Some(anchor) = this.anchor_from_local(
                                                &paint,
                                                bounds,
                                                local_x,
                                                local_y,
                                            ) {
                                                this.drawing_anchor = Some(anchor);
                                                this.draft_line = Some(TrendLine::new(
                                                    anchor,
                                                    anchor,
                                                    this.draw_color_ix,
                                                ));
                                                this.hover_ix = None;
                                                cx.notify();
                                            }
                                        }
                                        return;
                                    }
                                    if ev.click_count >= 2 {
                                        this.reset_chart_view();
                                        this.hover_ix = None;
                                        this.status = shared(if this.work_mode {
                                            "zoom reset"
                                        } else {
                                            "已重置图表缩放"
                                        });
                                        cx.notify();
                                    }
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _ev, _w, cx| {
                                    if this.drawing_mode {
                                        if let Some(draft) = this.draft_line.take() {
                                            let commit = {
                                                let from = draft.from;
                                                let to = draft.to;
                                                let same_bar = from.0 == to.0;
                                                let near_price = (to.1 - from.1).abs()
                                                    <= (from.1.abs() * 0.005).max(0.01);
                                                if same_bar && near_price {
                                                    // 单击 → 水平价格线，横跨当前可见区间。
                                                    let (vs, ve) = this.chart_visible_range();
                                                    let (a, b) =
                                                        (vs.min(ve.saturating_sub(1)), ve.saturating_sub(1));
                                                    Some(TrendLine::price_line(
                                                        a,
                                                        b,
                                                        from.1,
                                                        this.draw_color_ix,
                                                    ))
                                                } else {
                                                    Some(draft)
                                                }
                                            };
                                            if let Some(line) = commit {
                                                this.chart_lines
                                                    .entry(this.selected.to_string())
                                                    .or_default()
                                                    .push(line);
                                                this.draw_color_ix = this.draw_color_ix.wrapping_add(1);
                                                this.status = shared(if this.work_mode {
                                                    "line added"
                                                } else {
                                                    "已添加画线"
                                                });
                                                this.persist();
                                            }
                                        }
                                        this.drawing_anchor = None;
                                        this.draft_line = None;
                                        cx.notify();
                                    }
                                }),
                            )
                            .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _w, cx| {
                                this.on_chart_scroll(ev, cx);
                            })),
                    )
            })
    }

    fn ma_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Button::new(id)
            .xsmall()
            .when(on, |b| b.primary())
            .when(!on, |b| b.ghost())
            .label(label)
            .on_click(cx.listener(move |this, _, _w, cx| {
                match id {
                    "ma5" => this.show_ma5 = !this.show_ma5,
                    "ma10" => this.show_ma10 = !this.show_ma10,
                    "ma20" => this.show_ma20 = !this.show_ma20,
                    "ma60" => this.show_ma60 = !this.show_ma60,
                    "vol" => this.show_volume = !this.show_volume,
                    "macd" => this.show_macd = !this.show_macd,
                    "boll" => this.show_boll = !this.show_boll,
                    _ => {}
                }
                this.persist();
                cx.notify();
            }))
    }

    fn toggle_drawing_mode(&mut self, cx: &mut Context<Self>) {
        self.drawing_mode = !self.drawing_mode;
        self.drawing_anchor = None;
        self.draft_line = None;
        self.hover_ix = None;
        if self.drawing_mode {
            self.status = shared(if self.work_mode {
                "draw mode: drag on the chart to add a line"
            } else {
                "画线模式：在图上拖拽画趋势线；单击生成水平线；再点按钮退出"
            });
        } else {
            self.status = shared(if self.work_mode {
                "draw mode off"
            } else {
                "已退出画线模式"
            });
        }
        cx.notify();
    }

    /// 清空当前标的的全部画线。
    fn clear_chart_lines(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let removed = self.chart_lines.remove(&code).unwrap_or_default().len();
        self.draft_line = None;
        self.drawing_anchor = None;
        self.status = shared(format!(
            "{}",
            if self.work_mode {
                format!("removed {removed} line(s)")
            } else {
                format!("已清除 {removed} 条画线")
            }
        ));
        self.persist();
        cx.notify();
    }

    /// 把屏幕坐标转换为画线锚点（可见切片内的索引 + 价格）。
    fn anchor_from_local(
        &self,
        paint: &ChartPaintData,
        bounds: Bounds<Pixels>,
        local_x: f32,
        local_y: f32,
    ) -> Option<(usize, f64)> {
        if paint.candles.is_empty() {
            return None;
        }
        let layout = chart_layout(paint, bounds);
        let visible = paint.candles.len();
        let (start, end) = self.chart_visible_range();
        if start >= end {
            return None;
        }
        let local_ix = index_from_x(local_x, bounds.size.width.as_f32(), visible)?;
        let ix = start + local_ix;
        if ix >= end {
            return None;
        }
        // 只有价格窗格内的点击才落锚；副图区域忽略。
        if local_y < layout.plot_top || local_y > layout.plot_top + layout.price_h {
            return None;
        }
        let price = price_from_y(&layout, local_y);
        Some((ix, price))
    }

    /// The minute period to select when the 分钟K button is pressed.
    fn current_minute_period(&self) -> MinutePeriod {
        match self.chart_kind {
            ChartKind::MinuteK(p) => p,
            _ => MinutePeriod::M5,
        }
    }

    fn kind_button(
        &self,
        label: &'static str,
        kind: ChartKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.chart_kind == kind;
        Button::new(kind.to_label())
            .xsmall()
            .when(active, |b| b.primary())
            .when(!active, |b| b.ghost())
            .label(label)
            .on_click(cx.listener(move |this, _, _w, cx| {
                this.set_chart_kind(kind, cx);
            }))
    }

    fn render_hover_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let work = self.work_mode;
        if candles_match && matches!(self.chart_kind, ChartKind::Intraday) {
            if let Some(ix) = self.hover_ix {
                if let (Some(m), Some(p)) = (self.minute.as_ref(), self.minute.as_ref().and_then(|m| m.points.get(ix))) {
                    let color = self.chg_color(p.price >= m.prev_close, cx);
                    let vol = p.minute_volume(ix.checked_sub(1).map(|j| &m.points[j]));
                    return h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            div()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(p.time.clone()),
                        )
                        .child(
                            div()
                                .text_color(color)
                                .child(format!("价 {}", format_price(p.price))),
                        )
                        .child(format!("均价 {}", format_price(p.avg_price())))
                        .child(format!(
                            "涨跌 {}",
                            format_pct(if m.prev_close > 0.0 {
                                (p.price - m.prev_close) / m.prev_close * 100.0
                            } else {
                                0.0
                            })
                        ))
                        .child(format!("量 {}", format_volume(vol)))
                        .into_any_element();
                }
            }
        }
        if candles_match {
            if let Some(ix) = self.hover_ix {
                if let Some(c) = self.candles.get(ix) {
                    let color = self.chg_color(c.close >= c.open, cx);
                    let (m5, m10, m20, m60) = self.ma.value_at(ix);
                    let (dif, dea, hist) = self.macd.value_at(ix);
                    let (b_up, b_mid, b_low) = self.boll.value_at(ix);
                    let date_label = format_candle_date(c.date.as_ref());
                    if work {
                        return h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(date_label),
                            )
                            .child(format!("v {}", self.format_value(c.close)))
                            .child(format!("lo {}", self.format_value(c.low)))
                            .child(format!("hi {}", self.format_value(c.high)))
                            .when(m5.is_some(), |this| {
                                this.child(format!("L1 {}", self.format_value(m5.unwrap())))
                            })
                            .when(m10.is_some(), |this| {
                                this.child(format!("L2 {}", self.format_value(m10.unwrap())))
                            })
                            .when(m20.is_some(), |this| {
                                this.child(format!("L3 {}", self.format_value(m20.unwrap())))
                            })
                            .when(m60.is_some(), |this| {
                                this.child(format!("L4 {}", self.format_value(m60.unwrap())))
                            })
                            .into_any_element();
                    }
                    let row = h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            div()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(date_label),
                        )
                        .child(format!(
                            "开{} 高{} 低{}",
                            format_price(c.open),
                            format_price(c.high),
                            format_price(c.low)
                        ))
                        .child(
                            div()
                                .text_color(color)
                                .child(format!("收{}", format_price(c.close))),
                        )
                        .child(format!("量{}", format_volume(c.volume)))
                        .when(m5.is_some(), |this| {
                            this.child(format!("MA5 {}", format_price(m5.unwrap())))
                        })
                        .when(m10.is_some(), |this| {
                            this.child(format!("MA10 {}", format_price(m10.unwrap())))
                        })
                        .when(m20.is_some(), |this| {
                            this.child(format!("MA20 {}", format_price(m20.unwrap())))
                        })
                        .when(m60.is_some(), |this| {
                            this.child(format!("MA60 {}", format_price(m60.unwrap())))
                        })
                        .when(
                            self.show_macd && dif.is_some() && dea.is_some() && hist.is_some(),
                            |this| {
                                this.child(format!(
                                    "MACD {:.3}/{:.3}/{:.3}",
                                    dif.unwrap(),
                                    dea.unwrap(),
                                    hist.unwrap()
                                ))
                            },
                        )
                        .when(
                            self.show_boll && b_up.is_some() && b_mid.is_some() && b_low.is_some(),
                            |this| {
                                this.child(format!(
                                    "BOLL {:.2}/{:.2}/{:.2}",
                                    b_up.unwrap(),
                                    b_mid.unwrap(),
                                    b_low.unwrap()
                                ))
                            },
                        );
                    return row.into_any_element();
                }
            }
        }
        let (vs, ve) = self.chart_visible_range();
        let zoom_hint = if self.drawing_mode && !work {
            "画线模式：拖拽画趋势线 · 单击水平线 · Esc 退出".to_string()
        } else if matches!(self.chart_kind, ChartKind::Intraday) {
            if let Some(m) = self.minute.as_ref() {
                let date = if m.date.len() >= 8 {
                    format!("{}-{}-{}", &m.date[..4], &m.date[4..6], &m.date[6..8])
                } else {
                    m.date.clone()
                };
                if work {
                    format!(
                        "intraday {date} · {} pts · scroll/pinch zoom · pan · dblclick reset",
                        m.points.len()
                    )
                } else {
                    format!(
                        "分时 {date} · {} 点 · 滚轮/捏合缩放 · 横向平移 · 双击重置",
                        m.points.len()
                    )
                }
            } else if work {
                "intraday · scroll/pinch zoom · pan · dblclick reset".into()
            } else {
                "分时 · 滚轮/捏合缩放 · 横向平移 · 双击重置".into()
            }
        } else if !self.candles.is_empty() && ve > vs {
            let first = self.candles.get(vs).map(|c| c.date.as_ref()).unwrap_or("?");
            let last = self
                .candles
                .get(ve - 1)
                .map(|c| c.date.as_ref())
                .unwrap_or("?");
            if work {
                format!(
                    "scroll/pinch zoom · pan · dblclick reset · {}…{} ({} pts)",
                    format_candle_date(first),
                    format_candle_date(last),
                    ve - vs
                )
            } else {
                format!(
                    "滚轮/捏合缩放 · 横向平移 · 双击重置 · 可见 {}～{}（{}根）",
                    format_candle_date(first),
                    format_candle_date(last),
                    ve - vs
                )
            }
        } else if work {
            "scroll/pinch zoom · pan · dblclick reset · hover for values".into()
        } else {
            "滚轮/捏合缩放 · 横向平移 · 双击重置 · 移动鼠标查看十字线".into()
        };
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(if self.loading {
                if work {
                    "Loading…".to_string()
                } else {
                    "加载中…".to_string()
                }
            } else if !candles_match || self.candles.is_empty() {
                if work {
                    "No series data".to_string()
                } else {
                    "暂无K线数据".to_string()
                }
            } else {
                zoom_hint
            })
            .into_any_element()
    }

    fn set_detail_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        if self.detail_tab == tab {
            return;
        }
        self.detail_tab = tab;
        cx.notify();
    }

    fn render_detail_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let active = self.detail_tab;

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            // Tab strip：功能分区，一次只看一类信息
            .child(
                h_flex()
                    .h(px(34.))
                    .px_2()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .children(DetailTab::all().map(|tab| {
                        let is_on = active == tab;
                        Button::new(("detail-tab", tab as u32))
                            .xsmall()
                            .when(is_on, |b| b.primary())
                            .when(!is_on, |b| b.ghost())
                            .label(tab.label(work))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.set_detail_tab(tab, cx);
                            }))
                    }))
                    .child(div().flex_1())
                    .child(
                        div()
                            .max_w(px(360.))
                            .overflow_hidden()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.status.clone()),
                    ),
            )
            .child(
                div()
                    .id("detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_x_hidden()
                    .overflow_y_scroll()
                    .p_3()
                    .child(match self.detail_tab {
                        DetailTab::Overview => self.render_detail_overview(cx).into_any_element(),
                        DetailTab::Strategy => self.render_signal_detail_col(cx).into_any_element(),
                        DetailTab::Ai => self.render_ai_detail_col(cx).into_any_element(),
                        DetailTab::Treasure => {
                            self.render_treasure_detail_col(cx).into_any_element()
                        }
                        DetailTab::Indicators => {
                            self.render_indicators_detail(cx).into_any_element()
                        }
                    }),
            )
    }

    /// 概览：紧凑两行——评分/因子 + OHLC/量能/快捷，填满底栏而不是漂在大片空底上。
    fn render_detail_overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let signal = self.current_signal();
        let sym = self.current_symbol();
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let snap = if candles_match {
            match self.chart_kind {
                ChartKind::Intraday => self.minute.as_ref().and_then(|m| m.snapshot()),
                ChartKind::DayK | ChartKind::MinuteK(_) => {
                    QuoteSnapshot::from_candles(&self.candles)
                }
            }
        } else {
            None
        };
        let last_candle = if candles_match {
            self.candles.last()
        } else {
            None
        };
        let code = self.selected.as_ref();
        let name_raw = sym.map(|s| s.name.as_ref()).unwrap_or("");
        let title = if work {
            self.display_code(code)
        } else if is_real_name(name_raw, code) {
            format!("{code}  {name_raw}")
        } else {
            code.to_string()
        };
        let period = if work {
            format!("{} · {} pts", self.chart_label(), self.candles.len())
        } else {
            format!("{} · {} 根", self.chart_label(), self.candles.len())
        };
        let prev = self.format_value(snap.as_ref().map(|s| s.prev_close).unwrap_or(0.0));
        let last_price = sym.map(|s| s.last).unwrap_or(0.0);
        let change_pct = sym.map(|s| s.change_pct).unwrap_or(0.0);
        let volume = sym.map(|s| s.volume).unwrap_or(0);

        v_flex()
            .w_full()
            .gap_2()
            // Row 1: score + title/chips + quick links
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .child(self.render_score_badge(signal.as_ref(), cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(200.))
                            .gap_1p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_baseline()
                                    .flex_wrap()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(cx.theme().muted)
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if work {
                                                "svc".to_string()
                                            } else {
                                                sym.map(|s| s.board.as_ref().to_string())
                                                    .unwrap_or_else(|| "--".into())
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(period),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(self.chg_color(change_pct >= 0.0, cx))
                                            .child(if last_price > 0.0 {
                                                format!(
                                                    "{}  {}",
                                                    self.format_value(last_price),
                                                    format_pct(change_pct)
                                                )
                                            } else {
                                                "—".into()
                                            }),
                                    ),
                            )
                            .child(if let Some(s) = signal.as_ref() {
                                h_flex()
                                    .gap_1p5()
                                    .flex_wrap()
                                    .child(metric_chip(
                                        if work { "RSI" } else { "RSI14" },
                                        &s.rsi14
                                            .map(|v| format!("{v:.1}"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "Mom20" } else { "20日动量" },
                                        &s.momentum_20_pct
                                            .map(|v| format!("{v:+.1}%"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "Vol×" } else { "量能比" },
                                        &s.volume_ratio_20
                                            .map(|v| format!("{v:.1}x"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "DD1Y" } else { "1Y回撤" },
                                        &s.max_drawdown_1y_pct
                                            .map(|v| format!("{v:.1}%"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "σ20" } else { "波动" },
                                        &s.volatility_20_ann_pct
                                            .map(|v| format!("{v:.1}%"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "Conf" } else { "置信" },
                                        &format!("{:.0}%", s.confidence),
                                        cx,
                                    ))
                                    .into_any_element()
                            } else {
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work {
                                        "Need ≥20 daily bars for signal."
                                    } else {
                                        "至少需要 20 根有效日 K 才能生成策略评分。"
                                    })
                                    .into_any_element()
                            })
                            .when_some(signal.as_ref(), |col, s| {
                                col.child(
                                    h_flex().gap_1().flex_wrap().children(
                                        s.reasons.iter().take(5).map(|r| {
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded(cx.theme().radius)
                                                .bg(cx.theme().muted.opacity(0.55))
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child((*r).to_string())
                                        }),
                                    ),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .min_w(px(120.))
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "Quick" } else { "快捷" }),
                            )
                            .child(
                                Button::new("goto-strategy")
                                    .xsmall()
                                    .ghost()
                                    .label(if work { "Signal →" } else { "策略 →" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_detail_tab(DetailTab::Strategy, cx);
                                    })),
                            )
                            .child(
                                Button::new("goto-ai")
                                    .xsmall()
                                    .ghost()
                                    .label(if work { "AI →" } else { "AI →" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_detail_tab(DetailTab::Ai, cx);
                                    })),
                            )
                            .child(
                                Button::new("goto-treasure")
                                    .xsmall()
                                    .ghost()
                                    .label(if work { "Scan →" } else { "寻宝 →" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_detail_tab(DetailTab::Treasure, cx);
                                    })),
                            ),
                    ),
            )
            // Row 2: OHLC / volume / source — fills residual dock height with real info
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .px_1()
                    .py_1p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted.opacity(0.35))
                    .child(metric_chip(
                        if work { "Base" } else { "昨收" },
                        &prev,
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "O" } else { "开" },
                        &last_candle
                            .map(|c| self.format_value(c.open))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "H" } else { "高" },
                        &last_candle
                            .map(|c| self.format_value(c.high))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "L" } else { "低" },
                        &last_candle
                            .map(|c| self.format_value(c.low))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "C" } else { "收" },
                        &last_candle
                            .map(|c| self.format_value(c.close))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "Vol" } else { "量" },
                        &if volume > 0 {
                            format_volume(volume)
                        } else {
                            last_candle
                                .map(|c| format_volume(c.volume))
                                .unwrap_or_else(|| "—".into())
                        },
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.85))
                            .child(self.status.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.7))
                            .child(if work {
                                "For reference only."
                            } else {
                                "仅供学习研究，不构成投资建议。"
                            }),
                    ),
            )
    }

    fn render_score_badge(
        &self,
        signal: Option<&signals::SignalSnapshot>,
        cx: &App,
    ) -> impl IntoElement {
        let work = self.work_mode;
        let (score_txt, regime_txt, conf_txt, color) = if let Some(s) = signal {
            (
                format!("{:.0}", s.score),
                if work {
                    s.regime.service_state().to_string()
                } else {
                    s.regime.label().to_string()
                },
                format!("{:.0}%", s.confidence),
                self.regime_color(s.regime, cx),
            )
        } else {
            (
                "—".into(),
                if work { "n/a".into() } else { "无数据".into() },
                "—".into(),
                cx.theme().muted_foreground,
            )
        };

        v_flex()
            .items_center()
            .justify_center()
            .gap_1()
            .min_w(px(96.))
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .bg(color.opacity(0.12))
            .border_1()
            .border_color(color.opacity(0.35))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work { "Score" } else { "综合" }),
            )
            .child(
                h_flex()
                    .items_baseline()
                    .gap_0p5()
                    .child(
                        div()
                            .text_3xl()
                            .font_semibold()
                            .text_color(color)
                            .child(score_txt),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("/100"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(color)
                    .child(regime_txt),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        format!("conf {conf_txt}")
                    } else {
                        format!("置信 {conf_txt}")
                    }),
            )
    }

    fn regime_color(&self, regime: signals::SignalRegime, cx: &App) -> gpui::Hsla {
        if self.work_mode {
            return cx.theme().muted_foreground;
        }
        use signals::SignalRegime::*;
        match regime {
            Strong => cx.theme().chart_1,
            Constructive => cx.theme().chart_2,
            Neutral => cx.theme().muted_foreground,
            Weak => cx.theme().chart_4,
            Defensive => cx.theme().danger,
        }
    }

    /// 指标 Tab：MA / MACD / BOLL 三卡并排，上下文相关（分时隐藏无意义读数）。
    fn render_indicators_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let kline_ok = candles_match && !matches!(self.chart_kind, ChartKind::Intraday);

        h_flex()
            .w_full()
            .gap_4()
            .items_start()
            .child(
                v_flex()
                    .gap_1()
                    .min_w(px(140.))
                    .flex_1()
                    .child(section_title(if work { "Moving avg" } else { "均线" }, cx))
                    .child(detail_row(
                        if work { "L1" } else { "MA5" },
                        &if kline_ok {
                            self.ma
                                .ma5
                                .last()
                                .and_then(|x| *x)
                                .map(|v| self.format_value(v))
                                .unwrap_or_else(|| "--".into())
                        } else {
                            "--".into()
                        },
                        cx,
                    ))
                    .child(detail_row(
                        if work { "L2" } else { "MA10" },
                        &if kline_ok {
                            self.ma
                                .ma10
                                .last()
                                .and_then(|x| *x)
                                .map(|v| self.format_value(v))
                                .unwrap_or_else(|| "--".into())
                        } else {
                            "--".into()
                        },
                        cx,
                    ))
                    .child(detail_row(
                        if work { "L3" } else { "MA20" },
                        &if kline_ok {
                            self.ma
                                .ma20
                                .last()
                                .and_then(|x| *x)
                                .map(|v| self.format_value(v))
                                .unwrap_or_else(|| "--".into())
                        } else {
                            "--".into()
                        },
                        cx,
                    ))
                    .child(detail_row(
                        if work { "L4" } else { "MA60" },
                        &if kline_ok {
                            self.ma
                                .ma60
                                .last()
                                .and_then(|x| *x)
                                .map(|v| self.format_value(v))
                                .unwrap_or_else(|| "--".into())
                        } else {
                            "--".into()
                        },
                        cx,
                    ))
                    .when(!kline_ok, |col| {
                        col.child(
                            div()
                                .mt_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "Switch to daily/minute K for MA."
                                } else {
                                    "切换到日 K / 分钟 K 查看均线。"
                                }),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .min_w(px(140.))
                    .flex_1()
                    .child(self.render_macd_detail_col(cx)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .min_w(px(140.))
                    .flex_1()
                    .child(self.render_boll_detail_col(cx)),
            )
    }

    fn render_macd_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref())
            && !matches!(self.chart_kind, ChartKind::Intraday);
        let (dif, dea, hist) = self.macd.value_at(self.macd.dif.len().saturating_sub(1));
        let fmt = |v: Option<f64>| {
            v.map(|n| format!("{n:.3}"))
                .unwrap_or_else(|| "--".into())
        };
        v_flex()
            .gap_1()
            .min_w(px(120.))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.work_mode { "MACD" } else { "MACD 12/26/9" }),
            )
            .child(detail_row(
                if self.work_mode { "DIF" } else { "DIF" },
                &if candles_match { fmt(dif) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "DEA" } else { "DEA" },
                &if candles_match { fmt(dea) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "HIST" } else { "柱" },
                &if candles_match { fmt(hist) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "Mode" } else { "显示" },
                if self.show_macd { "开" } else { "关" },
                cx,
            ))
    }

    fn render_boll_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref())
            && !matches!(self.chart_kind, ChartKind::Intraday);
        let (up, mid, low) = self.boll.value_at(self.boll.mid.len().saturating_sub(1));
        let fmt = |v: Option<f64>| {
            v.map(|n| self.format_value(n))
                .unwrap_or_else(|| "--".into())
        };
        v_flex()
            .gap_1()
            .min_w(px(120.))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.work_mode { "BOLL" } else { "BOLL 20·2σ" }),
            )
            .child(detail_row(
                "上轨",
                &if candles_match { fmt(up) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                "中轨",
                &if candles_match { fmt(mid) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                "下轨",
                &if candles_match { fmt(low) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "Mode" } else { "显示" },
                if self.show_boll { "开" } else { "关" },
                cx,
            ))
    }

    fn render_signal_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let signal = self.current_signal();

        let mut root = h_flex().w_full().gap_4().items_start();

        if let Some(s) = signal {
            let fmt = |v: Option<f64>, suffix: &str| {
                v.map(|n| format!("{n:.1}{suffix}"))
                    .unwrap_or_else(|| "—".into())
            };
            let regime = if work {
                s.regime.service_state()
            } else {
                s.regime.label()
            };
            root = root
                .child(self.render_score_badge(Some(&s), cx))
                .child(
                    v_flex()
                        .gap_1()
                        .min_w(px(220.))
                        .flex_1()
                        .child(section_title(
                            if work {
                                "Factors"
                            } else {
                                "策略雷达 · 多因子"
                            },
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Composite" } else { "综合" },
                            &format!("{:.0}/100 · {regime}", s.score),
                            cx,
                        ))
                        .child(detail_kv("RSI14", &fmt(s.rsi14, ""), cx))
                        .child(detail_kv(
                            if work { "Mom 20d" } else { "20日动量" },
                            &fmt(s.momentum_20_pct, "%"),
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Vol 20d ann" } else { "20日年化波动" },
                            &fmt(s.volatility_20_ann_pct, "%"),
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Max DD 1Y" } else { "1Y最大回撤" },
                            &fmt(s.max_drawdown_1y_pct, "%"),
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Vol ratio" } else { "量能比" },
                            &fmt(s.volume_ratio_20, "x"),
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Confidence" } else { "数据置信" },
                            &format!("{:.0}%", s.confidence),
                            cx,
                        )),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .min_w(px(200.))
                        .flex_1()
                        .child(section_title(if work { "Rationale" } else { "依据" }, cx))
                        .children(s.reasons.iter().map(|r| {
                            div()
                                .px_2()
                                .py_1()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().muted.opacity(0.45))
                                .text_xs()
                                .text_color(cx.theme().foreground)
                                .child((*r).to_string())
                        }))
                        .child(
                            div()
                                .mt_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.8))
                                .child(if work {
                                    "Explainable snapshot, not a trade instruction."
                                } else {
                                    "可解释技术快照，仅供学习研究，不构成投资建议。"
                                }),
                        ),
                );
        } else {
            root = root.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "Need ≥20 valid daily bars."
                    } else {
                        "至少需要 20 根有效日 K 数据。"
                    }),
            );
        }
        root
    }

    fn render_ai_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let current = self.ai_current_key();
        let shown = current.is_some() && self.ai_key.as_deref() == current.as_deref();
        let loading = matches!(&self.ai_panel, AiPanelState::Loading { .. });
        // 只有「正在分析当前标的」时才禁用按钮；其他标的可并行触发。
        let busy = shown && loading;
        let has_signal = self.current_signal().is_some();

        let mut col = v_flex()
            .gap_2()
            .w_full()
            .max_w(px(720.))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(section_title(if work { "AI Brief" } else { "AI 点评" }, cx))
                    .child(
                        Button::new("ai-request-btn")
                            .xsmall()
                            .when(!busy && has_signal, |b| b.primary())
                            .when(busy || !has_signal, |b| b.ghost())
                            .label(if busy {
                                if work { "Working…" } else { "分析中…" }
                            } else if work {
                                "Generate"
                            } else {
                                "生成点评"
                            })
                            .disabled(busy || !has_signal)
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.request_ai_commentary(cx);
                            })),
                    ),
            );

        if shown {
            match &self.ai_panel {
                AiPanelState::Loading { text } => {
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(text.clone()),
                    );
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work {
                                "LLM brief in progress…"
                            } else {
                                "正在请求 LLM 点评…"
                            }),
                    );
                }
                AiPanelState::Ready { text, source, note } => {
                    let source_color = if source.is_llm() {
                        cx.theme().accent
                    } else {
                        cx.theme().muted_foreground
                    };
                    col = col.child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "Source" } else { "来源" }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(source_color)
                                    .child(source.label(work)),
                            ),
                    );
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(text.clone()),
                    );
                    if let Some(note) = note {
                        col = col.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.9))
                                .child(note.clone()),
                        );
                    }
                }
                AiPanelState::Idle => {
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Not generated." } else { "尚未生成。" }),
                    );
                }
            }
        } else if !has_signal {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("至少需要 20 根有效日K数据。"),
            );
        } else {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "Click Generate for an AI brief."
                    } else {
                        "点击「生成点评」查看 AI 分析。"
                    }),
            );
        }

        col.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground.opacity(0.75))
                .child(if work {
                    "For reference only, not investment advice."
                } else {
                    "仅供学习研究，不构成投资建议。"
                }),
        )
    }

    fn render_treasure_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let hit = self
            .treasure_hits
            .iter()
            .find(|h| h.code == self.selected.as_ref())
            .cloned();

        // 参考建仓/减仓带：优先用当前图表日 K（与选中标的匹配时）。
        let levels = self
            .candles_code
            .as_ref()
            .filter(|c| c.as_str() == self.selected.as_ref())
            .and_then(|_| levels::compute(&self.candles));

        let mut col = v_flex().gap_2().w_full().max_w(px(640.)).child(section_title(
            if work {
                "Scout · levels"
            } else {
                "寻宝鼠 · 搜罗价位"
            },
            cx,
        ));

        if let Some(lv) = levels.as_ref() {
            col = col
                .child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .child(metric_chip(
                            if work { "Spot" } else { "现价" },
                            &format_price(lv.close),
                            cx,
                        ))
                        .child(metric_chip(
                            if work { "Buy band" } else { "建仓带" },
                            &lv.buy_band_text(),
                            cx,
                        ))
                        .child(metric_chip(
                            if work { "Sell band" } else { "减仓带" },
                            &lv.sell_band_text(),
                            cx,
                        )),
                )
                .child(detail_kv(
                    if work { "Buy (ref)" } else { "参考建仓" },
                    &format!("{} 元（支撑侧分批观察）", lv.buy_band_text()),
                    cx,
                ))
                .child(detail_kv(
                    if work { "Sell (ref)" } else { "参考减仓" },
                    &format!("{} 元（阻力侧反弹观察）", lv.sell_band_text()),
                    cx,
                ));
            if let Some(atr) = lv.atr14 {
                col = col.child(detail_kv(
                    "ATR14",
                    &format!("{} 元", format_price(atr)),
                    cx,
                ));
            }
            if !lv.notes.is_empty() {
                col = col.child(detail_kv(
                    if work { "Basis" } else { "依据" },
                    &lv.notes.join("；"),
                    cx,
                ));
            }
        }

        if let Some(h) = hit {
            let tags = h
                .tags
                .iter()
                .map(|t| t.label())
                .collect::<Vec<_>>()
                .join(" · ");
            let tags_disp = if tags.is_empty() {
                "—".to_string()
            } else {
                tags
            };
            col = col
                .child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .child(metric_chip(
                            if work { "Score" } else { "分数" },
                            &format!("{:.1}", h.score),
                            cx,
                        ))
                        .child(metric_chip(
                            if work { "Bars" } else { "样本" },
                            &format!("{}", h.bars),
                            cx,
                        ))
                        .child(metric_chip(
                            if work { "Src" } else { "来源" },
                            &h.source,
                            cx,
                        )),
                )
                .child(detail_kv(
                    if work { "Position" } else { "位置" },
                    &format!(
                        "1Y {} · 3Y {} · 全 {}",
                        fmt_pos(h.pos_1y),
                        fmt_pos(h.pos_3y),
                        fmt_pos(h.pos_all)
                    ),
                    cx,
                ))
                .child(detail_kv(
                    if work { "Percentile" } else { "分位" },
                    &format!(
                        "1Y {} · 3Y {} · 全 {}",
                        fmt_pos(h.pctile_1y),
                        fmt_pos(h.pctile_3y),
                        fmt_pos(h.pctile_all)
                    ),
                    cx,
                ))
                .child(detail_kv(
                    if work { "Drawdown" } else { "回撤" },
                    &format!("1Y {} · 全 {}", fmt_dd(h.dd_1y), fmt_dd(h.dd_all)),
                    cx,
                ))
                .child(detail_kv(if work { "Tags" } else { "标签" }, &tags_disp, cx))
                .when(!work, |c| {
                    c.child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "搜罗：低位扫描 + 技术参考价位。建仓/减仓带为本地指标推算，非买卖指令。仅供学习研究。",
                            ),
                    )
                });
        } else if levels.is_none() {
            col = col
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if work {
                            "Not in latest scan. Open the left Scan tab."
                        } else {
                            "当前标的不在最近寻宝结果中。可打开左侧「寻宝」扫描；加载日 K 后也会显示参考价位。"
                        }),
                )
                .child(
                    Button::new("open-treasure-tab")
                        .xsmall()
                        .ghost()
                        .label(if work { "Open Scan" } else { "打开寻宝" })
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.set_left_tab(LeftTab::Treasure, cx);
                        })),
                );
        } else if !work {
            col = col.child(
                div()
                    .mt_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "当前不在寻宝榜内，仍可根据日 K 显示技术参考价位。左侧「寻宝」可扩大搜罗。仅供学习，非投资建议。",
                    ),
            );
        }
        col
    }

    fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let local: Vec<(usize, Symbol)> = self
            .filtered_local
            .iter()
            .filter_map(|&i| self.symbols.get(i).cloned().map(|s| (i, s)))
            .collect();
        let remote = self.palette_hits.clone();

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(72.))
            .bg(gpui::hsla(0., 0., 0., 0.55))
            // Same modal isolation as the settings overlay: don't let wheel
            // scrolling or hover styles reach the app behind the palette.
            .occlude()
            .child(
                v_flex()
                    .id("palette-panel")
                    .w(px(560.))
                    .max_h(px(480.))
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .overflow_hidden()
                    .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
                        this.palette_open = false;
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .h(px(48.))
                            .px_3()
                            .items_center()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(div().flex_1().child(Input::new(&self.palette_query))),
                    )
                    .child({
                        let mut list = v_flex()
                            .id("palette-results")
                            .flex_1()
                            .overflow_y_scroll()
                            .p_1();
                        if !local.is_empty() {
                            list = list.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.work_mode { "List" } else { "自选" }),
                            );
                            for (i, (_, sym)) in local.into_iter().enumerate() {
                                list = list.child(palette_row(
                                    sym,
                                    true,
                                    i as u64,
                                    self.color_scheme,
                                    self.work_mode,
                                    self.work_identity_reveal,
                                    cx,
                                ));
                            }
                        }
                        if !remote.is_empty() {
                            list = list.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.work_mode {
                                        "Results (click to add)"
                                    } else {
                                        "搜索结果（点击添加）"
                                    }),
                            );
                            for (i, sym) in remote.into_iter().enumerate() {
                                list = list.child(palette_row(
                                    sym,
                                    false,
                                    10_000 + i as u64,
                                    self.color_scheme,
                                    self.work_mode,
                                    self.work_identity_reveal,
                                    cx,
                                ));
                            }
                        }
                        if self.filtered_local.is_empty() && self.palette_hits.is_empty() {
                            list = list.child(
                                div()
                                    .p_4()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.work_mode {
                                        "Type an id or name to search"
                                    } else {
                                        "输入代码或名称搜索 A 股"
                                    }),
                            );
                        }
                        list
                    })
                    .child(
                        h_flex()
                            .h(px(28.))
                            .px_3()
                            .items_center()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.work_mode {
                                        "⌘K toggle · click outside to close · local config"
                                    } else {
                                        "⌘K 开关 · 点击外部关闭 · 配置自动保存"
                                    }),
                            ),
                    ),
            )
    }
}

/// Full date for hover strip: `YYYY-MM-DD` when available, else original.
fn format_candle_date(raw: &str) -> String {
    let s = raw.trim();
    if s.len() >= 10 && s.as_bytes().get(4) == Some(&b'-') {
        if s.len() > 10 && s.as_bytes().get(10) == Some(&b' ') {
            // minute bar: `2026-07-31 15:00` → `07-31 15:00`
            format!("{}{}", &s[5..10], &s[10..])
        } else {
            // 2026-07-29 → keep ISO; also accept already short forms
            s[..10].to_string()
        }
    } else if s.len() == 5 && s.contains('/') {
        // legacy MM/DD — cannot recover year
        s.to_string()
    } else {
        s.to_string()
    }
}

/// Line color palette for user-drawn chart lines (cycles by `color_ix`).
fn chart_line_palette(theme: &gpui_component::Theme) -> Vec<gpui::Hsla> {
    vec![
        theme.yellow,
        theme.cyan,
        theme.magenta,
        theme.blue,
        theme.green,
        theme.red,
        theme.accent,
    ]
}

fn palette_row(
    sym: Symbol,
    in_watchlist: bool,
    row_id: u64,
    color_scheme: ColorScheme,
    work_mode: bool,
    reveal_identity: bool,
    cx: &mut Context<StockApp>,
) -> impl IntoElement {
    let code = sym.code.clone();
    let name = sym.name.to_string();
    let code_width = if work_mode && reveal_identity { 180.0 } else { 64.0 };
    let code_show = if work_mode && reveal_identity {
        if is_real_name(sym.name.as_ref(), &sym.code) {
            format!("{} · {}", sym.name, sym.code)
        } else {
            sym.code.clone()
        }
    } else if work_mode {
        disguise_label(&sym.code, sym.name.as_ref())
    } else {
        sym.code.clone()
    };
    let name_show = if work_mode {
        shared("series")
    } else {
        sym.name.clone()
    };
    let board = if work_mode {
        shared("svc")
    } else {
        sym.board.clone()
    };
    let last = if work_mode {
        if sym.last > 0.0 {
            format!("{}ms", format_price(sym.last))
        } else {
            "--".into()
        }
    } else {
        format_price(sym.last)
    };
    let chg = if work_mode {
        format!("{:+.2}", sym.change_pct)
    } else {
        format_pct(sym.change_pct)
    };
    let up = sym.is_up();
    let chg_color = if work_mode {
        if up {
            cx.theme().muted_foreground
        } else {
            cx.theme().muted_foreground.opacity(0.65)
        }
    } else {
        match color_scheme {
            ColorScheme::Cn => {
                if up {
                    cx.theme().red
                } else {
                    cx.theme().green
                }
            }
            ColorScheme::Us => {
                if up {
                    cx.theme().green
                } else {
                    cx.theme().red
                }
            }
        }
    };

    div()
        .id(("palette-item", row_id))
        .h(px(40.))
        .px_3()
        .rounded(cx.theme().radius)
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .hover(|this| this.bg(cx.theme().accent.opacity(0.15)))
        .on_click(cx.listener(move |this, _, window, cx| {
            if in_watchlist {
                this.select_symbol(shared(code.clone()), cx);
            } else {
                this.add_symbol(code.clone(), name.clone(), window, cx);
            }
        }))
        .child(
            div()
                .w(px(code_width))
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(code_show),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(name_show),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(board),
        )
        .when(in_watchlist, |this| {
            this.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(last),
            )
            .child(
                div()
                    .w(px(64.))
                    .text_right()
                    .text_xs()
                    .text_color(chg_color)
                    .child(chg),
            )
        })
        .when(!in_watchlist, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().accent)
                    .child(if work_mode { "attach" } else { "添加" }),
            )
        })
}

/// Host metric bar: shows `value_text` (e.g. +0.52%), bar fill 0–100, hover → real index.
fn sys_gauge(
    id: u64,
    label: &str,
    value_text: String,
    bar_pct: f64,
    tooltip_text: String,
    cx: &App,
) -> impl IntoElement {
    let bar_pct = bar_pct.clamp(0.0, 100.0);
    let fill_w = (bar_pct / 100.0 * 140.0) as f32;
    let tip = tooltip_text.clone();
    h_flex()
        .id(("sys-gauge", id))
        .w_full()
        .items_center()
        .gap_2()
        .cursor_default()
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(
            div()
                .w(px(36.))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .h(px(8.))
                .rounded_full()
                .bg(cx.theme().muted.opacity(0.55))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(px(fill_w))
                        .rounded_full()
                        .bg(cx.theme().blue.opacity(0.85)),
                ),
        )
        .child(
            div()
                .w(px(112.))
                .whitespace_nowrap()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(value_text),
        )
}

fn detail_row(label: &str, value: &str, cx: &App) -> impl IntoElement {
    detail_kv(label, value, cx)
}

/// Label/value row with room for Chinese multi-char keys.
fn detail_kv(label: &str, value: &str, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .w(px(88.))
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(value.to_string()),
        )
}

fn section_title(text: &str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

/// Compact metric pill used on the overview / treasure dashboards.
fn metric_chip(label: &str, value: &str, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .px_2()
        .py_1()
        .min_w(px(72.))
        .rounded(cx.theme().radius)
        .bg(cx.theme().background)
        .border_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(value.to_string()),
        )
}

/// 是否为可用的中文/展示名称（排除空、占位、与代码相同）。
fn is_real_name(name: &str, code: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n != "--" && n != code && n != code.trim_start_matches('0')
}

fn display_name_str(name: &str, code: &str) -> String {
    if is_real_name(name, code) {
        name.trim().to_string()
    } else {
        String::new()
    }
}

/// Strip common fund/ETF suffixes so long names can be shortened cleanly.
/// e.g. `华泰柏瑞沪深300ETF` → `华泰柏瑞沪深300`
fn strip_fund_suffix(name: &str) -> &str {
    let n = name.trim();
    for suffix in ["ETF联接", "ETF", "LOF", "基金"] {
        if let Some(rest) = n.strip_suffix(suffix) {
            let rest = rest.trim_end();
            if !rest.is_empty() {
                return rest;
            }
        }
    }
    n
}

/// Compact label for the macOS menu bar (space is tight).
/// Keeps a real Chinese/name fragment even for long ETF titles — never
/// collapses a known name to the bare code (that produced `588710 588710`).
fn short_status_name(name: &str, code: &str) -> String {
    if !is_real_name(name, code) {
        return code.to_string();
    }
    let n = strip_fund_suffix(name);
    let chars: Vec<char> = n.chars().collect();
    // ≤6: show full (most stocks + short fund nicknames).
    // Longer: keep first 4 chars (e.g. 华泰柏瑞沪深300 → 华泰柏瑞).
    if chars.len() <= 6 {
        n.to_string()
    } else {
        chars.into_iter().take(4).collect()
    }
}

#[cfg(test)]
mod name_label_tests {
    use super::{is_real_name, short_status_name, strip_fund_suffix};

    #[test]
    fn strip_etf_suffix() {
        assert_eq!(strip_fund_suffix("华泰柏瑞沪深300ETF"), "华泰柏瑞沪深300");
        assert_eq!(strip_fund_suffix("科创板50ETF"), "科创板50");
        assert_eq!(strip_fund_suffix("比亚迪"), "比亚迪");
    }

    #[test]
    fn short_name_keeps_etf_fragment() {
        // Long ETF names must NOT collapse to bare code (was: "588710 588710").
        let s = short_status_name("华泰柏瑞沪深300ETF", "510300");
        assert_ne!(s, "510300");
        assert!(s.chars().count() <= 6);
        assert!(!s.is_empty());
    }

    #[test]
    fn short_name_keeps_stock() {
        assert_eq!(short_status_name("比亚迪", "002594"), "比亚迪");
        assert_eq!(short_status_name("贵州茅台", "600519"), "贵州茅台");
    }

    #[test]
    fn missing_name_falls_back_to_code() {
        assert_eq!(short_status_name("", "588710"), "588710");
        assert_eq!(short_status_name("588710", "588710"), "588710");
        assert!(!is_real_name("588710", "588710"));
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
                self.persist();
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
                if !this.palette_open && !this.settings_open {
                    this.select_adjacent_symbol(-1, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectNextSymbol, _w, cx| {
                if !this.palette_open && !this.settings_open {
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
            Some((x, y, w, h)) if w >= 800.0 && h >= 500.0 => {
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

    /// Shared window/App setup for the layout regression tests: a throwaway
    /// HOME with a copy of the real config, so the persisted dock state is
    /// reproduced without mutating the user's config.
    fn test_window(
        cx: &mut TestAppContext,
        w: f32,
        h: f32,
    ) -> VisualTestContext {
        let tmp = std::env::temp_dir().join(format!("stock-analysis-test-{}", std::process::id()));
        let cfg_dir = tmp.join("Library/Application Support/stock-analysis");
        std::fs::create_dir_all(&cfg_dir).expect("create temp config dir");
        if let Some(data_dir) = dirs::data_dir() {
            let src = data_dir.join("stock-analysis/config.json");
            if src.exists() {
                std::fs::copy(&src, cfg_dir.join("config.json")).expect("copy config");
            }
        }
        unsafe {
            std::env::set_var("HOME", &tmp);
        }

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
