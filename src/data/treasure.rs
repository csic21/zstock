//! 寻宝鼠：多窗口历史低位扫描。
//!
//! 只看「近 1 年」会把**上行趋势中的中继回撤**误判成「历史低位」。
//! 本模块用 1Y / 3Y / 全样本（可用 K 线）的区间位置、收盘分位、高点回撤
//! 合成可解释分数，并打标签区分：
//! - 多年低位 / 近 1–3 年双低
//! - **上行中继回撤**（1Y 低但 3Y 仍高 → 降权）
//! - 样本不足 / ST 等

use serde::{Deserialize, Serialize};

use crate::model::Candle;

/// 约 1 年交易日。
pub const BARS_1Y: usize = 252;
/// 约 3 年交易日。
pub const BARS_3Y: usize = 750;
/// 扫描时拉取的最大日 K 根数（东财约 1000 ≈ 4 年；腾讯备源约 640）。
pub const TREASURE_KLINE_LIMIT: usize = 1000;

// 候选池大小 / 入榜数见 `universe::TREASURE_SCAN_CAP` / `TREASURE_TOP_N`。
/// 最少有效样本，否则标记样本不足且分数降权。
pub const MIN_BARS: usize = 120;
/// 至少有这么多根才认为 3Y 窗口可信。
pub const MIN_BARS_3Y: usize = 400;

/// 单只股票的多窗口低位指标（可序列化缓存）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreasureHit {
    pub code: String,
    pub name: String,
    pub close: f64,
    /// 扫描用 K 线根数。
    pub bars: usize,
    pub as_of: String,
    /// 区间位置 0=区间最低，1=区间最高（用 high/low 极值）。
    pub pos_1y: Option<f64>,
    pub pos_3y: Option<f64>,
    /// 全样本（可用历史）区间位置。
    pub pos_all: Option<f64>,
    /// 收盘价在窗口内的分位（0=最低收盘附近）。
    pub pctile_1y: Option<f64>,
    pub pctile_3y: Option<f64>,
    pub pctile_all: Option<f64>,
    /// 相对窗口最高价的回撤，负数，如 -0.45 = 距高点 -45%。
    pub dd_1y: Option<f64>,
    pub dd_3y: Option<f64>,
    pub dd_all: Option<f64>,
    /// 近 20 日均量（手，与行情源一致）。
    pub avg_vol_20: Option<f64>,
    /// 0–100，越高越「像历史低位宝藏」。
    pub score: f64,
    pub tags: Vec<TreasureTag>,
    pub source: String,
}

/// 可解释标签（UI 展示用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreasureTag {
    /// 全样本接近区间底部。
    MultiYearLow,
    /// 1Y 与 3Y 同时偏低。
    DualLow,
    /// 1Y 低但 3Y/全样本仍高：上行趋势中的回撤，不是多年底部。
    UptrendPullback,
    /// 距全样本高点回撤很深。
    DeepDrawdown,
    /// K 线历史偏短，结论置信度低。
    SampleShort,
    /// 名称含 ST。
    StRisk,
    /// 流动性偏弱。
    ThinLiquidity,
}

impl TreasureTag {
    pub fn label(self) -> &'static str {
        match self {
            Self::MultiYearLow => "多年低位",
            Self::DualLow => "1–3年双低",
            Self::UptrendPullback => "上行中继回撤",
            Self::DeepDrawdown => "深回撤",
            Self::SampleShort => "样本不足",
            Self::StRisk => "ST风险",
            Self::ThinLiquidity => "流动性弱",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::MultiYearLow => "全样本价格位置靠近区间底部",
            Self::DualLow => "近一年与近三年都处在相对低位",
            Self::UptrendPullback => "一年内偏低，但多年位置仍高，更像上涨中的回撤",
            Self::DeepDrawdown => "相对多年高点回撤幅度较大",
            Self::SampleShort => "可用历史较短，分数已降权",
            Self::StRisk => "名称含 ST，需单独风控",
            Self::ThinLiquidity => "近20日均量偏低",
        }
    }
}

