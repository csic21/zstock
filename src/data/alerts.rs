//! Local price-alert rules.
//!
//! Alerts are intentionally deterministic: a watchlist quote crossing a
//! fixed target price is enough to trigger one notification. AI can explain
//! the target, but it does not participate in the trigger path.
//!
//! Supports:
//! - **买入观察**：价格从上方跌入目标（原逻辑）
//! - **卖出/止盈**：价格从下方涨破目标
//! - **止损观察**：价格从上方跌破止损价

use serde::{Deserialize, Serialize};

/// How the target price was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BuyAlertBasis {
    /// User entered a price manually.
    #[default]
    Manual,
    /// The target was copied from the local technical reference level.
    Technical,
}

impl BuyAlertBasis {
    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "手动目标价",
            Self::Technical => "技术参考价",
        }
    }
}

/// A persisted multi-rule watch for one symbol.
///
/// Backward compatible: older configs only had buy target fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuyAlert {
    /// Fixed buy/watch target (price falls into zone from above).
    pub target_price: f64,
    #[serde(default)]
    pub basis: BuyAlertBasis,
    /// Set after a downward crossing. Rearms after price moves sufficiently above.
    #[serde(default)]
    pub triggered: bool,
    /// Optional take-profit / sell observation (cross up from below).
    #[serde(default)]
    pub sell_price: Option<f64>,
    #[serde(default)]
    pub sell_triggered: bool,
    /// Optional stop-loss observation (cross down through stop).
    #[serde(default)]
    pub stop_price: Option<f64>,
    #[serde(default)]
    pub stop_triggered: bool,
}

impl BuyAlert {
    pub fn new(target_price: f64, basis: BuyAlertBasis) -> Self {
        Self {
            target_price,
            basis,
            triggered: false,
            sell_price: None,
            sell_triggered: false,
            stop_price: None,
            stop_triggered: false,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.target_price.is_finite() && self.target_price > 0.0
    }

    pub fn has_sell(&self) -> bool {
        self.sell_price.is_some_and(|p| p.is_finite() && p > 0.0)
    }

    pub fn has_stop(&self) -> bool {
        self.stop_price.is_some_and(|p| p.is_finite() && p > 0.0)
    }

    pub fn any_armed(&self) -> bool {
        self.is_valid()
            || self.has_sell()
            || self.has_stop()
    }
}

/// Which leg of a multi-alert fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertLeg {
    Buy,
    Sell,
    Stop,
}

impl AlertLeg {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Buy, true) => "Buy zone",
            (Self::Buy, false) => "买入区",
            (Self::Sell, true) => "Take profit",
            (Self::Sell, false) => "止盈/减仓",
            (Self::Stop, true) => "Stop",
            (Self::Stop, false) => "止损观察",
        }
    }
}

/// Small hysteresis prevents a price hovering around the target from
/// immediately rearming and firing again.
pub const REARM_BUFFER_PCT: f64 = 0.003;

/// Whether the quote moved from above the target into the target zone.
/// `previous <= 0` represents the first valid quote after startup; if that
/// quote is already in the zone, notifying once is useful and still safe
/// because the caller persists the triggered state.
pub fn crossed_down(previous: f64, current: f64, target: f64) -> bool {
    current.is_finite()
        && current > 0.0
        && target.is_finite()
        && target > 0.0
        && current <= target
        && (previous <= 0.0 || previous > target)
}

/// Price moved from below (or first quote) up through the target.
pub fn crossed_up(previous: f64, current: f64, target: f64) -> bool {
    current.is_finite()
        && current > 0.0
        && target.is_finite()
        && target > 0.0
        && current >= target
        && (previous <= 0.0 || previous < target)
}

/// Whether a triggered buy alert should become armed for a later downward entry.
pub fn should_rearm(current: f64, target: f64) -> bool {
    current.is_finite()
        && target.is_finite()
        && target > 0.0
        && current > target * (1.0 + REARM_BUFFER_PCT)
}

/// Sell leg rearms after price falls back below target by buffer.
pub fn should_rearm_sell(current: f64, target: f64) -> bool {
    current.is_finite()
        && target.is_finite()
        && target > 0.0
        && current < target * (1.0 - REARM_BUFFER_PCT)
}

/// Stop leg rearms after price climbs back above stop by buffer.
pub fn should_rearm_stop(current: f64, stop: f64) -> bool {
    should_rearm(current, stop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crosses_down_only_when_entering_from_above() {
        assert!(crossed_down(10.0, 9.5, 9.5));
        assert!(crossed_down(0.0, 9.5, 9.5));
        assert!(!crossed_down(9.5, 9.4, 9.5));
        assert!(!crossed_down(10.0, 9.6, 9.5));
    }

    #[test]
    fn crosses_up_from_below() {
        assert!(crossed_up(9.0, 10.0, 9.5));
        assert!(!crossed_up(10.0, 10.5, 9.5));
        assert!(crossed_up(0.0, 10.0, 9.5));
    }

    #[test]
    fn rearm_has_a_small_buffer() {
        assert!(!should_rearm(10.02, 10.0));
        assert!(should_rearm(10.04, 10.0));
    }

    #[test]
    fn alert_defaults_to_manual_and_untriggered() {
        let alert = BuyAlert::new(12.3, BuyAlertBasis::Manual);
        assert!(alert.is_valid());
        assert!(!alert.triggered);
        assert!(!alert.has_sell());
        assert_eq!(alert.basis.label(), "手动目标价");
    }
}
