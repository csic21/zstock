use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::domain::market::{Availability, Freshness, Market, QuoteRecord};
use crate::model::normalize_code;
use crate::services::market_data::{ProviderError, QuoteProvider};

use super::eastmoney::EastmoneyProvider;
use super::tencent::TencentProvider;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderHealth {
    pub successes: u64,
    pub failures: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct QuoteBatch {
    /// One row per valid input code, in the exact requested order (including duplicates).
    pub records: Vec<QuoteRecord>,
    pub errors: Vec<ProviderError>,
}

pub struct MarketDataService {
    a_primary: Arc<dyn QuoteProvider>,
    a_fallback: Arc<dyn QuoteProvider>,
    h_primary: Arc<dyn QuoteProvider>,
    h_fallback: Arc<dyn QuoteProvider>,
    last_good: HashMap<String, QuoteRecord>,
    health: HashMap<String, ProviderHealth>,
}

impl Default for MarketDataService {
    fn default() -> Self {
        let eastmoney: Arc<dyn QuoteProvider> = Arc::new(EastmoneyProvider);
        let tencent: Arc<dyn QuoteProvider> = Arc::new(TencentProvider);
        Self::new(
            Arc::clone(&eastmoney),
            Arc::clone(&tencent),
            Arc::clone(&tencent),
            Arc::clone(&eastmoney),
        )
    }
}

impl MarketDataService {
    pub fn new(
        a_primary: Arc<dyn QuoteProvider>,
        a_fallback: Arc<dyn QuoteProvider>,
        h_primary: Arc<dyn QuoteProvider>,
        h_fallback: Arc<dyn QuoteProvider>,
    ) -> Self {
        Self {
            a_primary,
            a_fallback,
            h_primary,
            h_fallback,
            last_good: HashMap::new(),
            health: HashMap::new(),
        }
    }

    pub fn health(&self) -> &HashMap<String, ProviderHealth> {
        &self.health
    }

    pub fn fetch_quotes(&mut self, requested: &[String]) -> QuoteBatch {
        let normalized: Vec<String> = requested
            .iter()
            .filter_map(|code| normalize_code(code))
            .collect();
        let mut a_codes = unique_for_market(&normalized, Market::AShare);
        let mut h_codes = unique_for_market(&normalized, Market::HongKong);
        let mut by_code = HashMap::new();
        let mut errors = Vec::new();

        self.fill_group(
            &mut a_codes,
            Arc::clone(&self.a_primary),
            Arc::clone(&self.a_fallback),
            &mut by_code,
            &mut errors,
        );
        self.fill_group(
            &mut h_codes,
            Arc::clone(&self.h_primary),
            Arc::clone(&self.h_fallback),
            &mut by_code,
            &mut errors,
        );

        for (code, record) in &by_code {
            if record.usable() {
                self.last_good.insert(code.clone(), record.clone());
            }
        }

        let records = normalized
            .into_iter()
            .filter_map(|code| {
                if let Some(record) = by_code.get(&code) {
                    return Some(record.clone());
                }
                if let Some(record) = self.last_good.get(&code) {
                    return Some(record.clone().stale());
                }
                let market = Market::for_code(&code)?;
                Some(QuoteRecord {
                    code,
                    market,
                    currency: market.currency(),
                    name: String::new(),
                    price: None,
                    change_pct: None,
                    volume: None,
                    source: "unavailable".into(),
                    fetched_at: chrono::Utc::now().timestamp_millis(),
                    market_time: None,
                    availability: Availability::Missing,
                    freshness: Freshness::Stale,
                })
            })
            .collect();

        QuoteBatch { records, errors }
    }

    fn fill_group(
        &mut self,
        codes: &mut [String],
        primary: Arc<dyn QuoteProvider>,
        fallback: Arc<dyn QuoteProvider>,
        by_code: &mut HashMap<String, QuoteRecord>,
        errors: &mut Vec<ProviderError>,
    ) {
        if codes.is_empty() {
            return;
        }
        let requested: HashSet<_> = codes.iter().cloned().collect();
        match primary.fetch_quotes(codes) {
            Ok(records) => {
                self.record_success(primary.name());
                merge_usable(records, &requested, by_code);
            }
            Err(error) => {
                self.record_failure(primary.name(), &error);
                errors.push(error);
            }
        }

        let missing: Vec<_> = codes
            .iter()
            .filter(|code| !by_code.get(*code).is_some_and(QuoteRecord::usable))
            .cloned()
            .collect();
        if missing.is_empty() {
            return;
        }
        let requested_missing: HashSet<_> = missing.iter().cloned().collect();
        match fallback.fetch_quotes(&missing) {
            Ok(records) => {
                self.record_success(fallback.name());
                merge_usable(records, &requested_missing, by_code);
            }
            Err(error) => {
                self.record_failure(fallback.name(), &error);
                errors.push(error);
            }
        }
    }

    fn record_success(&mut self, provider: &str) {
        let health = self.health.entry(provider.into()).or_default();
        health.successes += 1;
        health.last_error = None;
    }

    fn record_failure(&mut self, provider: &str, error: &ProviderError) {
        let health = self.health.entry(provider.into()).or_default();
        health.failures += 1;
        health.last_error = Some(error.message.clone());
    }
}

fn unique_for_market(codes: &[String], market: Market) -> Vec<String> {
    let mut seen = HashSet::new();
    codes
        .iter()
        .filter(|code| Market::for_code(code) == Some(market))
        .filter(|code| seen.insert((*code).clone()))
        .cloned()
        .collect()
}

fn merge_usable(
    records: Vec<QuoteRecord>,
    requested: &HashSet<String>,
    output: &mut HashMap<String, QuoteRecord>,
) {
    for record in records {
        if requested.contains(&record.code) && record.usable() {
            output.insert(record.code.clone(), record);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crate::domain::money::Currency;
    use crate::services::market_data::ProviderErrorKind;

    use super::*;

    struct MockProvider {
        name: &'static str,
        rows: Mutex<Result<Vec<QuoteRecord>, ProviderError>>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl MockProvider {
        fn rows(name: &'static str, rows: Vec<QuoteRecord>) -> Arc<Self> {
            Arc::new(Self {
                name,
                rows: Mutex::new(Ok(rows)),
                calls: Mutex::new(Vec::new()),
            })
        }

        fn failing(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                rows: Mutex::new(Err(ProviderError::new(
                    name,
                    ProviderErrorKind::Timeout,
                    "timeout",
                ))),
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    impl QuoteProvider for MockProvider {
        fn name(&self) -> &'static str {
            self.name
        }

        fn fetch_quotes(&self, codes: &[String]) -> Result<Vec<QuoteRecord>, ProviderError> {
            self.calls.lock().unwrap().push(codes.to_vec());
            self.rows.lock().unwrap().clone()
        }
    }

    fn quote(code: &str, source: &str, price: f64) -> QuoteRecord {
        let market = Market::for_code(code).unwrap();
        QuoteRecord {
            code: code.into(),
            market,
            currency: match market {
                Market::AShare => Currency::Cny,
                Market::HongKong => Currency::Hkd,
            },
            name: code.into(),
            price: Some(price),
            change_pct: Some(1.0),
            volume: Some(1),
            source: source.into(),
            fetched_at: 1,
            market_time: Some("fixture".into()),
            availability: Availability::Available,
            freshness: Freshness::Live,
        }
    }

    #[test]
    fn fills_only_missing_codes_and_preserves_order_and_duplicates() {
        let a_primary = MockProvider::rows("east", vec![quote("000001", "east", 10.0)]);
        let a_fallback = MockProvider::rows("tencent", vec![quote("600519", "tencent", 20.0)]);
        let h_primary = MockProvider::rows("tencent", vec![quote("00700", "tencent", 30.0)]);
        let h_fallback = MockProvider::rows("east", Vec::new());
        let mut service =
            MarketDataService::new(a_primary.clone(), a_fallback.clone(), h_primary, h_fallback);
        let batch = service.fetch_quotes(&[
            "600519".into(),
            "00700".into(),
            "000001".into(),
            "600519".into(),
        ]);
        assert_eq!(
            batch
                .records
                .iter()
                .map(|row| row.code.as_str())
                .collect::<Vec<_>>(),
            vec!["600519", "00700", "000001", "600519"]
        );
        assert_eq!(
            a_fallback.calls.lock().unwrap().as_slice(),
            &[vec!["600519".to_string()]]
        );
        assert_eq!(batch.records[0].source, "tencent");
        assert_eq!(batch.records[1].currency, Currency::Hkd);
    }

    #[test]
    fn both_failures_retain_last_good_as_stale_instead_of_zero() {
        let primary = MockProvider::rows("p", vec![quote("600519", "p", 88.0)]);
        let fallback = MockProvider::rows("f", Vec::new());
        let h = MockProvider::rows("h", Vec::new());
        let mut service = MarketDataService::new(primary.clone(), fallback.clone(), h.clone(), h);
        let first = service.fetch_quotes(&["600519".into()]);
        assert_eq!(first.records[0].price, Some(88.0));
        *primary.rows.lock().unwrap() = Err(ProviderError::new(
            "p",
            ProviderErrorKind::Timeout,
            "timeout",
        ));
        *fallback.rows.lock().unwrap() = Ok(Vec::new());
        let second = service.fetch_quotes(&["600519".into()]);
        assert_eq!(second.records[0].price, Some(88.0));
        assert_eq!(second.records[0].freshness, Freshness::Stale);
    }

    #[test]
    fn empty_and_failed_responses_produce_explicit_missing_records() {
        let failed = MockProvider::failing("failed");
        let empty = MockProvider::rows("empty", Vec::new());
        let mut service = MarketDataService::new(failed.clone(), empty.clone(), failed, empty);
        let batch = service.fetch_quotes(&["600519".into(), "00700".into()]);
        assert_eq!(batch.records.len(), 2);
        assert!(batch.records.iter().all(|row| row.price.is_none()));
        assert!(
            batch
                .records
                .iter()
                .all(|row| row.availability == Availability::Missing)
        );
    }

    #[test]
    fn applying_one_hundred_quotes_stays_inside_store_budget() {
        let codes = (0..100)
            .map(|index| format!("{index:06}"))
            .collect::<Vec<_>>();
        let rows = codes
            .iter()
            .enumerate()
            .map(|(index, code)| quote(code, "primary", 10.0 + index as f64))
            .collect();
        let primary = MockProvider::rows("primary", rows);
        let empty = MockProvider::rows("empty", Vec::new());
        let mut service = MarketDataService::new(primary, empty.clone(), empty.clone(), empty);
        let started = std::time::Instant::now();
        let batch = service.fetch_quotes(&codes);
        let elapsed = started.elapsed();
        assert_eq!(batch.records.len(), 100);
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "100-quote apply took {elapsed:?}"
        );
    }
}
