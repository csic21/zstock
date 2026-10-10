//! Fail-closed financial document storage. An unreadable document is never an
//! empty writable document. Recovery locks remain until an explicit validated
//! restore/reset has durably preserved the original bytes.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail, ensure};
use serde::{Serialize, de::DeserializeOwned};

use super::{
    json_store::{self, LoadError},
    migrations::DocumentKind,
};
use crate::data::{journal::Journal, portfolio::Portfolio};

#[derive(Debug, Clone)]
pub struct RecoveryState {
    pub message: String,
    pub backups: Vec<PathBuf>,
}

pub struct FinancialLoad<T> {
    pub value: T,
    pub recovery: Option<RecoveryState>,
}

static OPERATIONS: Mutex<()> = Mutex::new(());
static LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, RecoveryState>>> = OnceLock::new();
fn locks() -> &'static Mutex<BTreeMap<PathBuf, RecoveryState>> {
    LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub trait FinancialDocument: Default + Serialize + DeserializeOwned {
    const KIND: DocumentKind;
    fn validate(&self) -> Result<()>;
}

impl FinancialDocument for Portfolio {
    const KIND: DocumentKind = DocumentKind::Portfolio;
    fn validate(&self) -> Result<()> {
        let mut ids = HashSet::new();
        for trade in &self.trades {
            ensure!(
                !trade.id.is_empty() && ids.insert(&trade.id),
                "empty or duplicate trade id"
            );
            ensure!(
                trade.shares.is_finite()
                    && trade.shares > 0.0
                    && trade.price.is_finite()
                    && trade.price > 0.0
                    && trade.fee.is_finite()
                    && trade.fee >= 0.0
                    && (trade.shares * trade.price).is_finite(),
                "invalid trade amount"
            );
        }
        for (currency, balance) in &self.cash_balances {
            ensure!(
                *currency == balance.currency,
                "cash currency does not match its account"
            );
        }
        Ok(())
    }
}

impl FinancialDocument for Journal {
    const KIND: DocumentKind = DocumentKind::Journal;
    fn validate(&self) -> Result<()> {
        for entry in &self.entries {
            ensure!(!entry.id.is_empty(), "empty journal id");
            ensure!(
                entry.price.is_none_or(|v| v.is_finite() && v > 0.0)
                    && entry.target.is_none_or(|v| v.is_finite() && v > 0.0),
                "invalid journal price"
            );
        }
        Ok(())
    }
}

fn describe(path: &Path, message: String) -> RecoveryState {
    RecoveryState {
        message,
        backups: json_store::latest_backups(path).unwrap_or_default(),
    }
}

pub fn state(path: &Path) -> Option<RecoveryState> {
    locks()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(path)
        .cloned()
}

pub fn load<T: FinancialDocument>(path: &Path) -> FinancialLoad<T> {
    load_with_migration_hook(path, || Ok(()))
}

fn load_with_migration_hook<T: FinancialDocument>(
    path: &Path,
    before_migration_save: impl FnOnce() -> Result<()>,
) -> FinancialLoad<T> {
    let _operation = OPERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(message) = state(path) {
        return FinancialLoad {
            value: T::default(),
            recovery: Some(message.clone()),
        };
    }
    let loaded = json_store::load::<T>(path, T::KIND);
    let (value, error) = match loaded {
        Ok(loaded) => {
            let result = loaded.value.validate().and_then(|_| {
                if loaded.migration.migrated {
                    json_store::backup_before_migration(path, loaded.migration.from_version)
                        .context("create pre-migration backup")?;
                    before_migration_save()?;
                    json_store::save(path, &loaded.value).context("persist migration")?;
                }
                Ok(())
            });
            match result {
                Ok(()) => (loaded.value, None),
                // Keep a decoded document visible, but immutable, on migration failure.
                Err(error) => (loaded.value, Some(format!("{error:#}"))),
            }
        }
        Err(LoadError::NotFound) if std::fs::symlink_metadata(path).is_err() => {
            (T::default(), None)
        }
        Err(error) => (T::default(), Some(error.to_string())),
    };
    let recovery = error.map(|message| {
        let recovery = describe(path, message);
        locks()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf(), recovery.clone());
        recovery
    });
    FinancialLoad { value, recovery }
}

pub fn save<T: FinancialDocument>(path: &Path, value: &T) -> Result<()> {
    let _operation = OPERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(message) = state(path) {
        bail!("只读恢复模式，原文件未覆盖：{}", message.message);
    }
    value.validate()?;
    // Recheck on every actual write, including changes made externally since
    // startup. Failure locks subsequent saves, even if the file disappears.
    let check = match json_store::load::<T>(path, T::KIND) {
        Ok(loaded) => loaded.value.validate().and_then(|_| {
            if loaded.migration.migrated {
                json_store::backup_before_migration(path, loaded.migration.from_version)?;
            }
            Ok(())
        }),
        Err(LoadError::NotFound) if std::fs::symlink_metadata(path).is_err() => Ok(()),
        Err(error) => Err(anyhow::Error::new(error)),
    };
    if let Err(error) = check {
        let message = format!("{error:#}");
        let recovery = describe(path, message.clone());
        locks()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_path_buf(), recovery);
        bail!("只读恢复模式，原文件未覆盖：{message}");
    }
    json_store::save(path, value)
}

