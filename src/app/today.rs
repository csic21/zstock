//! View-model composition and navigation for the task-oriented Today page.

use gpui::Context;

use crate::data::limitup::LimitVerdict;
use crate::data::radar::RadarStrategy;
use crate::data::scout::ScoutVerdict;
use crate::domain::climate::{
    ClimateEvidence, ClimateReport, IndexMove, NewEntryStance, PlaybookKind, assess_market_climate,
};
use crate::domain::journal::PlanStatus;
use crate::domain::market::CandleRecord;
use crate::domain::money::Currency;
use crate::domain::rule_ledger::{RuleLedgerReport, build_rule_ledger};
use crate::domain::strategy_application::{
    HoldingSnapshot, SizingLimits, StrategyStockPlan, apply_strategy_to_stock,
};
use crate::domain::today::{
    TodayAction, TodayActionTarget, TodayAlertSnapshot, TodayDashboard, TodayDashboardInput,
    TodayOpportunity, TodayPlanSnapshot, TodayRiskSnapshot, build_today_dashboard,
};
use crate::model::{Symbol, board_for_code, shared};

use super::{StockApp, state::PrimaryTask};

impl StockApp {
    pub(crate) fn market_climate_report(&self) -> ClimateReport {
        let mut report = assess_market_climate(&self.climate_evidence());
        if self.financial_recovery_required() {
            // Preserve market-only measurements, but never let a missing
            // portfolio or plan history yield an Open/100% allocation gate.
            let note = "本地持仓或日记尚未恢复；组合仓位、风险与新开仓结论不可用";
            report.stance = NewEntryStance::Freeze;
            report.risk_scale = 0.0;
            report.headline = "本地数据待恢复，暂停新仓计划".into();
            report.detail = format!("{note}。指数与市场宽度仍可用于行情研究。");
            report.reasons.insert(0, note.into());
        }
        report
    }

    pub(crate) fn climate_evidence(&self) -> ClimateEvidence {
        let open_count = if self.portfolio_recovery.is_none() && !self.recovery_busy {
            self.portfolio.positions().len()
        } else {
            0
        };
        self.climate_evidence_with_open_count(open_count)
    }

    fn climate_evidence_with_open_count(&self, open_positions: usize) -> ClimateEvidence {
        let indices = [
            ("上证综指", self.index_sh),
            ("沪深300", self.index_hs300),
            ("创业板指", self.index_cyb),
        ]
        .into_iter()
        .filter_map(|(name, snap)| {
            snap.map(|snap| IndexMove {
                name: name.into(),
                change_pct: snap.change_pct,
            })
        })
        .collect();
        let has_sectors = !self.market_analysis_sectors.is_empty();
        let stock_advances = has_sectors.then(|| {
            self.market_analysis_sectors
                .iter()
                .map(|sector| sector.advances)
                .sum()
        });
        let stock_declines = has_sectors.then(|| {
            self.market_analysis_sectors
                .iter()
                .map(|sector| sector.declines)
                .sum()
        });
        let stock_unchanged = has_sectors.then(|| {
            self.market_analysis_sectors
                .iter()
                .map(|sector| sector.unchanged)
                .sum()
        });
        let sector_advances = has_sectors.then(|| {
            self.market_analysis_sectors
                .iter()
                .filter(|sector| sector.change_pct > 0.0)
                .count() as u64
        });
        let sector_declines = has_sectors.then(|| {
            self.market_analysis_sectors
                .iter()
                .filter(|sector| sector.change_pct < 0.0)
                .count() as u64
        });
        let sector_unchanged = has_sectors.then(|| {
            self.market_analysis_sectors
                .iter()
                .filter(|sector| sector.change_pct == 0.0)
                .count() as u64
        });
        let sector_average_change = has_sectors.then(|| {
            self.market_analysis_sectors
                .iter()
                .map(|sector| sector.change_pct)
                .sum::<f64>()
                / self.market_analysis_sectors.len().max(1) as f64
        });
        ClimateEvidence {
            indices,
            stock_advances,
            stock_declines,
            stock_unchanged,
            sector_advances,
            sector_declines,
            sector_unchanged,
            sector_average_change,
            open_positions,
        }
    }

