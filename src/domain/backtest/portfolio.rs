use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};

use crate::domain::dataset::{FrozenDataset, FrozenSeries, validate_series};
use crate::domain::market::{CandleRecord, InstrumentId};
use crate::domain::strategy::{CompiledStrategy, PositionContext};

use super::config::{BenchmarkDefinition, PortfolioBacktestConfig};
use super::metrics::{calculate_metrics, metrics_by_instrument, metrics_by_year};
use super::report::{
    InstrumentFailure, OpenPositionSnapshot, OrderRejectionReason, OrderSide,
    PortfolioBacktestReport, PortfolioEquityPoint, PortfolioTrade, RejectedOrder,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunControl {
    Continue,
    Cancel,
}

#[derive(Debug)]
struct PreparedSeries {
    series: FrozenSeries,
    by_date: BTreeMap<String, usize>,
    entry_signals: Vec<bool>,
}

#[derive(Debug, Clone)]
struct PendingOrder {
    series_index: usize,
    instrument: InstrumentId,
    side: OrderSide,
    signal_date: String,
    created_session: usize,
}

#[derive(Debug, Clone)]
struct Position {
    instrument: InstrumentId,
    series_index: usize,
    quantity: u64,
    entry_signal_date: String,
    entry_date: String,
    entry_market_price: f64,
    entry_fill_price: f64,
    entry_cash_out: f64,
    entry_cost: f64,
    last_price: f64,
    holding_sessions: usize,
}

pub fn run_portfolio_backtest(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    config: &PortfolioBacktestConfig,
) -> Result<PortfolioBacktestReport> {
    run_portfolio_backtest_with_control(dataset, strategy, config, |_, _| RunControl::Continue)
}

pub fn run_portfolio_backtest_with_control(
    dataset: &FrozenDataset,
    strategy: &CompiledStrategy,
    config: &PortfolioBacktestConfig,
    mut control: impl FnMut(usize, usize) -> RunControl,
) -> Result<PortfolioBacktestReport> {
    validate_config(config)?;
    if strategy.spec().universe.id() != dataset.manifest.id {
        bail!("strategy universe does not match frozen dataset");
    }

    let mut failures = Vec::new();
    let mut prepared = Vec::new();
    let mut ordered_series = dataset.series.to_vec();
    if let Some(interval) = &config.evaluation_interval {
        for series in &mut ordered_series {
            series
                .candles
                .retain(|bar| bar.time >= interval.start && bar.time <= interval.end);
        }
    }
    ordered_series.sort_by(|left, right| left.instrument.cmp(&right.instrument));
    for series in ordered_series {
        let issues = validate_series(&series);
        if !issues.is_empty() {
            failures.push(InstrumentFailure {
                instrument: series.instrument.clone(),
                message: serde_json::to_string(&issues)?,
            });
            continue;
        }
        let by_date = series
            .candles
            .iter()
            .enumerate()
            .map(|(index, candle)| (candle.time.clone(), index))
            .collect();
        let entry_signals = strategy.entry_signals(&series.candles);
        prepared.push(PreparedSeries {
            series,
            by_date,
            entry_signals,
        });
    }
    if prepared.is_empty() {
        bail!("frozen dataset contains no valid tradable series");
    }

    let calendar: Vec<_> = prepared
        .iter()
        .flat_map(|item| item.series.candles.iter().map(|bar| bar.time.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let benchmark = benchmark_curve(&prepared, &calendar, &config.benchmark)?;
    let mut cash = config.initial_cash;
    let mut positions: BTreeMap<String, Position> = BTreeMap::new();
    let mut pending = Vec::new();
    let mut rejected_orders = Vec::new();
    let mut trades = Vec::new();
    let mut daily_equity = Vec::with_capacity(calendar.len());
    let mut cancelled = false;

    for (session, date) in calendar.iter().enumerate() {
        pending.sort_by(|left: &PendingOrder, right| {
            order_priority(left.side)
                .cmp(&order_priority(right.side))
                .then_with(|| left.instrument.cmp(&right.instrument))
                .then_with(|| left.signal_date.cmp(&right.signal_date))
        });
        let mut delayed = Vec::new();
        for order in pending.drain(..) {
            if order.created_session >= session {
                delayed.push(order);
                continue;
            }
            let prepared_series = &prepared[order.series_index];
            let bar = prepared_series
                .by_date
                .get(date)
                .and_then(|index| prepared_series.series.candles.get(*index));
            let unavailable_reason = match bar {
                None => Some(OrderRejectionReason::SuspendedOrNoVolume),
                Some(bar) if !valid_price(bar.open) => Some(OrderRejectionReason::InvalidOpen),
                Some(bar) if bar.volume == 0 => Some(OrderRejectionReason::SuspendedOrNoVolume),
                Some(_) => None,
            };
            if let Some(reason) = unavailable_reason {
                if session.saturating_sub(order.created_session)
                    >= usize::from(config.max_order_delay_sessions)
                {
                    rejected_orders.push(rejection(
                        &order,
                        date,
                        OrderRejectionReason::DelayExpired,
                        format!("order remained unavailable after {reason:?}"),
                    ));
                } else {
                    delayed.push(order);
                }
                continue;
            }
            let bar = bar.expect("availability check requires a bar");
            match order.side {
                OrderSide::Sell => execute_sell(
                    &order,
                    date,
                    bar,
                    config,
                    &mut cash,
                    &mut positions,
                    &mut trades,
                    &mut rejected_orders,
                ),
                OrderSide::Buy => execute_buy(
                    &order,
                    date,
                    bar,
                    strategy,
                    config,
                    &mut cash,
                    &mut positions,
                    &mut rejected_orders,
                ),
            }
        }
        pending = delayed;

        for position in positions.values_mut() {
            if let Some(index) = prepared[position.series_index].by_date.get(date) {
                let bar = &prepared[position.series_index].series.candles[*index];
                if valid_price(bar.close) {
                    position.last_price = bar.close;
                }
                position.holding_sessions += 1;
            }
        }

        let position_keys: Vec<_> = positions.keys().cloned().collect();
        for key in position_keys {
            let Some(position) = positions.get(&key) else {
                continue;
            };
            if pending.iter().any(|order| {
                order.side == OrderSide::Sell && order.instrument == position.instrument
            }) {
                continue;
            }
            let prepared_series = &prepared[position.series_index];
            let Some(index) = prepared_series.by_date.get(date).copied() else {
                continue;
            };
            if strategy.should_exit(
                &prepared_series.series.candles[..=index],
                index,
                PositionContext {
                    entry_price: position.entry_fill_price,
                    holding_days: position.holding_sessions,
                },
            ) {
                pending.push(PendingOrder {
                    series_index: position.series_index,
                    instrument: position.instrument.clone(),
                    side: OrderSide::Sell,
                    signal_date: date.clone(),
                    created_session: session,
                });
            }
        }

        for (series_index, item) in prepared.iter().enumerate() {
            let key = item.series.instrument.storage_key();
            if positions.contains_key(&key)
                || pending
                    .iter()
                    .any(|order| order.instrument == item.series.instrument)
            {
                continue;
            }
            let Some(index) = item.by_date.get(date).copied() else {
                continue;
            };
            if item.entry_signals.get(index).copied().unwrap_or(false) {
                pending.push(PendingOrder {
                    series_index,
                    instrument: item.series.instrument.clone(),
                    side: OrderSide::Buy,
                    signal_date: date.clone(),
                    created_session: session,
                });
            }
        }

        let positions_value = positions
            .values()
            .map(|position| position.last_price * position.quantity as f64)
            .sum::<f64>();
        let total_equity = cash + positions_value;
        daily_equity.push(PortfolioEquityPoint {
            date: date.clone(),
            cash,
            positions_value,
            total_equity,
            benchmark_equity: benchmark[session],
            open_positions: positions.len(),
            exposure_pct: if total_equity > 0.0 {
                positions_value / total_equity * 100.0
            } else {
                0.0
            },
        });
        if control(session + 1, calendar.len()) == RunControl::Cancel {
            cancelled = true;
            break;
        }
    }

    let completed_sessions = daily_equity.len();
    let metrics = calculate_metrics(
        &daily_equity,
        &trades,
        config.initial_cash,
        config.annual_trading_days,
    );
    let metrics_by_year = metrics_by_year(&daily_equity, &trades, config.annual_trading_days);
    let metrics_by_instrument = metrics_by_instrument(&trades, config.initial_cash);
    let open_positions = positions
        .into_values()
        .map(|position| OpenPositionSnapshot {
            instrument: position.instrument,
            quantity: position.quantity,
            entry_date: position.entry_date,
            entry_price: position.entry_fill_price,
            last_price: position.last_price,
            unrealized_pnl: position.last_price * position.quantity as f64
                - position.entry_cash_out,
            holding_sessions: position.holding_sessions,
        })
        .collect();
    let mut known_biases = dataset.manifest.known_biases.clone();
    known_biases
        .push("涨跌停排队无法由日线精确判断；零成交量按不可成交并执行确定性延迟规则".into());
    Ok(PortfolioBacktestReport {
        report_version: config.report_version.clone(),
        dataset_id: dataset.manifest.id.clone(),
        dataset_content_sha256: dataset.manifest.content_sha256.clone(),
        strategy_id: strategy.strategy_id().into(),
        strategy_schema_version: strategy.spec().schema_version,
        config: config.clone(),
        config_hash: config.stable_hash(),
        execution_rule:
            "close signal; next valid session open; sells before buys; stable instrument order"
                .into(),
        known_biases,
        instrument_failures: failures,
        rejected_orders,
        trades,
        open_positions,
        daily_equity,
        metrics,
        metrics_by_year,
        metrics_by_instrument,
        completed_sessions,
        cancelled,
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_buy(
    order: &PendingOrder,
    date: &str,
    bar: &CandleRecord,
    strategy: &CompiledStrategy,
    config: &PortfolioBacktestConfig,
    cash: &mut f64,
    positions: &mut BTreeMap<String, Position>,
    rejected: &mut Vec<RejectedOrder>,
) {
    let key = order.instrument.storage_key();
    if positions.contains_key(&key) {
        rejected.push(rejection(
            order,
            date,
            OrderRejectionReason::AlreadyHeld,
            "pyramiding is disabled for the first release",
        ));
        return;
    }
    if positions.len() >= usize::from(strategy.spec().position.max_positions) {
        rejected.push(rejection(
            order,
            date,
            OrderRejectionReason::MaximumPositions,
            "maximum open positions reached",
        ));
        return;
    }
    let allocation = (*cash).min(config.initial_cash * strategy.spec().position.size_pct / 100.0);
    let fill_price = bar.open * (1.0 + config.costs.slippage_bps_each_side / 10_000.0);
    let mut quantity = ((allocation / fill_price / 100.0).floor() as u64) * 100;
    while quantity > 0 && buy_cash_out(bar.open, fill_price, quantity, config).0 > allocation {
        quantity -= 100;
    }
    if quantity == 0 {
        rejected.push(rejection(
            order,
            date,
            OrderRejectionReason::BelowBoardLot,
            "available allocation cannot buy one 100-share board lot",
        ));
        return;
    }
    let (cash_out, total_cost) = buy_cash_out(bar.open, fill_price, quantity, config);
    if cash_out > *cash + 1e-8 {
        rejected.push(rejection(
            order,
            date,
            OrderRejectionReason::InsufficientCash,
            "cash is insufficient after commission and fees",
        ));
        return;
    }
    *cash -= cash_out;
    positions.insert(
        key,
        Position {
            instrument: order.instrument.clone(),
            series_index: order.series_index,
            quantity,
            entry_signal_date: order.signal_date.clone(),
            entry_date: date.into(),
            entry_market_price: bar.open,
            entry_fill_price: fill_price,
            entry_cash_out: cash_out,
            entry_cost: total_cost,
            last_price: bar.close,
            holding_sessions: 0,
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn execute_sell(
    order: &PendingOrder,
    date: &str,
    bar: &CandleRecord,
    config: &PortfolioBacktestConfig,
    cash: &mut f64,
    positions: &mut BTreeMap<String, Position>,
    trades: &mut Vec<PortfolioTrade>,
    rejected: &mut Vec<RejectedOrder>,
) {
    let key = order.instrument.storage_key();
    let Some(position) = positions.remove(&key) else {
        rejected.push(rejection(
            order,
            date,
            OrderRejectionReason::NoPosition,
            "sell order has no matching open position",
        ));
        return;
    };
    let fill_price = bar.open * (1.0 - config.costs.slippage_bps_each_side / 10_000.0);
    let raw_proceeds = fill_price * position.quantity as f64;
    let commission = commission(raw_proceeds, config);
    let tax = raw_proceeds * config.costs.sell_tax_bps / 10_000.0;
    let other_fees = raw_proceeds * config.costs.other_fees_bps_each_side / 10_000.0;
    let proceeds = raw_proceeds - commission - tax - other_fees;
    *cash += proceeds;
    let gross_pnl = (bar.open - position.entry_market_price) * position.quantity as f64;
    let net_pnl = proceeds - position.entry_cash_out;
    let exit_slippage = (bar.open - fill_price) * position.quantity as f64;
    let total_cost = position.entry_cost + commission + tax + other_fees + exit_slippage;
    trades.push(PortfolioTrade {
        instrument: position.instrument,
        signal_entry_date: position.entry_signal_date,
        entry_date: position.entry_date,
        signal_exit_date: order.signal_date.clone(),
        exit_date: date.into(),
        quantity: position.quantity,
        entry_price: position.entry_fill_price,
        exit_price: fill_price,
        gross_pnl,
        total_cost,
        net_pnl,
        net_return_pct: if position.entry_cash_out > 0.0 {
            net_pnl / position.entry_cash_out * 100.0
        } else {
            0.0
        },
        holding_sessions: position.holding_sessions,
    });
}

fn buy_cash_out(
    market_price: f64,
    fill_price: f64,
    quantity: u64,
    config: &PortfolioBacktestConfig,
) -> (f64, f64) {
    let quantity = quantity as f64;
    let notional = fill_price * quantity;
    let commission = commission(notional, config);
    let other_fees = notional * config.costs.other_fees_bps_each_side / 10_000.0;
    let slippage = (fill_price - market_price) * quantity;
    (
        notional + commission + other_fees,
        commission + other_fees + slippage,
    )
}

fn commission(notional: f64, config: &PortfolioBacktestConfig) -> f64 {
    (notional * config.costs.commission_bps_each_side / 10_000.0)
        .max(config.costs.minimum_commission)
}

fn benchmark_curve(
    prepared: &[PreparedSeries],
    calendar: &[String],
    benchmark: &BenchmarkDefinition,
) -> Result<Vec<f64>> {
    match benchmark {
        BenchmarkDefinition::EqualWeightedUniverse => {
            let mut bases: BTreeMap<String, f64> = BTreeMap::new();
            let mut last: BTreeMap<String, f64> = BTreeMap::new();
            Ok(calendar
                .iter()
                .map(|date| {
                    for item in prepared {
                        if let Some(index) = item.by_date.get(date) {
                            let close = item.series.candles[*index].close;
                            if valid_price(close) {
                                let key = item.series.instrument.storage_key();
                                bases.entry(key.clone()).or_insert(close);
                                last.insert(key, close);
                            }
                        }
                    }
                    let ratios: Vec<_> = last
                        .iter()
                        .filter_map(|(key, close)| bases.get(key).map(|base| close / base))
                        .collect();
                    if ratios.is_empty() {
                        1.0
                    } else {
                        ratios.iter().sum::<f64>() / ratios.len() as f64
                    }
                })
                .collect())
        }
        BenchmarkDefinition::FrozenInstrument { instrument_key } => {
            let item = prepared
                .iter()
                .find(|item| item.series.instrument.storage_key() == *instrument_key)
                .ok_or_else(|| anyhow::anyhow!("benchmark instrument is not in frozen dataset"))?;
            let mut base = None;
            let mut last = 1.0;
            Ok(calendar
                .iter()
                .map(|date| {
                    if let Some(index) = item.by_date.get(date) {
                        let close = item.series.candles[*index].close;
                        if valid_price(close) {
                            let base = *base.get_or_insert(close);
                            last = close / base;
                        }
                    }
                    last
                })
                .collect())
        }
    }
}

fn rejection(
    order: &PendingOrder,
    attempted_date: &str,
    reason: OrderRejectionReason,
    detail: impl Into<String>,
) -> RejectedOrder {
    RejectedOrder {
        instrument: order.instrument.clone(),
        side: order.side,
        signal_date: order.signal_date.clone(),
        attempted_date: attempted_date.into(),
        reason,
        detail: detail.into(),
    }
}

const fn order_priority(side: OrderSide) -> u8 {
    match side {
        OrderSide::Sell => 0,
        OrderSide::Buy => 1,
    }
}

fn valid_price(price: f64) -> bool {
    price.is_finite() && price > 0.0
}

fn validate_config(config: &PortfolioBacktestConfig) -> Result<()> {
    if !config.initial_cash.is_finite() || config.initial_cash <= 0.0 {
        bail!("initial cash must be finite and positive");
    }
    if config.annual_trading_days == 0 {
        bail!("annual trading days must be positive");
    }
    if !config.costs.minimum_commission.is_finite() || config.costs.minimum_commission < 0.0 {
        bail!("minimum commission must be finite and non-negative");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dataset::{
        DataQualityIssue, DatasetManifest, DateInterval, dataset_content_sha256,
    };
    use crate::domain::market::{Adjustment, AssetType, Market};
    use crate::domain::strategy::{
        CompareOperator, Comparison, ExitRule, Expression, LocalTemplate, ValueExpression,
    };

    fn series(code: &str, opens: &[f64]) -> FrozenSeries {
        FrozenSeries {
            instrument: InstrumentId {
                market: Market::AShare,
                asset_type: AssetType::Stock,
                code: code.into(),
            },
            source: "fixture-v1".into(),
            adjustment: Adjustment::Forward,
            candles: opens
                .iter()
                .enumerate()
                .map(|(index, open)| CandleRecord {
                    time: format!("2026-01-{:02}", index + 1),
                    open: *open,
                    high: open + 1.0,
                    low: open - 1.0,
                    close: *open,
                    volume: 10_000,
                })
                .collect(),
        }
    }

    fn dataset(mut series: Vec<FrozenSeries>) -> FrozenDataset {
        series.sort_by(|left, right| left.instrument.cmp(&right.instrument));
        let hash = dataset_content_sha256(Market::AShare, &series);
        let id = format!("dataset-sha256:{hash}");
        FrozenDataset {
            manifest: DatasetManifest {
                id,
                created_at: "2026-02-01T00:00:00Z".into(),
                market: Market::AShare,
                adjustment: Adjustment::Forward,
                source_versions: vec!["fixture-v1".into()],
                instruments: series.iter().map(|item| item.instrument.clone()).collect(),
                interval: DateInterval {
                    start: "2026-01-01".into(),
                    end: "2026-01-06".into(),
                },
                content_sha256: hash,
                known_biases: vec!["fixture bias".into()],
                quality_issues: Vec::<DataQualityIssue>::new(),
            },
            series,
        }
    }

    fn always_enter_strategy(dataset_id: &str) -> CompiledStrategy {
        let mut spec = LocalTemplate::NDayHighBreakout.build(dataset_id);
        spec.entry = Expression::Compare {
            compare: Comparison {
                left: ValueExpression::Constant { constant: 2.0 },
                op: CompareOperator::Above,
                right: ValueExpression::Constant { constant: 1.0 },
            },
        };
        spec.exit = ExitRule::HoldDays { hold_days: 2 };
        spec.position.size_pct = 50.0;
        spec.position.max_positions = 2;
        CompiledStrategy::compile(spec).unwrap()
    }

    fn config() -> PortfolioBacktestConfig {
        PortfolioBacktestConfig {
            initial_cash: 100_000.0,
            ..PortfolioBacktestConfig::default()
        }
    }

    #[test]
    fn multi_instrument_orders_use_next_open_board_lots_cash_and_sided_costs() {
        let fixture = dataset(vec![
            series("600002", &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0]),
            series("600001", &[20.0, 21.0, 22.0, 23.0, 24.0, 25.0]),
        ]);
        let strategy = always_enter_strategy(&fixture.manifest.id);
        let report = run_portfolio_backtest(&fixture, &strategy, &config()).unwrap();

        assert_eq!(report.trades.len(), 2);
        assert!(report.trades.iter().all(|trade| trade.quantity % 100 == 0));
        assert!(
            report
                .trades
                .iter()
                .all(|trade| trade.entry_date == "2026-01-02")
        );
        assert!(
            report
                .trades
                .iter()
                .all(|trade| trade.exit_date == "2026-01-04")
        );
        assert!(report.trades.iter().all(|trade| trade.total_cost >= 10.0));
        assert!(report.daily_equity.iter().all(|point| point.cash >= -1e-8));
        assert_eq!(report.metrics.trade_count, 2);
        assert!(report.metrics.total_cost > 0.0);
    }

    #[test]
    fn suspended_open_is_delayed_deterministically_and_failure_is_isolated() {
        let mut suspended = series("600001", &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0]);
        suspended.candles[1].volume = 0;
        let mut invalid = series("600099", &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0]);
        invalid.candles[2].high = 1.0;
        let fixture = dataset(vec![suspended, invalid]);
        let strategy = always_enter_strategy(&fixture.manifest.id);
        let report = run_portfolio_backtest(&fixture, &strategy, &config()).unwrap();

        assert_eq!(report.instrument_failures.len(), 1);
        assert!(
            report
                .trades
                .iter()
                .all(|trade| trade.entry_date == "2026-01-03")
        );
        assert!(!report.cancelled);
    }

    #[test]
    fn same_input_has_stable_order_and_identical_report() {
        let fixture = dataset(vec![
            series("600002", &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0]),
            series("600001", &[20.0, 21.0, 22.0, 23.0, 24.0, 25.0]),
        ]);
        let strategy = always_enter_strategy(&fixture.manifest.id);
        let first = run_portfolio_backtest(&fixture, &strategy, &config()).unwrap();
        let second = run_portfolio_backtest(&fixture, &strategy, &config()).unwrap();
        let mut shuffled = fixture.clone();
        shuffled.series.reverse();
        let reordered = run_portfolio_backtest(&shuffled, &strategy, &config()).unwrap();

        assert_eq!(first, second);
        assert_eq!(first, reordered);
        assert!(first.trades[0].instrument < first.trades[1].instrument);
    }

    #[test]
    fn increasing_costs_cannot_improve_net_return_and_removing_profit_cannot_improve_net_profit() {
        let fixture = dataset(vec![series(
            "600001",
            &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0],
        )]);
        let strategy = always_enter_strategy(&fixture.manifest.id);
        let baseline = run_portfolio_backtest(&fixture, &strategy, &config()).unwrap();
        let mut expensive = config();
        expensive.costs.commission_bps_each_side *= 4.0;
        expensive.costs.minimum_commission *= 4.0;
        expensive.costs.sell_tax_bps *= 4.0;
        expensive.costs.slippage_bps_each_side *= 4.0;
        let stressed = run_portfolio_backtest(&fixture, &strategy, &expensive).unwrap();
        assert!(stressed.metrics.net_profit <= baseline.metrics.net_profit + 1e-9);
        assert!(stressed.metrics.total_return_pct <= baseline.metrics.total_return_pct + 1e-9);

        let mut without_one_profit = baseline.trades.clone();
        let profitable = without_one_profit
            .iter()
            .position(|trade| trade.net_pnl > 0.0)
            .expect("rising fixture produces a profitable trade");
        without_one_profit.remove(profitable);
        let reduced = crate::domain::backtest::metrics::calculate_trade_metrics(
            &without_one_profit,
            baseline.config.initial_cash,
        );
        assert!(reduced.net_profit <= baseline.metrics.net_profit + 1e-9);
    }

    #[test]
    fn cancellation_returns_consistent_partial_report() {
        let fixture = dataset(vec![series(
            "600001",
            &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0],
        )]);
        let strategy = always_enter_strategy(&fixture.manifest.id);
        let report =
            run_portfolio_backtest_with_control(&fixture, &strategy, &config(), |completed, _| {
                if completed >= 2 {
                    RunControl::Cancel
                } else {
                    RunControl::Continue
                }
            })
            .unwrap();

        assert!(report.cancelled);
        assert_eq!(report.completed_sessions, 2);
        assert_eq!(report.daily_equity.len(), 2);
    }
}
