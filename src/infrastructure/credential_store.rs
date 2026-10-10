#[cfg(any(target_os = "linux", target_os = "windows"))]
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

// Security.framework preserves the same service/account identity as the legacy
// `security` tool but never places passwords in a process argument or environment.
#[cfg(target_os = "macos")]
fn get_secret(account: &str) -> Result<Option<String>, SecretError> {
    use security_framework::passwords::{PasswordOptions, generic_password};
    match generic_password(PasswordOptions::new_generic_password(SERVICE, account)) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| SecretError("credential store returned a non-UTF-8 password".into())),
        Err(error) if error.code() == -25300 => Ok(None), // errSecItemNotFound
        Err(error) => Err(SecretError(format!(
            "credential store read failed ({})",
            error.code()
        ))),
    }
}

#[cfg(target_os = "macos")]
fn set_secret(account: &str, secret: &str) -> Result<(), SecretError> {
    security_framework::passwords::set_generic_password(SERVICE, account, secret.as_bytes())
        .map_err(|error| SecretError(format!("credential store write failed ({})", error.code())))
}

#[cfg(target_os = "macos")]
fn delete_secret(account: &str) -> Result<(), SecretError> {
    match security_framework::passwords::delete_generic_password(SERVICE, account) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(error) => Err(SecretError(format!(
            "credential store delete failed ({})",
            error.code()
        ))),
    }
}

