use serde::{Deserialize, Serialize};

use super::expression::Expression;

pub const STRATEGY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Timeframe {
    #[serde(rename = "1d")]
    OneDay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UniverseSpec {
    DatasetSnapshot { id: String },
    WatchlistSnapshot { id: String },
}

impl UniverseSpec {
    pub fn id(&self) -> &str {
        match self {
            Self::DatasetSnapshot { id } | Self::WatchlistSnapshot { id } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExitRule {
    All { all: Vec<ExitRule> },
    Any { any: Vec<ExitRule> },
    HoldDays { hold_days: u16 },
    StopLossPct { stop_loss_pct: f64 },
    TakeProfitPct { take_profit_pct: f64 },
    Condition { condition: Expression },
}

impl ExitRule {
    pub fn node_count(&self) -> usize {
        1 + match self {
            Self::All { all } => all.iter().map(Self::node_count).sum(),
            Self::Any { any } => any.iter().map(Self::node_count).sum(),
            Self::Condition { condition } => condition.node_count(),
            Self::HoldDays { .. } | Self::StopLossPct { .. } | Self::TakeProfitPct { .. } => 0,
        }
    }

    pub fn depth(&self) -> usize {
        1 + match self {
            Self::All { all } => all.iter().map(Self::depth).max().unwrap_or(0),
            Self::Any { any } => any.iter().map(Self::depth).max().unwrap_or(0),
            Self::Condition { condition } => condition.depth(),
            Self::HoldDays { .. } | Self::StopLossPct { .. } | Self::TakeProfitPct { .. } => 0,
        }
    }

    pub fn visit_expressions(&self, visitor: &mut impl FnMut(&Expression)) {
        match self {
            Self::All { all } | Self::Any { any: all } => {
                all.iter().for_each(|item| item.visit_expressions(visitor));
            }
            Self::Condition { condition } => visitor(condition),
            Self::HoldDays { .. } | Self::StopLossPct { .. } | Self::TakeProfitPct { .. } => {}
        }
    }

    pub fn contains_time_exit(&self) -> bool {
        match self {
            Self::HoldDays { .. } => true,
            Self::All { all } | Self::Any { any: all } => all.iter().any(Self::contains_time_exit),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionRule {
    pub size_pct: f64,
    pub max_positions: u16,
    #[serde(default)]
    pub allow_pyramiding: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyMetadata {
    pub generator: String,
    pub prompt_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_strategy_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategySpec {
    pub schema_version: u16,
    pub name: String,
    pub hypothesis: String,
    pub timeframe: Timeframe,
    pub universe: UniverseSpec,
    pub entry: Expression,
    pub exit: ExitRule,
    pub position: PositionRule,
    pub metadata: StrategyMetadata,
}