/// Call only after the user explicitly confirms this exact document/backup.
/// Wrong-document, malformed and unsupported backups fail before any write.
pub fn restore<T: FinancialDocument>(path: &Path, backup: &Path) -> Result<T> {
    let _operation = OPERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ensure!(state(path).is_some(), "document is not in recovery mode");
    let (candidate, bytes) = json_store::read_backup::<T>(path, backup, T::KIND)?;
    candidate.value.validate()?;
    json_store::preserve_original(path)?;
    json_store::atomic_write(path, &bytes)?;
    // Restore migrates only in memory. The next ordinary save backs up a legacy
    // document before writing the current schema.
    locks()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(path);
    Ok(candidate.value)
}

/// Reset is never implicit: keep an immutable durable copy first, and refuse
/// reset if the original cannot be read or the backup cannot be synced.
pub fn reset<T: FinancialDocument>(path: &Path) -> Result<T> {
    let _operation = OPERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ensure!(state(path).is_some(), "document is not in recovery mode");
    json_store::preserve_original(path)?;
    let value = T::default();
    value.validate()?;
    json_store::save(path, &value)?;
    locks()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(path);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "zstock-recovery-fixture-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn corrupt_future_and_wrong_document_stay_read_only() {
        let fixture = Fixture::new();
        for (index, bytes) in [
            b"broken".as_slice(),
            br#"{"schema_version":99,"trades":[]}"#,
            br#"{"entries":[]}"#,
            b"{}",
        ]
        .into_iter()
        .enumerate()
        {
            let path = fixture.path(&format!("portfolio-{index}.json"));
            fs::write(&path, bytes).unwrap();
            assert!(load::<Portfolio>(&path).recovery.is_some());
            assert!(save(&path, &Portfolio::default()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            // Disappearance must not silently clear a latched lock.
            fs::remove_file(&path).unwrap();
            assert!(save(&path, &Portfolio::default()).is_err());
        }
    }

    #[test]
    fn journal_io_error_never_becomes_a_writable_empty_journal() {
        let fixture = Fixture::new();
        let path = fixture.path("journal.json");
        fs::create_dir(&path).unwrap();
        assert!(load::<Journal>(&path).recovery.is_some());
        assert!(save(&path, &Journal::default()).is_err());
        assert!(reset::<Journal>(&path).is_err());
        assert!(path.is_dir());
    }

    #[test]
    fn recovery_rejects_wrong_or_invalid_backup_without_changing_original() {
        let fixture = Fixture::new();
        let path = fixture.path("portfolio.json");
        fs::write(&path, b"original broken bytes").unwrap();
        assert!(load::<Portfolio>(&path).recovery.is_some());
        let wrong = fixture.path("portfolio.json.bak.v1.wrong");
        fs::write(&wrong, br#"{"schema_version":1,"entries":[]}"#).unwrap();
        assert!(restore::<Portfolio>(&path, &wrong).is_err());
        let foreign = fixture.path("journal.json.bak.v1.foreign");
        json_store::save(&foreign, &Portfolio::default()).unwrap();
        assert!(restore::<Portfolio>(&path, &foreign).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original broken bytes");
        assert!(state(&path).is_some());
    }

    #[test]
    fn confirmed_reset_and_restore_preserve_original_bytes_and_unlock() {
        let fixture = Fixture::new();
        for restore_backup in [false, true] {
            let path = fixture.path(if restore_backup {
                "portfolio.json"
            } else {
                "reset.json"
            });
            fs::write(&path, b"irreplaceable broken original").unwrap();
            assert!(load::<Portfolio>(&path).recovery.is_some());
            if restore_backup {
                let backup = fixture.path("portfolio.json.bak.v1.fixture");
                json_store::save(&backup, &Portfolio::default()).unwrap();
                restore::<Portfolio>(&path, &backup).unwrap();
            } else {
                reset::<Portfolio>(&path).unwrap();
            }
            assert!(state(&path).is_none());
            assert!(load::<Portfolio>(&path).recovery.is_none());
            save(&path, &Portfolio::default()).unwrap();
        }
        let preserved = fs::read_dir(&fixture.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".recovery."))
            .collect::<Vec<_>>();
        assert_eq!(preserved.len(), 2);
        for entry in preserved {
            assert_eq!(
                fs::read(entry.path()).unwrap(),
                b"irreplaceable broken original"
            );
        }
    }

    #[test]
    fn migration_write_failure_latches_read_only_and_preserves_original() {
        let fixture = Fixture::new();
        let path = fixture.path("portfolio.json");
        let original = br#"{"trades":[],"cash":123.45}"#;
        fs::write(&path, original).unwrap();
        let loaded = load_with_migration_hook::<Portfolio>(&path, || {
            bail!("fixture migration disk failure")
        });
        assert!(loaded.recovery.is_some());
        assert_eq!(
            loaded
                .value
                .cash(crate::domain::money::Currency::Cny)
                .major(),
            123.45
        );
        assert!(save(&path, &Portfolio::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let backups = json_store::latest_backups(&path).unwrap();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(&backups[0]).unwrap(), original);
    }
    #[test]
    fn external_corruption_is_detected_before_save() {
        let fixture = Fixture::new();
        let path = fixture.path("journal.json");
        save(&path, &Journal::default()).unwrap();
        fs::write(&path, b"external corrupt data").unwrap();
        assert!(save(&path, &Journal::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"external corrupt data");
        assert!(state(&path).is_some());
    }
}
