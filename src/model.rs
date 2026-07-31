//! Domain types for A-share symbols and OHLCV candles.

use gpui::SharedString;
use serde::{Deserialize, Serialize};

/// Stable, non-reversible-looking service id for work mode.
///
/// The visible alias deliberately does not include the security code or name
/// initials. It remains stable between launches so rows are still recognisable.
pub fn disguise_label(code: &str, _name: &str) -> String {
    const GROUP: &[&str] = &[
        "api", "cache", "index", "queue", "relay", "search", "store", "stream",
    ];
    let h = fnv1a32(code.as_bytes());
    let group = GROUP[(h as usize) % GROUP.len()];
    let instance = ((h >> 8) ^ h) & 0x0fff;
    format!("{group}-{instance:03x}")
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod disguise_tests {
    use super::disguise_label;

    #[test]
    fn alias_is_stable_and_does_not_leak_identity() {
        let alias = disguise_label("600519", "贵州茅台");
        assert_eq!(alias, disguise_label("600519", "renamed"));
        assert!(!alias.contains("600519"));
        assert!(!alias.contains("gzmt"));
        assert_ne!(alias, disguise_label("000001", "平安银行"));
    }
}

/// Major A-share index snapshot (work-mode host gauges).
#[derive(Debug, Clone, Copy)]
pub struct IndexSnap {
    pub last: f64,
    pub change_pct: f64,
}

impl IndexSnap {
    pub fn pct_label(self) -> String {
        format!("{:+.2}%", self.change_pct)
    }

    pub fn point_label(self) -> String {
        format!("{:.2}", self.last)
    }
}

/// Rebase a price onto index base 100 (work mode — looks like a metric, not a quote).
pub fn disguise_index(value: f64, base: f64) -> f64 {
    if base > 1e-9 {
        value / base * 100.0
    } else {
        value
    }
}

pub fn format_index(value: f64) -> String {
    format!("{value:.2}")
}

/// One row in the watchlist.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// Pure code, e.g. `600519`, `000001`.
    pub code: String,
    pub name: SharedString,
    pub last: f64,
    pub change_pct: f64,
    pub volume: u64,
    /// Market board label: 沪市 / 深市 / 创业板 / 科创板
    pub board: SharedString,
}

impl Symbol {
    pub fn is_up(&self) -> bool {
        self.change_pct >= 0.0
    }

    pub fn ticker_label(&self) -> SharedString {
        SharedString::from(self.code.clone())
    }

    /// Eastmoney `secid`: `1.600519` (SH) or `0.000001` (SZ).
    pub fn secid(&self) -> String {
        secid_for_code(&self.code)
    }
}

/// Daily (or bar) OHLCV candle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    pub date: SharedString,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
}

/// Header snapshot for the active symbol.
#[derive(Debug, Clone, Default)]
pub struct QuoteSnapshot {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: u64,
    pub change_pct: f64,
    pub prev_close: f64,
}

impl QuoteSnapshot {
    /// Snapshot from the **last** candle (当日/最近一根 OHLC)，不是整段区间极值。
    pub fn from_candles(candles: &[Candle]) -> Option<Self> {
        let last = candles.last()?;
        let prev = candles
            .get(candles.len().saturating_sub(2))
            .map(|c| c.close)
            .unwrap_or(last.open);
        let change_pct = if prev > 0.0 {
            (last.close - prev) / prev * 100.0
        } else {
            0.0
        };
        Some(Self {
            open: last.open,
            high: last.high,
            low: last.low,
            close: last.close,
            volume: last.volume,
            change_pct,
            prev_close: prev,
        })
    }
}

/// Map 6-digit A-share / ETF code → Eastmoney `secid` (`1.xxxxxx` SH, `0.xxxxxx` SZ).
///
/// Rules (simplified, covers common equities + ETFs):
/// - SH: 6xxxxx stocks, 688 科创, 5xxxxx funds/ETFs (e.g. 510300), 9xxxxx B
/// - SZ: 0xxxxx / 3xxxxx stocks, 1xxxxx funds/ETFs
pub fn secid_for_code(code: &str) -> String {
    let code = code.trim();
    let market = if is_sh_market(code) { 1 } else { 0 };
    format!("{market}.{code}")
}

fn is_sh_market(code: &str) -> bool {
    code.starts_with('6')
        || code.starts_with('5') // 上交所基金/ETF（510/511/512/513/515/516/518/588…）
        || code.starts_with('9')
}

pub fn board_for_code(code: &str) -> SharedString {
    let label = if code.starts_with("688") || code.starts_with("689") {
        "科创板"
    } else if code.starts_with("300") || code.starts_with("301") {
        "创业板"
    } else if code.starts_with('5') || code.starts_with('1') {
        "ETF"
    } else if is_sh_market(code) {
        "沪市"
    } else {
        "深市"
    };
    SharedString::from(label)
}

/// Built-in starter universe (used when no persisted list / as search corpus seed).
pub fn default_watchlist_codes() -> Vec<String> {
    vec![
        "600519".into(), // 贵州茅台
        "000858".into(), // 五粮液
        "300750".into(), // 宁德时代
        "002594".into(), // 比亚迪
        "601318".into(), // 中国平安
        "600036".into(), // 招商银行
        "000001".into(), // 平安银行
        "601012".into(), // 隆基绿能
        "600900".into(), // 长江电力
        "000333".into(), // 美的集团
        "002415".into(), // 海康威视
        "600276".into(), // 恒瑞医药
        "601888".into(), // 中国中免
        "300059".into(), // 东方财富
        "510300".into(), // 沪深300ETF（场内基金，东财可查）
    ]
}

pub fn format_price(v: f64) -> String {
    if v >= 1000.0 {
        format!("{v:.2}")
    } else if v >= 100.0 {
        format!("{v:.2}")
    } else if v >= 10.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

pub fn format_pct(v: f64) -> String {
    if v >= 0.0 {
        format!("+{v:.2}%")
    } else {
        format!("{v:.2}%")
    }
}

pub fn format_volume(v: u64) -> String {
    // A-share volume is usually in 手 (×100 shares) from EM — we display raw as-is with units
    if v >= 100_000_000 {
        format!("{:.2}亿", v as f64 / 100_000_000.0)
    } else if v >= 10_000 {
        format!("{:.1}万", v as f64 / 10_000.0)
    } else {
        v.to_string()
    }
}

pub fn shared(s: impl Into<String>) -> SharedString {
    SharedString::from(s.into())
}
