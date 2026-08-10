use std::io::Read;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use super::HttpLlmConfig;

pub(super) fn post_json(config: &HttpLlmConfig, path: &str, payload: &str) -> Result<String> {
    config.validate()?;
    let timeout = Duration::from_secs(config.timeout_secs.clamp(5, 120));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build();
    let response = agent
        .post(path)
        .set("Content-Type", "application/json")
        .set(
            "Authorization",
            &format!("Bearer {}", config.api_key.trim()),
        )
        .send_string(payload)
        .map_err(friendly_http_error)?;
    let mut reader = response.into_reader().take(
        u64::try_from(config.max_response_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    );
    let mut text = String::new();
    reader
        .read_to_string(&mut text)
        .map_err(|error| anyhow!("读取 LLM 响应失败：{error}"))?;
    if text.len() > config.max_response_bytes {
        bail!("LLM 响应超过 {} 字节上限", config.max_response_bytes);
    }
    Ok(text)
}

fn friendly_http_error(error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            let snippet = body.trim();
            if snippet.is_empty() {
                anyhow!("HTTP {code}")
            } else {
                anyhow!("HTTP {code}：{}", truncate(snippet, 180))
            }
        }
        ureq::Error::Transport(transport) => anyhow!("网络错误：{transport}"),
    }
}

pub(super) fn extract_api_error(value: &serde_json::Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
        .map(|message| truncate(message, 200))
}

pub(super) fn truncate(value: &str, max: usize) -> String {
    let mut output: String = value.chars().take(max).collect();
    if value.chars().count() > max {
        output.push('…');
    }
    output
}
