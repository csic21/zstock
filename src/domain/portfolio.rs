use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::money::{Currency, Money};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrencySummary {
    pub currency: Currency,
    pub cost: Money,
    pub market_value: Money,
    pub cash: Money,
    pub realized_pnl: Money,
    pub unrealized_pnl: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PortfolioByCurrency {
    pub groups: BTreeMap<Currency, CurrencySummary>,
    pub pending_currency_codes: Vec<String>,
}

impl PortfolioByCurrency {
    pub fn unified_total(&self) -> Option<Money> {
        if self.groups.len() != 1 || !self.pending_currency_codes.is_empty() {
            return None;
        }
        let summary = self.groups.values().next()?;
        summary.market_value.checked_add(summary.cash).ok()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RiskItem {
    pub code: String,
    pub currency: Currency,
    pub position_weight_pct: f64,
    pub risk_amount: Option<Money>,
    pub industry: Option<String>,
    pub quote_stale: bool,
    pub invalidation_breached: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortfolioRiskView {
    pub items: Vec<RiskItem>,
    pub largest_position: Option<String>,
    pub quote_coverage_pct: f64,
    pub invalidation_coverage_pct: f64,
    pub industry_coverage_pct: f64,
}

impl PortfolioRiskView {
    pub fn from_items(mut items: Vec<RiskItem>) -> Self {
        items.sort_by(|left, right| {
            right
                .position_weight_pct
                .total_cmp(&left.position_weight_pct)
                .then_with(|| left.code.cmp(&right.code))
        });
        let total = items.len();
        let quote_covered = items.iter().filter(|item| !item.quote_stale).count();
        let invalidation_covered = items
            .iter()
            .filter(|item| item.risk_amount.is_some())
            .count();
        let industry_covered = items.iter().filter(|item| item.industry.is_some()).count();
        let percentage = |covered| {
            if total == 0 {
                0.0
            } else {
                covered as f64 / total as f64 * 100.0
            }
        };
        let largest_position = items.first().map(|item| item.code.clone());
        Self {
            items,
            largest_position,
            quote_coverage_pct: percentage(quote_covered),
            invalidation_coverage_pct: percentage(invalidation_covered),
            industry_coverage_pct: percentage(industry_covered),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(code: &str, weight: f64, covered: bool) -> RiskItem {
        RiskItem {
            code: code.into(),
            currency: Currency::Cny,
            position_weight_pct: weight,
            risk_amount: covered.then(|| Money::from_major(Currency::Cny, 10.0).unwrap()),
            industry: None,
            quote_stale: false,
            invalidation_breached: false,
        }
    }

    #[test]
    fn risk_view_orders_concentration_and_reports_missing_coverage() {
        let view = PortfolioRiskView::from_items(vec![
            item("000001", 35.0, true),
            item("600000", 65.0, false),
        ]);
        assert_eq!(view.largest_position.as_deref(), Some("600000"));
        assert!((view.invalidation_coverage_pct - 50.0).abs() < f64::EPSILON);
        assert!((view.industry_coverage_pct - 0.0).abs() < f64::EPSILON);
    }
}
