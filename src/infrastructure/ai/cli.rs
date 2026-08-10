use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::services::llm::{LlmClient, LlmRequest, LlmResponse};

use super::http::truncate;

static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliProvider {
    Grok,
    Chatgpt,
    Opencode,
    Claude,
}

impl CliProvider {
    const fn label(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Chatgpt => "ChatGPT",
            Self::Opencode => "OpenCode",
            Self::Claude => "Claude",
        }
    }

    const fn default_bins(self) -> &'static [&'static str] {
        match self {
            Self::Grok => &["grok"],
            Self::Chatgpt => &["chatgpt", "codex"],
            Self::Opencode => &["opencode"],
            Self::Claude => &["claude"],
        }
    }
}

#[derive(Debug, Clone)]
pub struct CliLlmConfig {
    pub provider: CliProvider,
    pub binary: String,
    pub model: String,
    pub timeout_secs: u64,
    pub max_response_bytes: usize,
}

pub struct CliLlmClient {
    config: CliLlmConfig,
}

impl CliLlmClient {
    pub const fn new(config: CliLlmConfig) -> Self {
        Self { config }
    }
}

impl LlmClient for CliLlmClient {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse> {
        let binary = resolve_cli_bin(&self.config)?;
        let timeout = Duration::from_secs(self.config.timeout_secs.clamp(60, 600));
        let model = (!self.config.model.trim().is_empty()).then_some(self.config.model.trim());
        let text = match self.config.provider {
            CliProvider::Grok => run_grok(
                &binary,
                &request.system,
                &request.user,
                model,
                timeout,
                self.config.max_response_bytes,
            ),
            CliProvider::Chatgpt => {
                let name = binary
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if name == "codex" || name.starts_with("codex") {
                    run_codex(
                        &binary,
                        &request.system,
                        &request.user,
                        model,
                        timeout,
                        self.config.max_response_bytes,
                    )
                } else {
                    run_chatgpt_generic(
                        &binary,
                        &request.system,
                        &request.user,
                        model,
                        timeout,
                        self.config.max_response_bytes,
                    )
                }
            }
            CliProvider::Opencode => run_opencode(
                &binary,
                &request.system,
                &request.user,
                model,
                timeout,
                self.config.max_response_bytes,
            ),
            CliProvider::Claude => run_claude(
                &binary,
                &request.system,
                &request.user,
                model,
                timeout,
                self.config.max_response_bytes,
            ),
        }?;
        Ok(LlmResponse {
            text,
            model: self.config.model.trim().into(),
            transport: format!("cli_{}", self.config.provider.label().to_ascii_lowercase()),
        })
    }
}

fn resolve_cli_bin(config: &CliLlmConfig) -> Result<PathBuf> {
    let custom = config.binary.trim();
    if !custom.is_empty() {
        let path = PathBuf::from(custom);
        if path.is_file() {
            return Ok(path);
        }
        if let Some(found) = which_bin(custom) {
            return Ok(found);
        }
        bail!("找不到 CLI「{custom}」（请确认已安装，或在设置中填写绝对路径）");
    }
    for name in config.provider.default_bins() {
        if let Some(found) = which_bin(name) {
            return Ok(found);
        }
    }
    bail!(
        "未找到 {} CLI（已搜索 {}；可安装后重试，或在设置中填写 CLI 路径）",
        config.provider.label(),
        config.provider.default_bins().join(" / ")
    )
}

fn which_bin(name: &str) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    let mut directories = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ];
    if let Some(home) = dirs::home_dir() {
        directories.extend([
            home.join(".grok/bin"),
            home.join(".local/bin"),
            home.join(".npm-global/bin"),
            home.join("bin"),
            home.join(".cargo/bin"),
        ]);
    }
    directories
        .into_iter()
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn combined_prompt(system: &str, user: &str) -> String {
    format!("{system}\n\n---\n\n{user}")
}

fn temp_path(prefix: &str) -> PathBuf {
    let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    std::env::temp_dir().join(format!(
        "zstock-{prefix}-{}-{timestamp}-{nonce}.txt",
        std::process::id()
    ))
}

fn write_temp_prompt(content: &str) -> Result<PathBuf> {
    let path = temp_path("ai");
    std::fs::write(&path, content)
        .with_context(|| format!("写入临时提示失败：{}", path.display()))?;
    Ok(path)
}

