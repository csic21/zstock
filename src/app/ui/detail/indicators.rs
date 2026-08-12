use gpui::{Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder, px};
use gpui_component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use super::strategy::{EvidenceDisplay, GateState, PlaybookOutcome, StrategyPlaybook};
use crate::app::helpers::*;
use crate::app::{ChartKind, StockApp};
use crate::data::backtest::EvidenceVerdict;

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
        let card = self.decision_card_view_model();
        let levels = self.current_levels();
        let last_price = self
            .current_symbol()
            .map(|symbol| symbol.last)
            .filter(|price| price.is_finite() && *price > 0.0)
            .or_else(|| levels.as_ref().map(|levels| levels.close))
            .unwrap_or_default();
        let playbook = StrategyPlaybook::build(
            &card,
            signal.as_ref(),
            levels.as_ref(),
            last_price,
            self.backtest_report.as_ref(),
            work,
        );
        let outcome_color = match playbook.outcome {
            PlaybookOutcome::PlanReady => cx.theme().success,
            PlaybookOutcome::Wait => cx.theme().warning,
            PlaybookOutcome::NoAction => cx.theme().danger,
            PlaybookOutcome::NeedEvidence => cx.theme().muted_foreground,
        };
        let can_backtest = matches!(self.chart_kind, ChartKind::DayK) && self.candles.len() >= 60;
        let active_rule = self.backtest_active_rule;

        v_flex()
            .w_full()
            .gap_3()
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .p_3()
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(outcome_color.opacity(0.42))
                    .bg(outcome_color.opacity(0.06))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(if work { "Decision gates" } else { "今日交易剧本" }),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(outcome_color.opacity(0.16))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(outcome_color)
                                    .child(playbook.outcome.label(work)),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work {
                                        "all gates required"
                                    } else {
                                        "五道门槛必须同时通过"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(outcome_color)
                            .child(playbook.summary.clone()),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_start()
                            .flex_wrap()
                            .children(playbook.gates.iter().map(|gate| {
                                let color = match gate.state {
                                    GateState::Passed => cx.theme().success,
                                    GateState::Waiting => cx.theme().warning,
                                    GateState::Blocked => cx.theme().danger,
                                };
                                v_flex()
                                    .flex_1()
                                    .min_w(px(150.0))
                                    .gap_0p5()
                                    .p_2()
                                    .rounded(cx.theme().radius)
                                    .border_1()
                                    .border_color(color.opacity(0.28))
                                    .bg(cx.theme().background.opacity(0.48))
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .child(gate.title),
                                            )
                                            .child(div().flex_1())
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(color)
                                                    .child(gate.state.label(work)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(gate.value.clone()),
                                    )
                            })),
                    ),
            )
            .child(if let Some(signal) = signal.as_ref() {
                let fmt = |value: Option<f64>, suffix: &str| {
                    value
                        .map(|number| format!("{number:.1}{suffix}"))
                        .unwrap_or_else(|| "—".into())
                };
                let regime = if work {
                    signal.regime.service_state()
                } else {
                    signal.regime.label()
                };
                v_flex()
                    .w_full()
                    .gap_1p5()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(section_title(
                                if work { "Factor snapshot" } else { "因子快照 · 用于解释，不单独决策" },
                                cx,
                            ))
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} {:.0}/100", regime, signal.score)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .child(metric_chip("RSI14", &fmt(signal.rsi14, ""), cx))
                            .child(metric_chip(
                                if work { "Mom20" } else { "20日动量" },
                                &fmt(signal.momentum_20_pct, "%"),
                                cx,
                            ))
                            .child(metric_chip(
                                if work { "MA20 drift" } else { "偏离MA20" },
                                &format!("{:+.1}%", signal.price_vs_ma20_pct),
                                cx,
                            ))
                            .child(metric_chip(
                                if work { "Vol" } else { "年化波动" },
                                &fmt(signal.volatility_20_ann_pct, "%"),
                                cx,
                            ))
                            .child(metric_chip(
                                if work { "Volume" } else { "量能比" },
                                &fmt(signal.volume_ratio_20, "x"),
                                cx,
                            ))
                            .child(metric_chip(
                                if work { "Data" } else { "完整度" },
                                &format!("{:.0}%", signal.confidence),
                                cx,
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(signal.reasons.iter().take(6).map(|reason| {
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(cx.theme().muted.opacity(0.5))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child((*reason).to_string())
                            })),
                    )
                    .into_any_element()
            } else {
                div()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if work {
                        "Need at least 20 valid daily bars."
                    } else {
                        "至少需要 20 根有效日 K 才能生成因子快照。"
                    })
                    .into_any_element()
            })
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .p_3()
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.20))
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(if work { "Evidence comparison" } else { "策略证据 · 同口径比较" }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if work {
                                                "next-open execution · costs · 70/30 holdout"
                                            } else {
                                                "次日开盘成交 · 扣除费用与滑点 · 70/30 时间切分"
                                            }),
                                    ),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("bt-compare-all")
                                    .xsmall()
                                    .primary()
                                    .disabled(!can_backtest)
                                    .label(if work { "Compare 3" } else { "比较 3 条策略" })
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.run_backtest_comparison(cx);
                                    })),
                            )
                            .children(
                                crate::data::backtest::BacktestRule::all()
                                    .into_iter()
                                    .enumerate()
                                    .map(|(index, rule)| {
                                        Button::new(("bt-run", index))
                                            .xsmall()
                                            .when(rule == active_rule, |button| button.primary())
                                            .when(rule != active_rule, |button| button.ghost())
                                            .disabled(!can_backtest)
                                            .label(rule.label(work))
                                            .on_click(cx.listener(move |this, _, _window, cx| {
                                                this.run_selected_backtest(rule, cx);
                                            }))
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().accent)
                            .child(active_rule.playbook(work)),
                    )
                    .when(!self.backtest_comparison.is_empty(), |column| {
                        column.child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .items_start()
                                .flex_wrap()
                                .children(self.backtest_comparison.iter().map(|report| {
                                    let quality = report.quality_metrics();
                                    let verdict = report.verdict();
                                    let color = match verdict {
                                        EvidenceVerdict::Candidate => cx.theme().success,
                                        EvidenceVerdict::Observe => cx.theme().warning,
                                        EvidenceVerdict::Reject => cx.theme().danger,
                                        EvidenceVerdict::Insufficient => cx.theme().muted_foreground,
                                    };
                                    let selected = report.rule == active_rule.label(false);
                                    v_flex()
                                        .flex_1()
                                        .min_w(px(220.0))
                                        .gap_1()
                                        .p_2()
                                        .rounded(cx.theme().radius)
                                        .border_1()
                                        .border_color(if selected { cx.theme().accent } else { color.opacity(0.28) })
                                        .bg(cx.theme().background.opacity(0.58))
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_semibold()
                                                        .text_color(cx.theme().foreground)
                                                        .child(report.rule.clone()),
                                                )
                                                .child(div().flex_1())
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(color)
                                                        .child(verdict.label(work)),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .flex_wrap()
                                                .child(metric_chip(
                                                    if work { "Win" } else { "胜率" },
                                                    &quality.win_rate_pct.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "—".into()),
                                                    cx,
                                                ))
                                                .child(metric_chip(
                                                    if work { "OOS win" } else { "样本外胜率" },
                                                    &quality.out_of_sample_win_rate_pct.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "—".into()),
                                                    cx,
                                                ))
                                                .child(metric_chip(
                                                    "PF",
                                                    &quality.profit_factor.map(|value| format!("{value:.2}")).unwrap_or_else(|| "—".into()),
                                                    cx,
                                                ))
                                                .child(metric_chip(
                                                    if work { "Payoff" } else { "盈亏比" },
                                                    &quality.payoff_ratio.map(|value| format!("{value:.2}")).unwrap_or_else(|| "—".into()),
                                                    cx,
                                                )),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(format!(
                                                    "n={} · OOS {} · E {:+.2}% · OOS E {} · MDD {:+.1}%",
                                                    quality.trade_count,
                                                    quality.out_of_sample_count,
                                                    quality.expectancy_pct.unwrap_or_default(),
                                                    quality.out_of_sample_expectancy_pct.map(|value| format!("{value:+.2}%")).unwrap_or_else(|| "—".into()),
                                                    report.evidence.max_drawdown_pct,
                                                )),
                                        )
                                })),
                        )
                    })
                    .child(
                        self.backtest_report
                            .as_ref()
                            .map(|report| {
                                let display = EvidenceDisplay::from_report(report, work);
                                v_flex()
                                    .gap_0p5()
                                    .pt_1()
                                    .border_t_1()
                                    .border_color(cx.theme().border)
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(display.summary),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(display.evidence),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(display.execution),
                                    )
                                    .into_any_element()
                            })
                            .unwrap_or_else(|| {
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if can_backtest {
                                        if work { "Choose a rule or compare all three." } else { "选择规则或一次比较三条；当前标的结果不能替代跨股票验证。" }
                                    } else if work {
                                        "Switch to daily K and load at least 60 bars."
                                    } else {
                                        "请切换到日 K 并加载至少 60 根；分钟 K 不用于这组日线规则。"
                                    })
                                    .into_any_element()
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().warning)
                            .child(if work {
                                "Win rate alone is not an edge. Check payoff, OOS expectancy, drawdown and sample size together."
                            } else {
                                "胜率不能单独代表优势：必须同时看盈亏比、样本外期望、回撤和交易数；同一样本择优仍有过拟合风险。"
                            }),
                    ),
            )
    }
}
