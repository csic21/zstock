//! Technical indicators (computed client-side from OHLCV).

use crate::model::Candle;

#[derive(Debug, Clone, Default)]
pub struct MaSeries {
    pub ma5: Vec<Option<f64>>,
    pub ma10: Vec<Option<f64>>,
    pub ma20: Vec<Option<f64>>,
}

impl MaSeries {
    pub fn from_candles(candles: &[Candle]) -> Self {
        Self {
            ma5: sma(candles, 5),
            ma10: sma(candles, 10),
            ma20: sma(candles, 20),
        }
    }

    pub fn value_at(&self, ix: usize) -> (Option<f64>, Option<f64>, Option<f64>) {
        (
            self.ma5.get(ix).copied().flatten(),
            self.ma10.get(ix).copied().flatten(),
            self.ma20.get(ix).copied().flatten(),
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
