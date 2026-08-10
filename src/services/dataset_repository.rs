use crate::domain::dataset::{DatasetManifest, DateInterval, FrozenDataset, FrozenSeries};
use crate::domain::market::{Adjustment, InstrumentId, Market};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestSummary {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreezeDatasetRequest {
    pub market: Market,
    pub adjustment: Adjustment,
    pub source_versions: Vec<String>,
    pub instruments: Vec<InstrumentId>,
    pub interval: DateInterval,
    pub known_biases: Vec<String>,
}

pub trait DatasetRepository: Send + Sync {
    fn upsert_series(&self, series: &FrozenSeries) -> anyhow::Result<IngestSummary>;
    fn freeze_dataset(&self, request: &FreezeDatasetRequest) -> anyhow::Result<DatasetManifest>;
    fn load_dataset(&self, id: &str) -> anyhow::Result<Option<FrozenDataset>>;
    fn load_observation_dataset(
        &self,
        id: &str,
        _as_of: &str,
    ) -> anyhow::Result<Option<FrozenDataset>> {
        self.load_dataset(id)
    }
    fn list_manifests(&self) -> anyhow::Result<Vec<DatasetManifest>>;
}
