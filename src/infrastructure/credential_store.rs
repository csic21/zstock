use std::process::Command;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::process::Stdio;

use crate::services::secrets::{SecretError, SecretStore};

#[cfg(any(target_os = "macos", target_os = "linux"))]
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
    match parse_linux_lookup_output(&output)? {
        LinuxLookup::Found(value) => Ok(Some(value)),
        LinuxLookup::CheckAbsence => {
            // A cancelled unlock can look exactly like a missing key to lookup.
            // Search without --unlock also includes locked matches. Only a
            // successful, completely empty search proves absence.
            let search = Command::new("secret-tool")
                .args(["search", "--all", "service", SERVICE, "account", account])
                .output()
                .map_err(command_error)?;
            confirm_linux_secret_absent(&search)?;
            Ok(None)
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum LinuxLookup {
    Found(String),
    CheckAbsence,
}

/// GNOME secret-tool returns 0 with raw password bytes on stdout; it appends a
/// newline only for a TTY. Both lookup errors (with stderr) and a NULL result
/// return 1. NULL also covers an unsuccessful unlock, not just missing items.
/// Sources: official libsecret tool/secret-tool.c and on_lookup_unlocked in
/// https://gnome.pages.gitlab.gnome.org/libsecret/coverage/tool/secret-tool.c.gcov.html
/// https://gnome.pages.gitlab.gnome.org/libsecret/coverage/libsecret/secret-methods.c.gcov.html
#[cfg(target_os = "linux")]
fn parse_linux_lookup_output(output: &std::process::Output) -> Result<LinuxLookup, SecretError> {
    if !output.stderr.is_empty() {
        return Err(linux_output_error("read", output));
    }
    if output.status.success() {
        return String::from_utf8(output.stdout.clone())
            .map(LinuxLookup::Found)
            .map_err(|_| SecretError("credential store returned a non-UTF-8 password".into()));
    }
    if output.status.code() == Some(1) && output.stdout.is_empty() {
        return Ok(LinuxLookup::CheckAbsence);
    }
    Err(linux_output_error("read", output))
}

/// secret-tool search exits 0 even when there are no matches. Matching items
/// produce output, including locked ones; backend failures produce stderr.
/// https://gnome.pages.gitlab.gnome.org/libsecret/method.Service.search_sync.html
#[cfg(target_os = "linux")]
fn confirm_linux_secret_absent(output: &std::process::Output) -> Result<(), SecretError> {
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(linux_output_error("search", output));
    }
    if !output.stdout.is_empty() {
        // Search output may include a secret. Never put it in an error message.
        return Err(SecretError(
            "credential exists but could not be read; unlock the credential store and retry".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_output_error(operation: &str, output: &std::process::Output) -> SecretError {
    let detail = String::from_utf8_lossy(&output.stderr);
    SecretError(format!(
        "credential store {operation} failed ({}): {}",
        output.status,
        detail.trim()
    ))
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
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

#[cfg(test)]
mod native_tests {
    use super::*;

    #[test]
    #[ignore = "mutates the current user's native credential store temporarily"]
    fn native_secret_round_trip_smoke() {
        let store = NativeSecretStore;
        let account = format!(
            "codex-smoke-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let secret = "zstock-credential-smoke-value";
        store.delete(&account).expect("clean stale test credential");
        store
            .set(&account, secret)
            .expect("write native credential");
        assert_eq!(
            store.get(&account).expect("read native credential"),
            Some(secret.into())
        );
        store.delete(&account).expect("delete native credential");
        assert_eq!(store.get(&account).expect("confirm deletion"), None);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod linux_output_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

    fn output(code: i32, stdout: &[u8], stderr: &[u8]) -> Output {
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    #[test]
    fn successful_lookup_preserves_raw_password_including_empty_value() {
        for value in [
            b"fixture-password".as_slice(),
            b"  trailing  ",
            b"line\n",
            b"",
        ] {
            assert_eq!(
                parse_linux_lookup_output(&output(0, value, b"")).unwrap(),
                LinuxLookup::Found(String::from_utf8(value.to_vec()).unwrap())
            );
        }
    }

    #[test]
    fn silent_lookup_exit_one_requires_independent_absence_confirmation() {
        assert_eq!(
            parse_linux_lookup_output(&output(1, b"", b"")).unwrap(),
            LinuxLookup::CheckAbsence
        );
        assert!(confirm_linux_secret_absent(&output(0, b"", b"")).is_ok());
    }

    #[test]
    fn lookup_diagnostics_never_mean_absence_even_on_success() {
        for code in [0, 1, 2] {
            for stderr in [
                b"secret-tool: service unavailable".as_slice(),
                b"secret-tool: collection is locked",
                b"secret-tool: prompt dismissed",
                b"\n",
            ] {
                assert!(parse_linux_lookup_output(&output(code, b"", stderr)).is_err());
            }
        }
    }

    #[test]
    fn abnormal_lookup_status_and_partial_output_fail_closed() {
        for code in [2, 3, 126, 127] {
            assert!(parse_linux_lookup_output(&output(code, b"", b"")).is_err());
        }
        assert!(parse_linux_lookup_output(&output(1, b"partial", b"")).is_err());
        let signalled = Output {
            status: ExitStatus::from_raw(9),
            stdout: Vec::new(),
            stderr: Vec::new(),
        };
        assert!(parse_linux_lookup_output(&signalled).is_err());
    }

    #[test]
    fn invalid_utf8_lookup_fails_without_exposing_password_bytes() {
        let error = parse_linux_lookup_output(&output(0, b"fixture-password\xff", b""))
            .unwrap_err()
            .to_string();
        assert!(!error.contains("fixture-password"));
    }

    #[test]
    fn locked_or_other_matching_item_is_not_absence() {
        let error = confirm_linux_secret_absent(&output(
            0,
            b"[/item/fixture]\nsecret = fixture-password\n",
            b"",
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("could not be read"));
        assert!(!error.contains("fixture-password"));
        assert!(confirm_linux_secret_absent(&output(0, b"\n", b"")).is_err());
    }

    #[test]
    fn absence_search_errors_never_allow_migration() {
        for code in [0, 1, 2] {
            assert!(
                confirm_linux_secret_absent(&output(
                    code,
                    b"",
                    b"secret-tool: collection unavailable"
                ))
                .is_err()
            );
        }
        assert!(confirm_linux_secret_absent(&output(1, b"", b"")).is_err());
    }
}
