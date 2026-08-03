//! Preferences, layout, work mode, status bar, AI settings, updates.

#![allow(unused_imports)]

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    canvas, div, point, px, size, App, AppContext, Bounds, Context, Entity, FocusHandle,
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
use crate::data::portfolio::{
    self, format_money, format_shares, Portfolio, PortfolioSummary, TradeSide,
};
use crate::data::scout::{self, ScoutPick, ScoutVerdict, SCOUT_CANDIDATE_N};
use crate::data::treasure::{self, fmt_dd, fmt_pos, TreasureHit, TREASURE_KLINE_LIMIT};
use crate::data::universe::{self, FinFilter, TreasurePool, TREASURE_SCAN_CAP, TREASURE_TOP_N};
use crate::data::{
    indicators::{BollSeries, MaSeries, MacdSeries},
    market, session, signals,
};
use crate::data::market::Sourced;
use crate::data::session::{filter_codes_in_session, idle_delay_secs, open_markets_now, MarketSet};
use crate::model::{
    board_for_code, disguise_index, disguise_label, format_index, format_pct, format_price,
    format_volume, normalize_code, shared, Candle, IndexSnap, MinutePeriod, MinuteSeries,
    QuoteSnapshot, Symbol, TrendLine,
};
use crate::storage::{
    self, clamp_quote_interval_secs, normalize_status_bar, AppConfig, ColorScheme, DockLayout,
    WatchlistSort, STATUS_BAR_MAX_CODES,
};
use crate::update::{self, UpdateState};

use super::{
    AiCacheEntry, AiPanelState, AiSource, ChartKind, ChartRange, DetailTab, LeftTab, SettingsSection,
    StockApp, CHART_MIN_VISIBLE, QUOTE_INTERVAL_ERR_MAX, QUOTE_INTERVAL_PRESETS, TITLE_NORMAL,
    TITLE_WORK, TREASURE_SCAN_GAP,
};
use super::helpers::*;