    pub(crate) fn today_dashboard_view_model(&self) -> Option<TodayDashboard> {
        if self.financial_recovery_required() {
            return None;
        }
        let summary = self.portfolio_summary();
        let risk_view = self.portfolio_risk_view(&summary);
        let now_millis = chrono::Utc::now().timestamp_millis();
        let alerts = self
            .buy_alerts
            .iter()
            .filter(|(_, alert)| alert.any_armed())
            .map(|(code, alert)| {
                let quote = self.quote_for_code(code);
                let symbol = self
                    .symbols
                    .iter()
                    .find(|symbol| symbol.code == *code)
                    .or_else(|| {
                        self.preview_symbol
                            .as_ref()
                            .filter(|symbol| symbol.code == *code)
                    });
                TodayAlertSnapshot {
                    code: code.clone(),
                    name: quote
                        .map(|quote| quote.name.clone())
                        .filter(|name| !name.is_empty())
                        .or_else(|| symbol.map(|symbol| symbol.name.to_string()))
                        .unwrap_or_else(|| code.clone()),
                    // Near-target warnings need current evidence. Retained triggered
                    // rules still appear with “等待有效行情” when their quote ages out.
                    last: quote
                        .filter(|quote| {
                            quote.usable()
                                && quote.effective_freshness(now_millis)
                                    == crate::domain::market::Freshness::Live
                        })
                        .and_then(|quote| quote.price)
                        .unwrap_or_default(),
                    buy_target: alert.is_valid().then_some(alert.target_price),
                    buy_triggered: alert.triggered,
                    sell_target: alert.sell_price,
                    sell_triggered: alert.sell_triggered,
                    stop: alert.stop_price,
                    stop_triggered: alert.stop_triggered,
                }
            })
            .collect();
        let risks = risk_view
            .items
            .into_iter()
            .map(|risk| TodayRiskSnapshot {
                code: risk.code,
                position_weight_pct: risk.position_weight_pct,
                risk_amount_label: risk
                    .risk_amount
                    .map(|amount| format!("{} {:.0}", amount.currency.symbol(), amount.major())),
                quote_stale: risk.quote_stale,
                invalidation_breached: risk.invalidation_breached,
            })
            .collect();
        let plans = self
            .journal
            .entries
            .iter()
            .filter_map(|entry| {
                let plan = entry.plan.as_ref()?;
                Some(TodayPlanSnapshot {
                    id: plan.id.clone(),
                    code: entry.code.clone(),
                    name: entry.name.clone(),
                    review_on: plan.review_on.clone(),
                    due: plan.status == PlanStatus::DueForReview,
                })
            })
            .collect();
        let mut opportunities = self
            .scout_picks
            .iter()
            .map(|pick| TodayOpportunity {
                code: pick.code.clone(),
                name: pick.name.clone(),
                strategy: "低位策略".into(),
                playbook: PlaybookKind::LowPosition,
                score: pick.buy_score,
                observation: pick.buy_band_text(),
                ready: pick.verdict == ScoutVerdict::BuyWatch
                    && pick.close >= pick.buy_low
                    && pick.close <= pick.buy_high * 1.01,
                gate_reason: None,
            })
            .collect::<Vec<_>>();
        opportunities.extend(self.radar_hits.iter().map(|hit| {
            let in_band = hit.close >= hit.watch_low && hit.close <= hit.watch_high;
            let chasing = hit.strategy == RadarStrategy::Breakout && hit.change_pct > 5.0;
            let still_falling =
                hit.strategy == RadarStrategy::OversoldBounce && hit.change_pct <= -3.0;
            TodayOpportunity {
                code: hit.code.clone(),
                name: hit.name.clone(),
                strategy: hit.strategy.label(false).into(),
                playbook: playbook_for_radar(hit.strategy),
                score: hit.score,
                observation: hit.watch_band_text(),
                ready: in_band && !chasing && !still_falling,
                gate_reason: if chasing {
                    Some("当日涨幅已大，突破后不追价".into())
                } else if still_falling {
                    Some("超跌仍在下行，先等止跌再观察".into())
                } else {
                    None
                },
            }
        }));
        // 连板梯队：封板质量分决定初始 readiness，气候门再按连板规则二次拦截。
        // 炸板/高板在这里直接判为不行动（只看不做），不进入 ready 队列。
        opportunities.extend(self.limitup_hits.iter().map(|hit| {
            let (ready, gate_reason) = match hit.verdict {
                LimitVerdict::FirstBoard | LimitVerdict::SecondBoard => {
                    if hit.score >= 60.0 {
                        (true, None)
                    } else {
                        (false, Some("封板质量不足，先观察不行动".into()))
                    }
                }
                LimitVerdict::WatchSecondEntry => (true, None),
                LimitVerdict::HigherBoard => (false, Some("高位连板只看不做".into())),
                LimitVerdict::BrokenBoard => (false, Some("炸板当日不追".into())),
                LimitVerdict::Skip => (false, Some("不符合连板纪律".into())),
            };
            TodayOpportunity {
                code: hit.code.clone(),
                name: hit.name.clone(),
                strategy: hit.verdict.label().into(),
                playbook: PlaybookKind::LimitUp,
                score: hit.score,
                observation: hit.price_band_text(),
                ready,
                gate_reason,
            }
        }));

        Some(build_today_dashboard(TodayDashboardInput {
            alerts,
            risks,
            plans,
            opportunities,
            climate: self.climate_evidence_with_open_count(summary.open_count),
            open_positions: summary.open_count,
        }))
    }