/// 从日 K 计算多窗口指标与分数。`name` 用于 ST 检测。
pub fn analyze(code: &str, name: &str, candles: &[Candle], source: &str) -> Option<TreasureHit> {
    if candles.len() < MIN_BARS {
        return None;
    }
    let last = candles.last()?;
    let close = last.close;
    if !close.is_finite() || close <= 0.0 {
        return None;
    }

    let pos_1y = range_position(candles, BARS_1Y);
    let pos_3y = if candles.len() >= MIN_BARS_3Y {
        range_position(candles, BARS_3Y.min(candles.len()))
    } else {
        None
    };
    let pos_all = range_position(candles, candles.len());

    let pctile_1y = close_percentile(candles, BARS_1Y);
    let pctile_3y = if candles.len() >= MIN_BARS_3Y {
        close_percentile(candles, BARS_3Y.min(candles.len()))
    } else {
        None
    };
    let pctile_all = close_percentile(candles, candles.len());

    let dd_1y = drawdown_from_high(candles, BARS_1Y);
    let dd_3y = if candles.len() >= MIN_BARS_3Y {
        drawdown_from_high(candles, BARS_3Y.min(candles.len()))
    } else {
        None
    };
    let dd_all = drawdown_from_high(candles, candles.len());

    let avg_vol_20 = avg_volume(candles, 20);

    let mut tags = Vec::new();
    // A 股名称常见 `ST` / `*ST` / `S*ST`
    let is_st = name.to_ascii_uppercase().contains("ST");
    if is_st {
        tags.push(TreasureTag::StRisk);
    }
    if candles.len() < MIN_BARS_3Y {
        tags.push(TreasureTag::SampleShort);
    }
    if let Some(v) = avg_vol_20 {
        // A 股日量单位因源而异；用相对宽松阈值过滤僵尸股
        if v > 0.0 && v < 50_000.0 {
            tags.push(TreasureTag::ThinLiquidity);
        }
    }

    // 上行中继：短窗低、长窗仍高
    let uptrend_pullback = match (pos_1y, pos_3y.or(pos_all)) {
        (Some(p1), Some(pl)) if p1 <= 0.18 && pl >= 0.45 => true,
        _ => false,
    };
    if uptrend_pullback {
        tags.push(TreasureTag::UptrendPullback);
    }

    let multi_year_low = pos_all.is_some_and(|p| p <= 0.15)
        && pos_3y.map(|p| p <= 0.28).unwrap_or(true)
        && !uptrend_pullback;
    if multi_year_low {
        tags.push(TreasureTag::MultiYearLow);
    }

    let dual_low = matches!((pos_1y, pos_3y), (Some(a), Some(b)) if a <= 0.15 && b <= 0.28);
    if dual_low {
        tags.push(TreasureTag::DualLow);
    }

    if dd_all.is_some_and(|d| d <= -0.50) {
        tags.push(TreasureTag::DeepDrawdown);
    }

    let score = composite_score(
        pos_1y,
        pos_3y,
        pos_all,
        pctile_1y,
        pctile_3y,
        pctile_all,
        dd_all,
        &tags,
    );

    Some(TreasureHit {
        code: code.to_string(),
        name: name.to_string(),
        close,
        bars: candles.len(),
        as_of: last.date.to_string(),
        pos_1y,
        pos_3y,
        pos_all,
        pctile_1y,
        pctile_3y,
        pctile_all,
        dd_1y,
        dd_3y,
        dd_all,
        avg_vol_20,
        score,
        tags,
        source: source.to_string(),
    })
}

