//! Left panel: watchlist, portfolio, treasure lists.

mod discovery;
mod portfolio;
mod watchlist;

use gpui::{Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div, px};
use gpui_component::{ActiveTheme, StyledExt, TITLE_BAR_HEIGHT, h_flex, v_flex};

use crate::data::groups::{FindMode, WatchTag};

use super::super::{LeftTab, StockApp};

impl StockApp {
    pub(crate) fn render_left_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let work = self.work_mode;
        let avail_h = (window.bounds().size.height - TITLE_BAR_HEIGHT).max(px(0.));
        let (title, subtitle, badge) = match self.left_tab {
            LeftTab::Watchlist => (
                if work { "Services" } else { "自选研究" },
                if work {
                    "Tracked list"
                } else {
                    "选择标的，查看决策与图表"
                },
                if work { "R" } else { "研" },
            ),
            LeftTab::Portfolio => (
                if work { "Book" } else { "组合管理" },
                if work {
                    "Positions"
                } else {
                    "持仓、风险与交易记录"
                },
                if work { "B" } else { "组" },
            ),
            LeftTab::Treasure => (
                if work { "Discover" } else { "机会发现" },
                if work {
                    "Candidate scan"
                } else {
                    "筛选候选，再回到决策依据"
                },
                if work { "F" } else { "机" },
            ),
        };
        let count = match self.left_tab {
            LeftTab::Watchlist => {
                let n = self.watchlist_display_order().len();
                if self.watch_filter == WatchTag::None {
                    format!("{}", self.symbols.len())
                } else {
                    format!("{n}/{}", self.symbols.len())
                }
            }
            LeftTab::Portfolio => format!("{}", self.portfolio_summary().open_count),
            LeftTab::Treasure => match self.find_mode {
                FindMode::Long => {
                    if self.treasure_scanning {
                        format!("{}/{}", self.treasure_done, self.treasure_total)
                    } else if !self.scout_picks.is_empty() {
                        format!("{}", self.scout_picks.len())
                    } else {
                        format!("{}", self.treasure_hits.len())
                    }
                }
                FindMode::Short => {
                    if self.radar_scanning {
                        format!("{}/{}", self.radar_done, self.radar_total)
                    } else {
                        format!("{}", self.radar_hits.len())
                    }
                }
            },
        };
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
                    .h(px(50.))
                    .px_3()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .size(px(34.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().accent.opacity(0.13))
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().accent)
                            .child(badge),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        crate::app::helpers::status_pill_sized(
                            count,
                            cx.theme().muted_foreground,
                            cx.theme().muted,
                            Some(28.0),
                        ),
                    ),
            )
            .child(match self.left_tab {
                LeftTab::Watchlist => self.render_watchlist_body(cx).into_any_element(),
                LeftTab::Portfolio => self.render_portfolio_body(cx).into_any_element(),
                LeftTab::Treasure => self.render_treasure_body(cx).into_any_element(),
            })
    }
}
