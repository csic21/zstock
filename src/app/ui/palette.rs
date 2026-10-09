//! Command palette overlay.

use gpui::{
    Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};
use gpui_component::{ActiveTheme, h_flex, input::Input, v_flex};

use crate::controller::state::RequestState;
use crate::model::Symbol;

use super::super::StockApp;
use super::super::helpers::*;
use super::super::labels::L;

impl StockApp {
    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let local: Vec<(usize, Symbol)> = self
            .filtered_local
            .iter()
            .filter_map(|&i| self.symbols.get(i).cloned().map(|s| (i, s)))
            .collect();
        let remote = self.palette_hits.clone();
        let n_local = local.len();
        let highlight = self.palette_index;
        let work = self.work_mode;
        let search_status = match &self.palette_search.state {
            RequestState::Loading => Some(if work {
                "Searching…".to_string()
            } else {
                "正在搜索…".to_string()
            }),
            RequestState::Failed(message) => Some(if work {
                "Search failed. Edit the query to retry.".to_string()
            } else {
                format!("{message} · 修改关键词重试")
            }),
            _ => None,
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .py(px(40.))
            .px_3()
            .bg(gpui::hsla(0.61, 0.35, 0.035, 0.72))
            // Same modal isolation as the settings overlay: don't let wheel
            // scrolling or hover styles reach the app behind the palette.
            .occlude()
            // Capture ↑↓ while the search input is focused (Input would otherwise
            // eat MoveUp/MoveDown for multi-line caret motion).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _w, cx| {
                let key = event.keystroke.key.as_str();
                match key {
                    "up" => {
                        this.palette_move(-1, cx);
                        cx.stop_propagation();
                    }
                    "down" => {
                        this.palette_move(1, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                v_flex()
                    .id("palette-panel")
                    .debug_selector(|| "palette-panel".into())
                    .key_context("stock_palette")
                    .w_full()
                    .max_w(px(620.))
                    .max_h(gpui::relative(1.))
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().accent.opacity(0.28))
                    .bg(cx.theme().popover)
                    .overflow_hidden()
                    .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
                        this.close_palette(cx);
                    }))
                    .child(
                        h_flex()
                            .h(px(48.))
                            .flex_shrink_0()
                            .px_3()
                            .items_center()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(div().flex_1().child(Input::new(&self.palette_query))),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.45))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work {
                                "Tips · long / short / market · or a code"
                            } else {
                                "快捷：输入「长线」「短线」「市场」回车 · 或搜代码"
                            }),
                    )
                    .children(search_status.map(|message| {
                        div()
                            .px_3()
                            .py_1()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(message)
                    }))
                    .child({
                        let mut list = v_flex()
                            .id("palette-results")
                            .flex_1()
                            .min_h_0()
                            .max_h(px(360.))
                            .track_scroll(&self.palette_scroll)
                            .overflow_y_scroll()
                            .p_1();
                        if !local.is_empty() {
                            list = list.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(L::palette_section_local(work)),
                            );
                            for (i, (_, sym)) in local.into_iter().enumerate() {
                                list = list.child(palette_row(
                                    sym,
                                    PaletteRowOptions {
                                        in_watchlist: true,
                                        row_id: i as u64,
                                        highlighted: highlight == i,
                                        color_scheme: self.color_scheme,
                                        work_mode: self.work_mode,
                                        reveal_identity: self.work_identity_reveal,
                                    },
                                    cx,
                                ));
                            }
                        }
                        if !remote.is_empty() {
                            list = list.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(L::palette_section_remote(work)),
                            );
                            for (i, sym) in remote.into_iter().enumerate() {
                                let flat = n_local + i;
                                list = list.child(palette_row(
                                    sym,
                                    PaletteRowOptions {
                                        in_watchlist: false,
                                        row_id: 10_000 + i as u64,
                                        highlighted: highlight == flat,
                                        color_scheme: self.color_scheme,
                                        work_mode: self.work_mode,
                                        reveal_identity: self.work_identity_reveal,
                                    },
                                    cx,
                                ));
                            }
                        }
                        let empty = self.filtered_local.is_empty() && self.palette_hits.is_empty();
                        let message = match &self.palette_search.state {
                            RequestState::Ready(()) if empty => Some(if work {
                                "No results. Try a different name or valid code.".to_string()
                            } else {
                                "没有匹配结果 · 请更换关键词，或输入有效代码后回车预览".to_string()
                            }),
                            RequestState::Idle if empty => Some(L::palette_empty(work).to_string()),
                            _ => None,
                        };
                        if let Some(message) = message {
                            list = list.child(
                                div()
                                    .p_4()
                                    .flex_shrink_0()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(message),
                            );
                        }
                        list
                    })
                    .child(
                        h_flex()
                            .h(px(28.))
                            .flex_shrink_0()
                            .px_3()
                            .items_center()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(L::palette_footer(work)),
                            ),
                    ),
            )
    }
}
