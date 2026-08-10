use std::collections::BTreeMap;

use super::report::{PortfolioEquityPoint, PortfolioMetrics, PortfolioTrade};

pub fn calculate_metrics(
    equity: &[PortfolioEquityPoint],
    trades: &[PortfolioTrade],
    initial_cash: f64,
    annual_trading_days: u16,
) -> PortfolioMetrics {
    let mut metrics = calculate_trade_metrics(trades, initial_cash);
    if equity.is_empty() || initial_cash <= 0.0 {
        return metrics;
    }
    let ending_equity = equity
        .last()
        .map_or(initial_cash, |point| point.total_equity);
    metrics.total_return_pct = (ending_equity / initial_cash - 1.0) * 100.0;
    let years = equity.len() as f64 / f64::from(annual_trading_days.max(1));
    if years > 0.0 && ending_equity > 0.0 {
        metrics.annualized_return_pct =
            ((ending_equity / initial_cash).powf(1.0 / years) - 1.0) * 100.0;
    }
    let benchmark_start = equity.first().map_or(1.0, |point| point.benchmark_equity);
    let benchmark_end = equity.last().map_or(1.0, |point| point.benchmark_equity);
    if benchmark_start > 0.0 {
        metrics.benchmark_return_pct = (benchmark_end / benchmark_start - 1.0) * 100.0;
    }
    metrics.excess_return_pct = metrics.total_return_pct - metrics.benchmark_return_pct;

    let returns = daily_returns(equity);
    if !returns.is_empty() {
        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance = returns
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / returns.len() as f64;
        let daily_volatility = variance.sqrt();
        let scale = f64::from(annual_trading_days).sqrt();
        metrics.annualized_volatility_pct = daily_volatility * scale * 100.0;
        if daily_volatility > 1e-12 {
            metrics.sharpe = mean / daily_volatility * scale;
        }
        let downside = returns
            .iter()
            .filter(|value| **value < 0.0)
            .map(|value| value.powi(2))
            .sum::<f64>();
        let downside_deviation = (downside / returns.len() as f64).sqrt();
        if downside_deviation > 1e-12 {
            metrics.sortino = mean / downside_deviation * scale;
        }
    }
    let (drawdown, duration) = drawdown(equity);
    metrics.max_drawdown_pct = drawdown;
    metrics.max_drawdown_duration_days = duration;
    if drawdown < -1e-12 {
        metrics.calmar = metrics.annualized_return_pct / drawdown.abs();
    }
    metrics.market_exposure_pct =
        equity.iter().map(|point| point.exposure_pct).sum::<f64>() / equity.len() as f64;
    let average_equity =
        equity.iter().map(|point| point.total_equity).sum::<f64>() / equity.len() as f64;
    if average_equity > 0.0 {
        let traded_notional: f64 = trades
            .iter()
            .map(|trade| (trade.entry_price + trade.exit_price) * trade.quantity as f64)
            .sum();
        metrics.turnover_pct = traded_notional / average_equity * 100.0;
    }
    metrics
}

pub fn metrics_by_year(
    equity: &[PortfolioEquityPoint],
    trades: &[PortfolioTrade],
    annual_trading_days: u16,
) -> BTreeMap<String, PortfolioMetrics> {
    let mut years: BTreeMap<String, Vec<PortfolioEquityPoint>> = BTreeMap::new();
    for point in equity {
        years
            .entry(year_label(&point.date))
            .or_default()
            .push(point.clone());
    }
    years
        .into_iter()
        .map(|(year, points)| {
            let year_trades: Vec<_> = trades
                .iter()
                .filter(|trade| year_label(&trade.exit_date) == year)
                .cloned()
                .collect();
            let initial = points
                .first()
                .map_or(0.0, |point| point.total_equity.max(1e-12));
            let rebased: Vec<_> = points
                .iter()
                .map(|point| PortfolioEquityPoint {
                    total_equity: point.total_equity / initial,
                    benchmark_equity: point.benchmark_equity
                        / points
                            .first()
                            .map_or(1.0, |first| first.benchmark_equity.max(1e-12)),
                    cash: point.cash / initial,
                    positions_value: point.positions_value / initial,
                    ..point.clone()
                })
                .collect();
            (
                year,
                calculate_metrics(&rebased, &year_trades, 1.0, annual_trading_days),
            )
        })
        .collect()
}

