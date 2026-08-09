//! 技术面参考价位（分批建仓 / 减仓观察带）。
//!
//! 全部由本地日 K 推算，不预测涨跌，只给出可解释的支撑/阻力带。
//! UI 与 AI 快照共用，结尾必须强调「仅供学习研究，不构成投资建议」。

use serde::Serialize;

use crate::data::indicators::MaSeries;
use crate::model::Candle;

/// 参考价位带：搜罗场景下展示「多少元附近可关注买入 / 减仓」。
#[derive(Debug, Clone, Serialize)]
pub struct ReferenceLevels {
    pub close: f64,
    /// 参考建仓下沿（更靠近支撑 / 更深回撤处）。
    pub buy_low: f64,
    /// 参考建仓上沿（靠近现价或轻度回踩）。
    pub buy_high: f64,
    /// 参考减仓下沿（第一阻力附近）。
    pub sell_low: f64,
    /// 参考减仓上沿（更强阻力）。
    pub sell_high: f64,
    /// 14 日 ATR（元）。
    pub atr14: Option<f64>,
    /// 近 20 日低点。
    pub low_20: Option<f64>,
    /// 近 60 日低点。
    pub low_60: Option<f64>,
    /// 近 20 日高点。
    pub high_20: Option<f64>,
    /// 近 60 日高点。
    pub high_60: Option<f64>,
    pub ma20: Option<f64>,
    pub ma60: Option<f64>,
    /// 人类可读依据（中文短句）。
    pub notes: Vec<String>,
}

impl ReferenceLevels {
    /// 一行建仓带文案，如 `12.30 – 13.10`。
    pub fn buy_band_text(&self) -> String {
        format!("{} – {}", fmt_px(self.buy_low), fmt_px(self.buy_high))
    }

    /// 一行减仓带文案。
    pub fn sell_band_text(&self) -> String {
        format!("{} – {}", fmt_px(self.sell_low), fmt_px(self.sell_high))
    }
}

