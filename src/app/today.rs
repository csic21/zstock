//! View-model composition and navigation for the task-oriented Today page.

use gpui::Context;

use crate::data::scout::ScoutVerdict;
use crate::domain::journal::PlanStatus;
use crate::domain::rule_ledger::{RuleLedgerReport, build_rule_ledger};
use crate::domain::today::{
    TodayAction, TodayActionTarget, TodayAlertSnapshot, TodayDashboard, TodayDashboardInput,
    TodayOpportunity, TodayPlanSnapshot, TodayRiskSnapshot, build_today_dashboard,
};
use crate::model::shared;

use super::{StockApp, state::PrimaryTask};

impl StockApp {
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
                score: pick.buy_score,
                observation: pick.buy_band_text(),
                ready: pick.verdict == ScoutVerdict::BuyWatch
                    && pick.close >= pick.buy_low
                    && pick.close <= pick.buy_high * 1.01,
            })
            .collect::<Vec<_>>();
        opportunities.extend(self.radar_hits.iter().map(|hit| TodayOpportunity {
            code: hit.code.clone(),
            name: hit.name.clone(),
            strategy: hit.strategy.label(false).into(),
            score: hit.score,
            observation: hit.watch_band_text(),
            ready: hit.close >= hit.watch_low && hit.close <= hit.watch_high,
        }));

        build_today_dashboard(TodayDashboardInput {
            alerts,
            risks,
            plans,
            opportunities,
            open_positions: summary.open_count,
        })
    }

    pub(crate) fn rule_ledger_view_model(&self) -> RuleLedgerReport {
        build_rule_ledger(&self.journal.entries)
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

    fn ensure_today_symbol(&mut self, code: &str) {
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
