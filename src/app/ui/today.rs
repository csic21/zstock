use chrono::Datelike;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, PixelsExt, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::domain::climate::{ClimateReport, MarketClimate, NewEntryStance};
use crate::domain::rule_ledger::RuleLedgerReport;
use crate::domain::strategy_application::{StockPlanKind, actionable_plans};
use crate::domain::strategy_arena::ArenaRole;
use crate::domain::today::{TodayAction, TodayActionTarget, TodayOpportunity, TodaySeverity};

use super::super::{StockApp, state::PrimaryTask};

impl StockApp {
    pub(crate) fn render_today_dashboard(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dashboard = self.today_dashboard_view_model();
        let ledger = self.rule_ledger_view_model();
        let action_count = dashboard.actions.len();
        let window_width = window.bounds().size.width.as_f32();
        let window_height = window.bounds().size.height.as_f32();
        let wide = window_width >= 1280.0;
        let content_min_h = (window_height - 34.0 - 68.0 - 48.0).max(420.0);
        let board_min_h = (content_min_h - 220.0).max(280.0);
        let empty_min_h = (board_min_h - 52.0).max(180.0);
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
                    .child({
                        let (fg, bg, text) = match dashboard.climate.stance {
                            NewEntryStance::Freeze => (
                                cx.theme().danger,
                                cx.theme().danger.opacity(0.11),
                                "今日观望，先处理持仓",
                            ),
                            NewEntryStance::Selective if action_count > 0 => (
                                cx.theme().warning,
                                cx.theme().warning.opacity(0.11),
                                "先处理风险，再精选",
                            ),
                            NewEntryStance::Selective => (
                                cx.theme().accent,
                                cx.theme().accent.opacity(0.11),
                                "精选，不扩散新仓",
                            ),
                            NewEntryStance::Open if action_count > 0 => (
                                cx.theme().warning,
                                cx.theme().warning.opacity(0.11),
                                "先处理风险，再看机会",
                            ),
                            NewEntryStance::Open => (
                                cx.theme().success,
                                cx.theme().success.opacity(0.11),
                                "当前没有必须操作",
                            ),
                        };
                        crate::app::helpers::status_pill(text, fg, bg)
                    })
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
                    .px_5()
                    .py_4()
                    .child(
                        v_flex()
                            .id("today-dashboard-content")
                            .debug_selector(|| "today-dashboard-content".into())
                            .w_full()
                            .min_h(px(content_min_h))
                            .gap_4()
                            .child(self.render_today_climate_card(&dashboard.climate, cx))
                            .child(
                                h_flex()
                                    .w_full()
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
                                            &if dashboard.gated_opportunities > 0 {
                                                format!(
                                                    "另有 {} 只等待 · {} 只因市场暂缓",
                                                    dashboard.waiting_opportunities,
                                                    dashboard.gated_opportunities
                                                )
                                            } else {
                                                format!(
                                                    "另有 {} 只等待触发",
                                                    dashboard.waiting_opportunities
                                                )
                                            },
                                            if dashboard.ready_opportunities == 0 {
                                                cx.theme().muted_foreground
                                            } else {
                                                cx.theme().success
                                            },
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
                                    .id("today-board-row")
                                    .debug_selector(|| "today-board-row".into())
                                    .w_full()
                                    .flex_1()
                                    .min_h(px(board_min_h))
                                    .gap_4()
                                    .when(!wide, |row| row.flex_wrap())
                                    .child(
                                        v_flex()
                                            .id("today-actions-column")
                                            .debug_selector(|| "today-actions-column".into())
                                            .flex_1()
                                            .min_w(if wide { px(420.0) } else { px(320.0) })
                                            .min_h(px(220.0))
                                            .gap_2()
                                            .child(today_section_title(
                                                "需要处理",
                                                "提醒、持仓风险和到期计划集中在这里",
                                                cx,
                                            ))
                                            .when(dashboard.actions.is_empty(), |column| {
                                                column.child(
                                                    v_flex()
                                                        .id("today-empty-actions")
                                                        .debug_selector(|| {
                                                            "today-empty-actions".into()
                                                        })
                                                        .flex_1()
                                                        .w_full()
                                                        .min_h(px(empty_min_h))
                                                        .justify_center()
                                                        .gap_1()
                                                        .p_5()
                                                        .rounded(cx.theme().radius_lg)
                                                        .border_1()
                                                        .border_color(
                                                            cx.theme().success.opacity(0.3),
                                                        )
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
                                            .id("today-opportunities-column")
                                            .debug_selector(|| {
                                                "today-opportunities-column".into()
                                            })
                                            .flex_1()
                                            .min_w(if wide { px(420.0) } else { px(320.0) })
                                            .min_h(px(220.0))
                                            .gap_2()
                                            .child(today_section_title(
                                                "候选机会",
                                                if dashboard.climate.stance
                                                    == NewEntryStance::Freeze
                                                {
                                                    "今日气候暂缓新开仓，列表只作观察，不作为动手依据"
                                                } else {
                                                    "符合条件不等于立即买入，仍以观察区和失效条件为准"
                                                },
                                                cx,
                                            ))
                                            .when(dashboard.opportunities.is_empty(), |column| {
                                                column.child(
                                                    v_flex()
                                                        .flex_1()
                                                        .w_full()
                                                        .min_h(px(empty_min_h))
                                                        .justify_center()
                                                        .gap_2()
                                                        .p_5()
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
                                            .when(!dashboard.opportunities.is_empty(), |column| {
                                                column.child(
                                                    v_flex()
                                                        .id("today-opportunities-board")
                                                        .debug_selector(|| {
                                                            "today-opportunities-board".into()
                                                        })
                                                        .flex_1()
                                                        .w_full()
                                                        .min_h(px(empty_min_h))
                                                        .justify_start()
                                                        .content_start()
                                                        .gap_2()
                                                        .p_3()
                                                        .rounded(cx.theme().radius_lg)
                                                        .border_1()
                                                        .border_color(cx.theme().border)
                                                        .bg(cx.theme().sidebar)
                                                        .child(
                                                            v_flex()
                                                                .id("today-opportunities-list")
                                                                .w_full()
                                                                .flex_none()
                                                                .gap_2()
                                                                .children(
                                                                    dashboard
                                                                        .opportunities
                                                                        .iter()
                                                                        .cloned()
                                                                        .enumerate()
                                                                        .map(
                                                                            |(
                                                                                index,
                                                                                opportunity,
                                                                            )| {
                                                                                self.render_today_opportunity_row(
                                                                                    index,
                                                                                    opportunity,
                                                                                    cx,
                                                                                )
                                                                            },
                                                                        ),
                                                                ),
                                                        )
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .flex_none()
                                                                .justify_center()
                                                                .pt_1()
                                                                .child(
                                                                    Button::new(
                                                                        "today-all-opportunities",
                                                                    )
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
                                                                ),
                                                        ),
                                                )
                                            }),
                                    ),
                            )
                            .child(self.render_today_arena_card(cx))
                            .child(self.render_today_champion_orders(cx))
                            .child(self.render_rule_ledger_card(&ledger, cx))
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
                                                format!(
                                                    "先处理失效与集中风险；{}；没有进入观察区不追价；没有定义最大亏损不新增持仓。所有结果仅供学习研究，不构成投资建议。",
                                                    dashboard.climate.stance.label()
                                                ),
                                            ),
                                    ),
                            ),
                    ),
            )
    }

    fn render_today_climate_card(
        &self,
        climate: &ClimateReport,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let work = self.work_mode;
        let (color, badge) = match climate.climate {
            MarketClimate::Attack => (cx.theme().success, if work { "scale-up" } else { "进攻" }),
            MarketClimate::Select => (cx.theme().accent, if work { "selective" } else { "精选" }),
            MarketClimate::Defend => (cx.theme().warning, if work { "defensive" } else { "防守" }),
            MarketClimate::StandAside => (cx.theme().danger, if work { "hold" } else { "观望" }),
        };
        let tape = climate
            .reasons
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(" · ");
        let completeness = if climate.completeness_pct < 40.0 {
            if work {
                "evidence incomplete · default selective"
            } else {
                "宽度尚未齐，先按精选处理"
            }
        } else {
            ""
        };

        h_flex()
            .id("today-climate-card")
            .w_full()
            .gap_3()
            .items_center()
            .p_4()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(color.opacity(0.35))
            .bg(color.opacity(0.07))
            .child(crate::app::helpers::status_pill(
                badge,
                color,
                color.opacity(0.16),
            ))
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
                            .child(if work {
                                climate.climate.work_label().to_string()
                            } else {
                                climate.headline.clone()
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if completeness.is_empty() {
                                if tape.is_empty() {
                                    climate.detail.clone()
                                } else {
                                    tape
                                }
                            } else {
                                completeness.to_string()
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("新开仓风险 {:.0}%", climate.risk_scale * 100.0)),
            )
            .child(
                Button::new("today-climate-open-market")
                    .ghost()
                    .xsmall()
                    .label(if work { "Tape" } else { "查看宽度" })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.open_market_analysis(cx);
                    })),
            )
            .into_any_element()
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
            TodayActionTarget::Market => "查看市场",
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
            .child(crate::app::helpers::status_pill(
                badge,
                color,
                color.opacity(0.14),
            ))
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
        let gated = opportunity.gate_reason.is_some();
        let color = if opportunity.ready {
            cx.theme().success
        } else if gated {
            cx.theme().warning
        } else {
            cx.theme().muted_foreground
        };
        let status = if opportunity.ready {
            "符合观察条件"
        } else if gated {
            "市场暂缓"
        } else {
            "等待触发"
        };
        let code = opportunity.code.clone();
        h_flex()
            .id(("today-opportunity", index))
            .debug_selector(move || format!("today-opportunity-{index}"))
            .w_full()
            .flex_none()
            .gap_3()
            .items_center()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                div()
                    .w(px(58.0))
                    .flex_none()
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
                    .justify_start()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .flex_none()
                            .gap_1()
                            .items_center()
                            .min_w_0()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(opportunity.name),
                            )
                            .child(crate::app::helpers::status_pill(
                                status,
                                color,
                                color.opacity(0.14),
                            )),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_none()
                            .truncate()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(opportunity.gate_reason.as_ref().map_or_else(
                                || {
                                    format!(
                                        "{} · 匹配度 {:.0} · 观察区 {}",
                                        opportunity.strategy,
                                        opportunity.score,
                                        opportunity.observation
                                    )
                                },
                                |reason| {
                                    format!(
                                        "{} · 匹配度 {:.0} · {}",
                                        opportunity.strategy, opportunity.score, reason
                                    )
                                },
                            )),
                    ),
            )
            .child(
                Button::new(("today-opportunity-open", index))
                    .xsmall()
                    .flex_shrink_0()
                    .when(opportunity.ready, |button| button.primary())
                    .when(!opportunity.ready, |button| button.ghost())
                    .label("查看")
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.open_today_opportunity(&code, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_today_arena_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let work = self.work_mode;
        let arena = self.strategy_lab_feature.arena_snapshot();
        let challengers: Vec<_> = arena
            .standings
            .iter()
            .filter(|row| row.role == ArenaRole::Challenger)
            .take(2)
            .cloned()
            .collect();
        v_flex()
            .id("today-strategy-arena")
            .w_full()
            .gap_2()
            .p_4()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(today_section_title(
                        if work {
                            "Strategy arena"
                        } else {
                            "策略角逐"
                        },
                        if work {
                            "Composite robustness, not win rate alone. Daily paper and bounded offspring can dethrone the leader."
                        } else {
                            "按稳健分角逐最强策略；每日观察会校正排名，并从冠军进化有界变体。冠军不等于建议实盘"
                        },
                        cx,
                    ))
                    .child(
                        Button::new("today-open-arena")
                            .ghost()
                            .xsmall()
                            .label(if work { "Library" } else { "打开策略库" })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.strategy_lab_open_library_page(cx);
                            })),
                    ),
            )
            .when_some(arena.champion.as_ref(), |card, champion| {
                card.child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().success)
                                .child(format!(
                                    "{} · {} · 稳健分 {:.1}",
                                    if work { "Champion" } else { "当前冠军" },
                                    champion.record.strategy_name,
                                    champion.score
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{}{:.1}% · {} {:+.2}% · {} {:.1}% · {} {}",
                                    if work { "win " } else { "胜率 " },
                                    champion.record.win_rate_pct,
                                    if work { "excess" } else { "超额" },
                                    champion.record.excess_return_pct,
                                    if work { "DD" } else { "回撤" },
                                    champion.record.max_drawdown_pct,
                                    champion.record.trade_count,
                                    if work { "trades" } else { "笔" }
                                )),
                        ),
                )
            })
            .when(arena.champion.is_none(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(arena.headline.clone()),
                )
            })
            .when(!challengers.is_empty(), |card| {
                card.child(
                    v_flex()
                        .gap_1()
                        .children(challengers.into_iter().map(|row| {
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} #{:02} {} · {:.1}",
                                    if work { "Challenger" } else { "挑战者" },
                                    row.rank,
                                    row.record.strategy_name,
                                    row.score
                                ))
                        })),
                )
            })
            .into_any_element()
    }

    fn render_today_champion_orders(&self, cx: &mut Context<Self>) -> AnyElement {
        let work = self.work_mode;
        let (champion_name, plans) = self.champion_stock_plans(cx);
        let actions = actionable_plans(&plans);
        let hold_count = plans
            .iter()
            .filter(|plan| plan.kind == StockPlanKind::Hold)
            .count();
        let wait_count = plans
            .iter()
            .filter(|plan| plan.kind == StockPlanKind::Wait)
            .count();
        v_flex()
            .id("today-champion-orders")
            .w_full()
            .gap_2()
            .p_4()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(today_section_title(
                if work {
                    "Champion on your stocks"
                } else {
                    "冠军落到个股"
                },
                if work {
                    "Sized lots from the arena champion. Next-open proxy, local records only."
                } else {
                    "用当前冠军策略扫自选和持仓：买入/卖出股数按计划本金与止损计算，次日开盘成交，不自动下单"
                },
                cx,
            ))
            .when_some(champion_name.as_ref(), |card, name| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{}{} · {} {} · {} {} · {} {}",
                            if work { "Using " } else { "正在用 " },
                            name,
                            actions
                                .iter()
                                .filter(|plan| plan.kind == StockPlanKind::Buy)
                                .count(),
                            if work { "buys" } else { "只买入" },
                            actions
                                .iter()
                                .filter(|plan| plan.kind == StockPlanKind::Sell)
                                .count(),
                            if work { "sells" } else { "只卖出" },
                            hold_count,
                            if work { "holds" } else { "只继续持有" }
                        )),
                )
            })
            .when(champion_name.is_none(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if work {
                            "Run Strategy Lab until a champion exists, then lots can be sized here."
                        } else {
                            "先在策略实验室跑出冠军。有冠军后，这里会告诉你每只股票买多少、卖多少。"
                        }),
                )
            })
            .when(champion_name.is_some() && actions.is_empty(), |card| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if wait_count > 0 {
                            format!(
                                "{}{}{}",
                                if work {
                                    "No buy/sell today. "
                                } else {
                                    "今天没有买卖计划。"
                                },
                                wait_count,
                                if work {
                                    " names waiting for a signal or daily bars."
                                } else {
                                    " 只在等待信号或日 K。"
                                }
                            )
                        } else if work {
                            "Open a stock's daily chart so the champion can see enough bars.".into()
                        } else {
                            "打开自选或持仓的日 K 后，才能按冠军策略计算股数。".into()
                        }),
                )
            })
            .children(actions.into_iter().take(8).enumerate().map(|(index, plan)| {
                let code = plan.code.clone();
                let kind_color = match plan.kind {
                    StockPlanKind::Buy => cx.theme().success,
                    StockPlanKind::Sell => cx.theme().danger,
                    StockPlanKind::Hold => cx.theme().accent,
                    StockPlanKind::Wait => cx.theme().muted_foreground,
                };
                h_flex()
                    .id(("today-champion-order", index))
                    .w_full()
                    .gap_3()
                    .items_center()
                    .px_3()
                    .py_2()
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background.opacity(0.35))
                    .child(
                        div()
                            .w(px(48.0))
                            .flex_none()
                            .text_xs()
                            .font_semibold()
                            .text_color(kind_color)
                            .child(plan.kind.label()),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .child(format!("{} {}", plan.code, plan.name)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} 股 · {:.2} · {}",
                                        plan.shares, plan.price, plan.reason
                                    )),
                            ),
                    )
                    .child(
                        Button::new(("today-champion-prefill", index))
                            .xsmall()
                            .primary()
                            .label(if work { "Prefill" } else { "预填记录" })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.prefill_champion_stock_plan(&code, window, cx);
                            })),
                    )
            }))
            .into_any_element()
    }

    fn render_rule_ledger_card(
        &self,
        ledger: &RuleLedgerReport,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let work = self.work_mode;
        v_flex()
            .w_full()
            .gap_2()
            .p_4()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(today_section_title(
                if work { "Rule ledger" } else { "规则台账" },
                if work {
                    "Cross-symbol calibration · not a promotion score"
                } else {
                    "按策略、分数段、是否按计划和市场状态汇总 10 日真实结果，不单独用胜率晋级"
                },
                cx,
            ))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(ledger.headline.clone()),
            )
            .when(ledger.sample > 0, |card| {
                card.child(
                    h_flex().gap_2().flex_wrap().children(
                        [
                            (
                                if work { "Win" } else { "胜率" },
                                format_opt_pct(ledger.overall.win_rate_pct),
                            ),
                            (
                                if work { "Expectancy" } else { "期望" },
                                format_opt_signed(ledger.overall.expectancy_pct),
                            ),
                            ("MFE", format_opt_signed(ledger.overall.average_mfe_pct)),
                            ("MAE", format_opt_signed(ledger.overall.average_mae_pct)),
                            (
                                if work { "Target hit" } else { "目标触及" },
                                format_opt_pct(ledger.exit.target_touch_rate_pct),
                            ),
                        ]
                        .into_iter()
                        .map(|(label, value)| ledger_metric(label, &value, cx)),
                    ),
                )
            })
            .when_some(ledger.exit_hint.clone(), |card, hint| {
                card.child(div().text_xs().text_color(cx.theme().warning).child(hint))
            })
            .when(ledger.sample > 0, |card| {
                card.child(
                    h_flex().gap_3().flex_wrap().children(
                        [
                            ("策略", ledger.by_strategy.as_slice()),
                            ("分数段", ledger.by_score.as_slice()),
                            ("纪律", ledger.by_followed.as_slice()),
                            ("状态", ledger.by_regime.as_slice()),
                        ]
                        .into_iter()
                        .filter(|(_, slices)| !slices.is_empty())
                        .map(|(title, slices)| ledger_group(title, slices, work, cx)),
                    ),
                )
            })
            .into_any_element()
    }
}

