use anyhow::{Context, Result, bail};

use crate::services::llm::{LlmClient, LlmRequest, LlmResponse};

use super::HttpLlmConfig;
use super::http::{extract_api_error, post_json};

pub struct OpenAiResponsesClient {
    config: HttpLlmConfig,
}

impl OpenAiResponsesClient {
    pub const fn new(config: HttpLlmConfig) -> Self {
        Self { config }
    }
}

impl LlmClient for OpenAiResponsesClient {
    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse> {
        let base = self.config.base_url.trim().trim_end_matches('/');
        let path = format!("{base}/responses");
        let payload = serde_json::json!({
            "model": self.config.model.trim(),
            "instructions": request.system,
            "input": request.user,
            "max_output_tokens": request.max_output_tokens,
        })
        .to_string();
        let raw = post_json(&self.config, &path, &payload)?;
        Ok(LlmResponse {
            text: parse_responses(&raw)?,
            model: self.config.model.trim().into(),
            transport: "openai_responses".into(),
        })
    }
}

pub(crate) fn parse_responses(text: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(text).context("解析 Responses 响应失败（返回的不是 JSON）")?;
    if let Some(message) = extract_api_error(&value) {
        bail!("接口返回错误：{message}");
    }
    let mut parts = Vec::new();
    if let Some(output) = value.get("output").and_then(|output| output.as_array()) {
        for item in output {
            if item.get("type").and_then(|kind| kind.as_str()) != Some("message") {
                continue;
            }
            if let Some(content) = item.get("content").and_then(|content| content.as_array()) {
                for part in content {
                    if part.get("type").and_then(|kind| kind.as_str()) == Some("output_text")
                        && let Some(text) = part.get("text").and_then(|text| text.as_str())
                    {
                        parts.push(text);
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        bail!("Responses 响应中没有找到文本内容");
    }
    Ok(parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_all_output_text_parts_and_surfaces_errors() {
        let response = r#"{
            "output":[{"type":"message","content":[
                {"type":"output_text","text":"one"},
                {"type":"output_text","text":"two"}
            ]}]
        }"#;
        assert_eq!(parse_responses(response).unwrap(), "one\ntwo");
        assert!(
            parse_responses(r#"{"error":{"message":"bad key"}}"#)
                .unwrap_err()
                .to_string()
                .contains("bad key")
        );
    }
}
