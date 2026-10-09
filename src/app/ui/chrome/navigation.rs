use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, IconName, Sizable, StyledExt, TitleBar,
    button::{Button, ButtonVariant, ButtonVariants},
    h_flex,
};

use crate::storage::WorkDensity;

use crate::app::StockApp;
use crate::app::state::PrimaryTask;

/// Keep toolbar hit targets readable without increasing the native title bar.
fn toolbar_button(id: &'static str) -> Button {
    Button::new(id)
        .small()
        .h(px(28.))
        .min_w(px(28.))
        .px_2()
        .rounded(px(6.))
}

impl StockApp {
    pub(crate) fn render_title_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let work = self.work_mode;
        let compact = window.bounds().size.width < px(960.);
        let show_task_selection = !self.settings_open && !self.market_analysis_open;
        TitleBar::new().child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_2()
                .px_2()
                .debug_selector(|| "app-toolbar".into())
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when(work, |row| {
                            row.child(
                                toolbar_button("work-identity-map")
                                    .ghost()
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
                                toolbar_button("work-alias-tag")
                                    .ghost()
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
                                toolbar_button("work-density")
                                    .ghost()
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
                        .when(!compact, |row| row.child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(if work { "Workspace" } else { "ZStock" }),
                        ))
                        .when(!work && !compact, |row| {
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
                            .flex_shrink_0()
                            .debug_selector(|| "primary-task-tabs".into())
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
                                let active = show_task_selection && self.ui_state.primary_task == task;
                                let shortcut = if cfg!(target_os = "macos") {
                                    format!("⌘{digit}")
                                } else {
                                    format!("Ctrl+{digit}")
                                };
                                toolbar_button(id)
                                    .when(active, |button| button.primary())
                                    .when(!active, |button| button.ghost())
                                    .label(label)
                                    .debug_selector(move || id.into())
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
                        .flex_shrink_0()
                        .debug_selector(|| "toolbar-actions".into())
                        .child(
                            toolbar_button("work-mode")
                                .icon(IconName::Eye)
                                .when(work, |b| b.primary())
                                .when(!work, |b| b.ghost())
                                .label(if work { "Focus" } else { "专注" })
                                .debug_selector(|| "toolbar-focus".into())
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
                            toolbar_button("refresh")
                                .icon(IconName::Redo2)
                                .ghost()
                                .label(if work { "Sync" } else { "刷新" })
                                .debug_selector(|| "toolbar-refresh".into())
                                .tooltip(if work { "Sync" } else { "刷新全部行情" })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.refresh_all(cx);
                                })),
                        )
                        .when(!work, |row| {
                            let active = show_task_selection && self.ui_state.primary_task == PrimaryTask::StrategyLab;
                            row.child(
                                toolbar_button("task-strategy-lab")
                                    .icon(IconName::SquareTerminal)
                                    .ghost()
                                    .when(active, |button| button.primary())
                                    .label("验证")
                                    .debug_selector(|| "toolbar-strategy-lab".into())
                                    .tooltip("策略实验室 · 回测、验证与样本外观察")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.set_primary_task(PrimaryTask::StrategyLab, cx);
                                    })),
                            )
                        })
                        .child(
                            toolbar_button("cmd-palette-btn")
                                .icon(IconName::Search)
                                .ghost()
                                .label(if work { "Find" } else { "搜索" })
                                .debug_selector(|| "toolbar-search".into())
                                .tooltip(if work { "Find" } else { "搜索股票或跳转功能 · ⌘K" })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_palette(window, cx);
                                })),
                        )
                        .when(!work, |row| row.children(self.render_update_button(cx).map(|button| button.small().h(px(28.)))))
                        .child(
                            toolbar_button("settings-btn")
                                .icon(if self.settings_open { IconName::ArrowLeft } else { IconName::Settings2 })
                                .ghost()
                                .when(self.settings_open, |b| b.with_variant(ButtonVariant::Secondary))
                                .label(match (self.settings_open, work) {
                                    (true, true) => "Back",
                                    (true, false) => "返回",
                                    (false, true) => "Prefs",
                                    (false, false) => "设置",
                                })
                                .debug_selector(|| "toolbar-settings".into())
                                .tooltip(if self.settings_open {
                                    if work { "Return to previous page · Esc" } else { "返回上一页 · Esc" }
                                } else if work {
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
