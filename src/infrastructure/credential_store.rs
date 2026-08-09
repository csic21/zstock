use std::process::Command;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::process::Stdio;

use crate::services::secrets::{SecretError, SecretStore};

const SERVICE: &str = "com.karl.zstock";

#[derive(Debug, Clone, Default)]
pub struct NativeSecretStore;

impl SecretStore for NativeSecretStore {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        get_secret(account)
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        set_secret(account, secret)
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        delete_secret(account)
    }
}

#[cfg(target_os = "macos")]
fn get_secret(account: &str) -> Result<Option<String>, SecretError> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"])
        .output()
        .map_err(command_error)?;
    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map(|value| Some(value.trim_end().to_string()))
            .map_err(|error| SecretError(error.to_string()));
    }
    if output.status.code() == Some(44) {
        return Ok(None);
    }
    Err(status_error("read", &output.stderr))
}

#[cfg(target_os = "macos")]
fn set_secret(account: &str, secret: &str) -> Result<(), SecretError> {
    let output = Command::new("security")
        .args([
            "add-generic-password",
            "-U",
            "-s",
            SERVICE,
            "-a",
            account,
            "-w",
            secret,
        ])
        .output()
        .map_err(command_error)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(status_error("write", &output.stderr))
    }
}

#[cfg(target_os = "macos")]
fn delete_secret(account: &str) -> Result<(), SecretError> {
    let output = Command::new("security")
        .args(["delete-generic-password", "-s", SERVICE, "-a", account])
        .output()
        .map_err(command_error)?;
    if output.status.success() || output.status.code() == Some(44) {
        Ok(())
    } else {
        Err(status_error("delete", &output.stderr))
    }
}

#[cfg(target_os = "linux")]
fn get_secret(account: &str) -> Result<Option<String>, SecretError> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", account])
        .output()
        .map_err(command_error)?;
    if output.status.success() {
        let value =
            String::from_utf8(output.stdout).map_err(|error| SecretError(error.to_string()))?;
        return Ok((!value.trim().is_empty()).then(|| value.trim_end().to_string()));
    }
    Ok(None)
}

#[cfg(target_os = "linux")]
fn set_secret(account: &str, secret: &str) -> Result<(), SecretError> {
    use std::io::Write;

    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label",
            "ZStock API key",
            "service",
            SERVICE,
            "account",
            account,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(command_error)?;
    child
        .stdin
        .take()
        .ok_or_else(|| SecretError("credential store stdin unavailable".into()))?
        .write_all(secret.as_bytes())
        .map_err(|error| SecretError(error.to_string()))?;
    let status = child.wait().map_err(command_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(SecretError("credential store rejected the secret".into()))
    }
}

#[cfg(target_os = "linux")]
fn delete_secret(account: &str) -> Result<(), SecretError> {
    let status = Command::new("secret-tool")
        .args(["clear", "service", SERVICE, "account", account])
        .status()
        .map_err(command_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(SecretError("credential store delete failed".into()))
    }
}

#[cfg(target_os = "windows")]
fn get_secret(account: &str) -> Result<Option<String>, SecretError> {
    let path = windows_secret_path(account)?;
    if !path.exists() {
        return Ok(None);
    }
    let script = "$secure = Get-Content -Raw -LiteralPath $env:ZSTOCK_CREDENTIAL_PATH | ConvertTo-SecureString; $plain = [System.Net.NetworkCredential]::new('', $secure).Password; [Console]::Out.Write($plain)";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("ZSTOCK_CREDENTIAL_PATH", &path)
        .output()
        .map_err(command_error)?;
    if output.status.success() {
        String::from_utf8(output.stdout)
            .map(Some)
            .map_err(|error| SecretError(error.to_string()))
    } else {
        Err(status_error("read", &output.stderr))
    }
}

#[cfg(target_os = "windows")]
fn set_secret(account: &str, secret: &str) -> Result<(), SecretError> {
    use std::io::Write;

    let path = windows_secret_path(account)?;
    let parent = path
        .parent()
        .ok_or_else(|| SecretError("credential path has no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|error| SecretError(error.to_string()))?;
    let script = "$plain = [Console]::In.ReadToEnd(); $secure = ConvertTo-SecureString $plain -AsPlainText -Force; $secure | ConvertFrom-SecureString | Set-Content -NoNewline -LiteralPath $env:ZSTOCK_CREDENTIAL_PATH";
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("ZSTOCK_CREDENTIAL_PATH", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(command_error)?;
    child
        .stdin
        .take()
        .ok_or_else(|| SecretError("credential store stdin unavailable".into()))?
        .write_all(secret.as_bytes())
        .map_err(|error| SecretError(error.to_string()))?;
    let status = child.wait().map_err(command_error)?;
    if status.success() {
        Ok(())
    } else {
        Err(SecretError("Windows DPAPI rejected the secret".into()))
    }
}

#[cfg(target_os = "windows")]
fn delete_secret(account: &str) -> Result<(), SecretError> {
    let path = windows_secret_path(account)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SecretError(error.to_string())),
    }
}

#[cfg(target_os = "windows")]
fn windows_secret_path(account: &str) -> Result<std::path::PathBuf, SecretError> {
    use sha2::{Digest, Sha256};

    let base = dirs::data_local_dir()
        .ok_or_else(|| SecretError("Windows local app-data directory unavailable".into()))?;
    let digest = Sha256::digest(account.as_bytes());
    let name = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(base
        .join("ZStock")
        .join("credentials")
        .join(format!("{name}.dpapi")))
}

fn command_error(error: std::io::Error) -> SecretError {
    SecretError(format!("credential store unavailable: {error}"))
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn status_error(operation: &str, stderr: &[u8]) -> SecretError {
    let detail = String::from_utf8_lossy(stderr);
    SecretError(format!(
        "credential store {operation} failed: {}",
        detail.trim()
    ))
}

#[cfg(test)]
#[derive(Default)]
pub struct MemorySecretStore(std::sync::Mutex<std::collections::HashMap<String, String>>);

#[cfg(test)]
impl SecretStore for MemorySecretStore {
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        Ok(self.0.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().insert(account.into(), secret.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().remove(account);
        Ok(())
    }
}
