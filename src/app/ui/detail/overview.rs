use gpui::{Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::app::StockApp;
use crate::app::helpers::*;
use crate::domain::decision::{DecisionCard, DecisionStatus};

impl StockApp {
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

        v_flex()
            .w_full()
            .gap_1p5()
            .p_2()
            .rounded(cx.theme().radius)
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
                        Button::new("decision-alert")
                            .xsmall()
                            .ghost()
                            .label("创建提醒")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_recommended_buy_alert(window, cx);
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
