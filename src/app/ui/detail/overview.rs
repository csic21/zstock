use gpui::{Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px};
use gpui_component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    v_flex,
};

use crate::app::StockApp;
use crate::app::helpers::*;
use crate::domain::climate::NewEntryStance;
use crate::domain::decision::{DecisionCard, DecisionStatus};
use crate::domain::decision::{DecisionStepState, DecisionTrace};

impl StockApp {
    pub(super) fn render_decision_trace(
        &self,
        trace: &DecisionTrace,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let outcome_color = match trace.outcome {
            crate::domain::decision::DecisionOutcome::Calculating => cx.theme().accent,
            crate::domain::decision::DecisionOutcome::PlanReady => cx.theme().success,
            crate::domain::decision::DecisionOutcome::Wait => cx.theme().warning,
            crate::domain::decision::DecisionOutcome::NoAction => cx.theme().danger,
            crate::domain::decision::DecisionOutcome::NeedEvidence => cx.theme().muted_foreground,
        };

        v_flex()
            .w_full()
            .gap_1p5()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(outcome_color.opacity(0.45))
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("决策过程"),
                    )
                    .child(status_pill(
                        trace.outcome.label(),
                        outcome_color,
                        outcome_color.opacity(0.16),
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "已计算 {}/{} 步 · 规则引擎实时更新",
                                trace.evaluated_steps,
                                trace.steps.len()
                            )),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("不会自动下单"),
                    ),
            )
            .children(trace.steps.iter().enumerate().map(|(index, step)| {
                let color = match step.state {
                    DecisionStepState::Running => cx.theme().accent,
                    DecisionStepState::Passed => cx.theme().success,
                    DecisionStepState::Attention => cx.theme().warning,
                    DecisionStepState::Blocked => cx.theme().danger,
                };
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .px_1()
                    .py_0p5()
                    .child(
                        div()
                            .w(px(20.0))
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{}", index + 1)),
                    )
                    .child(
                        div()
                            .w(px(74.0))
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(step.title.clone()),
                    )
                    .child(status_pill_sized(
                        step.state.label(),
                        color,
                        color.opacity(0.14),
                        Some(48.0),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(if step.state == DecisionStepState::Blocked {
                                cx.theme().danger
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(step.summary.clone()),
                    )
            }))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .pt_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(outcome_color)
                            .child(format!("当前结论：{}", trace.outcome.label())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(trace.current_activity()),
                    ),
            )
    }

    pub(super) fn render_decision_card_summary(
        &self,
        card: &DecisionCard,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let status_color = match card.status {
            DecisionStatus::MatchesStrategy => cx.theme().success,
            DecisionStatus::Waiting => cx.theme().warning,
            DecisionStatus::NotEligible => cx.theme().danger,
            DecisionStatus::InsufficientEvidence => cx.theme().muted_foreground,
        };
        let score = card
            .score
            .map(|value| format!(" · 策略匹配度 {:.0}", value))
            .unwrap_or_default();
        let observation = card.observation.clone().unwrap_or_else(|| "—".into());
        let invalidation = card.invalidation.clone().unwrap_or_else(|| "未定义".into());
        let target = card.target.clone().unwrap_or_else(|| "证据不足".into());
        let position_plan = self.position_sizing_plan(cx);
        let climate = self.market_climate_report();
        let currency = crate::domain::money::Currency::for_code(self.selected.as_ref())
            .unwrap_or(crate::domain::money::Currency::Cny);
        let is_star_market = self.selected.starts_with("688") || self.selected.starts_with("689");
        let can_prefill = card.status == DecisionStatus::MatchesStrategy
            && position_plan.is_ok()
            && climate.stance != NewEntryStance::Freeze;

        v_flex()
            .w_full()
            .gap_1p5()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(status_color.opacity(0.35))
            .bg(status_color.opacity(0.06))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(status_color)
                            .child(format!("{}{}", card.status.label(), score)),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("数据完整度 {:.0}%", card.completeness_pct)),
                    ),
            )
            .when(!card.quality_evidence.is_empty(), |panel| {
                panel.child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child("基本面证据（point-in-time）"),
                        )
                        .children(card.quality_evidence.iter().map(|evidence| {
                            let value = if evidence.unit == "bool" {
                                evidence.value.clone()
                            } else {
                                format!("{}{}", evidence.value, evidence.unit)
                            };
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "{} {} · 报告期 {} · 公告 {} · {}",
                                    evidence.label,
                                    value,
                                    evidence.reporting_period,
                                    evidence.announced_on,
                                    evidence.source
                                ))
                        })),
                )
            })
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("支持因素（最多 3 条）"),
                            )
                            .children(card.supports.iter().map(|reason| {
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("• {reason}"))
                            })),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("风险（最多 2 条）"),
                            )
                            .when(card.risks.is_empty(), |column| {
                                column.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("• 暂无明确风险项；仍需遵守失效条件"),
                                )
                            })
                            .children(card.risks.iter().map(|risk| {
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().danger)
                                    .child(format!("• {risk}"))
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(metric_chip("参考观察区间", &observation, cx))
                    .child(metric_chip("失效条件", &invalidation, cx))
                    .child(metric_chip("目标区间", &target, cx))
                    .when_some(card.risk_reward, |row, ratio| {
                        row.child(metric_chip("风险收益比", &format!("{ratio:.2}"), cx))
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background.opacity(0.45))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("执行纪律 · 先定亏多少，再定买多少"),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("单票仓位上限 20%"),
                            ),
                    )
                    .when_some(climate.sizing_note(), |column, note| {
                        column.child(
                            div()
                                .text_xs()
                                .text_color(if climate.stance == NewEntryStance::Freeze {
                                    cx.theme().danger
                                } else {
                                    cx.theme().warning
                                })
                                .child(note),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("计划本金（{}）", currency.symbol())),
                            )
                            .child(
                                div()
                                    .w(px(120.0))
                                    .child(Input::new(&self.position_capital_input).small()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("单笔亏损上限"),
                            )
                            .child(
                                div()
                                    .w(px(72.0))
                                    .child(Input::new(&self.position_risk_pct_input).small()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("%"),
                            ),
                    )
                    .child(match position_plan {
                        Ok(plan) => h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .child(metric_chip("最多新买", &format!("{} 股", plan.shares), cx))
                            .child(metric_chip(
                                "加仓后持股",
                                &format!("{} 股", plan.resulting_shares),
                                cx,
                            ))
                            .child(metric_chip(
                                "计划金额",
                                &format!("{:.2} {}", plan.planned_notional, currency.symbol()),
                                cx,
                            ))
                            .child(metric_chip(
                                "失效损失",
                                &format!(
                                    "约 {:.2} / 预算 {:.2}",
                                    plan.planned_loss, plan.loss_budget
                                ),
                                cx,
                            ))
                            .child(metric_chip(
                                "实际仓位",
                                &format!("{:.1}%", plan.capital_pct),
                                cx,
                            ))
                            .when_some(plan.risk_reward, |row, ratio| {
                                row.child(metric_chip("计划盈亏比", &format!("{ratio:.2}"), cx))
                            })
                            .child(metric_chip("约束来源", plan.binding_constraint.label(), cx))
                            .into_any_element(),
                        Err(error) => div()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(error.user_message())
                            .into_any_element(),
                    })
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_wrap()
                            .child(
                                Button::new("decision-prefill-sized-buy")
                                    .xsmall()
                                    .primary()
                                    .disabled(!can_prefill)
                                    .label("预填买入记录")
                                    .tooltip(if can_prefill {
                                        "只预填本地持仓记录，不会连接券商或自动下单"
                                    } else if climate.stance == NewEntryStance::Freeze {
                                        "今日市场观望，不预填买入"
                                    } else {
                                        "仅当决策卡为“符合策略”且仓位计算有效时开放"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.prefill_position_sized_buy(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("decision-three-leg-alert")
                                    .xsmall()
                                    .ghost()
                                    .label("设三道提醒")
                                    .tooltip("同时设置观察价、失效价和目标价提醒")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.install_execution_alerts(cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if is_star_market {
                                        "科创板首次买入至少 200 股，之后按 1 股取整；跳空可能放大损失"
                                    } else if currency == crate::domain::money::Currency::Cny {
                                        "A 股按 100 股向下取整；跳空和滑点可能使实际损失更大"
                                    } else {
                                        "港股按股数估算；下单前请核对每手股数，跳空可能放大损失"
                                    }),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("decision-watch")
                            .xsmall()
                            .ghost()
                            .label("加入观察")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                let code = this.selected.to_string();
                                let (name, last) = this
                                    .current_symbol()
                                    .map(|symbol| (symbol.name.to_string(), symbol.last))
                                    .unwrap_or_else(|| (code.clone(), 0.0));
                                this.ensure_in_watchlist(&code, &name, last);
                                this.persist();
                                this.status = crate::model::shared("已加入观察");
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("decision-plan")
                            .xsmall()
                            .primary()
                            .label("记录计划")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.record_decision_plan_from_card(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "截至 {} · {} · {} · n={} · {} · {}",
                                card.data_as_of,
                                card.source,
                                card.adjustment,
                                card.sample_size,
                                card.strategy_version,
                                card.evidence_grade
                            )),
                    ),
            )
    }
}
