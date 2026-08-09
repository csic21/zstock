//! Left panel: watchlist, portfolio, treasure lists.

mod discovery;
mod portfolio;
mod watchlist;

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Sizable, TITLE_BAR_HEIGHT,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::data::groups::{FindMode, WatchTag};

use super::super::labels::L;
use super::super::{LeftTab, StockApp};

impl StockApp {
    pub(crate) fn render_left_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                            .label(L::left_watchlist(work))
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_left_tab(LeftTab::Watchlist, cx);
                            })),
                    )
                    .child(
                        Button::new("tab-portfolio")
                            .xsmall()
                            .when(self.left_tab == LeftTab::Portfolio, |b| b.primary())
                            .when(self.left_tab != LeftTab::Portfolio, |b| b.ghost())
                            .label(L::left_portfolio(work))
                            .tooltip(if work {
                                "Positions · buy/sell"
                            } else {
                                "持仓 · 买入/卖出 · AI 建议"
                            })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_left_tab(LeftTab::Portfolio, cx);
                            })),
                    )
                    .child(
                        Button::new("tab-treasure")
                            .xsmall()
                            .when(self.left_tab == LeftTab::Treasure, |b| b.primary())
                            .when(self.left_tab != LeftTab::Treasure, |b| b.ghost())
                            .label(L::left_treasure(work))
                            .tooltip(if work {
                                "Find longs / shorts · ⌘T"
                            } else {
                                "机会：低位策略 · 短线信号 · ⌘T"
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
                                LeftTab::Watchlist => {
                                    let n = self.watchlist_display_order().len();
                                    if self.watch_filter == WatchTag::None {
                                        format!("{} 只", self.symbols.len())
                                    } else {
                                        format!("{n}/{}", self.symbols.len())
                                    }
                                }
                                LeftTab::Portfolio => {
                                    format!("{} 只", self.portfolio_summary().open_count)
                                }
                                LeftTab::Treasure => match self.find_mode {
                                    FindMode::Long => {
                                        if self.treasure_scanning {
                                            format!(
                                                "{}/{}",
                                                self.treasure_done, self.treasure_total
                                            )
                                        } else if !self.scout_picks.is_empty() {
                                            format!("{} 候选", self.scout_picks.len())
                                        } else {
                                            format!("{} 只", self.treasure_hits.len())
                                        }
                                    }
                                    FindMode::Short => {
                                        if self.radar_scanning {
                                            format!("{}/{}", self.radar_done, self.radar_total)
                                        } else {
                                            format!("{} 只", self.radar_hits.len())
                                        }
                                    }
                                },
                            }),
                    ),
            )
            .child(match self.left_tab {
                LeftTab::Watchlist => self.render_watchlist_body(cx).into_any_element(),
                LeftTab::Portfolio => self.render_portfolio_body(cx).into_any_element(),
                LeftTab::Treasure => self.render_treasure_body(cx).into_any_element(),
            })
    }
}
