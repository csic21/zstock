//! State and data loading for the full-page market analysis view.

use gpui::Context;

use crate::data::{eastmoney, market};
use crate::model::shared;

use super::{MarketRegion, StockApp};

impl StockApp {
    pub(crate) fn open_market_analysis(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        self.palette_open = false;
        self.market_analysis_open = true;
        if self.market_analysis_region == MarketRegion::AShare
            && self.market_analysis_sectors.is_empty()
            && !self.market_analysis_loading
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
}
