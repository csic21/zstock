//! Deterministic aggregation for the task-oriented "Today" dashboard.
//!
//! The dashboard does not predict returns. It only promotes already-known
//! obligations, risk gaps, triggered rules and qualified observations into a
//! single queue that the user can act on.

use std::collections::{BTreeMap, BTreeSet};

use super::climate::{
    ClimateEvidence, ClimateReport, MarketClimate, PlaybookKind, assess_market_climate,
    gate_playbook,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodaySeverity {
    Critical,
    Warning,
    Info,
}

impl TodaySeverity {
    const fn rank(self) -> u8 {
        match self {
            Self::Critical => 0,
            Self::Warning => 1,
            Self::Info => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodayActionTarget {
    Research,
    Opportunities,
    Portfolio,
    Market,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TodayAction {
    pub id: String,
    pub code: Option<String>,
    pub title: String,
    pub detail: String,
    pub severity: TodaySeverity,
    pub target: TodayActionTarget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TodayAlertSnapshot {
    pub code: String,
    pub name: String,
    pub last: f64,
    pub buy_target: Option<f64>,
    pub buy_triggered: bool,
    pub sell_target: Option<f64>,
    pub sell_triggered: bool,
    pub stop: Option<f64>,
    pub stop_triggered: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TodayRiskSnapshot {
    pub code: String,
    pub position_weight_pct: f64,
    pub risk_amount_label: Option<String>,
    pub quote_stale: bool,
    pub invalidation_breached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodayPlanSnapshot {
    pub id: String,
    pub code: String,
    pub name: String,
    pub review_on: String,
    pub due: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TodayOpportunity {
    pub code: String,
    pub name: String,
    pub strategy: String,
    pub playbook: PlaybookKind,
    pub score: f64,
    pub observation: String,
    pub ready: bool,
    pub gate_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TodayDashboardInput {
    pub alerts: Vec<TodayAlertSnapshot>,
    pub risks: Vec<TodayRiskSnapshot>,
    pub plans: Vec<TodayPlanSnapshot>,
    pub opportunities: Vec<TodayOpportunity>,
    pub climate: ClimateEvidence,
    pub open_positions: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TodayDashboard {
    pub actions: Vec<TodayAction>,
    pub opportunities: Vec<TodayOpportunity>,
    pub climate: ClimateReport,
    pub active_alerts: usize,
    pub open_positions: usize,
    pub due_reviews: usize,
    pub ready_opportunities: usize,
    pub waiting_opportunities: usize,
    pub gated_opportunities: usize,
}

pub fn build_today_dashboard(input: TodayDashboardInput) -> TodayDashboard {
    let mut climate_evidence = input.climate;
    if climate_evidence.open_positions == 0 && input.open_positions > 0 {
        climate_evidence.open_positions = input.open_positions;
    }
    let climate = assess_market_climate(&climate_evidence);
    let active_alerts = input.alerts.len();
    let due_reviews = input.plans.iter().filter(|plan| plan.due).count();
    let mut actions = Vec::new();
    let mut invalidation_codes = BTreeSet::new();
    if let Some(action) = climate_action(&climate) {
        actions.push(action);
    }

    for risk in &input.risks {
        if risk.invalidation_breached {
            invalidation_codes.insert(risk.code.clone());
            actions.push(TodayAction {
                id: format!("risk-breached:{}", risk.code),
                code: Some(risk.code.clone()),
                title: format!("{} 已触及失效价", risk.code),
                detail: risk
                    .risk_amount_label
                    .as_ref()
                    .map(|amount| format!("计划风险约 {amount}，优先核对持仓与退出计划"))
                    .unwrap_or_else(|| "优先核对持仓与退出计划；当前风险金额未知".into()),
                severity: TodaySeverity::Critical,
                target: TodayActionTarget::Portfolio,
            });
        }
        if risk.position_weight_pct > 20.0 {
            actions.push(TodayAction {
                id: format!("risk-concentration:{}", risk.code),
                code: Some(risk.code.clone()),
                title: format!("{} 单票集中度偏高", risk.code),
                detail: format!(
                    "同币种持仓权重 {:.1}%，高于 20% 纪律上限",
                    risk.position_weight_pct
                ),
                severity: TodaySeverity::Warning,
                target: TodayActionTarget::Portfolio,
            });
        }
        if risk.quote_stale {
            actions.push(TodayAction {
                id: format!("risk-stale:{}", risk.code),
                code: Some(risk.code.clone()),
                title: format!("{} 行情证据不完整", risk.code),
                detail: "行情缺失或已过期，风险金额与决策结论暂不可靠".into(),
                severity: TodaySeverity::Warning,
                target: TodayActionTarget::Portfolio,
            });
        } else if risk.risk_amount_label.is_none() {
            actions.push(TodayAction {
                id: format!("risk-no-invalidation:{}", risk.code),
                code: Some(risk.code.clone()),
                title: format!("{} 尚未设置失效价", risk.code),
                detail: "无法计算这笔持仓最多计划亏损多少".into(),
                severity: TodaySeverity::Warning,
                target: TodayActionTarget::Research,
            });
        }
    }

    for alert in &input.alerts {
        let display = display_symbol(&alert.code, &alert.name);
        if alert.stop_triggered && !invalidation_codes.contains(&alert.code) {
            actions.push(TodayAction {
                id: format!("alert-stop:{}", alert.code),
                code: Some(alert.code.clone()),
                title: format!("{display} 止损观察已触发"),
                detail: price_detail(alert.last, alert.stop, "失效价"),
                severity: TodaySeverity::Critical,
                target: TodayActionTarget::Research,
            });
        }
        if alert.sell_triggered {
            actions.push(TodayAction {
                id: format!("alert-sell:{}", alert.code),
                code: Some(alert.code.clone()),
                title: format!("{display} 目标区间已触发"),
                detail: price_detail(alert.last, alert.sell_target, "目标价"),
                severity: TodaySeverity::Warning,
                target: TodayActionTarget::Research,
            });
        }
        if alert.buy_triggered {
            actions.push(TodayAction {
                id: format!("alert-buy:{}", alert.code),
                code: Some(alert.code.clone()),
                title: format!("{display} 已进入观察区"),
                detail: price_detail(alert.last, alert.buy_target, "观察价"),
                severity: TodaySeverity::Info,
                target: TodayActionTarget::Research,
            });
        }

        if !alert.stop_triggered
            && let Some(stop) = alert.stop
            && is_approaching_from_above(alert.last, stop)
        {
            actions.push(TodayAction {
                id: format!("alert-near-stop:{}", alert.code),
                code: Some(alert.code.clone()),
                title: format!("{display} 接近失效价"),
                detail: price_detail(alert.last, Some(stop), "失效价"),
                severity: TodaySeverity::Warning,
                target: TodayActionTarget::Research,
            });
        }
        if !alert.buy_triggered
            && let Some(target) = alert.buy_target
            && is_approaching_from_above(alert.last, target)
        {
            actions.push(TodayAction {
                id: format!("alert-near-buy:{}", alert.code),
                code: Some(alert.code.clone()),
                title: format!("{display} 接近观察区"),
                detail: price_detail(alert.last, Some(target), "观察价"),
                severity: TodaySeverity::Info,
                target: TodayActionTarget::Research,
            });
        }
    }

    for plan in input.plans.iter().filter(|plan| plan.due) {
        actions.push(TodayAction {
            id: format!("plan-review:{}", plan.id),
            code: Some(plan.code.clone()),
            title: format!("{} 的计划待复盘", display_symbol(&plan.code, &plan.name)),
            detail: format!(
                "约定复盘日 {}；核对是否执行、退出原因和计划纪律",
                plan.review_on
            ),
            severity: TodaySeverity::Warning,
            target: TodayActionTarget::Portfolio,
        });
    }

    actions.sort_by(|left, right| {
        left.severity
            .rank()
            .cmp(&right.severity.rank())
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut best_by_code: BTreeMap<String, TodayOpportunity> = BTreeMap::new();
    for mut opportunity in input.opportunities {
        if opportunity.ready
            && let Some(reason) = gate_playbook(&climate, opportunity.playbook, opportunity.score)
        {
            opportunity.ready = false;
            opportunity.gate_reason = Some(reason);
        }
        best_by_code
            .entry(opportunity.code.clone())
            .and_modify(|current| {
                if opportunity.ready && !current.ready
                    || opportunity.ready == current.ready && opportunity.score > current.score
                {
                    *current = opportunity.clone();
                }
            })
            .or_insert(opportunity);
    }
    let mut opportunities: Vec<_> = best_by_code.into_values().collect();
    opportunities.sort_by(|left, right| {
        right
            .ready
            .cmp(&left.ready)
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| left.code.cmp(&right.code))
    });
    let ready_opportunities = opportunities.iter().filter(|item| item.ready).count();
    let waiting_opportunities = opportunities.len().saturating_sub(ready_opportunities);
    let gated_opportunities = opportunities
        .iter()
        .filter(|item| item.gate_reason.is_some())
        .count();
    opportunities.truncate(6);

    TodayDashboard {
        actions,
        opportunities,
        climate,
        active_alerts,
        open_positions: input.open_positions,
        due_reviews,
        ready_opportunities,
        waiting_opportunities,
        gated_opportunities,
    }
}

fn climate_action(climate: &ClimateReport) -> Option<TodayAction> {
    let (severity, title) = match climate.climate {
        MarketClimate::StandAside => (TodaySeverity::Warning, "今日不宜新开仓"),
        MarketClimate::Defend => (TodaySeverity::Warning, "今日先防守，不扩散新仓"),
        MarketClimate::Select | MarketClimate::Attack => return None,
    };
    Some(TodayAction {
        id: format!("climate:{}", climate.climate.label()),
        code: None,
        title: title.into(),
        detail: format!("{}。{}", climate.headline, climate.detail),
        severity,
        target: TodayActionTarget::Market,
    })
}

fn display_symbol(code: &str, name: &str) -> String {
    if name.trim().is_empty() || name == code {
        code.into()
    } else {
        format!("{code} {name}")
    }
}

fn price_detail(last: f64, target: Option<f64>, target_label: &str) -> String {
    match target {
        Some(target) if last.is_finite() && last > 0.0 => {
            format!("现价 {last:.2} · {target_label} {target:.2}")
        }
        Some(target) => format!("{target_label} {target:.2} · 等待有效行情"),
        None => format!("{target_label}缺失"),
    }
}

fn is_approaching_from_above(last: f64, target: f64) -> bool {
    last.is_finite() && last > target && target.is_finite() && target > 0.0 && last <= target * 1.02
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_risk_is_first_and_missing_invalidation_is_visible() {
        let dashboard = build_today_dashboard(TodayDashboardInput {
            risks: vec![
                TodayRiskSnapshot {
                    code: "600000".into(),
                    position_weight_pct: 12.0,
                    risk_amount_label: Some("¥ 800".into()),
                    quote_stale: false,
                    invalidation_breached: true,
                },
                TodayRiskSnapshot {
                    code: "000001".into(),
                    position_weight_pct: 21.0,
                    risk_amount_label: None,
                    quote_stale: false,
                    invalidation_breached: false,
                },
            ],
            open_positions: 2,
            ..TodayDashboardInput::default()
        });

        assert_eq!(dashboard.actions[0].severity, TodaySeverity::Critical);
        assert_eq!(dashboard.actions[0].code.as_deref(), Some("600000"));
        assert!(
            dashboard
                .actions
                .iter()
                .any(|item| item.id == "risk-no-invalidation:000001")
        );
        assert_eq!(dashboard.open_positions, 2);
    }

    #[test]
    fn triggered_and_nearby_alerts_become_actions_without_duplicates() {
        let dashboard = build_today_dashboard(TodayDashboardInput {
            alerts: vec![TodayAlertSnapshot {
                code: "600519".into(),
                name: "贵州茅台".into(),
                last: 101.0,
                buy_target: Some(100.0),
                buy_triggered: false,
                sell_target: Some(120.0),
                sell_triggered: true,
                stop: Some(90.0),
                stop_triggered: false,
            }],
            ..TodayDashboardInput::default()
        });

        assert_eq!(dashboard.active_alerts, 1);
        assert!(
            dashboard
                .actions
                .iter()
                .any(|item| item.id == "alert-sell:600519")
        );
        assert!(
            dashboard
                .actions
                .iter()
                .any(|item| item.id == "alert-near-buy:600519")
        );
    }

    #[test]
    fn opportunities_are_deduplicated_and_ready_items_rank_first() {
        let dashboard = build_today_dashboard(TodayDashboardInput {
            opportunities: vec![
                opportunity("600000", 90.0, false),
                opportunity("600000", 70.0, true),
                opportunity("000001", 88.0, true),
            ],
            ..TodayDashboardInput::default()
        });

        assert_eq!(dashboard.opportunities.len(), 2);
        assert_eq!(dashboard.opportunities[0].code, "000001");
        assert_eq!(dashboard.opportunities[1].code, "600000");
        assert!(dashboard.opportunities[1].ready);
        assert_eq!(dashboard.ready_opportunities, 2);
        assert_eq!(dashboard.waiting_opportunities, 0);
    }

    #[test]
    fn due_plan_is_promoted_to_the_action_queue() {
        let dashboard = build_today_dashboard(TodayDashboardInput {
            plans: vec![TodayPlanSnapshot {
                id: "p1".into(),
                code: "00700".into(),
                name: "腾讯控股".into(),
                review_on: "2026-08-12".into(),
                due: true,
            }],
            ..TodayDashboardInput::default()
        });

        assert_eq!(dashboard.due_reviews, 1);
        assert_eq!(dashboard.actions.len(), 1);
        assert_eq!(dashboard.actions[0].target, TodayActionTarget::Portfolio);
    }

    fn opportunity(code: &str, score: f64, ready: bool) -> TodayOpportunity {
        TodayOpportunity {
            code: code.into(),
            name: code.into(),
            strategy: "测试策略".into(),
            playbook: PlaybookKind::Pullback,
            score,
            observation: "10.00–11.00".into(),
            ready,
            gate_reason: None,
        }
    }

    #[test]
    fn weak_tape_demotes_breakouts_and_adds_a_stand_aside_action() {
        let dashboard = build_today_dashboard(TodayDashboardInput {
            climate: ClimateEvidence {
                indices: vec![
                    crate::domain::climate::IndexMove {
                        name: "上证综指".into(),
                        change_pct: -1.3,
                    },
                    crate::domain::climate::IndexMove {
                        name: "沪深300".into(),
                        change_pct: -1.1,
                    },
                    crate::domain::climate::IndexMove {
                        name: "创业板指".into(),
                        change_pct: -1.6,
                    },
                ],
                stock_advances: Some(160),
                stock_declines: Some(840),
                stock_unchanged: Some(40),
                sector_advances: Some(8),
                sector_declines: Some(78),
                sector_unchanged: Some(4),
                sector_average_change: Some(-1.4),
                open_positions: 2,
            },
            opportunities: vec![TodayOpportunity {
                code: "600000".into(),
                name: "浦发银行".into(),
                strategy: "放量突破".into(),
                playbook: PlaybookKind::Breakout,
                score: 88.0,
                observation: "10.00–10.50".into(),
                ready: true,
                gate_reason: None,
            }],
            open_positions: 2,
            ..TodayDashboardInput::default()
        });

        assert_eq!(
            dashboard.climate.stance,
            crate::domain::climate::NewEntryStance::Freeze
        );
        assert!(!dashboard.opportunities[0].ready);
        assert!(dashboard.opportunities[0].gate_reason.is_some());
        assert_eq!(dashboard.ready_opportunities, 0);
        assert_eq!(dashboard.gated_opportunities, 1);
        assert!(
            dashboard
                .actions
                .iter()
                .any(|item| item.id == "climate:观望")
        );
    }
}
