//! Root application: A-share watchlist, chart (MA + crosshair), resizable layout, persistence.

use std::time::Duration;

use gpui::{
    actions, canvas, div, px, size, App, AppContext, Context, Entity, FocusHandle,
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
    index_from_x, paint_chart, paint_sparkline, ChartPaintData, ChartStyle, MinutePaintData,
};
use crate::data::treasure::{self, fmt_dd, fmt_pos, TreasureHit, TREASURE_KLINE_LIMIT};
use crate::data::universe::{self, TREASURE_SCAN_CAP, TREASURE_TOP_N};
use crate::data::{indicators::MaSeries, market, signals};
use crate::data::market::Sourced;
use crate::model::{
    board_for_code, disguise_index, disguise_label, format_index, format_pct, format_price,
    format_volume, shared, Candle, IndexSnap, MinutePeriod, MinuteSeries, QuoteSnapshot, Symbol,
};
use crate::storage::{self, clamp_quote_interval_secs, AppConfig, ColorScheme, WatchlistSort};
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
    hover_ix: Option<usize>,
    /// Visible window into `candles` for zoom/pan (inclusive start).
    chart_view_start: usize,
    /// Number of candles shown; clamped to series length. `0` means “show all”.
    chart_view_count: usize,
    chart_width: f32,
    chart_origin: Point<Pixels>,
    status: SharedString,
    loading: bool,
    data_source: SharedString,
    palette_open: bool,
    /// Settings overlay.
    settings_open: bool,
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
    /// 寻宝扫描结果（按 score 降序）。
    treasure_hits: Vec<TreasureHit>,
    treasure_scanning: bool,
    treasure_done: usize,
    treasure_total: usize,
    treasure_status: SharedString,
    /// 取消过期扫描。
    treasure_gen: u64,
    /// 上证综指（work 模式 cpu）。
    index_sh: Option<IndexSnap>,
    /// 沪深300（work 模式 mem）。
    index_hs300: Option<IndexSnap>,
    /// 创业板指（work 模式 disk）。
    index_cyb: Option<IndexSnap>,
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

        let _subscriptions = vec![cx.subscribe_in(&palette_query, window, {
            move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change) {
                    let q = state.read(cx).value().to_string();
                    this.on_palette_query_changed(&q, cx);
                }
            }
        })];

        let treasure_cache = storage::load_treasure_cache();
        let treasure_hits = treasure_cache.hits;
        let treasure_status = if treasure_hits.is_empty() {
            shared("点「开始寻宝」扫描自选+扩展池的多年低位")
        } else {
            shared(format!(
                "缓存 {} 只 · {}",
                treasure_hits.len(),
                if treasure_cache.updated_at.is_empty() {
                    "—".into()
                } else {
                    treasure_cache.updated_at
                }
            ))
        };

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
            hover_ix: None,
            chart_view_start: 0,
            chart_view_count: 0,
            chart_width: 800.0,
            chart_origin: Point::default(),
            status: shared("正在连接行情源…"),
            loading: true,
            data_source: shared(market::SRC_LABEL),
            palette_open: false,
            settings_open: false,
            update_state: UpdateState::Idle,
            quote_interval_secs: clamp_quote_interval_secs(cfg.quote_interval_secs),
            palette_query,
            palette_focus,
            palette_hits: Vec::new(),
            filtered_local,
            left_width: cfg.left_width,
            bottom_height: cfg.bottom_height,
            color_scheme: cfg.color_scheme,
            work_mode: cfg.work_mode,
            work_identity_reveal: false,
            watchlist_sort: cfg.watchlist_sort,
            quote_fail_streak: 0,
            left_tab: LeftTab::Watchlist,
            treasure_hits,
            treasure_scanning: false,
            treasure_done: 0,
            treasure_total: 0,
            treasure_status,
            treasure_gen: 0,
            index_sh: None,
            index_hs300: None,
            index_cyb: None,
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
            left_width: self.left_width,
            bottom_height: self.bottom_height,
            color_scheme: self.color_scheme,
            work_mode: self.work_mode,
            quote_interval_secs: self.quote_interval_secs,
            watchlist_sort: self.watchlist_sort,
        };
        let _ = storage::save_config(&cfg);
    }

    fn dismiss_overlay(&mut self, cx: &mut Context<Self>) {
        if self.palette_open || self.settings_open {
            self.palette_open = false;
            self.settings_open = false;
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
        if let Some(w) = sizes.first() {
            let w = w.as_f32();
            if (w - self.left_width).abs() > 0.5 {
                self.left_width = w;
                self.persist();
            }
        }
    }

    fn on_main_v_resize(
        &mut self,
        state: &Entity<ResizableState>,
        cx: &mut Context<Self>,
    ) {
        let sizes = state.read(cx).sizes().clone();
        // Vertical group: [chart, detail] — persist detail (bottom) height.
        if let Some(h) = sizes.get(1) {
            let h = h.as_f32();
            if (h - self.bottom_height).abs() > 0.5 {
                self.bottom_height = h;
                self.persist();
            }
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

    fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.palette_open = false;
        }
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

        self.treasure_gen = self.treasure_gen.wrapping_add(1);
        let scan_id = self.treasure_gen;
        self.treasure_scanning = true;
        self.treasure_done = 0;
        self.treasure_total = 0;
        self.treasure_hits.clear();
        self.treasure_status = shared(format!(
            "拉取扩大池（最多 {TREASURE_SCAN_CAP}）· 将取 Top {TREASURE_TOP_N}…"
        ));
        self.status = shared(format!(
            "🐭 扩大搜索 · 市值池≤{TREASURE_SCAN_CAP} · 入榜{TREASURE_TOP_N}"
        ));
        self.left_tab = LeftTab::Treasure;
        cx.notify();

        cx.spawn(async move |this, cx| {
            // 网络拉扩大候选（失败则内置表）
            let (codes, pool_src) = smol::unblock(move || {
                universe::build_scan_universe_expanded(&watchlist)
            })
            .await;

            if codes.is_empty() {
                let _ = this.update(cx, |app, cx| {
                    if app.treasure_gen != scan_id {
                        return;
                    }
                    app.treasure_scanning = false;
                    app.treasure_status = shared("没有可扫描代码");
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
                    "池 {total} 只（{pool_src}）· 深评中 0/{total} · 将取 Top {TREASURE_TOP_N}"
                ));
                app.status = shared(format!("🐭 深评 {total} 只 · 源 {pool_src}"));
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
                    "完成 · 深评 {total} · 入榜 {} · {pool_src} · {updated_at}",
                    app.treasure_hits.len(),
                ));
                app.status = shared(format!(
                    "🐭 寻宝完成 · Top {} / 扫描 {total}（多窗口·上行中继降权）",
                    app.treasure_hits.len(),
                ));
                app.persist();
                cx.notify();
            });
        })
        .detach();
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
        self.persist();
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
        let work = self.work_mode;
        let minute = if matches!(self.chart_kind, ChartKind::Intraday) {
            self.minute_paint_data(cx)
        } else {
            None
        };
        ChartPaintData {
            candles,
            ma,
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
                                .label(if work { "Prefs" } else { "设置" })
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

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let interval = self.quote_interval_secs;
        let scheme = self.color_scheme;

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(72.))
            .bg(gpui::hsla(0., 0., 0., 0.55))
            .child(
                v_flex()
                    .id("settings-panel")
                    .w(px(440.))
                    .max_h(px(520.))
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .overflow_hidden()
                    .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
                        this.settings_open = false;
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .h(px(44.))
                            .px_4()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(if work { "Preferences" } else { "设置" }),
                            )
                            .child(
                                Button::new("settings-close")
                                    .ghost()
                                    .xsmall()
                                    .label(if work { "Close" } else { "关闭" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.settings_open = false;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("settings-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .p_4()
                            .gap_4()
                            // —— Quote interval ——
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if work {
                                                "Poll interval"
                                            } else {
                                                "行情刷新间隔"
                                            }),
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
                            // —— Color scheme ——
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if work {
                                                "Color scheme"
                                            } else {
                                                "涨跌配色"
                                            }),
                                    )
                                    .child(
                                        h_flex().gap_1().children(
                                            [ColorScheme::Cn, ColorScheme::Us].map(|s| {
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
                                            }),
                                        ),
                                    ),
                            )
                            // —— Work mode ——
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if work {
                                                "Focus layout"
                                            } else {
                                                "工作模式"
                                            }),
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
                            // —— Update ——
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
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
                                                    .label(if work {
                                                        "Check"
                                                    } else {
                                                        "检查更新"
                                                    })
                                                    .disabled(matches!(
                                                        self.update_state,
                                                        UpdateState::Checking
                                                            | UpdateState::Downloading(_)
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
                                                        .label(if work {
                                                            "Update now"
                                                        } else {
                                                            "立即更新"
                                                        })
                                                        .on_click(cx.listener(
                                                            |this, _, _w, cx| {
                                                                this.start_update(cx);
                                                            },
                                                        )),
                                                ),
                                                _ => None,
                                            }),
                                    ),
                            )
                            // —— About / compliance ——
                            .child(
                                v_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
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
                                    ),
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
                            ),
                    ),
            )
    }

    fn render_left_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
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
                            .child(
                                Button::new("treasure-scan")
                                    .xsmall()
                                    .primary()
                                    .label(if self.treasure_scanning {
                                        if work {
                                            "Running…"
                                        } else {
                                            "扫描中…"
                                        }
                                    } else if work {
                                        "Start"
                                    } else {
                                        "开始寻宝"
                                    })
                                    .disabled(self.treasure_scanning)
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_treasure_scan(cx);
                                    })),
                            )
                            .child(
                                Button::new("treasure-cancel")
                                    .xsmall()
                                    .ghost()
                                    .label(if work { "Cancel" } else { "取消" })
                                    .disabled(!self.treasure_scanning)
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.cancel_treasure_scan(cx);
                                    })),
                            ),
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
                            .child(format!(
                                "市值池≤{TREASURE_SCAN_CAP} · Top{TREASURE_TOP_N} · 1Y/3Y/全 · 前复权"
                            )),
                    ),
            )
            .child(
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
                            .child("标的 / 位置"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("分"),
                    ),
            )
            .child(
                v_flex()
                    .id("treasure-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(self.treasure_hits.is_empty() && !self.treasure_scanning, |el| {
                        el.child(
                            div()
                                .p_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "尚无结果。将从东财按市值扩大候选（最多 {TREASURE_SCAN_CAP} 只，含自选），\
                                     多窗口评分后取 Top {TREASURE_TOP_N}。\
                                     「上行中继回撤」表示一年低但多年仍高。"
                                )),
                        )
                    })
                    .children(self.treasure_hits.iter().enumerate().map(|(ix, hit)| {
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
                    })),
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

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // header
            .child(
                h_flex()
                    .h(px(52.))
                    .px_4()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_3()
                            .items_baseline()
                            .child(
                                div()
                                    .text_xl()
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
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_baseline()
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
            // toolbar: OHLC + range + MA toggles
            .child(
                h_flex()
                    .h(px(36.))
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child({
                        if candles_match {
                            let o = snap.as_ref().map(|s| s.open).unwrap_or(0.0);
                            let h = snap.as_ref().map(|s| s.high).unwrap_or(0.0);
                            let l = snap.as_ref().map(|s| s.low).unwrap_or(0.0);
                            let v = snap.as_ref().map(|s| s.volume).unwrap_or(0);
                            if work {
                                h_flex()
                                    .gap_3()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("min {}", self.format_value(l)))
                                    .child(format!("max {}", self.format_value(h)))
                                    .child(format!("pts {}", format_volume(v)))
                            } else {
                                h_flex()
                                    .gap_3()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("开 {}", format_price(o)))
                                    .child(format!("高 {}", format_price(h)))
                                    .child(format!("低 {}", format_price(l)))
                                    .child(format!("量 {}", format_volume(v)))
                            }
                        } else {
                            h_flex()
                                .gap_3()
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
                        }
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .when(
                                !matches!(self.chart_kind, ChartKind::Intraday),
                                |row| {
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
                                    })
                                },
                            )
                            .child(div().w(px(8.)))
                            .child(self.kind_button("分时", ChartKind::Intraday, cx))
                            .child(self.kind_button("日K", ChartKind::DayK, cx))
                            .child(
                                self.kind_button(
                                    "分钟",
                                    ChartKind::MinuteK(self.current_minute_period()),
                                    cx,
                                ),
                            )
                            .when(matches!(self.chart_kind, ChartKind::DayK), |row| {
                                row.child(div().w(px(8.)))
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
                            .when(
                                matches!(self.chart_kind, ChartKind::MinuteK(_)),
                                |row| {
                                    row.child(div().w(px(8.)))
                                        .children(MinutePeriod::all().map(|p| {
                                            let active =
                                                self.chart_kind == ChartKind::MinuteK(p);
                                            Button::new(("mperiod", p as u32))
                                                .xsmall()
                                                .when(active, |b| b.primary())
                                                .when(!active, |b| b.ghost())
                                                .label(p.label())
                                                .on_click(cx.listener(
                                                    move |this, _, _w, cx| {
                                                        this.set_chart_kind(
                                                            ChartKind::MinuteK(p),
                                                            cx,
                                                        );
                                                    },
                                                ))
                                        }))
                                },
                            ),
                    ),
            )
            // hover strip
            .child(
                h_flex()
                    .h(px(26.))
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
                    _ => {}
                }
                this.persist();
                cx.notify();
            }))
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
                        .into_any_element();
                }
            }
        }
        let (vs, ve) = self.chart_visible_range();
        let zoom_hint = if matches!(self.chart_kind, ChartKind::Intraday) {
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

    fn render_detail_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
        let code = self.selected.as_ref();
        let name_raw = sym.map(|s| s.name.as_ref()).unwrap_or("");
        let title = if self.work_mode {
            self.display_code(code)
        } else if is_real_name(name_raw, code) {
            format!("{code} {name_raw}")
        } else {
            code.to_string()
        };

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h(px(32.))
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.work_mode {
                                "Inspector"
                            } else {
                                "详情 / 状态"
                            }),
                    ),
            )
            .child(
                h_flex()
                    .id("detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .overflow_y_scroll()
                    .p_3()
                    .gap_5()
                    .items_start()
                    .child(
                        v_flex()
                            .gap_1()
                            .min_w(px(180.))
                            .child(detail_row(
                                if self.work_mode { "Item" } else { "标的" },
                                &title,
                                cx,
                            ))
                            .child(detail_row(
                                if self.work_mode { "Group" } else { "板块" },
                                if self.work_mode {
                                    "svc"
                                } else {
                                    sym.map(|s| s.board.as_ref()).unwrap_or("--")
                                },
                                cx,
                            ))
                            .child(detail_row(
                                if self.work_mode { "Range" } else { "周期" },
                                &if self.work_mode {
                                    format!("{} · {} pts", self.chart_label(), self.candles.len())
                                } else {
                                    format!("{} · {} 根", self.chart_label(), self.candles.len())
                                },
                                cx,
                            ))
                            .child(detail_row(
                                if self.work_mode { "Base" } else { "昨收" },
                                &self.format_value(
                                    snap.as_ref().map(|s| s.prev_close).unwrap_or(0.0),
                                ),
                                cx,
                            ))
                            .child(detail_row(
                                if self.work_mode { "Theme" } else { "配色" },
                                if self.work_mode {
                                    "neutral"
                                } else {
                                    self.color_scheme.label()
                                },
                                cx,
                            )),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .min_w(px(120.))
                            .child(detail_row(
                                if self.work_mode { "L1" } else { "MA5" },
                                &if candles_match {
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
                                if self.work_mode { "L2" } else { "MA10" },
                                &if candles_match {
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
                                if self.work_mode { "L3" } else { "MA20" },
                                &if candles_match {
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
                                if self.work_mode { "L4" } else { "MA60" },
                                &if candles_match {
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
                            )),
                    )
                    .child(self.render_signal_detail_col(cx))
                    .child(self.render_treasure_detail_col(cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(160.))
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.work_mode { "Status" } else { "状态" }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(self.status.clone()),
                            )
                            .when(!self.work_mode, |col| {
                                col.child(
                                    div()
                                        .mt_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(
                                            "1Y/3Y/全样本评分；「上行中继回撤」= 一年低但多年仍高。仅供学习，非投资建议。",
                                        ),
                                )
                            }),
                    ),
            )
    }

    fn render_signal_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let signal = self.current_signal();
        let mut col = v_flex().gap_1().min_w(px(240.)).child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child("策略雷达 · 多因子"),
        );

        if let Some(s) = signal {
            let fmt = |v: Option<f64>, suffix: &str| {
                v.map(|n| format!("{n:.1}{suffix}"))
                    .unwrap_or_else(|| "—".into())
            };
            let reasons = s.reasons.iter().take(4).copied().collect::<Vec<_>>().join(" · ");
            col = col
                .child(detail_row(
                    "综合",
                    &format!("{:.0}/100 · {}", s.score, s.regime.label()),
                    cx,
                ))
                .child(detail_row("RSI14", &fmt(s.rsi14, ""), cx))
                .child(detail_row("20日动量", &fmt(s.momentum_20_pct, "%"), cx))
                .child(detail_row(
                    "20日年化波动",
                    &fmt(s.volatility_20_ann_pct, "%"),
                    cx,
                ))
                .child(detail_row(
                    "1Y最大回撤",
                    &fmt(s.max_drawdown_1y_pct, "%"),
                    cx,
                ))
                .child(detail_row("量能比", &fmt(s.volume_ratio_20, "x"), cx))
                .child(detail_row("数据置信", &format!("{:.0}%", s.confidence), cx))
                .child(detail_row("依据", &reasons, cx));
        } else {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("至少需要 20 根有效日K数据。"),
            );
        }
        col
    }

    fn render_treasure_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let hit = self
            .treasure_hits
            .iter()
            .find(|h| h.code == self.selected.as_ref())
            .cloned();

        let mut col = v_flex().gap_1().min_w(px(220.)).child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(if self.work_mode {
                    "Scan · multi-window"
                } else {
                    "寻宝鼠 · 多窗口"
                }),
        );

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
                .child(detail_row("分数", &format!("{:.1}", h.score), cx))
                .child(detail_row(
                    "位置",
                    &format!(
                        "1Y {} · 3Y {} · 全 {}",
                        fmt_pos(h.pos_1y),
                        fmt_pos(h.pos_3y),
                        fmt_pos(h.pos_all)
                    ),
                    cx,
                ))
                .child(detail_row(
                    "分位",
                    &format!(
                        "1Y {} · 3Y {} · 全 {}",
                        fmt_pos(h.pctile_1y),
                        fmt_pos(h.pctile_3y),
                        fmt_pos(h.pctile_all)
                    ),
                    cx,
                ))
                .child(detail_row(
                    "回撤",
                    &format!("1Y {} · 全 {}", fmt_dd(h.dd_1y), fmt_dd(h.dd_all)),
                    cx,
                ))
                .child(detail_row("标签", &tags_disp, cx))
                .child(detail_row(
                    "样本",
                    &format!("{} 根 · {}", h.bars, h.source),
                    cx,
                ));
        } else {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("当前标的不在最近寻宝结果中。可打开左侧「寻宝」扫描。"),
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
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .w(px(36.))
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

impl Render for StockApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let left_w = self.left_width;
        let bottom_h = self.bottom_height;
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
            .child(if work {
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
                                    .child(self.render_left_panel(cx)),
                            )
                            .child(
                                resizable_panel().child(
                                    v_resizable("main-v")
                                        .on_resize(move |state, _window, cx| {
                                            entity_v.update(cx, |this, cx| {
                                                this.on_main_v_resize(state, cx);
                                            });
                                        })
                                        .child(resizable_panel().child(self.render_chart_area(cx)))
                                        .child(
                                            resizable_panel()
                                                .size(px(bottom_h.max(160.0)))
                                                .size_range(px(140.)..px(420.))
                                                .child(self.render_detail_panel(cx)),
                                        ),
                                ),
                            ),
                    )
                    .into_any_element()
            })
            .when(self.palette_open, |this| this.child(self.render_palette(cx)))
            .when(self.settings_open, |this| this.child(self.render_settings(cx)))
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

        let window_options = WindowOptions {
            titlebar: Some(TitleBar::title_bar_options()),
            window_bounds: Some(WindowBounds::centered(size(px(1320.), px(860.)), cx)),
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
