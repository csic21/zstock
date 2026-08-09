//! Market data loading, quote loops, kline/minute refresh.

use std::time::Duration;

use gpui::{Context, ScrollDelta, ScrollWheelEvent, Timer};
use gpui_component::PixelsExt;

use crate::chart::index_from_x;
use crate::data::market::Sourced;
use crate::data::session::{MarketSet, filter_codes_in_session, idle_delay_secs, open_markets_now};
use crate::data::{
    indicators::{BollSeries, MaSeries, MacdSeries},
    market, session,
};
use crate::domain::market::{Adjustment, CandleRecord, KlineSeries, Market};
use crate::model::{Candle, MinuteSeries, Symbol, shared};
use crate::update::{self, UpdateState};

use super::helpers::*;
use super::series_cache::CachedKlines;
use super::{
    CHART_MIN_VISIBLE, ChartKind, QUOTE_INTERVAL_ERR_MAX, StockApp, TITLE_NORMAL, TITLE_WORK,
};

impl StockApp {
    pub(crate) fn window_title(&self) -> &'static str {
        if self.work_mode {
            TITLE_WORK
        } else {
            TITLE_NORMAL
        }
    }

    /// Restore day/minute/intraday series from the in-memory cache if present.
    /// Returns true when the chart was populated from cache (UI can paint immediately).
    pub(crate) fn try_restore_series_cache(&mut self) -> bool {
        let code = self.selected.to_string();
        match self.chart_kind {
            ChartKind::Intraday => {
                if let Some(series) = self.series_cache.get_minute(&code).cloned() {
                    self.apply_minute_inner(&code, series, /*from_cache*/ true);
                    return true;
                }
                false
            }
            ChartKind::DayK | ChartKind::MinuteK(_) => {
                let bars = self.current_bars();
                if let Some(entry) = self
                    .series_cache
                    .lookup_klines(self.chart_kind, &code, bars)
                {
                    let CachedKlines {
                        name,
                        candles,
                        source,
                    } = entry;
                    self.apply_klines_inner(
                        &code, name, candles, /*from_cache*/ true, &source,
                    );
                    return true;
                }
                false
            }
        }
    }

    fn remember_klines(&mut self, code: &str, name: &str, source: &str) {
        if self.candles.is_empty() {
            return;
        }
        let chart_kind = self.chart_kind;
        let candles = self.candles.clone();
        self.series_cache.put_klines_smart(
            chart_kind,
            code,
            CachedKlines {
                name: name.to_string(),
                candles,
                source: source.to_string(),
            },
        );
    }

    pub(crate) fn bootstrap(&mut self, cx: &mut Context<Self>) {
        // Paint instantly from cache if we have a prior series for the selected symbol.
        let _ = self.try_restore_series_cache();
        // Initial hydrate + klines
        self.refresh_all(cx);
        // 旧缓存可能只有代码没有中文名
        self.enrich_treasure_names_if_needed(cx);
        // 每 3 小时检查是否需要盘后静默预扫（真正开扫条件在 maybe_background_rescan）
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(3 * 60 * 60)).await;
                if this
                    .update(cx, |app, cx| {
                        app.maybe_background_rescan(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
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

        // macOS trackpad pinch (NSEvent magnify) — GPUI does not forward this itself.
        // Poll ~30 Hz (was 8 ms / 125 Hz): still smooth for gestures, far less idle wakeups.
        #[cfg(target_os = "macos")]
        {
            let pinch_rx = crate::mac_gesture::install_pinch_receiver();
            cx.spawn(async move |this, cx| {
                loop {
                    Timer::after(Duration::from_millis(32)).await;
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

        // Quote polling: only during relevant market sessions (A / 港股).
        // Off-hours: no network; startup snapshot is `refresh_all` once.
        cx.spawn(async move |this, cx| {
            let mut delay = Duration::from_secs(1);
            loop {
                Timer::after(delay).await;
                let codes = match this.read_with(cx, |app, _| {
                    let mut codes: Vec<String> =
                        app.symbols.iter().map(|s| s.code.clone()).collect();
                    for c in app.portfolio.open_codes() {
                        if !codes.iter().any(|x| x == &c) {
                            codes.push(c);
                        }
                    }
                    codes
                }) {
                    Ok(c) => c,
                    Err(_) => break,
                };
                if codes.is_empty() {
                    if let Ok(secs) = this.read_with(cx, |app, _| app.quote_interval_secs) {
                        delay = Duration::from_secs(secs);
                    }
                    continue;
                }

                let present = MarketSet::from_codes(&codes);
                let open = open_markets_now(present);
                let active = filter_codes_in_session(&codes, open);
                if active.is_empty() {
                    // Closed for all markets in the list — idle without fetching.
                    delay = Duration::from_secs(idle_delay_secs(present, 60));
                    continue;
                }

                let need_idx =
                    this.read_with(cx, |app, _| app.work_mode).unwrap_or(false) && open.a; // 指数只在 A 股时段刷新
                let quote_ticket =
                    match this.update(cx, |app, _| app.services.market.begin_refresh(&active)) {
                        Ok(ticket) => ticket,
                        Err(_) => break,
                    };
                let result = smol::unblock(move || market::fetch_quotes(&active)).await;
                let idx_result = if need_idx {
                    Some(smol::unblock(market::fetch_major_indices).await)
                } else {
                    None
                };
                let ok = this.update(cx, |app, cx| {
                    match result {
                        Ok(sourced) => {
                            app.quote_fail_streak = 0;
                            let contracts = sourced
                                .data
                                .iter()
                                .filter_map(|tick| {
                                    let market =
                                        crate::domain::market::Market::for_code(&tick.code)?;
                                    Some(crate::domain::market::QuoteRecord {
                                        code: tick.code.clone(),
                                        market,
                                        currency: tick.currency,
                                        name: tick.name.clone(),
                                        price: (tick.last > 0.0).then_some(tick.last),
                                        change_pct: Some(tick.change_pct),
                                        volume: Some(tick.volume),
                                        source: tick.source.clone(),
                                        fetched_at: tick.fetched_at,
                                        market_time: tick.market_time.clone(),
                                        availability: tick.availability,
                                        freshness: tick.freshness,
                                    })
                                })
                                .collect();
                            if app.services.market.apply_refresh(&quote_ticket, contracts) {
                                app.market_state.last_applied_at =
                                    Some(chrono::Utc::now().timestamp_millis());
                            }
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
                            let mut quotes_changed = false;
                            let mut status_bar_dirty = false;
                            // Only codes that need alert evaluation (price moved, or
                            // already-triggered alert that may rearm at a stable price).
                            let mut transitions = Vec::new();
                            let status_bar_codes = app.status_bar_codes.clone();
                            for t in sourced.data {
                                if let Some(&ix) = symbol_ix.get(&t.code) {
                                    let sym = &mut app.symbols[ix];
                                    if is_real_name(&t.name, &t.code)
                                        && sym.name.as_ref() != t.name.as_str()
                                    {
                                        sym.name = shared(t.name.clone());
                                        quotes_changed = true;
                                        if status_bar_codes.iter().any(|c| c == &t.code) {
                                            status_bar_dirty = true;
                                        }
                                    }
                                    if t.last > 0.0 {
                                        let previous = sym.last;
                                        let price_dirty = (sym.last - t.last).abs() > 1e-9
                                            || (sym.change_pct - t.change_pct).abs() > 1e-6
                                            || sym.volume != t.volume;
                                        if price_dirty {
                                            transitions.push((t.code.clone(), previous, t.last));
                                            sym.last = t.last;
                                            sym.change_pct = t.change_pct;
                                            sym.volume = t.volume;
                                            quotes_changed = true;
                                            if app.status_bar_codes.iter().any(|c| c == &t.code) {
                                                status_bar_dirty = true;
                                            }
                                        } else if app
                                            .buy_alerts
                                            .get(&t.code)
                                            .is_some_and(|a| a.triggered)
                                        {
                                            // Price flat but may still rearm above target.
                                            transitions.push((t.code.clone(), previous, t.last));
                                        }
                                    }
                                }
                                // 顺带补寻宝列表中文名
                                if is_real_name(&t.name, &t.code)
                                    && let Some(&ix) = treasure_ix.get(&t.code)
                                {
                                    let hit = &mut app.treasure_hits[ix];
                                    if hit.name != t.name {
                                        hit.name = t.name.clone();
                                        quotes_changed = true;
                                    }
                                }
                            }
                            let alert_hits = app.evaluate_buy_alerts(&transitions, cx);
                            let mut index_changed = false;
                            if let Some(Ok(idx)) = &idx_result {
                                let rows: Vec<_> = idx
                                    .data
                                    .iter()
                                    .map(|t| (t.code.clone(), t.name.clone(), t.last, t.change_pct))
                                    .collect();
                                index_changed = app.apply_index_ticks(&rows);
                            }
                            // Skip full UI rebuild when nothing visible changed.
                            if quotes_changed || index_changed || !alert_hits.is_empty() {
                                // Don't clobber an in-flight kline status unless idle.
                                // Omit wall-clock seconds so identical ticks don't thrash status.
                                if !alert_hits.is_empty() {
                                    app.status = shared(app.format_buy_alert_status(&alert_hits));
                                } else if !app.loading {
                                    let mkt = match (open.a, open.hk) {
                                        (true, true) => "A+港",
                                        (true, false) => "A股",
                                        (false, true) => "港股",
                                        _ => "—",
                                    };
                                    app.status =
                                        shared(format!("行情已更新 · {mkt} · {}", sourced.source));
                                }
                                if app.status_bar_enabled && status_bar_dirty {
                                    app.sync_status_bar();
                                }
                                app.notify_buy_alert_hits(&alert_hits);
                                cx.notify();
                            }
                            Duration::from_secs(app.quote_interval_secs)
                        }
                        Err(e) => {
                            app.services
                                .market
                                .fail_refresh(&quote_ticket, e.to_string());
                            app.quote_fail_streak = app.quote_fail_streak.saturating_add(1);
                            let base = app.quote_interval_secs.max(1);
                            let backoff_secs = (base * 2u64.pow(app.quote_fail_streak.min(5)))
                                .min(QUOTE_INTERVAL_ERR_MAX.as_secs());
                            app.status =
                                shared(format!("行情刷新失败: {e} · {}s 后重试", backoff_secs));
                            if let Some(Ok(idx)) = &idx_result {
                                let rows: Vec<_> = idx
                                    .data
                                    .iter()
                                    .map(|t| (t.code.clone(), t.name.clone(), t.last, t.change_pct))
                                    .collect();
                                let _ = app.apply_index_ticks(&rows);
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

        // 分时自动刷新（仅 Intraday 模式 + 该标的所属市场交易时段）。
        self.spawn_minute_refresh_loop(cx);
    }

    // 分时自动刷新：仅 Intraday 模式生效，约每 5 秒补一根新分钟线。
    // 盘外不拉；所属市场开盘后再刷。
    pub(crate) fn spawn_minute_refresh_loop(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mut delay = Duration::from_secs(5);
            loop {
                Timer::after(delay).await;
                let is_intraday = this
                    .read_with(cx, |app, _| matches!(app.chart_kind, ChartKind::Intraday))
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
                // Gate on the selected symbol's market session.
                let present = MarketSet::from_codes(std::iter::once(selected.as_str()));
                if !session::should_poll_quotes(present) {
                    delay = Duration::from_secs(idle_delay_secs(present, 60));
                    continue;
                }
                let fetch_code = selected.clone();
                let result = smol::unblock(move || market::fetch_minute_series(&fetch_code)).await;
                let ok = this.update(cx, |app, cx| {
                    if !matches!(app.chart_kind, ChartKind::Intraday)
                        || app.selected.as_ref() != selected
                    {
                        return;
                    }
                    if let Ok(sourced) = result {
                        if app.minute_unchanged(&selected, &sourced.data) {
                            return;
                        }
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

    pub(crate) fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.reload_fundamentals(cx);
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
        let from_cache = self.try_restore_series_cache();
        // Keep previous candles painted until the new series arrives (no blank flash).
        self.hover_ix = None;
        self.loading = !from_cache;
        self.refreshing = from_cache;
        self.status = shared(if from_cache {
            if is_intraday {
                format!("缓存 · {selected} 分时 · 刷新中…")
            } else {
                format!("缓存 · {selected} · 刷新中…")
            }
        } else if is_intraday {
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
                Some(
                    smol::unblock(move || {
                        market::fetch_minute_klines(&code, p, bars).map(|s| Sourced {
                            data: (code.clone(), String::new(), s.data),
                            source: s.source,
                        })
                    })
                    .await,
                )
            } else {
                Some(smol::unblock(move || market::fetch_klines(&selected, bars)).await)
            };

            this.update(cx, |app, cx| {
                let mut quote_src = None;
                let mut hydrate_transitions = Vec::new();
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
                                if s.last > 0.0 {
                                    hydrate_transitions.push((s.code.clone(), keep_last, s.last));
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
                            let src = sourced.source;
                            app.apply_minute(&req_code, sourced.data);
                            if let Some(m) = app.minute.clone() {
                                app.series_cache.put_minute(&req_code, m);
                            }
                            app.status = shared(format!(
                                "已加载 {} · 分时 {} · 行情{} · {} · {}",
                                req_code,
                                name,
                                quote_src.unwrap_or("—"),
                                src,
                                chrono::Local::now().format("%H:%M:%S")
                            ));
                        }
                        Some(Err(e)) => {
                            if app.candles_code.as_deref() != Some(req_code.as_str()) {
                                app.status = shared(format!("分时加载失败: {e}"));
                            }
                        }
                        None => {}
                    }
                } else {
                    match kline {
                        Some(Ok(sourced)) => {
                            let (_resp_code, name, candles) = sourced.data;
                            let src = sourced.source;
                            app.apply_klines(&req_code, name.clone(), candles, src);
                            app.remember_klines(&req_code, &name, src);
                            app.status = shared(format!(
                                "已加载 {} · {} 根K线 · 行情{} · K线{} · {}",
                                req_code,
                                app.candles.len(),
                                quote_src.unwrap_or("—"),
                                src,
                                chrono::Local::now().format("%H:%M:%S")
                            ));
                        }
                        Some(Err(e)) => {
                            if app.candles_code.as_deref() != Some(req_code.as_str()) {
                                app.status = shared(format!("K线加载失败: {e}"));
                            }
                        }
                        None => {}
                    }
                }
                let alert_hits = app.evaluate_buy_alerts(&hydrate_transitions, cx);
                if !alert_hits.is_empty() {
                    app.status = shared(app.format_buy_alert_status(&alert_hits));
                    app.notify_buy_alert_hits(&alert_hits);
                }
                app.loading = false;
                app.refreshing = false;
                app.schedule_persist(cx);
                // Hydrate fills last/change before the quote poll may succeed;
                // push to the menu bar immediately so it is not stuck on "…".
                app.sync_status_bar();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn apply_klines(
        &mut self,
        code: &str,
        name: String,
        candles: Vec<Candle>,
        source: &str,
    ) {
        self.apply_klines_inner(code, name, candles, /*from_cache*/ false, source);
    }

    fn apply_klines_inner(
        &mut self,
        code: &str,
        name: String,
        candles: Vec<Candle>,
        _from_cache: bool,
        source: &str,
    ) {
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
        // Same symbol refresh keeps zoom/pan; switching symbols resets the window.
        let same_series = self.candles_code.as_deref() == Some(code) && !self.candles.is_empty();
        self.candles = candles;
        self.candles_code = Some(code.to_string());
        self.data_source = shared(if source.is_empty() {
            market::SRC_LABEL.to_string()
        } else {
            source.to_string()
        });
        if let Some(market) = Market::for_code(code) {
            let series = KlineSeries {
                code: code.to_string(),
                market,
                currency: market.currency(),
                source: self.data_source.to_string(),
                as_of: chrono::Utc::now().timestamp_millis(),
                market_time: self.candles.last().map(|candle| candle.date.to_string()),
                adjustment: if matches!(self.chart_kind, ChartKind::DayK) {
                    Adjustment::Forward
                } else {
                    Adjustment::None
                },
                candles: self
                    .candles
                    .iter()
                    .map(|candle| CandleRecord {
                        time: candle.date.to_string(),
                        open: candle.open,
                        high: candle.high,
                        low: candle.low,
                        close: candle.close,
                        volume: candle.volume,
                    })
                    .collect(),
            };
            let ticket = self.chart_state.controller.select(code);
            if self.chart_state.controller.apply(&ticket, series.clone()) {
                self.chart_state.visible = Some(series);
            }
        } else {
            self.chart_state.visible = None;
        }
        let outcome_candles = self.candles.clone();
        if self
            .journal
            .update_outcomes_for_series(code, &outcome_candles)
            > 0
        {
            self.persist_journal();
        }
        // Day/minute K replaces intraday overlay.
        if !matches!(self.chart_kind, ChartKind::Intraday) {
            self.minute = None;
            self.minute_code = None;
        }
        self.ma = MaSeries::from_candles(&self.candles);
        self.macd = MacdSeries::from_candles(&self.candles);
        self.boll = BollSeries::from_candles(&self.candles);
        self.hover_ix = None;
        self.refresh_analysis_cache();
        if same_series {
            let n = self.candles.len();
            if self.chart_view_count > 0 {
                self.chart_view_count = self.chart_view_count.clamp(CHART_MIN_VISIBLE.min(n), n);
                if self.chart_view_count >= n {
                    self.chart_view_count = 0;
                    self.chart_view_start = 0;
                } else {
                    let count = self.chart_view_count;
                    self.chart_view_start = self.chart_view_start.min(n.saturating_sub(count));
                }
            }
        } else {
            self.reset_chart_view();
        }
    }

    /// True when the fetched minute series matches what we already paint (skip apply/notify).
    pub(crate) fn minute_unchanged(&self, code: &str, series: &MinuteSeries) -> bool {
        if self.minute_code.as_deref() != Some(code) {
            return false;
        }
        let Some(old) = self.minute.as_ref() else {
            return false;
        };
        if old.date != series.date
            || (old.prev_close - series.prev_close).abs() > 1e-9
            || old.points.len() != series.points.len()
        {
            return false;
        }
        match (old.points.last(), series.points.last()) {
            (Some(a), Some(b)) => {
                a.time == b.time && (a.price - b.price).abs() < 1e-9 && a.cum_volume == b.cum_volume
            }
            (None, None) => true,
            _ => false,
        }
    }

    pub(crate) fn apply_minute(&mut self, code: &str, series: MinuteSeries) {
        self.apply_minute_inner(code, series, /*from_cache*/ false);
    }

    fn apply_minute_inner(&mut self, code: &str, series: MinuteSeries, _from_cache: bool) {
        // Periodic refresh of the same code keeps the user's zoom/pan window.
        let same_series = self.minute_code.as_deref() == Some(code) && self.minute.is_some();
        if let Some(sym) = self.symbols.iter_mut().find(|s| s.code == code) {
            if is_real_name(&series.name, code) {
                sym.name = shared(series.name.clone());
            }
            if sym.last <= 0.0
                && let Some(snap) = series.snapshot()
            {
                sym.last = snap.close;
                sym.change_pct = snap.change_pct;
                sym.volume = snap.volume;
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
        self.refresh_analysis_cache();
        if !same_series {
            self.reset_chart_view();
        }
    }

    /// Bars requested for the current chart kind.
    pub(crate) fn current_bars(&self) -> usize {
        match self.chart_kind {
            ChartKind::Intraday => 0,
            ChartKind::DayK => self.range.bars(),
            ChartKind::MinuteK(p) => p.bars(),
        }
    }

    /// Human label for the current chart, e.g. `日K · 3M`, `5分K`, `分时`.
    pub(crate) fn chart_label(&self) -> String {
        match self.chart_kind {
            ChartKind::Intraday => "分时".into(),
            ChartKind::DayK => format!("日K · {}", self.range.label()),
            ChartKind::MinuteK(p) => format!("{}K", p.label()),
        }
    }

    pub(crate) fn reset_chart_view(&mut self) {
        self.chart_view_start = 0;
        self.chart_view_count = 0; // show all
    }

    /// Half-open `[start, end)` index range currently painted.
    pub(crate) fn chart_visible_range(&self) -> (usize, usize) {
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
    pub(crate) fn chart_zoom_factor(&mut self, factor: f32, anchor: Option<usize>) {
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

        let anchor = anchor.filter(|a| *a < n).unwrap_or(start + old_count / 2);
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
    pub(crate) fn chart_zoom(&mut self, zoom_in: bool, anchor: Option<usize>) {
        self.chart_zoom_factor(if zoom_in { 0.82 } else { 1.22 }, anchor);
    }

    pub(crate) fn clamp_hover_to_view(&mut self) {
        if let Some(h) = self.hover_ix {
            let (s, e) = self.chart_visible_range();
            if h < s || h >= e {
                self.hover_ix = None;
            }
        }
    }

    pub(crate) fn chart_pan(&mut self, delta_bars: i32) {
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
    pub(crate) fn on_chart_pinch(&mut self, magnification: f32, cx: &mut Context<Self>) {
        if self.candles.is_empty() || magnification.abs() < 1e-5 {
            return;
        }
        // Magnify > 0 → zoom in (fewer bars): factor < 1
        let factor = (1.0 - magnification * 2.4).clamp(0.65, 1.45);
        self.chart_zoom_factor(factor, self.hover_ix);
        cx.notify();
    }

    pub(crate) fn on_chart_scroll(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
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

    pub(crate) fn reload_klines(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected.to_string();
        let bars = self.current_bars();
        let minute_period = match self.chart_kind {
            ChartKind::MinuteK(p) => Some(p),
            ChartKind::DayK | ChartKind::Intraday => None,
        };
        let req_kind = self.chart_kind;
        self.kline_gen = self.kline_gen.wrapping_add(1);
        let req_gen = self.kline_gen;
        let from_cache = self.try_restore_series_cache();
        // Keep last series visible while loading (header uses live quote + loading flag).
        self.hover_ix = None;
        self.loading = !from_cache;
        self.refreshing = from_cache;
        self.status = shared(if from_cache {
            format!("缓存 · {selected} {} · 刷新中…", self.chart_label())
        } else {
            format!("加载 {selected} {}…", self.chart_label())
        });
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
                        let src = sourced.source;
                        app.apply_klines(&req_code, name.clone(), candles, src);
                        app.remember_klines(&req_code, &name, src);
                        app.status = shared(format!(
                            "{} · {} 根 {} · {}",
                            req_code,
                            app.candles.len(),
                            app.chart_label(),
                            src
                        ));
                    }
                    Err(e) => {
                        if app.candles_code.as_deref() != Some(req_code.as_str()) {
                            app.status = shared(format!("{}加载失败: {e}", app.chart_label()));
                        } else {
                            app.status = shared(format!(
                                "{} · {} 根 {} · 缓存（刷新失败）",
                                req_code,
                                app.candles.len(),
                                app.chart_label()
                            ));
                        }
                    }
                }
                app.loading = false;
                app.refreshing = false;
                app.schedule_persist(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn reload_minute(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected.to_string();
        self.minute_gen = self.minute_gen.wrapping_add(1);
        let req_gen = self.minute_gen;
        let from_cache = self.try_restore_series_cache();
        self.hover_ix = None;
        self.loading = !from_cache;
        self.refreshing = from_cache;
        self.status = shared(if from_cache {
            format!("缓存 · {selected} 分时 · 刷新中…")
        } else {
            format!("加载 {selected} 分时…")
        });
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
                        let src = sourced.source;
                        app.apply_minute(&req_code, sourced.data);
                        if let Some(m) = app.minute.clone() {
                            app.series_cache.put_minute(&req_code, m);
                        }
                        app.status = shared(format!(
                            "{} · 分时 {} 点 · {}",
                            req_code,
                            app.minute.as_ref().map(|m| m.points.len()).unwrap_or(0),
                            src
                        ));
                    }
                    Err(e) => {
                        if app.minute_code.as_deref() != Some(req_code.as_str()) {
                            app.status = shared(format!("分时失败: {e}"));
                        }
                    }
                }
                app.loading = false;
                app.refreshing = false;
                app.schedule_persist(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn reload_chart(&mut self, cx: &mut Context<Self>) {
        match self.chart_kind {
            ChartKind::Intraday => self.reload_minute(cx),
            ChartKind::DayK | ChartKind::MinuteK(_) => self.reload_klines(cx),
        }
    }

    pub(crate) fn reload_fundamentals(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let ticket = self.analysis_state.fundamentals.begin(code.clone());
        self.analysis_state.decision_card = None;
        let provider = std::sync::Arc::clone(&self.services.fundamentals);
        cx.spawn(async move |this, cx| {
            let request_code = code.clone();
            let result = smol::unblock(move || provider.fetch_fundamentals(&code, 8)).await;
            let _ = this.update(cx, |app, cx| {
                let accepted = match result {
                    Ok(snapshot) => app.analysis_state.fundamentals.apply(&ticket, snapshot),
                    Err(error) => app
                        .analysis_state
                        .fundamentals
                        .fail(&ticket, error.to_string()),
                };
                if !accepted || app.selected.as_ref() != request_code {
                    return;
                }
                app.analysis_state.decision_card =
                    (!app.candles.is_empty()).then(|| app.decision_card_view_model());
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn set_chart_kind(&mut self, kind: ChartKind, cx: &mut Context<Self>) {
        if self.chart_kind == kind {
            return;
        }
        self.chart_kind = kind;
        self.schedule_persist(cx);
        self.reload_chart(cx);
    }
}
