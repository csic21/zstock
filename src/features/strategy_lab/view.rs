use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, relative,
};
use gpui_component::{
    ActiveTheme, Disableable, PixelsExt, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex,
    scroll::ScrollableElement,
    v_flex,
};

use crate::app::StockApp;
use crate::domain::backtest::validation::PromotionConclusion;
use crate::domain::dataset::DatasetManifest;
use crate::domain::experiment::{ExperimentRecord, ExperimentStatus};

use super::presenter::{StrategyLabLayout, leaderboard};
use super::state::{StrategyLabPage, StrategyLabState, TemplateFamily};

impl StockApp {
    pub(crate) fn render_strategy_lab(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = &self.strategy_lab_feature.state;
        let layout = StrategyLabLayout::for_width(window.bounds().size.width.as_f32());
        let selected_experiment = state.selected_experiment_id.as_deref();
        let selected_dataset = selected_manifest(state);
        let navigation = h_flex().w_full().gap_1().flex_wrap().children(
            StrategyLabPage::ALL
                .into_iter()
                .enumerate()
                .map(|(index, page)| {
                    let active = state.page == page;
                    let available = page_available(page, state);
                    Button::new(("strategy-lab-page", index))
                        .xsmall()
                        .when(active, |button| button.primary())
                        .when(!active, |button| button.ghost())
                        .disabled(!available)
                        .label(format!("{}  {}", index + 1, page.label()))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.strategy_lab_set_page(page, cx);
                        }))
                }),
        );

        let status_color = if state.busy {
            cx.theme().accent
        } else if state.status.contains("失败")
            || state.status.contains("无法")
            || state.status.contains("错误")
        {
            cx.theme().danger
        } else if state.status.contains("已") {
            cx.theme().success
        } else {
            cx.theme().warning
        };

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
                    .bg(cx.theme().sidebar)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .gap_3()
                            .flex_wrap()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_lg()
                                                    .font_semibold()
                                                    .child("AI 策略实验室"),
                                            )
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_full()
                                                    .bg(cx.theme().success.opacity(0.10))
                                                    .text_xs()
                                                    .text_color(cx.theme().success)
                                                    .child("本地确定性验证"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("先冻结数据，再审核策略，最后运行验证；AI 不参与排名与晋级"),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .items_end()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("当前实验"),
                                    )
                                    .child(
                                        div().text_sm().font_semibold().child(
                                            selected_experiment
                                                .map(short_id)
                                                .unwrap_or_else(|| "尚未创建".into()),
                                        ),
                                    )
                                    .when_some(selected_dataset, |column, dataset| {
                                        column.child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(format!(
                                                    "{} 只股票 · {} 至 {}",
                                                    dataset.instruments.len(),
                                                    dataset.interval.start,
                                                    dataset.interval.end
                                                )),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("研究流程"),
                            )
                            .child(navigation)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                                    .child("未完成的阶段会保持锁定，完成当前步骤后自动进入下一步"),
                            ),
                    )
                    .when(!state.status.is_empty(), |column| {
                        column.child(
                            div()
                                .text_xs()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .border_1()
                                .border_color(status_color.opacity(0.28))
                                .bg(status_color.opacity(0.08))
                                .text_color(status_color)
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
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .px_4()
                            .py_4()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(1_180.0))
                                    .child(self.render_strategy_lab_page(layout, cx)),
                            ),
                    ),
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
            StrategyLabPage::Report => self.render_strategy_lab_report(layout, cx),
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
        let (current_code, current_market, watchlist_count, candle_count) =
            self.strategy_lab_data_context();
        let count_selector = h_flex().gap_2().flex_wrap().children((3..=5).map(|count| {
            Button::new(("strategy-count", count))
                .xsmall()
                .when(form.strategy_count == count, |button| button.primary())
                .when(form.strategy_count != count, |button| button.ghost())
                .label(format!("{count} 个策略"))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.strategy_lab_set_count(count, cx);
                }))
        }));
        let family_selector = h_flex().gap_2().flex_wrap().children(
            [TemplateFamily::Generic, TemplateFamily::ScanPlaybooks]
                .into_iter()
                .enumerate()
                .map(|(index, family)| {
                    Button::new(("strategy-family", index))
                        .xsmall()
                        .when(form.template_family == family, |button| button.primary())
                        .when(form.template_family != family, |button| button.ghost())
                        .label(family.label())
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.strategy_lab_set_template_family(family, cx);
                        }))
                }),
        );

        let setup = v_flex()
            .flex_1()
            .min_w_0()
            .gap_4()
            .child(section_title(
                "创建新实验",
                "先选择数据范围。创建后会自动生成本地草案，并带你进入审核页。",
                cx,
            ))
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().accent.opacity(0.35))
                    .bg(cx.theme().accent.opacity(0.06))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(div().font_semibold().child("自选股票池"))
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_full()
                                                    .bg(cx.theme().accent.opacity(0.14))
                                                    .text_xs()
                                                    .text_color(cx.theme().accent)
                                                    .child("推荐"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("用于跨标的比较，结果更接近正式策略研究。"),
                                    ),
                            )
                            .child(
                                Button::new("strategy-create-watchlist")
                                    .primary()
                                    .disabled(state.busy || watchlist_count == 0)
                                    .label(if state.busy {
                                        "正在冻结…".into()
                                    } else {
                                        format!("冻结 {watchlist_count} 只并创建")
                                    })
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.strategy_lab_create_watchlist_pool(cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "当前市场：{current_market} · 最多读取 100 只 · 后台拉取约 1000 根前复权日 K"
                            )),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(div().font_semibold().child("当前标的快速试验"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("适合先熟悉流程；单股结果不能用于跨标的结论。"),
                                    ),
                            )
                            .child(
                                Button::new("strategy-create-current")
                                    .ghost()
                                    .disabled(state.busy || candle_count < 30)
                                    .label("用当前标的创建")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.strategy_lab_create_current(cx);
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if candle_count >= 30 {
                                cx.theme().muted_foreground
                            } else {
                                cx.theme().warning
                            })
                            .child(format!(
                                "{current_code} · 已加载 {candle_count} 根日 K{}",
                                if candle_count >= 30 {
                                    ""
                                } else {
                                    " · 至少需要 30 根，请先回到研究页加载行情"
                                }
                            )),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(section_title(
                        "本次研究设定",
                        "这些参数会写入实验版本；策略数量和模板来源可在创建前调整。",
                        cx,
                    ))
                    .child(family_selector)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(form.template_family.hint()),
                    )
                    .child(count_selector)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .flex_wrap()
                            .child(metric_tile("研究目标", form.goal.clone(), cx))
                            .child(metric_tile(
                                "最大回撤预算",
                                format!("{:.1}%", form.max_drawdown_pct),
                                cx,
                            ))
                            .child(metric_tile(
                                "初始资金",
                                format!("¥{:.0}", form.initial_cash),
                                cx,
                            ))
                            .child(metric_tile("基准", "冻结股票池等权", cx)),
                    ),
            );

        let history = v_flex()
            .w_full()
            .when(!layout.actions_stacked(), |column| {
                column.w(px(350.0)).flex_shrink_0()
            })
            .gap_3()
            .child(section_title(
                "继续已有实验",
                "选择记录后，可从已完成的阶段继续。",
                cx,
            ))
            .when(state.experiments.is_empty(), |column| {
                column.child(empty_state(
                    "还没有实验",
                    "从左侧选择一个数据范围开始。",
                    cx,
                ))
            })
            .children(
                state
                    .experiments
                    .iter()
                    .enumerate()
                    .map(|(index, experiment)| {
                        let id = experiment.definition.id.clone();
                        let selected = state.selected_experiment_id.as_deref() == Some(id.as_str());
                        let instrument_count = state
                            .datasets
                            .iter()
                            .find(|dataset| dataset.id == experiment.definition.dataset_id)
                            .map(|dataset| dataset.instruments.len())
                            .unwrap_or(0);
                        let status_color = match experiment.status {
                            ExperimentStatus::Completed => cx.theme().success,
                            ExperimentStatus::Running => cx.theme().accent,
                            ExperimentStatus::Cancelled => cx.theme().warning,
                            ExperimentStatus::Failed => cx.theme().danger,
                            ExperimentStatus::Draft => cx.theme().muted_foreground,
                        };
                        div()
                            .id(("strategy-experiment", index))
                            .w_full()
                            .cursor_pointer()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(if selected {
                                cx.theme().accent.opacity(0.45)
                            } else {
                                cx.theme().border
                            })
                            .when(selected, |row| row.bg(cx.theme().accent.opacity(0.07)))
                            .hover(|row| row.bg(cx.theme().accent.opacity(0.05)))
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .child(experiment_created_date(experiment)),
                                            )
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_0p5()
                                                    .rounded_full()
                                                    .bg(status_color.opacity(0.10))
                                                    .text_xs()
                                                    .text_color(status_color)
                                                    .child(experiment_status_label(
                                                        experiment.status,
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(
                                                "{} 只股票 · {} 个策略 · {}",
                                                instrument_count,
                                                experiment.definition.strategy_ids.len(),
                                                short_id(&experiment.definition.id)
                                            )),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.strategy_lab_select_experiment(id.clone(), cx);
                            }))
                    }),
            )
            .when_some(
                state.selected_experiment_id.as_deref().and_then(|id| {
                    state
                        .experiments
                        .iter()
                        .find(|item| item.definition.id == id)
                }),
                |column, experiment| {
                    column.child(
                        v_flex()
                            .gap_2()
                            .p_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(cx.theme().accent.opacity(0.35))
                            .bg(cx.theme().accent.opacity(0.06))
                            .child(div().text_sm().font_semibold().child("当前实验的下一步"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "已有 {} 个策略草案。AI 重新生成会替换当前草案版本。",
                                        experiment.definition.strategy_ids.len()
                                    )),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .child(
                                        Button::new("strategy-continue-drafts")
                                            .primary()
                                            .label("审核策略草案")
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.strategy_lab_set_page(
                                                    StrategyLabPage::Drafts,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new("strategy-create-ai")
                                            .ghost()
                                            .disabled(state.busy)
                                            .label(if state.busy {
                                                "AI 生成中…"
                                            } else {
                                                "AI 替换当前草案"
                                            })
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.strategy_lab_generate_ai(cx);
                                            })),
                                    ),
                            ),
                    )
                },
            );

        if layout.actions_stacked() {
            v_flex()
                .w_full()
                .gap_5()
                .child(setup)
                .child(history)
                .into_any_element()
        } else {
            h_flex()
                .w_full()
                .items_start()
                .gap_5()
                .child(setup)
                .child(history)
                .into_any_element()
        }
    }

    fn render_strategy_lab_drafts(
        &self,
        layout: StrategyLabLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let universe = selected_manifest(state).map(|dataset| {
            (
                dataset,
                self.strategy_lab_instrument_labels(&dataset.instruments),
            )
        });
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
        let title = section_title(
            "审核策略草案",
            "先确认假设和仓位规则，再开始耗时的批量验证。",
            cx,
        );
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("strategy-regenerate-ai")
                    .ghost()
                    .disabled(state.drafts.is_empty() || state.busy)
                    .label(if state.busy {
                        "生成中…"
                    } else {
                        "让 AI 重新生成"
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.strategy_lab_generate_ai(cx);
                    })),
            )
            .child(
                Button::new("strategy-run")
                    .primary()
                    .disabled(state.drafts.is_empty() || state.busy)
                    .label(if state.busy {
                        "运行中…"
                    } else {
                        "确认草案并运行"
                    })
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.strategy_lab_start_run(cx);
                    })),
            );
        let heading = if layout.actions_stacked() {
            v_flex()
                .gap_2()
                .child(title)
                .child(actions)
                .into_any_element()
        } else {
            h_flex()
                .w_full()
                .items_end()
                .justify_between()
                .gap_3()
                .child(title)
                .child(actions)
                .into_any_element()
        };
        v_flex()
            .w_full()
            .gap_4()
            .child(heading)
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(cx.theme().muted.opacity(0.55))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(versions.clone()),
            )
            .when_some(universe, |column, (dataset, labels)| {
                column.child(dataset_universe_card(dataset, labels, cx))
            })
            .when(state.drafts.is_empty(), |column| {
                column.child(empty_state(
                    "暂无策略草案",
                    "回到实验配置，先冻结当前标的或自选股票池。",
                    cx,
                ))
            })
            .children(state.drafts.iter().enumerate().map(|(index, draft)| {
                let versions = versions.clone();
                let source_color = if draft.source.starts_with("AI") {
                    cx.theme().accent
                } else {
                    cx.theme().muted_foreground
                };
                v_flex()
                    .w_full()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar.opacity(0.35))
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .justify_between()
                            .gap_3()
                            .child(div().font_semibold().child(format!(
                                "{:02}  {}",
                                index + 1,
                                draft.spec.name
                            )))
                            .child(
                                h_flex()
                                    .gap_1()
                                    .flex_wrap()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(source_color.opacity(0.10))
                                            .text_xs()
                                            .text_color(source_color)
                                            .child(draft.source.clone()),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(cx.theme().success.opacity(0.10))
                                            .text_xs()
                                            .text_color(cx.theme().success)
                                            .child(draft.validation_message.clone()),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("研究假设"),
                            )
                            .child(div().text_sm().child(draft.spec.hypothesis.clone())),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .flex_wrap()
                            .child(metric_tile(
                                "入场复杂度",
                                format!("{} 个规则节点", draft.spec.entry.node_count()),
                                cx,
                            ))
                            .child(metric_tile(
                                "退出复杂度",
                                format!("{} 个规则节点", draft.spec.exit.node_count()),
                                cx,
                            ))
                            .child(metric_tile(
                                "单次仓位",
                                format!("{:.0}%", draft.spec.position.size_pct),
                                cx,
                            ))
                            .child(metric_tile(
                                "最大持仓",
                                format!("{} 个", draft.spec.position.max_positions),
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "策略版本 {} · {}",
                                short_id(&draft.strategy_id),
                                versions
                            )),
                    )
            }))
            .into_any_element()
    }

    fn render_strategy_lab_progress(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let progress = state.progress.as_ref();
        let universe = selected_manifest(state).map(|dataset| {
            (
                dataset,
                self.strategy_lab_instrument_labels(&dataset.instruments),
            )
        });
        let progress_fraction = progress
            .map(|value| {
                if value.total_strategies == 0 {
                    0.0
                } else {
                    value.completed_strategies as f32 / value.total_strategies as f32
                }
            })
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let progress_label = progress
            .map(|value| {
                format!(
                    "已完成 {} / {} 个策略",
                    value.completed_strategies, value.total_strategies
                )
            })
            .unwrap_or_else(|| "尚未开始运行".into());
        v_flex()
            .w_full()
            .max_w(px(960.0))
            .gap_4()
            .child(section_title(
                "运行确定性验证",
                "任务在后台执行，可以离开本页；取消时会保留已经完成的报告。",
                cx,
            ))
            .when_some(universe, |column, (dataset, labels)| {
                column.child(dataset_universe_card(dataset, labels, cx))
            })
            .child(
                v_flex()
                    .w_full()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(if state.busy {
                        cx.theme().accent.opacity(0.35)
                    } else {
                        cx.theme().border
                    })
                    .bg(cx.theme().sidebar.opacity(0.40))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(div().text_sm().font_semibold().child(progress_label))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(
                                                progress
                                                    .and_then(|value| {
                                                        value.current_strategy_id.as_deref()
                                                    })
                                                    .map(|id| {
                                                        format!("正在验证策略 {}", short_id(id))
                                                    })
                                                    .unwrap_or_else(|| {
                                                        if state.busy {
                                                            "正在准备数据与策略…".into()
                                                        } else {
                                                            "运行完成后可进入排行榜比较结果".into()
                                                        }
                                                    }),
                                            ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_2xl()
                                    .font_semibold()
                                    .text_color(if state.busy {
                                        cx.theme().accent
                                    } else {
                                        cx.theme().foreground
                                    })
                                    .child(format!("{:.0}%", progress_fraction * 100.0)),
                            ),
                    )
                    .child(
                        div()
                            .h(px(10.0))
                            .w_full()
                            .rounded_full()
                            .overflow_hidden()
                            .bg(cx.theme().muted)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(progress_fraction))
                                    .rounded_full()
                                    .bg(cx.theme().accent),
                            ),
                    )
                    .when_some(progress, |panel, value| {
                        panel.child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .flex_wrap()
                                .child(metric_tile(
                                    "交易日进度",
                                    format!(
                                        "{} / {}",
                                        value.completed_sessions, value.total_sessions
                                    ),
                                    cx,
                                ))
                                .child(metric_tile(
                                    "缓存复用",
                                    format!("{} 份报告", value.cached_reports),
                                    cx,
                                ))
                                .child(metric_tile(
                                    "隔离失败",
                                    format!("{} 个", state.failures.len()),
                                    cx,
                                )),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .when(state.busy, |row| {
                        row.child(
                            Button::new("strategy-cancel")
                                .danger()
                                .label("取消当前任务")
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.strategy_lab_cancel(cx);
                                })),
                        )
                    })
                    .when(!state.busy && !state.reports.is_empty(), |row| {
                        row.child(
                            Button::new("strategy-view-results")
                                .primary()
                                .label("查看排行榜")
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.strategy_lab_set_page(StrategyLabPage::Leaderboard, cx);
                                })),
                        )
                    }),
            )
            .when(!state.failures.is_empty(), |column| {
                column.child(section_title(
                    "已隔离的问题",
                    "单个策略失败不会中断其他策略；可根据原因决定是否重建实验。",
                    cx,
                ))
            })
            .children(state.failures.iter().map(|failure| {
                warning_card(
                    "策略验证失败",
                    format!("{} · {}", short_id(&failure.strategy_id), failure.message),
                    cx,
                )
            }))
            .into_any_element()
    }

    fn render_strategy_lab_leaderboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.strategy_lab_feature.state;
        let rows = leaderboard(&state.reports, &state.robustness);
        let universe = selected_manifest(state).map(|dataset| {
            (
                dataset,
                self.strategy_lab_instrument_labels(&dataset.instruments),
            )
        });
        v_flex()
            .w_full()
            .gap_4()
            .child(
                h_flex()
                    .w_full()
                    .items_end()
                    .justify_between()
                    .gap_3()
                    .flex_wrap()
                    .child(section_title(
                        "策略排行榜",
                        "优先比较成本后超额收益，同时保留回撤、样本量和硬门槛结论。",
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("共 {} 份完成报告", rows.len())),
                    ),
            )
            .when_some(universe, |column, (dataset, labels)| {
                column.child(dataset_universe_card(dataset, labels, cx))
            })
            .when(rows.is_empty(), |column| {
                column.child(empty_state(
                    "还没有可比较的结果",
                    "审核策略草案并完成批量运行后，排行榜会自动生成。",
                    cx,
                ))
            })
            .children(rows.into_iter().enumerate().map(|(index, row)| {
                let strategy_id = row.strategy_id.clone();
                let selected =
                    state.selected_strategy_id.as_deref() == Some(row.strategy_id.as_str());
                let strategy_name = state
                    .drafts
                    .iter()
                    .find(|draft| draft.strategy_id == row.strategy_id)
                    .map(|draft| draft.spec.name.clone())
                    .unwrap_or_else(|| format!("策略 {}", short_id(&row.strategy_id)));
                let reason = row
                    .reasons
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "全部确定性门槛通过".into());
                let conclusion_color = match row.conclusion.as_str() {
                    "模拟盘候选" => cx.theme().success,
                    "淘汰" => cx.theme().danger,
                    _ => cx.theme().warning,
                };
                div()
                    .id(("strategy-leaderboard", index))
                    .w_full()
                    .cursor_pointer()
                    .p_4()
                    .rounded_lg()
                    .border_1()
                    .border_color(if selected {
                        cx.theme().accent.opacity(0.55)
                    } else {
                        cx.theme().border
                    })
                    .when(selected, |item| item.bg(cx.theme().accent.opacity(0.07)))
                    .hover(|item| item.bg(cx.theme().accent.opacity(0.05)))
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .justify_between()
                            .gap_3()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .w(px(42.0))
                                            .h(px(42.0))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_lg()
                                            .bg(cx.theme().muted)
                                            .font_semibold()
                                            .child(format!("#{:02}", index + 1)),
                                    )
                                    .child(
                                        v_flex()
                                            .gap_0p5()
                                            .child(div().font_semibold().child(strategy_name))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(short_id(&row.strategy_id)),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .flex_wrap()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(cx.theme().muted)
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(row.evidence),
                                    )
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(conclusion_color.opacity(0.10))
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(conclusion_color)
                                            .child(row.conclusion),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .mt_3()
                            .flex_wrap()
                            .child(metric_tile(
                                "总收益",
                                format!("{:+.2}%", row.return_pct),
                                cx,
                            ))
                            .child(metric_tile(
                                "成本后超额",
                                format!("{:+.2}%", row.excess_pct),
                                cx,
                            ))
                            .child(metric_tile(
                                "最大回撤",
                                format!("{:.2}%", row.drawdown_pct),
                                cx,
                            ))
                            .child(metric_tile("完成交易", format!("{} 笔", row.trades), cx)),
                    )
                    .child(
                        div()
                            .mt_2()
                            .text_xs()
                            .text_color(if reason == "全部确定性门槛通过" {
                                cx.theme().success
                            } else {
                                cx.theme().muted_foreground
                            })
                            .child(reason),
                    )
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.strategy_lab_select_report(strategy_id.clone(), cx);
                    }))
            }))
            .into_any_element()
    }

    fn render_strategy_lab_report(
        &self,
        layout: StrategyLabLayout,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        let robustness = state
            .robustness
            .iter()
            .find(|item| item.strategy_id == report.strategy_id);
        let robustness_summary = robustness
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
            .unwrap_or_else(|| "数据区间不足，未生成完整稳健性报告；不能晋级。".into());
        let paper_eligible = robustness
            .map(|item| item.promotion.conclusion == PromotionConclusion::PaperCandidate)
            .unwrap_or(false);
        let strategy_name = state
            .drafts
            .iter()
            .find(|draft| draft.strategy_id == report.strategy_id)
            .map(|draft| draft.spec.name.clone())
            .unwrap_or_else(|| format!("策略 {}", short_id(&report.strategy_id)));
        let report_universe = state
            .datasets
            .iter()
            .find(|dataset| dataset.id == report.dataset_id)
            .map(|dataset| {
                (
                    dataset,
                    self.strategy_lab_instrument_labels(&dataset.instruments),
                )
            });

        let evidence_details = v_flex()
            .flex_1()
            .min_w_0()
            .gap_3()
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
            .when_some(report_universe, |column, (dataset, labels)| {
                column.child(dataset_universe_card(dataset, labels, cx))
            })
            .child(info_card("逐日资金曲线", equity_summary, cx))
            .child(info_card("年度分组", yearly, cx))
            .child(info_card("市场状态分组", regime, cx));

        let validation_details = v_flex()
            .w_full()
            .when(!layout.actions_stacked(), |column| column.w(px(410.0)).flex_shrink_0())
            .gap_3()
            .child(info_card("样本与稳健性", robustness_summary, cx))
            .child(if let Some(result) = sealed_test {
                info_card(
                    "封存测试 · 已消费",
                    format!(
                        "{} · 收益 {:+.2}% · 超额 {:+.2}% · 回撤 {:.2}% · {} 笔",
                        result.consumed_at,
                        result.report.metrics.total_return_pct,
                        result.report.metrics.excess_return_pct,
                        result.report.metrics.max_drawdown_pct.abs(),
                        result.report.metrics.trade_count
                    ),
                    cx,
                )
            } else {
                warning_card(
                    "封存测试 · 尚未查看",
                    "这是一次性操作。只应在完成训练、验证和候选选择后查看；查看结果后不能让 AI 覆盖同一策略。",
                    cx,
                )
            })
            .child(info_card(
                "AI 解释",
                state.ai_explanation.clone().unwrap_or_else(|| {
                    "当前没有 AI 解释；不影响本地报告与确定性评级。".into()
                }),
                cx,
            ));

        let detail_layout = if layout.actions_stacked() {
            v_flex()
                .w_full()
                .gap_3()
                .child(evidence_details)
                .child(validation_details)
                .into_any_element()
        } else {
            h_flex()
                .w_full()
                .items_start()
                .gap_4()
                .child(evidence_details)
                .child(validation_details)
                .into_any_element()
        };

        v_flex()
            .w_full()
            .gap_4()
            .child(
                h_flex()
                    .w_full()
                    .items_end()
                    .justify_between()
                    .gap_3()
                    .flex_wrap()
                    .child(section_title(
                        strategy_name,
                        format!("证据报告 · {}", short_id(&report.strategy_id)),
                        cx,
                    ))
                    .child(
                        Button::new("strategy-back-leaderboard")
                            .ghost()
                            .label("返回排行榜")
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.strategy_lab_set_page(StrategyLabPage::Leaderboard, cx);
                            })),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .flex_wrap()
                    .child(metric_tile(
                        "总收益",
                        format!("{:+.2}%", report.metrics.total_return_pct),
                        cx,
                    ))
                    .child(metric_tile(
                        "基准收益",
                        format!("{:+.2}%", report.metrics.benchmark_return_pct),
                        cx,
                    ))
                    .child(metric_tile(
                        "成本后超额",
                        format!("{:+.2}%", report.metrics.excess_return_pct),
                        cx,
                    ))
                    .child(metric_tile(
                        "最大回撤",
                        format!("{:.2}%", report.metrics.max_drawdown_pct.abs()),
                        cx,
                    ))
                    .child(metric_tile(
                        "Sharpe",
                        format!("{:.2}", report.metrics.sharpe),
                        cx,
                    ))
                    .child(metric_tile(
                        "交易 / 成本",
                        format!("{} 笔 / ¥{:.2}", report.metrics.trade_count, report.metrics.total_cost),
                        cx,
                    )),
            )
            .child(detail_layout)
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .when(sealed_test.is_none(), |row| {
                        row.child(
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
                        Button::new("strategy-promote-paper")
                            .primary()
                            .disabled(!paper_eligible || state.busy)
                            .label(if paper_eligible {
                                "锁定版本并加入模拟观察"
                            } else {
                                "未通过模拟候选门槛"
                            })
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
            .gap_4()
            .child(
                h_flex()
                    .w_full()
                    .items_end()
                    .justify_between()
                    .gap_3()
                    .flex_wrap()
                    .child(section_title(
                        "模拟观察",
                        "只跟踪通过硬门槛且版本已锁定的策略；这里不会连接券商或真实下单。",
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} 个候选", candidates.len())),
                    ),
            )
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
                column.child(empty_state(
                    "还没有模拟候选",
                    "回到排行榜查看淘汰原因。只有通过全部门槛的版本才能从证据报告加入这里。",
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

fn selected_manifest(state: &StrategyLabState) -> Option<&DatasetManifest> {
    let experiment_id = state.selected_experiment_id.as_deref()?;
    let dataset_id = &state
        .experiments
        .iter()
        .find(|experiment| experiment.definition.id == experiment_id)?
        .definition
        .dataset_id;
    state
        .datasets
        .iter()
        .find(|dataset| &dataset.id == dataset_id)
}

fn dataset_universe_card(
    dataset: &DatasetManifest,
    labels: Vec<String>,
    cx: &mut Context<StockApp>,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().accent.opacity(0.30))
        .bg(cx.theme().accent.opacity(0.05))
        .child(
            h_flex()
                .w_full()
                .items_start()
                .justify_between()
                .gap_3()
                .flex_wrap()
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(format!("本次实际回测股票 · {} 只", labels.len())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "冻结区间 {} 至 {} · 数据集 {}",
                                    dataset.interval.start,
                                    dataset.interval.end,
                                    short_id(&dataset.id)
                                )),
                        ),
                )
                .child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(cx.theme().success.opacity(0.12))
                        .text_xs()
                        .text_color(cx.theme().success)
                        .child("股票池已冻结"),
                ),
        )
        .child(
            div()
                .w_full()
                .max_h(px(150.0))
                .overflow_y_scrollbar()
                .child(h_flex().w_full().gap_1().flex_wrap().children(
                    labels.into_iter().enumerate().map(|(index, label)| {
                        div()
                            .id(("strategy-universe-symbol", index))
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().background)
                            .text_xs()
                            .child(label)
                    }),
                )),
        )
        .into_any_element()
}

