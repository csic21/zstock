use crate::data;
use crate::domain::market::{
    Adjustment, Availability, KlineSeries, Market, QuoteRecord, SearchHit,
};
use crate::services::market_data::{
    KlineProvider, ProviderError, ProviderErrorKind, QuoteProvider, SearchProvider,
};

#[derive(Debug, Default)]
pub struct TencentProvider;

const PROVIDER: &str = "腾讯财经";

impl QuoteProvider for TencentProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn fetch_quotes(&self, codes: &[String]) -> Result<Vec<QuoteRecord>, ProviderError> {
        data::tencent::fetch_quotes(codes)
            .map(|values| values.into_iter().filter_map(quote_record).collect())
            .map_err(|error| provider_error(PROVIDER, error))
    }
}

/// Convert only finite supplied changes; absence must survive into canonical UI.
fn quote_record(value: data::eastmoney::QuoteTick) -> Option<QuoteRecord> {
    let market = Market::for_code(&value.code)?;
    let price = (value.last.is_finite() && value.last > 0.0).then_some(value.last);
    Some(QuoteRecord {
        code: value.code,
        market,
        currency: value.currency,
        name: value.name,
        price,
        change_pct: value.change_pct.filter(|change| change.is_finite()),
        volume: Some(value.volume),
        source: PROVIDER.into(),
        fetched_at: value.fetched_at,
        market_time: value.market_time,
        availability: if price.is_some() {
            value.availability
        } else {
            Availability::Invalid
        },
        freshness: value.freshness,
    })
}

impl KlineProvider for TencentProvider {
    fn name(&self) -> &'static str {
        <Self as QuoteProvider>::name(self)
    }

    fn fetch_klines(&self, code: &str, limit: usize) -> Result<KlineSeries, ProviderError> {
        let market = Market::for_code(code).ok_or_else(|| {
            ProviderError::new(
                PROVIDER,
                ProviderErrorKind::InvalidPayload,
                "unknown market code",
            )
        })?;
        data::tencent::fetch_klines(code, limit)
            .map(|(_, market_time, candles)| KlineSeries {
                code: code.into(),
                market,
                currency: market.currency(),
                source: PROVIDER.into(),
                as_of: chrono::Utc::now().timestamp_millis(),
                market_time: Some(market_time),
                adjustment: Adjustment::Forward,
                candles: candles.into_iter().map(Into::into).collect(),
            })
            .map_err(|error| provider_error(PROVIDER, error))
    }
}

impl SearchProvider for TencentProvider {
    fn name(&self) -> &'static str {
        <Self as QuoteProvider>::name(self)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, ProviderError> {
        data::tencent::search_symbols(query, limit)
            .map(|symbols| {
                symbols
                    .into_iter()
                    .filter_map(|symbol| {
                        Some(SearchHit {
                            market: Market::for_code(&symbol.code)?,
                            code: symbol.code,
                            name: symbol.name.to_string(),
                        })
                    })
                    .collect()
            })
            .map_err(|error| provider_error(PROVIDER, error))
    }
}

fn provider_error(provider: &str, error: anyhow::Error) -> ProviderError {
    let message = error.to_string();
    let kind = if message.to_ascii_lowercase().contains("timeout") {
        ProviderErrorKind::Timeout
    } else {
        ProviderErrorKind::Transport
    };
    ProviderError::new(provider, kind, message)
}

#[cfg(test)]
mod quote_adapter_tests {
    use super::*;

    fn assert_json_round_trip(record: &QuoteRecord) {
        let value = serde_json::to_value(record).expect("canonical quote serializes");
        assert_eq!(
            value["change_pct"],
            serde_json::to_value(record.change_pct).unwrap()
        );
        let decoded: QuoteRecord =
            serde_json::from_value(value).expect("canonical quote round-trip");
        assert_eq!(decoded.price, record.price);
        assert_eq!(decoded.change_pct, record.change_pct);
    }

    #[test]
    fn missing_invalid_and_zero_changes_survive_provider_to_canonical_conversion() {
        let mut fields = vec![""; 35];
        fields[1] = "fixture";
        fields[2] = "600519";
        fields[3] = "10.5";
        for change in ["", "-", "invalid", "NaN", "inf", "-inf"] {
            fields[32] = change;
            let body = format!("v_sh600519=\"{}\";", fields.join("~"));
            let record =
                quote_record(data::tencent::parse_quote_body(&body).unwrap().remove(0)).unwrap();
            assert_json_round_trip(&record);
            assert_eq!(record.price, Some(10.5));
            assert_eq!(record.availability, Availability::Available);
            assert_eq!(record.change_pct, None, "field {change:?}");
        }
        for change in ["0", "0.00"] {
            fields[32] = change;
            let body = format!("v_sh600519=\"{}\";", fields.join("~"));
            let record =
                quote_record(data::tencent::parse_quote_body(&body).unwrap().remove(0)).unwrap();
            assert_json_round_trip(&record);
            assert_eq!(record.change_pct, Some(0.0));
        }
    }
}
