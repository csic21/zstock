//! Left panel: watchlist, portfolio, treasure lists.

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
use crate::data::freshness::{self, Freshness};
use crate::data::groups::{FindMode, WatchTag};
use crate::data::radar::{self, RadarHit, RadarStrategy};
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

use super::super::{
    AiCacheEntry, AiPanelState, AiSource, ChartKind, ChartRange, DetailTab, LeftTab, SettingsSection,
    StockApp, CHART_MIN_VISIBLE, QUOTE_INTERVAL_ERR_MAX, QUOTE_INTERVAL_PRESETS, TITLE_NORMAL,
    TITLE_WORK, TREASURE_SCAN_GAP,
};
use super::super::helpers::*;
use super::super::labels::L;



impl StockApp {
    pub(crate) fn render_left_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                                "现在找：长线低位 · 短线雷达 · ⌘T"
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
                                            format!("{}/{}", self.treasure_done, self.treasure_total)
                                        } else if !self.scout_picks.is_empty() {
                                            format!("{} 可买", self.scout_picks.len())
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

    pub(crate) fn render_watchlist_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let work = self.work_mode;
        let sort = self.watchlist_sort;
        let display_order = self.watchlist_display_order();
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                h_flex()
                    .h(px(28.))
                    .px_2()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "ID / Name" } else { "代码 / 名称" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Value" } else { "最新" }),
                    ),
            )
            .child(
                h_flex()
                    .h(px(26.))
                    .px_1()
                    .items_center()
                    .gap_0p5()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.6))
                    .children(WatchlistSort::all().map(|s| {
                        let active = sort == s;
                        Button::new(("wl-sort", s as u32))
                            .ghost()
                            .xsmall()
                            .when(active, |b| b.primary())
                            .label(s.label(work))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.set_watchlist_sort(s, cx);
                            }))
                    })),
            )
            .child(
                h_flex()
                    .h(px(26.))
                    .px_1()
                    .items_center()
                    .gap_0p5()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.45))
                    .children(WatchTag::all_filters().into_iter().enumerate().map(|(ix, tag)| {
                        let active = self.watch_filter == tag;
                        Button::new(("wl-tag-filter", ix as u32))
                            .ghost()
                            .xsmall()
                            .when(active, |b| b.primary())
                            .label(tag.label(work))
                            .tooltip(if work {
                                "Filter by pool tag"
                            } else {
                                "按长线/短线/观察池筛选"
                            })
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.set_watch_filter(tag, cx);
                            }))
                    })),
            )
            .child(
                v_flex()
                    .id("watchlist-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(display_order.is_empty(), |col| {
                        col.child(
                            div()
                                .px_3()
                                .py_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "No symbols in this filter"
                                } else if self.watch_filter == WatchTag::None {
                                    "自选为空 · 点下方添加，或去「找」扫票"
                                } else {
                                    "该分组暂无标的 · 在列表点标签或从「找」一键入池"
                                }),
                        )
                    })
                    .children(display_order.into_iter().map(|ix| {
                        let sym = &self.symbols[ix];
                        let is_selected = sym.code == selected.as_ref();
                        let code = shared(sym.code.clone());
                        let code_show = self.display_code(&sym.code);
                        let name_show = if work {
                            shared("series")
                        } else {
                            sym.name.clone()
                        };
                        let code_for_tag = code.clone();
                        let code_rm = code.clone();
                        let name_rm = name_show.clone();
                        let code_tip = code_show.clone();
                        let last = format_price(sym.last);
                        let chg = self.format_change(sym.change_pct);
                        let chg_color = self.chg_color(sym.is_up(), cx);
                        let board = if work {
                            shared("svc")
                        } else {
                            sym.board.clone()
                        };
                        let tag = self.tag_for(&sym.code);
                        let tag_badge = tag.short_badge();
                        let has_alert = self.buy_alerts.contains_key(&sym.code);

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
                                                    .child(code_show),
                                            )
                                            .when(!tag_badge.is_empty(), |row| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .px_1()
                                                        .rounded(cx.theme().radius)
                                                        .bg(cx.theme().accent.opacity(0.2))
                                                        .text_color(cx.theme().accent)
                                                        .child(tag_badge),
                                                )
                                            })
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
                                            .child(name_show),
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
                            .when(has_alert, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().yellow)
                                        .child(if work { "T" } else { "🔔" }),
                                )
                            })
                            .child(
                                Button::new(("wl-tag", ix))
                                    .ghost()
                                    .xsmall()
                                    .label(if tag_badge.is_empty() {
                                        if work { "+" } else { "标" }
                                    } else {
                                        tag_badge
                                    })
                                    .tooltip(if work {
                                        "Cycle pool tag · Long/Short/Watch"
                                    } else {
                                        "循环标记：长线 → 短线 → 观察 → 清除"
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.select_symbol(code_for_tag.clone(), cx);
                                        this.cycle_selected_watch_tag(cx);
                                    })),
                            )
                            .child(
                                Button::new(("wl-rm", ix))
                                    .icon(IconName::Delete)
                                    .ghost()
                                    .xsmall()
                                    .tooltip(if work {
                                        format!("Remove {code_tip}")
                                    } else {
                                        format!("删除 {name_rm}")
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.remove_symbol(&code_rm, cx);
                                    })),
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
                            .label(if work { "+ Add" } else { "+ 添加" })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_palette(window, cx);
                            })),
                    )
                    .child(
                        Button::new("rm-sym")
                            .ghost()
                            .xsmall()
                            .label(if work { "Remove" } else { "移除" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.remove_selected_from_watchlist(cx);
                            })),
                    )
                    .child(
                        Button::new("wl-find")
                            .ghost()
                            .xsmall()
                            .label(if work { "Find" } else { "去找票" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_left_tab(LeftTab::Treasure, cx);
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.75))
                            .child(if work { "↑↓ · tag" } else { "↑↓ · 标分组" }),
                    ),
            )
    }

    pub(crate) fn render_portfolio_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let selected = self.selected.clone();
        let summary = self.portfolio_summary();
        let form_side = self.trade_form_side;
        let pnl_up = summary.total_unrealized_pnl >= 0.0;
        let pnl_color = self.chg_color(pnl_up, cx);

        let mut root = v_flex().flex_1().min_h_0().w_full();

        // 组合汇总
        root = root.child(
            v_flex()
                .gap_1()
                .px_2()
                .py_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "Book value" } else { "持仓市值" }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(format!("{:.0}", summary.total_market_value)),
                        ),
                )
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "Unrealized" } else { "浮动盈亏" }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(pnl_color)
                                .child(format!(
                                    "{} ({})",
                                    format_money(summary.total_unrealized_pnl),
                                    format_pct(summary.total_unrealized_pnl_pct)
                                )),
                        ),
                )
                .when(summary.track_cash, |this| {
                    this.child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "Cash" } else { "现金" }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(format!("{:.0}", summary.cash)),
                            ),
                    )
                }),
        );

        // 买卖表单
        if let Some(side) = form_side {
            let side_label = if work {
                side.label_work()
            } else {
                side.label()
            };
            root = root.child(
                v_flex()
                    .gap_1()
                    .px_2()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(format!(
                                        "{} · {}",
                                        side_label,
                                        self.selected.as_ref()
                                    )),
                            )
                            .child(
                                Button::new("trade-form-close")
                                    .ghost()
                                    .xsmall()
                                    .label(if work { "Close" } else { "取消" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.trade_form_side = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .w(px(36.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "Qty" } else { "股数" }),
                            )
                            .child(div().flex_1().child(Input::new(&self.trade_shares_input).small())),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .w(px(36.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "Px" } else { "价格" }),
                            )
                            .child(div().flex_1().child(Input::new(&self.trade_price_input).small())),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .w(px(36.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "Fee" } else { "费用" }),
                            )
                            .child(div().flex_1().child(Input::new(&self.trade_fee_input).small())),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .w(px(36.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "Note" } else { "备注" }),
                            )
                            .child(div().flex_1().child(Input::new(&self.trade_note_input).small())),
                    )
                    .child(
                        Button::new("trade-submit")
                            .xsmall()
                            .primary()
                            .label(if work {
                                "Submit"
                            } else {
                                "确认成交"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_trade(window, cx);
                            })),
                    ),
            );
        }

        // 持仓列表头
        root = root.child(
            h_flex()
                .h(px(26.))
                .px_2()
                .items_center()
                .gap_1()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if work { "ID" } else { "代码" }),
                )
                .child(
                    div()
                        .w(px(52.))
                        .text_right()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if work { "Qty" } else { "股数" }),
                )
                .child(
                    div()
                        .w(px(64.))
                        .text_right()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if work { "P&L" } else { "盈亏" }),
                ),
        );

        // 持仓行
        let rows: Vec<_> = summary.positions.iter().cloned().collect();
        root = root.child(
            div()
                .id("portfolio-scroll")
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_y_scroll()
                .children(rows.into_iter().enumerate().map(|(ix, mark)| {
                    let code = mark.position.code.clone();
                    let code_s = shared(code.clone());
                    let is_selected = selected.as_ref() == code.as_str();
                    let code_show = if work {
                        disguise_label(&code, &mark.position.name)
                    } else {
                        code.clone()
                    };
                    let name_show = if work {
                        String::new()
                    } else if is_real_name(&mark.position.name, &code) {
                        mark.position.name.clone()
                    } else {
                        String::new()
                    };
                    let up = mark.unrealized_pnl >= 0.0;
                    let pnl_c = self.chg_color(up, cx);
                    let shares_s = format_shares(mark.position.shares);
                    let pnl_s = format!(
                        "{} {}",
                        format_money(mark.unrealized_pnl),
                        format_pct(mark.unrealized_pnl_pct)
                    );

                    div()
                        .id(("port-row", ix))
                        .px_2()
                        .py_1p5()
                        .flex()
                        .items_center()
                        .gap_1()
                        .cursor_pointer()
                        .border_b_1()
                        .border_color(cx.theme().border.opacity(0.35))
                        .when(is_selected, |this| this.bg(cx.theme().accent.opacity(0.18)))
                        .hover(|this| this.bg(cx.theme().accent.opacity(0.10)))
                        .on_click(cx.listener(move |this, _, _w, cx| {
                            this.select_symbol(code_s.clone(), cx);
                            this.set_detail_tab(DetailTab::Portfolio, cx);
                        }))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child(code_show),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .truncate()
                                        .child(if name_show.is_empty() {
                                            format!(
                                                "成本 {} · 现 {}",
                                                format_price(mark.position.avg_cost),
                                                format_price(mark.last)
                                            )
                                        } else {
                                            format!(
                                                "{name_show} · 成本 {}",
                                                format_price(mark.position.avg_cost)
                                            )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .w(px(52.))
                                .text_right()
                                .text_xs()
                                .text_color(cx.theme().foreground)
                                .child(shares_s),
                        )
                        .child(
                            div()
                                .w(px(72.))
                                .text_right()
                                .text_xs()
                                .font_semibold()
                                .text_color(pnl_c)
                                .child(pnl_s),
                        )
                })),
        );

        if summary.open_count == 0 && form_side.is_none() {
            root = root.child(
                div()
                    .px_3()
                    .py_4()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "No positions. Buy to open."
                    } else {
                        "暂无持仓。选中标的后点「买入」开仓；点「建议」看 AI 与成交明细。"
                    }),
            );
        }

        // 底部操作
        root.child(
            h_flex()
                .h(px(32.))
                .px_1()
                .items_center()
                .gap_0p5()
                .border_t_1()
                .border_color(cx.theme().border)
                .child(
                    Button::new("port-buy")
                        .xsmall()
                        .primary()
                        .label(if work { "Buy" } else { "买入" })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_trade_form(TradeSide::Buy, window, cx);
                        })),
                )
                .child(
                    Button::new("port-sell")
                        .xsmall()
                        .ghost()
                        .label(if work { "Sell" } else { "卖出" })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_trade_form(TradeSide::Sell, window, cx);
                        })),
                )
                .child(
                    Button::new("port-close")
                        .xsmall()
                        .ghost()
                        .label(if work { "Flat" } else { "清仓" })
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.close_selected_position(cx);
                        })),
                )
                .child(div().flex_1())
                .child(
                    Button::new("port-detail")
                        .xsmall()
                        .ghost()
                        .label(if work { "AI" } else { "建议" })
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.set_detail_tab(DetailTab::Portfolio, cx);
                            this.request_portfolio_ai(cx);
                        })),
                ),
        )
    }

    pub(crate) fn render_treasure_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let long_active = self.find_mode == FindMode::Long;
        let short_active = self.find_mode == FindMode::Short;

        // 顶部统一入口：长线 / 短线
        let header = v_flex()
            .px_2()
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(if work { "Find now" } else { "现在找" }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("find-mode-long")
                            .xsmall()
                            .when(long_active, |b| b.primary())
                            .when(!long_active, |b| b.ghost())
                            .label(L::find_long(work))
                            .tooltip(FindMode::Long.headline(work))
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_find_mode(FindMode::Long, cx);
                            })),
                    )
                    .child(
                        Button::new("find-mode-short")
                            .xsmall()
                            .when(short_active, |b| b.primary())
                            .when(!short_active, |b| b.ghost())
                            .label(L::find_short(work))
                            .tooltip(FindMode::Short.headline(work))
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_find_mode(FindMode::Short, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.find_mode.headline(work)),
            )
            .child(self.render_find_freshness_banner(cx));

        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(header)
            .child(if self.find_mode == FindMode::Short {
                self.render_radar_body(cx).into_any_element()
            } else {
                self.render_long_find_body(cx).into_any_element()
            })
    }

    fn render_find_freshness_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let (stamp, scanning) = match self.find_mode {
            FindMode::Long => (
                self.treasure_updated_at.as_str(),
                self.treasure_scanning || self.scout_running,
            ),
            FindMode::Short => (self.radar_updated_at.as_str(), self.radar_scanning),
        };
        let fresh = if scanning {
            Freshness::Fresh
        } else {
            freshness::classify(stamp)
        };
        let color = match fresh {
            Freshness::Fresh => cx.theme().green,
            Freshness::Aging => cx.theme().yellow,
            Freshness::Stale => cx.theme().red,
            Freshness::Unknown => cx.theme().muted_foreground,
        };
        let silent = scanning && self.treasure_scan_silent && self.find_mode == FindMode::Long;
        let text = if scanning {
            if silent {
                if work {
                    "Background refresh…".into()
                } else {
                    "后台静默更新中（可继续看盘）…".into()
                }
            } else if work {
                "Scanning…".into()
            } else {
                "扫描进行中…".into()
            }
        } else {
            freshness::banner_text(stamp, work)
        };
        h_flex()
            .px_1()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(color.opacity(0.10))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(color)
                    .child(if work { "Cache" } else { "时效" }),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(text),
            )
    }

    fn render_radar_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let selected = self.selected.clone();
        let busy = self.radar_scanning;
        let has_hits = !self.radar_hits.is_empty();
        let visible = self.visible_radar_hits();

        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                v_flex()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_wrap()
                            .child(
                                Button::new("radar-scan")
                                    .xsmall()
                                    .when(!busy && !has_hits, |b| b.primary())
                                    .when(busy || has_hits, |b| b.ghost())
                                    .label(if busy {
                                        if work {
                                            "Scanning…"
                                        } else {
                                            "扫描中…"
                                        }
                                    } else if has_hits {
                                        if work {
                                            "Rescan"
                                        } else {
                                            "重新扫描"
                                        }
                                    } else if work {
                                        "Run radar"
                                    } else {
                                        "开始短线扫描"
                                    })
                                    .disabled(busy)
                                    .tooltip(if work {
                                        "Pullback / breakout / oversold on liquid A-shares"
                                    } else {
                                        "扫描强势回踩、放量突破、超跌反弹"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_radar_scan(cx);
                                    })),
                            )
                            .when(busy, |row| {
                                row.child(
                                    Button::new("radar-cancel")
                                        .xsmall()
                                        .ghost()
                                        .label(if work { "Cancel" } else { "取消" })
                                        .on_click(cx.listener(|this, _, _w, cx| {
                                            this.cancel_radar_scan(cx);
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if busy {
                                        format!("{}/{}", self.radar_done, self.radar_total)
                                    } else {
                                        format!("{} 只", visible.len())
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.radar_status.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                Button::new("radar-f-all")
                                    .xsmall()
                                    .when(self.radar_filter.is_none(), |b| b.primary())
                                    .when(self.radar_filter.is_some(), |b| b.ghost())
                                    .label(if work { "All" } else { "全部" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_radar_filter(None, cx);
                                    })),
                            )
                            .children(RadarStrategy::all().into_iter().enumerate().map(
                                |(ix, st)| {
                                    let active = self.radar_filter == Some(st);
                                    Button::new(("radar-f", ix as u32))
                                        .xsmall()
                                        .when(active, |b| b.primary())
                                        .when(!active, |b| b.ghost())
                                        .label(st.label(work))
                                        .tooltip(st.hint())
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.set_radar_filter(Some(st), cx);
                                        }))
                                },
                            )),
                    ),
            )
            .when(
                !self.radar_summary.as_ref().is_empty() && !busy,
                |col| {
                    col.child(
                        div()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.radar_summary.clone()),
                    )
                },
            )
            .child(
                v_flex()
                    .id("radar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(visible.is_empty() && !busy, |col| {
                        col.child(
                            div()
                                .px_3()
                                .py_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "No short-term hits yet · run radar"
                                } else {
                                    "还没有短线命中 · 点上方「开始短线扫描」\n或先去「市场分析」看板块主线"
                                }),
                        )
                    })
                    .children(visible.into_iter().enumerate().map(|(ix, hit)| {
                        let is_sel = hit.code == selected.as_ref();
                        let hit_c = hit.clone();
                        let code_l = hit.code.clone();
                        let name_l = hit.name.clone();
                        let last_l = hit.close;
                        let code_s = hit.code.clone();
                        let name_s = hit.name.clone();
                        let last_s = hit.close;
                        let chg_color = self.chg_color(hit.change_pct >= 0.0, cx);
                        v_flex()
                            .id(("radar-row", ix))
                            .px_3()
                            .py_2()
                            .gap_1()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.35))
                            .cursor_pointer()
                            .when(is_sel, |r| r.bg(cx.theme().accent.opacity(0.16)))
                            .hover(|r| r.bg(cx.theme().accent.opacity(0.08)))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.select_radar_hit(&hit_c, cx);
                            }))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(if work {
                                                hit.code.clone()
                                            } else {
                                                format!("{}  {}", hit.code, hit.name)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .px_1()
                                            .rounded(cx.theme().radius)
                                            .bg(cx.theme().muted)
                                            .text_color(cx.theme().muted_foreground)
                                            .child(hit.strategy.label(work)),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().accent)
                                            .child(format!("{:.0}", hit.score)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(chg_color)
                                            .child(format!("{:+.1}%", hit.change_pct)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work {
                                        hit.headline.clone()
                                    } else {
                                        format!(
                                            "{} · 观察带 {}",
                                            hit.headline,
                                            hit.watch_band_text()
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new(("radar-long", ix as u32))
                                            .xsmall()
                                            .ghost()
                                            .label(if work { "+Long" } else { "+长线池" })
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.add_pick_to_group(
                                                    &code_l, &name_l, last_l, WatchTag::Long, cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new(("radar-short", ix as u32))
                                            .xsmall()
                                            .ghost()
                                            .label(if work { "+Short" } else { "+短线池" })
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.add_pick_to_group(
                                                    &code_s, &name_s, last_s, WatchTag::Short, cx,
                                                );
                                            })),
                                    ),
                            )
                    })),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                    .child(if work {
                        "Local rules · not advice"
                    } else {
                        "本地规则排序 · 仅供学习研究，不构成投资建议"
                    }),
            )
    }

    fn render_long_find_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let work = self.work_mode;
        // 主按钮：空榜 → 搜罗；有榜无清单/重跑 → 筛可买；两边都不抢 primary。
        let has_hits = !self.treasure_hits.is_empty();
        let has_picks = !self.scout_picks.is_empty();
        let busy = self.treasure_scanning || self.scout_running;
        let scan_is_primary = !busy && !has_hits;
        let pick_is_primary = !busy && has_hits && !has_picks;
        let show_full_list = !has_picks || self.treasure_list_expanded;

        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                v_flex()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_wrap()
                            .child(
                                Button::new("treasure-scan")
                                    .xsmall()
                                    .when(scan_is_primary, |b| b.primary())
                                    .when(!scan_is_primary, |b| b.ghost())
                                    .label(if self.treasure_scanning {
                                        if work {
                                            "① Running…"
                                        } else {
                                            "① 扫描中…"
                                        }
                                    } else if work {
                                        "① Scout pool"
                                    } else {
                                        "① 开始搜罗"
                                    })
                                    .disabled(busy)
                                    .tooltip(if work {
                                        "Scan historical lows (then auto AI picks)"
                                    } else {
                                        "扫描历史低位；完成后自动筛可买"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_treasure_scan(cx);
                                    })),
                            )
                            .child(
                                Button::new("treasure-ai-scout")
                                    .xsmall()
                                    .when(pick_is_primary, |b| b.primary())
                                    .when(!pick_is_primary, |b| b.ghost())
                                    .label(if self.scout_running {
                                        if work {
                                            "② Picking…"
                                        } else {
                                            "② 筛可买中…"
                                        }
                                    } else if has_picks {
                                        if work {
                                            "② Re-pick"
                                        } else {
                                            "② 重新筛可买"
                                        }
                                    } else if work {
                                        "② AI picks"
                                    } else {
                                        "② 筛可买"
                                    })
                                    .disabled(busy || !has_hits)
                                    .tooltip(if work {
                                        "Batch-rank buy-watch names from the scan list"
                                    } else {
                                        "从寻宝榜批量给出可关注清单与建仓价（不必一只只点）"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_scout_picks(cx);
                                    })),
                            )
                            .when(busy, |row| {
                                row.child(
                                    Button::new("treasure-cancel")
                                        .xsmall()
                                        .ghost()
                                        .label(if work { "Cancel" } else { "取消" })
                                        .on_click(cx.listener(|this, _, _w, cx| {
                                            if this.scout_running {
                                                this.cancel_scout_picks(cx);
                                            } else {
                                                this.cancel_treasure_scan(cx);
                                            }
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.treasure_status.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.85))
                            .child(if work {
                                format!(
                                    "{} · {} · ≤{TREASURE_SCAN_CAP} · Top{TREASURE_TOP_N}",
                                    self.treasure_pool.label(),
                                    self.treasure_fin.label(),
                                )
                            } else {
                                format!(
                                    "流程：①搜罗低位 → ②筛可买（自动）→ 点可关注看图 · {}池 · {} · Top{TREASURE_TOP_N}",
                                    self.treasure_pool.label(),
                                    self.treasure_fin.label(),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(TreasurePool::all().into_iter().enumerate().map(
                                |(ix, p)| {
                                    let active = self.treasure_pool == p;
                                    Button::new(("tpool", ix as u32))
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(p.label())
                                    .tooltip(if p == TreasurePool::Mcap {
                                        "东财按总市值取沪深 A（默认）"
                                    } else {
                                        "指数成分动态拉取（新浪）"
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_treasure_pool(p, cx);
                                    }))
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(FinFilter::all().into_iter().enumerate().map(
                                |(ix, f)| {
                                    let active = self.treasure_fin == f;
                                    Button::new(("tfin", ix as u32))
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(f.label())
                                    .tooltip(match f {
                                        FinFilter::Off => "不做财务过滤",
                                        FinFilter::Pe => "保留池内 PE 分位 ≤ 50%",
                                        FinFilter::Pb => "保留池内 PB 分位 ≤ 50%",
                                        FinFilter::Value => "PE 与 PB 分位均 ≤ 50%",
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_treasure_fin(f, cx);
                                    }))
                                },
                            )),
                    ),
            )
            .child(
                v_flex()
                    .id("treasure-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    // —— 可买观察清单（批量筛结果，优先展示）——
                    .when(
                        !self.scout_picks.is_empty()
                            || self.scout_running
                            || !self.scout_summary.as_ref().is_empty(),
                        |el| {
                            let visible = self.visible_scout_picks();
                            let buy_n = self
                                .scout_picks
                                .iter()
                                .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
                                .count();
                            let watch_n = self
                                .scout_picks
                                .iter()
                                .filter(|p| p.verdict == ScoutVerdict::Watch)
                                .count();
                            let only_buy = self.scout_only_buy_watch;
                            let count_label = if self.scout_running {
                                format!("{}/{}", self.scout_done, self.scout_total)
                            } else if only_buy {
                                format!("{}/{} 可关注", visible.len(), self.scout_picks.len())
                            } else {
                                format!("{} 只", self.scout_picks.len())
                            };

                            el.child(
                                v_flex()
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .child(
                                        h_flex()
                                            .h(px(26.))
                                            .px_3()
                                            .items_center()
                                            .gap_2()
                                            .bg(cx.theme().accent.opacity(0.08))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .child(if work {
                                                        "Buy watchlist"
                                                    } else {
                                                        "🎯 可买观察"
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(count_label),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .px_3()
                                            .py_1()
                                            .gap_1()
                                            .items_center()
                                            .border_b_1()
                                            .border_color(cx.theme().border.opacity(0.5))
                                            .child(
                                                Button::new("scout-filter-all")
                                                    .xsmall()
                                                    .when(!only_buy, |b| b.primary())
                                                    .when(only_buy, |b| b.ghost())
                                                    .label(if work {
                                                        "All"
                                                    } else {
                                                        "全部"
                                                    })
                                                    .tooltip(if work {
                                                        "Show BuyWatch + Watch"
                                                    } else {
                                                        "显示可关注与观察"
                                                    })
                                                    .on_click(cx.listener(|this, _, _w, cx| {
                                                        this.set_scout_only_buy_watch(false, cx);
                                                    })),
                                            )
                                            .child(
                                                Button::new("scout-filter-buy")
                                                    .xsmall()
                                                    .when(only_buy, |b| b.primary())
                                                    .when(!only_buy, |b| b.ghost())
                                                    .label(if work {
                                                        "Buy only"
                                                    } else {
                                                        "仅可关注"
                                                    })
                                                    .tooltip(if work {
                                                        "Hide Watch rows"
                                                    } else {
                                                        "只显示可关注，隐藏观察"
                                                    })
                                                    .on_click(cx.listener(|this, _, _w, cx| {
                                                        this.set_scout_only_buy_watch(true, cx);
                                                    })),
                                            )
                                            .when(!self.scout_running && !work, |row| {
                                                row.child(
                                                    div()
                                                        .ml_1()
                                                        .text_xs()
                                                        .text_color(
                                                            cx.theme().muted_foreground.opacity(0.85),
                                                        )
                                                        .child(format!(
                                                            "可关注 {buy_n} · 观察 {watch_n}"
                                                        )),
                                                )
                                            }),
                                    )
                                    .when(!self.scout_summary.as_ref().is_empty(), |c| {
                                        c.child(
                                            div()
                                                .px_3()
                                                .py_2()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(self.scout_summary.clone()),
                                        )
                                        .when(!self.scout_source.as_ref().is_empty(), |c2| {
                                            c2.child(
                                                div()
                                                    .px_3()
                                                    .pb_1()
                                                    .text_xs()
                                                    .text_color(
                                                        cx.theme().muted_foreground.opacity(0.8),
                                                    )
                                                    .child(self.scout_source.clone()),
                                            )
                                        })
                                    })
                                    .when(
                                        visible.is_empty()
                                            && !self.scout_running
                                            && !self.scout_picks.is_empty()
                                            && only_buy,
                                        |c| {
                                            c.child(
                                                div()
                                                    .px_3()
                                                    .py_2()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(if work {
                                                        "No BuyWatch rows. Switch to All to see Watch."
                                                    } else {
                                                        "当前没有「可关注」；点「全部」可查看观察名单。"
                                                    }),
                                            )
                                        },
                                    )
                                    .children(visible.into_iter().enumerate().map(
                                        |(ix, pick)| {
                                            let is_selected =
                                                pick.code == selected.as_ref();
                                            let pick_owned = pick.clone();
                                            let code_label = if work {
                                                disguise_label(&pick.code, &pick.name)
                                            } else {
                                                pick.code.clone()
                                            };
                                            let name =
                                                display_name_str(&pick.name, &pick.code);
                                            let show_name =
                                                !work && is_real_name(&name, &pick.code);
                                            let verdict_color = match pick.verdict {
                                                ScoutVerdict::BuyWatch => {
                                                    cx.theme().success
                                                }
                                                ScoutVerdict::Watch => {
                                                    cx.theme().warning
                                                }
                                                ScoutVerdict::Skip => {
                                                    cx.theme().muted_foreground
                                                }
                                            };
                                            let band = format!(
                                                "建仓 {} · 减仓 {}",
                                                pick.buy_band_text(),
                                                pick.sell_band_text()
                                            );
                                            let why = pick
                                                .reasons
                                                .iter()
                                                .take(2)
                                                .cloned()
                                                .collect::<Vec<_>>()
                                                .join(" · ");

                                            div()
                                                .id(("scout-row", ix as u64))
                                                .px_3()
                                                .py_2()
                                                .flex()
                                                .items_start()
                                                .gap_2()
                                                .cursor_pointer()
                                                .border_b_1()
                                                .border_color(
                                                    cx.theme().border.opacity(0.35),
                                                )
                                                .when(is_selected, |this| {
                                                    this.bg(cx.theme().accent.opacity(0.18))
                                                })
                                                .hover(|this| {
                                                    this.bg(cx.theme().accent.opacity(0.10))
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, _w, cx| {
                                                        this.select_scout_pick(
                                                            &pick_owned, cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .gap_1()
                                                        .child(
                                                            h_flex()
                                                                .gap_2()
                                                                .items_center()
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .font_semibold()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .foreground,
                                                                        )
                                                                        .child(code_label),
                                                                )
                                                                .when(show_name, |row| {
                                                                    row.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(name),
                                                                    )
                                                                })
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .font_semibold()
                                                                        .text_color(
                                                                            verdict_color,
                                                                        )
                                                                        .child(
                                                                            pick.verdict
                                                                                .label(),
                                                                        ),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().foreground
                                                                        .opacity(0.9),
                                                                )
                                                                .child(if work {
                                                                    format!(
                                                                        "buy {} / sell {}",
                                                                        pick.buy_band_text(),
                                                                        pick.sell_band_text()
                                                                    )
                                                                } else {
                                                                    band
                                                                }),
                                                        )
                                                        .when(!why.is_empty() && !work, |r| {
                                                            r.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(
                                                                        cx.theme()
                                                                            .muted_foreground,
                                                                    )
                                                                    .child(why),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_semibold()
                                                        .text_color(verdict_color)
                                                        .child(format!(
                                                            "{:.0}",
                                                            pick.buy_score
                                                        )),
                                                )
                                        },
                                    )),
                            )
                        },
                    )
                    // 完整寻宝榜：有可买清单时默认折叠，避免抢注意力
                    .when(has_picks, |el| {
                        el.child(
                            h_flex()
                                .id("treasure-list-fold")
                                .h(px(28.))
                                .px_3()
                                .items_center()
                                .gap_2()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .cursor_pointer()
                                .hover(|this| this.bg(cx.theme().accent.opacity(0.06)))
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    let next = !this.treasure_list_expanded;
                                    this.set_treasure_list_expanded(next, cx);
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work {
                                            format!(
                                                "{} full list ({})",
                                                if self.treasure_list_expanded {
                                                    "▾"
                                                } else {
                                                    "▸"
                                                },
                                                self.treasure_hits.len()
                                            )
                                        } else {
                                            format!(
                                                "{} 完整寻宝榜（{} 只，参考用）",
                                                if self.treasure_list_expanded {
                                                    "▾"
                                                } else {
                                                    "▸"
                                                },
                                                self.treasure_hits.len()
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if self.treasure_list_expanded {
                                            if work {
                                                "Hide"
                                            } else {
                                                "收起"
                                            }
                                        } else if work {
                                            "Show"
                                        } else {
                                            "展开"
                                        }),
                                ),
                        )
                    })
                    .when(!has_picks, |el| {
                        el.child(
                            h_flex()
                                .h(px(26.))
                                .px_3()
                                .items_center()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work {
                                            "Scan list / position"
                                        } else {
                                            "寻宝榜 / 位置（筛可买后会出现上方清单）"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work { "Scr" } else { "分" }),
                                ),
                        )
                    })
                    .when(self.treasure_hits.is_empty() && !self.treasure_scanning, |el| {
                        el.child(
                            div()
                                .p_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "三步：①「开始搜罗」扫历史低位（≤{TREASURE_SCAN_CAP}）→ \
                                     ②自动筛可买清单与建仓价 → ③点「可关注」看图。\
                                     有缓存榜时可直接点「② 筛可买」。"
                                )),
                        )
                    })
                    .when(show_full_list, |el| {
                        el.children(self.treasure_hits.iter().enumerate().map(|(ix, hit)| {
                        let is_selected = hit.code == selected.as_ref();
                        let hit_owned = hit.clone();
                        let code_label = if work {
                            disguise_label(&hit.code, &hit.name)
                        } else {
                            hit.code.clone()
                        };
                        let name = display_name_str(&hit.name, &hit.code);
                        let show_name = !work && is_real_name(&name, &hit.code);
                        let score = format!("{:.0}", hit.score);
                        let pos_line = format!(
                            "1Y {} · 3Y {} · 全 {}",
                            fmt_pos(hit.pos_1y),
                            fmt_pos(hit.pos_3y),
                            fmt_pos(hit.pos_all)
                        );
                        let dd_line = format!("回撤全 {}", fmt_dd(hit.dd_all));
                        let tags: String = hit
                            .tags
                            .iter()
                            .take(3)
                            .map(|t| t.label())
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let is_pullback = hit
                            .tags
                            .iter()
                            .any(|t| matches!(t, treasure::TreasureTag::UptrendPullback));

                        div()
                            .id(("treasure-row", ix as u64))
                            .px_3()
                            .py_2()
                            .flex()
                            .items_start()
                            .gap_2()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.35))
                            .when(is_selected, |this| {
                                this.bg(cx.theme().accent.opacity(0.18))
                            })
                            .hover(|this| this.bg(cx.theme().accent.opacity(0.10)))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.select_treasure_hit(&hit_owned, cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .child(code_label),
                                            )
                                            .when(show_name, |row| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .truncate()
                                                        .child(name),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(pos_line),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(dd_line),
                                            )
                                            .when(!tags.is_empty(), |row| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(if is_pullback {
                                                            cx.theme().yellow
                                                        } else {
                                                            cx.theme().blue
                                                        })
                                                        .truncate()
                                                        .child(tags),
                                                )
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(if is_pullback {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().yellow
                                    })
                                    .child(score),
                            )
                        }))
                    }),
            )
    }

}
