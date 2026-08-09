use gpui::{Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use super::strategy::EvidenceDisplay;
use crate::app::helpers::*;
use crate::app::{ChartKind, StockApp};

impl StockApp {
    /// Indicator and strategy-evidence views for the active point-in-time series.
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
        let fmt = |v: Option<f64>| v.map(|n| format!("{n:.3}")).unwrap_or_else(|| "--".into());
        v_flex()
            .gap_1()
            .min_w(px(120.))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.work_mode {
                        "MACD"
                    } else {
                        "MACD 12/26/9"
                    }),
            )
            .child(detail_row(
                "DIF",
                &if candles_match { fmt(dif) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                "DEA",
                &if candles_match { fmt(dea) } else { "--".into() },
                cx,
            ))
            .child(detail_row(
                if self.work_mode { "HIST" } else { "柱" },
                &if candles_match {
                    fmt(hist)
                } else {
                    "--".into()
                },
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
                    .child(if self.work_mode {
                        "BOLL"
                    } else {
                        "BOLL 20·2σ"
                    }),
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
                            if work {
                                "Vol 20d ann"
                            } else {
                                "20日年化波动"
                            },
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
                            if work {
                                "Data completeness"
                            } else {
                                "数据完整度"
                            },
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

        // Versioned evidence report with explicit costs and chronological holdout.
        root = root.child(
            v_flex()
                .gap_1()
                .min_w(px(200.))
                .p_2()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().muted.opacity(0.28))
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().foreground)
                                .child(if work {
                                    "Strategy evidence"
                                } else {
                                    "策略证据报告"
                                }),
                        )
                        .child(div().flex_1())
                        .children(
                            crate::data::backtest::BacktestRule::all()
                                .into_iter()
                                .enumerate()
                                .map(|(index, rule)| {
                                    Button::new(("bt-run", index))
                                        .xsmall()
                                        .when(index == 0, |button| button.primary())
                                        .when(index != 0, |button| button.ghost())
                                        .label(rule.label(work))
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.run_selected_backtest(rule, cx);
                                        }))
                                }),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            self.backtest_report
                                .as_ref()
                                .map(|report| EvidenceDisplay::from_report(report, work).summary)
                                .unwrap_or_else(|| {
                                    if work {
                                        "Run on current daily series".into()
                                    } else {
                                        "对当前日 K 样本做可解释统计，非预测".into()
                                    }
                                }),
                        ),
                )
                .when_some(self.backtest_report.as_ref(), |col, r| {
                    let display = EvidenceDisplay::from_report(r, work);
                    col.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.85))
                            .child(display.evidence),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.85))
                            .child(display.execution),
                    )
                }),
        );

        root
    }
}
