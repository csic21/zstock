//! Watchlist, selection, treasure scan, scout picks, command palette.

use std::collections::HashMap;

use gpui::{Context, SharedString, Timer, Window};

use crate::data::groups::{FindMode, WatchTag};
use crate::data::radar::{
    self, RADAR_KLINE_LIMIT, RADAR_PROBE_N, RADAR_RESULT_N, RadarHit, RadarStrategy,
};
use crate::data::scout::{self, SCOUT_CANDIDATE_N, ScoutPick, ScoutVerdict};
use crate::data::treasure::{self, TREASURE_KLINE_LIMIT, TreasureHit};
use crate::data::universe::{self, FinFilter, TREASURE_TOP_N, TreasurePool};
use crate::data::{eastmoney, market, session};
use crate::model::{Symbol, board_for_code, normalize_code, shared};
use crate::storage::{self};

use super::helpers::*;
use super::{ChartKind, ChartRange, DetailTab, LeftTab, StockApp, TREASURE_SCAN_GAP};

impl StockApp {
    pub(crate) fn select_symbol(&mut self, code: SharedString, cx: &mut Context<Self>) {
        if self.selected == code {
            self.palette_open = false;
            cx.notify();
            return;
        }
        self.portfolio_state.selected_currency =
            crate::domain::money::Currency::for_code(code.as_ref());
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
                        if !is_real_name(sym.name.as_ref(), &t.code) || sym.name.as_ref() != t.name
                        {
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
                    if let Err(error) = storage::save_treasure_cache(&cache) {
                        storage::record_storage_error(format!("保存机会缓存失败：{error:#}"));
                    }
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

    /// 盘后 / 缓存过期：静默预扫长线（不抢焦点）。可反复调用。
    pub(crate) fn maybe_background_rescan(&mut self, cx: &mut Context<Self>) {
        if self.treasure_scanning || self.scout_running || self.radar_scanning {
            return;
        }
        if !session::should_background_long_rescan() {
            return;
        }
        let need = self.treasure_hits.is_empty()
            || matches!(
                crate::data::freshness::classify(&self.treasure_updated_at),
                crate::data::freshness::Freshness::Stale
                    | crate::data::freshness::Freshness::Unknown
            );
        if !need {
            return;
        }
        self.start_treasure_scan_with(true, cx);
    }

    /// 前台扫描：自选 ∪ 扩大池 → 深评 → Top100。
    pub(crate) fn start_treasure_scan(&mut self, cx: &mut Context<Self>) {
        self.start_treasure_scan_with(false, cx);
    }

    /// `silent`：保留旧榜、不切 Tab、状态更轻；完成后仍自动筛可买（静默 UX）。
    pub(crate) fn start_treasure_scan_with(&mut self, silent: bool, cx: &mut Context<Self>) {
        if self.treasure_scanning {
            if !silent {
                self.status = shared("低位策略扫描进行中…");
                cx.notify();
            }
            return;
        }
        let watchlist: Vec<String> = self.symbols.iter().map(|s| s.code.clone()).collect();
        let pool = self.treasure_pool;
        let fin = self.treasure_fin;

        self.treasure_gen = self.treasure_gen.wrapping_add(1);
        let scan_id = self.treasure_gen;
        let discovery_ticket = self.discovery_state.controller.begin("long-term");
        self.treasure_scanning = true;
        self.treasure_scan_silent = silent;
        self.treasure_done = 0;
        self.treasure_total = 0;
        if !silent {
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
            self.left_tab = LeftTab::Treasure;
        }
        self.treasure_status = shared(if silent {
            format!(
                "后台更新 · {}池 · {} · Top {TREASURE_TOP_N}…",
                pool.label(),
                fin.label()
            )
        } else {
            format!(
                "① 拉取 {} 池（{}）· 入榜 Top {TREASURE_TOP_N}…",
                pool.label(),
                fin.label()
            )
        });
        if silent {
            // 不覆盖用户正在看的状态栏主文案（仅轻提示）
            if self.status.as_ref().contains("连接")
                || self.status.as_ref().is_empty()
                || self.status.as_ref().contains("就绪")
            {
                self.status = shared("长线榜后台更新中…");
            }
        } else {
            self.status = shared(format!(
                "机会 · {}池 · {} · 候选{TREASURE_TOP_N}",
                pool.label(),
                fin.label()
            ));
        }
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
                    app.discovery_state
                        .controller
                        .fail(&discovery_ticket, "候选池为空");
                    app.treasure_status = shared(format!("没有可扫描代码 · {filter_note}"));
                    app.status = shared("机会扫描失败：候选池为空");
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
            if let Err(error) = storage::save_treasure_cache(&cache) {
                storage::record_storage_error(format!("保存机会缓存失败：{error:#}"));
            }

            let _ = this.update(cx, |app, cx| {
                if app.treasure_gen != scan_id {
                    return;
                }
                app.treasure_scanning = false;
                app.treasure_hits = hits;
                let result_codes = app
                    .treasure_hits
                    .iter()
                    .map(|hit| hit.code.clone())
                    .collect();
                app.discovery_state
                    .controller
                    .finish(&discovery_ticket, result_codes);
                app.treasure_updated_at = updated_at.clone();
                app.treasure_done = total;
                // 同步名称到已在自选里的同代码
                let treasure_hits = app.treasure_hits.clone();
                for hit in &treasure_hits {
                    if !is_real_name(&hit.name, &hit.code) {
                        continue;
                    }
                    if let Some(sym) = app.symbols.iter_mut().find(|s| s.code == hit.code)
                        && !is_real_name(sym.name.as_ref(), &hit.code) {
                            sym.name = shared(hit.name.clone());
                        }
                }
                let silent = app.treasure_scan_silent;
                app.treasure_scan_silent = false;
                app.treasure_status = shared(format!(
                    "完成 · 深评 {total} · 入榜 {} · {pool_src} · {filter_note} · {updated_at}",
                    app.treasure_hits.len(),
                ));
                if silent {
                    app.status = shared(format!(
                        "低位策略已更新 · Top {} · 正在静默运行规则筛选…",
                        app.treasure_hits.len(),
                    ));
                } else {
                    app.status = shared(format!(
                        "机会扫描完成 · Top {} / 扫描 {total} · 正在运行规则筛选…",
                        app.treasure_hits.len(),
                    ));
                }
                app.persist();
                cx.notify();
                // 扫完自动批量筛「可买观察」，避免用户一只只点
                if !app.treasure_hits.is_empty() {
                    app.start_scout_picks_with(silent, cx);
                }
            });
        })
        .detach();
    }

