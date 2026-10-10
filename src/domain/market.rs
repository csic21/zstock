use serde::{Deserialize, Serialize};

use super::money::Currency;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Market {
    AShare,
    HongKong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetType {
    Stock,
    Index,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct InstrumentId {
    pub market: Market,
    pub asset_type: AssetType,
    pub code: String,
}

impl InstrumentId {
    pub fn storage_key(&self) -> String {
        let market = match self.market {
            Market::AShare => "a_share",
            Market::HongKong => "hong_kong",
        };
        let asset_type = match self.asset_type {
            AssetType::Stock => "stock",
            AssetType::Index => "index",
        };
        format!("{market}:{asset_type}:{}", self.code)
    }
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
    /// The provider did not supply a parseable market timestamp.
    Unknown,
    Delayed,
    Stale,
}

impl Freshness {
    pub fn from_market_time(market_time: Option<&str>, now_millis: i64) -> Self {
        let Some(at) = market_time.and_then(parse_market_timestamp) else {
            return Self::Unknown;
        };
        let age = now_millis.saturating_sub(at);
        if age < -30_000 {
            return Self::Unknown;
        }
        Self::from_age_secs(age.max(0) as u64 / 1_000)
    }

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

    /// Freshness uses the exchange timestamp, never the HTTP fetch timestamp.
    pub fn effective_freshness(&self, now_millis: i64) -> Freshness {
        if self.freshness == Freshness::Stale {
            Freshness::Stale
        } else {
            Freshness::from_market_time(self.market_time.as_deref(), now_millis)
        }
    }

    pub fn freshness_label(&self) -> &'static str {
        match self.effective_freshness(chrono::Utc::now().timestamp_millis()) {
            Freshness::Live => "实时",
            Freshness::Delayed => "延迟",
            Freshness::Stale => "过期",
            Freshness::Unknown => "时效未知",
        }
    }

    pub fn as_of_label(&self) -> &str {
        self.market_time.as_deref().unwrap_or("行情时间未知")
    }

    /// Canonical status for compact surfaces. The timestamp is provider evidence,
    /// never the wall clock or HTTP receipt time.
    pub fn display_status(&self, work: bool, now_millis: i64) -> String {
        let label = if !self.usable() {
            if work { "unavailable" } else { "缺失" }
        } else {
            match (work, self.effective_freshness(now_millis)) {
                (true, Freshness::Live) => "live",
                (true, Freshness::Delayed) => "delayed",
                (true, Freshness::Stale) => "stale",
                (true, Freshness::Unknown) => "time unknown",
                (false, Freshness::Live) => "实时",
                (false, Freshness::Delayed) => "延迟",
                (false, Freshness::Stale) => "过期",
                (false, Freshness::Unknown) => "时效未知",
            }
        };
        let time = self.market_time.as_deref().unwrap_or(if work {
            "time unknown"
        } else {
            "时间未知"
        });
        format!("{label} · {time}")
    }

    pub fn stale(mut self) -> Self {
        self.freshness = Freshness::Stale;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Both supported exchanges use UTC+8 without daylight saving time.
pub fn exchange_offset() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(8 * 60 * 60).expect("valid exchange offset")
}

pub fn parse_market_timestamp(value: &str) -> Option<i64> {
    use chrono::TimeZone;
    let value = value.trim();
    if let Ok(time) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(time.timestamp_millis());
    }
    [
        "%Y%m%d%H%M%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M:%S",
        "%Y/%m/%d %H:%M",
    ]
    .iter()
    .find_map(|format| chrono::NaiveDateTime::parse_from_str(value, format).ok())
    .and_then(|time| exchange_offset().from_local_datetime(&time).single())
    .map(|time| time.timestamp_millis())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BarKind {
    Daily,
    Intraday,
    Minute(u32),
}

/// A code alone cannot identify the data painted under a chart header.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SeriesIdentity {
    pub instrument: InstrumentId,
    pub bars: BarKind,
    pub adjustment: Adjustment,
}

impl SeriesIdentity {
    pub fn stock(code: &str, bars: BarKind, adjustment: Adjustment) -> Option<Self> {
        Some(Self {
            instrument: InstrumentId {
                market: Market::for_code(code)?,
                asset_type: AssetType::Stock,
                code: code.into(),
            },
            bars,
            adjustment,
        })
    }

    /// Drawings use bar indices, so their scope includes the complete timestamp layout.
    /// Quotes can update prices without changing this key; a shifted history cannot reuse it.
    pub fn drawing_scope_key<'a>(
        &self,
        range: &str,
        times: impl IntoIterator<Item = &'a str>,
    ) -> String {
        let mut hash = 0xcbf29ce484222325_u64;
        for time in times {
            for byte in (time.len() as u64)
                .to_le_bytes()
                .into_iter()
                .chain(time.bytes())
            {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        format!("drawing:v2:{}:{range}:{hash:016x}", self.storage_key())
    }

    pub fn storage_key(&self) -> String {
        format!(
            "{}:{:?}:{:?}",
            self.instrument.storage_key(),
            self.bars,
            self.adjustment
        )
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::*;

    #[test]
    fn unknown_market_time_never_claims_live() {
        assert_eq!(Freshness::from_market_time(None, 0), Freshness::Unknown);
        assert_eq!(
            Freshness::from_market_time(Some("bad timestamp"), 0),
            Freshness::Unknown
        );
    }

    #[test]
    fn provider_time_ages_even_when_price_is_unchanged() {
        let timestamp = "2026-10-09 10:00:00";
        let now = parse_market_timestamp(timestamp).unwrap();
        assert_eq!(
            Freshness::from_market_time(Some(timestamp), now),
            Freshness::Live
        );
        assert_eq!(
            Freshness::from_market_time(Some(timestamp), now + 31_000),
            Freshness::Delayed
        );
        assert_eq!(
            Freshness::from_market_time(Some(timestamp), now + 301_000),
            Freshness::Stale
        );
        assert_eq!(parse_market_timestamp("20261009100000"), Some(now));
        assert_eq!(parse_market_timestamp("2026/10/09 10:00:00"), Some(now));
        assert_eq!(parse_market_timestamp(" 2026/10/09 10:00 "), Some(now));
        assert_eq!(parse_market_timestamp("2026-10-09T02:00:00Z"), Some(now));
    }
    #[test]
    fn drawing_scope_rejects_changed_period_range_and_index_layout() {
        let daily = SeriesIdentity::stock("600519", BarKind::Daily, Adjustment::Forward).unwrap();
        let dates = ["2026-10-08", "2026-10-09"];
        let key = daily.drawing_scope_key("3M", dates);
        assert_eq!(key, daily.drawing_scope_key("3M", dates));
        assert_ne!(key, "600519"); // Legacy code-only drawings are preserved but never auto-adopted.
        assert_ne!(key, daily.drawing_scope_key("1M", dates));
        assert_ne!(
            key,
            daily.drawing_scope_key("3M", ["2026-10-09", "2026-10-12"])
        );
        let minutes =
            SeriesIdentity::stock("600519", BarKind::Minute(5), Adjustment::None).unwrap();
        assert_ne!(key, minutes.drawing_scope_key("3M", dates));
        let intraday =
            SeriesIdentity::stock("600519", BarKind::Intraday, Adjustment::None).unwrap();
        assert_ne!(
            intraday.drawing_scope_key("20261009", ["09:30", "09:31"]),
            intraday.drawing_scope_key("20261012", ["09:30", "09:31"])
        );
    }
    #[test]
    fn compact_status_preserves_old_or_missing_provider_timestamp() {
        let mut quote = QuoteRecord {
            code: "600519".into(),
            market: Market::AShare,
            currency: Currency::Cny,
            name: "fixture".into(),
            price: Some(10.0),
            change_pct: Some(1.0),
            volume: Some(1),
            source: "fixture".into(),
            fetched_at: 9_999_999_999_999,
            market_time: Some("2000-01-01 09:30:00".into()),
            availability: Availability::Available,
            freshness: Freshness::Live,
        };
        let now = 1_800_000_000_000;
        let work = quote.display_status(true, now);
        assert!(work.contains("stale"));
        assert!(work.contains("2000-01-01 09:30:00"));
        assert!(!work.contains("live"));
        assert!(quote.display_status(false, now).contains("过期"));
        quote.market_time = None;
        assert!(quote.display_status(true, now).contains("time unknown"));
        quote.availability = Availability::Missing;
        assert!(quote.display_status(true, now).contains("unavailable"));
    }
}
