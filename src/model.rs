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

/// Sanitize a user-chosen work-mode nickname.
///
/// Returns `None` when the input is empty (caller should clear the alias).
/// Keeps ASCII letters/digits plus `-_./`, max 24 chars — looks like a service
/// id, not a ticker.
pub fn sanitize_work_alias(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        .take(24)
        .collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
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
    use super::{disguise_label, sanitize_work_alias};

    #[test]
    fn alias_is_stable_and_does_not_leak_identity() {
        let alias = disguise_label("600519", "贵州茅台");
        assert_eq!(alias, disguise_label("600519", "renamed"));
        assert!(!alias.contains("600519"));
        assert!(!alias.contains("gzmt"));
        assert_ne!(alias, disguise_label("000001", "平安银行"));
    }

    #[test]
    fn sanitize_work_alias_accepts_service_like_names() {
        assert_eq!(
            sanitize_work_alias("  core-db_v2  ").as_deref(),
            Some("core-db_v2")
        );
        assert_eq!(sanitize_work_alias("edge/cdn").as_deref(), Some("edge/cdn"));
        assert_eq!(sanitize_work_alias("   ").as_deref(), None);
        assert_eq!(
            sanitize_work_alias("茅台600519!!!").as_deref(),
            Some("600519")
        );
        let long = "a".repeat(40);
        assert_eq!(sanitize_work_alias(&long).unwrap().len(), 24);
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
    /// Pure code: A 股 6 位（`600519`），港股 5 位（`00700`）。
    pub code: String,
    pub name: SharedString,
    pub last: f64,
    pub change_pct: f64,
    pub volume: u64,
    /// Market board label: 沪市 / 深市 / 创业板 / 科创板 / 港股
    pub board: SharedString,
}

impl Symbol {
    pub fn is_up(&self) -> bool {
        self.change_pct >= 0.0
    }

    pub fn ticker_label(&self) -> SharedString {
        SharedString::from(self.code.clone())
    }

    /// Eastmoney `secid`: A 股 `1.600519` / `0.000001`，港股 `116.00700`。
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

/// A user-drawn chart line (trend / price line).
///
/// Anchors are stored in **series space** (candle index, price) so the line
/// stays glued to the same candles when the chart is zoomed or panned.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrendLine {
    /// Start anchor `(candle index, price)`.
    pub from: (usize, f64),
    /// End anchor `(candle index, price)`.
    pub to: (usize, f64),
    /// Index into the chart's line-color palette.
    pub color_ix: usize,
}

impl TrendLine {
    pub fn new(from: (usize, f64), to: (usize, f64), color_ix: usize) -> Self {
        Self { from, to, color_ix }
    }

    /// Horizontal price line helper (same price at both anchors).
    pub fn price_line(ix_a: usize, ix_b: usize, price: f64, color_ix: usize) -> Self {
        Self::new((ix_a, price), (ix_b, price), color_ix)
    }
}

/// 分钟 K 周期（腾讯 `mkline` 支持 m1/m5/m15/m30/m60）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MinutePeriod {
    M1,
    M5,
    M15,
    M30,
    M60,
}

impl MinutePeriod {
    /// Endpoint param, e.g. `m5`.
    pub fn param(self) -> &'static str {
        match self {
            Self::M1 => "m1",
            Self::M5 => "m5",
            Self::M15 => "m15",
            Self::M30 => "m30",
            Self::M60 => "m60",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::M1 => "1分",
            Self::M5 => "5分",
            Self::M15 => "15分",
            Self::M30 => "30分",
            Self::M60 => "60分",
        }
    }

    /// Practical bar cap of the Tencent `mkline` endpoint per period.
    pub fn bars(self) -> usize {
        match self {
            Self::M1 => 320,
            Self::M5 | Self::M15 | Self::M30 | Self::M60 => 800,
        }
    }

    pub fn all() -> [Self; 5] {
        [Self::M1, Self::M5, Self::M15, Self::M30, Self::M60]
    }
}

/// One minute of an intraday (分时) series.
#[derive(Debug, Clone)]
pub struct MinutePoint {
    /// `HH:MM` clock label, e.g. `09:30`.
    pub time: SharedString,
    pub price: f64,
    /// Cumulative volume (手) up to this minute.
    pub cum_volume: u64,
    /// Cumulative turnover (元) up to this minute.
    pub cum_amount: f64,
}

impl MinutePoint {
    /// Volume-weighted average price up to this minute (cum_amount / shares).
    pub fn avg_price(&self) -> f64 {
        let shares = self.cum_volume as f64 * 100.0;
        if shares > 1e-9 {
            self.cum_amount / shares
        } else {
            self.price
        }
    }

    /// Volume traded in this minute vs the previous point.
    pub fn minute_volume(&self, prev: Option<&MinutePoint>) -> u64 {
        self.cum_volume.saturating_sub(prev.map(|p| p.cum_volume).unwrap_or(0))
    }
}

/// Full-day intraday series from Tencent `minute/query`.
#[derive(Debug, Clone, Default)]
pub struct MinuteSeries {
    /// Trading date `YYYYMMDD`.
    pub date: String,
    pub points: Vec<MinutePoint>,
    /// Previous close (基准价).
    pub prev_close: f64,
    pub name: String,
}

