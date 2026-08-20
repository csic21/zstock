use crate::domain::strategy_library::StrategyLibraryRecord;

pub trait StrategyLibraryRepository: Send + Sync {
    fn save_library_record(&self, record: &StrategyLibraryRecord) -> anyhow::Result<()>;
    fn list_library_records(&self) -> anyhow::Result<Vec<StrategyLibraryRecord>>;
    fn dismiss_library_record(&self, record_id: &str) -> anyhow::Result<bool>;
    fn library_initialized(&self) -> anyhow::Result<bool>;
    fn library_contains_strategy(&self, strategy_id: &str) -> anyhow::Result<bool>;
    fn list_library_strategy_ids(&self) -> anyhow::Result<Vec<String>>;
}
