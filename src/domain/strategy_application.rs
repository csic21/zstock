//! Apply a compiled research strategy to one instrument's point-in-time bars.
//!
//! This produces a local plan (buy / sell / hold / wait) with share counts.
//! It does not place broker orders. Entry uses the same next-open convention
//! as the backtest: the last closed bar is the signal, the planned price is
//! that close as a proxy for the next session open.

use super::market::CandleRecord;
use super::position_sizing::{
    BindingConstraint, PositionSizingError, PositionSizingInput, calculate_position_plan,
};
use super::strategy::{CompiledStrategy, PositionContext};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockPlanKind {
    Buy,
    Sell,
    Hold,
    Wait,
}

impl StockPlanKind {
    pub const fn rank(self) -> u8 {
        match self {
            Self::Sell => 0,
            Self::Buy => 1,
            Self::Hold => 2,
            Self::Wait => 3,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Buy => "买入",
            Self::Sell => "卖出",
            Self::Hold => "持有",
            Self::Wait => "等待",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoldingSnapshot {
    pub shares: u64,
    pub avg_cost: f64,
    pub opened_on: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizingLimits {
    pub capital: f64,
    pub risk_pct: f64,
    pub max_position_pct: f64,
    pub lot_size: u64,
    pub minimum_shares: u64,
    pub allow_new_entries: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrategyStockPlan {
    pub code: String,
    pub name: String,
    pub strategy_name: String,
    pub as_of: String,
    pub kind: StockPlanKind,
    pub shares: u64,
    pub price: f64,
    pub stop: Option<f64>,
    pub target: Option<f64>,
    pub notional: f64,
    pub reason: String,
    pub constraint: Option<&'static str>,
}

pub fn apply_strategy_to_stock(
    strategy: &CompiledStrategy,
    code: &str,
    name: &str,
    candles: &[CandleRecord],
    holding: Option<&HoldingSnapshot>,
    sizing: SizingLimits,
) -> StrategyStockPlan {
    let strategy_name = strategy.spec().name.clone();
    let wait = |reason: String, as_of: &str, price: f64| StrategyStockPlan {
        code: code.into(),
        name: name.into(),
        strategy_name: strategy_name.clone(),
        as_of: as_of.into(),
        kind: StockPlanKind::Wait,
        shares: 0,
        price,
        stop: None,
        target: None,
        notional: 0.0,
        reason,
        constraint: None,
    };
    let Some(last) = candles.last() else {
        return wait("没有日 K，无法把冠军策略落到这只股票".into(), "—", 0.0);
    };
    if candles.len() < strategy.warm_up_bars().saturating_add(1) {
        return wait(
            format!(
                "日 K 只有 {} 根，策略至少需要 {} 根",
                candles.len(),
                strategy.warm_up_bars() + 1
            ),
            &last.time,
            last.close,
        );
    }
    let index = candles.len() - 1;
    let open_shares = holding.map(|item| item.shares).unwrap_or(0);
    if open_shares > 0 {
        let Some(holding) = holding else {
            return wait("持仓快照缺失".into(), &last.time, last.close);
        };
        let holding_days = holding_sessions(candles, holding.opened_on.as_deref());
        let exit = strategy.should_exit(
            candles,
            index,
            PositionContext {
                entry_price: holding.avg_cost,
                holding_days,
            },
        );
        if exit {
            return StrategyStockPlan {
                code: code.into(),
                name: name.into(),
                strategy_name,
                as_of: last.time.clone(),
                kind: StockPlanKind::Sell,
                shares: open_shares,
                price: last.close,
                stop: None,
                target: None,
                notional: open_shares as f64 * last.close,
                reason: format!(
                    "冠军策略触发出场；按现有持仓卖出 {open_shares} 股（次日开盘成交，现价只是代理）"
                ),
                constraint: None,
            };
        }
        return StrategyStockPlan {
            code: code.into(),
            name: name.into(),
            strategy_name,
            as_of: last.time.clone(),
            kind: StockPlanKind::Hold,
            shares: open_shares,
            price: last.close,
            stop: None,
            target: None,
            notional: open_shares as f64 * last.close,
            reason: "持仓中，冠军策略尚未给出场信号，继续持有".into(),
            constraint: None,
        };
    }
    if !sizing.allow_new_entries {
        return wait(
            "今日市场观望，即使冠军给出买入信号也不新开仓".into(),
            &last.time,
            last.close,
        );
    }
    if !strategy.entry_signal(candles, index) {
        return wait(
            "冠军策略今日无入场信号，不买".into(),
            &last.time,
            last.close,
        );
    }
    let Some(stop_pct) = strategy.spec().exit.stop_loss_pct() else {
        return wait(
            "策略没有止损，无法按亏损上限计算买入股数".into(),
            &last.time,
            last.close,
        );
    };
    let entry = last.close;
    let stop = entry * (1.0 - stop_pct / 100.0);
    let target = strategy
        .spec()
        .exit
        .take_profit_pct()
        .map(|pct| entry * (1.0 + pct / 100.0));
    match calculate_position_plan(PositionSizingInput {
        capital: sizing.capital,
        risk_pct: sizing.risk_pct,
        max_position_pct: sizing.max_position_pct,
        entry_price: entry,
        invalidation_price: stop,
        target_price: target,
        existing_shares: 0,
        lot_size: sizing.lot_size,
        minimum_shares: sizing.minimum_shares,
    }) {
        Ok(plan) => StrategyStockPlan {
            code: code.into(),
            name: name.into(),
            strategy_name,
            as_of: last.time.clone(),
            kind: StockPlanKind::Buy,
            shares: plan.shares,
            price: entry,
            stop: Some(stop),
            target,
            notional: plan.planned_notional,
            reason: format!(
                "冠军策略给出入场信号；按计划本金和 {} 计算，最多买 {} 股，次日开盘成交",
                plan.binding_constraint.label(),
                plan.shares
            ),
            constraint: Some(constraint_label(plan.binding_constraint)),
        },
        Err(error) => wait(
            format!("{}：{}", StockPlanKind::Buy.label(), sizing_message(error)),
            &last.time,
            last.close,
        ),
    }
}

pub fn actionable_plans(plans: &[StrategyStockPlan]) -> Vec<StrategyStockPlan> {
    let mut rows: Vec<_> = plans
        .iter()
        .filter(|plan| matches!(plan.kind, StockPlanKind::Buy | StockPlanKind::Sell))
        .cloned()
        .collect();
    rows.sort_by(|left, right| {
        left.kind
            .rank()
            .cmp(&right.kind.rank())
            .then_with(|| left.code.cmp(&right.code))
    });
    rows
}

fn constraint_label(constraint: BindingConstraint) -> &'static str {
    constraint.label()
}

fn sizing_message(error: PositionSizingError) -> &'static str {
    error.user_message()
}

fn holding_sessions(candles: &[CandleRecord], opened_on: Option<&str>) -> usize {
    let Some(opened_on) = opened_on else {
        return 0;
    };
    let start: String = opened_on.chars().take(10).collect();
    candles
        .iter()
        .filter(|bar| bar.time.as_str() >= start.as_str())
        .count()
        .saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::strategy::{CompiledStrategy, LocalTemplate};

    fn bars(closes: &[f64]) -> Vec<CandleRecord> {
        closes
            .iter()
            .enumerate()
            .map(|(index, close)| CandleRecord {
                time: format!("2024-01-{:02}", index + 1),
                open: *close,
                high: *close + 1.0,
                low: *close - 1.0,
                close: *close,
                volume: 1_000,
            })
            .collect()
    }

    fn sizing() -> SizingLimits {
        SizingLimits {
            capital: 100_000.0,
            risk_pct: 1.0,
            max_position_pct: 20.0,
            lot_size: 100,
            minimum_shares: 100,
            allow_new_entries: true,
        }
    }

    #[test]
    fn breakout_signal_sizes_a_round_lot_buy() {
        let strategy =
            CompiledStrategy::compile(LocalTemplate::NDayHighBreakout.build("fixture")).unwrap();
        let mut closes = vec![100.0; 30];
        closes[29] = 102.0;
        let plan =
            apply_strategy_to_stock(&strategy, "600000", "浦发", &bars(&closes), None, sizing());
        assert_eq!(plan.kind, StockPlanKind::Buy);
        assert_eq!(plan.shares % 100, 0);
        assert!(plan.shares >= 100);
        assert!(plan.stop.is_some());
        assert!(plan.notional > 0.0);
    }

    #[test]
    fn open_position_exits_when_stop_is_hit() {
        let strategy =
            CompiledStrategy::compile(LocalTemplate::NDayHighBreakout.build("fixture")).unwrap();
        let plan = apply_strategy_to_stock(
            &strategy,
            "600000",
            "浦发",
            &bars(&vec![100.0; 30]),
            Some(&HoldingSnapshot {
                shares: 500,
                avg_cost: 120.0,
                opened_on: Some("2024-01-01".into()),
            }),
            sizing(),
        );
        assert_eq!(plan.kind, StockPlanKind::Sell);
        assert_eq!(plan.shares, 500);
    }

    #[test]
    fn freeze_blocks_new_buys_but_not_exits() {
        let strategy =
            CompiledStrategy::compile(LocalTemplate::NDayHighBreakout.build("fixture")).unwrap();
        let mut closes = vec![100.0; 30];
        closes[29] = 102.0;
        let mut frozen = sizing();
        frozen.allow_new_entries = false;
        let buy =
            apply_strategy_to_stock(&strategy, "600000", "浦发", &bars(&closes), None, frozen);
        assert_eq!(buy.kind, StockPlanKind::Wait);
        assert!(buy.reason.contains("观望"));
    }

    #[test]
    fn no_signal_does_not_invent_a_buy() {
        let strategy =
            CompiledStrategy::compile(LocalTemplate::NDayHighBreakout.build("fixture")).unwrap();
        let plan = apply_strategy_to_stock(
            &strategy,
            "600000",
            "浦发",
            &bars(&vec![100.0; 30]),
            None,
            sizing(),
        );
        assert_eq!(plan.kind, StockPlanKind::Wait);
        assert_eq!(plan.shares, 0);
    }
}
