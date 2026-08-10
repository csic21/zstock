use serde::{Deserialize, Serialize};

use crate::domain::dataset::FrozenSeries;
use crate::domain::market::InstrumentId;
use crate::services::dataset_repository::{DatasetRepository, IngestSummary};
use crate::services::market_data::KlineProvider;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestFailure {
    pub instrument: InstrumentId,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchIngestReport {
    pub requested: usize,
    pub completed: usize,
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub failures: Vec<IngestFailure>,
}

pub fn ingest_instruments(
    provider: &dyn KlineProvider,
    repository: &dyn DatasetRepository,
    instruments: &[InstrumentId],
    limit: usize,
) -> BatchIngestReport {
    let mut report = BatchIngestReport {
        requested: instruments.len(),
        completed: 0,
        inserted: 0,
        updated: 0,
        unchanged: 0,
        failures: Vec::new(),
    };
    for instrument in instruments {
        let result = provider
            .fetch_klines(&instrument.code, limit)
            .map_err(|error| error.to_string())
            .and_then(|fetched| {
                if fetched.market != instrument.market || fetched.code != instrument.code {
                    return Err("provider returned a different instrument".into());
                }
                repository
                    .upsert_series(&FrozenSeries {
                        instrument: instrument.clone(),
                        source: fetched.source,
                        adjustment: fetched.adjustment,
                        candles: fetched.candles,
                    })
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(IngestSummary {
                inserted,
                updated,
                unchanged,
            }) => {
                report.completed += 1;
                report.inserted += inserted;
                report.updated += updated;
                report.unchanged += unchanged;
            }
            Err(message) => report.failures.push(IngestFailure {
                instrument: instrument.clone(),
                message,
            }),
        }
    }
    report
}
