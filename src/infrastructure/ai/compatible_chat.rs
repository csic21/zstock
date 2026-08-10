use anyhow::{Context, Result, bail};

use crate::services::llm::{LlmClient, LlmRequest, LlmResponse};

use super::HttpLlmConfig;
use super::http::{extract_api_error, post_json};

pub struct CompatibleChatClient {
    config: HttpLlmConfig,
}

impl CompatibleChatClient {
    pub const fn new(config: HttpLlmConfig) -> Self {
        Self { config }
    }
}

impl LlmClient for CompatibleChatClient {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse> {
        let base = self.config.base_url.trim().trim_end_matches('/');
        let path = format!("{base}/chat/completions");
        let payload = serde_json::json!({
            "model": self.config.model.trim(),
            "messages": [
                { "role": "system", "content": request.system },
                { "role": "user", "content": request.user },
            ],
            "temperature": 0.3,
            "max_tokens": request.max_output_tokens,
        })
        .to_string();
        let raw = post_json(&self.config, &path, &payload)?;
        Ok(LlmResponse {
            text: parse_chat(&raw)?,
            model: self.config.model.trim().into(),
            transport: "compatible_chat".into(),
        })
    }
}

pub(crate) fn parse_chat(text: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("解析 Chat 响应失败（返回的不是 JSON）")?;
    if let Some(message) = extract_api_error(&value) {
        bail!("接口返回错误：{message}");
    }
    match value
        .pointer("/choices/0/message/content")
        .and_then(|content| content.as_str())
    {
        Some(content) if !content.trim().is_empty() => Ok(content.into()),
        _ => bail!("Chat 响应中没有文本内容（可能是配额或限流错误）"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_chat_content() {
        assert_eq!(
            parse_chat(r#"{"choices":[{"message":{"content":"hello"}}]}"#).unwrap(),
            "hello"
        );
    }
}
