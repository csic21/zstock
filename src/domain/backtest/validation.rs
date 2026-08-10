use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::dataset::{DateInterval, FrozenDataset};
use crate::domain::strategy::{CompiledStrategy, StrategySpec, strategy_id};

use super::EvidenceGrade;
use super::config::PortfolioBacktestConfig;
use super::metrics::calculate_trade_metrics;
use super::portfolio::run_portfolio_backtest;
use super::report::{PortfolioBacktestReport, PortfolioMetrics, PortfolioTrade};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedSplitConfig {
    pub training_pct: u8,
    pub validation_pct: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalkForwardConfig {
    pub training_sessions: usize,
    pub validation_sessions: usize,
    pub step_sessions: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionConfig {
    pub version: String,
    pub minimum_trades: usize,
    pub minimum_instruments: usize,
    pub minimum_walk_forward_windows: usize,
    pub max_drawdown_pct: f64,
    pub max_contribution_pct: f64,
    pub minimum_positive_neighbor_fraction: f64,
}

impl Default for PromotionConfig {
    fn default() -> Self {
        Self {
            version: "promotion-gates-v1".into(),
            minimum_trades: 50,
            minimum_instruments: 20,
            minimum_walk_forward_windows: 3,
            max_drawdown_pct: 20.0,
            max_contribution_pct: 30.0,
            minimum_positive_neighbor_fraction: 0.7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RobustnessConfig {
    pub version: String,
    pub fixed_split: FixedSplitConfig,
    pub walk_forward: WalkForwardConfig,
    pub promotion: PromotionConfig,
    pub bootstrap_block_sessions: usize,
    pub bootstrap_samples: usize,
    pub high_volatility_threshold_pct: f64,
    #[serde(default)]
    pub industry_by_instrument: BTreeMap<String, String>,
}

impl Default for RobustnessConfig {
    fn default() -> Self {
        Self {
            version: "robustness-validation-v1".into(),
            fixed_split: FixedSplitConfig {
                training_pct: 60,
                validation_pct: 20,
            },
            walk_forward: WalkForwardConfig {
                training_sessions: 252,
                validation_sessions: 63,
                step_sessions: 63,
            },
            promotion: PromotionConfig::default(),
            bootstrap_block_sessions: 20,
            bootstrap_samples: 1_000,
            high_volatility_threshold_pct: 30.0,
            industry_by_instrument: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketRegime {
    Bull,
    Bear,
    Sideways,
    HighVolatility,
    InsufficientHistory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WalkForwardWindowResult {
    pub ordinal: usize,
    pub training_interval: DateInterval,
    pub validation_interval: DateInterval,
    pub metrics: PortfolioMetrics,
    pub trade_count: usize,
    pub covered_instruments: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StressTestResult {
    pub label: String,
    pub cost_multiplier: f64,
    pub metrics: PortfolioMetrics,
    pub survived: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterStabilityResult {
    pub attempted_neighbors: usize,
    pub valid_neighbors: usize,
    pub positive_neighbors: usize,
    pub positive_fraction: f64,
    pub worst_excess_return_pct: f64,
    pub median_excess_return_pct: f64,
    pub stable: bool,
    pub neighbor_strategy_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConcentrationResult {
    pub by_instrument_net_pnl: BTreeMap<String, f64>,
    pub by_industry_net_pnl: BTreeMap<String, f64>,
    pub by_year_net_pnl: BTreeMap<String, f64>,
    pub by_window_net_pnl: BTreeMap<String, f64>,
    pub max_instrument_contribution_pct: f64,
    pub max_industry_contribution_pct: Option<f64>,
    pub max_year_contribution_pct: f64,
    pub max_window_contribution_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BootstrapInterval {
    pub samples: usize,
    pub block_sessions: usize,
    pub annualized_mean_return_low_pct: f64,
    pub annualized_mean_return_high_pct: f64,
    pub deterministic_seed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionConclusion {
    Rejected,
    ContinueResearch,
    PaperCandidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateCode {
    DataIntegrity,
    MinimumTrades,
    MinimumInstruments,
    WalkForwardWindows,
    PositiveOosExcess,
    CostStress,
    DrawdownBudget,
    InstrumentConcentration,
    YearConcentration,
    ParameterStability,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateResult {
    pub code: GateCode,
    pub passed: bool,
    pub observed: String,
    pub required: String,
    pub explanation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionDecision {
    pub config_version: String,
    pub evidence_grade: EvidenceGrade,
    pub conclusion: PromotionConclusion,
    pub gates: Vec<GateResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RobustnessReport {
    pub validation_version: String,
    pub strategy_id: String,
    pub dataset_id: String,
    pub training_interval: DateInterval,
    pub validation_interval: DateInterval,
    pub sealed_test_interval: DateInterval,
    pub training_report: PortfolioBacktestReport,
    pub validation_report: PortfolioBacktestReport,
    pub walk_forward: Vec<WalkForwardWindowResult>,
    pub stress_tests: Vec<StressTestResult>,
    pub parameter_stability: ParameterStabilityResult,
    pub concentration: ConcentrationResult,
    pub bootstrap: Option<BootstrapInterval>,
    pub metrics_by_regime: BTreeMap<MarketRegime, PortfolioMetrics>,
    pub strategy_attempts: usize,
    pub parameter_attempts: usize,
    pub promotion: PromotionDecision,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SealedTestResult {
    pub strategy_id: String,
    pub interval: DateInterval,
    pub consumed_at: String,
    pub report: PortfolioBacktestReport,
}

pub fn evaluate_robustness(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    base_config: &PortfolioBacktestConfig,
    config: &RobustnessConfig,
    strategy_attempts: usize,
) -> Result<RobustnessReport> {
    validate_robustness_config(config)?;
    let calendar = dataset_calendar(dataset);
    let (training_interval, validation_interval, sealed_test_interval, pretest_end) =
        fixed_intervals(&calendar, &config.fixed_split)?;
    let training_report = run_interval(dataset, strategy, base_config, &training_interval)?;
    let validation_report = run_interval(dataset, strategy, base_config, &validation_interval)?;
    let walk_forward = run_walk_forward(
        dataset,
        strategy,
        base_config,
        config,
        &calendar[..=pretest_end],
    )?;
    let stress_tests = run_stress_tests(dataset, strategy, base_config, &validation_interval)?;
    let parameter_stability = run_parameter_neighbors(
        dataset,
        strategy,
        base_config,
        &validation_interval,
        &config.promotion,
    )?;
    let concentration = concentration(
        &validation_report.trades,
        &walk_forward,
        &config.industry_by_instrument,
    );
    let bootstrap = block_bootstrap(
        &validation_report,
        config.bootstrap_block_sessions,
        config.bootstrap_samples,
    );
    let regimes = classify_market_regimes(dataset, &calendar, config.high_volatility_threshold_pct);
    let metrics_by_regime = regime_metrics(&validation_report, &regimes, base_config.initial_cash);
    let promotion = promotion_decision(
        &validation_report,
        &walk_forward,
        &stress_tests,
        &parameter_stability,
        &concentration,
        &config.promotion,
    );
    Ok(RobustnessReport {
        validation_version: config.version.clone(),
        strategy_id: strategy.strategy_id().into(),
        dataset_id: dataset.manifest.id.clone(),
        training_interval,
        validation_interval,
        sealed_test_interval,
        training_report,
        validation_report,
        walk_forward,
        stress_tests,
        parameter_attempts: parameter_stability.attempted_neighbors,
        parameter_stability,
        concentration,
        bootstrap,
        metrics_by_regime,
        strategy_attempts,
        promotion,
    })
}

pub fn consume_sealed_test(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    base_config: &PortfolioBacktestConfig,
    robustness: &RobustnessReport,
    already_consumed_at: Option<&str>,
    consumed_at: &str,
) -> Result<SealedTestResult> {
    if let Some(previous) = already_consumed_at {
        bail!("sealed test was already consumed at {previous}");
    }
    if robustness.strategy_id != strategy.strategy_id()
        || robustness.dataset_id != dataset.manifest.id
    {
        bail!("sealed test inputs do not match robustness report versions");
    }
    let report = run_interval(
        dataset,
        strategy,
        base_config,
        &robustness.sealed_test_interval,
    )?;
    Ok(SealedTestResult {
        strategy_id: strategy.strategy_id().into(),
        interval: robustness.sealed_test_interval.clone(),
        consumed_at: consumed_at.into(),
        report,
    })
}

pub fn promotion_decision(
    validation: &PortfolioBacktestReport,
    walk_forward: &[WalkForwardWindowResult],
    stress_tests: &[StressTestResult],
    parameter_stability: &ParameterStabilityResult,
    concentration: &ConcentrationResult,
    config: &PromotionConfig,
) -> PromotionDecision {
    let covered: BTreeSet<_> = validation
        .trades
        .iter()
        .map(|trade| trade.instrument.storage_key())
        .collect();
    let positive_walk = walk_forward
        .iter()
        .filter(|window| window.metrics.excess_return_pct > 0.0)
        .count();
    let gates = vec![
        gate(
            GateCode::DataIntegrity,
            validation.instrument_failures.is_empty(),
            format!(
                "{} instrument failures",
                validation.instrument_failures.len()
            ),
            "0 instrument failures",
            "冻结数据必须通过完整性检查",
        ),
        gate(
            GateCode::MinimumTrades,
            validation.trades.len() >= config.minimum_trades,
            validation.trades.len().to_string(),
            format!(">={}", config.minimum_trades),
            "样本外交易数不足会放大偶然性",
        ),
        gate(
            GateCode::MinimumInstruments,
            covered.len() >= config.minimum_instruments,
            covered.len().to_string(),
            format!(">={}", config.minimum_instruments),
            "需要跨标的覆盖而非单股拟合",
        ),
        gate(
            GateCode::WalkForwardWindows,
            walk_forward.len() >= config.minimum_walk_forward_windows
                && positive_walk * 2 >= walk_forward.len(),
            format!("{positive_walk}/{} positive", walk_forward.len()),
            format!(
                ">={} windows and majority positive",
                config.minimum_walk_forward_windows
            ),
            "滚动样本外窗口需要数量和方向稳定性",
        ),
        gate(
            GateCode::PositiveOosExcess,
            validation.metrics.excess_return_pct > 0.0,
            format!("{:.3}%", validation.metrics.excess_return_pct),
            ">0%",
            "扣除成本后的样本外超额必须为正",
        ),
        gate(
            GateCode::CostStress,
            !stress_tests.is_empty() && stress_tests.iter().all(|stress| stress.survived),
            format!(
                "{} of {} survived",
                stress_tests.iter().filter(|stress| stress.survived).count(),
                stress_tests.len()
            ),
            "all survived",
            "成本和滑点翻倍后不能整体失效",
        ),
        gate(
            GateCode::DrawdownBudget,
            validation.metrics.max_drawdown_pct.abs() <= config.max_drawdown_pct,
            format!("{:.3}%", validation.metrics.max_drawdown_pct.abs()),
            format!("<={:.3}%", config.max_drawdown_pct),
            "最大回撤必须处于用户风险预算内",
        ),
        gate(
            GateCode::InstrumentConcentration,
            concentration.max_instrument_contribution_pct <= config.max_contribution_pct,
            format!("{:.3}%", concentration.max_instrument_contribution_pct),
            format!("<={:.3}%", config.max_contribution_pct),
            "单一股票贡献不能主导净收益",
        ),
        gate(
            GateCode::YearConcentration,
            concentration.max_year_contribution_pct <= config.max_contribution_pct,
            format!("{:.3}%", concentration.max_year_contribution_pct),
            format!("<={:.3}%", config.max_contribution_pct),
            "单一年份贡献不能主导净收益",
        ),
        gate(
            GateCode::ParameterStability,
            parameter_stability.stable,
            format!("{:.3} positive", parameter_stability.positive_fraction),
            format!(">={:.3}", config.minimum_positive_neighbor_fraction),
            "相邻合理参数不能出现断崖式恶化",
        ),
    ];
    let all_passed = gates.iter().all(|gate| gate.passed);
    let decisive_failure = gates.iter().any(|gate| {
        !gate.passed
            && matches!(
                gate.code,
                GateCode::DataIntegrity
                    | GateCode::PositiveOosExcess
                    | GateCode::CostStress
                    | GateCode::DrawdownBudget
            )
    });
    let evidence_grade = if validation.trades.is_empty() {
        EvidenceGrade::None
    } else if validation.trades.len() < config.minimum_trades {
        EvidenceGrade::InsufficientSample
    } else if validation.metrics.excess_return_pct <= 0.0 {
        EvidenceGrade::InSampleExploration
    } else if all_passed {
        EvidenceGrade::MultiPeriodStable
    } else {
        EvidenceGrade::OutOfSampleObservation
    };
    PromotionDecision {
        config_version: config.version.clone(),
        evidence_grade,
        conclusion: if all_passed {
            PromotionConclusion::PaperCandidate
        } else if decisive_failure {
            PromotionConclusion::Rejected
        } else {
            PromotionConclusion::ContinueResearch
        },
        gates,
    }
}

fn run_interval(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    base_config: &PortfolioBacktestConfig,
    interval: &DateInterval,
) -> Result<PortfolioBacktestReport> {
    let mut config = base_config.clone();
    config.evaluation_interval = Some(interval.clone());
    run_portfolio_backtest(dataset, strategy, &config)
}

fn run_walk_forward(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    base_config: &PortfolioBacktestConfig,
    config: &RobustnessConfig,
    pretest_calendar: &[String],
) -> Result<Vec<WalkForwardWindowResult>> {
    let walk = &config.walk_forward;
    let window_size = walk.training_sessions + walk.validation_sessions;
    if pretest_calendar.len() < window_size {
        return Ok(vec![]);
    }
    let mut windows = Vec::new();
    let mut start = 0;
    while start + window_size <= pretest_calendar.len() {
        let training_interval = DateInterval {
            start: pretest_calendar[start].clone(),
            end: pretest_calendar[start + walk.training_sessions - 1].clone(),
        };
        let validation_start = start + walk.training_sessions;
        let validation_interval = DateInterval {
            start: pretest_calendar[validation_start].clone(),
            end: pretest_calendar[validation_start + walk.validation_sessions - 1].clone(),
        };
        let report = run_interval(dataset, strategy, base_config, &validation_interval)?;
        let covered_instruments = report
            .trades
            .iter()
            .map(|trade| trade.instrument.storage_key())
            .collect::<BTreeSet<_>>()
            .len();
        windows.push(WalkForwardWindowResult {
            ordinal: windows.len(),
            training_interval,
            validation_interval,
            metrics: report.metrics,
            trade_count: report.trades.len(),
            covered_instruments,
        });
        start += walk.step_sessions;
    }
    Ok(windows)
}

fn run_stress_tests(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    base_config: &PortfolioBacktestConfig,
    interval: &DateInterval,
) -> Result<Vec<StressTestResult>> {
    let mut stressed = base_config.clone();
    stressed.costs.commission_bps_each_side *= 2.0;
    stressed.costs.minimum_commission *= 2.0;
    stressed.costs.sell_tax_bps *= 2.0;
    stressed.costs.slippage_bps_each_side *= 2.0;
    stressed.costs.other_fees_bps_each_side *= 2.0;
    stressed.costs.version = format!("{}-2x-stress", stressed.costs.version);
    let report = run_interval(dataset, strategy, &stressed, interval)?;
    Ok(vec![StressTestResult {
        label: "costs-and-slippage-2x".into(),
        cost_multiplier: 2.0,
        survived: report.metrics.excess_return_pct > 0.0 && report.metrics.total_return_pct > 0.0,
        metrics: report.metrics,
    }])
}

fn run_parameter_neighbors(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    base_config: &PortfolioBacktestConfig,
    interval: &DateInterval,
    promotion: &PromotionConfig,
) -> Result<ParameterStabilityResult> {
    let neighbors = parameter_neighbors(strategy.spec());
    let attempted_neighbors = neighbors.len();
    let mut excess = Vec::new();
    let mut ids = Vec::new();
    for spec in neighbors {
        let Ok(compiled) = CompiledStrategy::compile(spec) else {
            continue;
        };
        let report = run_interval(dataset, &compiled, base_config, interval)?;
        excess.push(report.metrics.excess_return_pct);
        ids.push(compiled.strategy_id().into());
    }
    excess.sort_by(f64::total_cmp);
    let positive_neighbors = excess.iter().filter(|value| **value > 0.0).count();
    let positive_fraction = if excess.is_empty() {
        0.0
    } else {
        positive_neighbors as f64 / excess.len() as f64
    };
    let worst = excess.first().copied().unwrap_or(0.0);
    let median = percentile(&excess, 0.5).unwrap_or(0.0);
    Ok(ParameterStabilityResult {
        attempted_neighbors,
        valid_neighbors: excess.len(),
        positive_neighbors,
        positive_fraction,
        worst_excess_return_pct: worst,
        median_excess_return_pct: median,
        stable: !excess.is_empty()
            && positive_fraction >= promotion.minimum_positive_neighbor_fraction,
        neighbor_strategy_ids: ids,
    })
}

pub fn parameter_neighbors(spec: &StrategySpec) -> Vec<StrategySpec> {
    let root = serde_json::to_value(spec).expect("StrategySpec is serializable");
    let mut paths = Vec::new();
    collect_parameter_paths(&root, String::new(), &mut paths);
    let base_id = strategy_id(spec);
    let mut seen = BTreeSet::new();
    let mut output = Vec::new();
    for (path, key) in paths.into_iter().take(12) {
        let original = root.pointer(&path).and_then(|value| value.as_f64());
        let Some(original) = original else {
            continue;
        };
        for factor in [0.9, 1.1] {
            let mut changed = root.clone();
            let adjusted = adjusted_parameter(&key, original, factor);
            if (adjusted - original).abs() < 1e-12 {
                continue;
            }
            if let Some(value) = changed.pointer_mut(&path) {
                *value = if matches!(
                    key.as_str(),
                    "period" | "fast_period" | "slow_period" | "signal_period" | "hold_days"
                ) {
                    serde_json::Value::from(adjusted as u64)
                } else {
                    serde_json::Value::from(adjusted)
                };
            }
            let Ok(mut neighbor) = serde_json::from_value::<StrategySpec>(changed) else {
                continue;
            };
            neighbor.metadata.generator = "parameter-neighborhood".into();
            neighbor.metadata.parent_strategy_id = Some(base_id.clone());
            let id = strategy_id(&neighbor);
            if seen.insert(id) {
                output.push(neighbor);
            }
        }
    }
    output
}

fn collect_parameter_paths(
    value: &serde_json::Value,
    path: String,
    output: &mut Vec<(String, String)>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object {
                let child_path = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                if matches!(
                    key.as_str(),
                    "period"
                        | "fast_period"
                        | "slow_period"
                        | "signal_period"
                        | "hold_days"
                        | "stop_loss_pct"
                        | "take_profit_pct"
                ) && child.is_number()
                {
                    output.push((child_path.clone(), key.clone()));
                }
                collect_parameter_paths(child, child_path, output);
            }
        }
        serde_json::Value::Array(array) => {
            for (index, child) in array.iter().enumerate() {
                collect_parameter_paths(child, format!("{path}/{index}"), output);
            }
        }
        _ => {}
    }
}

fn adjusted_parameter(key: &str, value: f64, factor: f64) -> f64 {
    match key {
        "period" | "fast_period" | "slow_period" | "signal_period" => {
            (value * factor).round().clamp(2.0, 250.0)
        }
        "hold_days" => (value * factor).round().clamp(1.0, 250.0),
        "stop_loss_pct" => (value * factor * 10.0).round().clamp(5.0, 300.0) / 10.0,
        "take_profit_pct" => (value * factor * 10.0).round().clamp(5.0, 1_000.0) / 10.0,
        _ => value,
    }
}

fn concentration(
    trades: &[PortfolioTrade],
    windows: &[WalkForwardWindowResult],
    industry_by_instrument: &BTreeMap<String, String>,
) -> ConcentrationResult {
    let mut by_instrument = BTreeMap::new();
    let mut by_industry = BTreeMap::new();
    let mut by_year = BTreeMap::new();
    for trade in trades {
        let instrument_key = trade.instrument.storage_key();
        *by_instrument.entry(instrument_key.clone()).or_insert(0.0) += trade.net_pnl;
        if let Some(industry) = industry_by_instrument.get(&instrument_key) {
            *by_industry.entry(industry.clone()).or_insert(0.0) += trade.net_pnl;
        }
        *by_year.entry(year_label(&trade.exit_date)).or_insert(0.0) += trade.net_pnl;
    }
    let by_window: BTreeMap<_, _> = windows
        .iter()
        .map(|window| {
            (
                format!("window-{:03}", window.ordinal),
                window.metrics.net_profit,
            )
        })
        .collect();
    ConcentrationResult {
        max_instrument_contribution_pct: max_positive_contribution(&by_instrument),
        max_industry_contribution_pct: (!by_industry.is_empty())
            .then(|| max_positive_contribution(&by_industry)),
        max_year_contribution_pct: max_positive_contribution(&by_year),
        max_window_contribution_pct: max_positive_contribution(&by_window),
        by_instrument_net_pnl: by_instrument,
        by_industry_net_pnl: by_industry,
        by_year_net_pnl: by_year,
        by_window_net_pnl: by_window,
    }
}

fn max_positive_contribution(values: &BTreeMap<String, f64>) -> f64 {
    let positive_total: f64 = values.values().filter(|value| **value > 0.0).sum();
    if positive_total <= 0.0 {
        return 100.0;
    }
    values
        .values()
        .copied()
        .filter(|value| *value > 0.0)
        .map(|value| value / positive_total * 100.0)
        .fold(0.0, f64::max)
}

fn block_bootstrap(
    report: &PortfolioBacktestReport,
    block_sessions: usize,
    samples: usize,
) -> Option<BootstrapInterval> {
    let returns: Vec<_> = report
        .daily_equity
        .windows(2)
        .filter_map(|window| {
            (window[0].total_equity > 0.0)
                .then_some(window[1].total_equity / window[0].total_equity - 1.0)
        })
        .collect();
    if returns.len() < 2 || samples == 0 {
        return None;
    }
    let block_sessions = block_sessions.clamp(1, returns.len());
    let seed = seed_from_id(&report.strategy_id);
    let mut state = seed;
    let max_start = returns.len() - block_sessions + 1;
    let mut means = Vec::with_capacity(samples);
    for _ in 0..samples {
        let mut sample = Vec::with_capacity(returns.len());
        while sample.len() < returns.len() {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let start = (state as usize) % max_start;
            for value in &returns[start..start + block_sessions] {
                if sample.len() == returns.len() {
                    break;
                }
                sample.push(*value);
            }
        }
        means.push(sample.iter().sum::<f64>() / sample.len() as f64 * 252.0 * 100.0);
    }
    means.sort_by(f64::total_cmp);
    Some(BootstrapInterval {
        samples,
        block_sessions,
        annualized_mean_return_low_pct: percentile(&means, 0.025)?,
        annualized_mean_return_high_pct: percentile(&means, 0.975)?,
        deterministic_seed: seed,
    })
}

pub fn classify_market_regimes(
    dataset: &FrozenDataset,
    calendar: &[String],
    high_volatility_threshold_pct: f64,
) -> BTreeMap<String, MarketRegime> {
    let mut bases = BTreeMap::new();
    let mut last = BTreeMap::new();
    let mut market_values = Vec::with_capacity(calendar.len());
    for date in calendar {
        for series in &dataset.series {
            if let Some(bar) = series.candles.iter().find(|bar| &bar.time == date)
                && bar.close.is_finite()
                && bar.close > 0.0
            {
                let key = series.instrument.storage_key();
                bases.entry(key.clone()).or_insert(bar.close);
                last.insert(key, bar.close);
            }
        }
        let ratios: Vec<_> = last
            .iter()
            .filter_map(|(key, close)| bases.get(key).map(|base| close / base))
            .collect();
        market_values.push(if ratios.is_empty() {
            1.0
        } else {
            ratios.iter().sum::<f64>() / ratios.len() as f64
        });
    }
    calendar
        .iter()
        .enumerate()
        .map(|(index, date)| {
            let regime = if index < 60 {
                MarketRegime::InsufficientHistory
            } else {
                let recent = &market_values[index - 20..=index];
                let returns: Vec<_> = recent
                    .windows(2)
                    .map(|window| window[1] / window[0] - 1.0)
                    .collect();
                let mean = returns.iter().sum::<f64>() / returns.len() as f64;
                let volatility = (returns
                    .iter()
                    .map(|value| (value - mean).powi(2))
                    .sum::<f64>()
                    / returns.len() as f64)
                    .sqrt()
                    * 252.0_f64.sqrt()
                    * 100.0;
                let momentum = (market_values[index] / market_values[index - 60] - 1.0) * 100.0;
                if volatility >= high_volatility_threshold_pct {
                    MarketRegime::HighVolatility
                } else if momentum >= 10.0 {
                    MarketRegime::Bull
                } else if momentum <= -10.0 {
                    MarketRegime::Bear
                } else {
                    MarketRegime::Sideways
                }
            };
            (date.clone(), regime)
        })
        .collect()
}

fn regime_metrics(
    report: &PortfolioBacktestReport,
    regimes: &BTreeMap<String, MarketRegime>,
    initial_cash: f64,
) -> BTreeMap<MarketRegime, PortfolioMetrics> {
    let mut grouped: BTreeMap<MarketRegime, Vec<PortfolioTrade>> = BTreeMap::new();
    for trade in &report.trades {
        if let Some(regime) = regimes.get(&trade.signal_entry_date) {
            grouped.entry(*regime).or_default().push(trade.clone());
        }
    }
    grouped
        .into_iter()
        .map(|(regime, trades)| (regime, calculate_trade_metrics(&trades, initial_cash)))
        .collect()
}

fn fixed_intervals(
    calendar: &[String],
    split: &FixedSplitConfig,
) -> Result<(DateInterval, DateInterval, DateInterval, usize)> {
    if calendar.len() < 5 {
        bail!("at least five sessions are required for fixed split");
    }
    let training_len = calendar.len() * usize::from(split.training_pct) / 100;
    let validation_len = calendar.len() * usize::from(split.validation_pct) / 100;
    let test_len = calendar.len().saturating_sub(training_len + validation_len);
    if training_len == 0 || validation_len == 0 || test_len == 0 {
        bail!("fixed split produced an empty training, validation, or test interval");
    }
    let training_end = training_len - 1;
    let validation_start = training_len;
    let validation_end = training_len + validation_len - 1;
    let test_start = validation_end + 1;
    Ok((
        DateInterval {
            start: calendar[0].clone(),
            end: calendar[training_end].clone(),
        },
        DateInterval {
            start: calendar[validation_start].clone(),
            end: calendar[validation_end].clone(),
        },
        DateInterval {
            start: calendar[test_start].clone(),
            end: calendar.last().cloned().context("empty calendar")?,
        },
        validation_end,
    ))
}

fn dataset_calendar(dataset: &FrozenDataset) -> Vec<String> {
    dataset
        .series
        .iter()
        .flat_map(|series| series.candles.iter().map(|bar| bar.time.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn validate_robustness_config(config: &RobustnessConfig) -> Result<()> {
    if config.fixed_split.training_pct == 0
        || config.fixed_split.validation_pct == 0
        || u16::from(config.fixed_split.training_pct) + u16::from(config.fixed_split.validation_pct)
            >= 100
    {
        bail!("fixed split must leave non-empty training, validation, and test sets");
    }
    if config.walk_forward.training_sessions == 0
        || config.walk_forward.validation_sessions == 0
        || config.walk_forward.step_sessions == 0
    {
        bail!("walk-forward sizes and step must be positive");
    }
    Ok(())
}

fn gate(
    code: GateCode,
    passed: bool,
    observed: impl Into<String>,
    required: impl Into<String>,
    explanation: impl Into<String>,
) -> GateResult {
    GateResult {
        code,
        passed,
        observed: observed.into(),
        required: required.into(),
        explanation: explanation.into(),
    }
}

fn percentile(values: &[f64], quantile: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let index = ((values.len() - 1) as f64 * quantile.clamp(0.0, 1.0)).round() as usize;
    values.get(index).copied()
}

fn seed_from_id(value: &str) -> u64 {
    value
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        })
}

fn year_label(date: &str) -> String {
    date.get(..4).unwrap_or(date).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::backtest::CostModel;
    use crate::domain::dataset::{
        DataQualityIssue, DatasetManifest, FrozenSeries, dataset_content_sha256,
    };
    use crate::domain::market::{Adjustment, AssetType, CandleRecord, InstrumentId, Market};
    use crate::domain::strategy::{
        CompareOperator, Comparison, ExitRule, Expression, LocalTemplate, ValueExpression,
    };

    fn fixture(instruments: usize, sessions: usize) -> FrozenDataset {
        let series: Vec<_> = (0..instruments)
            .map(|symbol| FrozenSeries {
                instrument: InstrumentId {
                    market: Market::AShare,
                    asset_type: AssetType::Stock,
                    code: format!("{symbol:06}"),
                },
                source: "robustness-fixture-v1".into(),
                adjustment: Adjustment::Forward,
                candles: (0..sessions)
                    .map(|index| {
                        let close = 20.0
                            + symbol as f64 * 0.01
                            + index as f64 * 0.005
                            + (index as f64 / 10.0).sin();
                        CandleRecord {
                            time: format!("d{index:04}"),
                            open: close * 0.999,
                            high: close * 1.01,
                            low: close * 0.99,
                            close,
                            volume: 100_000 + index as u64,
                        }
                    })
                    .collect(),
            })
            .collect();
        let hash = dataset_content_sha256(Market::AShare, &series);
        let id = format!("dataset-sha256:{hash}");
        FrozenDataset {
            manifest: DatasetManifest {
                id,
                created_at: "2026-01-01T00:00:00Z".into(),
                market: Market::AShare,
                adjustment: Adjustment::Forward,
                source_versions: vec!["robustness-fixture-v1".into()],
                instruments: series.iter().map(|item| item.instrument.clone()).collect(),
                interval: DateInterval {
                    start: "d0000".into(),
                    end: format!("d{:04}", sessions - 1),
                },
                content_sha256: hash,
                known_biases: vec![],
                quality_issues: Vec::<DataQualityIssue>::new(),
            },
            series,
        }
    }

    fn strategy(dataset_id: &str) -> CompiledStrategy {
        let mut spec = LocalTemplate::NDayHighBreakout.build(dataset_id);
        spec.entry = Expression::Compare {
            compare: Comparison {
                left: ValueExpression::Constant { constant: 2.0 },
                op: CompareOperator::Above,
                right: ValueExpression::Constant { constant: 1.0 },
            },
        };
        spec.exit = ExitRule::HoldDays { hold_days: 5 };
        spec.position.size_pct = 4.0;
        spec.position.max_positions = 25;
        CompiledStrategy::compile(spec).unwrap()
    }

    fn base_config() -> PortfolioBacktestConfig {
        PortfolioBacktestConfig {
            initial_cash: 1_000_000.0,
            costs: CostModel {
                commission_bps_each_side: 0.0,
                minimum_commission: 0.0,
                sell_tax_bps: 0.0,
                slippage_bps_each_side: 0.0,
                other_fees_bps_each_side: 0.0,
                version: "zero-cost-fixture".into(),
            },
            ..PortfolioBacktestConfig::default()
        }
    }

    fn robustness_config() -> RobustnessConfig {
        RobustnessConfig {
            walk_forward: WalkForwardConfig {
                training_sessions: 80,
                validation_sessions: 40,
                step_sessions: 40,
            },
            bootstrap_samples: 100,
            bootstrap_block_sessions: 10,
            ..RobustnessConfig::default()
        }
    }

    #[test]
    fn robustness_pipeline_keeps_sealed_test_unseen_until_explicit_consumption() {
        let dataset = fixture(25, 400);
        let strategy = strategy(&dataset.manifest.id);
        let report =
            evaluate_robustness(&dataset, &strategy, &base_config(), &robustness_config(), 5)
                .unwrap();

        assert_eq!(report.strategy_attempts, 5);
        assert!(report.validation_interval.end < report.sealed_test_interval.start);
        assert!(report.walk_forward.len() >= 3);
        assert_eq!(report.stress_tests[0].cost_multiplier, 2.0);
        assert!(report.parameter_attempts >= 1);
        assert!(report.bootstrap.is_some());

        let consumed = consume_sealed_test(
            &dataset,
            &strategy,
            &base_config(),
            &report,
            None,
            "2026-02-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(consumed.interval, report.sealed_test_interval);
        assert!(
            consume_sealed_test(
                &dataset,
                &strategy,
                &base_config(),
                &report,
                Some(&consumed.consumed_at),
                "2026-02-02T00:00:00Z",
            )
            .is_err()
        );
    }

    #[test]
    fn market_regime_classification_is_point_in_time() {
        let original = fixture(2, 140);
        let calendar = dataset_calendar(&original);
        let original_regimes = classify_market_regimes(&original, &calendar, 30.0);
        let mut changed = original.clone();
        for series in &mut changed.series {
            for bar in &mut series.candles[110..] {
                bar.close *= 10.0;
                bar.high = bar.close * 1.01;
                bar.low = bar.close * 0.99;
                bar.open = bar.close;
            }
        }
        let changed_regimes = classify_market_regimes(&changed, &calendar, 30.0);
        for date in &calendar[..110] {
            assert_eq!(original_regimes.get(date), changed_regimes.get(date));
        }
    }

    #[test]
    fn parameter_neighbors_are_new_valid_versions() {
        let dataset = fixture(1, 100);
        let strategy = strategy(&dataset.manifest.id);
        let neighbors = parameter_neighbors(strategy.spec());
        assert!(!neighbors.is_empty());
        for neighbor in neighbors {
            let compiled = CompiledStrategy::compile(neighbor.clone()).unwrap();
            assert_ne!(compiled.strategy_id(), strategy.strategy_id());
            assert_eq!(
                neighbor.metadata.parent_strategy_id.as_deref(),
                Some(strategy.strategy_id())
            );
        }
    }

    #[test]
    fn deterministic_promotion_gates_cannot_be_overridden_by_summary_text() {
        let dataset = fixture(25, 200);
        let strategy = strategy(&dataset.manifest.id);
        let interval = DateInterval {
            start: "d0040".into(),
            end: "d0159".into(),
        };
        let mut validation = run_interval(&dataset, &strategy, &base_config(), &interval).unwrap();
        validation.trades = (0..60).map(sample_trade).collect();
        validation.metrics.trade_count = 60;
        validation.metrics.excess_return_pct = 5.0;
        validation.metrics.total_return_pct = 7.0;
        validation.metrics.max_drawdown_pct = -10.0;
        let windows: Vec<_> = (0..3)
            .map(|ordinal| WalkForwardWindowResult {
                ordinal,
                training_interval: interval.clone(),
                validation_interval: interval.clone(),
                metrics: PortfolioMetrics {
                    excess_return_pct: 2.0,
                    ..PortfolioMetrics::default()
                },
                trade_count: 20,
                covered_instruments: 20,
            })
            .collect();
        let concentration = concentration(&validation.trades, &windows, &BTreeMap::new());
        let stress = vec![StressTestResult {
            label: "2x".into(),
            cost_multiplier: 2.0,
            metrics: PortfolioMetrics::default(),
            survived: true,
        }];
        let stability = ParameterStabilityResult {
            attempted_neighbors: 10,
            valid_neighbors: 10,
            positive_neighbors: 9,
            positive_fraction: 0.9,
            worst_excess_return_pct: 0.1,
            median_excess_return_pct: 2.0,
            stable: true,
            neighbor_strategy_ids: vec![],
        };
        let config = PromotionConfig::default();

        let accepted = promotion_decision(
            &validation,
            &windows,
            &stress,
            &stability,
            &concentration,
            &config,
        );
        assert_eq!(accepted.conclusion, PromotionConclusion::PaperCandidate);
        assert_eq!(accepted.evidence_grade, EvidenceGrade::MultiPeriodStable);

        let rejected = promotion_decision(
            &validation,
            &windows,
            &[StressTestResult {
                survived: false,
                ..stress[0].clone()
            }],
            &stability,
            &concentration,
            &config,
        );
        assert_eq!(rejected.conclusion, PromotionConclusion::Rejected);
        assert!(
            rejected
                .gates
                .iter()
                .any(|gate| gate.code == GateCode::CostStress && !gate.passed)
        );
    }

    #[test]
    fn block_bootstrap_is_deterministic_for_same_report() {
        let dataset = fixture(1, 100);
        let strategy = strategy(&dataset.manifest.id);
        let report = run_portfolio_backtest(&dataset, &strategy, &base_config()).unwrap();
        assert_eq!(
            block_bootstrap(&report, 5, 100),
            block_bootstrap(&report, 5, 100)
        );
    }

    fn sample_trade(index: usize) -> PortfolioTrade {
        let year = 2020 + index % 4;
        PortfolioTrade {
            instrument: InstrumentId {
                market: Market::AShare,
                asset_type: AssetType::Stock,
                code: format!("{:06}", index % 20),
            },
            signal_entry_date: format!("{year}-01-01"),
            entry_date: format!("{year}-01-02"),
            signal_exit_date: format!("{year}-01-10"),
            exit_date: format!("{year}-01-11"),
            quantity: 100,
            entry_price: 10.0,
            exit_price: 11.05,
            gross_pnl: 105.0,
            total_cost: 5.0,
            net_pnl: 100.0,
            net_return_pct: 10.0,
            holding_sessions: 8,
        }
    }
}
