use crate::domain::fundamentals::FundamentalSnapshot;

use super::market_data::ProviderError;

pub trait FundamentalsProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn fetch_fundamentals(
        &self,
        code: &str,
        report_limit: usize,
    ) -> Result<FundamentalSnapshot, ProviderError>;
}
