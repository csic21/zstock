//! In-memory LRU cache for K-line / minute series so symbol switches feel instant.

use std::collections::{HashMap, VecDeque};

use crate::model::{Candle, MinutePeriod, MinuteSeries};

use super::types::ChartKind;

/// Max distinct series kept in RAM (day + minute + intraday keys share the budget).
const MAX_KLINE_ENTRIES: usize = 48;
const MAX_MINUTE_ENTRIES: usize = 24;

#[derive(Debug, Clone)]
pub(crate) struct CachedKlines {
    pub name: String,
    pub candles: Vec<Candle>,
    pub source: String,
}

#[derive(Debug, Default)]
pub(crate) struct SeriesCache {
    klines: HashMap<String, CachedKlines>,
    kline_order: VecDeque<String>,
    minutes: HashMap<String, MinuteSeries>,
    minute_order: VecDeque<String>,
}

impl SeriesCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Cache key for day / minute-K series (not intraday points).
    pub(crate) fn kline_key(kind: ChartKind, code: &str) -> Option<String> {
        match kind {
            ChartKind::DayK => Some(format!("d:{code}")),
            ChartKind::MinuteK(p) => Some(format!("{}:{code}", p.param())),
            ChartKind::Intraday => None,
        }
    }

    pub(crate) fn put_klines(&mut self, key: String, entry: CachedKlines) {
        if self.klines.contains_key(&key) {
            self.kline_order.retain(|k| k != &key);
        }
        self.klines.insert(key.clone(), entry);
        self.kline_order.push_back(key);
        while self.kline_order.len() > MAX_KLINE_ENTRIES {
            if let Some(old) = self.kline_order.pop_front() {
                self.klines.remove(&old);
            }
        }
    }

    pub(crate) fn get_klines(&self, key: &str) -> Option<&CachedKlines> {
        self.klines.get(key)
    }

    /// Look up day/minute series, optionally trimming a longer day cache to `bars`.
    pub(crate) fn lookup_klines(
        &self,
        kind: ChartKind,
        code: &str,
        bars: usize,
    ) -> Option<CachedKlines> {
        let key = Self::kline_key(kind, code)?;
        let entry = self.get_klines(&key)?;
        if entry.candles.is_empty() {
            return None;
        }
        // DayK: if we previously fetched a longer window, serve a tail slice.
        if matches!(kind, ChartKind::DayK) && bars > 0 && entry.candles.len() > bars {
            let start = entry.candles.len() - bars;
            return Some(CachedKlines {
                name: entry.name.clone(),
                candles: entry.candles[start..].to_vec(),
                source: entry.source.clone(),
            });
        }
        Some(entry.clone())
    }

    /// When storing day bars, keep the longer of old vs new for the same code.
    pub(crate) fn put_klines_smart(&mut self, kind: ChartKind, code: &str, entry: CachedKlines) {
        let Some(key) = Self::kline_key(kind, code) else {
            return;
        };
        if matches!(kind, ChartKind::DayK) {
            if let Some(old) = self.klines.get(&key) {
                if old.candles.len() > entry.candles.len() {
                    let old_last = old.candles.last().map(|c| c.date.as_ref().to_string());
                    let new_last = entry.candles.last().map(|c| c.date.as_ref().to_string());
                    // Prefer longer history when the new fetch is a shorter window
                    // and the series end date still matches (same session snapshot).
                    if old_last == new_last {
                        let mut merged = old.clone();
                        if !entry.name.is_empty() {
                            merged.name = entry.name;
                        }
                        if !entry.source.is_empty() {
                            merged.source = entry.source;
                        }
                        self.put_klines(key, merged);
                        return;
                    }
                }
            }
        }
        self.put_klines(key, entry);
    }

    pub(crate) fn put_minute(&mut self, code: &str, series: MinuteSeries) {
        let key = code.to_string();
        if self.minutes.contains_key(&key) {
            self.minute_order.retain(|k| k != &key);
        }
        self.minutes.insert(key.clone(), series);
        self.minute_order.push_back(key);
        while self.minute_order.len() > MAX_MINUTE_ENTRIES {
            if let Some(old) = self.minute_order.pop_front() {
                self.minutes.remove(&old);
            }
        }
    }

    pub(crate) fn get_minute(&self, code: &str) -> Option<&MinuteSeries> {
        self.minutes.get(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::shared;

    fn candle(date: &str, close: f64) -> Candle {
        Candle {
            date: shared(date),
            open: close,
            high: close,
            low: close,
            close,
            volume: 1,
        }
    }

    #[test]
    fn day_cache_serves_tail_slice() {
        let mut c = SeriesCache::new();
        let candles: Vec<_> = (0..100)
            .map(|i| candle(&format!("2024-01-{:02}", (i % 28) + 1), i as f64))
            .collect();
        c.put_klines_smart(
            ChartKind::DayK,
            "600519",
            CachedKlines {
                name: "茅台".into(),
                candles,
                source: "test".into(),
            },
        );
        let hit = c.lookup_klines(ChartKind::DayK, "600519", 30).unwrap();
        assert_eq!(hit.candles.len(), 30);
        assert!((hit.candles.last().unwrap().close - 99.0).abs() < 1e-9);
    }

    #[test]
    fn lru_evicts_oldest() {
        let mut c = SeriesCache::new();
        for i in 0..(MAX_KLINE_ENTRIES + 5) {
            let code = format!("{i:06}");
            c.put_klines(
                SeriesCache::kline_key(ChartKind::DayK, &code).unwrap(),
                CachedKlines {
                    name: code.clone(),
                    candles: vec![candle("2024-01-01", 1.0)],
                    source: "t".into(),
                },
            );
        }
        assert!(c.get_klines("d:000000").is_none());
        assert!(c
            .get_klines(&format!("d:{:06}", MAX_KLINE_ENTRIES + 4))
            .is_some());
    }

    #[test]
    fn minute_period_keys_are_distinct() {
        let mut c = SeriesCache::new();
        c.put_klines_smart(
            ChartKind::MinuteK(MinutePeriod::M5),
            "600519",
            CachedKlines {
                name: "x".into(),
                candles: vec![candle("2024-01-01 10:00", 1.0)],
                source: "t".into(),
            },
        );
        assert!(c
            .lookup_klines(ChartKind::MinuteK(MinutePeriod::M15), "600519", 30)
            .is_none());
        assert!(c
            .lookup_klines(ChartKind::MinuteK(MinutePeriod::M5), "600519", 30)
            .is_some());
    }
}