    /// 从当前寻宝榜批量深评，筛出「可关注 / 观察」清单（本地规则；可选 LLM 整榜摘要）。
    pub(crate) fn start_scout_picks(&mut self, cx: &mut Context<Self>) {
        self.start_scout_picks_with(false, cx);
    }

    pub(crate) fn start_scout_picks_with(&mut self, silent: bool, cx: &mut Context<Self>) {
        if self.scout_running {
            if !silent {
                self.status = shared("规则筛选进行中…");
                cx.notify();
            }
            return;
        }
        if self.treasure_scanning {
            if !silent {
                self.status = shared("请等机会扫描结束后再运行规则筛选");
                cx.notify();
            }
            return;
        }
        if self.treasure_hits.is_empty() {
            if !silent {
                self.scout_summary = shared("请先运行低位策略生成候选，再执行规则筛选。");
                self.status = shared("无可筛标的 · 先搜罗");
                cx.notify();
            }
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
        self.scout_silent = silent;
        self.scout_done = 0;
        self.scout_total = total;
        self.scout_picks.clear();
        self.scout_summary = shared(format!(
            "对机会 Top {total} 做规则深评（位置+雷达+观察区间）…"
        ));
        self.scout_source = shared(if self.work_mode {
            "Scoring…"
        } else {
            "本地规则筛分中"
        });
        if !silent {
            self.left_tab = LeftTab::Treasure;
            self.status = shared(format!("规则筛选 0/{total}"));
        }
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
                        app.status = shared(format!("规则筛选 {done}/{total}"));
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
                        "② 候选观察就绪 · 符合 {buy_n} · 等待 {} · 本地规则",
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
                        app.scout_summary =
                            shared(format!("{local}\n\n（LLM 摘要失败：{e} · 已保留本地清单）"));
                        app.scout_source = shared("本地规则 · LLM 失败回退");
                        app.status = shared(format!(
                            "🎯 可关注 {buy_n} / 共 {} · 本地回退",
                            app.scout_picks.len()
                        ));
                    }
                }
                app.treasure_status = shared(format!(
                    "② 候选观察就绪 · 符合 {buy_n} · 等待 {} · {}",
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
        self.status = shared("已取消规则筛选");
        cx.notify();
    }

    pub(crate) fn select_scout_pick(&mut self, pick: &ScoutPick, cx: &mut Context<Self>) {
        // 长线可买观察默认标入长线池
        self.watch_tags
            .entry(pick.code.clone())
            .or_insert(WatchTag::Long);
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
    /// 静默模式只更新缓存清单，不抢焦点。
    pub(crate) fn finish_scout_ux(&mut self, cx: &mut Context<Self>) {
        let silent = self.scout_silent;
        self.scout_silent = false;
        let buy_n = self
            .scout_picks
            .iter()
            .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
            .count();
        // 没有「可关注」时别留空列表，自动显示观察。
        self.scout_only_buy_watch = buy_n != 0;
        self.treasure_list_expanded = false;
        if silent {
            self.status = shared(format!(
                "长线就绪 · 可关注 {buy_n} / 共 {} · 打开「找」查看",
                self.scout_picks.len()
            ));
            cx.notify();
            return;
        }
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

    // —— 决策日记 ——

    pub(crate) fn persist_journal(&self) {
        if let Err(error) = storage::save_journal(&self.journal) {
            storage::record_storage_error(format!("保存复盘记录失败：{error:#}"));
        }
    }

    pub(crate) fn add_manual_journal_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::data::journal::{self, JournalEntry, JournalKind};
        let note = self.journal_note_input.read(cx).value().to_string();
        let note = note.trim().to_string();
        if note.is_empty() {
            self.status = shared(if self.work_mode {
                "Empty note"
            } else {
                "请先写一句观察或计划"
            });
            cx.notify();
            return;
        }
        let code = self.selected.to_string();
        let name = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.name.to_string())
            .unwrap_or_else(|| code.clone());
        let price = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.last)
            .filter(|p| *p > 0.0)
            .or_else(|| self.candles.last().map(|c| c.close));
        self.journal.push(JournalEntry {
            id: journal::new_id(),
            code,
            name,
            kind: JournalKind::Manual,
            price,
            target: None,
            note,
            created_at: journal::now_stamp(),
            plan: None,
            outcomes: Vec::new(),
        });
        self.persist_journal();
        cx.notify();
        self.journal_note_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.status = shared(if self.work_mode {
            "Journal saved"
        } else {
            "已写入决策日记"
        });
    }

