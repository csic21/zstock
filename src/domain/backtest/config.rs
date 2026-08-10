use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::CostModel;
use crate::domain::dataset::DateInterval;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkDefinition {
    EqualWeightedUniverse,
    FrozenInstrument { instrument_key: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioBacktestConfig {
    pub initial_cash: f64,
    pub costs: CostModel,
    pub benchmark: BenchmarkDefinition,
    pub benchmark_version: String,
    pub execution_model_version: String,
    pub report_version: String,
    pub annual_trading_days: u16,
    pub max_order_delay_sessions: u16,
    pub evaluation_interval: Option<DateInterval>,
}

impl Default for PortfolioBacktestConfig {
    fn default() -> Self {
        Self {
            initial_cash: 1_000_000.0,
            costs: CostModel::default(),
            benchmark: BenchmarkDefinition::EqualWeightedUniverse,
            benchmark_version: "equal-weighted-frozen-universe-v1".into(),
            execution_model_version: "cn-daily-next-open-v1".into(),
            report_version: "portfolio-backtest-report-v1".into(),
            annual_trading_days: 252,
            max_order_delay_sessions: 5,
            evaluation_interval: None,
        }
    }
}

impl PortfolioBacktestConfig {
    pub fn stable_hash(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("backtest config is always serializable");
        let digest = Sha256::digest(bytes);
        let mut output = String::with_capacity(digest.len() * 2 + 14);
        output.push_str("config-sha256:");
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
        }
        output
    }
}
