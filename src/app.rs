//! Root application: A-share watchlist, chart (MA + crosshair), resizable layout, persistence.

use std::time::Duration;

use gpui::{
    actions, canvas, div, px, size, App, AppContext, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, MouseMoveEvent, ParentElement, Pixels, Point,
    Render, ScrollDelta, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Timer,
    Window, WindowBounds, WindowOptions, prelude::FluentBuilder,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    input::{Input, InputEvent, InputState},
    resizable::{h_resizable, resizable_panel, v_resizable},
    v_flex, ActiveTheme, PixelsExt, Root, Sizable, StyledExt, Theme, ThemeMode, TitleBar,
};

use crate::chart::{index_from_x, paint_chart, ChartPaintData};
use crate::data::{indicators::MaSeries, market};
use crate::model::{
    board_for_code, format_pct, format_price, format_volume, shared, Candle, QuoteSnapshot, Symbol,
};
use crate::storage::{self, AppConfig, ColorScheme};

actions!(stock, [ToggleCommandPalette, RefreshData, Quit]);

const QUOTE_INTERVAL_OK: Duration = Duration::from_secs(8);
const QUOTE_INTERVAL_ERR_MAX: Duration = Duration::from_secs(45);
/// Minimum candles visible when zoomed in.
const CHART_MIN_VISIBLE: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
enum ChartRange {
    M1 = 0,
    M3 = 1,
    M6 = 2,
    Y1 = 3,
}