/// 从日 K 推算参考建仓 / 减仓价位带。
///
/// 需要至少约 30 根有效 K；不足则返回 `None`。
pub fn compute(candles: &[Candle]) -> Option<ReferenceLevels> {
    if candles.len() < 30 {
        return None;
    }
    let last = candles.last()?;
    let close = last.close;
    if !close.is_finite() || close <= 0.0 {
        return None;
    }

    let low_20 = window_extreme(candles, 20, Extreme::Low);
    let low_60 = window_extreme(candles, 60, Extreme::Low);
    let high_20 = window_extreme(candles, 20, Extreme::High);
    let high_60 = window_extreme(candles, 60, Extreme::High);
    let atr14 = atr(candles, 14);

    let ma = MaSeries::from_candles(candles);
    let ix = candles.len() - 1;
    let (_m5, _m10, ma20, ma60) = ma.value_at(ix);

    // —— 支撑候选：近端低点、均线、ATR 下沿 ——
    let mut supports: Vec<(f64, &'static str)> = Vec::new();
    if let Some(v) = low_20 {
        supports.push((v, "近20日低点"));
    }
    if let Some(v) = low_60 {
        supports.push((v, "近60日低点"));
    }
    if let Some(v) = ma20.filter(|m| *m < close) {
        supports.push((v, "MA20"));
    }
    if let Some(v) = ma60.filter(|m| *m < close) {
        supports.push((v, "MA60"));
    }
    if let Some(a) = atr14 {
        supports.push(((close - a).max(0.01), "现价−1×ATR"));
        supports.push(((close - 1.5 * a).max(0.01), "现价−1.5×ATR"));
    }
    supports.retain(|(v, _)| v.is_finite() && *v > 0.0 && *v <= close * 1.02);
    supports.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // —— 阻力候选 ——
    let mut resists: Vec<(f64, &'static str)> = Vec::new();
    if let Some(v) = high_20 {
        resists.push((v, "近20日高点"));
    }
    if let Some(v) = high_60 {
        resists.push((v, "近60日高点"));
    }
    if let Some(v) = ma20.filter(|m| *m > close) {
        resists.push((v, "MA20"));
    }
    if let Some(v) = ma60.filter(|m| *m > close) {
        resists.push((v, "MA60"));
    }
    if let Some(a) = atr14 {
        resists.push((close + a, "现价+1×ATR"));
        resists.push((close + 1.5 * a, "现价+1.5×ATR"));
    }
    resists.retain(|(v, _)| v.is_finite() && *v >= close * 0.98);
    resists.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // 建仓带：取「最靠近现价的下方支撑」到「稍深一档」
    let (buy_high, buy_low, buy_notes) = buy_band(close, atr14, &supports);
    // 减仓带：取「最靠近现价的上方阻力」到「更远一档」
    let (sell_low, sell_high, sell_notes) = sell_band(close, atr14, &resists);

    let mut notes = Vec::new();
    notes.extend(buy_notes);
    notes.extend(sell_notes);
    if notes.is_empty() {
        notes.push("以近端高低点与 ATR 构造观察带".into());
    }

    Some(ReferenceLevels {
        close,
        buy_low: round_px(buy_low.min(buy_high)),
        buy_high: round_px(buy_high.max(buy_low)),
        sell_low: round_px(sell_low.min(sell_high)),
        sell_high: round_px(sell_high.max(sell_low)),
        atr14: atr14.map(round_px),
        low_20: low_20.map(round_px),
        low_60: low_60.map(round_px),
        high_20: high_20.map(round_px),
        high_60: high_60.map(round_px),
        ma20: ma20.map(round_px),
        ma60: ma60.map(round_px),
        notes,
    })
}

fn buy_band(
    close: f64,
    atr14: Option<f64>,
    supports: &[(f64, &'static str)],
) -> (f64, f64, Vec<String>) {
    let mut notes = Vec::new();
    // 默认：现价下方 0.5–1.5 ATR
    let fallback_hi = atr14.map(|a| close - 0.3 * a).unwrap_or(close * 0.99);
    let fallback_lo = atr14.map(|a| close - 1.5 * a).unwrap_or(close * 0.94);

    let near = supports.iter().find(|(v, _)| *v < close * 0.999).copied();
    let deeper = supports
        .iter()
        .filter(|(v, _)| near.is_none_or(|(n, _)| *v < n * 0.995))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .copied();

    let buy_high = near
        .map(|(v, label)| {
            notes.push(format!("建仓上沿参考{label}"));
            // 允许略高于支撑一点作为「可接受回踩」
            atr14
                .map(|a| (v + 0.25 * a).min(close))
                .unwrap_or(v.min(close))
        })
        .unwrap_or(fallback_hi.min(close));

    let buy_low = deeper
        .map(|(v, label)| {
            notes.push(format!("建仓下沿参考{label}"));
            v
        })
        .or_else(|| near.map(|(v, _)| atr14.map(|a| v - 0.5 * a).unwrap_or(v * 0.98)))
        .unwrap_or(fallback_lo)
        .min(buy_high)
        .max(0.01);

    // 保证带宽有意义
    let (buy_low, buy_high) = ensure_band(buy_low, buy_high, close, true);
    (buy_high, buy_low, notes)
}

fn sell_band(
    close: f64,
    atr14: Option<f64>,
    resists: &[(f64, &'static str)],
) -> (f64, f64, Vec<String>) {
    let mut notes = Vec::new();
    let fallback_lo = atr14.map(|a| close + 0.5 * a).unwrap_or(close * 1.03);
    let fallback_hi = atr14.map(|a| close + 1.5 * a).unwrap_or(close * 1.08);

    let near = resists.iter().find(|(v, _)| *v > close * 1.001).copied();
    let farther = resists
        .iter()
        .filter(|(v, _)| near.is_none_or(|(n, _)| *v > n * 1.005))
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .copied();

    let sell_low = near
        .map(|(v, label)| {
            notes.push(format!("减仓下沿参考{label}"));
            atr14
                .map(|a| (v - 0.2 * a).max(close))
                .unwrap_or(v.max(close))
        })
        .unwrap_or(fallback_lo.max(close));

    let sell_high = farther
        .map(|(v, label)| {
            notes.push(format!("减仓上沿参考{label}"));
            v
        })
        .or_else(|| near.map(|(v, _)| atr14.map(|a| v + 0.5 * a).unwrap_or(v * 1.02)))
        .unwrap_or(fallback_hi)
        .max(sell_low);

    let (sell_low, sell_high) = ensure_band(sell_low, sell_high, close, false);
    (sell_low, sell_high, notes)
}

/// 保证价位带至少约 0.8% 宽，避免高低贴死。
fn ensure_band(lo: f64, hi: f64, close: f64, is_buy: bool) -> (f64, f64) {
    let min_w = (close * 0.008).max(0.02);
    if hi - lo >= min_w {
        return (lo, hi);
    }
    if is_buy {
        let mid = (lo + hi) / 2.0;
        ((mid - min_w / 2.0).max(0.01), mid + min_w / 2.0)
    } else {
        let mid = (lo + hi) / 2.0;
        (mid - min_w / 2.0, mid + min_w / 2.0)
    }
}

#[derive(Clone, Copy)]
enum Extreme {
    High,
    Low,
}

fn window_extreme(candles: &[Candle], n: usize, kind: Extreme) -> Option<f64> {
    if candles.is_empty() || n == 0 {
        return None;
    }
    let start = candles.len().saturating_sub(n);
    let window = &candles[start..];
    match kind {
        Extreme::High => {
            let v = window
                .iter()
                .map(|c| c.high)
                .filter(|v| v.is_finite())
                .fold(f64::NEG_INFINITY, f64::max);
            (v.is_finite() && v > 0.0).then_some(v)
        }
        Extreme::Low => {
            let v = window
                .iter()
                .map(|c| c.low)
                .filter(|v| v.is_finite() && *v > 0.0)
                .fold(f64::INFINITY, f64::min);
            (v.is_finite() && v > 0.0 && v < f64::INFINITY).then_some(v)
        }
    }
}

/// Wilder 风格简化 ATR：TR 的 SMA。
fn atr(candles: &[Candle], period: usize) -> Option<f64> {
    if period == 0 || candles.len() < period + 1 {
        return None;
    }
    let mut trs = Vec::with_capacity(candles.len());
    for i in 1..candles.len() {
        let c = &candles[i];
        let p = &candles[i - 1];
        if !(c.high.is_finite() && c.low.is_finite() && c.close.is_finite() && p.close.is_finite())
        {
            continue;
        }
        let tr = (c.high - c.low)
            .max((c.high - p.close).abs())
            .max((c.low - p.close).abs());
        if tr.is_finite() && tr >= 0.0 {
            trs.push(tr);
        }
    }
    if trs.len() < period {
        return None;
    }
    let window = &trs[trs.len() - period..];
    Some(window.iter().sum::<f64>() / period as f64)
}

fn round_px(v: f64) -> f64 {
    // A 股常见 2 位小数；高价股仍用 2 位足够展示。
    (v * 100.0).round() / 100.0
}

fn fmt_px(v: f64) -> String {
    if v >= 1000.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::shared;

    fn series(start: f64, daily: f64, n: usize) -> Vec<Candle> {
        (0..n)
            .map(|i| {
                let close = start * (1.0 + daily).powi(i as i32);
                Candle {
                    date: shared(format!("d{i}")),
                    open: close,
                    high: close * 1.015,
                    low: close * 0.985,
                    close,
                    volume: 100_000,
                }
            })
            .collect()
    }

    #[test]
    fn levels_on_rising_series_are_ordered() {
        let candles = series(10.0, 0.004, 120);
        let lv = compute(&candles).expect("levels");
        assert!(lv.buy_low <= lv.buy_high);
        assert!(lv.sell_low <= lv.sell_high);
        assert!(lv.buy_high <= lv.close * 1.02);
        assert!(lv.sell_low >= lv.close * 0.98);
    }

    #[test]
    fn too_short_returns_none() {
        let candles = series(10.0, 0.001, 10);
        assert!(compute(&candles).is_none());
    }
}
