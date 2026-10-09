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
    pub(crate) fn quote_for_code(&self, code: &str) -> Option<&crate::domain::market::QuoteRecord> {
        self.services.market.quote_for(code)
    }

    pub(crate) fn quote_status_text(&self) -> String {
        let quote = self
            .quote_for_code(self.selected.as_ref())
            .map(|quote| {
                format!(
                    "{} · {} · {}",
                    quote.source,
                    quote.as_of_label(),
                    quote.freshness_label()
                )
            })
            .unwrap_or_else(|| "行情缺失 · 时间未知".into());
        let errors = &self.services.market.errors;
        format!(
            "{quote}{} · {}",
            if errors.is_empty() {
                String::new()
            } else {
                format!(" · {}", errors.join("；"))
            },
            session::SESSION_CALENDAR_NOTE
        )
    }

    fn quote_request_codes(&self) -> Vec<String> {
        let mut codes: Vec<_> = self
            .symbols
            .iter()
            .map(|symbol| symbol.code.clone())
            .collect();
        codes.extend(self.portfolio.open_codes());
        codes.extend(
            self.buy_alerts
                .iter()
                .filter(|(_, alert)| alert.any_armed())
                .map(|(code, _)| code.clone()),
        );
        codes.push(self.selected.to_string());
        let mut seen = std::collections::HashSet::new();
        codes.retain(|code| seen.insert(code.clone()));
        codes
    }

    fn apply_quote_batch(
        &mut self,
        ticket: &crate::controller::state::RequestTicket,
        batch: crate::infrastructure::market::service::QuoteBatch,
        cx: &mut Context<Self>,
    ) -> bool {
        let errors: Vec<_> = batch.errors.iter().map(ToString::to_string).collect();
        let source = market::quote_source(&batch.records);
        let now = chrono::Utc::now().timestamp_millis();
        let stale_count = batch
            .records
            .iter()
            .filter(|record| {
                !record.usable()
                    || record.effective_freshness(now) == crate::domain::market::Freshness::Stale
            })
            .count();
        let usable_count = batch
            .records
            .iter()
            .filter(|record| record.usable())
            .count();
        let previous: std::collections::HashMap<_, _> = batch
            .records
            .iter()
            .map(|record| {
                (
                    record.code.clone(),
                    self.quote_for_code(&record.code)
                        .and_then(|quote| quote.price)
                        .unwrap_or_default(),
                )
            })
            .collect();
        if !self.services.market.apply_refresh_with_errors(
            ticket,
            batch.records.clone(),
            errors.clone(),
        ) {
            return false;
        }
        self.market_state.last_applied_at = Some(now);
        self.analysis_state.decision_card =
            (!self.current_daily_candles().is_empty()).then(|| self.decision_card_view_model());
        self.quote_fail_streak =
            if usable_count == 0 || (stale_count == batch.records.len() && !errors.is_empty()) {
                self.quote_fail_streak.saturating_add(1)
            } else {
                0
            };
        let mut transitions = Vec::new();
        for record in &batch.records {
            if record.usable()
                && record.effective_freshness(now) == crate::domain::market::Freshness::Live
                && let Some(price) = record.price
            {
                transitions.push((record.code.clone(), previous[&record.code], price));
            }
            if let Some(symbol) = self
                .symbols
                .iter_mut()
                .find(|symbol| symbol.code == record.code)
            {
                hydrate_symbol_from_quote(symbol, record);
            }
            if let Some(preview) = self.preview_symbol.as_mut() {
                hydrate_symbol_from_quote(preview, record);
            }
            if is_real_name(&record.name, &record.code)
                && let Some(hit) = self
                    .treasure_hits
                    .iter_mut()
                    .find(|hit| hit.code == record.code)
            {
                hit.name = record.name.clone();
            }
        }
        let alert_hits = self.evaluate_buy_alerts(&transitions, cx);
        if !alert_hits.is_empty() {
            self.status = shared(self.format_buy_alert_status(&alert_hits));
        } else {
            let unknown = batch
                .records
                .iter()
                .filter(|record| {
                    record.effective_freshness(now) == crate::domain::market::Freshness::Unknown
                })
                .count();
            self.status = shared(format!(
                "行情 {source} · 过期/缺失 {stale_count} · 时效未知 {unknown}{}",
                if errors.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", errors.join("；"))
                }
            ));
        }
        if self.status_bar_enabled {
            self.sync_status_bar();
        }
        self.notify_buy_alert_hits(&alert_hits);
        // Notify even when all prices are flat: metadata and errors are visible data.
        cx.notify();
        true
    }

    pub(crate) fn visible_series_matches_selection(&self) -> bool {
        let expected = self.chart_kind.series_identity(self.selected.as_ref());
        expected.as_ref().is_some_and(|identity| {
            self.chart_state.visible_identity.as_ref() == Some(identity)
                && (matches!(self.chart_kind, ChartKind::Intraday)
                    || self.chart_state.controller.matches_visible(identity))
        }) && self.candles_code.as_deref() == Some(self.selected.as_ref())
    }

    pub(crate) fn current_daily_candles(&self) -> &[Candle] {
        self.chart_state.daily.candles_for(self.selected.as_ref())
    }

    pub(crate) fn daily_analysis_source(&self) -> &str {
        if self.current_daily_candles().is_empty() {
            "日线证据未知"
        } else {
            &self.chart_state.daily.source
        }
    }

    /// Always request the same daily window; chart period/range does not change evidence.
    pub(crate) fn reload_daily_analysis(&mut self, force: bool, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        if !force {
            if self.chart_state.daily.requested_code.as_deref() == Some(code.as_str())
                && matches!(
                    self.chart_state.daily.request.state,
                    crate::controller::state::RequestState::Loading
                )
            {
                // A chart-period change must not restart or cancel daily evidence loading.
                return;
            }
            if self.chart_state.daily.code.as_deref() == Some(code.as_str())
                && !self.chart_state.daily.candles.is_empty()
            {
                if self.chart_state.daily.requested_code.as_deref() != Some(code.as_str()) {
                    self.chart_state.daily.request.cancel();
                    self.chart_state.daily.requested_code = None;
                }
                self.refresh_analysis_cache();
                return;
            }
        }
        let ticket = self.chart_state.daily.request.begin(code.clone());
        self.chart_state.daily.requested_code = Some(code.clone());
        self.signal_cache = None;
        self.levels_cache = None;
        self.backtest_report = None;
        self.analysis_state.decision_card = None;
        cx.spawn(async move |this, cx| {
            let req_code = code.clone();
            let result = smol::unblock(move || market::fetch_klines(&code, 1_000)).await;
            let _ = this.update(cx, |app, cx| {
                if app.selected.as_ref() != req_code
                    || !app.chart_state.daily.request.is_current(&ticket)
                {
                    return;
                }
                match result {
                    Ok(sourced) => {
                        let (_, _, candles) = sourced.data;
                        app.chart_state.daily.code = Some(req_code.clone());
                        app.chart_state.daily.candles = candles;
                        app.chart_state.daily.source = sourced.source.into();
                        app.chart_state.daily.error = None;
                        app.chart_state.daily.request.apply(&ticket, ());
                        let candles = app.current_daily_candles().to_vec();
                        if app.journal.update_outcomes_for_series(&req_code, &candles) > 0 {
                            app.persist_journal();
                        }
                        app.refresh_analysis_cache();
                    }
                    Err(error) => {
                        app.chart_state.daily.error = Some((req_code.clone(), error.to_string()));
                        app.chart_state
                            .daily
                            .request
                            .fail(&ticket, error.to_string());
                        app.status = shared(format!("日线分析加载失败：{error}"));
                        app.refresh_analysis_cache();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

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
        self.start_performance_monitor(cx);
        self.start_a6_validation(cx);
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
                if this
                    .update(cx, |app, cx| {
                        if app
                            .services
                            .market
                            .age_quotes(chrono::Utc::now().timestamp_millis())
                        {
                            app.analysis_state.decision_card =
                                (!app.current_daily_candles().is_empty())
                                    .then(|| app.decision_card_view_model());
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                let codes = match this.read_with(cx, |app, _| app.quote_request_codes()) {
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
                let batch = smol::unblock(move || market::fetch_quote_records(&active)).await;
                let idx_result = if need_idx {
                    Some(smol::unblock(market::fetch_major_indices).await)
                } else {
                    None
                };
                let ok = this.update(cx, |app, cx| {
                    if !app.apply_quote_batch(&quote_ticket, batch, cx) {
                        return Duration::from_secs(app.quote_interval_secs);
                    }
                    if let Some(Ok(idx)) = &idx_result {
                        let rows: Vec<_> = idx
                            .data
                            .iter()
                            .map(|tick| {
                                (
                                    tick.code.clone(),
                                    tick.name.clone(),
                                    tick.last,
                                    tick.change_pct,
                                )
                            })
                            .collect();
                        app.apply_index_ticks(&rows);
                    }
                    // Price-flat updates still change source, provider timestamp and error state.
                    cx.notify();
                    if app.quote_fail_streak == 0 {
                        Duration::from_secs(app.quote_interval_secs)
                    } else {
                        Duration::from_secs(
                            (app.quote_interval_secs.max(1)
                                * 2u64.pow(app.quote_fail_streak.min(5)))
                            .min(QUOTE_INTERVAL_ERR_MAX.as_secs()),
                        )
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
                    .read_with(cx, |app, _| {
                        matches!(app.chart_kind, ChartKind::Intraday)
                            && !app.loading
                            && !app.refreshing
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
                // Gate on the selected symbol's market session.
                let present = MarketSet::from_codes(std::iter::once(selected.as_str()));
                if !session::should_poll_quotes(present) {
                    delay = Duration::from_secs(idle_delay_secs(present, 60));
                    continue;
                }
                let generation = match this.update(cx, |app, _| {
                    app.minute_gen = app.minute_gen.wrapping_add(1);
                    (app.minute_gen, app.kline_gen)
                }) {
                    Ok(generation) => generation,
                    Err(_) => break,
                };
                let fetch_code = selected.clone();
                let result = smol::unblock(move || market::fetch_minute_series(&fetch_code)).await;
                let ok = this.update(cx, |app, cx| {
                    if !matches!(app.chart_kind, ChartKind::Intraday)
                        || app.selected.as_ref() != selected
                        || generation != (app.minute_gen, app.kline_gen)
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
        self.drawing_anchor = None;
        self.draft_line = None;
        self.reload_fundamentals(cx);
        self.reload_daily_analysis(true, cx);
        self.ensure_market_climate_data(cx);
        let codes = self.quote_request_codes();
        let quote_ticket = self.services.market.begin_refresh(&codes);
        let selected = self.selected.to_string();
        let bars = self.current_bars();
        let is_intraday = matches!(self.chart_kind, ChartKind::Intraday);
        let minute_period = match self.chart_kind {
            ChartKind::MinuteK(p) => Some(p),
            ChartKind::DayK | ChartKind::Intraday => None,
        };
        let req_kind = self.chart_kind;
        self.minute_gen = self.minute_gen.wrapping_add(1);
        self.kline_gen = self.kline_gen.wrapping_add(1);
        let req_gen = self.kline_gen;
        let from_cache = self.try_restore_series_cache();
        // Only an exact cached identity is paintable while this request is pending.
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
            let quotes = smol::unblock(move || market::fetch_quote_records(&codes2)).await;
            let quote_src = market::quote_source(&quotes.records);
            if this
                .update(cx, |app, cx| {
                    app.apply_quote_batch(&quote_ticket, quotes, cx);
                })
                .is_err()
            {
                return;
            }
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
                                quote_src,
                                src,
                                chrono::Local::now().format("%H:%M:%S")
                            ));
                        }
                        #[allow(clippy::collapsible_match)]
                        Some(Err(e)) => {
                            if !app.visible_series_matches_selection() {
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
                                quote_src,
                                src,
                                chrono::Local::now().format("%H:%M:%S")
                            ));
                        }
                        #[allow(clippy::collapsible_match)]
                        Some(Err(e)) => {
                            if !app.visible_series_matches_selection() {
                                app.status = shared(format!("K线加载失败: {e}"));
                            }
                        }
                        None => {}
                    }
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
        from_cache: bool,
        source: &str,
    ) {
        let previous_drawing_scope = self.drawing_scope_key();
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
        let identity = self.chart_kind.series_identity(code);
        let same_series = self.chart_state.visible_identity == identity && !self.candles.is_empty();
        self.chart_state.visible_identity = identity;
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
                as_of: if from_cache {
                    0
                } else {
                    chrono::Utc::now().timestamp_millis()
                },
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
            let Some(identity) = self.chart_kind.series_identity(code) else {
                return;
            };
            let ticket = self.chart_state.controller.select(identity);
            if self.chart_state.controller.apply(&ticket, series.clone()) {
                self.chart_state.visible = Some(series);
            }
        } else {
            self.chart_state.visible = None;
        }
        // Day/minute K replaces intraday overlay.
        if !matches!(self.chart_kind, ChartKind::Intraday) {
            self.minute = None;
            self.minute_code = None;
        }
        self.ma = MaSeries::from_candles(&self.candles);
        self.macd = MacdSeries::from_candles(&self.candles);
        self.boll = BollSeries::from_candles(&self.candles);
        if previous_drawing_scope != self.drawing_scope_key() {
            self.drawing_anchor = None;
            self.draft_line = None;
        }
        self.hover_ix = None;
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
        if self.minute_code.as_deref() != Some(code) || !self.visible_series_matches_selection() {
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
        let previous_drawing_scope = self.drawing_scope_key();
        // Periodic refresh of the same code keeps the user's zoom/pan window.
        let identity = ChartKind::Intraday.series_identity(code);
        let same_series = self.chart_state.visible_identity == identity && self.minute.is_some();
        self.chart_state.visible_identity = identity;
        self.chart_state.visible = None;
        self.data_source = shared(market::SRC_TENCENT);
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
        if previous_drawing_scope != self.drawing_scope_key() {
            self.drawing_anchor = None;
            self.draft_line = None;
        }
        self.hover_ix = None;
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
        if n == 0 || !self.visible_series_matches_selection() {
            return (0, 0);
        }
        let count = if self.chart_view_count == 0 {
            n
        } else {
            self.chart_view_count.clamp(CHART_MIN_VISIBLE.min(n), n)
        };
        let start = self.chart_view_start.min(n.saturating_sub(count));
        (start, start + count)
    }

    /// Continuous zoom: `factor < 1` shows fewer bars (zoom in).
    pub(crate) fn chart_zoom_factor(&mut self, factor: f32, anchor: Option<usize>) {
        let n = self.candles.len();
        if n <= CHART_MIN_VISIBLE || !self.visible_series_matches_selection() {
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
        if n == 0 || !self.visible_series_matches_selection() {
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
    #[cfg(target_os = "macos")]
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
        if n == 0 || !self.visible_series_matches_selection() {
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
        self.drawing_anchor = None;
        self.draft_line = None;
        let selected = self.selected.to_string();
        let bars = self.current_bars();
        let minute_period = match self.chart_kind {
            ChartKind::MinuteK(p) => Some(p),
            ChartKind::DayK | ChartKind::Intraday => None,
        };
        let req_kind = self.chart_kind;
        self.minute_gen = self.minute_gen.wrapping_add(1);
        self.kline_gen = self.kline_gen.wrapping_add(1);
        let req_gen = self.kline_gen;
        let from_cache = self.try_restore_series_cache();
        // Incompatible previous data remains cached but cannot be rendered.
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
                        if !app.visible_series_matches_selection() {
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
        self.drawing_anchor = None;
        self.draft_line = None;
        self.kline_gen = self.kline_gen.wrapping_add(1);
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
                        app.status = shared(format!("分时刷新失败: {e}"));
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
        // A gesture belongs to the old instrument and interval, even if a new cache restores instantly.
        self.drawing_anchor = None;
        self.draft_line = None;
        self.hover_ix = None;
        self.reload_daily_analysis(false, cx);
        if self.quote_for_code(self.selected.as_ref()).is_none() {
            let codes = self.quote_request_codes();
            let ticket = self.services.market.begin_refresh(&codes);
            cx.spawn(async move |this, cx| {
                let batch = smol::unblock(move || market::fetch_quote_records(&codes)).await;
                let _ = this.update(cx, |app, cx| {
                    app.apply_quote_batch(&ticket, batch, cx);
                });
            })
            .detach();
        }
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
                app.analysis_state.decision_card = (!app.current_daily_candles().is_empty())
                    .then(|| app.decision_card_view_model());
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

/// Keep temporary research metadata useful without treating it as valuation evidence.
fn hydrate_symbol_from_quote(symbol: &mut Symbol, quote: &crate::domain::market::QuoteRecord) {
    if symbol.code != quote.code {
        return;
    }
    if is_real_name(&quote.name, &quote.code) {
        symbol.name = shared(quote.name.clone());
    }
    if quote.usable()
        && let Some(price) = quote.price
    {
        symbol.last = price;
        symbol.change_pct = quote.change_pct.unwrap_or_default();
        symbol.volume = quote.volume.unwrap_or_default();
    }
}

#[cfg(test)]
mod preview_hydration_tests {
    use super::*;
    use crate::domain::market::{Availability, Freshness, QuoteRecord};
    use crate::domain::money::Currency;

    #[test]
    fn typed_preview_is_hydrated_from_its_canonical_quote_only() {
        let mut preview = Symbol {
            code: "600519".into(),
            name: shared("600519"),
            last: 0.0,
            change_pct: 0.0,
            volume: 0,
            board: shared("fixture"),
        };
        let mut quote = QuoteRecord {
            code: "000001".into(),
            market: Market::AShare,
            currency: Currency::Cny,
            name: "*ST离线样本".into(),
            price: Some(12.0),
            change_pct: Some(-3.0),
            volume: Some(100),
            source: "offline fixture".into(),
            fetched_at: 0,
            market_time: None,
            availability: Availability::Available,
            freshness: Freshness::Unknown,
        };
        hydrate_symbol_from_quote(&mut preview, &quote);
        assert_eq!(preview.name.as_ref(), "600519");
        quote.code = preview.code.clone();
        hydrate_symbol_from_quote(&mut preview, &quote);
        assert_eq!(preview.name.as_ref(), "*ST离线样本");
        assert_eq!(preview.last, 12.0);
        assert_eq!(preview.change_pct, -3.0);
    }
}
