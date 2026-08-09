use std::fmt;

use serde::{Deserialize, Serialize};

/// Trading currency. Cross-currency arithmetic must go through an explicit FX conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Currency {
    Cny,
    Hkd,
}

impl Currency {
    pub fn for_code(code: &str) -> Option<Self> {
        let code = code.trim().to_ascii_uppercase();
        if code.is_empty() {
            return None;
        }
        if code.starts_with("HK")
            || (code.len() == 5 && code.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Some(Self::Hkd);
        }
        if code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit()) {
            return Some(Self::Cny);
        }
        None
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Cny => "CNY",
            Self::Hkd => "HKD",
        }
    }
}

/// Fixed-point money in the currency's minor unit (fen/cents).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub currency: Currency,
    pub minor: i64,
}

impl Money {
    pub const fn zero(currency: Currency) -> Self {
        Self { currency, minor: 0 }
    }

    pub fn from_major(currency: Currency, major: f64) -> Option<Self> {
        if !major.is_finite() || major.abs() > i64::MAX as f64 / 100.0 {
            return None;
        }
        Some(Self {
            currency,
            minor: (major * 100.0).round() as i64,
        })
    }

    pub fn major(self) -> f64 {
        self.minor as f64 / 100.0
    }

    pub fn checked_add(self, other: Self) -> Result<Self, CurrencyMismatch> {
        if self.currency != other.currency {
            return Err(CurrencyMismatch {
                left: self.currency,
                right: other.currency,
            });
        }
        let minor = self
            .minor
            .checked_add(other.minor)
            .ok_or(CurrencyMismatch {
                left: self.currency,
                right: other.currency,
            })?;
        Ok(Self {
            currency: self.currency,
            minor,
        })
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, CurrencyMismatch> {
        self.checked_add(Self {
            currency: other.currency,
            minor: -other.minor,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencyMismatch {
    pub left: Currency,
    pub right: Currency,
}

impl fmt::Display for CurrencyMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot combine {} with {} without an FX rate",
            self.left.symbol(),
            self.right.symbol()
        )
    }
}

impl std::error::Error for CurrencyMismatch {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_cross_currency_addition() {
        let cny = Money::from_major(Currency::Cny, 10.25).unwrap();
        let hkd = Money::from_major(Currency::Hkd, 8.0).unwrap();
        assert_eq!(cny.checked_add(hkd).unwrap_err().right, Currency::Hkd);
    }

    #[test]
    fn infers_a_and_h_codes_without_guessing_unknown_values() {
        assert_eq!(Currency::for_code("600519"), Some(Currency::Cny));
        assert_eq!(Currency::for_code("00700"), Some(Currency::Hkd));
        assert_eq!(Currency::for_code("HK00700"), Some(Currency::Hkd));
        assert_eq!(Currency::for_code("UNKNOWN"), None);
    }
}
