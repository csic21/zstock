//! Safe, serializable strategy definitions and deterministic local execution.

pub mod compiler;
pub mod expression;
pub mod spec;
pub mod templates;
pub mod validation;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub use compiler::{CompiledStrategy, PositionContext};
pub use expression::{
    BollBand, CompareOperator, Comparison, Crossing, Expression, IndicatorRef, MacdComponent,
    ValueExpression,
};
pub use spec::{
    ExitRule, PositionRule, STRATEGY_SCHEMA_VERSION, StrategyMetadata, StrategySpec, Timeframe,
    UniverseSpec,
};
pub use templates::{LocalTemplate, local_templates};
pub use validation::{ValidatedStrategy, ValidationError, ValidationLimits, validate};

const MAX_STRATEGY_JSON_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StrategyLoadError {
    TooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    InvalidJson {
        message: String,
    },
    Validation {
        errors: Vec<ValidationError>,
    },
}

pub fn parse_and_compile(json: &str) -> Result<CompiledStrategy, StrategyLoadError> {
    if json.len() > MAX_STRATEGY_JSON_BYTES {
        return Err(StrategyLoadError::TooLarge {
            max_bytes: MAX_STRATEGY_JSON_BYTES,
            actual_bytes: json.len(),
        });
    }
    let spec: StrategySpec =
        serde_json::from_str(json).map_err(|error| StrategyLoadError::InvalidJson {
            message: error.to_string(),
        })?;
    CompiledStrategy::compile(spec).map_err(|errors| StrategyLoadError::Validation { errors })
}

pub fn normalized_json(spec: &StrategySpec) -> String {
    let value = serde_json::to_value(spec).expect("StrategySpec is always serializable");
    serde_json::to_string(&canonicalize(value, None))
        .expect("canonical StrategySpec is always serializable")
}

/// Stable execution identity. Presentation and generator metadata are excluded
/// so two candidates with the same executable meaning deduplicate correctly.
pub fn strategy_id(spec: &StrategySpec) -> String {
    let identity = serde_json::json!({
        "schema_version": spec.schema_version,
        "timeframe": spec.timeframe,
        "universe": spec.universe,
        "entry": spec.entry,
        "exit": spec.exit,
        "position": spec.position,
    });
    let canonical = serde_json::to_vec(&canonicalize(identity, None))
        .expect("strategy identity is always serializable");
    let digest = Sha256::digest(canonical);
    format!("strategy-sha256:{}", hex(&digest))
}

