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
    pub(crate) fn render_portfolio_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let work = self.work_mode;
        if self.portfolio_recovery.is_some() || self.recovery_busy {
            return v_flex().w_full().gap_2().p_3()
                .id("portfolio-data-unavailable")
                .debug_selector(|| "portfolio-data-unavailable".into())
                .child(self.render_financial_storage_status(crate::storage::Slot::Portfolio, cx))
                .child(div().text_sm().child(if work { "Holdings and risk unavailable" } else { "持仓与风险暂不可用" }))
                .child(div().text_xs().child(if work {
                    "The portfolio could not be validated. Quantity, cash, P&L and concentration are unknown until recovery. Market research remains available."
                } else {
                    "持仓文件尚未通过校验，无法确认持股、现金、盈亏及集中度。请先恢复本地数据；行情研究仍可使用。"
                }))
                .into_any_element();
        }
        let selected = self.selected.clone();
        let summary = self.portfolio_summary();
        let risk_view = self.portfolio_risk_view(&summary);
        let form_open = self.trade_form.is_some();
        let currency_groups: Vec<_> = summary.by_currency.values().cloned().collect();

        let mut root = v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(self.render_financial_storage_status(crate::storage::Slot::Portfolio, cx))
            .when(self.journal_recovery.is_some(), |panel| {
                panel.child(self.render_financial_storage_status(crate::storage::Slot::Journal, cx))
            });

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
                    let has_value = totals.valued_count > 0 || totals.missing_quote_count == 0;
                    let partial = totals.missing_quote_count > 0;
                    let pnl_color = if has_value {
                        self.chg_color(totals.total_unrealized_pnl >= 0.0, cx)
                    } else {
                        cx.theme().muted_foreground
                    };
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
                                            if partial {
                                                if work {
                                                    "Partial value"
                                                } else {
                                                    "已知市值（不完整）"
                                                }
                                            } else if work {
                                                "Market value"
                                            } else {
                                                "持仓市值"
                                            }
                                        )),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child(if has_value {
                                            format!("{:.0}", totals.total_market_value)
                                        } else {
                                            "—".into()
                                        }),
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
                                        .child(if partial {
                                            if work {
                                                "Known P&L only"
                                            } else {
                                                "已知部分浮盈亏"
                                            }
                                        } else if work {
                                            "Unrealized"
                                        } else {
                                            "浮动盈亏"
                                        }),
                                )
                                .child(
                                    div().text_xs().font_semibold().text_color(pnl_color).child(
                                        if has_value {
                                            format!(
                                                "{} ({})",
                                                format_money(totals.total_unrealized_pnl),
                                                format_pct(totals.total_unrealized_pnl_pct)
                                            )
                                        } else {
                                            if work { "Unavailable" } else { "估值未知" }.into()
                                        },
                                    ),
                                ),
                        )
                        .when(
                            partial
                                || totals.stale_quote_count > 0
                                || totals.uncertain_quote_count > 0,
                            |this| {
                                this.child(div().text_xs().text_color(cx.theme().warning).child(
                                    format!(
                                        "{} {} · {} {} · {} {}",
                                        if work { "Missing" } else { "缺少行情" },
                                        totals.missing_quote_count,
                                        if work { "Stale" } else { "过期估值" },
                                        totals.stale_quote_count,
                                        if work { "Unknown time" } else { "时间未知" },
                                        totals.uncertain_quote_count
                                    ),
                                ))
                            },
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
                                "行情覆盖 {:.0}% · 失效价覆盖 {:.0}% · 行业覆盖 {:.0}%（仓位比例仅含已知估值，缺失不计作低风险）",
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
                            "行情缺失/过期"
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
        if let Some(form) = &self.trade_form {
            let side = form.side;
            let side_label = if work {
                side.label_work()
            } else {
                side.label()
            };
            let mut form_panel = v_flex()
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
                                    "{} · {} {} · {}",
                                    side_label,
                                    form.name,
                                    form.code,
                                    form.currency.symbol()
                                )),
                        )
                        .child(
                            Button::new("trade-form-close")
                                .ghost()
                                .xsmall()
                                .label(if work { "Close" } else { "取消" })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.trade_form = None;
                                    this.trade_feedback = None;
                                    cx.notify();
                                })),
                        ),
                );
            form_panel = form_panel.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "Local transaction record only. No broker connection."
                    } else {
                        "仅保存本地交易记录，不连接券商或发送订单。"
                    }),
            );
            if let Some(confirmed) = &form.confirmation {
                form_panel = form_panel
                    .child(div().text_xs().child(format!(
                        "{} {} · {} · {}",
                        form.name,
                        form.code,
                        form.currency.symbol(),
                        side_label
                    )))
                    .child(div().text_xs().child(format!(
                        "{} {} · {} {} {}",
                        if work { "Quantity" } else { "股数" },
                        confirmed.shares,
                        if work {
                            "Actual price"
                        } else {
                            "实际成交价"
                        },
                        form.currency.symbol(),
                        confirmed.price
                    )))
                    .child(div().text_xs().child(format!(
                        "{} {} {:.2} · {} {} {}",
                        if work { "Gross" } else { "成交金额" },
                        form.currency.symbol(),
                        confirmed.notional(),
                        if work { "Fee" } else { "费用" },
                        form.currency.symbol(),
                        confirmed.fee
                    )))
                    .child(div().text_sm().font_semibold().child(format!(
                        "{} {} {:.2}",
                        if side == TradeSide::Buy {
                            if work {
                                "Total outflow"
                            } else {
                                "合计支出"
                            }
                        } else if work {
                            "Net proceeds"
                        } else {
                            "合计收入"
                        },
                        form.currency.symbol(),
                        confirmed.total(side)
                    )))
                    .when(!confirmed.note.is_empty(), |panel| {
                        panel.child(div().text_xs().child(confirmed.note.clone()))
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("trade-edit")
                                    .xsmall()
                                    .ghost()
                                    .label(if work { "Back to edit" } else { "返回修改" })
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        if let Some(form) = &mut this.trade_form {
                                            form.confirmation = None;
                                        }
                                        this.trade_feedback = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("trade-submit")
                                    .xsmall()
                                    .primary()
                                    .label(if work {
                                        "Confirm local record"
                                    } else {
                                        "确认保存本地记录"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_trade(window, cx)
                                    })),
                            ),
                    );
            } else {
                form_panel = form_panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(form.reference_hint.clone()),
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
                        Button::new("trade-review")
                            .xsmall()
                            .primary()
                            .label(if work {
                                "Review local record"
                            } else {
                                "核对本地记录"
                            })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.review_trade(cx);
                            })),
                    );
            }
            root = root.child(form_panel);
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
                    let pnl_c = mark
                        .unrealized_pnl
                        .map(|pnl| self.chg_color(pnl >= 0.0, cx))
                        .unwrap_or(cx.theme().muted_foreground);
                    let shares_s = format_shares(mark.position.shares);
                    let pnl_s = match (mark.unrealized_pnl, mark.unrealized_pnl_pct) {
                        (Some(pnl), Some(pct)) => {
                            format!("{} {}", format_money(pnl), format_pct(pct))
                        }
                        _ => if work { "Unavailable" } else { "未知" }.into(),
                    };
                    let valuation_note = format!(
                        "{}{}",
                        mark.valuation_label(work),
                        mark.quote_as_of
                            .as_ref()
                            .map(|as_of| format!(" · {as_of}"))
                            .unwrap_or_default()
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
                                                    mark.last
                                                        .map(format_price)
                                                        .unwrap_or_else(|| "—".into())
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
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(if mark.quote_is_uncertain() {
                                            cx.theme().warning
                                        } else {
                                            cx.theme().muted_foreground
                                        })
                                        .child(valuation_note),
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

        if summary.open_count == 0 && !form_open {
            root = root.child(
                div()
                    .px_3()
                    .py_4()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "No positions. Add a local buy record."
                    } else {
                        "暂无持仓。选中标的后添加本地买入记录；不发送券商订单。"
                    }),
            );
        }

        // Keep validation, save feedback, and undo alongside the action.
        if let Some(feedback) = &self.trade_feedback {
            root = root.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(feedback.clone()),
            );
        }
        if let Some(last_trade) = self.portfolio.trades.last() {
            let undo_code = last_trade.code.clone();
            root = root.child(
                Button::new("port-undo-last")
                    .xsmall()
                    .ghost()
                    .label(format!(
                        "{} {}",
                        if work {
                            "Undo last local record"
                        } else {
                            "撤销最近本地记录"
                        },
                        undo_code
                    ))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.undo_last_trade_for_code(&undo_code, cx)
                    })),
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
                        .label(if work {
                            "Full sell…"
                        } else {
                            "清仓记录…"
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_selected_position(window, cx);
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
        .into_any_element()
    }
}
