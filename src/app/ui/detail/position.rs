use gpui::{Context, IntoElement, ParentElement, Styled, div, px};
use gpui_component::{ActiveTheme, StyledExt, h_flex, v_flex};

use crate::app::StockApp;
use crate::app::helpers::*;
use crate::domain::position_review::{
    DimensionTone, PositionReview, PositionReviewStance, ReviewDimension,
};
use crate::model::{format_pct, format_price};

impl StockApp {
    pub(super) fn render_position_review(
        &self,
        review: &PositionReview,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let work = self.work_mode;
        let stance_color = match review.stance {
            PositionReviewStance::Protect => cx.theme().danger,
            PositionReviewStance::Hold => cx.theme().accent,
            PositionReviewStance::ReduceWatch => cx.theme().warning,
            PositionReviewStance::AddWatch => cx.theme().success,
        };
        let vs_cost = if review.last > 0.0 {
            format!(
                "{} · 成本 {} / 现价 {}",
                format_pct(review.price_vs_cost_pct),
                format_price(review.cost),
                format_price(review.last)
            )
        } else {
            format!("成本 {}", format_price(review.cost))
        };
        let atr = review
            .atr_from_cost
            .map(|value| format!(" · 距成本 {value:+.1}×ATR"))
            .unwrap_or_default();
        let cards = review.dimensions.clone();

        v_flex()
            .w_full()
            .gap_1p5()
            .p_3()
            .rounded(cx.theme().radius_lg)
            .border_1()
            .border_color(stance_color.opacity(0.45))
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
                            .child(if work {
                                "Position review"
                            } else {
                                "持仓多维分析"
                            }),
                    )
                    .child(status_pill(
                        review.stance.label(work),
                        stance_color,
                        stance_color.opacity(0.16),
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work {
                                "Local rules · no order"
                            } else {
                                "本地规则 · 不会自动下单"
                            }),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(review.headline.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{vs_cost}{atr}")),
            )
            .child(
                h_flex().w_full().gap_1p5().flex_wrap().children(
                    cards
                        .into_iter()
                        .map(|item| review_dimension_card(item, work, cx)),
                ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "Study only. Not investment advice."
                    } else {
                        "仅供学习研究，不构成任何投资建议。"
                    }),
            )
    }
}

fn review_dimension_card(
    item: ReviewDimension,
    work: bool,
    cx: &Context<StockApp>,
) -> impl IntoElement {
    let color = match item.tone {
        DimensionTone::Support => cx.theme().success,
        DimensionTone::Neutral => cx.theme().muted_foreground,
        DimensionTone::Caution => cx.theme().warning,
        DimensionTone::Blocked => cx.theme().danger,
        DimensionTone::Unknown => cx.theme().muted_foreground,
    };
    v_flex()
        .w(px(228.))
        .min_w(px(200.))
        .flex_1()
        .gap_0p5()
        .px_2()
        .py_1p5()
        .rounded(cx.theme().radius)
        .bg(cx.theme().background)
        .border_1()
        .border_color(color.opacity(0.35))
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(if work { item.work_title } else { item.title }),
                )
                .child(status_pill_sized(
                    item.tone.label(work),
                    color,
                    color.opacity(0.14),
                    Some(48.0),
                )),
        )
        .child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(if item.tone == DimensionTone::Blocked {
                    cx.theme().danger
                } else {
                    cx.theme().foreground
                })
                .child(item.headline),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(item.detail),
        )
}
