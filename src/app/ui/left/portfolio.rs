use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    v_flex,
};

use crate::app::helpers::*;
use crate::app::{DetailTab, StockApp};
use crate::data::portfolio::{TradeSide, format_money, format_shares};
use crate::model::{disguise_label, format_pct, format_price, shared};

impl StockApp {
    pub(crate) fn render_portfolio_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let selected = self.selected.clone();
        let summary = self.portfolio_summary();
        let risk_view = self.portfolio_risk_view(&summary);
        let form_side = self.trade_form_side;
        let currency_groups: Vec<_> = summary.by_currency.values().cloned().collect();

        let mut root = v_flex().flex_1().min_h_0().w_full();

        // 分币种组合汇总；没有 FX 时绝不显示伪精确总计。
        root = root.child(
            v_flex()
                .gap_1()
                .px_2()
                .py_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .when(currency_groups.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "No positions" } else { "暂无持仓" }),
                    )
                })
                .children(currency_groups.into_iter().map(|totals| {
                    let pnl_color = self.chg_color(totals.total_unrealized_pnl >= 0.0, cx);
                    v_flex()
                        .gap_0p5()
                        .py_1()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{} · {}",
                                            totals.currency.symbol(),
                                            if work { "Book value" } else { "持仓市值" }
                                        )),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child(format!("{:.0}", totals.total_market_value)),
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
                                    div().text_xs().font_semibold().text_color(pnl_color).child(
                                        format!(
                                            "{} ({})",
                                            format_money(totals.total_unrealized_pnl),
                                            format_pct(totals.total_unrealized_pnl_pct)
                                        ),
                                    ),
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
                                            .child(format!("{:.0}", totals.cash.major())),
                                    ),
                            )
                        })
                }))
                .when(!summary.pending_currency_codes.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(format!(
                                "{} 条旧记录币种待确认",
                                summary.pending_currency_codes.len()
                            )),
                    )
                }),
        );

        if !risk_view.items.is_empty() {
            let largest = risk_view
                .items
                .first()
                .map(|item| {
                    format!(
                        "{} · {} {:.1}%",
                        item.code,
                        item.currency.symbol(),
                        item.position_weight_pct
                    )
                })
                .unwrap_or_else(|| "—".into());
            let risk_rows = risk_view.items.iter().take(3).cloned().collect::<Vec<_>>();
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
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(if work { "Risk center" } else { "组合风险" }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().warning)
                                    .child(format!("最大集中：{largest}")),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "行情覆盖 {:.0}% · 失效价覆盖 {:.0}% · 行业覆盖 {:.0}%（缺失不计作低风险）",
                                risk_view.quote_coverage_pct,
                                risk_view.invalidation_coverage_pct,
                                risk_view.industry_coverage_pct
                            )),
                    )
                    .children(risk_rows.into_iter().map(|item| {
                        let amount = item
                            .risk_amount
                            .map(|money| format!("{} {:.0}", money.currency.symbol(), money.major()))
                            .unwrap_or_else(|| "风险金额未知".into());
                        let state = if item.quote_stale {
                            "行情缺失"
                        } else if item.invalidation_breached {
                            "已触及失效价"
                        } else {
                            ""
                        };
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(format!(
                                        "{} · {} {:.1}%",
                                        item.code,
                                        item.currency.symbol(),
                                        item.position_weight_pct
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if state.is_empty() {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().danger
                                    })
                                    .child(if state.is_empty() {
                                        amount
                                    } else {
                                        format!("{state} · {amount}")
                                    }),
                            )
                    })),
            );
        }

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
                                    .child(format!("{} · {}", side_label, self.selected.as_ref())),
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
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.trade_shares_input).small()),
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
                                    .child(if work { "Px" } else { "价格" }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.trade_price_input).small()),
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
                                    .child(if work { "Fee" } else { "费用" }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.trade_fee_input).small()),
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
                                    .child(if work { "Note" } else { "备注" }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.trade_note_input).small()),
                            ),
                    )
                    .child(
                        Button::new("trade-submit")
                            .xsmall()
                            .primary()
                            .label(if work { "Submit" } else { "确认成交" })
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
        let rows: Vec<_> = summary.positions.to_vec();
        let selected_review = self.position_review_view_model();
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
                                        .child({
                                            let cost_line = if name_show.is_empty() {
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
                                            };
                                            if is_selected {
                                                if let Some(review) = &selected_review {
                                                    format!(
                                                        "{} · {}",
                                                        review.stance.label(work),
                                                        cost_line
                                                    )
                                                } else {
                                                    cost_line
                                                }
                                            } else {
                                                cost_line
                                            }
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
}
