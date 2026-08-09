use crate::data::journal::Journal;
use crate::data::portfolio::Portfolio;
use crate::storage::AppConfig;

pub trait ConfigRepository {
    fn load(&self) -> anyhow::Result<AppConfig>;
    fn save(&self, value: &AppConfig) -> anyhow::Result<()>;
}

pub trait PortfolioRepository {
    fn load(&self) -> anyhow::Result<Portfolio>;
    fn save(&self, value: &Portfolio) -> anyhow::Result<()>;
}

pub trait JournalRepository {
    fn load(&self) -> anyhow::Result<Journal>;
    fn save(&self, value: &Journal) -> anyhow::Result<()>;
}
