//! State and data loading for the full-page market analysis view.

use std::sync::Arc;

use gpui::Context;

use crate::data::{
    eastmoney, market,
    market_analysis::{self as analysis, MarketAnalysisContext, MarketIndexPoint},
};
use crate::model::shared;

use super::{AiPanelState, AiSource, MarketRegion, StockApp, state::PrimaryTask};

impl StockApp {
    /// Load sector breadth when Today needs a climate reading but the
    /// market page has not been opened yet. Indices arrive from the quote poll.
    pub(crate) fn ensure_market_climate_data(&mut self, cx: &mut Context<Self>) {
        if self.market_analysis_region != MarketRegion::AShare {
            return;
        }
        if !self.market_analysis_sectors.is_empty() || self.market_analysis_loading {
            return;
        }
        self.refresh_market_analysis(cx);
    }

    pub(crate) fn open_market_analysis(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.palette_open = false;
        self.market_analysis_open = true;
        if self.market_analysis_region == MarketRegion::AShare
            && ((self.market_analysis_sectors.is_empty() && !self.market_analysis_loading)
                || (self.market_heatmap_sectors.is_empty() && !self.market_heatmap_loading))
        {
            self.refresh_market_analysis(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn close_market_analysis(&mut self, cx: &mut Context<Self>) {
        if !self.market_analysis_open {
            return;
        }
        self.market_analysis_open = false;
        self.market_heatmap_fullscreen = false;
        cx.notify();
    }

    pub(crate) fn set_market_analysis_region(
        &mut self,
        region: MarketRegion,
        cx: &mut Context<Self>,
    ) {
        if self.market_analysis_region == region {
            return;
        }
        self.market_analysis_region = region;
        self.market_analysis_error = match region {
            MarketRegion::AShare => None,
            MarketRegion::Hk => Some(shared("港股市场分析即将接入")),
            MarketRegion::Us => Some(shared("美股市场分析即将接入")),
        };
        cx.notify();
    }

    pub(crate) fn refresh_market_analysis(&mut self, cx: &mut Context<Self>) {
        if self.market_analysis_region != MarketRegion::AShare {
            return;
        }

        self.market_analysis_gen = self.market_analysis_gen.wrapping_add(1);
        let generation = self.market_analysis_gen;
        self.market_analysis_loading = true;
        self.market_analysis_error = None;
        self.market_heatmap_loading = true;
        self.market_heatmap_error = None;
        cx.notify();

        // The sector list and index quotes are independent requests so the page
        // can paint whichever response arrives first.
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(eastmoney::fetch_a_share_industry_sectors).await;
            let _ = this.update(cx, |app, cx| {
                if app.market_analysis_gen != generation {
                    return;
                }
                app.market_analysis_loading = false;
                match result {
                    Ok(sectors) if !sectors.is_empty() => {
                        app.market_analysis_sectors = sectors;
                        app.market_analysis_source = shared(market::SRC_EASTMONEY);
                        app.market_analysis_updated =
                            Some(shared(chrono::Local::now().format("%H:%M:%S").to_string()));
                    }
                    Ok(_) => {
                        app.market_analysis_error = Some(shared("板块数据为空"));
                    }
                    Err(e) => {
                        app.market_analysis_error = Some(shared(format!("板块数据暂不可用：{e}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(eastmoney::fetch_a_share_industry_heatmap).await;
            let _ = this.update(cx, |app, cx| {
                if app.market_analysis_gen != generation {
                    return;
                }
                app.market_heatmap_loading = false;
                match result {
                    Ok(groups) if !groups.is_empty() => {
                        app.market_heatmap_sectors = Arc::new(groups);
                    }
                    Ok(_) => {
                        app.market_heatmap_error = Some(shared("全景热力图数据为空"));
                    }
                    Err(error) => {
                        app.market_heatmap_error =
                            Some(shared(format!("全景热力图暂不可用：{error}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(market::fetch_major_indices).await;
            let _ = this.update(cx, |app, cx| {
                if let Ok(sourced) = result {
                    let rows: Vec<_> = sourced
                        .data
                        .iter()
                        .map(|t| (t.code.clone(), t.name.clone(), t.last, t.change_pct))
                        .collect();
                    if app.apply_index_ticks(&rows) {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(crate) fn market_index_points(&self) -> Vec<MarketIndexPoint> {
        [
            ("上证综指", self.index_sh),
            ("沪深300", self.index_hs300),
            ("创业板指", self.index_cyb),
        ]
        .into_iter()
        .filter_map(|(name, snap)| {
            snap.map(|snap| MarketIndexPoint {
                name: name.to_string(),
                last: snap.last,
                change_pct: snap.change_pct,
            })
        })
        .collect()
    }

    /// Generate a local-first market brief and optionally upgrade it with the
    /// configured LLM. The candidate list is fetched only after the user
    /// presses the button, so opening the page stays fast and predictable.
    pub(crate) fn request_market_ai_analysis(&mut self, cx: &mut Context<Self>) {
        if matches!(self.market_ai_panel, AiPanelState::Loading { .. }) {
            return;
        }

        self.market_ai_gen = self.market_ai_gen.wrapping_add(1);
        let run_id = self.market_ai_gen;
        let sectors = self.market_analysis_sectors.clone();
        let indices = self.market_index_points();
        let cfg = self.ai_config.clone();
        let cfg_enabled = cfg.enabled;
        self.market_ai_picks.clear();
        self.market_ai_panel = AiPanelState::Loading {
            text: shared("正在读取候选股、计算技术快照…"),
        };
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(|| analysis::fetch_market_picks(6)).await;
            let (picks, fetch_note) = match result {
                Ok(picks) => (picks, None),
                Err(error) => (Vec::new(), Some(format!("候选扫描失败：{error}"))),
            };
            let context: MarketAnalysisContext = analysis::build_context(
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                &sectors,
                indices,
                picks.clone(),
            );
            let local = analysis::local_market_summary(&context);
            let want_llm = fetch_note.is_none() && cfg.is_configured();
            let source_label = cfg.source_label();
            let llm_context = context.clone();
            let local_note = fetch_note.map(shared);

            let _ = this.update(cx, |app, cx| {
                if app.market_ai_gen != run_id {
                    return;
                }
                app.market_ai_picks = picks.clone();
                if want_llm {
                    app.market_ai_panel = AiPanelState::Loading {
                        text: local.clone().into(),
                    };
                } else {
                    app.market_ai_panel = AiPanelState::Ready {
                        text: local.clone().into(),
                        source: AiSource::Local,
                        note: local_note.clone().or_else(|| {
                            Some(shared(if cfg_enabled {
                                "LLM 尚未完整配置，已使用本地规则分析"
                            } else {
                                "未开启 LLM，已使用本地规则分析"
                            }))
                        }),
                    };
                }
                cx.notify();
            });

            if !want_llm {
                return;
            }

            let res = smol::unblock(move || analysis::llm_market_summary(&cfg, &llm_context)).await;
            let _ = this.update(cx, |app, cx| {
                if app.market_ai_gen != run_id {
                    return;
                }
                match res {
                    Ok(text) if !text.trim().is_empty() => {
                        app.market_ai_panel = AiPanelState::Ready {
                            text: text.into(),
                            source: AiSource::Llm {
                                label: source_label.clone(),
                            },
                            note: None,
                        };
                    }
                    Ok(_) => {
                        app.market_ai_panel = AiPanelState::Ready {
                            text: local.clone().into(),
                            source: AiSource::Local,
                            note: Some(shared("LLM 返回空内容，已保留本地规则分析")),
                        };
                    }
                    Err(error) => {
                        app.market_ai_panel = AiPanelState::Ready {
                            text: local.clone().into(),
                            source: AiSource::Local,
                            note: Some(shared(format!("LLM 请求失败，已回退本地规则：{error}"))),
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Expand a level-2 industry using the stocks already loaded into the
    /// panorama heatmap, so the click does not wait on another clist fetch.
    pub(crate) fn open_industry_drill(
        &mut self,
        sector_code: String,
        sector_name: String,
        industry_name: String,
        cx: &mut Context<Self>,
    ) {
        let stocks = self
            .market_heatmap_sectors
            .iter()
            .find(|group| group.sector.code == sector_code)
            .and_then(|group| {
                group
                    .industries
                    .iter()
                    .find(|industry| industry.name == industry_name)
            })
            .map(|industry| industry.stocks.clone())
            .unwrap_or_default();
        if stocks.is_empty() {
            self.open_sector_drill(sector_code, sector_name, cx);
            return;
        }

        self.sector_drill_gen = self.sector_drill_gen.wrapping_add(1);
        self.sector_drill_code = Some(sector_code);
        self.sector_drill_name = Some(shared(format!("{sector_name} / {industry_name}")));
        self.sector_drill_quotes = stocks;
        self.sector_drill_loading = false;
        self.sector_drill_error = None;
        self.status = shared(format!(
            "行业 {industry_name} · {} 只成分",
            self.sector_drill_quotes.len()
        ));
        cx.notify();
    }

    /// 点击行业板块 → 拉取成分股列表（下钻）。
    pub(crate) fn open_sector_drill(&mut self, code: String, name: String, cx: &mut Context<Self>) {
        self.sector_drill_gen = self.sector_drill_gen.wrapping_add(1);
        let drill_id = self.sector_drill_gen;
        self.sector_drill_code = Some(code.clone());
        self.sector_drill_name = Some(shared(name.clone()));
        self.sector_drill_quotes.clear();
        self.sector_drill_loading = true;
        self.sector_drill_error = None;
        self.status = shared(format!("加载板块成分 · {name}"));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let code_fetch = code.clone();
            let result =
                smol::unblock(move || market::fetch_sector_constituents(&code_fetch, 1000)).await;
            let _ = this.update(cx, |app, cx| {
                if app.sector_drill_gen != drill_id {
                    return;
                }
                app.sector_drill_loading = false;
                match result {
                    Ok(sourced) => {
                        app.sector_drill_quotes = sourced.data;
                        app.status = shared(format!(
                            "板块 {name} · {} 只成分",
                            app.sector_drill_quotes.len()
                        ));
                    }
                    Err(e) => {
                        app.sector_drill_error = Some(shared(format!("{e}")));
                        app.status = shared(format!("板块成分加载失败 · {name}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn clear_sector_drill(&mut self, cx: &mut Context<Self>) {
        self.clear_sector_drill_state();
        cx.notify();
    }

    pub(crate) fn set_market_heatmap_list(&mut self, list: bool, cx: &mut Context<Self>) {
        if self.market_heatmap_list == list {
            return;
        }
        self.market_heatmap_list = list;
        cx.notify();
    }

    pub(crate) fn toggle_market_heatmap_fullscreen(&mut self, cx: &mut Context<Self>) {
        self.market_heatmap_fullscreen = !self.market_heatmap_fullscreen;
        cx.notify();
    }

    pub(crate) fn market_heatmap_can_go_back(&self) -> bool {
        self.sector_drill_code.is_some()
    }

    pub(crate) fn back_market_heatmap(&mut self, cx: &mut Context<Self>) {
        if self.sector_drill_code.is_some() {
            self.clear_sector_drill(cx);
        }
    }

    fn clear_sector_drill_state(&mut self) {
        self.sector_drill_gen = self.sector_drill_gen.wrapping_add(1);
        self.sector_drill_code = None;
        self.sector_drill_name = None;
        self.sector_drill_quotes.clear();
        self.sector_drill_loading = false;
        self.sector_drill_error = None;
    }

    pub(crate) fn select_sector_constituent(
        &mut self,
        code: String,
        name: String,
        last: f64,
        cx: &mut Context<Self>,
    ) {
        self.ensure_in_watchlist(&code, &name, last);
        self.set_watch_tag(&code, crate::data::groups::WatchTag::Short, cx);
        self.market_analysis_open = false;
        self.market_heatmap_fullscreen = false;
        // Market analysis can be opened from Today. Selecting a heatmap stock
        // must therefore switch to Research as the overlay closes, otherwise
        // the selected symbol remains hidden behind the Today dashboard.
        self.set_primary_task(PrimaryTask::Research, cx);
        self.select_symbol(shared(code), cx);
    }
}
