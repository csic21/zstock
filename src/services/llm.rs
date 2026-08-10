use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRequest {
    pub system: String,
    pub user: String,
    pub max_output_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResponse {
    pub text: String,
    pub model: String,
    pub transport: String,
}

pub trait LlmClient: Send + Sync {
    fn complete(&self, request: &LlmRequest) -> anyhow::Result<LlmResponse>;
}