pub fn metrics_by_instrument(
    trades: &[PortfolioTrade],
    initial_cash: f64,
) -> BTreeMap<String, PortfolioMetrics> {
    let mut grouped: BTreeMap<String, Vec<PortfolioTrade>> = BTreeMap::new();
    for trade in trades {
        grouped
            .entry(trade.instrument.storage_key())
            .or_default()
            .push(trade.clone());
    }
    grouped
        .into_iter()
        .map(|(instrument, trades)| (instrument, calculate_trade_metrics(&trades, initial_cash)))
        .collect()
}

pub fn calculate_trade_metrics(trades: &[PortfolioTrade], initial_cash: f64) -> PortfolioMetrics {
    let wins: Vec<_> = trades.iter().filter(|trade| trade.net_pnl > 0.0).collect();
    let losses: Vec<_> = trades.iter().filter(|trade| trade.net_pnl < 0.0).collect();
    let average_win = mean(wins.iter().map(|trade| trade.net_pnl));
    let average_loss = mean(losses.iter().map(|trade| trade.net_pnl));
    let gross_wins = wins.iter().map(|trade| trade.net_pnl).sum::<f64>();
    let gross_losses = losses.iter().map(|trade| trade.net_pnl).sum::<f64>().abs();
    let gross_profit = trades.iter().map(|trade| trade.gross_pnl).sum::<f64>();
    let total_cost = trades.iter().map(|trade| trade.total_cost).sum::<f64>();
    let net_profit = trades.iter().map(|trade| trade.net_pnl).sum::<f64>();
    PortfolioMetrics {
        total_return_pct: if initial_cash > 0.0 {
            net_profit / initial_cash * 100.0
        } else {
            0.0
        },
        win_rate_pct: if trades.is_empty() {
            0.0
        } else {
            wins.len() as f64 / trades.len() as f64 * 100.0
        },
        average_win,
        average_loss,
        payoff_ratio: if average_loss < -1e-12 {
            average_win / average_loss.abs()
        } else {
            0.0
        },
        profit_factor: if gross_losses > 1e-12 {
            gross_wins / gross_losses
        } else {
            0.0
        },
        trade_count: trades.len(),
        average_holding_sessions: mean(trades.iter().map(|trade| trade.holding_sessions as f64)),
        gross_profit,
        total_cost,
        net_profit,
        cost_to_gross_profit_pct: if gross_profit > 1e-12 {
            total_cost / gross_profit * 100.0
        } else {
            0.0
        },
        ..PortfolioMetrics::default()
    }
}

fn daily_returns(equity: &[PortfolioEquityPoint]) -> Vec<f64> {
    equity
        .windows(2)
        .filter_map(|window| {
            let previous = window[0].total_equity;
            (previous > 0.0).then_some(window[1].total_equity / previous - 1.0)
        })
        .collect()
}

fn drawdown(equity: &[PortfolioEquityPoint]) -> (f64, usize) {
    let mut peak = 0.0_f64;
    let mut worst = 0.0_f64;
    let mut duration = 0;
    let mut worst_duration = 0;
    for point in equity {
        if point.total_equity >= peak {
            peak = point.total_equity;
            duration = 0;
        } else if peak > 0.0 {
            duration += 1;
            worst_duration = worst_duration.max(duration);
            worst = worst.min((point.total_equity / peak - 1.0) * 100.0);
        }
    }
    (worst, worst_duration)
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let values: Vec<_> = values.collect();
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn year_label(date: &str) -> String {
    date.get(..4).unwrap_or(date).to_string()
}
