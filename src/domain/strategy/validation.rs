use serde::{Deserialize, Serialize};

use super::expression::{IndicatorRef, ValueExpression};
use super::spec::{ExitRule, STRATEGY_SCHEMA_VERSION, StrategySpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationLimits {
    pub max_depth: usize,
    pub max_nodes: usize,
    pub min_period: u16,
    pub max_period: u16,
    pub max_lag: i16,
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self {
            max_depth: 6,
            max_nodes: 32,
            min_period: 2,
            max_period: 250,
            max_lag: 250,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    pub code: String,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedStrategy {
    pub warm_up_bars: usize,
    pub entry_nodes: usize,
    pub exit_nodes: usize,
}

pub fn validate(
    spec: &StrategySpec,
    limits: ValidationLimits,
) -> Result<ValidatedStrategy, Vec<ValidationError>> {
    let mut errors = Vec::new();
    let mut warm_up_bars = 1;
    if spec.schema_version != STRATEGY_SCHEMA_VERSION {
        push(
            &mut errors,
            "unsupported_schema_version",
            "schema_version",
            format!(
                "expected schema version {STRATEGY_SCHEMA_VERSION}, got {}",
                spec.schema_version
            ),
        );
    }
    if spec.name.trim().is_empty() || spec.name.chars().count() > 80 {
        push(
            &mut errors,
            "invalid_name",
            "name",
            "name must contain 1–80 characters",
        );
    }
    if spec.hypothesis.trim().is_empty() || spec.hypothesis.chars().count() > 500 {
        push(
            &mut errors,
            "invalid_hypothesis",
            "hypothesis",
            "hypothesis must contain 1–500 characters",
        );
    }
    if spec.universe.id().trim().is_empty() {
        push(
            &mut errors,
            "missing_universe_id",
            "universe.id",
            "universe snapshot id cannot be empty",
        );
    }
    if spec.metadata.generator.trim().is_empty() || spec.metadata.prompt_version.trim().is_empty() {
        push(
            &mut errors,
            "missing_generator_metadata",
            "metadata",
            "generator and prompt_version are required",
        );
    }

    let entry_nodes = spec.entry.node_count();
    if entry_nodes > limits.max_nodes {
        push(
            &mut errors,
            "entry_too_complex",
            "entry",
            format!(
                "entry has {entry_nodes} nodes; maximum is {}",
                limits.max_nodes
            ),
        );
    }
    if spec.entry.depth() > limits.max_depth {
        push(
            &mut errors,
            "entry_too_deep",
            "entry",
            format!("entry nesting depth exceeds {}", limits.max_depth),
        );
    }
    validate_expression(&spec.entry, "entry", limits, &mut warm_up_bars, &mut errors);

    let exit_nodes = spec.exit.node_count();
    if exit_nodes > limits.max_nodes {
        push(
            &mut errors,
            "exit_too_complex",
            "exit",
            format!(
                "exit has {exit_nodes} nodes; maximum is {}",
                limits.max_nodes
            ),
        );
    }
    if spec.exit.depth() > limits.max_depth {
        push(
            &mut errors,
            "exit_too_deep",
            "exit",
            format!("exit nesting depth exceeds {}", limits.max_depth),
        );
    }
    validate_exit(&spec.exit, "exit", limits, &mut warm_up_bars, &mut errors);

    if !spec.position.size_pct.is_finite()
        || spec.position.size_pct <= 0.0
        || spec.position.size_pct > 100.0
    {
        push(
            &mut errors,
            "invalid_position_size",
            "position.size_pct",
            "position size must be in (0, 100]",
        );
    }
    if spec.position.max_positions == 0 || spec.position.max_positions > 100 {
        push(
            &mut errors,
            "invalid_max_positions",
            "position.max_positions",
            "max_positions must be in 1–100",
        );
    }
    if spec.position.size_pct.is_finite()
        && spec.position.size_pct * f64::from(spec.position.max_positions) > 100.0 + 1e-9
    {
        push(
            &mut errors,
            "position_exceeds_cash",
            "position",
            "size_pct × max_positions cannot exceed 100%",
        );
    }

    if errors.is_empty() {
        Ok(ValidatedStrategy {
            warm_up_bars,
            entry_nodes,
            exit_nodes,
        })
    } else {
        Err(errors)
    }
}

fn validate_expression(
    expression: &super::expression::Expression,
    path: &str,
    limits: ValidationLimits,
    warm_up_bars: &mut usize,
    errors: &mut Vec<ValidationError>,
) {
    match expression {
        super::expression::Expression::All { all } => {
            if all.is_empty() {
                push(
                    errors,
                    "empty_all",
                    path,
                    "all must contain at least one condition",
                );
            }
        }
        super::expression::Expression::Any { any } => {
            if any.is_empty() {
                push(
                    errors,
                    "empty_any",
                    path,
                    "any must contain at least one condition",
                );
            }
        }
        _ => {}
    }
    expression.visit_values(&mut |value| {
        validate_value(value, path, limits, warm_up_bars, errors);
    });
}

fn validate_value(
    value: &ValueExpression,
    path: &str,
    limits: ValidationLimits,
    warm_up_bars: &mut usize,
    errors: &mut Vec<ValidationError>,
) {
    let ValueExpression::Indicator(indicator) = value else {
        if let ValueExpression::Constant { constant } = value
            && !constant.is_finite()
        {
            push(
                errors,
                "non_finite_constant",
                path,
                "constant must be finite",
            );
        }
        return;
    };
    if indicator.lag() < 0 {
        push(
            errors,
            "future_lag",
            path,
            "negative lag would read future data",
        );
    } else if indicator.lag() > limits.max_lag {
        push(
            errors,
            "lag_out_of_range",
            path,
            format!("lag cannot exceed {}", limits.max_lag),
        );
    }
    for period in indicator.periods() {
        if period < limits.min_period || period > limits.max_period {
            push(
                errors,
                "period_out_of_range",
                path,
                format!(
                    "indicator period {period} is outside {}–{}",
                    limits.min_period, limits.max_period
                ),
            );
        }
    }
    match indicator {
        IndicatorRef::Macd {
            fast_period,
            slow_period,
            ..
        } if fast_period >= slow_period => push(
            errors,
            "invalid_macd_periods",
            path,
            "MACD fast_period must be less than slow_period",
        ),
        IndicatorRef::Boll { std_dev, .. }
            if !std_dev.is_finite() || !(0.5..=5.0).contains(std_dev) =>
        {
            push(
                errors,
                "invalid_boll_std_dev",
                path,
                "BOLL std_dev must be in 0.5–5.0",
            );
        }
        _ => {}
    }
    *warm_up_bars = (*warm_up_bars).max(indicator.warm_up());
}

fn validate_exit(
    exit: &ExitRule,
    path: &str,
    limits: ValidationLimits,
    warm_up_bars: &mut usize,
    errors: &mut Vec<ValidationError>,
) {
    match exit {
        ExitRule::All { all } | ExitRule::Any { any: all } => {
            if all.is_empty() {
                push(
                    errors,
                    "empty_exit_group",
                    path,
                    "exit group must contain at least one rule",
                );
            }
            for child in all {
                validate_exit(child, path, limits, warm_up_bars, errors);
            }
        }
        ExitRule::HoldDays { hold_days } if !(1..=250).contains(hold_days) => push(
            errors,
            "hold_days_out_of_range",
            path,
            "hold_days must be in 1–250",
        ),
        ExitRule::StopLossPct { stop_loss_pct }
            if !stop_loss_pct.is_finite() || !(0.5..=30.0).contains(stop_loss_pct) =>
        {
            push(
                errors,
                "stop_loss_out_of_range",
                path,
                "stop_loss_pct must be in 0.5–30.0",
            );
        }
        ExitRule::TakeProfitPct { take_profit_pct }
            if !take_profit_pct.is_finite() || !(0.5..=100.0).contains(take_profit_pct) =>
        {
            push(
                errors,
                "take_profit_out_of_range",
                path,
                "take_profit_pct must be in 0.5–100.0",
            );
        }
        ExitRule::Condition { condition } => {
            validate_expression(condition, path, limits, warm_up_bars, errors);
        }
        _ => {}
    }
}

fn push(
    errors: &mut Vec<ValidationError>,
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    errors.push(ValidationError {
        code: code.into(),
        path: path.into(),
        message: message.into(),
    });
}
