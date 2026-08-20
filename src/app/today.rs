//! View-model composition and navigation for the task-oriented Today page.

use gpui::Context;

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
use crate::model::shared;

use super::{StockApp, state::PrimaryTask};

impl StockApp {
    pub(crate) fn market_climate_report(&self) -> ClimateReport {
        assess_market_climate(&self.climate_evidence())
    }

    pub(crate) fn climate_evidence(&self) -> ClimateEvidence {
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
            open_positions: self.portfolio_summary().open_count,
        }
    }

    pub(crate) fn today_dashboard_view_model(&self) -> TodayDashboard {
        let summary = self.portfolio_summary();
        let risk_view = self.portfolio_risk_view(&summary);
        let alerts = self
            .buy_alerts
            .iter()
            .filter(|(_, alert)| alert.any_armed())
            .map(|(code, alert)| {
                let symbol = self.symbols.iter().find(|symbol| symbol.code == *code);
                TodayAlertSnapshot {
                    code: code.clone(),
                    name: symbol
                        .map(|symbol| symbol.name.to_string())
                        .unwrap_or_else(|| code.clone()),
                    last: symbol.map(|symbol| symbol.last).unwrap_or_default(),
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

        build_today_dashboard(TodayDashboardInput {
            alerts,
            risks,
            plans,
            opportunities,
            climate: self.climate_evidence(),
            open_positions: summary.open_count,
        })
    }

    pub(crate) fn rule_ledger_view_model(&self) -> RuleLedgerReport {
        build_rule_ledger(&self.journal.entries)
    }

    pub(crate) fn champion_stock_plans(
        &self,
        cx: &Context<Self>,
    ) -> (Option<String>, Vec<StrategyStockPlan>) {
        let Some((strategy_name, compiled)) = self.strategy_lab_feature.compiled_champion() else {
            return (None, Vec::new());
        };
        let climate = self.market_climate_report();
        let capital = super::helpers::parse_f64(&self.position_capital_input.read(cx).value())
            .unwrap_or(100_000.0);
        let risk_pct = super::helpers::parse_f64(&self.position_risk_pct_input.read(cx).value())
            .unwrap_or(1.0)
            * climate.risk_scale;
        let mut codes = Vec::new();
        for position in self.portfolio.positions() {
            if position.is_open() && !codes.iter().any(|code| code == &position.code) {
                codes.push(position.code);
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
                let name = self
                    .symbols
                    .iter()
                    .find(|symbol| symbol.code == code)
                    .map(|symbol| symbol.name.to_string())
                    .or_else(|| {
                        self.portfolio
                            .position_of(&code)
                            .map(|position| position.name)
                    })
                    .unwrap_or_else(|| code.clone());
                let candles = self.daily_records_for(&code);
                let holding = self.portfolio.position_of(&code).and_then(|position| {
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
        if let Some(cached) =
            self.series_cache
                .lookup_klines(super::types::ChartKind::DayK, code, 0)
            && !cached.candles.is_empty()
        {
            return Some(candles_to_records(&cached.candles));
        }
        if self.selected.as_ref() == code
            && self.chart_kind == super::types::ChartKind::DayK
            && !self.candles.is_empty()
        {
            return Some(candles_to_records(&self.candles));
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
        if let Some(mark) = self
            .portfolio_summary()
            .positions
            .into_iter()
            .find(|mark| mark.position.code == code)
        {
            self.ensure_in_watchlist(&mark.position.code, &mark.position.name, mark.last);
            return;
        }
        if let Some(pick) = self.scout_picks.iter().find(|pick| pick.code == code) {
            let (name, close) = (pick.name.clone(), pick.close);
            self.ensure_in_watchlist(code, &name, close);
            return;
        }
        if let Some(hit) = self.radar_hits.iter().find(|hit| hit.code == code) {
            let (name, close) = (hit.name.clone(), hit.close);
            self.ensure_in_watchlist(code, &name, close);
        }
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
