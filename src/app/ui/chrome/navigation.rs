use gpui::{Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt, TitleBar,
    button::{Button, ButtonVariants},
    h_flex,
};

use crate::storage::{ColorScheme, WorkDensity};

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
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(if work { "Workspace" } else { "Stock" }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "local" } else { "A股分析" }),
                        )
                        .when(!work, |row| {
                            row.child(
                                div()
                                    .text_xs()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.data_source.clone()),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            Button::new("work-mode")
                                .xsmall()
                                .when(work, |b| b.primary())
                                .when(!work, |b| b.ghost())
                                .label(if work { "Focus" } else { "工作" })
                                .tooltip(if work {
                                    "Exit focus layout · ⌘⇧W"
                                } else {
                                    "工作模式：中性配色与文案 · ⌘⇧W"
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_work_mode(window, cx);
                                })),
                        )
                        .when(!work, |row| {
                            row.child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("涨跌色"),
                                    )
                                    .children([ColorScheme::Cn, ColorScheme::Us].map(|scheme| {
                                        let active = self.color_scheme == scheme;
                                        let id = match scheme {
                                            ColorScheme::Cn => "color-scheme-cn",
                                            ColorScheme::Us => "color-scheme-us",
                                        };
                                        Button::new(id)
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(scheme.short_label())
                                            .tooltip(scheme.label())
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.set_color_scheme(scheme, cx);
                                            }))
                                    })),
                            )
                        })
                        .child(
                            Button::new("refresh")
                                .ghost()
                                .xsmall()
                                .label(if work { "Sync" } else { "刷新" })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.refresh_all(cx);
                                })),
                        )
                        .when(!work, |row| {
                            row.children([
                                ("task-today", "今日", PrimaryTask::Today),
                                ("task-research", "研究", PrimaryTask::Research),
                                ("task-opportunities", "机会", PrimaryTask::Opportunities),
                                ("task-portfolio", "组合", PrimaryTask::Portfolio),
                                ("task-strategy-lab", "策略实验室", PrimaryTask::StrategyLab),
                            ]
                            .map(|(id, label, task)| {
                                let active = self.ui_state.primary_task == task;
                                Button::new(id)
                                    .xsmall()
                                    .when(active, |button| button.primary())
                                    .when(!active, |button| button.ghost())
                                    .label(label)
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.set_primary_task(task, cx);
                                    }))
                            }))
                        })
                        .child(
                            Button::new("cmd-palette-btn")
                                .ghost()
                                .xsmall()
                                .label(if work { "Find" } else { "⌘K 搜索" })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_palette(window, cx);
                                })),
                        )
                        .when(!work, |row| row.children(self.render_update_button(cx)))
                        .child(
                            Button::new("settings-btn")
                                .ghost()
                                .xsmall()
                                .when(self.settings_open, |b| b.primary())
                                .label(if self.settings_open {
                                    if work {
                                        "Back"
                                    } else {
                                        "返回"
                                    }
                                } else if work {
                                    "Prefs"
                                } else {
                                    "设置"
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
