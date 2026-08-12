use gpui::{Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px};
use gpui_component::{
    ActiveTheme, IconName, Sizable, StyledExt, TitleBar,
    button::{Button, ButtonVariants},
    h_flex,
};

use crate::storage::WorkDensity;

use crate::app::StockApp;
use crate::app::state::PrimaryTask;

impl StockApp {
    pub(crate) fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        TitleBar::new().child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .px_3()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when(work, |row| {
                            row.child(
                                Button::new("work-identity-map")
                                    .ghost()
                                    .xsmall()
                                    .when(self.work_identity_map_latched, |b| b.primary())
                                    .when(!self.work_identity_map_latched, |b| b.ghost())
                                    .label(if self.work_identity_map_latched {
                                        "Hide"
                                    } else {
                                        "Map"
                                    })
                                    .tooltip(if self.work_identity_map_latched {
                                        "Hide identity · auto-hides in ~6s · hold ` or Space to peek"
                                    } else {
                                        "Latch identity map (~6s) · hold ` or Space to peek"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.toggle_work_identity(cx);
                                    })),
                            )
                            .child(
                                Button::new("work-alias-tag")
                                    .ghost()
                                    .xsmall()
                                    .when(self.work_alias_editing, |b| b.primary())
                                    .label(if self.work_alias_editing { "Save" } else { "Tag" })
                                    .tooltip(if self.work_alias_editing {
                                        "Save private service tag · Enter · Esc cancel"
                                    } else {
                                        "Set a private nickname for the selected service"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if this.work_alias_editing {
                                            this.commit_work_alias(window, cx);
                                        } else {
                                            this.start_work_alias_edit(window, cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("work-density")
                                    .ghost()
                                    .xsmall()
                                    .when(self.work_density != WorkDensity::Wide, |b| b.primary())
                                    .label(self.work_density.label())
                                    .tooltip(self.work_density.tooltip())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.cycle_work_density(window, cx);
                                    })),
                            )
                        })
                        .when(!work, |row| {
                            row.child(
                                div()
                                    .size(px(8.))
                                    .rounded_full()
                                    .bg(cx.theme().accent),
                            )
                        })
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(if work { "Workspace" } else { "ZStock" }),
                        )
                        .when(!work, |row| {
                            row.child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        div()
                                            .size(px(5.))
                                            .rounded_full()
                                            .bg(if self.loading {
                                                cx.theme().warning
                                            } else {
                                                cx.theme().success
                                            }),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(92.))
                                            .truncate()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(self.data_source.clone()),
                                    ),
                            )
                        }),
                )
                .when(!work, |bar| {
                    bar.child(
                        h_flex()
                            .p_0p5()
                            .gap_0p5()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border.opacity(0.65))
                            .bg(cx.theme().background.opacity(0.72))
                            .children([
                                ("task-today", "今日", "1", PrimaryTask::Today),
                                (
                                    "task-research",
                                    "研究",
                                    "2",
                                    PrimaryTask::Research,
                                ),
                                (
                                    "task-opportunities",
                                    "机会",
                                    "3",
                                    PrimaryTask::Opportunities,
                                ),
                                (
                                    "task-portfolio",
                                    "组合",
                                    "4",
                                    PrimaryTask::Portfolio,
                                ),
                            ]
                            .map(|(id, label, digit, task)| {
                                let active = self.ui_state.primary_task == task;
                                let shortcut = if cfg!(target_os = "macos") {
                                    format!("⌘{digit}")
                                } else {
                                    format!("Ctrl+{digit}")
                                };
                                Button::new(id)
                                    .xsmall()
                                    .when(active, |button| button.primary())
                                    .when(!active, |button| button.ghost())
                                    .label(label)
                                    .tooltip(format!("{label} · {shortcut}"))
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.set_primary_task(task, cx);
                                    }))
                            })),
                    )
                })
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(
                            Button::new("work-mode")
                                .icon(IconName::Eye)
                                .xsmall()
                                .when(work, |b| b.primary())
                                .when(!work, |b| b.ghost())
                                .when(work, |b| b.label("Focus"))
                                .when(!work, |b| b.label("专注"))
                                .tooltip(if work {
                                    "Exit focus layout · ⌘⇧W"
                                } else {
                                    "专注模式：隐藏股票身份并切换中性文案 · ⌘⇧W"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_work_mode(window, cx);
                                })),
                        )
                        .child(
                            Button::new("refresh")
                                .icon(IconName::Redo2)
                                .ghost()
                                .xsmall()
                                .when(work, |b| b.label("Sync"))
                                .when(!work, |b| b.label("刷新"))
                                .tooltip(if work { "Sync" } else { "刷新全部行情" })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.refresh_all(cx);
                                })),
                        )
                        .when(!work, |row| {
                            let active = self.ui_state.primary_task == PrimaryTask::StrategyLab;
                            row.child(
                                Button::new("task-strategy-lab")
                                    .icon(IconName::SquareTerminal)
                                    .ghost()
                                    .xsmall()
                                    .when(active, |button| button.primary())
                                    .label("验证")
                                    .tooltip("策略实验室 · 回测、验证与样本外观察")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.set_primary_task(PrimaryTask::StrategyLab, cx);
                                    })),
                            )
                        })
                        .child(
                            Button::new("cmd-palette-btn")
                                .icon(IconName::Search)
                                .ghost()
                                .xsmall()
                                .label(if work { "Find" } else { "搜索" })
                                .tooltip(if work { "Find" } else { "搜索股票或跳转功能 · ⌘K" })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_palette(window, cx);
                                })),
                        )
                        .when(!work, |row| row.children(self.render_update_button(cx)))
                        .child(
                            // When open, drop the gear icon so 「返回」is truly
                            // centered in the primary pill (icon+label was
                            // optically left-heavy).
                            Button::new("settings-btn")
                                .when(!self.settings_open, |b| b.icon(IconName::Settings2))
                                .ghost()
                                .xsmall()
                                .when(self.settings_open, |b| b.primary())
                                .when(self.settings_open, |b| {
                                    b.label(if work { "Back" } else { "返回" })
                                })
                                .when(!self.settings_open, |b| {
                                    b.label(if work { "Prefs" } else { "设置" })
                                })
                                .tooltip(if work {
                                    "Preferences · ⌘,"
                                } else {
                                    "设置 · ⌘,"
                                })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.toggle_settings(cx);
                                })),
                        ),
                ),
        )
    }
}