/// 合成分数 0–100。
///
/// **长窗口权重大于短窗口**，避免「两年大涨后一年回调」被刷成高分。
fn composite_score(
    pos_1y: Option<f64>,
    pos_3y: Option<f64>,
    pos_all: Option<f64>,
    pctile_1y: Option<f64>,
    pctile_3y: Option<f64>,
    pctile_all: Option<f64>,
    dd_all: Option<f64>,
    tags: &[TreasureTag],
) -> f64 {
    // (weight, low_is_good value in 0..1)
    let mut parts: Vec<(f64, f64)> = Vec::new();

    // 位置：越低越好 → 贡献 (1 - pos)
    if let Some(p) = pos_1y {
        parts.push((0.12, 1.0 - p.clamp(0.0, 1.0)));
    }
    if let Some(p) = pos_3y {
        parts.push((0.32, 1.0 - p.clamp(0.0, 1.0)));
    }
    if let Some(p) = pos_all {
        parts.push((0.22, 1.0 - p.clamp(0.0, 1.0)));
    }
    // 收盘分位：比 pure high-low 更抗单日插针
    if let Some(p) = pctile_1y {
        parts.push((0.06, 1.0 - p.clamp(0.0, 1.0)));
    }
    if let Some(p) = pctile_3y {
        parts.push((0.14, 1.0 - p.clamp(0.0, 1.0)));
    }
    if let Some(p) = pctile_all {
        parts.push((0.14, 1.0 - p.clamp(0.0, 1.0)));
    }

    if parts.is_empty() {
        return 0.0;
    }
    let w_sum: f64 = parts.iter().map(|(w, _)| w).sum();
    let mut score: f64 = parts.iter().map(|(w, v)| w * v).sum::<f64>() / w_sum * 100.0;

    // 深回撤轻微加分（已在位置里体现，只做小 bonus）
    if let Some(dd) = dd_all {
        if dd <= -0.60 {
            score += 4.0;
        } else if dd <= -0.40 {
            score += 2.0;
        }
    }

    // 标签调整：上行中继大幅降权（核心）
    if tags.contains(&TreasureTag::UptrendPullback) {
        score *= 0.48;
    }
    if tags.contains(&TreasureTag::SampleShort) {
        score *= 0.75;
    }
    if tags.contains(&TreasureTag::StRisk) {
        score *= 0.55;
    }
    if tags.contains(&TreasureTag::ThinLiquidity) {
        score *= 0.85;
    }
    // 真正的多年低位略抬升排序
    if tags.contains(&TreasureTag::MultiYearLow) {
        score = (score + 6.0).min(100.0);
    }
    if tags.contains(&TreasureTag::DualLow) {
        score = (score + 3.0).min(100.0);
    }

    score.clamp(0.0, 100.0)
}

/// 当前收盘在最近 `n` 根的 [min low, max high] 中的位置。
pub fn range_position(candles: &[Candle], n: usize) -> Option<f64> {
    let window = tail(candles, n)?;
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for c in window {
        if c.low.is_finite() {
            lo = lo.min(c.low);
        }
        if c.high.is_finite() {
            hi = hi.max(c.high);
        }
    }
    let close = window.last()?.close;
    if !close.is_finite() || hi <= lo || lo == f64::MAX {
        return None;
    }
    Some(((close - lo) / (hi - lo)).clamp(0.0, 1.0))
}

/// 当前收盘在窗口收盘价中的分位（0 = 最低，1 = 最高）。
pub fn close_percentile(candles: &[Candle], n: usize) -> Option<f64> {
    let window = tail(candles, n)?;
    let close = window.last()?.close;
    if !close.is_finite() {
        return None;
    }
    let mut closes: Vec<f64> = window
        .iter()
        .map(|c| c.close)
        .filter(|v| v.is_finite())
        .collect();
    if closes.len() < 2 {
        return None;
    }
    closes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // 小于 close 的比例
    let below = closes.iter().filter(|&&v| v < close).count() as f64;
    let equal = closes.iter().filter(|&&v| (v - close).abs() < 1e-9).count() as f64;
    let rank = (below + 0.5 * equal) / closes.len() as f64;
    Some(rank.clamp(0.0, 1.0))
}

/// close / max(high) - 1。
pub fn drawdown_from_high(candles: &[Candle], n: usize) -> Option<f64> {
    let window = tail(candles, n)?;
    let peak = window
        .iter()
        .map(|c| c.high)
        .filter(|v| v.is_finite())
        .fold(f64::MIN, f64::max);
    let close = window.last()?.close;
    if !close.is_finite() || peak <= 0.0 || peak == f64::MIN {
        return None;
    }
    Some(close / peak - 1.0)
}

fn avg_volume(candles: &[Candle], n: usize) -> Option<f64> {
    let window = tail(candles, n)?;
    if window.is_empty() {
        return None;
    }
    let sum: u64 = window.iter().map(|c| c.volume).sum();
    Some(sum as f64 / window.len() as f64)
}