#[cfg(target_os = "linux")]
fn get_secret(account: &str) -> Result<Option<String>, SecretError> {
    let output = run_credential_command(
        Command::new("secret-tool").args(["lookup", "service", SERVICE, "account", account]),
        None,
    )?;
    match parse_linux_lookup_output(&output)? {
        LinuxLookup::Found(value) => Ok(Some(value)),
        LinuxLookup::CheckAbsence => {
            // A cancelled unlock can look exactly like a missing key to lookup.
            // Search without --unlock also includes locked matches. Only a
            // successful, completely empty search proves absence.
            let search = run_credential_command(
                Command::new("secret-tool")
                    .args(["search", "--all", "service", SERVICE, "account", account]),
                None,
            )?;
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
    // A backend diagnostic may echo input; never include it in user/log errors.
    SecretError(format!(
        "credential store {operation} failed ({})",
        output.status
    ))
}

#[cfg(target_os = "linux")]
fn set_secret(account: &str, secret: &str) -> Result<(), SecretError> {
    let output = run_credential_command(
        Command::new("secret-tool").args([
            "store",
            "--label",
            "ZStock API key",
            "service",
            SERVICE,
            "account",
            account,
        ]),
        Some(secret.as_bytes()),
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(SecretError("credential store rejected the secret".into()))
    }
}

#[cfg(target_os = "linux")]
fn delete_secret(account: &str) -> Result<(), SecretError> {
    let output = run_credential_command(
        Command::new("secret-tool").args(["clear", "service", SERVICE, "account", account]),
        None,
    )?;
    confirm_linux_clear_output(&output)?;
    // clear removes unlocked matches only: exit 0 means at least one deletion,
    // not that no locked duplicate remains. Exit 1 may also mean absence.
    // Both require independent successful empty search, including locked items.
    let search = run_credential_command(
        Command::new("secret-tool")
            .args(["search", "--all", "service", SERVICE, "account", account]),
        None,
    )?;
    confirm_linux_secret_absent(&search)
}

#[cfg(target_os = "linux")]
fn confirm_linux_clear_output(output: &std::process::Output) -> Result<(), SecretError> {
    if !output.stderr.is_empty() || !output.stdout.is_empty() {
        return Err(linux_output_error("delete", output));
    }
    if matches!(output.status.code(), Some(0 | 1)) {
        return Ok(());
    }
    Err(linux_output_error("delete", output))
}

#[cfg(target_os = "windows")]
fn get_secret(account: &str) -> Result<Option<String>, SecretError> {
    let path = windows_secret_path(account)?;
    if !path.exists() {
        return Ok(None);
    }
    let script = "$secure = Get-Content -Raw -LiteralPath $env:ZSTOCK_CREDENTIAL_PATH | ConvertTo-SecureString; $plain = [System.Net.NetworkCredential]::new('', $secure).Password; [Console]::Out.Write($plain)";
    let output = run_credential_command(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("ZSTOCK_CREDENTIAL_PATH", &path),
        None,
    )?;
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
    let path = windows_secret_path(account)?;
    let parent = path
        .parent()
        .ok_or_else(|| SecretError("credential path has no parent".into()))?;
    std::fs::create_dir_all(parent).map_err(|error| SecretError(error.to_string()))?;
    let script = "$plain = [Console]::In.ReadToEnd(); $secure = ConvertTo-SecureString $plain -AsPlainText -Force; $secure | ConvertFrom-SecureString | Set-Content -NoNewline -LiteralPath $env:ZSTOCK_CREDENTIAL_PATH";
    let output = run_credential_command(
        Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("ZSTOCK_CREDENTIAL_PATH", &path),
        Some(secret.as_bytes()),
    )?;
    if output.status.success() {
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

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn command_error(error: std::io::Error) -> SecretError {
    SecretError(format!("credential store unavailable: {error}"))
}

#[cfg(target_os = "windows")]
fn status_error(operation: &str, _stderr: &[u8]) -> SecretError {
    SecretError(format!("credential store {operation} failed"))
}

/// Bound external credential tools, including a dismissed/locked keyring prompt.
/// Concurrent pipe drains prevent large stderr/stdout from deadlocking the child.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_credential_command(
    command: &mut Command,
    input: Option<&[u8]>,
) -> Result<std::process::Output, SecretError> {
    run_credential_command_with_timeout(command, input, std::time::Duration::from_secs(20))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_credential_command_with_timeout(
    command: &mut Command,
    input: Option<&[u8]>,
    timeout: std::time::Duration,
) -> Result<std::process::Output, SecretError> {
    use std::io::{Read, Write};
    const MAX_BYTES: usize = 64 * 1024;
    if input.is_some_and(|bytes| bytes.len() > MAX_BYTES) {
        return Err(SecretError("credential exceeds size limit".into()));
    }
    let started = std::time::Instant::now();
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(command_error)?;
    let (send, receive) = std::sync::mpsc::channel();
    for (is_stdout, pipe) in [
        (
            true,
            child
                .stdout
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        ),
        (
            false,
            child
                .stderr
                .take()
                .map(|pipe| Box::new(pipe) as Box<dyn Read + Send>),
        ),
    ] {
        let send = send.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let mut bytes = Vec::new();
                pipe.ok_or_else(|| std::io::Error::other("pipe unavailable"))?
                    .take(MAX_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > MAX_BYTES {
                    return Err(std::io::Error::other("credential output too large"));
                }
                Ok(bytes)
            })();
            let _ = send.send((is_stdout, result));
        });
    }
    let (input_send, input_receive) = std::sync::mpsc::channel();
    if let Some(input) = input {
        let input = input.to_vec();
        let pipe = child.stdin.take();
        std::thread::spawn(move || {
            let result = pipe
                .ok_or_else(|| std::io::Error::other("stdin unavailable"))
                .and_then(|mut pipe| pipe.write_all(&input));
            let _ = input_send.send(result);
        });
    } else {
        let _ = input_send.send(Ok(()));
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SecretError(
                    if result.is_err() {
                        "credential tool failed"
                    } else {
                        "credential tool timed out; unlock the store and retry"
                    }
                    .into(),
                ));
            }
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    for _ in 0..2 {
        let remaining = timeout.saturating_sub(started.elapsed());
        let (is_stdout, bytes) = receive
            .recv_timeout(remaining)
            .map_err(|_| SecretError("credential output timed out".into()))?;
        let bytes =
            bytes.map_err(|_| SecretError("credential output failed or exceeded limit".into()))?;
        if is_stdout {
            stdout = bytes;
        } else {
            stderr = bytes;
        }
    }
    input_receive
        .recv_timeout(timeout.saturating_sub(started.elapsed()))
        .map_err(|_| SecretError("credential input timed out".into()))?
        .map_err(|_| SecretError("credential input failed".into()))?;
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
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
    fn deleting_absent_secret_requires_independent_absence_confirmation() {
        for code in [0, 1] {
            let cleared = output(code, b"", b"");
            assert!(confirm_linux_clear_output(&cleared).is_ok());
            assert!(
                confirm_linux_clear_output(&cleared)
                    .and_then(|_| confirm_linux_secret_absent(&output(
                        0,
                        b"locked matching item",
                        b""
                    )))
                    .is_err()
            );
        }
        for result in [
            output(1, b"", b"backend failure"),
            output(0, b"", b"backend failure"),
            output(2, b"", b""),
            output(1, b"unexpected output", b""),
        ] {
            assert!(confirm_linux_clear_output(&result).is_err());
        }
        assert!(confirm_linux_secret_absent(&output(0, b"locked matching item", b"")).is_err());
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
    #[test]
    fn credential_subprocess_timeout_kills_and_reaps_without_secret_output() {
        let started = std::time::Instant::now();
        let error = run_credential_command_with_timeout(
            Command::new("sh").args(["-c", "exec sleep 5"]),
            None,
            std::time::Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn credential_pipe_preserves_dummy_bytes_without_arguments() {
        let output = run_credential_command(
            Command::new("cat").args([] as [&str; 0]),
            Some(b"dummy fixture only\n"),
        )
        .unwrap();
        assert_eq!(output.stdout, b"dummy fixture only\n");
        assert!(output.status.success());
    }
}
