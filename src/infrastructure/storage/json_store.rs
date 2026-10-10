#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    decode(&bytes, kind)
}

/// Validate the document identity before permissive serde defaults can make a
/// different document look like an empty portfolio or journal.
pub fn decode<T: DeserializeOwned>(
    bytes: &[u8],
    kind: DocumentKind,
) -> Result<Loaded<T>, LoadError> {
    let raw: Value = serde_json::from_slice(bytes)
        .map_err(|error| LoadError::Invalid(anyhow::Error::new(error).context("parse JSON")))?;
    let required = match kind {
        DocumentKind::Portfolio => Some(("trades", "entries")),
        DocumentKind::Journal => Some(("entries", "trades")),
        DocumentKind::Config => None,
    };
    if let Some((expected, other)) = required
        && (!raw.get(expected).is_some_and(Value::is_array) || raw.get(other).is_some())
    {
        return Err(LoadError::Invalid(anyhow::anyhow!(
            "wrong document shape: expected {expected} array"
        )));
    }
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
    backup_before_migration_with_bytes(path, from_version, None)
}

/// Back up a redacted document instead of copying sensitive legacy bytes.
pub fn backup_before_migration_with_value<T: Serialize>(
    path: &Path,
    from_version: u32,
    value: &T,
) -> Result<Option<PathBuf>> {
    let bytes = serde_json::to_vec_pretty(value).context("serialize migration backup")?;
    backup_before_migration_with_bytes(path, from_version, Some(&bytes))
}

fn backup_before_migration_with_bytes(
    path: &Path,
    from_version: u32,
    redacted: Option<&[u8]>,
) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = match redacted {
        Some(bytes) => bytes.to_vec(),
        None => fs::read(path).with_context(|| format!("read {} before backup", path.display()))?,
    };
    let backup = preserve_bytes(path, &format!("bak.v{from_version}"), &bytes)?;
    retain_latest_backups(path, 3)?;
    Ok(Some(backup))
}

/// Immutable, uniquely named backup. Never pruned by migration retention. The
/// directory entry is durable before any recovery is allowed to replace data.
pub fn preserve_original(path: &Path) -> Result<Option<PathBuf>> {
    match fs::read(path) {
        Ok(bytes) => preserve_bytes(path, "recovery", &bytes).map(Some),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound && fs::symlink_metadata(path).is_err() =>
        {
            Ok(None)
        }
        Err(error) => Err(error).with_context(|| format!("preserve original {}", path.display())),
    }
}

fn preserve_bytes(path: &Path, category: &str, bytes: &[u8]) -> Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .context("backup target has no file name")?
        .to_string_lossy();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    let backup = parent.join(format!(
        "{name}.{category}.{stamp}-{}-{sequence}",
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&backup)
        .with_context(|| format!("create {}", backup.display()))?;
    file.write_all(bytes).context("write immutable backup")?;
    file.sync_all().context("sync immutable backup")?;
    sync_directory(parent)?;
    Ok(backup)
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
            fs::symlink_metadata(candidate).is_ok_and(|metadata| metadata.is_file())
                && candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();
    backups.sort_by_key(|path| {
        std::cmp::Reverse(
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok(),
        )
    });
    Ok(backups)
}

/// A backup must belong to this exact target and fully decode as its schema.
/// Parsing JSON alone is insufficient because financial documents use defaults.
pub fn read_backup<T: DeserializeOwned>(
    path: &Path,
    backup: &Path,
    kind: DocumentKind,
) -> Result<(Loaded<T>, Vec<u8>)> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let backup_parent = backup.parent().unwrap_or_else(|| Path::new("."));
    let prefix = format!(
        "{}.bak.",
        path.file_name()
            .context("missing target name")?
            .to_string_lossy()
    );
    anyhow::ensure!(
        fs::canonicalize(parent)? == fs::canonicalize(backup_parent)?
            && backup
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
            && fs::symlink_metadata(backup)?.is_file(),
        "backup does not belong to the selected document"
    );
    let bytes = fs::read(backup).with_context(|| format!("read {}", backup.display()))?;
    let loaded = decode::<T>(&bytes, kind)
        .map_err(anyhow::Error::new)
        .context("validate backup schema")?;
    Ok((loaded, bytes))
}

#[cfg(test)]
pub fn restore_backup<T: DeserializeOwned>(
    path: &Path,
    backup: &Path,
    kind: DocumentKind,
) -> Result<()> {
    let (_, bytes) = read_backup::<T>(path, backup, kind)?;
    preserve_original(path)?;
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
        restore_backup::<Value>(&path, &backups[0], DocumentKind::Config).unwrap();
        let _: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for backup in backups {
            fs::remove_file(backup).unwrap();
        }
        fs::remove_file(path).unwrap();
    }
}
