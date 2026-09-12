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
    LimitUpFirstBoard,
    LimitUpSecondBoard,
}

impl ScanPlaybook {
    pub const fn all() -> [Self; 6] {
        [
            Self::RadarPullback,
            Self::RadarBreakout,
            Self::RadarOversold,
            Self::TreasureLow,
            Self::LimitUpFirstBoard,
            Self::LimitUpSecondBoard,
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
            // 连板是 DSL 表达能力的边界：白名单没有“涨停板幅度”算子，
            // 这里用近似条件（单日涨幅 ≥9% + 放量 + 未过热）做可回测代理；
            // 精确的板数/炸板/一字板识别由 `data::limitup` 离线计算。
            // 20% 板的大涨同样会触发本模板，hypothesis 已如实说明。
            Self::LimitUpFirstBoard => (
                "连板·首板次日",
                "首板次日开盘溢价博弈的近似回测：昨日大涨近板且放量，持有3日短打",
                Expression::All {
                    all: vec![
                        compare(
                            indicator(IndicatorRef::Return { period: 1, lag: 0 }),
                            CompareOperator::AtLeast,
                            constant(9.0),
                        ),
                        compare(
                            indicator(IndicatorRef::Volume { lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::Volume { lag: 1 }),
                        ),
                        compare(
                            indicator(IndicatorRef::Rsi { period: 14, lag: 0 }),
                            CompareOperator::AtMost,
                            constant(82.0),
                        ),
                    ],
                },
                3,
                6.0,
                8.0,
            ),
            Self::LimitUpSecondBoard => (
                "连板·二进一",
                "连续两日大涨（二连板近似）的接力回测：只做情绪连续确认，持有3日",
                Expression::All {
                    all: vec![
                        compare(
                            indicator(IndicatorRef::Return { period: 1, lag: 0 }),
                            CompareOperator::AtLeast,
                            constant(9.0),
                        ),
                        compare(
                            indicator(IndicatorRef::Return { period: 1, lag: 1 }),
                            CompareOperator::AtLeast,
                            constant(9.0),
                        ),
                        compare(
                            indicator(IndicatorRef::Volume { lag: 0 }),
                            CompareOperator::Above,
                            indicator(IndicatorRef::Volume { lag: 1 }),
                        ),
                    ],
                },
                3,
                7.0,
                10.0,
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
