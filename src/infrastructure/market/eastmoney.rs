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
const FUNDAMENTALS_SOURCE: &str = "东方财富财务/公告/分红；百度股市通估值历史";
const FINANCIAL_SOURCE: &str = "东方财富财务数据中心";
const DIVIDEND_SOURCE: &str = "东方财富分红派息";
const VALUATION_SOURCE: &str = "百度股市通估值历史（近三年）";

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
        let reports = match market {
            Market::AShare => data::eastmoney::fetch_fundamental_reports(code, report_limit),
            Market::HongKong => data::eastmoney::fetch_hk_fundamental_reports(code, report_limit),
        }
        .map_err(|error| provider_error(FUNDAMENTALS_SOURCE, error))?;
        let dividend_continuity = data::eastmoney::fetch_dividend_continuity(code, &reports)
            .map_err(|error| provider_error(FUNDAMENTALS_SOURCE, error))?;
        let pe_percentiles = data::baidu::fetch_valuation_percentiles(code, "市盈率(TTM)")
            .map_err(|error| provider_error(FUNDAMENTALS_SOURCE, error))?;
        let pb_percentiles = data::baidu::fetch_valuation_percentiles(code, "市净率")
            .map_err(|error| provider_error(FUNDAMENTALS_SOURCE, error))?;
        let currency = reports
            .first()
            .map(|report| report.currency)
            .unwrap_or_else(|| market.currency());
        let mut metrics = Vec::with_capacity(
            reports.len() * 8
                + dividend_continuity.len()
                + pe_percentiles.len()
                + pb_percentiles.len(),
        );
        for report in reports {
            let is_annual = report.is_annual;
            let period = report.reporting_period;
            let announced = report.announced_on;
            append_metric(
                &mut metrics,
                "roe_pct",
                report.roe_pct,
                "%",
                &period,
                &announced,
                FINANCIAL_SOURCE,
            );
            append_metric(
                &mut metrics,
                "roic_pct",
                report.roic_pct,
                "%",
                &period,
                &announced,
                FINANCIAL_SOURCE,
            );
            append_metric(
                &mut metrics,
                "operating_cash_to_profit",
                report.operating_cash_to_profit,
                "ratio",
                &period,
                &announced,
                FINANCIAL_SOURCE,
            );
            append_metric(
                &mut metrics,
                "debt_ratio_pct",
                report.debt_ratio_pct,
                "%",
                &period,
                &announced,
                FINANCIAL_SOURCE,
            );
            append_metric(
                &mut metrics,
                "revenue_growth_pct",
                report.revenue_growth_pct,
                "%",
                &period,
                &announced,
                FINANCIAL_SOURCE,
            );
            append_metric(
                &mut metrics,
                "profit_growth_pct",
                report.profit_growth_pct,
                "%",
                &period,
                &announced,
                FINANCIAL_SOURCE,
            );
            append_metric(
                &mut metrics,
                "goodwill_ratio_pct",
                report.goodwill_ratio_pct,
                "%",
                &period,
                &announced,
                FINANCIAL_SOURCE,
            );
            if is_annual || report.audit_risk_flag.is_some() {
                append_metric(
                    &mut metrics,
                    "audit_risk_flag",
                    report.audit_risk_flag,
                    "bool",
                    &period,
                    &announced,
                    FINANCIAL_SOURCE,
                );
            }
        }
        for point in dividend_continuity {
            append_metric(
                &mut metrics,
                "dividend_continuity_years",
                point.consecutive_years.map(f64::from),
                "年",
                &format!("{}-12-31", point.fiscal_year),
                &point.announced_on,
                DIVIDEND_SOURCE,
            );
        }
        append_valuation_metrics(&mut metrics, "pe_ttm_percentile_pct", pe_percentiles);
        append_valuation_metrics(&mut metrics, "pb_percentile_pct", pb_percentiles);
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
    source: &str,
) {
    metrics.push(ReportedMetric {
        name: name.into(),
        value,
        unit: unit.into(),
        reporting_period: reporting_period.into(),
        announced_on: announced_on.into(),
        source: source.into(),
    });
}

