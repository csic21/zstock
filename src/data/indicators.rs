//! Technical indicators (computed client-side from OHLCV).
//!
//! Includes moving averages (MA), MACD (12/26/9) and Bollinger Bands (20, 2σ).

use crate::model::Candle;

#[derive(Debug, Clone, Default)]
pub struct MaSeries {
    pub ma5: Vec<Option<f64>>,
    pub ma10: Vec<Option<f64>>,
    pub ma20: Vec<Option<f64>>,
    pub ma60: Vec<Option<f64>>,
}

impl MaSeries {
    pub fn from_candles(candles: &[Candle]) -> Self {
        Self {
            ma5: sma(candles, 5),
            ma10: sma(candles, 10),
            ma20: sma(candles, 20),
            ma60: sma(candles, 60),
        }
    }

    pub fn value_at(&self, ix: usize) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
        (
            self.ma5.get(ix).copied().flatten(),
            self.ma10.get(ix).copied().flatten(),
            self.ma20.get(ix).copied().flatten(),
            self.ma60.get(ix).copied().flatten(),
        )
    }

    /// Slice indicator series to a half-open `[start, end)` window (for chart zoom).
    pub fn slice(&self, start: usize, end: usize) -> Self {
        let clip = |v: &[Option<f64>]| {
            if start >= v.len() {
                Vec::new()
            } else {
                v[start..end.min(v.len())].to_vec()
            }
        };
        Self {
            ma5: clip(&self.ma5),
            ma10: clip(&self.ma10),
            ma20: clip(&self.ma20),
            ma60: clip(&self.ma60),
        }
    }
}

/// MACD (12, 26, 9): DIF = EMA12 − EMA26, DEA = EMA9(DIF), HIST = 2×(DIF−DEA).
#[derive(Debug, Clone, Default)]
pub struct MacdSeries {
    pub dif: Vec<Option<f64>>,
    pub dea: Vec<Option<f64>>,
    pub hist: Vec<Option<f64>>,
}

/// Standard MACD parameters.
pub const MACD_FAST: usize = 12;
pub const MACD_SLOW: usize = 26;
pub const MACD_SIGNAL: usize = 9;

impl MacdSeries {
    pub fn from_candles(candles: &[Candle]) -> Self {
        let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
        let ema_fast = ema(&closes, MACD_FAST);
        let ema_slow = ema(&closes, MACD_SLOW);
        let mut dif = vec![None; closes.len()];
        for i in MACD_SLOW.saturating_sub(1)..closes.len() {
            let (Some(f), Some(s)) = (ema_fast[i], ema_slow[i]) else {
                continue;
            };
            dif[i] = Some(f - s);
        }
        // DEA = EMA9 of DIF (treat leading None as gap, seed on first Some).
        let dea_raw = ema_of_options(&dif, MACD_SIGNAL);
        let mut dea = vec![None; closes.len()];
        for i in 0..closes.len() {
            if dif[i].is_some() && i >= MACD_SLOW + MACD_SIGNAL - 2 {
                dea[i] = dea_raw[i];
            }
        }
        let mut hist = vec![None; closes.len()];
        for i in 0..closes.len() {
            if let (Some(d), Some(e)) = (dif[i], dea[i]) {
                hist[i] = Some((d - e) * 2.0);
            }
        }
        Self { dif, dea, hist }
    }

    pub fn value_at(&self, ix: usize) -> (Option<f64>, Option<f64>, Option<f64>) {
        (
            self.dif.get(ix).copied().flatten(),
            self.dea.get(ix).copied().flatten(),
            self.hist.get(ix).copied().flatten(),
        )
    }

    /// Slice to a half-open `[start, end)` window (for chart zoom).
    pub fn slice(&self, start: usize, end: usize) -> Self {
        let clip = |v: &[Option<f64>]| {
            if start >= v.len() {
                Vec::new()
            } else {
                v[start..end.min(v.len())].to_vec()
            }
        };
        Self {
            dif: clip(&self.dif),
            dea: clip(&self.dea),
            hist: clip(&self.hist),
        }
    }
}

/// Bollinger Bands (20, 2): middle = SMA20, bands = middle ± 2σ (population std).
#[derive(Debug, Clone, Default)]
pub struct BollSeries {
    pub mid: Vec<Option<f64>>,
    pub upper: Vec<Option<f64>>,
    pub lower: Vec<Option<f64>>,
}

/// Bollinger window / width.
pub const BOLL_PERIOD: usize = 20;
pub const BOLL_K: f64 = 2.0;

impl BollSeries {
    pub fn from_candles(candles: &[Candle]) -> Self {
        let n = candles.len();
        let mut mid = vec![None; n];
        let mut upper = vec![None; n];
        let mut lower = vec![None; n];
        if n < BOLL_PERIOD {
            return Self { mid, upper, lower };
        }
        let closes: Vec<f64> = candles.iter().map(|c| c.close).collect();
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        for i in 0..n {
            sum += closes[i];
            sum_sq += closes[i] * closes[i];
            if i >= BOLL_PERIOD {
                let drop = closes[i - BOLL_PERIOD];
                sum -= drop;
                sum_sq -= drop * drop;
            }
            if i + 1 >= BOLL_PERIOD {
                let m = sum / BOLL_PERIOD as f64;
                let var = (sum_sq / BOLL_PERIOD as f64 - m * m).max(0.0);
                let sd = var.sqrt();
                mid[i] = Some(m);
                upper[i] = Some(m + BOLL_K * sd);
                lower[i] = Some(m - BOLL_K * sd);
            }
        }
        Self { mid, upper, lower }
    }