fn canonicalize(value: Value, parent_key: Option<&str>) -> Value {
    match value {
        Value::Object(object) => {
            let mut normalized = Map::new();
            let mut entries: Vec<_> = object.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, value) in entries {
                normalized.insert(key.clone(), canonicalize(value, Some(&key)));
            }
            Value::Object(normalized)
        }
        Value::Array(array) => {
            let mut normalized: Vec<_> = array
                .into_iter()
                .map(|item| canonicalize(item, None))
                .collect();
            if matches!(parent_key, Some("all" | "any")) {
                normalized.sort_by_key(|item| item.to_string());
            }
            Value::Array(normalized)
        }
        Value::Number(number) if number.as_f64() == Some(0.0) => Value::from(0.0),
        other => other,
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::CandleRecord;

    fn bars(closes: &[f64]) -> Vec<CandleRecord> {
        closes
            .iter()
            .enumerate()
            .map(|(index, close)| CandleRecord {
                time: format!("d{index:03}"),
                open: *close,
                high: *close + 1.0,
                low: *close - 1.0,
                close: *close,
                volume: 1_000,
            })
            .collect()
    }

    #[test]
    fn strategy_json_round_trip_matches_schema_example() {
        let json = r#"
        {
          "schema_version": 1,
          "name": "MA20 回踩反弹",
          "hypothesis": "中期趋势向上时，短期回踩后恢复可能延续趋势",
          "timeframe": "1d",
          "universe": { "kind": "dataset_snapshot", "id": "universe-sha256" },
          "entry": {
            "all": [
              {
                "compare": {
                  "left": { "indicator": "close", "lag": 0 },
                  "op": "above",
                  "right": { "indicator": "sma", "period": 20, "lag": 0 }
                }
              },
              {
                "crosses_above": {
                  "left": { "indicator": "rsi", "period": 14, "lag": 0 },
                  "right": { "constant": 40.0 }
                }
              }
            ]
          },
          "exit": {
            "any": [
              { "hold_days": 10 },
              { "stop_loss_pct": 6.0 },
              { "take_profit_pct": 12.0 }
            ]
          },
          "position": {
            "size_pct": 20.0,
            "max_positions": 5,
            "allow_pyramiding": false
          },
          "metadata": {
            "generator": "local-template-or-model",
            "prompt_version": "strategy-generator-v1"
          }
        }
        "#;

        let compiled = parse_and_compile(json).unwrap();
        let serialized = serde_json::to_string(compiled.spec()).unwrap();
        let reparsed = parse_and_compile(&serialized).unwrap();

        assert_eq!(compiled.spec(), reparsed.spec());
        assert_eq!(compiled.strategy_id(), reparsed.strategy_id());
        assert_eq!(compiled.warm_up_bars(), 20);
    }

    #[test]
    fn invalid_json_unknown_indicator_future_lag_period_and_empty_exit_are_rejected() {
        let valid = LocalTemplate::NDayHighBreakout.build("fixture");
        let mut unknown = serde_json::to_value(&valid).unwrap();
        unknown["entry"]["compare"]["right"]["indicator"] = Value::from("future_oracle");
        assert!(matches!(
            parse_and_compile(&unknown.to_string()),
            Err(StrategyLoadError::InvalidJson { .. })
        ));

        let mut future = valid.clone();
        future.entry = Expression::Compare {
            compare: Comparison {
                left: ValueExpression::Indicator(IndicatorRef::Close { lag: -1 }),
                op: CompareOperator::Above,
                right: ValueExpression::Constant { constant: 1.0 },
            },
        };
        let future_errors = CompiledStrategy::compile(future).unwrap_err();
        assert!(future_errors.iter().any(|error| error.code == "future_lag"));

        let mut period = valid.clone();
        period.entry = Expression::Compare {
            compare: Comparison {
                left: ValueExpression::Indicator(IndicatorRef::Sma {
                    period: 251,
                    lag: 0,
                }),
                op: CompareOperator::Above,
                right: ValueExpression::Constant { constant: 1.0 },
            },
        };
        let period_errors = CompiledStrategy::compile(period).unwrap_err();
        assert!(
            period_errors
                .iter()
                .any(|error| error.code == "period_out_of_range")
        );

        let mut no_exit = valid;
        no_exit.exit = ExitRule::Any { any: vec![] };
        let exit_errors = CompiledStrategy::compile(no_exit).unwrap_err();
        assert!(
            exit_errors
                .iter()
                .any(|error| error.code == "empty_exit_group")
        );
    }

    #[test]
    fn normalization_deduplicates_commutative_conditions_and_ignores_labels() {
        let mut left = LocalTemplate::MaTrendPullback.build("fixture");
        let mut right = left.clone();
        let Expression::All { all } = &mut right.entry else {
            panic!("MA template should contain all")
        };
        all.reverse();
        right.name = "renamed candidate".into();
        right.hypothesis = "same executable meaning".into();
        right.metadata.generator = "different-model".into();

        assert_ne!(normalized_json(&left), normalized_json(&right));
        assert_eq!(strategy_id(&left), strategy_id(&right));

        left.position.size_pct = 10.0;
        assert_ne!(strategy_id(&left), strategy_id(&right));
    }

    #[test]
    fn five_local_templates_compile_and_emit_expected_signals() {
        for spec in local_templates("fixture") {
            CompiledStrategy::compile(spec).unwrap();
        }

        let mut ma_closes = vec![100.0; 60];
        ma_closes[58] = 99.0;
        ma_closes[59] = 102.0;
        let ma =
            CompiledStrategy::compile(LocalTemplate::MaTrendPullback.build("fixture")).unwrap();
        assert!(ma.entry_signal(&bars(&ma_closes), 59));

        let mut rsi_closes: Vec<_> = (0..18).map(|index| 100.0 - index as f64).collect();
        rsi_closes.extend([84.0, 90.0, 95.0]);
        let rsi =
            CompiledStrategy::compile(LocalTemplate::RsiOversoldRecovery.build("fixture")).unwrap();
        assert!(
            rsi.entry_signals(&bars(&rsi_closes))
                .into_iter()
                .any(|value| value)
        );

        let mut breakout_bars = bars(&vec![100.0; 30]);
        breakout_bars[29].close = 102.0;
        breakout_bars[29].high = 103.0;
        let breakout =
            CompiledStrategy::compile(LocalTemplate::NDayHighBreakout.build("fixture")).unwrap();
        assert!(breakout.entry_signal(&breakout_bars, 29));

        let mut boll_closes = vec![100.0; 30];
        boll_closes[27] = 70.0;
        boll_closes[28] = 90.0;
        let boll =
            CompiledStrategy::compile(LocalTemplate::BollMeanReversion.build("fixture")).unwrap();
        assert!(boll.entry_signal(&bars(&boll_closes), 28));

        let mut volume_bars = bars(&vec![100.0; 30]);
        volume_bars[29].close = 102.0;
        volume_bars[29].volume = 2_000;
        let volume =
            CompiledStrategy::compile(LocalTemplate::VolumeTrendConfirmation.build("fixture"))
                .unwrap();
        assert!(volume.entry_signal(&volume_bars, 29));
    }

    #[test]
    fn compiled_signal_cannot_observe_future_bars() {
        let strategy =
            CompiledStrategy::compile(LocalTemplate::NDayHighBreakout.build("fixture")).unwrap();
        let mut original = bars(&vec![100.0; 40]);
        original[25].close = 102.0;
        let mut changed = original.clone();
        for bar in &mut changed[26..] {
            bar.close = 1_000_000.0;
            bar.high = 1_000_001.0;
        }

        assert_eq!(
            strategy.entry_signal(&original, 25),
            strategy.entry_signal(&changed, 25)
        );
    }

    #[test]
    fn every_compare_operator_has_deterministic_semantics() {
        let cases = [
            (CompareOperator::Above, 2.0, 1.0, true),
            (CompareOperator::Below, 1.0, 2.0, true),
            (CompareOperator::AtLeast, 2.0, 2.0, true),
            (CompareOperator::AtMost, 2.0, 2.0, true),
            (CompareOperator::Equal, 2.0, 2.0, true),
        ];
        for (operator, left, right, expected) in cases {
            let mut spec = LocalTemplate::NDayHighBreakout.build("fixture");
            spec.entry = Expression::Compare {
                compare: Comparison {
                    left: ValueExpression::Constant { constant: left },
                    op: operator,
                    right: ValueExpression::Constant { constant: right },
                },
            };
            let strategy = CompiledStrategy::compile(spec).unwrap();
            assert_eq!(strategy.entry_signal(&bars(&[100.0; 25]), 20), expected);
        }
    }
}
