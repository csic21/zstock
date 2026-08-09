use gpui::{Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px};
use gpui_component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::app::helpers::*;
use crate::app::{AiPanelState, StockApp};

impl StockApp {
    pub(crate) fn render_ai_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let current = self.ai_current_key();
        let shown = current.is_some() && self.ai_key.as_deref() == current.as_deref();
        let loading = matches!(&self.ai_panel, AiPanelState::Loading { .. });
        // 只有「正在分析当前标的」时才禁用按钮；其他标的可并行触发。
        let busy = shown && loading;
        let has_signal = self.current_signal().is_some();

        let mut col = v_flex().gap_2().w_full().max_w(px(720.)).child(
            h_flex()
                .items_center()
                .justify_between()
                .child(section_title(if work { "AI Brief" } else { "AI 点评" }, cx))
                .child(
                    Button::new("ai-request-btn")
                        .xsmall()
                        .when(!busy && has_signal, |b| b.primary())
                        .when(busy || !has_signal, |b| b.ghost())
                        .label(if busy {
                            if work { "Working…" } else { "分析中…" }
                        } else if work {
                            "Generate"
                        } else {
                            "生成点评"
                        })
                        .disabled(busy || !has_signal)
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.request_ai_commentary(cx);
                        })),
                ),
        );

        if shown {
            match &self.ai_panel {
                AiPanelState::Loading { text } => {
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(text.clone()),
                    );
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work {
                                "LLM brief in progress…"
                            } else {
                                "正在请求 LLM 点评…"
                            }),
                    );
                }
                AiPanelState::Ready { text, source, note } => {
                    let source_color = if source.is_llm() {
                        cx.theme().accent
                    } else {
                        cx.theme().muted_foreground
                    };
                    col = col.child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "Source" } else { "来源" }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(source_color)
                                    .child(source.label(work)),
                            ),
                    );
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(text.clone()),
                    );
                    if let Some(note) = note {
                        col = col.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.9))
                                .child(note.clone()),
                        );
                    }
                }
                AiPanelState::Idle => {
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work {
                                "Not generated."
                            } else {
                                "尚未生成。"
                            }),
                    );
                }
            }
        } else if !has_signal {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("至少需要 20 根有效日K数据。"),
            );
        } else {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "Click Generate for an AI brief."
                    } else {
                        "点击「生成点评」查看 AI 分析。"
                    }),
            );
        }

        col.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground.opacity(0.75))
                .child(if work {
                    "For reference only, not investment advice."
                } else {
                    "仅供学习研究，不构成投资建议。"
                }),
        )
    }
}