    pub fn value_at(&self, ix: usize) -> (Option<f64>, Option<f64>, Option<f64>) {
        (
            self.upper.get(ix).copied().flatten(),
            self.mid.get(ix).copied().flatten(),
            self.lower.get(ix).copied().flatten(),
        )
    }

    /// Slice to a half-open `[start, end)` window (for chart zoom).
    pub fn slice(&self, start: usize, end: usize) -> Self {
        let clip = |v: &[Option<f64>]| {
            if start >= v.len() {
                Vec::new()
            } else {
                v[start..end.min(v.len())].to_vec()
            }
        };
        Self {
            mid: clip(&self.mid),
            upper: clip(&self.upper),
            lower: clip(&self.lower),
        }
    }
}

/// Simple moving average of close prices. Leading values are `None` until window is full.
pub fn sma(candles: &[Candle], period: usize) -> Vec<Option<f64>> {
    if period == 0 {
        return vec![None; candles.len()];
    }
    let mut out = Vec::with_capacity(candles.len());
    let mut sum = 0.0;
    for i in 0..candles.len() {
        sum += candles[i].close;
        if i >= period {
            sum -= candles[i - period].close;
        }
        if i + 1 >= period {
            out.push(Some(sum / period as f64));
        } else {
            out.push(None);
        }
    }
    out
}

/// Exponential moving average, seeded with the first value.
pub fn ema(values: &[f64], period: usize) -> Vec<Option<f64>> {
    if period == 0 || values.is_empty() {
        return vec![None; values.len()];
    }
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut out = Vec::with_capacity(values.len());
    let mut prev = values[0];
    out.push(Some(prev));
    for v in values.iter().skip(1) {
        prev = alpha * v + (1.0 - alpha) * prev;
        out.push(Some(prev));
    }
    out
}

/// EMA over an option series: gaps become `None`, seeded at the first `Some`.
fn ema_of_options(values: &[Option<f64>], period: usize) -> Vec<Option<f64>> {
    let mut out = vec![None; values.len()];
    if period == 0 {
        return out;
    }
    let alpha = 2.0 / (period as f64 + 1.0);
    let mut prev: Option<f64> = None;
    for (i, v) in values.iter().enumerate() {
        let Some(v) = v else {
            prev = None;
            continue;
        };
        match prev {
            None => {
                prev = Some(*v);
                out[i] = Some(*v);
            }
            Some(p) => {
                let next = alpha * v + (1.0 - alpha) * p;
                prev = Some(next);
                out[i] = Some(next);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Candle;

    fn candle(close: f64) -> Candle {
        Candle {
            date: "2026-01-01".into(),
            open: close,
            high: close,
            low: close,
            close,
            volume: 100,
        }
    }

    #[test]
    fn macd_has_expected_shape() {
        let candles: Vec<Candle> = (0..80).map(|i| candle(10.0 + (i % 7) as f64)).collect();
        let macd = MacdSeries::from_candles(&candles);
        assert_eq!(macd.dif.len(), 80);
        // DIF starts after the slow window; DEA after slow+signal.
        assert!(macd.dif[..25].iter().all(|v| v.is_none()));
        assert!(macd.dif[30..].iter().all(|v| v.is_some()));
        assert!(macd.dea[..33].iter().all(|v| v.is_none()));
        assert!(macd.dea[34..].iter().all(|v| v.is_some()));
        assert!(macd.hist[34..].iter().all(|v| v.is_some()));
        // HIST = 2 * (DIF - DEA)
        let ix = 70;
        let (d, e, h) = macd.value_at(ix);
        assert!((h.unwrap() - (d.unwrap() - e.unwrap()) * 2.0).abs() < 1e-9);
    }

    #[test]
    fn macd_constant_series_is_zero() {
        let candles: Vec<Candle> = (0..60).map(|_| candle(10.0)).collect();
        let macd = MacdSeries::from_candles(&candles);
        for i in 40..60 {
            let (d, e, h) = macd.value_at(i);
            assert!(d.unwrap().abs() < 1e-9);
            assert!(e.unwrap().abs() < 1e-9);
            assert!(h.unwrap().abs() < 1e-9);
        }
    }

    #[test]
    fn boll_bands_center_on_sma() {
        let candles: Vec<Candle> = (0..60).map(|i| candle(10.0 + (i % 5) as f64)).collect();
        let boll = BollSeries::from_candles(&candles);
        assert!(boll.mid[..19].iter().all(|v| v.is_none()));
        for i in 20..60 {
            let (up, mid, low) = boll.value_at(i);
            let (up, mid, low) = (up.unwrap(), mid.unwrap(), low.unwrap());
            assert!(up > mid);
            assert!(mid > low);
            // symmetric around mid
            assert!((up - mid - (mid - low)).abs() < 1e-9);
        }
    }

    #[test]
    fn boll_narrow_when_flat() {
        let mut candles: Vec<Candle> = (0..60).map(|_| candle(10.0)).collect();
        for i in 30..60 {
            candles[i].close = 20.0;
        }
        let boll = BollSeries::from_candles(&candles);
        // In the flat region std ≈ 0 → bands collapse onto the mid.
        let (up, mid, low) = boll.value_at(59);
        assert!((up.unwrap() - mid.unwrap()).abs() < 1e-6);
        assert!((mid.unwrap() - low.unwrap()).abs() < 1e-6);
    }
}
