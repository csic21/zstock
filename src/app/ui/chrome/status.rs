use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::model::disguise_label;
use crate::storage::STATUS_BAR_MAX_CODES;

use crate::app::StockApp;
use crate::app::helpers::*;

impl StockApp {
    pub(crate) fn render_settings_status_bar(
        &self,
        work: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let enabled = self.status_bar_enabled;
        let pinned = self.status_bar_codes.clone();
        let pin_count = pinned.len();

        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "Menu bar quotes" } else { "菜单栏行情" }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.9))
                    .child(if work {
                        format!(
                            "Pin up to {STATUS_BAR_MAX_CODES} watchlist symbols. All pinned quotes (price + change) show together in the macOS menu bar. Click a row in the dropdown to open that symbol."
                        )
                    } else {
                        format!(
                            "从自选固定最多 {STATUS_BAR_MAX_CODES} 只；菜单栏会同时显示全部固定标的的现价与涨跌（例：比亚迪 98.50 +1.2% · 楚天 …）。点下拉项可打开对应股票。Windows/Linux 暂不支持。"
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("set-statusbar-off")
                            .xsmall()
                            .when(!enabled, |b| b.primary())
                            .when(enabled, |b| b.ghost())
                            .label(if work { "Off" } else { "关闭" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_status_bar_enabled(false, cx);
                            })),
                    )
                    .child(
                        Button::new("set-statusbar-on")
                            .xsmall()
                            .when(enabled, |b| b.primary())
                            .when(!enabled, |b| b.ghost())
                            .label(if work { "On" } else { "开启" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_status_bar_enabled(true, cx);
                            })),
                    ),
            )
            .when(enabled, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if work {
                            format!("Pinned {pin_count}/{STATUS_BAR_MAX_CODES} · click to pin/unpin · all show in menu bar")
                        } else {
                            format!("已固定 {pin_count}/{STATUS_BAR_MAX_CODES} · 点击切换固定 · 全部同时显示在菜单栏")
                        }),
                )
                .child(
                    // Vertical list: name (left) + code (muted) + pin state.
                    // Horizontal wrap chips looked cramped and double-coded ETFs.
                    v_flex()
                        .gap_0()
                        .max_w(px(480.))
                        .border_1()
                        .border_color(cx.theme().border.opacity(0.5))
                        .rounded(px(6.))
                        .overflow_hidden()
                        .children(self.symbols.iter().enumerate().map(|(ix, sym)| {
                            let code = sym.code.clone();
                            let is_pinned = pinned.iter().any(|c| c == &code);
                            let name_raw = sym.name.as_ref();
                            let (name_show, code_show) = if work {
                                (
                                    disguise_label(&sym.code, name_raw),
                                    String::new(),
                                )
                            } else if is_real_name(name_raw, &sym.code) {
                                (
                                    short_status_name(name_raw, &sym.code),
                                    sym.code.clone(),
                                )
                            } else {
                                (sym.code.clone(), String::new())
                            };
                            let pin_hint = if work {
                                if is_pinned { "pinned" } else { "pin" }
                            } else if is_pinned {
                                "已固定"
                            } else {
                                "固定"
                            };
                            let row_id = SharedString::from(format!("sb-pin-{}", sym.code));
                            div()
                                .id(row_id)
                                .w_full()
                                .h(px(32.))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .when(ix > 0, |r| {
                                    r.border_t_1()
                                        .border_color(cx.theme().border.opacity(0.35))
                                })
                                .when(is_pinned, |r| {
                                    r.bg(cx.theme().accent.opacity(0.16))
                                })
                                .hover(|r| r.bg(cx.theme().accent.opacity(0.10)))
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.toggle_status_bar_code(&code, cx);
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .font_semibold()
                                        .text_color(cx.theme().foreground)
                                        .child(name_show),
                                )
                                .when(!code_show.is_empty(), |r| {
                                    r.child(
                                        div()
                                            .text_xs()
                                            .font_family("Menlo")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(code_show),
                                    )
                                })
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(if is_pinned {
                                            cx.theme().accent_foreground
                                        } else {
                                            cx.theme().muted_foreground.opacity(0.8)
                                        })
                                        .child(pin_hint),
                                )
                        })),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child(if work {
                            "Highlighted = pinned (shown in menu bar) · click to toggle."
                        } else {
                            "高亮 = 已固定并显示在菜单栏 · 点击切换。可多选。"
                        }),
                )
            })
    }
}
