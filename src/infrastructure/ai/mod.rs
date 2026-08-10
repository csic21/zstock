mod http;

pub mod cli;
pub mod compatible_chat;
pub mod openai;

#[derive(Clone)]
pub struct HttpLlmConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    pub timeout_secs: u64,
    pub max_response_bytes: usize,
}

impl HttpLlmConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.api_key.trim().is_empty(),
            "未配置 API Key（设置 → AI 分析）"
        );
        anyhow::ensure!(
            !self.base_url.trim().is_empty(),
            "未配置 API 地址（设置 → AI 分析）"
        );
        anyhow::ensure!(
            !self.model.trim().is_empty(),
            "未配置模型名称（设置 → AI 分析）"
        );
        anyhow::ensure!(
            self.max_response_bytes > 0,
            "LLM response size limit must be positive"
        );
        Ok(())
    }
}
