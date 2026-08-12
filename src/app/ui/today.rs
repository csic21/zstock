use chrono::Datelike;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::domain::today::{TodayAction, TodayActionTarget, TodayOpportunity, TodaySeverity};

use super::super::{StockApp, state::PrimaryTask};

impl StockApp {
    pub(crate) fn render_today_dashboard(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dashboard = self.today_dashboard_view_model();
        let action_count = dashboard.actions.len();
        let now = chrono::Local::now();
        let weekday = match now.weekday() {
            chrono::Weekday::Mon => "星期一",
            chrono::Weekday::Tue => "星期二",
            chrono::Weekday::Wed => "星期三",
            chrono::Weekday::Thu => "星期四",
            chrono::Weekday::Fri => "星期五",
            chrono::Weekday::Sat => "星期六",
            chrono::Weekday::Sun => "星期日",
        };
        let date = format!("{} · {weekday}", now.format("%Y年%m月%d日"));

        v_flex()
            .id("today-dashboard")
            .debug_selector(|| "today-dashboard-root".into())
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .h(px(68.0))
                    .flex_shrink_0()
                    .px_5()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("今天，先处理什么？"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(date),
                            ),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_full()
                            .bg(if action_count == 0 {
                                cx.theme().success.opacity(0.11)
                            } else {
                                cx.theme().warning.opacity(0.11)
                            })
                            .text_xs()
                            .font_semibold()
                            .text_color(if action_count == 0 {
                                cx.theme().success
                            } else {
                                cx.theme().warning
                            })
                            .child(if action_count == 0 {
                                "当前没有必须操作"
                            } else {
                                "先处理风险，再看机会"
                            }),
                    )
                    .child(
                        Button::new("today-market-context")
                            .ghost()
                            .xsmall()
                            .label("查看市场")
                            .tooltip("查看指数、市场宽度、行业榜和热力图")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.open_market_analysis(cx);
                            })),
                    )
                    .child(
                        Button::new("today-refresh")
                            .ghost()
                            .xsmall()
                            .label("刷新全部")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.refresh_all(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .id("today-dashboard-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_5()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(1360.0))
                            .gap_5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .children([
                                        today_metric(
                                            "待处理",
                                            &action_count.to_string(),
                                            if action_count == 0 {
                                                "无需为了交易而交易"
                                            } else {
                                                "按严重度排列"
                                            },
                                            if action_count == 0 {
                                                cx.theme().success
                                            } else {
                                                cx.theme().warning
                                            },
                                            cx,
                                        ),
                                        today_metric(
                                            "持仓",
                                            &dashboard.open_positions.to_string(),
                                            "先检查失效价与集中度",
                                            cx.theme().foreground,
                                            cx,
                                        ),
                                        today_metric(
                                            "已设提醒",
                                            &dashboard.active_alerts.to_string(),
                                            "观察 · 目标 · 失效",
                                            cx.theme().accent,
                                            cx,
                                        ),
                                        today_metric(
                                            "符合观察条件",
                                            &dashboard.ready_opportunities.to_string(),
                                            &format!(
                                                "另有 {} 只等待触发",
                                                dashboard.waiting_opportunities
                                            ),
                                            cx.theme().success,
                                            cx,
                                        ),
                                        today_metric(
                                            "待复盘",
                                            &dashboard.due_reviews.to_string(),
                                            "复核执行与计划纪律",
                                            if dashboard.due_reviews == 0 {
                                                cx.theme().muted_foreground
                                            } else {
                                                cx.theme().warning
                                            },
                                            cx,
                                        ),
                                    ]),
                            )
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_4()
                                    .items_start()
                                    .flex_wrap()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w(px(500.0))
                                            .gap_2()
                                            .child(today_section_title(
                                                "需要处理",
                                                "提醒、持仓风险和到期计划集中在这里",
                                                cx,
                                            ))
                                            .when(dashboard.actions.is_empty(), |column| {
                                                column.child(
                                                    v_flex()
                                                        .gap_1()
                                                        .p_4()
                                                        .rounded(cx.theme().radius_lg)
                                                        .border_1()
                                                        .border_color(cx.theme().success.opacity(0.3))
                                                        .bg(cx.theme().success.opacity(0.05))
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .font_semibold()
                                                                .text_color(cx.theme().success)
                                                                .child("今天没有必须操作的事项"),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .child(
                                                                    "没有触发条件时，保持等待也是有效决策。",
                                                                ),
                                                        ),
                                                )
                                            })
                                            .children(
                                                dashboard
                                                    .actions
                                                    .iter()
                                                    .take(10)
                                                    .cloned()
                                                    .enumerate()
                                                    .map(|(index, action)| {
                                                        self.render_today_action_row(
                                                            index, action, cx,
                                                        )
                                                    }),
                                            )
                                            .when(dashboard.actions.len() > 10, |column| {
                                                column.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(format!(
                                                            "另有 {} 项；处理当前风险后会自动更新",
                                                            dashboard.actions.len() - 10
                                                        )),
                                                )
                                            }),
                                    )
                                    .child(
                                        v_flex()
                                            .w(px(430.0))
                                            .min_w(px(340.0))
                                            .gap_2()
                                            .child(today_section_title(
                                                "候选机会",
                                                "符合条件不等于立即买入，仍以观察区和失效条件为准",
                                                cx,
                                            ))
                                            .when(dashboard.opportunities.is_empty(), |column| {
                                                column.child(
                                                    v_flex()
                                                        .gap_2()
                                                        .p_4()
                                                        .rounded(cx.theme().radius_lg)
                                                        .border_1()
                                                        .border_color(cx.theme().border)
                                                        .bg(cx.theme().sidebar)
                                                        .child(
                                                            div()
                                                                .text_sm()
                                                                .font_semibold()
                                                                .child("还没有候选"),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .child(
                                                                    "运行低位策略或短线雷达后，结果会自动汇总到这里。",
                                                                ),
                                                        )
                                                        .child(
                                                            Button::new("today-empty-opportunities")
                                                                .ghost()
                                                                .xsmall()
                                                                .label("打开机会")
                                                                .on_click(cx.listener(
                                                                    |this, _, _window, cx| {
                                                                        this.set_primary_task(
                                                                            PrimaryTask::Opportunities,
                                                                            cx,
                                                                        );
                                                                    },
                                                                )),
                                                        ),
                                                )
                                            })
                                            .children(
                                                dashboard
                                                    .opportunities
                                                    .iter()
                                                    .cloned()
                                                    .enumerate()
                                                    .map(|(index, opportunity)| {
                                                        self.render_today_opportunity_row(
                                                            index,
                                                            opportunity,
                                                            cx,
                                                        )
                                                    }),
                                            )
                                            .when(!dashboard.opportunities.is_empty(), |column| {
                                                column.child(
                                                    Button::new("today-all-opportunities")
                                                        .ghost()
                                                        .xsmall()
                                                        .label("查看全部机会")
                                                        .on_click(cx.listener(
                                                            |this, _, _window, cx| {
                                                                this.set_primary_task(
                                                                    PrimaryTask::Opportunities,
                                                                    cx,
                                                                );
                                                            },
                                                        )),
                                                )
                                            }),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_4()
                                    .rounded(cx.theme().radius_lg)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().sidebar.opacity(0.7))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child("今日纪律"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(
                                                "先处理失效与集中风险；没有进入观察区不追价；没有定义最大亏损不新增持仓。所有结果仅供学习研究，不构成投资建议。",
                                            ),
                                    ),
                            ),
                    ),
            )
    }

    fn render_today_action_row(
        &self,
        index: usize,
        action: TodayAction,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = match action.severity {
            TodaySeverity::Critical => cx.theme().danger,
            TodaySeverity::Warning => cx.theme().warning,
            TodaySeverity::Info => cx.theme().accent,
        };
        let badge = match action.severity {
            TodaySeverity::Critical => "立即核对",
            TodaySeverity::Warning => "需要处理",
            TodaySeverity::Info => "条件触发",
        };
        let button_label = match action.target {
            TodayActionTarget::Research => "查看决策",
            TodayActionTarget::Opportunities => "打开机会",
            TodayActionTarget::Portfolio => "处理",
        };
        let action_for_click = action.clone();

        h_flex()
            .id(("today-action", index))
            .w_full()
            .gap_3()
            .items_center()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_l_1()
            .border_color(color.opacity(0.35))
            .bg(color.opacity(0.05))
            .child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(color.opacity(0.14))
                    .text_xs()
                    .font_semibold()
                    .text_color(color)
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
                            .child(action.title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(action.detail),
                    ),
            )
            .child(
                Button::new(("today-action-open", index))
                    .xsmall()
                    .when(action.severity == TodaySeverity::Critical, |button| {
                        button.danger()
                    })
                    .when(action.severity != TodaySeverity::Critical, |button| {
                        button.ghost()
                    })
                    .label(button_label)
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.open_today_action(action_for_click.clone(), cx);
                    })),
            )
            .into_any_element()
    }

    fn render_today_opportunity_row(
        &self,
        index: usize,
        opportunity: TodayOpportunity,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let color = if opportunity.ready {
            cx.theme().success
        } else {
            cx.theme().muted_foreground
        };
        let code = opportunity.code.clone();
        h_flex()
            .id(("today-opportunity", index))
            .w_full()
            .gap_2()
            .items_center()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .w(px(58.0))
                    .text_xs()
                    .font_family("Menlo")
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(opportunity.code.clone()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(opportunity.name),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(color)
                                    .child(if opportunity.ready {
                                        "符合观察条件"
                                    } else {
                                        "等待触发"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} · 匹配度 {:.0} · 观察区 {}",
                                opportunity.strategy, opportunity.score, opportunity.observation
                            )),
                    ),
            )
            .child(
                Button::new(("today-opportunity-open", index))
                    .xsmall()
                    .when(opportunity.ready, |button| button.primary())
                    .when(!opportunity.ready, |button| button.ghost())
                    .label("查看")
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.open_today_opportunity(&code, cx);
                    })),
            )
            .into_any_element()
    }
}

fn today_metric(
    label: &str,
    value: &str,
    hint: &str,
    color: gpui::Hsla,
    cx: &mut Context<StockApp>,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w(px(188.0))
        .max_w(px(260.0))
        .gap_1()
        .p_4()
        .rounded(cx.theme().radius_lg)
        .border_1()
        .border_color(cx.theme().border.opacity(0.82))
        .bg(cx.theme().sidebar)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_xl()
                .font_semibold()
                .text_color(color)
                .child(value.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(hint.to_string()),
        )
        .into_any_element()
}

fn today_section_title(title: &str, subtitle: &str, cx: &mut Context<StockApp>) -> AnyElement {
    v_flex()
        .gap_0p5()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(title.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(subtitle.to_string()),
        )
        .into_any_element()
}
