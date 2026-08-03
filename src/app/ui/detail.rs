//! Bottom analysis dock tabs.

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
use super::super::labels::L;



impl StockApp {
    pub(crate) fn render_detail_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let active = self.detail_tab;

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            // Tab strip：功能分区，一次只看一类信息
            .child(
                h_flex()
                    .h(px(34.))
                    .px_2()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    // Primary analysis tabs only — lists live in the left sidebar.
                    .children(DetailTab::dock_tabs().map(|tab| {
                        let is_on = active == tab;
                        Button::new(("detail-tab", tab as u32))
                            .xsmall()
                            .when(is_on, |b| b.primary())
                            .when(!is_on, |b| b.ghost())
                            .label(tab.label(work))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.set_detail_tab(tab, cx);
                            }))
                    }))
                    // Ephemeral side tabs (opened from left list actions).
                    .when(!active.is_dock_primary(), |row| {
                        row.child(
                            Button::new(("detail-tab-ephemeral", active as u32))
                                .xsmall()
                                .primary()
                                .label(active.label(work)),
                        )
                        .child(
                            Button::new("detail-tab-ephemeral-close")
                                .xsmall()
                                .ghost()
                                .label(if work { "×" } else { "关闭" })
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    this.set_detail_tab(DetailTab::Overview, cx);
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .max_w(px(360.))
                            .overflow_hidden()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.status.clone()),
                    ),
            )
            .child(
                div()
                    .id("detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_x_hidden()
                    .overflow_y_scroll()
                    .p_3()
                    .child(match self.detail_tab {
                        DetailTab::Overview => self.render_detail_overview(cx).into_any_element(),
                        DetailTab::Strategy => self.render_signal_detail_col(cx).into_any_element(),
                        DetailTab::Ai => self.render_ai_detail_col(cx).into_any_element(),
                        DetailTab::Portfolio => {
                            self.render_portfolio_detail_col(cx).into_any_element()
                        }
                        DetailTab::Treasure => {
                            self.render_treasure_detail_col(cx).into_any_element()
                        }
                        DetailTab::Indicators => {
                            self.render_indicators_detail(cx).into_any_element()
                        }
                    }),
            )
    }

    /// 概览：紧凑两行——评分/因子 + OHLC/量能/快捷，填满底栏而不是漂在大片空底上。
    pub(crate) fn render_detail_overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let signal = self.current_signal();
        let sym = self.current_symbol();
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let snap = if candles_match {
            match self.chart_kind {
                ChartKind::Intraday => self.minute.as_ref().and_then(|m| m.snapshot()),
                ChartKind::DayK | ChartKind::MinuteK(_) => {
                    QuoteSnapshot::from_candles(&self.candles)
                }
            }
        } else {
            None
        };
        let last_candle = if candles_match {
            self.candles.last()
        } else {
            None
        };
        let code = self.selected.as_ref();
        let name_raw = sym.map(|s| s.name.as_ref()).unwrap_or("");
        let title = if work {
            self.display_code(code)
        } else if is_real_name(name_raw, code) {
            format!("{code}  {name_raw}")
        } else {
            code.to_string()
        };
        let period = if work {
            format!("{} · {} pts", self.chart_label(), self.candles.len())
        } else {
            format!("{} · {} 根", self.chart_label(), self.candles.len())
        };
        let prev = self.format_value(snap.as_ref().map(|s| s.prev_close).unwrap_or(0.0));
        let last_price = sym.map(|s| s.last).unwrap_or(0.0);
        let change_pct = sym.map(|s| s.change_pct).unwrap_or(0.0);
        let volume = sym.map(|s| s.volume).unwrap_or(0);

        v_flex()
            .w_full()
            .gap_2()
            // Row 1: score + title/chips + quick links
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_start()
                    .child(self.render_score_badge(signal.as_ref(), cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(200.))
                            .gap_1p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_baseline()
                                    .flex_wrap()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(cx.theme().muted)
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if work {
                                                "svc".to_string()
                                            } else {
                                                sym.map(|s| s.board.as_ref().to_string())
                                                    .unwrap_or_else(|| "--".into())
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(period),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(self.chg_color(change_pct >= 0.0, cx))
                                            .child(if last_price > 0.0 {
                                                format!(
                                                    "{}  {}",
                                                    self.format_value(last_price),
                                                    format_pct(change_pct)
                                                )
                                            } else {
                                                "—".into()
                                            }),
                                    ),
                            )
                            .child(if let Some(s) = signal.as_ref() {
                                h_flex()
                                    .gap_1p5()
                                    .flex_wrap()
                                    .child(metric_chip(
                                        if work { "RSI" } else { "RSI14" },
                                        &s.rsi14
                                            .map(|v| format!("{v:.1}"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "Mom20" } else { "20日动量" },
                                        &s.momentum_20_pct
                                            .map(|v| format!("{v:+.1}%"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "Vol×" } else { "量能比" },
                                        &s.volume_ratio_20
                                            .map(|v| format!("{v:.1}x"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "DD1Y" } else { "1Y回撤" },
                                        &s.max_drawdown_1y_pct
                                            .map(|v| format!("{v:.1}%"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "σ20" } else { "波动" },
                                        &s.volatility_20_ann_pct
                                            .map(|v| format!("{v:.1}%"))
                                            .unwrap_or_else(|| "—".into()),
                                        cx,
                                    ))
                                    .child(metric_chip(
                                        if work { "Conf" } else { "置信" },
                                        &format!("{:.0}%", s.confidence),
                                        cx,
                                    ))
                                    .into_any_element()
                            } else {
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work {
                                        "Need ≥20 daily bars for signal."
                                    } else {
                                        "至少需要 20 根有效日 K 才能生成策略评分。"
                                    })
                                    .into_any_element()
                            })
                            .when_some(signal.as_ref(), |col, s| {
                                col.child(
                                    h_flex().gap_1().flex_wrap().children(
                                        s.reasons.iter().take(5).map(|r| {
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded(cx.theme().radius)
                                                .bg(cx.theme().muted.opacity(0.55))
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child((*r).to_string())
                                        }),
                                    ),
                                )
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .min_w(px(120.))
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(L::quick_links(work)),
                            )
                            .child(
                                Button::new("goto-strategy")
                                    .xsmall()
                                    .ghost()
                                    .label(L::goto_strategy(work))
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_detail_tab(DetailTab::Strategy, cx);
                                    })),
                            )
                            .child(
                                Button::new("goto-ai")
                                    .xsmall()
                                    .ghost()
                                    .label(L::goto_ai(work))
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_detail_tab(DetailTab::Ai, cx);
                                    })),
                            )
                            .child(
                                Button::new("goto-indicators")
                                    .xsmall()
                                    .ghost()
                                    .label(L::goto_indicators(work))
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_detail_tab(DetailTab::Indicators, cx);
                                    })),
                            )
                            .child(
                                Button::new("goto-portfolio")
                                    .xsmall()
                                    .ghost()
                                    .label(L::goto_portfolio(work))
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        // Lists live in the left sidebar.
                                        this.set_left_tab(LeftTab::Portfolio, cx);
                                    })),
                            )
                            .child(
                                Button::new("goto-treasure")
                                    .xsmall()
                                    .ghost()
                                    .label(L::goto_treasure(work))
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_left_tab(LeftTab::Treasure, cx);
                                    })),
                            ),
                    ),
            )
            // Row 2: OHLC / volume / source — fills residual dock height with real info
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .flex_wrap()
                    .items_center()
                    .px_1()
                    .py_1p5()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted.opacity(0.35))
                    .child(metric_chip(
                        if work { "Base" } else { "昨收" },
                        &prev,
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "O" } else { "开" },
                        &last_candle
                            .map(|c| self.format_value(c.open))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "H" } else { "高" },
                        &last_candle
                            .map(|c| self.format_value(c.high))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "L" } else { "低" },
                        &last_candle
                            .map(|c| self.format_value(c.low))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "C" } else { "收" },
                        &last_candle
                            .map(|c| self.format_value(c.close))
                            .unwrap_or_else(|| "—".into()),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "Vol" } else { "量" },
                        &if volume > 0 {
                            format_volume(volume)
                        } else {
                            last_candle
                                .map(|c| format_volume(c.volume))
                                .unwrap_or_else(|| "—".into())
                        },
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.85))
                            .child(self.status.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.7))
                            .child(if work {
                                "For reference only."
                            } else {
                                "仅供学习研究，不构成投资建议。"
                            }),
                    ),
            )
    }

    pub(crate) fn render_score_badge(
        &self,
        signal: Option<&signals::SignalSnapshot>,
        cx: &App,
    ) -> impl IntoElement {
        let work = self.work_mode;
        let (score_txt, regime_txt, conf_txt, color) = if let Some(s) = signal {
            (
                format!("{:.0}", s.score),
                if work {
                    s.regime.service_state().to_string()
                } else {
                    s.regime.label().to_string()
                },
                format!("{:.0}%", s.confidence),
                self.regime_color(s.regime, cx),
            )
        } else {
            (
                "—".into(),
                if work { "n/a".into() } else { "无数据".into() },
                "—".into(),
                cx.theme().muted_foreground,
            )
        };

        v_flex()
            .items_center()
            .justify_center()
            .gap_1()
            .min_w(px(96.))
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .bg(color.opacity(0.12))
            .border_1()
            .border_color(color.opacity(0.35))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work { "Score" } else { "综合" }),
            )
            .child(
                h_flex()
                    .items_baseline()
                    .gap_0p5()
                    .child(
                        div()
                            .text_3xl()
                            .font_semibold()
                            .text_color(color)
                            .child(score_txt),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("/100"),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(color)
                    .child(regime_txt),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        format!("conf {conf_txt}")
                    } else {
                        format!("置信 {conf_txt}")
                    }),
            )
    }

    pub(crate) fn regime_color(&self, regime: signals::SignalRegime, cx: &App) -> gpui::Hsla {
        if self.work_mode {
            return cx.theme().muted_foreground;
        }
        use signals::SignalRegime::*;
        match regime {
            Strong => cx.theme().chart_1,
            Constructive => cx.theme().chart_2,
            Neutral => cx.theme().muted_foreground,
            Weak => cx.theme().chart_4,
            Defensive => cx.theme().danger,
        }
    }

    /// 指标 Tab：MA / MACD / BOLL 三卡并排，上下文相关（分时隐藏无意义读数）。
    pub(crate) fn render_indicators_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let kline_ok = candles_match && !matches!(self.chart_kind, ChartKind::Intraday);

        h_flex()
            .w_full()
            .gap_4()
            .items_start()
            .child(
                v_flex()
                    .gap_1()
                    .min_w(px(140.))
                    .flex_1()
                    .child(section_title(if work { "Moving avg" } else { "均线" }, cx))
                    .child(detail_row(
                        if work { "L1" } else { "MA5" },
                        &if kline_ok {
                            self.ma
                                .ma5
                                .last()
                                .and_then(|x| *x)
                                .map(|v| self.format_value(v))
                                .unwrap_or_else(|| "--".into())
                        } else {
                            "--".into()
                        },
                        cx,
                    ))
                    .child(detail_row(
                        if work { "L2" } else { "MA10" },
                        &if kline_ok {
                            self.ma
                                .ma10
                                .last()
                                .and_then(|x| *x)
                                .map(|v| self.format_value(v))
                                .unwrap_or_else(|| "--".into())
                        } else {
                            "--".into()
                        },
                        cx,
                    ))
                    .child(detail_row(
                        if work { "L3" } else { "MA20" },
                        &if kline_ok {
                            self.ma
                                .ma20
                                .last()
                                .and_then(|x| *x)
                                .map(|v| self.format_value(v))
                                .unwrap_or_else(|| "--".into())
                        } else {
                            "--".into()
                        },
                        cx,
                    ))
                    .child(detail_row(
                        if work { "L4" } else { "MA60" },
                        &if kline_ok {
                            self.ma
                                .ma60
                                .last()
                                .and_then(|x| *x)
                                .map(|v| self.format_value(v))
                                .unwrap_or_else(|| "--".into())
                        } else {
                            "--".into()
                        },
                        cx,
                    ))
                    .when(!kline_ok, |col| {
                        col.child(
                            div()
                                .mt_1()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "Switch to daily/minute K for MA."
                                } else {
                                    "切换到日 K / 分钟 K 查看均线。"
                                }),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .min_w(px(140.))
                    .flex_1()
                    .child(self.render_macd_detail_col(cx)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .min_w(px(140.))
                    .flex_1()
                    .child(self.render_boll_detail_col(cx)),
            )
    }

    pub(crate) fn render_macd_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref())
            && !matches!(self.chart_kind, ChartKind::Intraday);
        let (dif, dea, hist) = self.macd.value_at(self.macd.dif.len().saturating_sub(1));
        let fmt = |v: Option<f64>| {
            v.map(|n| format!("{n:.3}"))
                .unwrap_or_else(|| "--".into())
        };
        v_flex()
            .gap_1()
            .min_w(px(120.))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.work_mode { "MACD" } else { "MACD 12/26/9" }),
            )
            .child(detail_row(
                if self.work_mode { "DIF" } else { "DIF" },
                &if candles_match { fmt(dif) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "DEA" } else { "DEA" },
                &if candles_match { fmt(dea) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "HIST" } else { "柱" },
                &if candles_match { fmt(hist) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "Mode" } else { "显示" },
                if self.show_macd { "开" } else { "关" },
                cx,
            ))
    }

    pub(crate) fn render_boll_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref())
            && !matches!(self.chart_kind, ChartKind::Intraday);
        let (up, mid, low) = self.boll.value_at(self.boll.mid.len().saturating_sub(1));
        let fmt = |v: Option<f64>| {
            v.map(|n| self.format_value(n))
                .unwrap_or_else(|| "--".into())
        };
        v_flex()
            .gap_1()
            .min_w(px(120.))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.work_mode { "BOLL" } else { "BOLL 20·2σ" }),
            )
            .child(detail_row(
                "上轨",
                &if candles_match { fmt(up) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                "中轨",
                &if candles_match { fmt(mid) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                "下轨",
                &if candles_match { fmt(low) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "Mode" } else { "显示" },
                if self.show_boll { "开" } else { "关" },
                cx,
            ))
    }

    pub(crate) fn render_signal_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let signal = self.current_signal();

        let mut root = h_flex().w_full().gap_4().items_start();

        if let Some(s) = signal {
            let fmt = |v: Option<f64>, suffix: &str| {
                v.map(|n| format!("{n:.1}{suffix}"))
                    .unwrap_or_else(|| "—".into())
            };
            let regime = if work {
                s.regime.service_state()
            } else {
                s.regime.label()
            };
            root = root
                .child(self.render_score_badge(Some(&s), cx))
                .child(
                    v_flex()
                        .gap_1()
                        .min_w(px(220.))
                        .flex_1()
                        .child(section_title(
                            if work {
                                "Factors"
                            } else {
                                "策略雷达 · 多因子"
                            },
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Composite" } else { "综合" },
                            &format!("{:.0}/100 · {regime}", s.score),
                            cx,
                        ))
                        .child(detail_kv("RSI14", &fmt(s.rsi14, ""), cx))
                        .child(detail_kv(
                            if work { "Mom 20d" } else { "20日动量" },
                            &fmt(s.momentum_20_pct, "%"),
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Vol 20d ann" } else { "20日年化波动" },
                            &fmt(s.volatility_20_ann_pct, "%"),
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Max DD 1Y" } else { "1Y最大回撤" },
                            &fmt(s.max_drawdown_1y_pct, "%"),
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Vol ratio" } else { "量能比" },
                            &fmt(s.volume_ratio_20, "x"),
                            cx,
                        ))
                        .child(detail_kv(
                            if work { "Confidence" } else { "数据置信" },
                            &format!("{:.0}%", s.confidence),
                            cx,
                        )),
                )
                .child(
                    v_flex()
                        .gap_1()
                        .min_w(px(200.))
                        .flex_1()
                        .child(section_title(if work { "Rationale" } else { "依据" }, cx))
                        .children(s.reasons.iter().map(|r| {
                            div()
                                .px_2()
                                .py_1()
                                .rounded(cx.theme().radius)
                                .bg(cx.theme().muted.opacity(0.45))
                                .text_xs()
                                .text_color(cx.theme().foreground)
                                .child((*r).to_string())
                        }))
                        .child(
                            div()
                                .mt_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground.opacity(0.8))
                                .child(if work {
                                    "Explainable snapshot, not a trade instruction."
                                } else {
                                    "可解释技术快照，仅供学习研究，不构成投资建议。"
                                }),
                        ),
                );
        } else {
            root = root.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "Need ≥20 valid daily bars."
                    } else {
                        "至少需要 20 根有效日 K 数据。"
                    }),
            );
        }
        root
    }

    pub(crate) fn render_portfolio_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let code = self.selected.to_string();
        let pos = self.portfolio.position_state_of(&code);
        let last = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.last)
            .filter(|p| *p > 0.0)
            .or_else(|| self.candles.last().map(|c| c.close))
            .unwrap_or(0.0);
        let mark = pos.as_ref().filter(|p| p.shares > 1e-9).map(|p| {
            portfolio::PositionMark::from_position(p.clone(), last, 0.0)
        });
        let trades: Vec<_> = self
            .portfolio
            .trades_for(&code)
            .into_iter()
            .rev()
            .take(12)
            .cloned()
            .collect();

        let current_key = self.candles.last().map(|c| {
            let shares = pos.as_ref().map(|p| p.shares).unwrap_or(0.0);
            let avg = pos.as_ref().map(|p| p.avg_cost).unwrap_or(0.0);
            format!("pos:{}@{}:{:.4}:{:.4}", code, c.date, shares, avg)
        });
        let shown = current_key
            .as_ref()
            .is_some_and(|k| self.portfolio_ai_key.as_ref() == Some(k));
        let loading = matches!(&self.portfolio_ai_panel, AiPanelState::Loading { .. });
        let busy = shown && loading;
        let has_signal = self.current_signal().is_some();

        let mut col = v_flex()
            .gap_2()
            .w_full()
            .max_w(px(780.))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(section_title(
                        if work {
                            "Position"
                        } else {
                            "持仓与买卖建议"
                        },
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("pd-buy")
                                    .xsmall()
                                    .primary()
                                    .label(if work { "Buy" } else { "买入" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_trade_form(TradeSide::Buy, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("pd-sell")
                                    .xsmall()
                                    .ghost()
                                    .label(if work { "Sell" } else { "卖出" })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_trade_form(TradeSide::Sell, window, cx);
                                    })),
                            )
                            .child(
                                Button::new("pd-ai")
                                    .xsmall()
                                    .when(!busy && has_signal, |b| b.primary())
                                    .when(busy || !has_signal, |b| b.ghost())
                                    .label(if busy {
                                        if work {
                                            "Working…"
                                        } else {
                                            "分析中…"
                                        }
                                    } else if work {
                                        "Advice"
                                    } else {
                                        "AI 建议"
                                    })
                                    .disabled(busy || !has_signal)
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.request_portfolio_ai(cx);
                                    })),
                            ),
                    ),
            );

        // 持仓数字
        if let Some(m) = &mark {
            let pnl_c = self.chg_color(m.unrealized_pnl >= 0.0, cx);
            col = col.child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(metric_chip(
                        if work { "Qty" } else { "持股" },
                        &format_shares(m.position.shares),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "Cost" } else { "成本" },
                        &format_price(m.position.avg_cost),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "Last" } else { "现价" },
                        &format_price(m.last),
                        cx,
                    ))
                    .child(metric_chip(
                        if work { "Value" } else { "市值" },
                        &format!("{:.0}", m.market_value),
                        cx,
                    ))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .px_2()
                            .py_1()
                            .min_w(px(88.))
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().background)
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work { "P&L" } else { "浮盈亏" }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(pnl_c)
                                    .child(format!(
                                        "{} ({})",
                                        format_money(m.unrealized_pnl),
                                        format_pct(m.unrealized_pnl_pct)
                                    )),
                            ),
                    ),
            );
            if m.position.realized_pnl.abs() > 1e-6 {
                col = col.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{} {}",
                            if work { "Realized" } else { "已实现盈亏" },
                            format_money(m.position.realized_pnl)
                        )),
                );
            }
        } else if let Some(p) = &pos {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} · {} {}",
                        if work {
                            "Flat (history)"
                        } else {
                            "已清仓（保留流水）"
                        },
                        if work { "realized" } else { "已实现" },
                        format_money(p.realized_pnl)
                    )),
            );
        } else {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "No position on this symbol. Use Buy to open."
                    } else {
                        "当前标的无持仓。可用「买入」开仓，或先生成 AI 建仓观察建议。"
                    }),
            );
        }

        // AI 建议
        col = col.child(section_title(
            if work {
                "AI position advice"
            } else {
                "AI 买卖建议"
            },
            cx,
        ));
        if shown {
            match &self.portfolio_ai_panel {
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
                                "LLM advice in progress…"
                            } else {
                                "正在请求 LLM 持仓建议…"
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
                                "尚未生成建议。"
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
                        "Click Advice for cost-aware buy/sell guidance."
                    } else {
                        "点击「AI 建议」：结合成本、现价与技术面给出买卖观察倾向。"
                    }),
            );
        }

        // 成交流水
        col = col.child(
            h_flex()
                .items_center()
                .justify_between()
                .child(section_title(
                    if work { "Trades" } else { "成交记录" },
                    cx,
                ))
                .child(
                    Button::new("pd-undo")
                        .xsmall()
                        .ghost()
                        .label(if work { "Undo last" } else { "撤销最近" })
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.undo_last_trade_for_selected(cx);
                        })),
                ),
        );

        if trades.is_empty() {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "No trades yet."
                    } else {
                        "暂无成交。"
                    }),
            );
        } else {
            for (ix, t) in trades.iter().enumerate() {
                let side_c = match t.side {
                    TradeSide::Buy => self.chg_color(true, cx),
                    TradeSide::Sell => self.chg_color(false, cx),
                };
                let line = if work {
                    format!(
                        "{} {} × {} @ {}  {}",
                        t.side.label_work(),
                        format_shares(t.shares),
                        format_price(t.price),
                        if t.fee > 0.0 {
                            format!("fee {:.2}", t.fee)
                        } else {
                            String::new()
                        },
                        t.time
                    )
                } else {
                    format!(
                        "{} {} 股 @ {} 元{}  · {}",
                        t.side.label(),
                        format_shares(t.shares),
                        format_price(t.price),
                        if t.fee > 0.0 {
                            format!(" · 费 {:.2}", t.fee)
                        } else {
                            String::new()
                        },
                        t.time
                    )
                };
                col = col.child(
                    h_flex()
                        .id(("trade-row", ix))
                        .gap_2()
                        .items_center()
                        .px_1()
                        .py_0p5()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(side_c)
                                .child(if work {
                                    t.side.label_work()
                                } else {
                                    t.side.label()
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(cx.theme().foreground)
                                .child(line),
                        ),
                );
                if !t.note.is_empty() && !work {
                    col = col.child(
                        div()
                            .pl_4()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t.note.clone()),
                    );
                }
            }
        }

        // 现金设置
        col = col
            .child(section_title(
                if work {
                    "Cash tracking"
                } else {
                    "现金（可选）"
                },
                cx,
            ))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("pd-track-cash")
                            .xsmall()
                            .when(self.portfolio.track_cash, |b| b.primary())
                            .when(!self.portfolio.track_cash, |b| b.ghost())
                            .label(if self.portfolio.track_cash {
                                if work {
                                    "On"
                                } else {
                                    "约束开"
                                }
                            } else if work {
                                "Off"
                            } else {
                                "约束关"
                            })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.toggle_track_cash(cx);
                            })),
                    )
                    .child(div().flex_1().child(Input::new(&self.portfolio_cash_input).small()))
                    .child(
                        Button::new("pd-set-cash")
                            .xsmall()
                            .ghost()
                            .label(if work { "Set" } else { "设定" })
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.apply_portfolio_cash(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                    .child(if work {
                        "Optional cash balance. When On, buys require enough cash."
                    } else {
                        "可选记录现金。开启约束后，买入会检查余额；卖出回补现金。"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.75))
                    .child(if work {
                        "For reference only, not investment advice."
                    } else {
                        "仅供学习研究，不构成投资建议。持仓数据仅存本地。"
                    }),
            );

        col
    }

    pub(crate) fn render_ai_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let current = self.ai_current_key();
        let shown = current.is_some() && self.ai_key.as_deref() == current.as_deref();
        let loading = matches!(&self.ai_panel, AiPanelState::Loading { .. });
        // 只有「正在分析当前标的」时才禁用按钮；其他标的可并行触发。
        let busy = shown && loading;
        let has_signal = self.current_signal().is_some();

        let mut col = v_flex()
            .gap_2()
            .w_full()
            .max_w(px(720.))
            .child(
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
                            .child(if work { "Not generated." } else { "尚未生成。" }),
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

    pub(crate) fn render_treasure_detail_col(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let hit = self
            .treasure_hits
            .iter()
            .find(|h| h.code == self.selected.as_ref())
            .cloned();

        // 参考建仓/减仓带：优先用当前图表日 K（与选中标的匹配时）。
        let levels = self
            .candles_code
            .as_ref()
            .filter(|c| c.as_str() == self.selected.as_ref())
            .and_then(|_| levels::compute(&self.candles));

        let mut col = v_flex().gap_2().w_full().max_w(px(640.)).child(section_title(
            if work {
                "Scout · levels"
            } else {
                "寻宝鼠 · 搜罗价位"
            },
            cx,
        ));

        if let Some(lv) = levels.as_ref() {
            col = col
                .child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .child(metric_chip(
                            if work { "Spot" } else { "现价" },
                            &format_price(lv.close),
                            cx,
                        ))
                        .child(metric_chip(
                            if work { "Buy band" } else { "建仓带" },
                            &lv.buy_band_text(),
                            cx,
                        ))
                        .child(metric_chip(
                            if work { "Sell band" } else { "减仓带" },
                            &lv.sell_band_text(),
                            cx,
                        )),
                )
                .child(detail_kv(
                    if work { "Buy (ref)" } else { "参考建仓" },
                    &format!("{} 元（支撑侧分批观察）", lv.buy_band_text()),
                    cx,
                ))
                .child(detail_kv(
                    if work { "Sell (ref)" } else { "参考减仓" },
                    &format!("{} 元（阻力侧反弹观察）", lv.sell_band_text()),
                    cx,
                ));
            if let Some(atr) = lv.atr14 {
                col = col.child(detail_kv(
                    "ATR14",
                    &format!("{} 元", format_price(atr)),
                    cx,
                ));
            }
            if !lv.notes.is_empty() {
                col = col.child(detail_kv(
                    if work { "Basis" } else { "依据" },
                    &lv.notes.join("；"),
                    cx,
                ));
            }
        }

        if let Some(h) = hit {
            let tags = h
                .tags
                .iter()
                .map(|t| t.label())
                .collect::<Vec<_>>()
                .join(" · ");
            let tags_disp = if tags.is_empty() {
                "—".to_string()
            } else {
                tags
            };
            col = col
                .child(
                    h_flex()
                        .gap_3()
                        .flex_wrap()
                        .child(metric_chip(
                            if work { "Score" } else { "分数" },
                            &format!("{:.1}", h.score),
                            cx,
                        ))
                        .child(metric_chip(
                            if work { "Bars" } else { "样本" },
                            &format!("{}", h.bars),
                            cx,
                        ))
                        .child(metric_chip(
                            if work { "Src" } else { "来源" },
                            &h.source,
                            cx,
                        )),
                )
                .child(detail_kv(
                    if work { "Position" } else { "位置" },
                    &format!(
                        "1Y {} · 3Y {} · 全 {}",
                        fmt_pos(h.pos_1y),
                        fmt_pos(h.pos_3y),
                        fmt_pos(h.pos_all)
                    ),
                    cx,
                ))
                .child(detail_kv(
                    if work { "Percentile" } else { "分位" },
                    &format!(
                        "1Y {} · 3Y {} · 全 {}",
                        fmt_pos(h.pctile_1y),
                        fmt_pos(h.pctile_3y),
                        fmt_pos(h.pctile_all)
                    ),
                    cx,
                ))
                .child(detail_kv(
                    if work { "Drawdown" } else { "回撤" },
                    &format!("1Y {} · 全 {}", fmt_dd(h.dd_1y), fmt_dd(h.dd_all)),
                    cx,
                ))
                .child(detail_kv(if work { "Tags" } else { "标签" }, &tags_disp, cx))
                .when(!work, |c| {
                    c.child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "搜罗：低位扫描 + 技术参考价位。建仓/减仓带为本地指标推算，非买卖指令。仅供学习研究。",
                            ),
                    )
                });
        } else if levels.is_none() {
            col = col
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if work {
                            "Not in latest scan. Open the left Scan tab."
                        } else {
                            "当前标的不在最近寻宝结果中。可打开左侧「寻宝」扫描；加载日 K 后也会显示参考价位。"
                        }),
                )
                .child(
                    Button::new("open-treasure-tab")
                        .xsmall()
                        .ghost()
                        .label(if work { "Open Scan" } else { "打开寻宝" })
                        .on_click(cx.listener(|this, _, _w, cx| {
                            this.set_left_tab(LeftTab::Treasure, cx);
                        })),
                );
        } else if !work {
            col = col.child(
                div()
                    .mt_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        "当前不在寻宝榜内，仍可根据日 K 显示技术参考价位。左侧「寻宝」可扩大搜罗。仅供学习，非投资建议。",
                    ),
            );
        }
        col
    }

}