impl MinuteSeries {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Convert points to Candle rows so the existing zoom / pan / hover machinery
    /// can be reused (open=high=low=close=price, volume=per-minute volume).
    pub fn as_candles(&self) -> Vec<Candle> {
        let mut prev: Option<&MinutePoint> = None;
        let mut out = Vec::with_capacity(self.points.len());
        for p in &self.points {
            let vol = p.minute_volume(prev);
            out.push(Candle {
                date: p.time.clone(),
                open: p.price,
                high: p.price,
                low: p.price,
                close: p.price,
                volume: vol,
            });
            prev = Some(p);
        }
        out
    }

    /// Day snapshot: 开=首笔价, 高/低=全天极值, 收=最新价, 量=累计量, 涨跌 vs 昨收.
    pub fn snapshot(&self) -> Option<QuoteSnapshot> {
        let first = self.points.first()?;
        let last = self.points.last()?;
        let high = self.points.iter().map(|p| p.price).fold(f64::MIN, f64::max);
        let low = self.points.iter().map(|p| p.price).fold(f64::MAX, f64::min);
        let change_pct = if self.prev_close > 0.0 {
            (last.price - self.prev_close) / self.prev_close * 100.0
        } else {
            0.0
        };
        Some(QuoteSnapshot {
            open: first.price,
            high,
            low,
            close: last.price,
            volume: last.cum_volume,
            change_pct,
            prev_close: self.prev_close,
        })
    }
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

/// Canonical pure code: A 股 6 位数字，港股 5 位数字（左侧补 0）。
///
/// Accepts common aliases: `hk00700` / `00700.HK` / `HK.00700` / `sh600519` / `sz000001`.
pub fn normalize_code(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();

    // Explicit HK prefixes / suffixes.
    if let Some(rest) = lower.strip_prefix("hk") {
        let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
        return pad_hk_digits(&digits);
    }
    if let Some(rest) = lower.strip_suffix(".hk") {
        let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
        return pad_hk_digits(&digits);
    }
    if let Some(rest) = lower.strip_prefix("hk.") {
        let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
        return pad_hk_digits(&digits);
    }

    // A-share exchange prefixes.
    for p in ["sh", "sz", "bj"] {
        if let Some(rest) = lower.strip_prefix(p) {
            let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() == 6 {
                return Some(digits);
            }
            return None;
        }
    }

    // Bare digits: 6 → A 股, 5 → 港股。1–4 位易与 A 股半成品码混淆，需 `hk`/` .HK` 前缀。
    if s.chars().all(|c| c.is_ascii_digit()) {
        if s.len() == 6 {
            return Some(s.to_string());
        }
        if s.len() == 5 {
            return Some(s.to_string());
        }
    }
    None
}

fn pad_hk_digits(digits: &str) -> Option<String> {
    if digits.is_empty() || digits.len() > 5 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(format!("{digits:0>5}"))
}

/// 港股：规范为 5 位数字代码（如 `00700`）。
pub fn is_hk_code(code: &str) -> bool {
    let c = code.trim();
    c.len() == 5 && c.chars().all(|ch| ch.is_ascii_digit())
}

/// A 股 / 场内基金：6 位数字。
pub fn is_a_share_code(code: &str) -> bool {
    let c = code.trim();
    c.len() == 6 && c.chars().all(|ch| ch.is_ascii_digit())
}

/// Map pure code → Eastmoney `secid`.
///
/// - A 股 SH `1.xxxxxx` / SZ·BJ `0.xxxxxx`
/// - 港股 `116.00700`
pub fn secid_for_code(code: &str) -> String {
    let code = code.trim();
    if is_hk_code(code) {
        return format!("116.{code}");
    }
    let market = if is_sh_market(code) { 1 } else { 0 };
    format!("{market}.{code}")
}

fn is_sh_market(code: &str) -> bool {
    if is_hk_code(code) {
        return false;
    }
    code.starts_with('6')
        || code.starts_with('5') // 上交所基金/ETF（510/511/512/513/515/516/518/588…）
        || code.starts_with('9')
}

pub fn board_for_code(code: &str) -> SharedString {
    let label = if is_hk_code(code) {
        "港股"
    } else if code.starts_with("688") || code.starts_with("689") {
        "科创板"
    } else if code.starts_with("300") || code.starts_with("301") {
        "创业板"
    } else if code.starts_with('5') || code.starts_with('1') {
        "ETF"
    } else if is_sh_market(code) {
        "沪市"
    } else if code.starts_with('4') || code.starts_with('8') {
        "北交所"
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

#[cfg(test)]
mod code_tests {
    use super::*;

    #[test]
    fn normalize_a_and_hk() {
        assert_eq!(normalize_code("600519").as_deref(), Some("600519"));
        assert_eq!(normalize_code("sh600519").as_deref(), Some("600519"));
        assert_eq!(normalize_code("00700").as_deref(), Some("00700"));
        assert_eq!(normalize_code("hk00700").as_deref(), Some("00700"));
        assert_eq!(normalize_code("HK700").as_deref(), Some("00700"));
        assert_eq!(normalize_code("700.HK").as_deref(), Some("00700"));
        assert_eq!(normalize_code("hk700").as_deref(), Some("00700"));
        // Bare short digits are ambiguous (could be partial A-share).
        assert!(normalize_code("700").is_none());
        assert!(normalize_code("").is_none());
    }

    #[test]
    fn secid_and_board_hk() {
        assert_eq!(secid_for_code("00700"), "116.00700");
        assert_eq!(secid_for_code("600519"), "1.600519");
        assert_eq!(board_for_code("00700").as_ref(), "港股");
        assert!(is_hk_code("00700"));
        assert!(is_a_share_code("600519"));
        assert!(!is_hk_code("600519"));
    }
}
