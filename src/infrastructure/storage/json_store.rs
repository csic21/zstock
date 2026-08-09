use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::migrations::{DocumentKind, MigrationResult, migrate};

#[derive(Debug)]
pub enum LoadError {
    NotFound,
    Io(io::Error),
    Invalid(anyhow::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("file not found"),
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Invalid(error) => write!(formatter, "invalid data: {error:#}"),
        }
    }
}

impl std::error::Error for LoadError {}

#[derive(Debug)]
pub struct Loaded<T> {
    pub value: T,
    pub migration: MigrationResult,
}

pub fn load<T: DeserializeOwned>(path: &Path, kind: DocumentKind) -> Result<Loaded<T>, LoadError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(LoadError::NotFound),
        Err(error) => return Err(LoadError::Io(error)),
    };
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|error| LoadError::Invalid(anyhow::Error::new(error).context("parse JSON")))?;
    let migration = migrate(kind, raw).map_err(LoadError::Invalid)?;
    let value = serde_json::from_value(migration.value.clone()).map_err(|error| {
        LoadError::Invalid(anyhow::Error::new(error).context("decode document"))
    })?;
    Ok(Loaded { value, migration })
}

pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).context("serialize JSON")?;
    atomic_write(path, &bytes)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with_hook(path, bytes, || Ok(()))
}

fn atomic_write_with_hook<F>(path: &Path, bytes: &[u8], before_rename: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let temp = temporary_path(path);
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)
            .with_context(|| format!("create {}", temp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("write {}", temp.display()))?;
        file.flush()
            .with_context(|| format!("flush {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", temp.display()))?;
        before_rename()?;
        fs::rename(&temp, path)
            .with_context(|| format!("rename {} to {}", temp.display(), path.display()))?;
        sync_directory(parent)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    write_result
}

pub fn backup_before_migration(path: &Path, from_version: u32) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("data.json");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let backup = parent.join(format!("{file_name}.bak.v{from_version}.{stamp}"));
    fs::copy(path, &backup).with_context(|| format!("backup {}", path.display()))?;
    let file = File::open(&backup).with_context(|| format!("open {}", backup.display()))?;
    file.sync_all()
        .with_context(|| format!("sync {}", backup.display()))?;
    retain_latest_backups(path, 3)?;
    Ok(Some(backup))
}

pub fn latest_backups(path: &Path) -> Result<Vec<PathBuf>> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let prefix = format!(
        "{}.bak.",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("data.json")
    );
    let mut backups: Vec<_> = fs::read_dir(parent)
        .with_context(|| format!("read {}", parent.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|candidate| {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();
    backups.sort_by(|left, right| right.file_name().cmp(&left.file_name()));
    Ok(backups)
}

pub fn restore_backup(path: &Path, backup: &Path) -> Result<()> {
    let bytes = fs::read(backup).with_context(|| format!("read {}", backup.display()))?;
    let _: Value = serde_json::from_slice(&bytes).context("backup is not valid JSON")?;
    atomic_write(path, &bytes)
}

fn retain_latest_backups(path: &Path, keep: usize) -> Result<()> {
    for old in latest_backups(path)?.into_iter().skip(keep) {
        fs::remove_file(&old).with_context(|| format!("remove {}", old.display()))?;
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("data.json");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    parent.join(format!(".{name}.tmp-{}-{nonce}", std::process::id()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("sync directory {}", path.display()))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "zstock-json-store-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn interrupted_write_keeps_original_readable() {
        let path = temp_file("interrupt.json");
        atomic_write(&path, br#"{"old":true}"#).unwrap();
        let result = atomic_write_with_hook(&path, br#"{"new":true}"#, || anyhow::bail!("stop"));
        assert!(result.is_err());
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value, serde_json::json!({"old": true}));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn keeps_three_latest_backups_and_can_restore() {
        let path = temp_file("backup.json");
        atomic_write(&path, br#"{"schema_version":0,"n":0}"#).unwrap();
        for index in 0..5 {
            backup_before_migration(&path, index).unwrap();
            atomic_write(
                &path,
                format!(r#"{{"schema_version":0,"n":{index}}}"#).as_bytes(),
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let backups = latest_backups(&path).unwrap();
        assert_eq!(backups.len(), 3);
        restore_backup(&path, &backups[0]).unwrap();
        let _: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for backup in backups {
            fs::remove_file(backup).unwrap();
        }
        fs::remove_file(path).unwrap();
    }
}