fn page_available(page: StrategyLabPage, state: &StrategyLabState) -> bool {
    match page {
        StrategyLabPage::Configure => true,
        StrategyLabPage::Drafts => {
            state.selected_experiment_id.is_some() && !state.drafts.is_empty()
        }
        StrategyLabPage::Progress => {
            state.busy || state.progress.is_some() || !state.reports.is_empty()
        }
        StrategyLabPage::Leaderboard => !state.reports.is_empty(),
        StrategyLabPage::Report => state.selected_report().is_some(),
        StrategyLabPage::PaperCandidates => !state.paper_candidates.is_empty(),
    }
}

fn experiment_created_date(experiment: &ExperimentRecord) -> String {
    experiment.created_at.chars().take(10).collect()
}

const fn experiment_status_label(status: ExperimentStatus) -> &'static str {
    match status {
        ExperimentStatus::Draft => "待运行",
        ExperimentStatus::Running => "运行中",
        ExperimentStatus::Completed => "已完成",
        ExperimentStatus::Cancelled => "已取消",
        ExperimentStatus::Failed => "失败",
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
        .bg(cx.theme().sidebar.opacity(0.28))
        .child(div().text_sm().font_semibold().child(title.into()))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(body.into()),
        )
        .into_any_element()
}

fn metric_tile(
    label: impl Into<String>,
    value: impl Into<String>,
    cx: &mut Context<StockApp>,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_w(px(145.0))
        .gap_0p5()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().sidebar.opacity(0.32))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.into()),
        )
        .child(div().text_sm().font_semibold().child(value.into()))
        .into_any_element()
}

fn empty_state(
    title: impl Into<String>,
    body: impl Into<String>,
    cx: &mut Context<StockApp>,
) -> AnyElement {
    v_flex()
        .w_full()
        .items_center()
        .gap_1()
        .px_4()
        .py_5()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted.opacity(0.28))
        .child(div().text_sm().font_semibold().child(title.into()))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(body.into()),
        )
        .into_any_element()
}

fn warning_card(
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
        .border_color(cx.theme().warning.opacity(0.30))
        .bg(cx.theme().warning.opacity(0.07))
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().warning)
                .child(title.into()),
        )
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