impl StockApp {
    pub(crate) fn select_adjacent_symbol(&mut self, delta: i32, cx: &mut Context<Self>) {
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
    pub(crate) fn watchlist_display_order(&self) -> Vec<usize> {
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

    pub(crate) fn set_watchlist_sort(&mut self, sort: WatchlistSort, cx: &mut Context<Self>) {
        if self.watchlist_sort == sort {
            return;
        }
        self.watchlist_sort = sort;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn on_main_h_resize(
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
            self.schedule_persist(cx);
        }
    }

    pub(crate) fn on_main_v_resize(
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
            self.schedule_persist(cx);
        }
    }

    pub(crate) fn set_color_scheme(&mut self, scheme: ColorScheme, cx: &mut Context<Self>) {
        if self.color_scheme == scheme {
            return;
        }
        self.color_scheme = scheme;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn set_work_mode(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        self.schedule_persist(cx);
        self.sync_status_bar();
        cx.notify();
    }

    pub(crate) fn toggle_work_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_work_mode(!self.work_mode, window, cx);
    }

    pub(crate) fn toggle_work_identity(&mut self, cx: &mut Context<Self>) {
        if !self.work_mode {
            return;
        }
        self.work_identity_reveal = !self.work_identity_reveal;
        cx.notify();
    }

    pub(crate) fn set_quote_interval_secs(&mut self, secs: u64, cx: &mut Context<Self>) {
        let secs = clamp_quote_interval_secs(secs);
        if self.quote_interval_secs == secs {
            return;
        }
        self.quote_interval_secs = secs;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn set_status_bar_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
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
        self.schedule_persist(cx);
        self.sync_status_bar();
        cx.notify();
    }

    /// Install the native status item once and start polling menu actions.
    #[cfg(target_os = "macos")]
    pub(crate) fn ensure_status_bar_installed(&mut self, cx: &mut Context<Self>) {
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
    pub(crate) fn ensure_status_bar_installed(&mut self, _cx: &mut Context<Self>) {}

    pub(crate) fn toggle_status_bar_code(&mut self, code: &str, cx: &mut Context<Self>) {
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
        self.schedule_persist(cx);
        self.sync_status_bar();
        cx.notify();
    }

    pub(crate) fn set_status_bar_active(&mut self, code: &str, cx: &mut Context<Self>) {
        if !self.status_bar_codes.iter().any(|c| c == code) {
            return;
        }
        if self.status_bar_active == code {
            return;
        }
        self.status_bar_active = code.to_string();
        self.schedule_persist(cx);
        self.sync_status_bar();
        cx.notify();
    }

    pub(crate) fn normalize_status_bar_state(&mut self) {
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
    pub(crate) fn sync_status_bar(&self) {
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
    pub(crate) fn status_bar_multi_title_for(&self, syms: &[&Symbol]) -> String {
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

    pub(crate) fn status_bar_title_for(&self, sym: &Symbol) -> String {
        // Always include last price when available. Work-mode / multi compact used
        // to show only ±% (2dp); the main window re-renders every poll with live
        // prices, so the menu bar looked "stuck" whenever % didn't tick.
        if self.work_mode {
            let alias = disguise_label(&sym.code, sym.name.as_ref());
            if sym.last > 0.0 {
                format!(
                    "{alias} {} {:+.2}%",
                    format_price(sym.last),
                    sym.change_pct
                )
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

    /// Compact segment for multi-symbol titles: `名 价±%` (price keeps it live).
    pub(crate) fn status_bar_compact_for(&self, sym: &Symbol) -> String {
        if self.work_mode {
            let alias = disguise_label(&sym.code, sym.name.as_ref());
            if sym.last > 0.0 {
                format!(
                    "{alias} {}{:+.2}%",
                    format_price(sym.last),
                    sym.change_pct
                )
            } else {
                alias
            }
        } else {
            let name = short_status_name(sym.name.as_ref(), &sym.code);
            if sym.last > 0.0 {
                format!(
                    "{name} {}{}",
                    format_price(sym.last),
                    format_pct(sym.change_pct)
                )
            } else {
                format!("{name}…")
            }
        }
    }

    pub(crate) fn status_bar_menu_label_for(&self, sym: &Symbol) -> String {
        if self.work_mode {
            let alias = disguise_label(&sym.code, sym.name.as_ref());
            if sym.last > 0.0 {
                format!(
                    "{alias}  {}  {:+.2}%",
                    format_price(sym.last),
                    sym.change_pct
                )
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
    pub(crate) fn handle_status_bar_action(
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
                self.schedule_persist(cx);
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
    pub(crate) fn activate_main_window(&self, cx: &mut Context<Self>) {
        cx.activate(true);
        for handle in cx.windows() {
            let _ = handle.update(cx, |_root, window, _cx| {
                window.activate_window();
            });
        }
    }

    /// 当前展示的 AI 点评对应的缓存键（`code@最后一根 K 日期`）。
    /// 与 `ai_key` 不一致时，详情栏按「未生成」展示，避免串股。
    pub(crate) fn ai_current_key(&self) -> Option<String> {
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

    pub(crate) fn request_ai_commentary(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn set_ai_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.ai_config.enabled == enabled {
            return;
        }
        self.ai_config.enabled = enabled;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn set_ai_kind(&mut self, kind: AiKind, cx: &mut Context<Self>) {
        if self.ai_config.kind == kind {
            return;
        }
        self.ai_config.kind = kind;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn set_ai_transport(&mut self, transport: AiTransport, cx: &mut Context<Self>) {
        if self.ai_config.transport == transport {
            return;
        }
        self.ai_config.transport = transport;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn set_ai_cli_provider(&mut self, provider: AiCliProvider, cx: &mut Context<Self>) {
        if self.ai_config.cli_provider == provider {
            return;
        }
        self.ai_config.cli_provider = provider;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn toggle_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.palette_open = false;
            // Re-enter on General so the page feels fresh each open.
            self.settings_section = SettingsSection::General;
        }
        cx.notify();
    }

    pub(crate) fn set_settings_section(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        if self.settings_section == section {
            return;
        }
        self.settings_section = section;
        cx.notify();
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        if !self.settings_open {
            return;
        }
        self.settings_open = false;
        cx.notify();
    }

    pub(crate) fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
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

    pub(crate) fn start_update(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn render_update_button(&self, cx: &mut Context<Self>) -> Option<Button> {
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

    pub(crate) fn update_status_line(&self, work: bool) -> String {
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
    pub(crate) fn chg_color(&self, up: bool, cx: &App) -> gpui::Hsla {
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

}