fn run_grok(
    binary: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<String> {
    let prompt_file = write_temp_prompt(user)?;
    let mut command = Command::new(binary);
    command
        .arg("--prompt-file")
        .arg(&prompt_file)
        .arg("--output-format")
        .arg("plain")
        .arg("--system-prompt-override")
        .arg(system)
        .arg("--max-turns")
        .arg("1")
        .arg("--no-subagents")
        .arg("--disable-web-search")
        .arg("--permission-mode")
        .arg("dontAsk");
    if let Some(model) = model {
        command.arg("-m").arg(model);
    }
    let result = run_command(command, timeout, max_bytes);
    std::fs::remove_file(prompt_file).ok();
    result
}

fn run_claude(
    binary: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<String> {
    let mut command = Command::new(binary);
    command
        .arg("-p")
        .arg("--output-format")
        .arg("text")
        .arg("--system-prompt")
        .arg(system)
        .arg("--tools")
        .arg("")
        .arg("--bare")
        .arg("--permission-mode")
        .arg("dontAsk");
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    command.arg("--").arg(user);
    run_command(command, timeout, max_bytes)
}

fn run_opencode(
    binary: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<String> {
    let mut command = Command::new(binary);
    command.arg("run").arg("--format").arg("default");
    if let Some(model) = model {
        command.arg("-m").arg(model);
    }
    command.arg("--").arg(combined_prompt(system, user));
    run_command(command, timeout, max_bytes)
}

fn run_codex(
    binary: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<String> {
    let output_path = temp_path("codex");
    let mut command = Command::new(binary);
    command
        .arg("exec")
        .arg("--ephemeral")
        .arg("--skip-git-repo-check")
        .arg("-s")
        .arg("read-only")
        .arg("-o")
        .arg(&output_path);
    if let Some(model) = model {
        command.arg("-m").arg(model);
    }
    command.arg(combined_prompt(system, user));
    let run = run_command(command, timeout, max_bytes);
    let file_output = read_limited_file(&output_path, max_bytes).ok();
    std::fs::remove_file(output_path).ok();
    match (run, file_output.filter(|text| !text.trim().is_empty())) {
        (_, Some(text)) => Ok(text),
        (Ok(stdout), None) => Ok(stdout),
        (Err(error), None) => Err(error),
    }
}

fn run_chatgpt_generic(
    binary: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<String> {
    let mut command = Command::new(binary);
    if let Some(model) = model {
        command.arg("-m").arg(model);
    }
    command.arg(combined_prompt(system, user));
    run_command(command, timeout, max_bytes)
}

fn run_command(mut command: Command, timeout: Duration, max_bytes: usize) -> Result<String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("CI", "1");
    let mut child = command.spawn().map_err(|error| {
        anyhow!("启动 CLI 失败：{error}（请确认二进制在 PATH 中或已填写绝对路径）")
    })?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let read_output = |reader: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || read_limited(reader, max_bytes))
    };
    let out_handle = read_output(stdout);
    let err_handle = std::thread::spawn(move || read_limited(stderr, max_bytes));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                child.kill().ok();
                child.wait().ok();
                out_handle.join().ok();
                err_handle.join().ok();
                bail!("CLI 超时（{}s）", timeout.as_secs());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(error) => bail!("等待 CLI 失败：{error}"),
        }
    };
    let stdout = out_handle.join().unwrap_or_else(|_| Ok(String::new()))?;
    let stderr = err_handle.join().unwrap_or_else(|_| Ok(String::new()))?;
    if !status.success() {
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        let code = status.code().unwrap_or(-1);
        if detail.is_empty() {
            bail!("CLI 退出码 {code}");
        }
        bail!("CLI 退出码 {code}：{}", truncate(detail, 240));
    }
    let text = if stdout.trim().is_empty() {
        stderr.trim()
    } else {
        stdout.trim()
    };
    if text.is_empty() {
        bail!("CLI 返回了空内容");
    }
    Ok(text.into())
}

fn read_limited<R: Read>(reader: Option<R>, max_bytes: usize) -> Result<String> {
    let Some(reader) = reader else {
        return Ok(String::new());
    };
    let mut output = String::new();
    reader
        .take(
            u64::try_from(max_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_string(&mut output)?;
    if output.len() > max_bytes {
        bail!("CLI 响应超过 {max_bytes} 字节上限");
    }
    Ok(output)
}

fn read_limited_file(path: &Path, max_bytes: usize) -> Result<String> {
    read_limited(Some(std::fs::File::open(path)?), max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_combination_and_temp_names_are_deterministic_in_shape_and_unique() {
        assert!(combined_prompt("SYS", "USER").contains("---"));
        assert_ne!(temp_path("test"), temp_path("test"));
    }
}