fn tail(candles: &[Candle], n: usize) -> Option<&[Candle]> {
    if candles.is_empty() || n == 0 {
        return None;
    }
    let n = n.min(candles.len());
    if n < MIN_BARS.min(candles.len()) && candles.len() < MIN_BARS {
        return None;
    }
    // 对 1Y 窗口：若总长 >= MIN_BARS 但不足 252，仍用全部（并在上层标 SampleShort）
    let start = candles.len().saturating_sub(n);
    Some(&candles[start..])
}

/// 格式化区间位置为百分比文案，如 `12%`（越低越接近底部）。
pub fn fmt_pos(p: Option<f64>) -> String {
    match p {
        Some(v) => format!("{:.0}%", v * 100.0),
        None => "—".into(),
    }
}

/// 格式化回撤，如 `-42%`。
pub fn fmt_dd(d: Option<f64>) -> String {
    match d {
        Some(v) => format!("{:.0}%", v * 100.0),
        None => "—".into(),
    }
}

/// 扫描结果缓存文件结构。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TreasureCache {
    pub updated_at: String,
    pub universe: String,
    pub hits: Vec<TreasureHit>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::shared;

    fn c(close: f64, high: f64, low: f64) -> Candle {
        Candle {
            date: shared("2020-01-01"),
            open: close,
            high,
            low,
            close,
            volume: 100_000,
        }
    }

    /// 构造：多年大涨后近端回撤 —— 典型「上行中继」（1Y 近底、3Y 仍高）。
    fn uptrend_pullback_series() -> Vec<Candle> {
        let mut v = Vec::new();
        // 600 根：10 → 100 的上行
        for i in 0..600 {
            let px = 10.0 + (i as f64) * (90.0 / 599.0);
            v.push(c(px, px * 1.01, px * 0.99));
        }
        // 最近 120 根：100 → 68（近一年窗口内贴近低点，全样本仍处高位）
        for i in 0..120 {
            let px = 100.0 - (i as f64) * (32.0 / 119.0);
            v.push(c(px, px * 1.01, px * 0.99));
        }
        v
    }

    /// 构造：从高位阴跌到低位 —— 多年低位。
    fn multi_year_low_series() -> Vec<Candle> {
        let mut v = Vec::new();
        for i in 0..800 {
            let px = 50.0 - (i as f64) * (40.0 / 799.0); // 50 → 10
            v.push(c(px, px * 1.02, px * 0.98));
        }
        v
    }

    #[test]
    fn range_position_at_bottom() {
        let candles: Vec<_> = (0..200)
            .map(|i| {
                let px = 10.0 + (i as f64) * 0.1;
                c(px, px + 0.5, px - 0.5)
            })
            .collect();
        // force last near low of window by appending a dump
        let mut candles = candles;
        candles.push(c(10.0, 10.2, 9.8));
        let p = range_position(&candles, 200).unwrap();
        assert!(p < 0.15, "pos={p}");
    }

    #[test]
    fn uptrend_pullback_tagged_and_downweighted() {
        let series = uptrend_pullback_series();
        let hit = analyze("TEST01", "测试上行", &series, "test").expect("hit");
        assert!(
            hit.tags.contains(&TreasureTag::UptrendPullback),
            "tags={:?}",
            hit.tags
        );
        // 1Y 可能偏低
        assert!(hit.pos_1y.unwrap() < 0.5);
        // 3Y/all 应仍偏高
        if let Some(p3) = hit.pos_3y {
            assert!(p3 > 0.4, "pos_3y={p3}");
        }
        // 分数不应虚高
        assert!(hit.score < 55.0, "score={}", hit.score);
    }

    #[test]
    fn multi_year_low_scores_higher() {
        let low = analyze("LOW001", "低位股", &multi_year_low_series(), "test").unwrap();
        let mid = analyze("UP001", "上行股", &uptrend_pullback_series(), "test").unwrap();
        assert!(
            low.score > mid.score + 10.0,
            "low={} mid={}",
            low.score,
            mid.score
        );
        assert!(low.tags.contains(&TreasureTag::MultiYearLow) || low.pos_all.unwrap() < 0.2);
    }

    #[test]
    fn st_penalty() {
        let series = multi_year_low_series();
        let normal = analyze("600001", "普通股", &series, "t").unwrap();
        let st = analyze("600002", "*ST测试", &series, "t").unwrap();
        assert!(st.tags.contains(&TreasureTag::StRisk));
        assert!(st.score < normal.score);
    }
}