    pub(crate) fn rule_ledger_view_model(&self) -> Option<RuleLedgerReport> {
        if self.journal_recovery.is_some() || self.recovery_busy {
            return None;
        }
        Some(build_rule_ledger(&self.journal.entries))
    }

    pub(crate) fn champion_stock_plans(
        &self,
        cx: &Context<Self>,
    ) -> (Option<String>, Vec<StrategyStockPlan>) {
        if self.financial_recovery_required() {
            return (None, Vec::new());
        }
        let Some((strategy_name, compiled)) = self.strategy_lab_feature.compiled_champion() else {
            return (None, Vec::new());
        };
        let positions = self.portfolio.positions();
        let climate =
            assess_market_climate(&self.climate_evidence_with_open_count(positions.len()));
        let capital = super::helpers::parse_f64(&self.position_capital_input.read(cx).value())
            .unwrap_or(100_000.0);
        let risk_pct = super::helpers::parse_f64(&self.position_risk_pct_input.read(cx).value())
            .unwrap_or(1.0)
            * climate.risk_scale;
        let mut codes = Vec::new();
        for position in &positions {
            if position.is_open() && !codes.iter().any(|code| code == &position.code) {
                codes.push(position.code.clone());
            }
        }
        for symbol in &self.symbols {
            if !codes.iter().any(|code| code == &symbol.code) {
                codes.push(symbol.code.clone());
            }
        }
        codes.truncate(40);
        let max_position_pct = compiled.spec().position.size_pct.clamp(1.0, 20.0);
        let plans = codes
            .into_iter()
            .map(|code| {
                let position = positions.iter().find(|position| position.code == code);
                let name = self
                    .symbols
                    .iter()
                    .find(|symbol| symbol.code == code)
                    .map(|symbol| symbol.name.to_string())
                    .or_else(|| position.map(|position| position.name.clone()))
                    .unwrap_or_else(|| code.clone());
                let candles = self.daily_records_for(&code);
                let holding = position.and_then(|position| {
                    position.is_open().then(|| HoldingSnapshot {
                        shares: position.shares.floor().max(0.0) as u64,
                        avg_cost: position.avg_cost,
                        opened_on: self.portfolio.open_lot_opened_on(&code),
                    })
                });
                let currency = Currency::for_code(&code).unwrap_or(Currency::Cny);
                let star = code.starts_with("688") || code.starts_with("689");
                apply_strategy_to_stock(
                    &compiled,
                    &code,
                    &name,
                    candles.as_deref().unwrap_or(&[]),
                    holding.as_ref(),
                    SizingLimits {
                        capital,
                        risk_pct,
                        max_position_pct,
                        lot_size: if star {
                            1
                        } else if currency == Currency::Cny {
                            100
                        } else {
                            1
                        },
                        minimum_shares: if star {
                            200
                        } else if currency == Currency::Cny {
                            100
                        } else {
                            1
                        },
                        allow_new_entries: climate.stance != NewEntryStance::Freeze,
                    },
                )
            })
            .collect();
        (Some(strategy_name), plans)
    }

    fn daily_records_for(&self, code: &str) -> Option<Vec<CandleRecord>> {
        if self.selected.as_ref() == code && !self.current_daily_candles().is_empty() {
            return Some(candles_to_records(self.current_daily_candles()));
        }
        if let Some(key) =
            super::series_cache::SeriesCache::kline_key(super::types::ChartKind::DayK, code)
            && let Some(cached) = self.series_cache.get_klines(&key)
            && !cached.candles.is_empty()
        {
            return Some(candles_to_records(&cached.candles));
        }
        None
    }