fn ledger_metric(label: &str, value: &str, cx: &mut Context<StockApp>) -> AnyElement {
    v_flex()
        .gap_0p5()
        .px_3()
        .py_2()
        .rounded(cx.theme().radius)
        .bg(cx.theme().muted.opacity(0.35))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(value.to_string()),
        )
        .into_any_element()
}

fn ledger_group(
    title: &str,
    slices: &[crate::domain::rule_ledger::LedgerSlice],
    work: bool,
    cx: &mut Context<StockApp>,
) -> AnyElement {
    v_flex()
        .min_w(px(180.0))
        .gap_1()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(title.to_string()),
        )
        .children(slices.iter().take(4).map(|slice| {
            div()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(format!(
                    "{} · {} {} · {} {}",
                    slice.label,
                    slice.sample,
                    if work { "n" } else { "笔" },
                    format_opt_pct(slice.win_rate_pct),
                    format_opt_signed(slice.expectancy_pct)
                ))
        }))
        .into_any_element()
}

fn format_opt_pct(value: Option<f64>) -> String {
    value
        .map(|number| format!("{number:.0}%"))
        .unwrap_or_else(|| "—".into())
}

fn format_opt_signed(value: Option<f64>) -> String {
    value
        .map(|number| format!("{number:+.2}%"))
        .unwrap_or_else(|| "—".into())
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
        .min_w(px(168.0))
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
