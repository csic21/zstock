//! Title bar and full-page settings.

#![allow(unused_imports)]

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    canvas, div, point, px, size, App, AppContext, Bounds, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, Timer, Window, WindowBounds, WindowOptions,
    prelude::FluentBuilder,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    IconName,
    input::{Input, InputEvent, InputState},
    resizable::{h_resizable, resizable_panel, v_resizable, ResizableState},
    v_flex, ActiveTheme, Disableable, PixelsExt, Root, Sizable, StyledExt, Theme, ThemeMode,
    TitleBar, TITLE_BAR_HEIGHT,
};
use gpui_component::tooltip::Tooltip;

use crate::chart::{
    chart_layout, index_from_x, paint_chart, paint_sparkline, price_from_y, BollPaintData,
    ChartPaintData, ChartStyle, MacdPaintData, MinutePaintData,
};
use crate::data::ai::{self, AiCliProvider, AiConfig, AiKind, AiTransport};
use crate::data::levels;
use crate::data::portfolio::{
    self, format_money, format_shares, Portfolio, PortfolioSummary, TradeSide,
};
use crate::data::scout::{self, ScoutPick, ScoutVerdict, SCOUT_CANDIDATE_N};
use crate::data::treasure::{self, fmt_dd, fmt_pos, TreasureHit, TREASURE_KLINE_LIMIT};
use crate::data::universe::{self, FinFilter, TreasurePool, TREASURE_SCAN_CAP, TREASURE_TOP_N};
use crate::data::{
    indicators::{BollSeries, MaSeries, MacdSeries},
    market, session, signals,
};
use crate::data::market::Sourced;
use crate::data::session::{filter_codes_in_session, idle_delay_secs, open_markets_now, MarketSet};
use crate::model::{
    board_for_code, disguise_index, disguise_label, format_index, format_pct, format_price,
    format_volume, normalize_code, shared, Candle, IndexSnap, MinutePeriod, MinuteSeries,
    QuoteSnapshot, Symbol, TrendLine,
};
use crate::storage::{
    self, clamp_quote_interval_secs, normalize_status_bar, AppConfig, ColorScheme, DockLayout,
    WatchlistSort, STATUS_BAR_MAX_CODES,
};
use crate::update::{self, UpdateState};

