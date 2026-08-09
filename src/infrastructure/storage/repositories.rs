use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::data::journal::Journal;
use crate::data::portfolio::Portfolio;
use crate::services::repositories::{ConfigRepository, JournalRepository, PortfolioRepository};
use crate::storage::AppConfig;

use super::json_store;
use super::migrations::DocumentKind;

pub struct JsonConfigRepository {
    path: PathBuf,
}

impl JsonConfigRepository {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl ConfigRepository for JsonConfigRepository {
    fn load(&self) -> Result<AppConfig> {
        json_store::load(&self.path, DocumentKind::Config)
            .map(|loaded| loaded.value)
            .map_err(anyhow::Error::new)
    }

    fn save(&self, value: &AppConfig) -> Result<()> {
        json_store::save(&self.path, value)
    }
}

pub struct JsonPortfolioRepository {
    path: PathBuf,
}

impl JsonPortfolioRepository {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl PortfolioRepository for JsonPortfolioRepository {
    fn load(&self) -> Result<Portfolio> {
        json_store::load(&self.path, DocumentKind::Portfolio)
            .map(|loaded| loaded.value)
            .map_err(anyhow::Error::new)
    }

    fn save(&self, value: &Portfolio) -> Result<()> {
        json_store::save(&self.path, value)
    }
}

pub struct JsonJournalRepository {
    path: PathBuf,
}

impl JsonJournalRepository {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl JournalRepository for JsonJournalRepository {
    fn load(&self) -> Result<Journal> {
        json_store::load(&self.path, DocumentKind::Journal)
            .map(|loaded| loaded.value)
            .map_err(anyhow::Error::new)
    }

    fn save(&self, value: &Journal) -> Result<()> {
        json_store::save(&self.path, value)
    }
}

pub fn migrate_and_save<T>(
    path: &Path,
    _kind: DocumentKind,
    loaded: &json_store::Loaded<T>,
) -> Result<()>
where
    T: serde::Serialize,
{
    if !loaded.migration.migrated {
        return Ok(());
    }
    json_store::backup_before_migration(path, loaded.migration.from_version)
        .context("create pre-migration backup")?;
    json_store::save(path, &loaded.value)
}
