use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::market::{Adjustment, CandleRecord, InstrumentId, Market};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateInterval {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataQualityIssueKind {
    EmptySeries,
    DuplicateDate,
    OutOfOrderDate,
    InvalidPrice,
    InvalidOhlc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataQualityIssue {
    pub instrument: InstrumentId,
    pub date: Option<String>,
    pub kind: DataQualityIssueKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrozenSeries {
    pub instrument: InstrumentId,
    pub source: String,
    pub adjustment: Adjustment,
    pub candles: Vec<CandleRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub id: String,
    pub created_at: String,
    pub market: Market,
    pub adjustment: Adjustment,
    pub source_versions: Vec<String>,
    pub instruments: Vec<InstrumentId>,
    pub interval: DateInterval,
    pub content_sha256: String,
    pub known_biases: Vec<String>,
    pub quality_issues: Vec<DataQualityIssue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrozenDataset {
    pub manifest: DatasetManifest,
    pub series: Vec<FrozenSeries>,
}

pub fn validate_series(series: &FrozenSeries) -> Vec<DataQualityIssue> {
    let mut issues = Vec::new();
    if series.candles.is_empty() {
        issues.push(DataQualityIssue {
            instrument: series.instrument.clone(),
            date: None,
            kind: DataQualityIssueKind::EmptySeries,
            message: "series contains no daily bars".into(),
        });
        return issues;
    }
    let mut previous: Option<&str> = None;
    for candle in &series.candles {
        if let Some(previous) = previous {
            let (kind, message) = if candle.time == previous {
                (
                    Some(DataQualityIssueKind::DuplicateDate),
                    "duplicate daily bar date",
                )
            } else if candle.time.as_str() < previous {
                (
                    Some(DataQualityIssueKind::OutOfOrderDate),
                    "daily bars must be strictly ordered",
                )
            } else {
                (None, "")
            };
            if let Some(kind) = kind {
                issues.push(DataQualityIssue {
                    instrument: series.instrument.clone(),
                    date: Some(candle.time.clone()),
                    kind,
                    message: message.into(),
                });
            }
        }
        previous = Some(&candle.time);

        let prices = [candle.open, candle.high, candle.low, candle.close];
        if !prices.iter().all(|price| price.is_finite() && *price > 0.0) {
            issues.push(DataQualityIssue {
                instrument: series.instrument.clone(),
                date: Some(candle.time.clone()),
                kind: DataQualityIssueKind::InvalidPrice,
                message: "OHLC prices must be finite and positive".into(),
            });
            continue;
        }
        if candle.high < candle.open.max(candle.close)
            || candle.low > candle.open.min(candle.close)
            || candle.high < candle.low
        {
            issues.push(DataQualityIssue {
                instrument: series.instrument.clone(),
                date: Some(candle.time.clone()),
                kind: DataQualityIssueKind::InvalidOhlc,
                message: "high/low do not contain open and close".into(),
            });
        }
    }
    issues
}

pub fn dataset_content_sha256(market: Market, series: &[FrozenSeries]) -> String {
    let mut ordered: Vec<_> = series.iter().collect();
    ordered.sort_by(|left, right| left.instrument.cmp(&right.instrument));
    let mut hasher = Sha256::new();
    hasher.update(b"zstock-frozen-dataset-v1\0");
    hash_bytes(
        &mut hasher,
        match market {
            Market::AShare => b"a_share",
            Market::HongKong => b"hong_kong",
        },
    );
    for item in ordered {
        hash_bytes(&mut hasher, item.instrument.storage_key().as_bytes());
        hash_bytes(&mut hasher, item.source.as_bytes());
        hash_bytes(
            &mut hasher,
            match item.adjustment {
                Adjustment::None => b"none",
                Adjustment::Forward => b"forward",
                Adjustment::Backward => b"backward",
            },
        );
        hasher.update((item.candles.len() as u64).to_be_bytes());
        for candle in &item.candles {
            hash_bytes(&mut hasher, candle.time.as_bytes());
            hasher.update(candle.open.to_bits().to_be_bytes());
            hasher.update(candle.high.to_bits().to_be_bytes());
            hasher.update(candle.low.to_bits().to_be_bytes());
            hasher.update(candle.close.to_bits().to_be_bytes());
            hasher.update(candle.volume.to_be_bytes());
        }
    }
    hex(&hasher.finalize())
}

fn hash_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::AssetType;

    fn series() -> FrozenSeries {
        FrozenSeries {
            instrument: InstrumentId {
                market: Market::AShare,
                asset_type: AssetType::Stock,
                code: "600000".into(),
            },
            source: "fixture-v1".into(),
            adjustment: Adjustment::Forward,
            candles: vec![CandleRecord {
                time: "2026-01-05".into(),
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 1_000,
            }],
        }
    }

    #[test]
    fn content_hash_covers_instrument_source_adjustment_and_ohlcv() {
        let original = series();
        let hash = dataset_content_sha256(Market::AShare, std::slice::from_ref(&original));
        let mut changed = original.clone();
        changed.candles[0].volume += 1;
        assert_ne!(hash, dataset_content_sha256(Market::AShare, &[changed]));
    }

    #[test]
    fn invalid_and_duplicate_bars_are_explicit() {
        let mut fixture = series();
        fixture.candles.push(fixture.candles[0].clone());
        fixture.candles[1].high = 1.0;
        let issues = validate_series(&fixture);
        assert!(
            issues
                .iter()
                .any(|issue| issue.kind == DataQualityIssueKind::DuplicateDate)
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.kind == DataQualityIssueKind::InvalidOhlc)
        );
    }
}