use super::super::{
    AiCacheEntry, AiPanelState, AiSource, ChartKind, ChartRange, DetailTab, LeftTab, SettingsSection,
    StockApp, CHART_MIN_VISIBLE, QUOTE_INTERVAL_ERR_MAX, QUOTE_INTERVAL_PRESETS, TITLE_NORMAL,
    TITLE_WORK, TREASURE_SCAN_GAP,
};
use super::super::helpers::*;



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
                                    .label(if self.work_identity_reveal { "Hide" } else { "Map" })
                                    .tooltip(if self.work_identity_reveal {
                                        "Hide stock identity mapping"
                                    } else {
                                        "Temporarily map services to stock names and codes"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.toggle_work_identity(cx);
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
                            row.child(
                                Button::new("treasure-btn")
                                    .ghost()
                                    .xsmall()
                                    .label("🐭 寻宝")
                                    .tooltip("多窗口历史低位 · ⌘T")
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_left_tab(LeftTab::Treasure, cx);
                                    })),
                            )
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

    /// Full-page settings (replaces the old centered modal).
    pub(crate) fn render_settings(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let section = self.settings_section;
        let _ = window; // height comes from flex layout

        v_flex()
            .id("settings-panel")
            .debug_selector(|| "settings-panel-root".into())
            .size_full()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .h(px(44.))
                    .px_3()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .child(
                        Button::new("settings-back")
                            .ghost()
                            .xsmall()
                            .label(if work { "← Back" } else { "← 返回行情" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.close_settings(cx);
                            })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(if work { "Preferences" } else { "设置" }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work {
                                "Saved locally · Esc to leave"
                            } else {
                                "本地保存 · Esc 返回"
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        v_flex()
                            .w(px(200.))
                            .h_full()
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().sidebar)
                            .p_2()
                            .gap_1()
                            .children(SettingsSection::all().map(|sec| {
                                let on = section == sec;
                                Button::new(("settings-nav", sec as u32))
                                    .ghost()
                                    .when(on, |b| b.primary())
                                    .label(sec.label(work))
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_settings_section(sec, cx);
                                    }))
                            })),
                    )
                    .child(
                        div()
                            .id("settings-body")
                            .flex_1()
                            .min_h_0()
                            .h_full()
                            .overflow_y_scroll()
                            .p_5()
                            .child(match section {
                                SettingsSection::General => {
                                    self.render_settings_general(work, cx).into_any_element()
                                }
                                SettingsSection::StatusBar => {
                                    self.render_settings_status_bar(work, cx).into_any_element()
                                }
                                SettingsSection::Ai => {
                                    self.render_settings_ai(work, cx).into_any_element()
                                }
                                SettingsSection::Update => {
                                    self.render_settings_update(work, cx).into_any_element()
                                }
                                SettingsSection::About => {
                                    self.render_settings_about(work, cx).into_any_element()
                                }
                            }),
                    ),
            )
    }

    pub(crate) fn render_settings_general(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let interval = self.quote_interval_secs;
        let scheme = self.color_scheme;

        v_flex()
            .gap_5()
            .max_w(px(640.))
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "General" } else { "常规" }),
            )
            // Quote interval
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Poll interval" } else { "行情刷新间隔" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .child(if work {
                                "Poll only in session (CN 09:15–15:00 incl. auction; HK 09:00–16:10). Off-hours: once on open."
                            } else {
                                "仅交易时段轮询：A股 09:15–11:30/13:00–15:00（含竞价）；港股 09:00–12:00/13:00–16:10。盘外启动只拉一次。"
                            }),
                    )
                    .child(
                        h_flex().gap_1().flex_wrap().children(
                            QUOTE_INTERVAL_PRESETS.iter().map(|&secs| {
                                let active = interval == secs;
                                Button::new(("qi", secs as u32))
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(format!("{secs}s"))
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_quote_interval_secs(secs, cx);
                                    }))
                            }),
                        ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{}: {interval}s",
                                if work { "Current" } else { "当前" }
                            )),
                    ),
            )
            // Color scheme
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Color scheme" } else { "涨跌配色" }),
                    )
                    .child(
                        h_flex().gap_1().children([ColorScheme::Cn, ColorScheme::Us].map(|s| {
                            let active = scheme == s;
                            let id = match s {
                                ColorScheme::Cn => "set-scheme-cn",
                                ColorScheme::Us => "set-scheme-us",
                            };
                            Button::new(id)
                                .xsmall()
                                .when(active, |b| b.primary())
                                .when(!active, |b| b.ghost())
                                .label(s.label())
                                .on_click(cx.listener(move |this, _, _w, cx| {
                                    this.set_color_scheme(s, cx);
                                }))
                        })),
                    ),
            )
            // Work mode
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Focus layout" } else { "工作模式" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .child(if work {
                                "Metrics dashboard + neutral chrome. Toggle with ⌘⇧W."
                            } else {
                                "服务指标台 + 中性文案。快捷键 ⌘⇧W。"
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("set-work-off")
                                    .xsmall()
                                    .when(!work, |b| b.primary())
                                    .when(work, |b| b.ghost())
                                    .label(if work { "Off" } else { "关闭" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_work_mode(false, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("set-work-on")
                                    .xsmall()
                                    .when(work, |b| b.primary())
                                    .when(!work, |b| b.ghost())
                                    .label(if work { "On" } else { "开启" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_work_mode(true, window, cx);
                                    })),
                            ),
                    ),
            )
    }

    pub(crate) fn render_settings_update(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .max_w(px(640.))
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "Update" } else { "更新" }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.9))
                    .child(self.update_status_line(work)),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("check-update-btn")
                            .xsmall()
                            .ghost()
                            .label(if work { "Check" } else { "检查更新" })
                            .disabled(matches!(
                                self.update_state,
                                UpdateState::Checking | UpdateState::Downloading(_)
                            ))
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.check_for_updates(true, cx);
                            })),
                    )
                    .children(match &self.update_state {
                        UpdateState::Available(_) => Some(
                            Button::new("settings-update-now")
                                .xsmall()
                                .primary()
                                .label(if work { "Update now" } else { "立即更新" })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.start_update(cx);
                                })),
                        ),
                        _ => None,
                    }),
            )
    }

    pub(crate) fn render_settings_ai(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let use_cli = self.ai_config.transport == AiTransport::Cli;
        let status = if self.ai_config.enabled {
            if self.ai_config.is_configured() {
                if work {
                    format!("Enabled · {}", self.ai_config.source_label())
                } else {
                    format!("已开启 · {}", self.ai_config.source_label())
                }
            } else if work {
                "Enabled · missing base URL / model / key.".to_string()
            } else {
                "已开启 · 尚未填全 API 地址 / 模型 / Key。".to_string()
            }
        } else if work {
            "Disabled · local rules only.".to_string()
        } else {
            "未开启 · 仅使用本地点评。".to_string()
        };

        let mut col = v_flex()
            .gap_5()
            .w_full()
            .max_w(px(640.))
            // Page title
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "AI analysis" } else { "AI 分析" }),
            )
            // Enable / disable
            .child(
                v_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Enable" } else { "开关" }),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .child(if work {
                                "Optional LLM brief. Falls back to local rules when off or failed."
                            } else {
                                "可选 LLM 点评；关闭或请求失败时自动使用本地规则。"
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("ai-on")
                                    .xsmall()
                                    .when(self.ai_config.enabled, |b| b.primary())
                                    .when(!self.ai_config.enabled, |b| b.ghost())
                                    .label(if work { "On" } else { "开启" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_ai_enabled(true, cx);
                                    })),
                            )
                            .child(
                                Button::new("ai-off")
                                    .xsmall()
                                    .when(!self.ai_config.enabled, |b| b.primary())
                                    .when(self.ai_config.enabled, |b| b.ghost())
                                    .label(if work { "Off" } else { "关闭" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_ai_enabled(false, cx);
                                    })),
                            ),
                    ),
            )
            // Transport
            .child(
                v_flex()
                    .gap_2()
                    .w_full()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if work { "Transport" } else { "调用方式" }),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .child(if work {
                                "HTTP API or a local CLI already logged in on this machine."
                            } else {
                                "HTTP API，或本机已登录的 CLI（Grok / ChatGPT·Codex / OpenCode / Claude）。"
                            }),
                    )
                    .child(h_flex().gap_1().children(AiTransport::all().map(|t| {
                        let active = self.ai_config.transport == t;
                        let id = match t {
                            AiTransport::Api => "ai-transport-api",
                            AiTransport::Cli => "ai-transport-cli",
                        };
                        Button::new(id)
                            .xsmall()
                            .when(active, |b| b.primary())
                            .when(!active, |b| b.ghost())
                            .label(t.label())
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.set_ai_transport(t, cx);
                            }))
                    }))),
            );

        if use_cli {
            col = col
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "CLI tool" } else { "CLI 工具" }),
                        )
                        .child(
                            h_flex()
                                .gap_1()
                                .flex_wrap()
                                .children(AiCliProvider::all().map(|p| {
                                    let active = self.ai_config.cli_provider == p;
                                    let id = match p {
                                        AiCliProvider::Grok => "ai-cli-grok",
                                        AiCliProvider::Chatgpt => "ai-cli-chatgpt",
                                        AiCliProvider::Opencode => "ai-cli-opencode",
                                        AiCliProvider::Claude => "ai-cli-claude",
                                    };
                                    Button::new(id)
                                        .xsmall()
                                        .when(active, |b| b.primary())
                                        .when(!active, |b| b.ghost())
                                        .label(p.label())
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.set_ai_cli_provider(p, cx);
                                        }))
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "Model (optional)"
                                } else {
                                    "模型（可选）"
                                }),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.9))
                                .child(if work {
                                    "Leave empty to use the CLI default model."
                                } else {
                                    "留空则使用 CLI 默认模型。"
                                }),
                        )
                        .child(Input::new(&self.ai_model_input).small()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "CLI path (optional)"
                                } else {
                                    "CLI 路径（可选）"
                                }),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.9))
                                .child(if work {
                                    "Absolute path if the binary is not on PATH."
                                } else {
                                    "不在 PATH 时填写绝对路径，例如 /opt/homebrew/bin/claude。"
                                }),
                        )
                        .child(Input::new(&self.ai_cli_bin_input).small()),
                )
                .child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child(if work {
                            "Uses your logged-in CLI. Only the metric snapshot is sent as the prompt."
                        } else {
                            "使用本机 CLI 登录态；只把指标快照作为提示词，不上传原始行情。"
                        }),
                );
        } else {
            col = col
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "Protocol" } else { "协议" }),
                        )
                        .child(
                            h_flex().gap_1().children(AiKind::all().map(|kind| {
                                let active = self.ai_config.kind == kind;
                                let id = match kind {
                                    AiKind::Responses => "ai-kind-responses",
                                    AiKind::Chat => "ai-kind-chat",
                                };
                                Button::new(id)
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(kind.label())
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_ai_kind(kind, cx);
                                    }))
                            })),
                        ),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "Base URL" } else { "API 地址" }),
                        )
                        .child(Input::new(&self.ai_base_url_input).small()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "Model" } else { "模型" }),
                        )
                        .child(Input::new(&self.ai_model_input).small()),
                )
                .child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work { "API key" } else { "API Key" }),
                        )
                        .child(Input::new(&self.ai_api_key_input).small().mask_toggle()),
                )
                .child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground.opacity(0.75))
                        .child(if work {
                            "Key stays in local config.json. Only the metric snapshot is sent."
                        } else {
                            "Key 仅保存在本机 config.json；只上传指标快照，不上传原始行情。"
                        }),
                );
        }

        col.child(
            div()
                .w_full()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(status),
        )
    }

    pub(crate) fn render_settings_about(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .max_w(px(640.))
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(if work { "About" } else { "关于" }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.9))
                    .child(format!(
                        "{} v{}",
                        if work { "Version" } else { "版本" },
                        env!("CARGO_PKG_VERSION")
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.9))
                    .child(if work {
                        "Data: Eastmoney & Tencent public endpoints, personal study only."
                    } else {
                        "数据来源：东方财富 / 腾讯财经公开接口，仅供个人学习研究。"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                    .child(if work {
                        "For reference only. Quotes may be delayed or erroneous; no investment advice."
                    } else {
                        "行情可能有延迟或误差，所有指标与评分仅供参考，不构成任何投资建议。"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                    .child(if work {
                        "Prefs are saved locally and apply immediately."
                    } else {
                        "设置会写入本地配置，立即生效。"
                    }),
            )
    }

}