impl ChartRange {
    fn label(self) -> &'static str {
        match self {
            Self::M1 => "1M",
            Self::M3 => "3M",
            Self::M6 => "6M",
            Self::Y1 => "1Y",
        }
    }

    fn bars(self) -> usize {
        match self {
            Self::M1 => 22,
            Self::M3 => 66,
            Self::M6 => 130,
            Self::Y1 => 252,
        }
    }

    fn all() -> [Self; 4] {
        [Self::M1, Self::M3, Self::M6, Self::Y1]
    }

    fn from_label(s: &str) -> Self {
        match s {
            "1M" => Self::M1,
            "6M" => Self::M6,
            "1Y" => Self::Y1,
            _ => Self::M3,
        }
    }
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
    show_ma5: bool,
    show_ma10: bool,
    show_ma20: bool,
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
    palette_query: Entity<InputState>,
    palette_focus: FocusHandle,
    /// Search results for palette (remote + local).
    palette_hits: Vec<Symbol>,
    filtered_local: Vec<usize>,
    left_width: f32,
    bottom_height: f32,
    /// 涨跌配色：中国红涨绿跌 / 美国绿涨红跌
    color_scheme: ColorScheme,
    quote_fail_streak: u32,
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

        let mut app = Self {
            symbols,
            selected,
            candles_code: None,
            kline_gen: 0,
            candles: Vec::new(),
            ma: MaSeries::default(),
            range,
            show_ma5: cfg.show_ma5,
            show_ma10: cfg.show_ma10,
            show_ma20: cfg.show_ma20,
            hover_ix: None,
            chart_view_start: 0,
            chart_view_count: 0,
            chart_width: 800.0,
            chart_origin: Point::default(),
            status: shared("正在连接行情源…"),
            loading: true,
            data_source: shared(market::SRC_LABEL),
            palette_open: false,
            palette_query,
            palette_focus,
            palette_hits: Vec::new(),
            filtered_local,
            left_width: cfg.left_width,
            bottom_height: cfg.bottom_height,
            color_scheme: cfg.color_scheme,
            quote_fail_streak: 0,
            _subscriptions,
        };

        app.bootstrap(cx);
        app
    }

    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        // Initial hydrate + klines
        self.refresh_all(cx);

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

        // Quote polling loop with backoff on failure
        cx.spawn(async move |this, cx| {
            let mut delay = QUOTE_INTERVAL_OK;
            loop {
                Timer::after(delay).await;
                let codes = match this.read_with(cx, |app, _| {
                    app.symbols.iter().map(|s| s.code.clone()).collect::<Vec<_>>()
                }) {
                    Ok(c) => c,
                    Err(_) => break,
                };
                if codes.is_empty() {
                    continue;
                }
                let result = smol::unblock(move || market::fetch_quotes(&codes)).await;
                let ok = this.update(cx, |app, cx| {
                    match result {
                        Ok(sourced) => {
                            app.quote_fail_streak = 0;
                            for t in sourced.data {
                                if let Some(sym) = app.symbols.iter_mut().find(|s| s.code == t.code)
                                {
                                    if !t.name.is_empty() && t.name != "--" {
                                        sym.name = shared(t.name);
                                    }
                                    if t.last > 0.0 {
                                        sym.last = t.last;
                                        sym.change_pct = t.change_pct;
                                        sym.volume = t.volume;
                                    }
                                }
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
                            QUOTE_INTERVAL_OK
                        }
                        Err(e) => {
                            app.quote_fail_streak = app.quote_fail_streak.saturating_add(1);
                            let backoff_secs = (8u64 * 2u64.pow(app.quote_fail_streak.min(3)))
                                .min(QUOTE_INTERVAL_ERR_MAX.as_secs());
                            app.status = shared(format!(
                                "行情刷新失败: {e} · {}s 后重试",
                                backoff_secs
                            ));
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
    }

    fn refresh_all(&mut self, cx: &mut Context<Self>) {
        let codes: Vec<String> = self.symbols.iter().map(|s| s.code.clone()).collect();
        let selected = self.selected.to_string();
        let bars = self.range.bars();
        self.kline_gen = self.kline_gen.wrapping_add(1);
        let req_gen = self.kline_gen;
        // Clear stale chart immediately so header won't show previous symbol's OHLC
        self.candles.clear();
        self.ma = MaSeries::default();
        self.candles_code = None;
        self.hover_ix = None;
        self.reset_chart_view();
        self.loading = true;
        self.status = shared("加载中…");
        cx.notify();

        cx.spawn(async move |this, cx| {
            let codes2 = codes.clone();
            let req_code = selected.clone();
            let quotes = smol::unblock(move || market::hydrate_symbols(&codes2)).await;
            let kline = smol::unblock(move || market::fetch_klines(&selected, bars)).await;

            this.update(cx, |app, cx| {
                let mut quote_src = None;
                match quotes {
                    Ok(sourced) => {
                        quote_src = Some(sourced.source);
                        for s in &mut app.symbols {
                            if let Some(n) = sourced.data.iter().find(|x| x.code == s.code) {
                                // Keep existing last if hydrate returned zeros
                                let keep_last = s.last;
                                let keep_chg = s.change_pct;
                                let keep_vol = s.volume;
                                *s = n.clone();
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
                if req_gen != app.kline_gen || app.selected.as_ref() != req_code {
                    return;
                }
                match kline {
                    Ok(sourced) => {
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
                    Err(e) => {
                        app.status = shared(format!("K线加载失败: {e}"));
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
            if !name.is_empty() {
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
        let bars = self.range.bars();
        self.kline_gen = self.kline_gen.wrapping_add(1);
        let req_gen = self.kline_gen;
        // Immediately drop previous symbol's series to avoid price/chart mismatch
        self.candles.clear();
        self.ma = MaSeries::default();
        self.candles_code = None;
        self.hover_ix = None;
        self.reset_chart_view();
        self.loading = true;
        self.status = shared(format!("加载 {selected} K线…"));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let req_code = selected.clone();
            let result = smol::unblock(move || market::fetch_klines(&selected, bars)).await;
            this.update(cx, |app, cx| {
                if req_gen != app.kline_gen || app.selected.as_ref() != req_code {
                    // A newer request is in flight / selection changed
                    return;
                }
                match result {
                    Ok(sourced) => {
                        let (_resp_code, name, candles) = sourced.data;
                        app.apply_klines(&req_code, name, candles);
                        app.status = shared(format!(
                            "{} · {} 根K线 · {}",
                            req_code,
                            app.candles.len(),
                            sourced.source
                        ));
                    }
                    Err(e) => {
                        app.status = shared(format!("K线失败: {e}"));
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

    fn persist(&self) {
        let cfg = AppConfig {
            watchlist: self.symbols.iter().map(|s| s.code.clone()).collect(),
            selected: self.selected.to_string(),
            range: self.range.label().into(),
            show_ma5: self.show_ma5,
            show_ma10: self.show_ma10,
            show_ma20: self.show_ma20,
            left_width: self.left_width,
            bottom_height: self.bottom_height,
            color_scheme: self.color_scheme,
        };
        let _ = storage::save_config(&cfg);
    }

    fn set_color_scheme(&mut self, scheme: ColorScheme, cx: &mut Context<Self>) {
        if self.color_scheme == scheme {
            return;
        }
        self.color_scheme = scheme;
        self.persist();
        cx.notify();
    }

    /// Color for a rising (`up`) or falling move under the active convention.
    fn chg_color(&self, up: bool, cx: &App) -> gpui::Hsla {
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
        self.reload_klines(cx);
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

    fn remove_selected_from_watchlist(&mut self, cx: &mut Context<Self>) {
        if self.symbols.len() <= 1 {
            self.status = shared("至少保留一只自选");
            cx.notify();
            return;
        }
        let code = self.selected.to_string();
        self.symbols.retain(|s| s.code != code);
        self.filtered_local = (0..self.symbols.len()).collect();
        if let Some(first) = self.symbols.first() {
            self.selected = shared(first.code.clone());
            self.persist();
            self.reload_klines(cx);
        }
    }

    fn set_range(&mut self, range: ChartRange, cx: &mut Context<Self>) {
        if self.range == range {
            return;
        }
        self.range = range;
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
        let (start, end) = if matched {
            self.chart_visible_range()
        } else {
            (0, 0)
        };
        let candles = if matched && end > start {
            self.candles[start..end].to_vec()
        } else {
            Vec::new()
        };
        let ma = if matched && end > start {
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
        ChartPaintData {
            candles,
            ma,
            show_ma5: self.show_ma5,
            show_ma10: self.show_ma10,
            show_ma20: self.show_ma20,
            hover_ix,
            bullish: self.chg_color(true, cx),
            bearish: self.chg_color(false, cx),
            border: theme.border,
            ma5_color: theme.yellow,
            ma10_color: theme.blue,
            ma20_color: theme.magenta,
            crosshair: theme.muted_foreground.opacity(0.7),
        }
    }

    // ---------- UI pieces ----------

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("Stock"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("A股分析"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .px_2()
                                .py_0p5()
                                .rounded_full()
                                .bg(cx.theme().muted)
                                .text_color(cx.theme().muted_foreground)
                                .child(self.data_source.clone()),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("涨跌色"),
                                )
                                .children(
                                    [ColorScheme::Cn, ColorScheme::Us].map(|scheme| {
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
                                    }),
                                ),
                        )
                        .child(
                            Button::new("refresh")
                                .ghost()
                                .xsmall()
                                .label("刷新")
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.refresh_all(cx);
                                })),
                        )
                        .child(
                            Button::new("cmd-palette-btn")
                                .ghost()
                                .xsmall()
                                .label("⌘K 搜索")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_palette(window, cx);
                                })),
                        ),
                ),
        )
    }

    fn render_watchlist(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h(px(36.))
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
                            .child("自选股"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} 只", self.symbols.len())),
                    ),
            )
            .child(
                h_flex()
                    .h(px(28.))
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("代码 / 名称"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("最新"),
                    ),
            )
            .child(
                v_flex()
                    .id("watchlist-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(self.symbols.iter().enumerate().map(|(ix, sym)| {
                        let is_selected = sym.code == selected.as_ref();
                        let code = shared(sym.code.clone());
                        let name = sym.name.clone();
                        let last = format_price(sym.last);
                        let chg = format_pct(sym.change_pct);
                        let chg_color = self.chg_color(sym.is_up(), cx);
                        let board = sym.board.clone();

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
                                                    .child(sym.code.clone()),
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
                                            .child(name),
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
                            .label("+ 添加")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_palette(window, cx);
                            })),
                    )
                    .child(
                        Button::new("rm-sym")
                            .ghost()
                            .xsmall()
                            .label("移除")
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.remove_selected_from_watchlist(cx);
                            })),
                    ),
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
            QuoteSnapshot::from_candles(&self.candles)
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

        let name = sym
            .map(|s| s.name.clone())
            .unwrap_or_else(|| shared("--"));
        let code = self.selected.clone();
        let board = sym
            .map(|s| s.board.clone())
            .unwrap_or_else(|| shared(""));
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
                                    .child(code),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(name),
                            )
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
                                    .child(format_price(close)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(chg_color)
                                    .child(format_pct(chg)),
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
                            h_flex()
                                .gap_3()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("开 {}", format_price(o)))
                                .child(format!("高 {}", format_price(h)))
                                .child(format!("低 {}", format_price(l)))
                                .child(format!("量 {}", format_volume(v)))
                        } else {
                            h_flex()
                                .gap_3()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if self.loading {
                                    "K线加载中…"
                                } else {
                                    "暂无匹配的 K 线"
                                })
                        }
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(self.ma_toggle("ma5", "MA5", self.show_ma5, cx))
                            .child(self.ma_toggle("ma10", "MA10", self.show_ma10, cx))
                            .child(self.ma_toggle("ma20", "MA20", self.show_ma20, cx))
                            .child(div().w(px(8.)))
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
                            })),
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
                    _ => {}
                }
                this.persist();
                cx.notify();
            }))
    }

    fn render_hover_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        if candles_match {
        if let Some(ix) = self.hover_ix {
            if let Some(c) = self.candles.get(ix) {
                let color = self.chg_color(c.close >= c.open, cx);
                let (m5, m10, m20) = self.ma.value_at(ix);
                let date_label = format_candle_date(c.date.as_ref());
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
                    .into_any_element();
            }
        }
        }
        let (vs, ve) = self.chart_visible_range();
        let zoom_hint = if !self.candles.is_empty() && ve > vs {
            let first = self.candles.get(vs).map(|c| c.date.as_ref()).unwrap_or("?");
            let last = self
                .candles
                .get(ve - 1)
                .map(|c| c.date.as_ref())
                .unwrap_or("?");
            format!(
                "滚轮/双指捏合缩放 · 横向滑动平移 · 可见 {}～{}（{}根）",
                format_candle_date(first),
                format_candle_date(last),
                ve - vs
            )
        } else {
            "滚轮/双指捏合缩放 · 横向滑动平移 · 移动鼠标查看十字线".into()
        };
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(if self.loading {
                "加载中…".to_string()
            } else if !candles_match || self.candles.is_empty() {
                "暂无K线数据".to_string()
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
            QuoteSnapshot::from_candles(&self.candles)
        } else {
            None
        };
        let path = storage::config_path();

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
                            .child("详情 / 状态"),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .p_3()
                    .gap_6()
                    .child(
                        v_flex()
                            .gap_1()
                            .min_w(px(200.))
                            .child(detail_row(
                                "标的",
                                &format!(
                                    "{} {}",
                                    self.selected,
                                    sym.map(|s| s.name.as_ref()).unwrap_or("")
                                ),
                                cx,
                            ))
                            .child(detail_row(
                                "板块",
                                sym.map(|s| s.board.as_ref()).unwrap_or("--"),
                                cx,
                            ))
                            .child(detail_row(
                                "周期",
                                &format!("{} ({} bars)", self.range.label(), self.candles.len()),
                                cx,
                            ))
                            .child(detail_row(
                                "昨收",
                                &format_price(snap.as_ref().map(|s| s.prev_close).unwrap_or(0.0)),
                                cx,
                            )),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .min_w(px(220.))
                            .child(detail_row(
                                "MA5",
                                &if candles_match {
                                    self.ma
                                        .ma5
                                        .last()
                                        .and_then(|x| *x)
                                        .map(format_price)
                                        .unwrap_or_else(|| "--".into())
                                } else {
                                    "--".into()
                                },
                                cx,
                            ))
                            .child(detail_row(
                                "MA10",
                                &if candles_match {
                                    self.ma
                                        .ma10
                                        .last()
                                        .and_then(|x| *x)
                                        .map(format_price)
                                        .unwrap_or_else(|| "--".into())
                                } else {
                                    "--".into()
                                },
                                cx,
                            ))
                            .child(detail_row(
                                "MA20",
                                &if candles_match {
                                    self.ma
                                        .ma20
                                        .last()
                                        .and_then(|x| *x)
                                        .map(format_price)
                                        .unwrap_or_else(|| "--".into())
                                } else {
                                    "--".into()
                                },
                                cx,
                            ))
                            .child(detail_row("配置", &path.display().to_string(), cx))
                            .child(detail_row(
                                "涨跌色",
                                self.color_scheme.label(),
                                cx,
                            )),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("状态"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(self.status.clone()),
                            )
                            .child(
                                div()
                                    .mt_2()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        "数据源说明：东方财富公开 HTTP（无 Key）。\
                                         免费、适合个人研究；无官方 SLA，请控制频率。\
                                         仅供学习，不构成投资建议。",
                                    ),
                            ),
                    ),
            )
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
                                    .child("自选"),
                            );
                            for (i, (_, sym)) in local.into_iter().enumerate() {
                                list = list.child(palette_row(
                                    sym,
                                    true,
                                    i as u64,
                                    self.color_scheme,
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
                                    .child("搜索结果（点击添加）"),
                            );
                            for (i, sym) in remote.into_iter().enumerate() {
                                list = list.child(palette_row(
                                    sym,
                                    false,
                                    10_000 + i as u64,
                                    self.color_scheme,
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
                                    .child("输入代码或名称搜索 A 股"),
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
                                    .child("⌘K 开关 · 点击外部关闭 · 配置自动保存"),
                            ),
                    ),
            )
    }
}

