use crate::data::ai::{AiCliProvider, AiKind, AiTransport};
use crate::domain::dataset::FrozenSeries;
use crate::domain::market::{Adjustment, AssetType, CandleRecord, InstrumentId, Market};
use crate::features::strategy_lab::state::StrategyLabPage;
use crate::features::strategy_lab::{StrategyLabFeature, StrategyLabWorkerResult};
use crate::infrastructure::ai::HttpLlmConfig;
use crate::infrastructure::ai::cli::{CliLlmClient, CliLlmConfig, CliProvider};
use crate::infrastructure::ai::compatible_chat::CompatibleChatClient;
use crate::infrastructure::ai::openai::OpenAiResponsesClient;
use crate::infrastructure::market::eastmoney::EastmoneyProvider;
use crate::services::llm::{LlmClient, LlmRequest, LlmResponse};
use crate::services::market_data::KlineProvider;
use crate::services::strategy_generator::{StrategyBatchDraft, StrategyGenerator};

impl super::StockApp {
    pub(crate) fn strategy_lab_data_context(&self) -> (String, &'static str, usize, usize) {
        let code = self.selected.to_string();
        let market = Market::for_code(&code).unwrap_or(Market::AShare);
        let market_label = match market {
            Market::AShare => "A 股",
            Market::HongKong => "港股",
        };
        let watchlist_count = self
            .symbols
            .iter()
            .filter(|symbol| Market::for_code(&symbol.code) == Some(market))
            .count()
            .min(100);
        (code, market_label, watchlist_count, self.candles.len())
    }

    pub(crate) fn strategy_lab_instrument_labels(
        &self,
        instruments: &[InstrumentId],
    ) -> Vec<String> {
        instruments
            .iter()
            .map(|instrument| {
                self.symbols
                    .iter()
                    .find(|symbol| symbol.code == instrument.code)
                    .map(|symbol| {
                        let name = symbol.name.trim();
                        if name.is_empty() || name == instrument.code {
                            instrument.code.clone()
                        } else {
                            format!("{} {}", instrument.code, name)
                        }
                    })
                    .unwrap_or_else(|| instrument.code.clone())
            })
            .collect()
    }

    pub(crate) fn strategy_lab_start_daily_observation(&mut self, cx: &mut gpui::Context<Self>) {
        let today = chrono::Local::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        let already_current = !self.strategy_lab_feature.state.paper_candidates.is_empty()
            && self
                .strategy_lab_feature
                .state
                .paper_candidates
                .iter()
                .all(|candidate| {
                    self.strategy_lab_feature
                        .state
                        .paper_runs
                        .iter()
                        .any(|run| run.candidate_id == candidate.id && run.as_of == today)
                });
        if !self.strategy_lab_feature.state.paper_candidates.is_empty() && !already_current {
            self.strategy_lab_run_paper(cx);
        }
    }

    pub(crate) fn strategy_lab_set_page(
        &mut self,
        page: StrategyLabPage,
        cx: &mut gpui::Context<Self>,
    ) {
        self.strategy_lab_feature.state.page = page;
        cx.notify();
    }

    pub(crate) fn strategy_lab_set_count(&mut self, count: usize, cx: &mut gpui::Context<Self>) {
        self.strategy_lab_feature.state.form.strategy_count = count.clamp(3, 8);
        cx.notify();
    }

    pub(crate) fn strategy_lab_set_template_family(
        &mut self,
        family: crate::features::strategy_lab::state::TemplateFamily,
        cx: &mut gpui::Context<Self>,
    ) {
        self.strategy_lab_feature.state.form.template_family = family;
        if family == crate::features::strategy_lab::state::TemplateFamily::ScanPlaybooks {
            self.strategy_lab_feature.state.form.strategy_count = 4;
        }
        cx.notify();
    }

    pub(crate) fn strategy_lab_create_current(&mut self, cx: &mut gpui::Context<Self>) {
        let code = self.selected.to_string();
        let market = Market::for_code(&code).unwrap_or(Market::AShare);
        let candles = self
            .candles
            .iter()
            .map(|candle| CandleRecord {
                time: candle.date.to_string(),
                open: candle.open,
                high: candle.high,
                low: candle.low,
                close: candle.close,
                volume: candle.volume,
            })
            .collect();
        let series = FrozenSeries {
            instrument: InstrumentId {
                market,
                asset_type: AssetType::Stock,
                code,
            },
            source: "zstock-ui-current-series-v1".into(),
            adjustment: Adjustment::Forward,
            candles,
        };
        if let Err(error) = self.strategy_lab_feature.create_local_experiment(series) {
            self.strategy_lab_feature.state.status = format!("创建实验失败：{error:#}");
        }
        cx.notify();
    }

