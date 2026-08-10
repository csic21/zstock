use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::market::InstrumentId;

use super::config::PortfolioBacktestConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderRejectionReason {
    InvalidOpen,
    SuspendedOrNoVolume,
    DelayExpired,
    InsufficientCash,
    BelowBoardLot,
    MaximumPositions,
    AlreadyHeld,
    NoPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedOrder {
    pub instrument: InstrumentId,
    pub side: OrderSide,
    pub signal_date: String,
    pub attempted_date: String,
    pub reason: OrderRejectionReason,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioTrade {
    pub instrument: InstrumentId,
    pub signal_entry_date: String,
    pub entry_date: String,
    pub signal_exit_date: String,
    pub exit_date: String,
    pub quantity: u64,
    pub entry_price: f64,
    pub exit_price: f64,
    pub gross_pnl: f64,
    pub total_cost: f64,
    pub net_pnl: f64,
    pub net_return_pct: f64,
    pub holding_sessions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioEquityPoint {
    pub date: String,
    pub cash: f64,
    pub positions_value: f64,
    pub total_equity: f64,
    pub benchmark_equity: f64,
    pub open_positions: usize,
    pub exposure_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentFailure {
    pub instrument: InstrumentId,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenPositionSnapshot {
    pub instrument: InstrumentId,
    pub quantity: u64,
    pub entry_date: String,
    pub entry_price: f64,
    pub last_price: f64,
    pub unrealized_pnl: f64,
    pub holding_sessions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PortfolioMetrics {
    pub total_return_pct: f64,
    pub annualized_return_pct: f64,
    pub annualized_volatility_pct: f64,
    pub benchmark_return_pct: f64,
    pub excess_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub max_drawdown_duration_days: usize,
    pub sharpe: f64,
    pub sortino: f64,
    pub calmar: f64,
    pub win_rate_pct: f64,
    pub average_win: f64,
    pub average_loss: f64,
    pub payoff_ratio: f64,
    pub profit_factor: f64,
    pub trade_count: usize,
    pub average_holding_sessions: f64,
    pub market_exposure_pct: f64,
    pub turnover_pct: f64,
    pub gross_profit: f64,
    pub total_cost: f64,
    pub net_profit: f64,
    pub cost_to_gross_profit_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioBacktestReport {
    pub report_version: String,
    pub dataset_id: String,
    pub dataset_content_sha256: String,
    pub strategy_id: String,
    pub strategy_schema_version: u16,
    pub config: PortfolioBacktestConfig,
    pub config_hash: String,
    pub execution_rule: String,
    pub known_biases: Vec<String>,
    pub instrument_failures: Vec<InstrumentFailure>,
    pub rejected_orders: Vec<RejectedOrder>,
    pub trades: Vec<PortfolioTrade>,
    pub open_positions: Vec<OpenPositionSnapshot>,
    pub daily_equity: Vec<PortfolioEquityPoint>,
    pub metrics: PortfolioMetrics,
    pub metrics_by_year: BTreeMap<String, PortfolioMetrics>,
    pub metrics_by_instrument: BTreeMap<String, PortfolioMetrics>,
    pub completed_sessions: usize,
    pub cancelled: bool,
}