/// Full date for hover strip: `YYYY-MM-DD` when available, else original.
fn format_candle_date(raw: &str) -> String {
    let s = raw.trim();
    if s.len() >= 10 && s.as_bytes().get(4) == Some(&b'-') {
        // 2026-07-29 → keep ISO; also accept already short forms
        s[..10].to_string()
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
    cx: &mut Context<StockApp>,
) -> impl IntoElement {
    let code = sym.code.clone();
    let name = sym.name.to_string();
    let code_show = sym.code.clone();
    let name_show = sym.name.clone();
    let board = sym.board.clone();
    let last = format_price(sym.last);
    let chg = format_pct(sym.change_pct);
    let up = sym.is_up();
    let chg_color = match color_scheme {
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
                .w(px(64.))
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
                    .child("添加"),
            )
        })
}

fn detail_row(label: &str, value: &str, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(px(48.))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().foreground)
                .truncate()
                .child(value.to_string()),
        )
}

impl Render for StockApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let left_w = self.left_width;
        let bottom_h = self.bottom_height;

        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .track_focus(&self.palette_focus)
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, window, cx| {
                this.toggle_palette(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RefreshData, _w, cx| {
                this.refresh_all(cx);
            }))
            .child(self.render_title_bar(cx))
            .child(
                div().flex_1().min_h_0().w_full().child(
                    h_resizable("main-h")
                        .child(
                            resizable_panel()
                                .size(px(left_w))
                                .size_range(px(200.)..px(420.))
                                .child(self.render_watchlist(cx)),
                        )
                        .child(
                            resizable_panel().child(
                                v_resizable("main-v")
                                    .child(resizable_panel().child(self.render_chart_area(cx)))
                                    .child(
                                        resizable_panel()
                                            .size(px(bottom_h))
                                            .size_range(px(100.)..px(320.))
                                            .child(self.render_detail_panel(cx)),
                                    ),
                            ),
                        ),
                ),
            )
            .when(self.palette_open, |this| this.child(self.render_palette(cx)))
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
                window.set_window_title("Stock Analysis · A股");
                Theme::change(ThemeMode::Dark, Some(window), cx);
                let view = cx.new(|cx| StockApp::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("Failed to open window");
        })
        .detach();
    });
}
