use serde::{Deserialize, Serialize};

use super::money::Currency;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Market {
    AShare,
    HongKong,
}

impl Market {
    pub fn for_code(code: &str) -> Option<Self> {
        match Currency::for_code(code) {
            Some(Currency::Cny) => Some(Self::AShare),
            Some(Currency::Hkd) => Some(Self::HongKong),
            None => None,
        }
    }

    pub const fn currency(self) -> Currency {
        match self {
            Self::AShare => Currency::Cny,
            Self::HongKong => Currency::Hkd,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Suspended,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Live,
    Delayed,
    Stale,
}

impl Freshness {
    pub fn from_age_secs(age_secs: u64) -> Self {
        match age_secs {
            0..=30 => Self::Live,
            31..=300 => Self::Delayed,
            _ => Self::Stale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuoteRecord {
    pub code: String,
    pub market: Market,
    pub currency: Currency,
    pub name: String,
    pub price: Option<f64>,
    pub change_pct: Option<f64>,
    pub volume: Option<u64>,
    pub source: String,
    /// Unix milliseconds when the provider response was observed.
    pub fetched_at: i64,
    pub market_time: Option<String>,
    pub availability: Availability,
    pub freshness: Freshness,
}

impl QuoteRecord {
    pub fn usable(&self) -> bool {
        matches!(
            self.availability,
            Availability::Available | Availability::Suspended
        ) && self
            .price
            .is_some_and(|price| price.is_finite() && price > 0.0)
    }

    pub fn stale(mut self) -> Self {
        self.freshness = Freshness::Stale;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adjustment {
    None,
    Forward,
    Backward,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandleRecord {
    pub time: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KlineSeries {
    pub code: String,
    pub market: Market,
    pub currency: Currency,
    pub source: String,
    pub as_of: i64,
    pub market_time: Option<String>,
    pub adjustment: Adjustment,
    pub candles: Vec<CandleRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub code: String,
    pub name: String,
    pub market: Market,
}
