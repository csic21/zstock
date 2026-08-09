use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    Window, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    v_flex,
};

use crate::data::ai::{AiCliProvider, AiKind, AiTransport};
use crate::storage::{ColorScheme, WorkDensity};
use crate::update::UpdateState;

use crate::app::{QUOTE_INTERVAL_PRESETS, SettingsSection, StockApp};

impl StockApp {
    pub(crate) fn render_settings(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    pub(crate) fn render_settings_general(
        &self,
        work: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                                "Full-page metrics dashboard with neutral chrome. Looks like a service monitor; quotes stay readable under the skin."
                            } else {
                                "整页服务监控台 + 中性文案。外人看像运维面板，你自己仍能读行情。"
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
                    )
                    .when(work, |col| {
                        col.child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground.opacity(0.9))
                                        .child(
                                            "Window size · layout density (also in title bar): Wide → Fit → Mini. Drag the split between service list and host panel.",
                                        ),
                                )
                                .child(
                                    h_flex().gap_1().children(WorkDensity::all().map(|d| {
                                        let active = self.work_density == d;
                                        Button::new(("set-work-density", d as u32))
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(d.label())
                                            .tooltip(d.tooltip())
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.set_work_density(d, window, cx);
                                            }))
                                    })),
                                ),
                        )
                    })
                    .child(self.render_work_mode_help(work, cx)),
            )
    }

    /// Full keyboard map + Focus/work-mode field legend (settings help panel).
    fn render_work_mode_help(&self, work: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;
        let fg = cx.theme().foreground;
        let bg = cx.theme().muted.opacity(0.35);

        // Global app shortcuts (mirror README / bind_keys).
        let app_shortcuts: &[(&str, &str, &str)] = if work {
            &[
                (
                    "⌘K / Ctrl+K",
                    "Command palette",
                    "Search or add symbols · ↑↓ · Enter",
                ),
                ("⌘P / Ctrl+P", "Command palette", "Same as ⌘K"),
                (
                    "⌘, / Ctrl+,",
                    "Settings",
                    "This page (interval · colors · Focus)",
                ),
                ("⌘R / Ctrl+R", "Refresh", "Quotes + current series"),
                (
                    "⌘T / Ctrl+T",
                    "Treasure tab",
                    "Toggle watchlist / treasure (hidden in Focus)",
                ),
                (
                    "⌘⇧W / Ctrl+Shift+W",
                    "Focus layout",
                    "Service monitor skin · window title Notes",
                ),
                (
                    "↑ / ↓  or  k / j",
                    "Prev / next symbol",
                    "Watchlist navigation (disabled while typing)",
                ),
                (
                    "Backspace / Delete",
                    "Remove symbol",
                    "Drop selected from watchlist (min 1)",
                ),
                (
                    "0  or  double-click chart",
                    "Reset zoom",
                    "Restore full candle window",
                ),
                (
                    "Esc",
                    "Dismiss overlay",
                    "Close palette · settings · Tag edit · draw mode",
                ),
                ("⌘Q / Alt+F4", "Quit", "Exit the app"),
            ]
        } else {
            &[
                (
                    "⌘K / Ctrl+K",
                    "命令面板",
                    "搜索 / 添加自选 · ↑↓ 选择 · Enter 确认",
                ),
                ("⌘P / Ctrl+P", "命令面板", "与 ⌘K 相同"),
                (
                    "⌘, / Ctrl+,",
                    "设置",
                    "本页（刷新间隔 · 涨跌色 · 工作模式）",
                ),
                ("⌘R / Ctrl+R", "刷新", "行情 + 当前 K 线 / 分时"),
                (
                    "⌘T / Ctrl+T",
                    "机会",
                    "左侧在「研究 / 机会」间切换（工作模式下无效）",
                ),
                (
                    "⌘⇧W / Ctrl+Shift+W",
                    "工作模式",
                    "服务监控台皮肤 · 窗口标题 Notes",
                ),
                (
                    "↑ / ↓  或  k / j",
                    "上一只 / 下一只",
                    "自选切换（输入框聚焦时不触发）",
                ),
                (
                    "Backspace / Delete",
                    "删除自选",
                    "移除当前选中（至少保留 1 只）",
                ),
                ("0  或  图表双击", "重置缩放", "K 线缩放/平移恢复全览"),
                ("Esc", "关闭浮层", "命令面板 · 设置 · Tag 编辑 · 画线模式"),
                ("⌘Q / Alt+F4", "退出", "退出应用"),
            ]
        };

        let focus_shortcuts: &[(&str, &str, &str)] = if work {
            &[
                (
                    "` or Space (hold)",
                    "Peek identity",
                    "Show real names while held; release to cloak",
                ),
                (
                    "Map / Hide",
                    "Latch identity ~6s",
                    "Title-bar; auto-hides so Map is not left open",
                ),
                (
                    "Tag",
                    "Private nickname",
                    "Name the selected service; empty + Save clears; local config",
                ),
                (
                    "Wide / Fit / Mini",
                    "Window size",
                    "Cycle density + OS window; Mini is ~720×440; drag the host split",
                ),
                ("Find", "Command palette", "Same as ⌘K under Focus chrome"),
                ("Sync", "Refresh", "Same as ⌘R"),
            ]
        } else {
            &[
                (
                    "` 或 Space（按住）",
                    "窥视真身份",
                    "按住显示代码/名称，松手立刻恢复伪装",
                ),
                (
                    "Map / Hide",
                    "锁定约 6 秒",
                    "标题栏按钮；超时自动 Hide，避免忘记关掉",
                ),
                (
                    "Tag",
                    "私有服务昵称",
                    "给当前选中起助记名；清空保存即删除；写入本地 config",
                ),
                (
                    "Wide / Fit / Mini",
                    "窗口大小",
                    "循环压缩布局与窗口；Mini 约 720×440；可拖右侧分栏",
                ),
                ("Find", "命令面板", "工作模式标题栏，等同 ⌘K"),
                ("Sync", "刷新", "工作模式标题栏，等同 ⌘R"),
            ]
        };

        let fields: &[(&str, &str)] = if work {
            &[
                ("service", "Stable alias or your Tag (not the ticker)"),
                ("p50", "Last price (shown as latency ms)"),
                ("drift", "Day change %"),
                ("load", "Relative volume vs busiest row"),
                ("health", "Strategy score + state (optimal / degraded…)"),
                ("cpu / mem / disk", "SSE / CSI 300 / ChiNext index points"),
                ("process cpu / rss", "Abs change heat / volume-sized memory"),
                ("window title", "Notes"),
            ]
        } else {
            &[
                ("service", "稳定伪装名，或你设的 Tag（不是股票代码）"),
                ("p50", "现价（伪装成延迟 ms）"),
                ("drift", "涨跌幅 %"),
                ("load", "相对成交量（相对最活跃那只）"),
                ("health", "策略评分 + 状态（optimal / degraded…）"),
                ("cpu / mem / disk", "上证 / 沪深300 / 创业板点位"),
                ("process cpu / rss", "波动热度 / 量能伪装的内存占用"),
                ("窗口标题", "Notes"),
            ]
        };

        let tips: &[&str] = if work {
            &[
                "Plain keys (↑↓ j k 0 Backspace) yield to focused text fields.",
                "Default Focus view never shows stock codes or Chinese quote jargon.",
                "Prefer hold-to-peek over leaving Map latched; hover still shows a temporary tip.",
                "Tags are private local mnemonics; unset rows fall back to the hash alias.",
            ]
        } else {
            &[
                "纯按键（↑↓ j k 0 Backspace）在输入框聚焦时让位，不会误删/切股。",
                "工作模式默认不出现股票代码与行情黑话。",
                "日常更推荐按住窥视，而不是长期开着 Map；悬停行仍可短暂看到真身份。",
                "Tag 只存在本机；未设置时仍用 hash 伪装名。",
            ]
        };

        let row = |key: &'static str, title: &'static str, desc: &'static str| {
            h_flex()
                .w_full()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .min_w(px(168.))
                        .max_w(px(200.))
                        .text_xs()
                        .font_semibold()
                        .text_color(fg)
                        .child(key),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_xs().text_color(fg).child(title))
                        .child(div().text_xs().text_color(muted.opacity(0.9)).child(desc)),
                )
        };

        v_flex()
            .mt_1()
            .gap_3()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(border)
            .bg(bg)
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(fg)
                    .child(if work {
                        "Shortcuts & features"
                    } else {
                        "快捷键与功能说明"
                    }),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(muted)
                            .child(if work {
                                "App shortcuts"
                            } else {
                                "全局快捷键"
                            }),
                    )
                    .children(
                        app_shortcuts
                            .iter()
                            .map(|(key, title, desc)| row(key, title, desc)),
                    ),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(muted)
                            .child(if work {
                                "Focus layout"
                            } else {
                                "工作模式专用"
                            }),
                    )
                    .children(
                        focus_shortcuts
                            .iter()
                            .map(|(key, title, desc)| row(key, title, desc)),
                    ),
            )
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(muted)
                            .child(if work {
                                "Field map (skin → real)"
                            } else {
                                "字段对照（伪装 → 真实）"
                            }),
                    )
                    .children(fields.iter().map(|(field, meaning)| {
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_start()
                            .child(
                                div()
                                    .min_w(px(120.))
                                    .max_w(px(140.))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(fg)
                                    .child(*field),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(muted.opacity(0.95))
                                    .child(*meaning),
                            )
                    })),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(muted)
                            .child(if work { "Tips" } else { "使用提示" }),
                    )
                    .children(tips.iter().map(|line| {
                        div()
                            .text_xs()
                            .text_color(muted.opacity(0.9))
                            .child(format!("· {line}"))
                    })),
            )
    }

    pub(crate) fn render_settings_update(
        &self,
        work: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    pub(crate) fn render_settings_ai(
        &self,
        work: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                        .child(h_flex().gap_1().children(AiKind::all().map(|kind| {
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
                        }))),
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

    pub(crate) fn render_settings_about(
        &self,
        work: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
