//! Watchlist, selection, treasure scan, scout picks, command palette.

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
    pub(crate) fn select_symbol(&mut self, code: SharedString, cx: &mut Context<Self>) {
        if self.selected == code {
            self.palette_open = false;
            cx.notify();
            return;
        }
        self.selected = code;
        self.palette_open = false;
        self.schedule_persist(cx);
        self.reload_chart(cx);
    }

    /// 从寻宝列表点选：必要时临时加入自选，并切到 3Y 以便对照多年高低。
    pub(crate) fn select_treasure_hit(&mut self, hit: &TreasureHit, cx: &mut Context<Self>) {
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
    pub(crate) fn fill_names_for_codes(&mut self, codes: Vec<String>, cx: &mut Context<Self>) {
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
                app.sync_status_bar();
                cx.notify();
            });
        })
        .detach();
    }

    /// 启动时若缓存名称是代码，后台补一次名称。
    pub(crate) fn enrich_treasure_names_if_needed(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn set_left_tab(&mut self, tab: LeftTab, cx: &mut Context<Self>) {
        if self.left_tab == tab {
            return;
        }
        self.left_tab = tab;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn set_treasure_pool(&mut self, pool: TreasurePool, cx: &mut Context<Self>) {
        if self.treasure_pool == pool {
            return;
        }
        self.treasure_pool = pool;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn set_treasure_fin(&mut self, fin: FinFilter, cx: &mut Context<Self>) {
        if self.treasure_fin == fin {
            return;
        }
        self.treasure_fin = fin;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn toggle_treasure_tab(&mut self, cx: &mut Context<Self>) {
        let next = match self.left_tab {
            LeftTab::Treasure => LeftTab::Watchlist,
            _ => LeftTab::Treasure,
        };
        self.set_left_tab(next, cx);
    }

    /// 后台扫描：自选 ∪ 东财扩大池（市值前列）→ 深评 → Top100。
    pub(crate) fn start_treasure_scan(&mut self, cx: &mut Context<Self>) {
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
    pub(crate) fn start_scout_picks(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn cancel_scout_picks(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn select_scout_pick(&mut self, pick: &ScoutPick, cx: &mut Context<Self>) {
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
            // Open ephemeral levels panel from the analysis dock.
            self.detail_tab = DetailTab::Treasure;
            self.schedule_persist(cx);
            self.select_symbol(shared(pick.code.clone()), cx);
        }
    }

    pub(crate) fn set_scout_only_buy_watch(&mut self, only: bool, cx: &mut Context<Self>) {
        if self.scout_only_buy_watch == only {
            return;
        }
        self.scout_only_buy_watch = only;
        cx.notify();
    }

    pub(crate) fn set_treasure_list_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if self.treasure_list_expanded == expanded {
            return;
        }
        self.treasure_list_expanded = expanded;
        cx.notify();
    }

    /// 当前过滤后的可买清单（视图用；不改动 `scout_picks` 源数据）。
    pub(crate) fn visible_scout_picks(&self) -> Vec<&ScoutPick> {
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
    pub(crate) fn finish_scout_ux(&mut self, cx: &mut Context<Self>) {
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
        self.schedule_persist(cx);

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

    pub(crate) fn cancel_treasure_scan(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn add_symbol(&mut self, code: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(crate) fn remove_symbol(&mut self, code: &str, cx: &mut Context<Self>) {
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

    pub(crate) fn remove_selected_from_watchlist(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        self.remove_symbol(&code, cx);
    }

    pub(crate) fn set_range(&mut self, range: ChartRange, cx: &mut Context<Self>) {
        if self.range == range && matches!(self.chart_kind, ChartKind::DayK) {
            return;
        }
        self.range = range;
        self.chart_kind = ChartKind::DayK;
        self.schedule_persist(cx);
        self.reload_klines(cx);
    }

    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = !self.palette_open;
        if self.palette_open {
            self.palette_hits.clear();
            self.filtered_local = (0..self.symbols.len()).collect();
            self.palette_index = 0;
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

    pub(crate) fn on_palette_query_changed(&mut self, q: &str, cx: &mut Context<Self>) {
        let q_l = q.trim().to_lowercase();
        self.palette_index = 0;
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
                    // Keep highlight in range after remote results arrive.
                    let n = app.palette_item_count();
                    if n > 0 {
                        app.palette_index = app.palette_index.min(n - 1);
                    } else {
                        app.palette_index = 0;
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Flattened palette row count: local matches first, then remote hits.
    pub(crate) fn palette_item_count(&self) -> usize {
        self.filtered_local.len() + self.palette_hits.len()
    }

    pub(crate) fn palette_move(&mut self, delta: i32, cx: &mut Context<Self>) {
        let n = self.palette_item_count();
        if n == 0 {
            return;
        }
        let cur = self.palette_index.min(n - 1);
        let next = if delta < 0 {
            if cur == 0 {
                n - 1
            } else {
                cur - 1
            }
        } else {
            (cur + 1) % n
        };
        self.palette_index = next;
        cx.notify();
    }

    /// Activate the highlighted palette row, or try to add the typed code.
    pub(crate) fn palette_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        let n_local = self.filtered_local.len();
        let n_remote = self.palette_hits.len();
        let total = n_local + n_remote;
        if total == 0 {
            let q = self.palette_query.read(cx).value().to_string();
            let q = q.trim();
            if q.is_empty() {
                return;
            }
            if let Some(code) = normalize_code(q) {
                let name = q.to_string();
                self.add_symbol(code, name, window, cx);
            } else {
                // Raw 5-digit HK / free-form: still try as code.
                self.add_symbol(q.to_string(), q.to_string(), window, cx);
            }
            return;
        }
        let ix = self.palette_index.min(total - 1);
        if ix < n_local {
            let code = self.symbols[self.filtered_local[ix]].code.clone();
            self.select_symbol(shared(code), cx);
        } else {
            let hit = self.palette_hits[ix - n_local].clone();
            self.add_symbol(hit.code.clone(), hit.name.to_string(), window, cx);
        }
    }

    pub(crate) fn current_symbol(&self) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.code == self.selected.as_ref())
    }

}
