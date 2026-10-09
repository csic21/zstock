use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IconName, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    tooltip::Tooltip,
    v_flex,
};

use crate::app::{LeftTab, StockApp};
use crate::data::groups::WatchTag;
use crate::domain::market::Freshness;
use crate::model::{format_price, shared};
use crate::storage::WatchlistSort;

impl StockApp {
    pub(crate) fn render_watchlist_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let work = self.work_mode;
        let sort = self.watchlist_sort;
        let display_order = self.watchlist_display_order();
        let now_millis = chrono::Utc::now().timestamp_millis();
        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                h_flex()
                    .h(px(30.))
                    .px_3()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.65))
                    .bg(cx.theme().background.opacity(0.36))
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
                    .h(px(28.))
                    .px_2()
                    .items_center()
                    .gap_0p5()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .bg(cx.theme().background.opacity(0.24))
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
                    .h(px(28.))
                    .px_2()
                    .items_center()
                    .gap_0p5()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.5))
                    .bg(cx.theme().background.opacity(0.24))
                    .children(
                        WatchTag::all_filters()
                            .into_iter()
                            .enumerate()
                            .map(|(ix, tag)| {
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
                            }),
                    ),
            )
            .child(
                v_flex()
                    .id("watchlist-scroll")
                    .track_scroll(&self.watchlist_scroll)
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
                        let quote = self.quote_for_code(&sym.code);
                        let usable_quote = quote.filter(|quote| quote.usable());
                        let freshness = quote.map(|quote| quote.effective_freshness(now_millis));
                        let name_show = if work {
                            shared("series")
                        } else {
                            quote
                                .map(|quote| quote.name.as_str())
                                .filter(|name| !name.is_empty())
                                .map(|name| shared(name.to_string()))
                                .unwrap_or_else(|| sym.name.clone())
                        };
                        let code_for_tag = code.clone();
                        let code_rm = code.clone();
                        let name_rm = name_show.clone();
                        let code_tip = code_show.clone();
                        let last = usable_quote
                            .and_then(|quote| quote.price)
                            .map(format_price)
                            .unwrap_or_else(|| "—".into());
                        let change = usable_quote
                            .and_then(|quote| quote.change_pct)
                            .filter(|change| change.is_finite());
                        let chg = change
                            .map(|change| self.format_change(change))
                            .unwrap_or_else(|| "—".into());
                        let chg_color = if freshness == Some(Freshness::Live) {
                            change
                                .map(|change| self.chg_color(change >= 0.0, cx))
                                .unwrap_or(cx.theme().muted_foreground)
                        } else {
                            cx.theme().muted_foreground
                        };
                        let freshness_label = match (usable_quote.is_some(), freshness, work) {
                            (false, _, false) => "缺失",
                            (false, _, true) => "N/A",
                            (_, Some(Freshness::Live), false) => "实时",
                            (_, Some(Freshness::Live), true) => "Live",
                            (_, Some(Freshness::Delayed), false) => "延迟",
                            (_, Some(Freshness::Delayed), true) => "Late",
                            (_, Some(Freshness::Stale), false) => "过期",
                            (_, Some(Freshness::Stale), true) => "Stale",
                            (_, _, false) => "时效未知",
                            (_, _, true) => "Unknown",
                        };
                        let quote_tooltip = quote
                            .map(|quote| {
                                format!(
                                    "{} · {} · {}",
                                    if work { "Feed" } else { quote.source.as_str() },
                                    quote.as_of_label(),
                                    freshness_label
                                )
                            })
                            .unwrap_or_else(|| {
                                if work {
                                    "No quote received".into()
                                } else {
                                    "尚未取得有效行情 · 价格与涨跌幅未知".into()
                                }
                            });
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
                            .h(px(52.))
                            .flex_shrink_0()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .tooltip(move |window, cx| {
                                Tooltip::new(quote_tooltip.clone()).build(window, cx)
                            })
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.30))
                            .when(is_selected, |this| {
                                this.border_l_2()
                                    .border_color(cx.theme().accent.opacity(0.82))
                                    .bg(cx.theme().accent.opacity(0.13))
                            })
                            .hover(|this| this.bg(cx.theme().accent.opacity(0.08)))
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
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .truncate()
                                                    .child(name_show),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .flex_shrink_0()
                                                    .text_color(
                                                        if freshness == Some(Freshness::Live)
                                                            && usable_quote.is_some()
                                                        {
                                                            cx.theme().muted_foreground
                                                        } else {
                                                            cx.theme().warning
                                                        },
                                                    )
                                                    .child(freshness_label),
                                            ),
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
                                    .child(div().text_xs().text_color(chg_color).child(chg)),
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
                                        cx.stop_propagation();
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
                                        format!("移出自选 {name_rm} · 保留研究数据")
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        cx.stop_propagation();
                                        this.remove_symbol(&code_rm, cx);
                                    })),
                            )
                    })),
            )
            .when_some(self.watchlist_undo.as_ref(), |column, removed| {
                column.child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .gap_1()
                        .items_center()
                        .flex_shrink_0()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .truncate()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "Removed · data kept".to_string()
                                } else {
                                    format!("已移出 {} · 数据保留", removed.symbol.code)
                                }),
                        )
                        .child(
                            Button::new("watchlist-undo")
                                .ghost()
                                .xsmall()
                                .label(if work { "Undo" } else { "撤销" })
                                .on_click(
                                    cx.listener(|this, _, _window, cx| this.undo_remove_symbol(cx)),
                                ),
                        ),
                )
            })
            .child(
                h_flex()
                    .h(px(38.))
                    .px_3()
                    .items_center()
                    .gap_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background.opacity(0.30))
                    .child(
                        Button::new("add-sym")
                            .primary()
                            .xsmall()
                            .icon(IconName::Plus)
                            .label(if work { "Add" } else { "添加自选" })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_palette(window, cx);
                            })),
                    )
                    .child(
                        Button::new("rm-sym")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Delete)
                            .tooltip(if work { "Remove" } else { "移除当前自选" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.remove_selected_from_watchlist(cx);
                            })),
                    )
                    .child(
                        Button::new("wl-find")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Search)
                            .label(if work { "Find" } else { "发现机会" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_left_tab(LeftTab::Treasure, cx);
                            })),
                    )
                    .child(div().flex_1()),
            )
    }
}
