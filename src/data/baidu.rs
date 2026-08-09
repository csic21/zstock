//! Dated A-share and Hong Kong valuation history from Baidu Stock Connect.
//!
//! The public chart endpoint returns one value per calendar date. We calculate
//! an expanding percentile for every observation so a past signal can only see
//! the valuation distribution that existed on or before that date.

use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::model::{is_hk_code, normalize_code};

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

#[derive(Debug, Clone, PartialEq)]
pub struct ValuationPercentilePoint {
    pub observed_on: String,
    pub window_start: String,
    pub percentile_pct: f64,
    pub sample_size: usize,
}

static AGENT: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(8))
        .timeout_read(Duration::from_secs(15))
        .build()
});

pub fn fetch_valuation_percentiles(
    code: &str,
    indicator: &str,
) -> Result<Vec<ValuationPercentilePoint>> {
    let normalized = normalize_code(code)
        .ok_or_else(|| anyhow!("valuation provider requires a valid stock code"))?;
    let market = if is_hk_code(&normalized) { "hk" } else { "ab" };
    let value: Value = AGENT
        .get("https://finance.baidu.com/opendata")
        .set("User-Agent", UA)
        .set("Referer", "https://gushitong.baidu.com/")
        .query("openapi", "1")
        .query("dspName", "iphone")
        .query("tn", "tangram")
        .query("client", "app")
        .query("query", indicator)
        .query("code", &normalized)
        .query("word", "")
        .query("resource_id", "51171")
        .query("market", market)
        .query("tag", indicator)
        .query("chart_select", "近三年")
        .query("industry_select", "")
        .query("skip_industry", "1")
        .query("finClientType", "pc")
        .call()
        .map_err(|error| anyhow!("valuation request failed: {error}"))?
        .into_json()
        .context("parse valuation response")?;
    parse_valuation_percentiles(&value)
}

fn parse_valuation_percentiles(value: &Value) -> Result<Vec<ValuationPercentilePoint>> {
    let body = value
        .pointer("/Result/0/DisplayData/resultData/tplData/result/chartInfo/0/body")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("valuation response missing chart body"))?;
    let mut observations = Vec::with_capacity(body.len());
    for row in body {
        let Some(values) = row.as_array() else {
            continue;
        };
        let Some(date) = values.first().and_then(Value::as_str) else {
            continue;
        };
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .with_context(|| format!("invalid valuation date {date}"))?;
        let number = values.get(1).and_then(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(number) => number.parse().ok(),
            _ => None,
        });
        if let Some(number) = number.filter(|number| number.is_finite()) {
            observations.push((date.to_string(), number));
        }
    }
    observations.sort_by(|left, right| left.0.cmp(&right.0));
    observations.dedup_by(|left, right| {
        if left.0 == right.0 {
            *left = right.clone();
            true
        } else {
            false
        }
    });
    let Some(window_start) = observations.first().map(|(date, _)| date.clone()) else {
        bail!("valuation response contains no finite observations");
    };

    let mut sorted = Vec::<f64>::with_capacity(observations.len());
    let mut points = Vec::with_capacity(observations.len());
    for (observed_on, number) in observations {
        let insertion = sorted.partition_point(|value| value.total_cmp(&number).is_le());
        sorted.insert(insertion, number);
        let rank = sorted.partition_point(|value| value.total_cmp(&number).is_le());
        points.push(ValuationPercentilePoint {
            observed_on,
            window_start: window_start.clone(),
            percentile_pct: rank as f64 / sorted.len() as f64 * 100.0,
            sample_size: sorted.len(),
        });
    }
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanding_percentile_never_uses_future_values() {
        let value: Value = serde_json::json!({
            "Result": [{"DisplayData": {"resultData": {"tplData": {"result": {
                "chartInfo": [{"body": [
                    ["2026-01-01", "10"],
                    ["2026-01-02", "20"],
                    ["2026-01-03", "15"]
                ]}]
            }}}}}]
        });
        let points = parse_valuation_percentiles(&value).unwrap();
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].percentile_pct, 100.0);
        assert_eq!(points[1].percentile_pct, 100.0);
        assert!((points[2].percentile_pct - 66.666_666).abs() < 0.001);
        assert_eq!(points[2].sample_size, 3);
    }

    #[test]
    fn malformed_rows_are_not_treated_as_zero() {
        let value: Value = serde_json::json!({
            "Result": [{"DisplayData": {"resultData": {"tplData": {"result": {
                "chartInfo": [{"body": [
                    ["2026-01-01", "not-a-number"],
                    ["2026-01-02", "2.5"]
                ]}]
            }}}}}]
        });
        let points = parse_valuation_percentiles(&value).unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].observed_on, "2026-01-02");
    }
}