    pub(crate) fn open_today_action(&mut self, action: TodayAction, cx: &mut Context<Self>) {
        self.open_today_target(action.target, action.code.as_deref(), cx);
    }

    pub(crate) fn open_today_opportunity(&mut self, code: &str, cx: &mut Context<Self>) {
        self.open_today_target(TodayActionTarget::Opportunities, Some(code), cx);
    }

    fn open_today_target(
        &mut self,
        target: TodayActionTarget,
        code: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let code = code.map(str::to_owned);
        match target {
            TodayActionTarget::Research => {
                self.set_primary_task(PrimaryTask::Research, cx);
                if let Some(code) = code {
                    self.ensure_today_symbol(&code);
                    self.select_symbol(shared(code), cx);
                }
            }
            TodayActionTarget::Portfolio => {
                self.set_primary_task(PrimaryTask::Portfolio, cx);
                if let Some(code) = code {
                    self.ensure_today_symbol(&code);
                    self.select_symbol(shared(code), cx);
                }
            }
            TodayActionTarget::Market => {
                self.open_market_analysis(cx);
            }
            TodayActionTarget::Opportunities => {
                self.set_primary_task(PrimaryTask::Opportunities, cx);
                let Some(code) = code else {
                    return;
                };
                if let Some(pick) = self
                    .scout_picks
                    .iter()
                    .find(|pick| pick.code == code)
                    .cloned()
                {
                    self.select_scout_pick(&pick, cx);
                } else if let Some(hit) =
                    self.radar_hits.iter().find(|hit| hit.code == code).cloned()
                {
                    self.select_radar_hit(&hit, cx);
                } else if let Some(hit) = self
                    .limitup_hits
                    .iter()
                    .find(|hit| hit.code == code)
                    .cloned()
                {
                    self.select_limitup_hit(&hit, cx);
                } else {
                    self.ensure_today_symbol(&code);
                    self.select_symbol(shared(code), cx);
                }
            }
        }
    }

    pub(crate) fn ensure_today_symbol(&mut self, code: &str) {
        if self.symbols.iter().any(|symbol| symbol.code == code) {
            return;
        }
        let quote = self.quote_for_code(code);
        let name = quote
            .map(|quote| quote.name.clone())
            .filter(|name| !name.is_empty())
            .or_else(|| {
                self.portfolio
                    .position_of(code)
                    .map(|position| position.name)
            })
            .or_else(|| {
                self.scout_picks
                    .iter()
                    .find(|pick| pick.code == code)
                    .map(|pick| pick.name.clone())
            })
            .or_else(|| {
                self.radar_hits
                    .iter()
                    .find(|hit| hit.code == code)
                    .map(|hit| hit.name.clone())
            })
            .or_else(|| {
                self.limitup_hits
                    .iter()
                    .find(|hit| hit.code == code)
                    .map(|hit| hit.name.clone())
            })
            .or_else(|| {
                self.treasure_hits
                    .iter()
                    .find(|hit| hit.code == code)
                    .map(|hit| hit.name.clone())
            })
            .unwrap_or_else(|| code.to_string());
        // Task navigation only stages research metadata. Saving membership is
        // a separate action, and unknown prices remain a metadata placeholder.
        self.preview_symbol = Some(Symbol {
            code: code.to_string(),
            name: shared(name),
            last: quote
                .filter(|quote| quote.usable())
                .and_then(|quote| quote.price)
                .unwrap_or(0.0),
            change_pct: quote.and_then(|quote| quote.change_pct).unwrap_or(0.0),
            volume: quote.and_then(|quote| quote.volume).unwrap_or(0),
            board: board_for_code(code),
        });
    }
}

fn candles_to_records(candles: &[crate::model::Candle]) -> Vec<CandleRecord> {
    candles
        .iter()
        .map(|candle| CandleRecord {
            time: candle.date.to_string(),
            open: candle.open,
            high: candle.high,
            low: candle.low,
            close: candle.close,
            volume: candle.volume,
        })
        .collect()
}

fn playbook_for_radar(strategy: RadarStrategy) -> PlaybookKind {
    match strategy {
        RadarStrategy::Pullback => PlaybookKind::Pullback,
        RadarStrategy::Breakout => PlaybookKind::Breakout,
        RadarStrategy::OversoldBounce => PlaybookKind::OversoldBounce,
    }
}