fn append_valuation_metrics(
    metrics: &mut Vec<ReportedMetric>,
    name: &str,
    points: Vec<data::baidu::ValuationPercentilePoint>,
) {
    for point in points.into_iter().filter(|point| {
        let start = chrono::NaiveDate::parse_from_str(&point.window_start, "%Y-%m-%d");
        let end = chrono::NaiveDate::parse_from_str(&point.observed_on, "%Y-%m-%d");
        match (start, end) {
            (Ok(start), Ok(end)) => (end - start).num_days() >= 1_080,
            _ => false,
        }
    }) {
        metrics.push(ReportedMetric {
            name: name.into(),
            value: Some(point.percentile_pct),
            unit: "%".into(),
            reporting_period: format!("{}..{}", point.window_start, point.observed_on),
            announced_on: point.observed_on,
            source: format!("{VALUATION_SOURCE}，{} 个日点", point.sample_size),
        });
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

#[cfg(test)]
mod fundamental_provider_tests {
    use super::*;
    use crate::domain::fundamentals::quality_gate;

    #[test]
    fn valuation_metrics_require_an_almost_three_year_window() {
        let mut metrics = Vec::new();
        append_valuation_metrics(
            &mut metrics,
            "pe_ttm_percentile_pct",
            vec![
                data::baidu::ValuationPercentilePoint {
                    observed_on: "2024-01-01".into(),
                    window_start: "2023-01-01".into(),
                    percentile_pct: 50.0,
                    sample_size: 366,
                },
                data::baidu::ValuationPercentilePoint {
                    observed_on: "2026-01-01".into(),
                    window_start: "2023-01-01".into(),
                    percentile_pct: 60.0,
                    sample_size: 1_097,
                },
            ],
        );
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].announced_on, "2026-01-01");
    }

    #[test]
    #[ignore = "requires public financial-data network"]
    fn a_share_enriched_fundamentals_smoke() {
        let snapshot = EastmoneyProvider
            .fetch_fundamentals("600519", 8)
            .expect("A-share enriched fundamentals");
        assert_eq!(snapshot.market, Market::AShare);
        assert!(snapshot.metrics.iter().any(|metric| {
            metric.name == "dividend_continuity_years" && metric.value.is_some()
        }));
        assert!(
            snapshot
                .metrics
                .iter()
                .any(|metric| metric.name == "pe_ttm_percentile_pct")
        );
        assert!(
            snapshot
                .metrics
                .iter()
                .any(|metric| metric.name == "pb_percentile_pct")
        );
        let today = chrono::Utc::now().date_naive().to_string();
        let gate = quality_gate(&snapshot.metrics, &today);
        assert!(
            gate.unknown.iter().all(|item| !item.contains("连续分红")
                && !item.contains("PE(TTM)")
                && !item.contains("PB 三年")),
            "gate={gate:?}"
        );
    }

    #[test]
    #[ignore = "requires public financial-data network"]
    fn hong_kong_enriched_fundamentals_smoke() {
        let snapshot = EastmoneyProvider
            .fetch_fundamentals("00700", 8)
            .expect("Hong Kong enriched fundamentals");
        assert_eq!(snapshot.market, Market::HongKong);
        assert!(snapshot.metrics.iter().any(|metric| {
            metric.name == "dividend_continuity_years" && metric.value.is_some()
        }));
        assert!(
            snapshot
                .metrics
                .iter()
                .any(|metric| metric.name == "pe_ttm_percentile_pct")
        );
        assert!(
            snapshot
                .metrics
                .iter()
                .any(|metric| metric.name == "pb_percentile_pct")
        );
        let today = chrono::Utc::now().date_naive().to_string();
        let gate = quality_gate(&snapshot.metrics, &today);
        assert!(gate.unknown.iter().any(|item| item.contains("审计")));
    }
}
