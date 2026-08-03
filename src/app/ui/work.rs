//! Work-mode dashboard rendering.

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
    pub(crate) fn render_work_dashboard(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let max_vol = self.max_watchlist_volume();
        let spark = self.spark_closes();
        let sel_alias = self.display_code(selected.as_ref());
        let sel_sym = self.current_symbol();
        let p50 = sel_sym
            .filter(|s| s.last > 0.0)
            .map(|s| format!("{}ms", format_price(s.last)))
            .unwrap_or_else(|| "--".into());
        let sel_identity = sel_sym
            .map(|s| format!("{} · {} · 现价 {}", s.code, s.name, format_price(s.last)))
            .unwrap_or_else(|| "identity unavailable".into());
        let delta = sel_sym
            .map(|s| format!("{:+.2}%", s.change_pct))
            .unwrap_or_else(|| "--".into());
        let load = sel_sym
            .map(|s| format!("{:.2}", Self::load_factor(s.volume, max_vol)))
            .unwrap_or_else(|| "--".into());
        let status = self.work_status_line();
        let range_label = self.range.label();
        let pts = if spark.is_empty() {
            self.candles.len()
        } else {
            spark.len()
        };
        let line = cx.theme().blue;
        let fill = cx.theme().blue.opacity(0.16);
        let border = cx.theme().border;
        let signal = self.current_signal();
        let health = signal
            .as_ref()
            .map(|s| format!("{:.0}%", s.score))
            .unwrap_or_else(|| "--".into());
        let service_state = signal
            .as_ref()
            .map(|s| s.regime.service_state())
            .unwrap_or("warming");

        v_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    // ── left: service table ──
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(360.))
                            .min_h_0()
                            .h_full()
                            .overflow_hidden()
                            .child(
                                h_flex()
                                    .h(px(32.))
                                    .flex_shrink_0()
                                    .px_3()
                                    .items_center()
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().sidebar)
                                    .child(
                                        div()
                                            .w(px(210.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("service"),
                                    )
                                    .child(
                                        div()
                                            .id("work-p50-header")
                                            .w(px(90.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("p50")
                                            .tooltip(|window, cx| {
                                                Tooltip::new(
                                                    "owner key · p50=现价 · drift=涨跌幅 · health=策略评分",
                                                )
                                                .build(window, cx)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .w(px(74.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("drift"),
                                    )
                                    .child(
                                        div()
                                            .w(px(58.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("load"),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{} workers", self.symbols.len())),
                                    ),
                            )
                            // Scroll on a plain div (more reliable than v_flex + overflow).
                            .child(
                                div()
                                    .id("work-metrics-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .w_full()
                                    .overflow_y_scroll()
                                    .children(self.symbols.iter().enumerate().map(|(ix, sym)| {
                                        let is_selected = sym.code == selected.as_ref();
                                        let code = shared(sym.code.clone());
                                        let alias = self.display_code(&sym.code);
                                        let p50 = if sym.last > 0.0 {
                                            format!("{}ms", format_price(sym.last))
                                        } else {
                                            "--".into()
                                        };
                                        let delta = format!("{:+.2}%", sym.change_pct);
                                        let load =
                                            format!("{:.2}", Self::load_factor(sym.volume, max_vol));
                                        let identity = format!(
                                            "{} · {} · 现价 {}",
                                            sym.code,
                                            sym.name,
                                            format_price(sym.last)
                                        );

                                        div()
                                            .id(("work-row", ix))
                                            .h(px(34.))
                                            .w_full()
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .flex_shrink_0()
                                            .cursor_pointer()
                                            .border_b_1()
                                            .border_color(cx.theme().border.opacity(0.3))
                                            .when(is_selected, |this| {
                                                this.bg(cx.theme().accent.opacity(0.14))
                                            })
                                            .hover(|this| this.bg(cx.theme().accent.opacity(0.08)))
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(identity.clone()).build(window, cx)
                                            })
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.select_symbol(code.clone(), cx);
                                            }))
                                            .child(
                                                div()
                                                    .w(px(210.))
                                                    .text_sm()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .truncate()
                                                    .child(alias),
                                            )
                                            .child(
                                                div()
                                                    .w(px(90.))
                                                    .text_sm()
                                                    .font_medium()
                                                    .text_color(cx.theme().foreground)
                                                    .child(p50),
                                            )
                                            .child(
                                                div()
                                                    .w(px(74.))
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(delta),
                                            )
                                            .child(
                                                div()
                                                    .w(px(58.))
                                                    .text_sm()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(load),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(if is_selected { "run" } else { "·" }),
                                            )
                                    })),
                            ),
                    )
                    // ── right: host / process panel (real data, system skin) ──
                    .child(self.render_work_system_panel(cx)),
            )
            // footer: selected + sparkline
            .child(
                v_flex()
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .child(
                        h_flex()
                            .h(px(32.))
                            .flex_shrink_0()
                            .px_3()
                            .items_center()
                            .justify_between()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.5))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_baseline()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("selected"),
                                    )
                                    .child(
                                        div()
                                            .id("work-selected-alias")
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(sel_alias)
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(sel_identity.clone()).build(window, cx)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("p50 {p50}")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("Δ {delta}")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("load {load}")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("health {health} · {service_state}")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("window {range_label} · {pts} pts")),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .children(ChartRange::all().map(|range| {
                                        let active = self.range == range;
                                        Button::new(("work-range", range as u32))
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(range.label())
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.set_range(range, cx);
                                            }))
                                    }))
                                    .child(
                                        div()
                                            .ml_2()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(status),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("work-spark")
                            .h(px(72.))
                            .w_full()
                            .px_2()
                            .pb_2()
                            .child(
                                div()
                                    .id("work-spark-surface")
                                    .size_full()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().background)
                                    .overflow_hidden()
                                    .child({
                                        let closes = spark.clone();
                                        canvas(
                                            move |bounds, _, _| bounds,
                                            move |bounds, _, window, _cx| {
                                                paint_sparkline(
                                                    bounds, &closes, line, fill, border, window,
                                                );
                                            },
                                        )
                                        .size_full()
                                    }),
                            ),
                    ),
            )
    }

    /// Right column: host gauges (major indices) + process table + journal.
    pub(crate) fn render_work_system_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let max_vol = self.max_watchlist_volume();
        let sel = self.current_symbol();
        let host_name = sel
            .map(|s| self.display_code(&s.code))
            .unwrap_or_else(|| "host".into());
        let (net_in, net_out) = if let Some(s) = sel {
            (
                Self::sys_net_mbs(s.volume, max_vol),
                Self::sys_net_mbs(s.volume.saturating_mul(3) / 4, max_vol),
            )
        } else {
            (1.2, 0.8)
        };

        // Major-index direction is encoded around a neutral 50% telemetry baseline.
        let sh = self.index_sh;
        let hs300 = self.index_hs300;
        let cyb = self.index_cyb;
        let telemetry = |pct: f64| (50.0 + pct * 12.0).clamp(5.0, 95.0);

        // Sort processes by abs change for top talkers.
        let mut procs: Vec<&Symbol> = self.symbols.iter().collect();
        procs.sort_by(|a, b| {
            b.change_pct
                .abs()
                .partial_cmp(&a.change_pct.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let top_procs: Vec<&Symbol> = procs.into_iter().take(12).collect();

        let t = chrono::Local::now().format("%H:%M:%S").to_string();
        let sh_load = sh.map(|s| telemetry(s.change_pct));
        let hs300_load = hs300.map(|s| telemetry(s.change_pct));
        let cyb_load = cyb.map(|s| telemetry(s.change_pct));
        let sh_pct = sh_load.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "--".into());
        let hs300_pct = hs300_load
            .map(|v| format!("{v:.0}%"))
            .unwrap_or_else(|| "--".into());
        let cyb_pct = cyb_load.map(|v| format!("{v:.0}%")).unwrap_or_else(|| "--".into());
        let gauge_value = |snap: Option<IndexSnap>, unit: &str| match snap {
            Some(s) if self.work_identity_reveal => {
                format!("{} {}", s.point_label(), s.pct_label())
            }
            Some(s) => format!("{}{unit}", s.point_label()),
            None => "--".into(),
        };
        let gauge_tip = |name: &str, snap: Option<IndexSnap>| match snap {
            Some(s) => format!("{name} · {} · {}", s.point_label(), s.pct_label()),
            None => format!("{name} · unavailable"),
        };
        let market_changes: Vec<f64> = [sh, hs300, cyb]
            .into_iter()
            .flatten()
            .map(|s| s.change_pct)
            .collect();
        let market_avg = if market_changes.is_empty() {
            None
        } else {
            Some(market_changes.iter().sum::<f64>() / market_changes.len() as f64)
        };
        let journal = [
            format!("{t}  scheduler tick · node={host_name}"),
            format!("{t}  sample cpu={sh_pct} mem={hs300_pct} disk={cyb_pct}"),
            format!("{t}  net rx={net_in:.1} tx={net_out:.1} MB/s"),
            format!(
                "{t}  cluster nodes={}",
                self.symbols.len()
            ),
            format!("{t}  gc pause ok · heap stable"),
            format!("{t}  worker pool active"),
        ];

        v_flex()
            .w(px(340.))
            .min_w(px(300.))
            .max_w(px(380.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_hidden()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h(px(32.))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child(if self.work_identity_reveal { "大盘" } else { "host" }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(if self.work_identity_reveal {
                                "沪深核心指数".to_string()
                            } else {
                                host_name.clone()
                            }),
                    ),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .p_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(sys_gauge(
                        1,
                        if self.work_identity_reveal { "上证" } else { "cpu" },
                        gauge_value(sh, "MHz"),
                        sh_load.unwrap_or(20.0),
                        gauge_tip("上证综指", sh),
                        cx,
                    ))
                    .child(sys_gauge(
                        2,
                        if self.work_identity_reveal { "沪深" } else { "mem" },
                        gauge_value(hs300, "MB"),
                        hs300_load.unwrap_or(20.0),
                        gauge_tip("沪深300", hs300),
                        cx,
                    ))
                    .child(sys_gauge(
                        3,
                        if self.work_identity_reveal { "创业" } else { "disk" },
                        gauge_value(cyb, "IOPS"),
                        cyb_load.unwrap_or(20.0),
                        gauge_tip("创业板指", cyb),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.work_identity_reveal { "大盘" } else { "net" }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().foreground)
                                    .child(if self.work_identity_reveal {
                                        market_avg
                                            .map(|v| format!("平均 {v:+.2}%"))
                                            .unwrap_or_else(|| "--".into())
                                    } else {
                                        format!("↓{net_in:.1}  ↑{net_out:.1} MB/s")
                                    }),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .h(px(28.))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .w(px(160.))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("process"),
                    )
                    .child(
                        div()
                            .w(px(48.))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("cpu"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("rss"),
                    ),
            )
            .child(
                div()
                    .id("work-proc-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .children(top_procs.into_iter().enumerate().map(|(ix, sym)| {
                        let is_selected = sym.code == self.selected.as_ref();
                        let code = shared(sym.code.clone());
                        let name = self.display_code(&sym.code);
                        let proc_cpu = Self::sys_cpu_pct(sym.change_pct);
                        let rss = Self::sys_rss_mb(&sym.code, sym.volume, max_vol);

                        div()
                            .id(("work-proc", ix))
                            .h(px(28.))
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.25))
                            .when(is_selected, |this| {
                                this.bg(cx.theme().accent.opacity(0.14))
                            })
                            .hover(|this| this.bg(cx.theme().accent.opacity(0.08)))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.select_symbol(code.clone(), cx);
                            }))
                            .child(
                                div()
                                    .w(px(160.))
                                    .text_xs()
                                    .font_medium()
                                    .text_color(cx.theme().foreground)
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .w(px(48.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{proc_cpu:.0}%")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{rss}M")),
                            )
                    })),
            )
            .child(
                v_flex()
                    .h(px(120.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .p_2()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .mb_1()
                            .child("journal"),
                    )
                    .children(journal.into_iter().map(|line| {
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.9))
                            .truncate()
                            .child(line)
                    })),
            )
    }

}
