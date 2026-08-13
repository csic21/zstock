use super::expression::{
    BollBand, CompareOperator, Comparison, Crossing, Expression, IndicatorRef, ValueExpression,
};
use crate::domain::exit_quality::mae_aware_exit;

use super::spec::{
    ExitRule, PositionRule, STRATEGY_SCHEMA_VERSION, StrategyMetadata, StrategySpec, Timeframe,
    UniverseSpec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalTemplate {
    MaTrendPullback,
    RsiOversoldRecovery,
    NDayHighBreakout,
    BollMeanReversion,
    VolumeTrendConfirmation,
}

impl LocalTemplate {
    pub const fn all() -> [Self; 5] {
        [
            Self::MaTrendPullback,
            Self::RsiOversoldRecovery,
            Self::NDayHighBreakout,
            Self::BollMeanReversion,
            Self::VolumeTrendConfirmation,
        ]
    }

    pub fn build(self, universe_id: &str) -> StrategySpec {
        let (name, hypothesis, entry, hold_days) = match self {
            Self::MaTrendPullback => (
                "MA 趋势回踩",
                "中期趋势向上时，价格重新站上短期均线可能延续趋势",
                Expression::All {
                    all: vec![
                        compare(
                            indicator(IndicatorRef::Close { lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::Sma { period: 50, lag: 0 }),
                        ),
                        crosses_above(
                            indicator(IndicatorRef::Close { lag: 0 }),
                            indicator(IndicatorRef::Sma { period: 20, lag: 0 }),
                        ),
                    ],
                },
                15,
            ),
            Self::RsiOversoldRecovery => (
                "RSI 超卖恢复",
                "RSI 从超卖区恢复可能对应短期均值回归",
                crosses_above(
                    indicator(IndicatorRef::Rsi { period: 14, lag: 0 }),
                    constant(30.0),
                ),
                10,
            ),
            Self::NDayHighBreakout => (
                "N 日高点突破",
                "价格突破过去 20 日高点可能表明趋势启动",
                compare(
                    indicator(IndicatorRef::Close { lag: 0 }),
                    CompareOperator::Above,
                    indicator(IndicatorRef::NDayHigh { period: 20, lag: 0 }),
                ),
                20,
            ),
            Self::BollMeanReversion => (
                "BOLL 均值回归",
                "价格跌破布林下轨后重新站回可能出现均值回归",
                crosses_above(
                    indicator(IndicatorRef::Close { lag: 0 }),
                    indicator(IndicatorRef::Boll {
                        period: 20,
                        std_dev: 2.0,
                        band: BollBand::Lower,
                        lag: 0,
                    }),
                ),
                10,
            ),
            Self::VolumeTrendConfirmation => (
                "放量趋势确认",
                "价格处于短期均线上方且成交量较前一日增加时，趋势信号更可信",
                Expression::All {
                    all: vec![
                        compare(
                            indicator(IndicatorRef::Close { lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::Sma { period: 20, lag: 0 }),
                        ),
                        compare(
                            indicator(IndicatorRef::Volume { lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::Volume { lag: 1 }),
                        ),
                    ],
                },
                12,
            ),
        };
        StrategySpec {
            schema_version: STRATEGY_SCHEMA_VERSION,
            name: name.into(),
            hypothesis: hypothesis.into(),
            timeframe: Timeframe::OneDay,
            universe: UniverseSpec::DatasetSnapshot {
                id: universe_id.into(),
            },
            entry,
            exit: default_exit(hold_days),
            position: PositionRule {
                size_pct: 20.0,
                max_positions: 5,
                allow_pyramiding: false,
            },
            metadata: StrategyMetadata {
                generator: "local-template".into(),
                prompt_version: "local-template-v1".into(),
                model: None,
                parent_strategy_id: None,
            },
        }
    }
}

pub fn local_templates(universe_id: &str) -> Vec<StrategySpec> {
    LocalTemplate::all()
        .into_iter()
        .map(|template| template.build(universe_id))
        .collect()
}

/// Scanner-aligned playbooks: radar pullback / breakout / oversold + treasure low.
pub fn scan_playbooks(universe_id: &str) -> Vec<StrategySpec> {
    ScanPlaybook::all()
        .into_iter()
        .map(|playbook| playbook.build(universe_id))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPlaybook {
    RadarPullback,
    RadarBreakout,
    RadarOversold,
    TreasureLow,
}

impl ScanPlaybook {
    pub const fn all() -> [Self; 4] {
        [
            Self::RadarPullback,
            Self::RadarBreakout,
            Self::RadarOversold,
            Self::TreasureLow,
        ]
    }

    pub fn build(self, universe_id: &str) -> StrategySpec {
        let (name, hypothesis, entry, hold_days, stop_loss_pct, take_profit_pct) = match self {
            Self::RadarPullback => (
                "雷达·强势回踩",
                "与短线雷达「强势回踩」对齐：价格仍在中期均线上方、贴近 MA20、RSI 未过热",
                Expression::All {
                    all: vec![
                        compare(
                            indicator(IndicatorRef::Close { lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::Sma { period: 20, lag: 0 }),
                        ),
                        compare(
                            indicator(IndicatorRef::Sma { period: 20, lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::Sma { period: 60, lag: 0 }),
                        ),
                        compare(
                            indicator(IndicatorRef::Rsi { period: 14, lag: 0 }),
                            CompareOperator::AtLeast,
                            constant(38.0),
                        ),
                        compare(
                            indicator(IndicatorRef::Rsi { period: 14, lag: 0 }),
                            CompareOperator::AtMost,
                            constant(62.0),
                        ),
                    ],
                },
                8,
                5.0,
                9.0,
            ),
            Self::RadarBreakout => (
                "雷达·放量突破",
                "与短线雷达「放量突破」对齐：收盘站上 20 日高且量能高于前一日，RSI 不过热",
                Expression::All {
                    all: vec![
                        compare(
                            indicator(IndicatorRef::Close { lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::NDayHigh { period: 20, lag: 0 }),
                        ),
                        compare(
                            indicator(IndicatorRef::Volume { lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::Volume { lag: 1 }),
                        ),
                        compare(
                            indicator(IndicatorRef::Rsi { period: 14, lag: 0 }),
                            CompareOperator::AtMost,
                            constant(78.0),
                        ),
                    ],
                },
                8,
                5.0,
                10.0,
            ),
            Self::RadarOversold => (
                "雷达·超跌反弹",
                "与短线雷达「超跌反弹」对齐：RSI 离开超卖区后再观察，不用固定持有把反弹坐回去",
                crosses_above(
                    indicator(IndicatorRef::Rsi { period: 14, lag: 0 }),
                    constant(30.0),
                ),
                6,
                5.0,
                8.0,
            ),
            Self::TreasureLow => (
                "寻宝·低位观察",
                "与长线寻宝「可关注」门槛对齐：价格低于中期均线、未贴近 60 日高、RSI 不高",
                Expression::All {
                    all: vec![
                        compare(
                            indicator(IndicatorRef::Close { lag: 0 }),
                            CompareOperator::Below,
                            indicator(IndicatorRef::Sma { period: 60, lag: 0 }),
                        ),
                        compare(
                            indicator(IndicatorRef::Close { lag: 0 }),
                            CompareOperator::Below,
                            indicator(IndicatorRef::NDayHigh { period: 60, lag: 0 }),
                        ),
                        compare(
                            indicator(IndicatorRef::Rsi { period: 14, lag: 0 }),
                            CompareOperator::AtMost,
                            constant(45.0),
                        ),
                    ],
                },
                20,
                8.0,
                16.0,
            ),
        };
        StrategySpec {
            schema_version: STRATEGY_SCHEMA_VERSION,
            name: name.into(),
            hypothesis: hypothesis.into(),
            timeframe: Timeframe::OneDay,
            universe: UniverseSpec::DatasetSnapshot {
                id: universe_id.into(),
            },
            entry,
            exit: mae_aware_exit(hold_days, stop_loss_pct, take_profit_pct),
            position: PositionRule {
                size_pct: 20.0,
                max_positions: 5,
                allow_pyramiding: false,
            },
            metadata: StrategyMetadata {
                generator: "scan-playbook".into(),
                prompt_version: "scan-playbook-v1".into(),
                model: None,
                parent_strategy_id: None,
            },
        }
    }
}

pub(crate) fn compare(
    left: ValueExpression,
    op: CompareOperator,
    right: ValueExpression,
) -> Expression {
    Expression::Compare {
        compare: Comparison { left, op, right },
    }
}

pub(crate) fn crosses_above(left: ValueExpression, right: ValueExpression) -> Expression {
    Expression::CrossesAbove {
        crosses_above: Crossing { left, right },
    }
}

pub(crate) fn indicator(indicator: IndicatorRef) -> ValueExpression {
    ValueExpression::Indicator(indicator)
}

pub(crate) const fn constant(constant: f64) -> ValueExpression {
    ValueExpression::Constant { constant }
}

pub(crate) fn default_exit(hold_days: u16) -> ExitRule {
    ExitRule::Any {
        any: vec![
            ExitRule::HoldDays { hold_days },
            ExitRule::StopLossPct { stop_loss_pct: 6.0 },
            ExitRule::TakeProfitPct {
                take_profit_pct: 12.0,
            },
        ],
    }
}
