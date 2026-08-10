use gpui::{
    AnyElement, Context, IntoElement, ParentElement, Styled, Window, div, prelude::FluentBuilder,
    px,
};
use gpui_component::{
    ActiveTheme, Disableable, PixelsExt, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    scroll::ScrollableElement,
    v_flex,
};

use crate::app::StockApp;

use super::presenter::{StrategyLabLayout, leaderboard};
use super::state::StrategyLabPage;

impl StockApp {
    pub(crate) fn render_strategy_lab(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = &self.strategy_lab_feature.state;
        let layout = StrategyLabLayout::for_width(window.bounds().size.width.as_f32());
        let selected_experiment = state.selected_experiment_id.as_deref();
        let navigation = h_flex().w_full().gap_1().flex_wrap().children(
            StrategyLabPage::ALL
                .into_iter()
                .enumerate()
                .map(|(index, page)| {
                    let active = state.page == page;
                    Button::new(("strategy-lab-page", index))
                        .xsmall()
                        .when(active, |button| button.primary())
                        .when(!active, |button| button.ghost())
                        .label(page.label())
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.strategy_lab_set_page(page, cx);
                        }))
                }),
        );

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(div().text_lg().font_semibold().child("AI 策略实验室"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("AI 只提出假设；数据、执行、指标和晋级结论均由本地确定性程序计算"),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        selected_experiment
                                            .map(short_id)
                                            .unwrap_or_else(|| "尚无实验".into()),
                                    ),
                            ),
                    )
                    .child(navigation)
                    .when(!state.status.is_empty(), |column| {
                        column.child(
                            div()
                                .text_xs()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(cx.theme().muted)
                                .text_color(cx.theme().muted_foreground)
                                .child(state.status.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scrollbar()
                    .p_4()
                    .child(self.render_strategy_lab_page(layout, cx)),
            )
    }

    fn render_strategy_lab_page(
        &self,
        layout: StrategyLabLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.strategy_lab_feature.state.page {
            StrategyLabPage::Configure => self.render_strategy_lab_config(layout, cx),
            StrategyLabPage::Drafts => self.render_strategy_lab_drafts(layout, cx),
            StrategyLabPage::Progress => self.render_strategy_lab_progress(cx),
            StrategyLabPage::Leaderboard => self.render_strategy_lab_leaderboard(cx),
            StrategyLabPage::Report => self.render_strategy_lab_report(cx),
            StrategyLabPage::PaperCandidates => self.render_strategy_lab_paper(cx),
        }
    }

    fn render_strategy_lab_config(
        &self,
        layout: StrategyLabLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let form = &state.form;
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .children((3..=5).map(|count| {
                Button::new(("strategy-count", count))
                    .xsmall()
                    .when(form.strategy_count == count, |button| button.primary())
                    .when(form.strategy_count != count, |button| button.ghost())
                    .label(format!("{count} 个策略"))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.strategy_lab_set_count(count, cx);
                    }))
            }))
            .child(
                Button::new("strategy-create-current")
                    .ghost()
                    .label("冻结当前日 K 并创建实验")
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.strategy_lab_create_current(cx);
                    })),
            )
            .child(
                Button::new("strategy-create-watchlist")
                    .ghost()
                    .disabled(state.busy)
                    .label("冻结自选池（最多 100 只）")
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.strategy_lab_create_watchlist_pool(cx);
                    })),
            )
            .child(
                Button::new("strategy-create-ai")
                    .primary()
                    .disabled(state.busy)
                    .label(if state.busy {
                        "生成中…"
                    } else {
                        "AI 生成 3–5 个策略"
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.strategy_lab_generate_ai(cx);
                    })),
            );
        let action_block = if layout.actions_stacked() {
            v_flex().gap_2().child(actions).into_any_element()
        } else {
            actions.into_any_element()
        };
        v_flex()
            .w_full()
            .max_w(px(1_000.0))
            .gap_4()
            .child(section_title(
                "新实验",
                "冻结版本化数据和参数，保证之后可以离线复现",
                cx,
            ))
            .child(info_card("研究目标", form.goal.clone(), cx))
            .child(info_card(
                "风险与资金",
                format!(
                    "最大回撤预算 {:.1}% · 初始资金 ¥{:.0} · 基准：冻结股票池等权",
                    form.max_drawdown_pct, form.initial_cash
                ),
                cx,
            ))
            .child(info_card(
                "股票池与时间区间",
                "首个 UI 入口使用当前已加载日 K；50–100 股正式批量研究可复用同一冻结数据集接口。",
                cx,
            ))
            .child(action_block)
            .when(!state.experiments.is_empty(), |column| {
                column
                    .child(section_title("历史实验", "关闭应用后仍从 SQLite 恢复", cx))
                    .children(
                        state
                            .experiments
                            .iter()
                            .enumerate()
                            .map(|(index, experiment)| {
                                let id = experiment.definition.id.clone();
                                Button::new(("strategy-experiment", index))
                                    .ghost()
                                    .label(format!(
                                        "{} · {:?} · {} 个策略",
                                        short_id(&id),
                                        experiment.status,
                                        experiment.definition.strategy_ids.len()
                                    ))
                                    .on_click(cx.listener(move |this, _, _window, cx| {
                                        this.strategy_lab_select_experiment(id.clone(), cx);
                                    }))
                            }),
                    )
            })
            .into_any_element()
    }

    fn render_strategy_lab_drafts(
        &self,
        _layout: StrategyLabLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let versions = state
            .selected_experiment_id
            .as_deref()
            .and_then(|id| {
                state
                    .experiments
                    .iter()
                    .find(|experiment| experiment.definition.id == id)
            })
            .map(|experiment| {
                format!(
                    "数据集 {} · 基准 {} · 成本 {} · 成交 cn-daily-next-open-v1 · 策略版本不可覆盖",
                    short_id(&experiment.definition.dataset_id),
                    experiment.definition.benchmark_version,
                    experiment.definition.cost_model_version
                )
            })
            .unwrap_or_else(|| "版本信息不可用".into());
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(section_title(
                        "策略草案",
                        "规范化规则先经本地校验，再允许进入回测",
                        cx,
                    ))
                    .child(
                        Button::new("strategy-run")
                            .primary()
                            .disabled(state.drafts.is_empty() || state.busy)
                            .label(if state.busy {
                                "运行中…"
                            } else {
                                "后台批量运行"
                            })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.strategy_lab_start_run(cx);
                            })),
                    ),
            )
            .when(state.drafts.is_empty(), |column| {
                column.child(info_card("暂无草案", "先在实验配置页创建实验。", cx))
            })
            .children(state.drafts.iter().enumerate().map(|(index, draft)| {
                let versions = versions.clone();
                let rules = serde_json::to_string(&serde_json::json!({
                    "entry": draft.spec.entry,
                    "exit": draft.spec.exit,
                    "position": draft.spec.position,
                }))
                .unwrap_or_else(|_| "规则序列化失败".into());
                v_flex()
                    .w_full()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(div().font_semibold().child(format!(
                                "{}. {}",
                                index + 1,
                                draft.spec.name
                            )))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} · {}",
                                        draft.source, draft.validation_message
                                    )),
                            ),
                    )
                    .child(div().text_sm().child(draft.spec.hypothesis.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("版本 {}", short_id(&draft.strategy_id))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family("monospace")
                            .text_color(cx.theme().muted_foreground)
                            .child(rules),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(versions),
                    )
            }))
            .into_any_element()
    }

    fn render_strategy_lab_progress(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let progress = state.progress.as_ref();
        v_flex()
            .w_full()
            .max_w(px(900.0))
            .gap_4()
            .child(section_title(
                "运行进度",
                "批量任务在后台执行；取消会保留一致的部分报告",
                cx,
            ))
            .child(info_card(
                "总体进度",
                progress
                    .map(|value| {
                        format!(
                            "策略 {}/{} · 当前交易日 {}/{} · 缓存命中 {}",
                            value.completed_strategies,
                            value.total_strategies,
                            value.completed_sessions,
                            value.total_sessions,
                            value.cached_reports
                        )
                    })
                    .unwrap_or_else(|| "尚未运行".into()),
                cx,
            ))
            .when(state.busy, |column| {
                column.child(
                    Button::new("strategy-cancel")
                        .danger()
                        .label("取消任务")
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.strategy_lab_cancel(cx);
                        })),
                )
            })
            .children(state.failures.iter().map(|failure| {
                info_card(
                    "隔离失败",
                    format!("{} · {}", short_id(&failure.strategy_id), failure.message),
                    cx,
                )
            }))
            .into_any_element()
    }

    fn render_strategy_lab_leaderboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let rows = leaderboard(&state.reports, &state.robustness);
        v_flex()
            .w_full()
            .gap_3()
            .child(section_title(
                "确定性排行榜",
                "先过硬门槛，再按成本后样本外超额、回撤与稳定性展示",
                cx,
            ))
            .when(rows.is_empty(), |column| {
                column.child(info_card("暂无报告", "创建实验并运行后，结果会出现在这里。", cx))
            })
            .children(rows.into_iter().enumerate().map(|(index, row)| {
                let strategy_id = row.strategy_id.clone();
                let reason = row
                    .reasons
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "全部确定性门槛通过".into());
                Button::new(("strategy-leaderboard", index))
                    .ghost()
                    .w_full()
                    .label(format!(
                        "#{:02}  {}  | 收益 {:+.2}% · 超额 {:+.2}% · 回撤 {:.2}% · {} 笔 | {} · {} | {}",
                        index + 1,
                        short_id(&row.strategy_id),
                        row.return_pct,
                        row.excess_pct,
                        row.drawdown_pct,
                        row.trades,
                        row.evidence,
                        row.conclusion,
                        reason
                    ))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.strategy_lab_select_report(strategy_id.clone(), cx);
                    }))
            }))
            .into_any_element()
    }

    fn render_strategy_lab_report(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let Some(report) = state.selected_report() else {
            return info_card("暂无选中报告", "从排行榜选择一个策略下钻。", cx).into_any_element();
        };
        let selected_trade = state.selected_trade();
        let equity_summary = match (report.daily_equity.first(), report.daily_equity.last()) {
            (Some(first), Some(last)) => format!(
                "{} 个交易日 · {} ¥{:.2} → {} ¥{:.2} · 期末现金 ¥{:.2} · 市场暴露 {:.1}% · 最大回撤 {:.2}%",
                report.daily_equity.len(),
                first.date,
                first.total_equity,
                last.date,
                last.total_equity,
                last.cash,
                report.metrics.market_exposure_pct,
                report.metrics.max_drawdown_pct.abs()
            ),
            _ => "没有可绘制的逐日资金点".into(),
        };
        let yearly = if report.metrics_by_year.is_empty() {
            "没有完整年度分组".into()
        } else {
            report
                .metrics_by_year
                .iter()
                .map(|(year, metrics)| {
                    format!(
                        "{year}: 收益 {:+.2}% / 回撤 {:.2}% / {} 笔",
                        metrics.total_return_pct,
                        metrics.max_drawdown_pct.abs(),
                        metrics.trade_count
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let regime = state
            .robustness
            .iter()
            .find(|item| item.strategy_id == report.strategy_id)
            .map(|item| {
                item.metrics_by_regime
                    .iter()
                    .map(|(regime, metrics)| {
                        format!(
                            "{regime:?}: {:+.2}% / {} 笔",
                            metrics.total_return_pct, metrics.trade_count
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| "市场状态样本不足".into());
        let sealed_test = state
            .sealed_tests
            .iter()
            .find(|result| result.strategy_id == report.strategy_id);
        v_flex()
            .w_full()
            .gap_3()
            .child(section_title(
                "证据报告",
                format!("策略 {}", short_id(&report.strategy_id)),
                cx,
            ))
            .child(info_card(
                "可复现输入",
                format!(
                    "数据集 {} · schema v{} · 基准 {} · 执行 {} · 配置 {}",
                    short_id(&report.dataset_id),
                    report.strategy_schema_version,
                    report.config.benchmark_version,
                    report.execution_rule,
                    short_id(&report.config_hash)
                ),
                cx,
            ))
            .child(info_card(
                "核心指标",
                format!(
                    "收益 {:+.2}% · 基准 {:+.2}% · 超额 {:+.2}% · 最大回撤 {:.2}% · Sharpe {:.2} · 成本 ¥{:.2} · {} 笔",
                    report.metrics.total_return_pct,
                    report.metrics.benchmark_return_pct,
                    report.metrics.excess_return_pct,
                    report.metrics.max_drawdown_pct.abs(),
                    report.metrics.sharpe,
                    report.metrics.total_cost,
                    report.metrics.trade_count
                ),
                cx,
            ))
            .child(info_card("逐日资金曲线", equity_summary, cx))
            .child(info_card("年度分组", yearly, cx))
            .child(info_card("市场状态分组", regime, cx))
            .child(info_card(
                "样本与稳健性",
                state
                    .robustness
                    .iter()
                    .find(|item| item.strategy_id == report.strategy_id)
                    .map(|item| {
                        let failed = item.promotion.gates.iter().filter(|gate| !gate.passed).count();
                        format!(
                            "训练 {}–{} · 验证 {}–{} · walk-forward {} 窗口 · 2×成本压力 {} 项 · {:?} / {:?} · {} 个门槛失败",
                            item.training_interval.start,
                            item.training_interval.end,
                            item.validation_interval.start,
                            item.validation_interval.end,
                            item.walk_forward.len(),
                            item.stress_tests.len(),
                            item.promotion.evidence_grade,
                            item.promotion.conclusion,
                            failed
                        )
                    })
                    .unwrap_or_else(|| "数据区间不足，未生成完整稳健性报告；不能晋级。".into()),
                cx,
            ))
            .child(info_card(
                "封存测试",
                sealed_test
                    .map(|result| {
                        format!(
                            "已于 {} 一次性消费 · 收益 {:+.2}% · 超额 {:+.2}% · 回撤 {:.2}% · {} 笔",
                            result.consumed_at,
                            result.report.metrics.total_return_pct,
                            result.report.metrics.excess_return_pct,
                            result.report.metrics.max_drawdown_pct.abs(),
                            result.report.metrics.trade_count
                        )
                    })
                    .unwrap_or_else(|| {
                        "尚未查看。只有完成训练/验证和候选选择后才应显式消费；查看后不可重复，也不可把结果交给 AI 覆盖同一策略。".into()
                    }),
                cx,
            ))
            .when(sealed_test.is_none(), |column| {
                column.child(
                    Button::new("strategy-consume-sealed")
                        .danger()
                        .disabled(state.busy)
                        .label("一次性查看封存测试")
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.strategy_lab_consume_sealed_test(cx);
                        })),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        state.ai_explanation.clone().unwrap_or_else(|| {
                            "AI 解释缺失；不影响本地报告与确定性评级。若生成解释，将标注“AI 生成，不参与确定性评级”。".into()
                        }),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("strategy-promote-paper")
                            .primary()
                            .label("锁定版本并加入模拟观察")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.strategy_lab_promote_paper(cx);
                            })),
                    )
                    .child(
                        Button::new("strategy-export-report")
                            .ghost()
                            .label("导出实验")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.strategy_lab_export(cx);
                            })),
                    ),
            )
            .child(section_title("逐笔交易", "选择任一交易查看信号日、成交日、成本和持有期", cx))
            .children(report.trades.iter().enumerate().map(|(index, trade)| {
                Button::new(("strategy-trade", index))
                    .ghost()
                    .w_full()
                    .label(format!(
                        "{} · {}→{} · {} 股 · 净收益 {:+.2} · 成本 {:.2} · 持有 {} 日",
                        trade.instrument.code,
                        trade.entry_date,
                        trade.exit_date,
                        trade.quantity,
                        trade.net_pnl,
                        trade.total_cost,
                        trade.holding_sessions
                    ))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.strategy_lab_select_trade(index, cx);
                    }))
            }))
            .when_some(selected_trade, |column, trade| {
                column.child(info_card(
                    "交易下钻",
                    format!(
                        "{} · 入场信号 {} / 次日有效开盘成交 {} @ {:.3} · 退出信号 {} / 成交 {} @ {:.3} · 净收益率 {:+.2}%",
                        trade.instrument.storage_key(),
                        trade.signal_entry_date,
                        trade.entry_date,
                        trade.entry_price,
                        trade.signal_exit_date,
                        trade.exit_date,
                        trade.exit_price,
                        trade.net_return_pct
                    ),
                    cx,
                ))
            })
            .into_any_element()
    }

    fn render_strategy_lab_paper(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let candidates = &state.paper_candidates;
        v_flex()
            .w_full()
            .gap_3()
            .child(section_title(
                "模拟候选",
                "只有通过版本化硬门槛的策略才能进入每日观察；不代表保证未来盈利",
                cx,
            ))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("paper-run-daily")
                            .primary()
                            .disabled(state.busy || candidates.is_empty())
                            .label(if state.busy {
                                "更新中…"
                            } else {
                                "更新每日模拟信号"
                            })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.strategy_lab_run_paper(cx);
                            })),
                    )
                    .child(
                        Button::new("paper-export")
                            .ghost()
                            .label("导出策略、报告与模拟记录")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.strategy_lab_export(cx);
                            })),
                    ),
            )
            .when(candidates.is_empty(), |column| {
                column.child(info_card(
                    "暂无模拟候选",
                    "未通过任一硬门槛的策略会明确保留淘汰原因，不会由 AI 改写结论。",
                    cx,
                ))
            })
            .children(candidates.iter().map(|candidate| {
                let run = state
                    .paper_runs
                    .iter()
                    .find(|run| run.candidate_id == candidate.id);
                let comparison = state
                    .paper_comparisons
                    .iter()
                    .find(|(id, _)| id == &candidate.id)
                    .map(|(_, comparison)| comparison);
                info_card(
                    format!("模拟候选 {}", short_id(&candidate.strategy_id)),
                    match (run, comparison) {
                        (Some(run), Some(comparison)) => format!(
                            "截至 {}：{} 个信号 · {} 笔完成交易 · {} 个持仓 · 无法/待成交 {:.1}% · 平均成交跳空 {:.1} bps · 观察 {} 天 · 最低观察条件 {}{}",
                            run.as_of,
                            run.signals.len(),
                            run.trades.len(),
                            run.open_positions.len(),
                            comparison.missed_signal_pct,
                            comparison.average_execution_gap_bps,
                            comparison.observation_days,
                            if comparison.minimum_observation_met { "已满足" } else { "未满足" },
                            comparison
                                .warnings
                                .first()
                                .map(|warning| format!(" · {warning}"))
                                .unwrap_or_default()
                        ),
                        (Some(run), None) => format!(
                            "截至 {}：{} 个信号 · {} 笔交易；原回测报告未加载，暂不比较行为偏差",
                            run.as_of,
                            run.signals.len(),
                            run.trades.len()
                        ),
                        (None, _) => "规则版本与冻结数据集已锁定；等待首次每日运行".into(),
                    },
                    cx,
                )
            }))
            .into_any_element()
    }
}

fn section_title(
    title: impl Into<String>,
    subtitle: impl Into<String>,
    cx: &mut Context<StockApp>,
) -> AnyElement {
    v_flex()
        .gap_0p5()
        .child(div().font_semibold().child(title.into()))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(subtitle.into()),
        )
        .into_any_element()
}

fn info_card(
    title: impl Into<String>,
    body: impl Into<String>,
    cx: &mut Context<StockApp>,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_1()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .child(div().text_sm().font_semibold().child(title.into()))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(body.into()),
        )
        .into_any_element()
}

fn short_id(id: &str) -> String {
    const MAX: usize = 28;
    if id.chars().count() <= MAX {
        id.into()
    } else {
        let prefix: String = id.chars().take(12).collect();
        let suffix: String = id
            .chars()
            .rev()
            .take(10)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("{prefix}…{suffix}")
    }
}
