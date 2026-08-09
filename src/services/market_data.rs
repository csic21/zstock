use std::fmt;

use crate::domain::market::{KlineSeries, QuoteRecord, SearchHit};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderErrorKind {
    Timeout,
    Transport,
    InvalidPayload,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub provider: String,
    pub kind: ProviderErrorKind,
    pub message: String,
}

impl ProviderError {
    pub fn new(
        provider: impl Into<String>,
        kind: ProviderErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.provider, self.message)
    }
}

impl std::error::Error for ProviderError {}

pub trait QuoteProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn fetch_quotes(&self, codes: &[String]) -> Result<Vec<QuoteRecord>, ProviderError>;
}

pub trait KlineProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn fetch_klines(&self, code: &str, limit: usize) -> Result<KlineSeries, ProviderError>;
}

pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, ProviderError>;
}
