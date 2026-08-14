//! Deterministic position sizing from a loss budget and an invalidation price.
//!
//! The rule is intentionally simple and auditable: size is capped by both the
//! maximum tolerated loss and the maximum position allocation, then rounded
//! down to the market lot. It does not assume that a stop can always fill at
//! the requested price.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingConstraint {
    LossBudget,
    PositionCap,
}

impl BindingConstraint {
    pub fn label(self) -> &'static str {
        match self {
            Self::LossBudget => "单笔亏损上限",
            Self::PositionCap => "单票仓位上限",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionSizingInput {
    pub capital: f64,
    pub risk_pct: f64,
    pub max_position_pct: f64,
    pub entry_price: f64,
    pub invalidation_price: f64,
    pub target_price: Option<f64>,
    pub existing_shares: u64,
    pub lot_size: u64,
    pub minimum_shares: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionPlan {
    pub shares: u64,
    pub resulting_shares: u64,
    pub entry_price: f64,
    pub invalidation_price: f64,
    pub target_price: Option<f64>,
    pub planned_notional: f64,
    pub loss_budget: f64,
    pub planned_loss: f64,
    pub capital_pct: f64,
    pub risk_reward: Option<f64>,
    pub binding_constraint: BindingConstraint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSizingError {
    InvalidCapital,
    InvalidRiskPercent,
    InvalidPositionCap,
    InvalidEntry,
    InvalidInvalidation,
    InvalidLotSize,
    BelowMinimumLot,
    ExistingPositionAtLimit,
    NewEntriesRestricted,
}

impl PositionSizingError {
    pub fn user_message(self) -> &'static str {
        match self {
            Self::InvalidCapital => "请输入大于 0 的计划本金",
            Self::InvalidRiskPercent => "单笔亏损上限须在 0–100% 之间",
            Self::InvalidPositionCap => "单票仓位上限须在 0–100% 之间",
            Self::InvalidEntry => "缺少有效的参考观察价",
            Self::InvalidInvalidation => "失效价必须低于参考观察价",
            Self::InvalidLotSize => "交易单位无效",
            Self::BelowMinimumLot => "按当前风险预算不足以买入一手，先跳过更安全",
            Self::ExistingPositionAtLimit => "现有持仓已达到风险或仓位上限，不宜继续加仓",
            Self::NewEntriesRestricted => "今日市场观望，不宜新开仓；先处理持仓风险",
        }
    }
}

pub fn calculate_position_plan(
    input: PositionSizingInput,
) -> Result<PositionPlan, PositionSizingError> {
    if !input.capital.is_finite() || input.capital <= 0.0 {
        return Err(PositionSizingError::InvalidCapital);
    }
    if !input.risk_pct.is_finite()
        || !(0.0..=100.0).contains(&input.risk_pct)
        || input.risk_pct == 0.0
    {
        return Err(PositionSizingError::InvalidRiskPercent);
    }
    if !input.max_position_pct.is_finite()
        || !(0.0..=100.0).contains(&input.max_position_pct)
        || input.max_position_pct == 0.0
    {
        return Err(PositionSizingError::InvalidPositionCap);
    }
    if !input.entry_price.is_finite() || input.entry_price <= 0.0 {
        return Err(PositionSizingError::InvalidEntry);
    }
    if !input.invalidation_price.is_finite()
        || input.invalidation_price <= 0.0
        || input.invalidation_price >= input.entry_price
    {
        return Err(PositionSizingError::InvalidInvalidation);
    }
    if input.lot_size == 0 || input.minimum_shares == 0 {
        return Err(PositionSizingError::InvalidLotSize);
    }

    let per_share_loss = input.entry_price - input.invalidation_price;
    let loss_budget = input.capital * input.risk_pct / 100.0;
    let position_cap = input.capital * input.max_position_pct / 100.0;
    let by_loss = (loss_budget / per_share_loss).floor().max(0.0) as u64;
    let by_position = (position_cap / input.entry_price).floor().max(0.0) as u64;
    let binding_constraint = if by_loss <= by_position {
        BindingConstraint::LossBudget
    } else {
        BindingConstraint::PositionCap
    };
    let target_total_shares = by_loss.min(by_position);
    if target_total_shares <= input.existing_shares {
        return Err(PositionSizingError::ExistingPositionAtLimit);
    }
    let shares = (target_total_shares - input.existing_shares) / input.lot_size * input.lot_size;
    if shares == 0 || (input.existing_shares == 0 && shares < input.minimum_shares) {
        return Err(PositionSizingError::BelowMinimumLot);
    }

    let resulting_shares = input.existing_shares.saturating_add(shares);
    let planned_notional = shares as f64 * input.entry_price;
    let planned_loss = resulting_shares as f64 * per_share_loss;
    let risk_reward = input.target_price.and_then(|target| {
        (target.is_finite() && target > input.entry_price)
            .then_some((target - input.entry_price) / per_share_loss)
    });

    Ok(PositionPlan {
        shares,
        resulting_shares,
        entry_price: input.entry_price,
        invalidation_price: input.invalidation_price,
        target_price: input.target_price,
        planned_notional,
        loss_budget,
        planned_loss,
        capital_pct: resulting_shares as f64 * input.entry_price / input.capital * 100.0,
        risk_reward,
        binding_constraint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> PositionSizingInput {
        PositionSizingInput {
            capital: 100_000.0,
            risk_pct: 1.0,
            max_position_pct: 20.0,
            entry_price: 10.0,
            invalidation_price: 9.0,
            target_price: Some(12.0),
            existing_shares: 0,
            lot_size: 100,
            minimum_shares: 100,
        }
    }

    #[test]
    fn loss_budget_caps_shares_and_rounds_down_to_a_share_lot() {
        let mut value = input();
        value.entry_price = 12.34;
        value.invalidation_price = 11.87;
        value.max_position_pct = 100.0;
        let plan = calculate_position_plan(value).unwrap();
        assert_eq!(plan.shares, 2_100);
        assert!(plan.planned_loss <= plan.loss_budget);
        assert_eq!(plan.binding_constraint, BindingConstraint::LossBudget);
    }

    #[test]
    fn position_cap_prevents_a_narrow_stop_from_creating_concentration() {
        let mut value = input();
        value.invalidation_price = 9.9;
        let plan = calculate_position_plan(value).unwrap();
        assert_eq!(plan.shares, 2_000);
        assert!((plan.capital_pct - 20.0).abs() < 1e-9);
        assert_eq!(plan.binding_constraint, BindingConstraint::PositionCap);
    }

    #[test]
    fn existing_position_is_subtracted_from_the_additional_buy() {
        let mut value = input();
        value.existing_shares = 500;
        let plan = calculate_position_plan(value).unwrap();
        assert_eq!(plan.shares, 500);
        assert_eq!(plan.resulting_shares, 1_000);
        assert!((plan.planned_loss - plan.loss_budget).abs() < 1e-9);
    }

    #[test]
    fn star_market_minimum_rejects_a_one_hundred_share_plan() {
        let mut value = input();
        value.capital = 1_000.0;
        value.risk_pct = 10.0;
        value.max_position_pct = 100.0;
        value.minimum_shares = 200;
        assert_eq!(
            calculate_position_plan(value),
            Err(PositionSizingError::BelowMinimumLot)
        );
    }

    #[test]
    fn invalidation_must_be_below_entry() {
        let mut value = input();
        value.invalidation_price = value.entry_price;
        assert_eq!(
            calculate_position_plan(value),
            Err(PositionSizingError::InvalidInvalidation)
        );
    }

    #[test]
    fn rejects_a_trade_when_budget_cannot_buy_one_lot() {
        let mut value = input();
        value.capital = 500.0;
        assert_eq!(
            calculate_position_plan(value),
            Err(PositionSizingError::BelowMinimumLot)
        );
    }
}
