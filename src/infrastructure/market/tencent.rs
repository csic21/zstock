use crate::data;
use crate::domain::market::{
    Adjustment, Availability, Freshness, KlineSeries, Market, QuoteRecord, SearchHit,
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
        let fetched_at = chrono::Utc::now().timestamp_millis();
        data::tencent::fetch_quotes(codes)
            .map(|values| {
                values
                    .into_iter()
                    .filter_map(|value| {
                        let market = Market::for_code(&value.code)?;
                        let price =
                            (value.last.is_finite() && value.last > 0.0).then_some(value.last);
                        Some(QuoteRecord {
                            code: value.code,
                            market,
                            currency: market.currency(),
                            name: value.name,
                            price,
                            change_pct: value.change_pct.is_finite().then_some(value.change_pct),
                            volume: Some(value.volume),
                            source: PROVIDER.into(),
                            fetched_at,
                            market_time: None,
                            availability: if price.is_some() {
                                Availability::Available
                            } else {
                                Availability::Invalid
                            },
                            freshness: Freshness::Live,
                        })
                    })
                    .collect()
            })
            .map_err(|error| provider_error(PROVIDER, error))
    }
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
