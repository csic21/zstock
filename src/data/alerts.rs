//! Local price-alert rules.
//!
//! Alerts are intentionally deterministic: a watchlist quote crossing a
//! fixed target price is enough to trigger one notification. AI can explain
//! the target, but it does not participate in the trigger path.

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

/// A persisted buy-watch rule for one symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuyAlert {
    /// Fixed price target. It is not continuously rewritten when the chart
    /// recalculates its reference band; this keeps an alert predictable.
    pub target_price: f64,
    #[serde(default)]
    pub basis: BuyAlertBasis,
    /// Set after a downward crossing. The alert rearms after price moves
    /// sufficiently above the target again.
    #[serde(default)]
    pub triggered: bool,
}

impl BuyAlert {
    pub fn new(target_price: f64, basis: BuyAlertBasis) -> Self {
        Self {
            target_price,
            basis,
            triggered: false,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.target_price.is_finite() && self.target_price > 0.0
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

/// Whether a triggered alert should become armed for a later downward entry.
pub fn should_rearm(current: f64, target: f64) -> bool {
    current.is_finite()
        && target.is_finite()
        && target > 0.0
        && current > target * (1.0 + REARM_BUFFER_PCT)
}

#[cfg(test)]
mod tests {
    use super::{crossed_down, should_rearm, BuyAlert, BuyAlertBasis};

    #[test]
    fn crosses_down_only_when_entering_from_above() {
        assert!(crossed_down(10.0, 9.5, 9.5));
        assert!(crossed_down(0.0, 9.5, 9.5));
        assert!(!crossed_down(9.5, 9.4, 9.5));
        assert!(!crossed_down(10.0, 9.6, 9.5));
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
        assert_eq!(alert.basis.label(), "手动目标价");
    }
}
