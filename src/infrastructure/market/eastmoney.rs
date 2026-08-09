use crate::data;
use crate::domain::fundamentals::{FundamentalSnapshot, ReportedMetric};
use crate::domain::market::{
    Adjustment, Availability, CandleRecord, Freshness, KlineSeries, Market, QuoteRecord, SearchHit,
};
use crate::services::fundamentals::FundamentalsProvider;
use crate::services::market_data::{
    KlineProvider, ProviderError, ProviderErrorKind, QuoteProvider, SearchProvider,
};

#[derive(Debug, Default)]
pub struct EastmoneyProvider;

const PROVIDER: &str = "东方财富";
const FUNDAMENTALS_SOURCE: &str = "东方财富财务数据中心";

impl QuoteProvider for EastmoneyProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn fetch_quotes(&self, codes: &[String]) -> Result<Vec<QuoteRecord>, ProviderError> {
        let fetched_at = now_millis();
        data::eastmoney::fetch_quotes(codes)
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

impl KlineProvider for EastmoneyProvider {
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
        data::eastmoney::fetch_klines(code, limit)
            .map(|(_, market_time, candles)| KlineSeries {
                code: code.into(),
                market,
                currency: market.currency(),
                source: PROVIDER.into(),
                as_of: now_millis(),
                market_time: Some(market_time),
                adjustment: Adjustment::Forward,
                candles: candles.into_iter().map(Into::into).collect(),
            })
            .map_err(|error| provider_error(PROVIDER, error))
    }
}

impl SearchProvider for EastmoneyProvider {
    fn name(&self) -> &'static str {
        <Self as QuoteProvider>::name(self)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, ProviderError> {
        data::eastmoney::search_symbols(query, limit)
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

impl FundamentalsProvider for EastmoneyProvider {
    fn name(&self) -> &'static str {
        FUNDAMENTALS_SOURCE
    }

    fn fetch_fundamentals(
        &self,
        code: &str,
        report_limit: usize,
    ) -> Result<FundamentalSnapshot, ProviderError> {
        let market = Market::for_code(code).ok_or_else(|| {
            ProviderError::new(
                FUNDAMENTALS_SOURCE,
                ProviderErrorKind::InvalidPayload,
                "unknown market code",
            )
        })?;
        if market != Market::AShare {
            return Err(ProviderError::new(
                FUNDAMENTALS_SOURCE,
                ProviderErrorKind::Unavailable,
                "point-in-time financial provider currently supports A shares only",
            ));
        }
        let reports = data::eastmoney::fetch_fundamental_reports(code, report_limit)
            .map_err(|error| provider_error(FUNDAMENTALS_SOURCE, error))?;
        let currency = reports
            .first()
            .map(|report| report.currency)
            .unwrap_or_else(|| market.currency());
        let mut metrics = Vec::with_capacity(reports.len() * 8);
        for report in reports {
            let period = report.reporting_period;
            let announced = report.announced_on;
            append_metric(
                &mut metrics,
                "roe_pct",
                report.roe_pct,
                "%",
                &period,
                &announced,
            );
            append_metric(
                &mut metrics,
                "roic_pct",
                report.roic_pct,
                "%",
                &period,
                &announced,
            );
            append_metric(
                &mut metrics,
                "operating_cash_to_profit",
                report.operating_cash_to_profit,
                "ratio",
                &period,
                &announced,
            );
            append_metric(
                &mut metrics,
                "debt_ratio_pct",
                report.debt_ratio_pct,
                "%",
                &period,
                &announced,
            );
            append_metric(
                &mut metrics,
                "revenue_growth_pct",
                report.revenue_growth_pct,
                "%",
                &period,
                &announced,
            );
            append_metric(
                &mut metrics,
                "profit_growth_pct",
                report.profit_growth_pct,
                "%",
                &period,
                &announced,
            );
            append_metric(
                &mut metrics,
                "goodwill_ratio_pct",
                report.goodwill_ratio_pct,
                "%",
                &period,
                &announced,
            );
            append_metric(
                &mut metrics,
                "audit_risk_flag",
                report.audit_risk_flag,
                "bool",
                &period,
                &announced,
            );
        }
        Ok(FundamentalSnapshot {
            code: code.into(),
            market,
            currency,
            fetched_at: now_millis(),
            source: FUNDAMENTALS_SOURCE.into(),
            metrics,
        })
    }
}

fn append_metric(
    metrics: &mut Vec<ReportedMetric>,
    name: &str,
    value: Option<f64>,
    unit: &str,
    reporting_period: &str,
    announced_on: &str,
) {
    metrics.push(ReportedMetric {
        name: name.into(),
        value,
        unit: unit.into(),
        reporting_period: reporting_period.into(),
        announced_on: announced_on.into(),
        source: FUNDAMENTALS_SOURCE.into(),
    });
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

fn now_millis() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl From<crate::model::Candle> for CandleRecord {
    fn from(value: crate::model::Candle) -> Self {
        Self {
            time: value.date.to_string(),
            open: value.open,
            high: value.high,
            low: value.low,
            close: value.close,
            volume: value.volume,
        }
    }
}