    pub(crate) fn strategy_lab_generate_ai(&mut self, cx: &mut gpui::Context<Self>) {
        let (experiment_id, input) = match self.strategy_lab_feature.prepare_ai_generation() {
            Ok(value) => value,
            Err(error) => {
                self.strategy_lab_feature.state.status = format!("无法生成策略：{error:#}");
                cx.notify();
                return;
            }
        };
        let config = self.ai_config.clone();
        cx.spawn(async move |this, cx| {
            let batch: StrategyBatchDraft = smol::unblock(move || {
                let client = configured_strategy_client(&config);
                StrategyGenerator::new(client.as_ref()).generate(&input)
            })
            .await;
            let _ = this.update(cx, |app, cx| {
                if let Err(error) = app
                    .strategy_lab_feature
                    .apply_ai_generation(&experiment_id, batch)
                {
                    app.strategy_lab_feature
                        .fail_ai_generation(format!("{error:#}"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn strategy_lab_create_watchlist_pool(&mut self, cx: &mut gpui::Context<Self>) {
        if self.strategy_lab_feature.state.busy {
            return;
        }
        let market = Market::for_code(self.selected.as_ref()).unwrap_or(Market::AShare);
        let codes: Vec<_> = self
            .symbols
            .iter()
            .filter(|symbol| Market::for_code(&symbol.code) == Some(market))
            .take(100)
            .map(|symbol| symbol.code.clone())
            .collect();
        if codes.is_empty() {
            self.strategy_lab_feature.state.status = "当前市场的自选股票池为空".into();
            cx.notify();
            return;
        }
        self.strategy_lab_feature.state.busy = true;
        self.strategy_lab_feature.state.status = format!("正在后台冻结自选池：0/{}…", codes.len());
        cx.spawn(async move |this, cx| {
            let (series, failures) = smol::unblock(move || {
                let provider = EastmoneyProvider;
                let mut series = Vec::new();
                let mut failures = Vec::new();
                for code in codes {
                    match provider.fetch_klines(&code, 1_000) {
                        Ok(fetched) if fetched.candles.len() >= 30 => series.push(FrozenSeries {
                            instrument: InstrumentId {
                                market: fetched.market,
                                asset_type: AssetType::Stock,
                                code: fetched.code,
                            },
                            source: fetched.source,
                            adjustment: fetched.adjustment,
                            candles: fetched.candles,
                        }),
                        Ok(_) => failures.push(format!("{code}: 少于 30 根日 K")),
                        Err(error) => failures.push(format!("{code}: {error}")),
                    }
                }
                (series, failures)
            })
            .await;
            let _ = this.update(cx, |app, cx| {
                app.strategy_lab_feature.state.busy = false;
                match app
                    .strategy_lab_feature
                    .create_local_experiment_from_series(series)
                {
                    Ok(_) if failures.is_empty() => {}
                    Ok(_) => {
                        app.strategy_lab_feature
                            .state
                            .status
                            .push_str(&format!("；{} 只标的抓取失败，已隔离", failures.len()));
                    }
                    Err(error) => {
                        app.strategy_lab_feature.state.status =
                            format!("创建自选池实验失败：{error:#}")
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn strategy_lab_start_run(&mut self, cx: &mut gpui::Context<Self>) {
        let work = match self.strategy_lab_feature.prepare_run() {
            Ok(work) => work,
            Err(error) => {
                self.strategy_lab_feature.state.status = format!("无法开始实验：{error:#}");
                cx.notify();
                return;
            }
        };
        let runner = self.strategy_lab_feature.runner();
        cx.spawn(async move |this, cx| {
            let (progress_tx, progress_rx) = smol::channel::bounded(64);
            let worker = smol::spawn(smol::unblock(move || {
                StrategyLabFeature::execute(runner, work, |snapshot| {
                    let _ = progress_tx.send_blocking(snapshot);
                })
            }));
            while let Ok(snapshot) = progress_rx.recv().await {
                if this
                    .update(cx, |app, cx| {
                        app.strategy_lab_feature.apply_progress(snapshot);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result: StrategyLabWorkerResult = worker.await;
            let _ = this.update(cx, |app, cx| {
                if let Err(error) = app.strategy_lab_feature.finish_run(result) {
                    let message = format!("保存实验结果失败：{error:#}");
                    let _ = app.strategy_lab_feature.fail_run(message);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn strategy_lab_cancel(&mut self, cx: &mut gpui::Context<Self>) {
        self.strategy_lab_feature.cancel();
        cx.notify();
    }

    pub(crate) fn strategy_lab_select_experiment(
        &mut self,
        experiment_id: String,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Err(error) = self.strategy_lab_feature.select_experiment(experiment_id) {
            self.strategy_lab_feature.state.status = format!("打开实验失败：{error:#}");
        }
        cx.notify();
    }

    pub(crate) fn strategy_lab_select_report(
        &mut self,
        strategy_id: String,
        cx: &mut gpui::Context<Self>,
    ) {
        self.strategy_lab_feature.state.selected_strategy_id = Some(strategy_id);
        self.strategy_lab_feature.state.selected_trade_index = None;
        self.strategy_lab_feature.state.page = StrategyLabPage::Report;
        cx.notify();
    }

    pub(crate) fn strategy_lab_select_trade(&mut self, index: usize, cx: &mut gpui::Context<Self>) {
        self.strategy_lab_feature.state.selected_trade_index = Some(index);
        cx.notify();
    }

    pub(crate) fn strategy_lab_set_library_sort(
        &mut self,
        sort: crate::domain::strategy_library::LibrarySort,
        cx: &mut gpui::Context<Self>,
    ) {
        self.strategy_lab_feature.set_library_sort(sort);
        cx.notify();
    }

    pub(crate) fn strategy_lab_set_library_filter(
        &mut self,
        filter: crate::domain::strategy_library::LibraryFilter,
        cx: &mut gpui::Context<Self>,
    ) {
        self.strategy_lab_feature.set_library_filter(filter);
        cx.notify();
    }

    pub(crate) fn strategy_lab_dismiss_library(
        &mut self,
        record_id: String,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Err(error) = self.strategy_lab_feature.dismiss_library_record(record_id) {
            self.strategy_lab_feature.state.status = format!("无法删除策略库条目：{error:#}");
        }
        cx.notify();
    }

    pub(crate) fn strategy_lab_open_library(
        &mut self,
        record_id: String,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Err(error) = self.strategy_lab_feature.open_library_record(&record_id) {
            self.strategy_lab_feature.state.status = format!("无法打开策略库条目：{error:#}");
        }
        cx.notify();
    }

    pub(crate) fn strategy_lab_promote_paper(&mut self, cx: &mut gpui::Context<Self>) {
        if let Err(error) = self.strategy_lab_feature.promote_selected_to_paper() {
            self.strategy_lab_feature.state.status = format!("无法加入模拟观察：{error:#}");
        }
        cx.notify();
    }

    pub(crate) fn strategy_lab_consume_sealed_test(&mut self, cx: &mut gpui::Context<Self>) {
        let work = match self.strategy_lab_feature.prepare_sealed_test() {
            Ok(work) => work,
            Err(error) => {
                self.strategy_lab_feature.state.status = format!("无法查看封存测试：{error:#}");
                cx.notify();
                return;
            }
        };
        let experiment_id = work.experiment_id.clone();
        cx.spawn(async move |this, cx| {
            let result =
                smol::unblock(move || StrategyLabFeature::execute_sealed_test(&work)).await;
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(result) => {
                        if let Err(error) = app
                            .strategy_lab_feature
                            .finish_sealed_test(&experiment_id, result)
                        {
                            app.strategy_lab_feature
                                .fail_sealed_test(format!("{error:#}"));
                        }
                    }
                    Err(error) => app
                        .strategy_lab_feature
                        .fail_sealed_test(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn strategy_lab_run_paper(&mut self, cx: &mut gpui::Context<Self>) {
        let (store, candidates, as_of) = match self.strategy_lab_feature.prepare_paper_run() {
            Ok(work) => work,
            Err(error) => {
                self.strategy_lab_feature.state.status = format!("无法更新模拟观察：{error:#}");
                cx.notify();
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            let results = smol::unblock(move || {
                StrategyLabFeature::execute_paper_runs(store, candidates, as_of)
            })
            .await;
            let _ = this.update(cx, |app, cx| {
                if let Err(error) = app.strategy_lab_feature.finish_paper_runs(results) {
                    app.strategy_lab_feature.state.busy = false;
                    app.strategy_lab_feature.state.status = format!("保存模拟观察失败：{error:#}");
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn strategy_lab_export(&mut self, cx: &mut gpui::Context<Self>) {
        self.strategy_lab_feature.state.status = match self.strategy_lab_feature.save_export() {
            Ok(path) => format!("实验已导出：{}", path.display()),
            Err(error) => format!("导出失败：{error:#}"),
        };
        cx.notify();
    }
}

fn configured_strategy_client(config: &crate::data::ai::AiConfig) -> Box<dyn LlmClient> {
    if !config.is_configured() {
        return Box::new(UnavailableLlm);
    }
    match config.transport {
        AiTransport::Api => {
            let http = HttpLlmConfig {
                base_url: config.base_url.clone(),
                model: config.model.clone(),
                api_key: config.api_key.clone(),
                timeout_secs: config.timeout_secs,
                max_response_bytes: 256 * 1024,
            };
            match config.kind {
                AiKind::Responses => Box::new(OpenAiResponsesClient::new(http)),
                AiKind::Chat => Box::new(CompatibleChatClient::new(http)),
            }
        }
        AiTransport::Cli => Box::new(CliLlmClient::new(CliLlmConfig {
            provider: match config.cli_provider {
                AiCliProvider::Grok => CliProvider::Grok,
                AiCliProvider::Chatgpt => CliProvider::Chatgpt,
                AiCliProvider::Opencode => CliProvider::Opencode,
                AiCliProvider::Claude => CliProvider::Claude,
            },
            binary: config.cli_bin.clone(),
            model: config.model.clone(),
            timeout_secs: config.timeout_secs,
            max_response_bytes: 256 * 1024,
        })),
    }
}

struct UnavailableLlm;

impl LlmClient for UnavailableLlm {
    fn complete(&self, _request: &LlmRequest) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("未启用 AI 服务，使用本地模板兜底")
    }
}