    pub(crate) fn remove_journal_entry(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.journal_delete_confirm_id.as_deref() != Some(id) {
            self.journal_delete_confirm_id = Some(id.to_string());
            self.status = shared("再次点击“确认删除”以删除本地记录");
            cx.notify();
            return;
        }
        self.journal_delete_confirm_id = None;
        if self.journal.remove(id) {
            self.persist_journal();
            cx.notify();
        }
    }

    pub(crate) fn export_journal_local(&mut self, cx: &mut Context<Self>) {
        match storage::export_journal(&self.journal) {
            Ok(path) => self.status = shared(format!("日记已导出：{}", path.display())),
            Err(error) => {
                storage::record_storage_error(format!("导出日记失败：{error:#}"));
                self.status = shared("日记导出失败，请查看数据状态");
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_journal_filter_selected(&mut self, cx: &mut Context<Self>) {
        self.journal_filter_selected = !self.journal_filter_selected;
        cx.notify();
    }

    /// 提醒触发时自动记日记（不打扰，不弹窗）。
    pub(crate) fn record_alert_journal_hits(&mut self, hits: &[super::alerts::BuyAlertHit]) {
        use crate::data::alerts::AlertLeg;
        use crate::data::journal::{self, JournalKind};
        if hits.is_empty() {
            return;
        }
        for hit in hits {
            let kind = match hit.leg {
                AlertLeg::Buy => JournalKind::AlertBuy,
                AlertLeg::Sell => JournalKind::AlertSell,
                AlertLeg::Stop => JournalKind::AlertStop,
            };
            let note = journal::note_for_alert(
                kind,
                &hit.code,
                &hit.name,
                hit.target_price,
                hit.current_price,
            );
            self.journal.push(journal::JournalEntry {
                id: journal::new_id(),
                code: hit.code.clone(),
                name: hit.name.clone(),
                kind,
                price: Some(hit.current_price),
                target: Some(hit.target_price),
                note,
                created_at: journal::now_stamp(),
                plan: None,
                outcomes: Vec::new(),
            });
        }
        self.persist_journal();
    }

    pub(crate) fn set_find_mode(&mut self, mode: FindMode, cx: &mut Context<Self>) {
        if self.find_mode == mode {
            self.left_tab = LeftTab::Treasure;
            cx.notify();
            return;
        }
        self.find_mode = mode;
        self.left_tab = LeftTab::Treasure;
        self.schedule_persist(cx);
        self.status = shared(if self.work_mode {
            format!("Find · {}", mode.label(true))
        } else {
            format!("机会 · {}", mode.label(false))
        });
        cx.notify();
    }

    /// 标题栏 / 命令面板：打开「现在找」并按模式开扫。
    pub(crate) fn open_find_and_scan(&mut self, mode: FindMode, cx: &mut Context<Self>) {
        self.find_mode = mode;
        self.left_tab = LeftTab::Treasure;
        self.schedule_persist(cx);
        match mode {
            FindMode::Long => {
                if self.treasure_hits.is_empty() || self.treasure_scanning {
                    self.start_treasure_scan(cx);
                } else {
                    self.status = shared(if self.work_mode {
                        "Long cache ready · rescan or pick"
                    } else {
                        "长线缓存已就绪 · 可重扫或点清单"
                    });
                    cx.notify();
                }
            }
            FindMode::Short => {
                if self.radar_hits.is_empty() || self.radar_scanning {
                    self.start_radar_scan(cx);
                } else {
                    self.status = shared(if self.work_mode {
                        "Short radar ready · rescan or pick"
                    } else {
                        "短线雷达已就绪 · 可重扫或点清单"
                    });
                    cx.notify();
                }
            }
        }
    }

    pub(crate) fn start_radar_scan(&mut self, cx: &mut Context<Self>) {
        if self.radar_scanning {
            self.status = shared(if self.work_mode {
                "Radar running…"
            } else {
                "短线雷达扫描中…"
            });
            cx.notify();
            return;
        }
        if self.treasure_scanning {
            self.status = shared("请等长线搜罗结束后再扫短线");
            cx.notify();
            return;
        }

        self.radar_gen = self.radar_gen.wrapping_add(1);
        let scan_id = self.radar_gen;
        self.radar_scanning = true;
        self.radar_done = 0;
        self.radar_total = 0;
        self.radar_hits.clear();
        self.radar_summary = shared("");
        self.find_mode = FindMode::Short;
        self.left_tab = LeftTab::Treasure;
        self.radar_status = shared("拉取流动性候选池…");
        self.status = shared(if self.work_mode {
            "Short radar · probing"
        } else {
            "📡 短线雷达 · 拉取候选"
        });
        cx.notify();

        let strategy_filter = self.radar_filter;

        cx.spawn(async move |this, cx| {
            let universe = smol::unblock(|| eastmoney::fetch_liquid_a_shares(220)).await;
            let universe = match universe {
                Ok(u) if !u.is_empty() => u,
                Ok(_) => {
                    let _ = this.update(cx, |app, cx| {
                        if app.radar_gen != scan_id {
                            return;
                        }
                        app.radar_scanning = false;
                        app.radar_status = shared("候选池为空");
                        app.status = shared("短线雷达失败：无候选");
                        cx.notify();
                    });
                    return;
                }
                Err(e) => {
                    let _ = this.update(cx, |app, cx| {
                        if app.radar_gen != scan_id {
                            return;
                        }
                        app.radar_scanning = false;
                        app.radar_status = shared(format!("候选池失败：{e}"));
                        app.status = shared("短线雷达失败");
                        cx.notify();
                    });
                    return;
                }
            };

            let codes: Vec<String> = universe.iter().map(|r| r.code.clone()).collect();
            let names: HashMap<String, String> =
                universe.into_iter().map(|r| (r.code, r.name)).collect();

            let quotes = smol::unblock(move || market::fetch_quotes(&codes)).await;
            let mut ticks = match quotes {
                Ok(s) => s.data,
                Err(e) => {
                    let _ = this.update(cx, |app, cx| {
                        if app.radar_gen != scan_id {
                            return;
                        }
                        app.radar_scanning = false;
                        app.radar_status = shared(format!("行情失败：{e}"));
                        app.status = shared("短线雷达失败");
                        cx.notify();
                    });
                    return;
                }
            };

            // 优先有波动的流动性标的；涨跌都保留以便回踩/超跌策略。
            ticks.retain(|q| {
                q.last > 0.0
                    && q.change_pct.is_finite()
                    && !q.name.to_ascii_uppercase().contains("ST")
            });
            ticks.sort_by(|a, b| {
                b.change_pct
                    .abs()
                    .partial_cmp(&a.change_pct.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if ticks.len() > RADAR_PROBE_N {
                ticks.truncate(RADAR_PROBE_N);
            }
            let total = ticks.len();
            let _ = this.update(cx, |app, cx| {
                if app.radar_gen != scan_id {
                    return;
                }
                app.radar_total = total;
                app.radar_status = shared(format!("深评 0/{total} · 回踩/突破/超跌"));
                app.status = shared(format!("📡 短线深评 0/{total}"));
                cx.notify();
            });

            let mut hits: Vec<RadarHit> = Vec::new();
            for (i, tick) in ticks.into_iter().enumerate() {
                let cancelled = this
                    .read_with(cx, |app, _| app.radar_gen != scan_id)
                    .unwrap_or(true);
                if cancelled {
                    return;
                }

                let code = tick.code.clone();
                let day_chg = tick.change_pct;
                let name_hint = names
                    .get(&code)
                    .cloned()
                    .unwrap_or_else(|| tick.name.clone());
                let code_fetch = code.clone();
                let result = smol::unblock(move || {
                    market::fetch_klines_adjusted(&code_fetch, RADAR_KLINE_LIMIT)
                })
                .await;

                if let Ok(sourced) = result {
                    let (_c, returned_name, candles) = sourced.data;
                    let name = if is_real_name(&returned_name, &code) {
                        returned_name
                    } else {
                        name_hint
                    };
                    let hit = match strategy_filter {
                        Some(st) => radar::evaluate_strategy(&code, &name, &candles, day_chg, st),
                        None => radar::evaluate(&code, &name, &candles, day_chg),
                    };
                    if let Some(h) = hit {
                        hits.push(h);
                    }
                }

                let done = i + 1;
                let _ = this.update(cx, |app, cx| {
                    if app.radar_gen != scan_id {
                        return;
                    }
                    app.radar_done = done;
                    let mut partial = hits.clone();
                    radar::sort_hits(&mut partial);
                    if partial.len() > RADAR_RESULT_N {
                        partial.truncate(RADAR_RESULT_N);
                    }
                    app.radar_hits = partial;
                    app.radar_status = shared(format!(
                        "深评 {done}/{total} · 命中 {}",
                        app.radar_hits.len()
                    ));
                    if done == total || done % 5 == 0 {
                        app.status = shared(format!("📡 短线深评 {done}/{total}"));
                    }
                    cx.notify();
                });

                if done < total {
                    Timer::after(TREASURE_SCAN_GAP).await;
                }
            }

            radar::sort_hits(&mut hits);
            if hits.len() > RADAR_RESULT_N {
                hits.truncate(RADAR_RESULT_N);
            }
            let summary = radar::local_summary(&hits);
            let updated_at = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
            let cache = radar::RadarCache {
                updated_at: updated_at.clone(),
                universe: format!("liquid/probe{total}/top{RADAR_RESULT_N}"),
                hits: hits.clone(),
            };
            if let Err(error) = storage::save_radar_cache(&cache) {
                storage::record_storage_error(format!("保存扫描缓存失败：{error:#}"));
            }

            let _ = this.update(cx, |app, cx| {
                if app.radar_gen != scan_id {
                    return;
                }
                app.radar_scanning = false;
                app.radar_hits = hits;
                app.radar_updated_at = updated_at.clone();
                app.radar_done = total;
                app.radar_summary = shared(summary);
                app.radar_status =
                    shared(format!("完成 · {} 只 · {updated_at}", app.radar_hits.len()));
                app.status = shared(format!("📡 短线雷达完成 · {} 只", app.radar_hits.len()));
                if let Some(first) = app.radar_hits.first().cloned() {
                    app.select_radar_hit(&first, cx);
                } else {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn cancel_radar_scan(&mut self, cx: &mut Context<Self>) {
        if !self.radar_scanning {
            return;
        }
        self.radar_gen = self.radar_gen.wrapping_add(1);
        self.radar_scanning = false;
        self.radar_status = shared(format!("已取消 · 保留 {} 条", self.radar_hits.len()));
        self.status = shared("已取消短线雷达");
        cx.notify();
    }

    pub(crate) fn set_radar_filter(
        &mut self,
        filter: Option<RadarStrategy>,
        cx: &mut Context<Self>,
    ) {
        self.radar_filter = filter;
        cx.notify();
    }

    pub(crate) fn select_radar_hit(&mut self, hit: &RadarHit, cx: &mut Context<Self>) {
        let code = hit.code.clone();
        let display = display_name_str(&hit.name, &code);
        if !self.symbols.iter().any(|s| s.code == code) {
            self.symbols.push(Symbol {
                code: code.clone(),
                name: shared(display),
                last: hit.close,
                change_pct: hit.change_pct,
                volume: 0,
                board: board_for_code(&code),
            });
            self.filtered_local = (0..self.symbols.len()).collect();
        }
        // 默认标为短线池，方便后续盯盘
        self.watch_tags
            .entry(code.clone())
            .or_insert(WatchTag::Short);
        self.left_tab = LeftTab::Treasure;
        self.find_mode = FindMode::Short;
        self.detail_tab = DetailTab::Strategy;
        self.schedule_persist(cx);
        if !matches!(self.range, ChartRange::M1 | ChartRange::M3) {
            self.range = ChartRange::M3;
        }
        self.select_symbol(shared(code), cx);
    }

    pub(crate) fn visible_radar_hits(&self) -> Vec<&RadarHit> {
        self.radar_hits
            .iter()
            .filter(|h| match self.radar_filter {
                None => true,
                Some(st) => h.strategy == st,
            })
            .collect()
    }

    pub(crate) fn tag_for(&self, code: &str) -> WatchTag {
        self.watch_tags.get(code).copied().unwrap_or(WatchTag::None)
    }

    pub(crate) fn set_watch_tag(&mut self, code: &str, tag: WatchTag, cx: &mut Context<Self>) {
        if tag == WatchTag::None {
            self.watch_tags.remove(code);
        } else {
            self.watch_tags.insert(code.to_string(), tag);
        }
        self.schedule_persist(cx);
        self.status = shared(if self.work_mode {
            format!("Tag · {} · {}", code, tag.label(true))
        } else {
            format!("已标记 {} → {}", code, tag.label(false))
        });
        cx.notify();
    }

    pub(crate) fn cycle_selected_watch_tag(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let cur = self.tag_for(&code);
        let next = match cur {
            WatchTag::None => WatchTag::Long,
            WatchTag::Long => WatchTag::Short,
            WatchTag::Short => WatchTag::Watch,
            WatchTag::Watch => WatchTag::None,
        };
        self.set_watch_tag(&code, next, cx);
    }

    pub(crate) fn set_watch_filter(&mut self, filter: WatchTag, cx: &mut Context<Self>) {
        if self.watch_filter == filter {
            return;
        }
        self.watch_filter = filter;
        self.schedule_persist(cx);
        cx.notify();
    }

    pub(crate) fn add_pick_to_group(
        &mut self,
        code: &str,
        name: &str,
        last: f64,
        tag: WatchTag,
        cx: &mut Context<Self>,
    ) {
        self.ensure_in_watchlist(code, name, last);
        self.set_watch_tag(code, tag, cx);
    }

    pub(crate) fn run_selected_backtest(
        &mut self,
        rule: crate::data::backtest::BacktestRule,
        cx: &mut Context<Self>,
    ) {
        use crate::data::backtest;
        if self.candles.len() < 60 {
            self.backtest_report = None;
            self.status = shared(if self.work_mode {
                "Need more daily bars"
            } else {
                "日 K 不足 60 根，无法回测"
            });
            cx.notify();
            return;
        }
        let currency = crate::domain::money::Currency::for_code(self.selected.as_ref())
            .unwrap_or(crate::domain::money::Currency::Cny);
        let report = backtest::run(&self.candles, rule, 10, currency);
        self.backtest_report = report;
        if let Some(ref r) = self.backtest_report {
            self.status = shared(r.summary_line(self.work_mode));
        }
        cx.notify();
    }

    pub(crate) fn cancel_treasure_scan(&mut self, cx: &mut Context<Self>) {
        if !self.treasure_scanning {
            return;
        }
        self.treasure_gen = self.treasure_gen.wrapping_add(1);
        self.discovery_state.controller.cancel();
        self.treasure_scanning = false;
        self.treasure_status = shared(format!(
            "已取消 · 保留 {} 条中间结果",
            self.treasure_hits.len()
        ));
        self.status = shared("机会扫描已取消");
        cx.notify();
    }

    pub(crate) fn add_symbol(
        &mut self,
        code: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        self.buy_alerts.remove(code);
        self.work_aliases.remove(code);
        self.chart_lines.remove(code);
        self.watch_tags.remove(code);
        self.filtered_local = (0..self.symbols.len()).collect();
        if was_selected {
            self.selected = shared(
                self.symbols
                    .get(pos)
                    .or_else(|| self.symbols.last())
                    .map(|s| s.code.clone())
                    .unwrap_or_default(),
            );
            if self.work_alias_editing {
                self.work_alias_editing = false;
            }
        }
        // Drop from status-bar pins if present.
        if let Some(ix) = self.status_bar_codes.iter().position(|c| c == code) {
            self.status_bar_codes.remove(ix);
            if self.status_bar_active == code {
                self.status_bar_active = self.status_bar_codes.first().cloned().unwrap_or_default();
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
            if cur == 0 { n - 1 } else { cur - 1 }
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
        // 意图快捷：即使有本地匹配也优先识别纯意图词
        let q_raw = self.palette_query.read(cx).value().to_string();
        let q_intent = q_raw.trim().to_lowercase();
        if matches!(
            q_intent.as_str(),
            "长线"
                | "找长线"
                | "long"
                | "寻宝"
                | "低位"
                | "短线"
                | "找短线"
                | "short"
                | "雷达"
                | "市场"
                | "板块"
                | "情绪"
                | "market"
        ) {
            self.palette_open = false;
            match q_intent.as_str() {
                "短线" | "找短线" | "short" | "雷达" => {
                    self.open_find_and_scan(FindMode::Short, cx);
                }
                "市场" | "板块" | "情绪" | "market" => {
                    self.open_market_analysis(cx);
                }
                _ => {
                    self.open_find_and_scan(FindMode::Long, cx);
                }
            }
            return;
        }

        let n_local = self.filtered_local.len();
        let n_remote = self.palette_hits.len();
        let total = n_local + n_remote;
        if total == 0 {
            let q = q_raw.trim();
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
        self.symbols
            .iter()
            .find(|s| s.code == self.selected.as_ref())
    }
}
