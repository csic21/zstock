//! 短线策略雷达：强势回踩 / 放量突破 / 超跌反弹。
//!
//! 全部由本地日 K 推算，可解释、可复现；不预测涨跌。
//! 与寻宝鼠（长线低位）并列，组成「现在找」双引擎。

use serde::{Deserialize, Serialize};

use crate::data::ai::{self, AiSnapshot};
use crate::data::indicators::MaSeries;
use crate::model::Candle;

/// 单次短线扫描最多深评候选（控制耗时）。
pub const RADAR_PROBE_N: usize = 40;
/// 最终展示上限。
pub const RADAR_RESULT_N: usize = 15;
/// 拉取日 K 根数（短线不需要 4 年）。
pub const RADAR_KLINE_LIMIT: usize = 180;

/// 短线策略类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RadarStrategy {
    /// 站上中期均线后回踩缩量，等待再放量。
    Pullback,
    /// 突破近 20/60 日高点 + 量能确认。
    Breakout,
    /// 短线超卖后止跌，量能回升。
    OversoldBounce,
}

impl RadarStrategy {
    pub fn id(self) -> &'static str {
        match self {
            Self::Pullback => "pullback",
            Self::Breakout => "breakout",
            Self::OversoldBounce => "oversold",
        }
    }

    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Pullback, true) => "Pullback",
            (Self::Pullback, false) => "强势回踩",
            (Self::Breakout, true) => "Breakout",
            (Self::Breakout, false) => "放量突破",
            (Self::OversoldBounce, true) => "Oversold",
            (Self::OversoldBounce, false) => "超跌反弹",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Pullback => "MA20 上方回踩不破，量能收敛",
            Self::Breakout => "突破 20 日高 + 量比抬升",
            Self::OversoldBounce => "RSI 低位 + 止跌 K + 量能回升（博弈）",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Pullback, Self::Breakout, Self::OversoldBounce]
    }
}

/// 单只短线雷达命中。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadarHit {
    pub code: String,
    pub name: String,
    pub strategy: RadarStrategy,
    /// 0–100 策略匹配分。
    pub score: f64,
    pub close: f64,
    pub change_pct: f64,
    pub rsi14: Option<f64>,
    pub volume_ratio_20: Option<f64>,
    pub regime: String,
    pub reasons: Vec<String>,
    pub risks: Vec<String>,
    /// 列表一行摘要。
    pub headline: String,
    /// 参考观察下沿（回踩/超跌用支撑，突破用现价附近）。
    pub watch_low: f64,
    /// 参考观察上沿。
    pub watch_high: f64,
}

impl RadarHit {
    pub fn watch_band_text(&self) -> String {
        format!("{:.2} – {:.2}", self.watch_low, self.watch_high)
    }
}

/// 短线雷达缓存。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RadarCache {
    pub updated_at: String,
    pub universe: String,
    pub hits: Vec<RadarHit>,
}

/// 对单只标的评估三条策略，返回匹配分最高且 ≥ 阈值的一条。
pub fn evaluate(
    code: &str,
    name: &str,
    candles: &[Candle],
    day_change_pct: f64,
) -> Option<RadarHit> {
    if candles.len() < 40 {
        return None;
    }
    let snap = ai::build_snapshot(candles, code, name)?;
    let mut candidates = Vec::with_capacity(3);
    if let Some(h) = score_pullback(code, name, candles, &snap, day_change_pct) {
        candidates.push(h);
    }
    if let Some(h) = score_breakout(code, name, candles, &snap, day_change_pct) {
        candidates.push(h);
    }
    if let Some(h) = score_oversold(code, name, candles, &snap, day_change_pct) {
        candidates.push(h);
    }
    candidates.into_iter().max_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// 评估指定策略（过滤 UI 用）。
pub fn evaluate_strategy(
    code: &str,
    name: &str,
    candles: &[Candle],
    day_change_pct: f64,
    strategy: RadarStrategy,
) -> Option<RadarHit> {
    if candles.len() < 40 {
        return None;
    }
    let snap = ai::build_snapshot(candles, code, name)?;
    match strategy {
        RadarStrategy::Pullback => score_pullback(code, name, candles, &snap, day_change_pct),
        RadarStrategy::Breakout => score_breakout(code, name, candles, &snap, day_change_pct),
        RadarStrategy::OversoldBounce => {
            score_oversold(code, name, candles, &snap, day_change_pct)
        }
    }
}

fn score_pullback(
    code: &str,
    name: &str,
    candles: &[Candle],
    snap: &AiSnapshot,
    day_change_pct: f64,
) -> Option<RadarHit> {
    let last = candles.last()?;
    let ma = MaSeries::from_candles(candles);
    let ix = candles.len() - 1;
    let (_m5, _m10, ma20, ma60) = ma.value_at(ix);
    let ma20 = ma20?;
    let close = last.close;
    if close <= 0.0 || !close.is_finite() {
        return None;
    }

    let mut score: f64 = 40.0;
    let mut reasons = Vec::new();
    let mut risks = Vec::new();

    // 站上 MA20
    let dist_ma20 = close / ma20 - 1.0;
    if dist_ma20 < -0.02 {
        return None; // 明显在均线下方，不是回踩
    }
    if dist_ma20 >= 0.0 && dist_ma20 <= 0.06 {
        score += 18.0;
        reasons.push("价格贴近/略上 MA20".into());
    } else if dist_ma20 > 0.06 && dist_ma20 <= 0.12 {
        score += 8.0;
        reasons.push("仍在 MA20 上方".into());
    } else if dist_ma20 > 0.12 {
        score -= 6.0;
        risks.push("距 MA20 偏远，回踩未到位".into());
    }

    if let Some(ma60) = ma60 {
        if ma20 > ma60 * 1.002 {
            score += 10.0;
            reasons.push("MA20>MA60 中期偏多".into());
        } else if close < ma60 {
            score -= 8.0;
            risks.push("跌破 MA60".into());
        }
    }

    // 量能：回踩宜缩量
    if let Some(vr) = snap.volume_ratio_20 {
        if vr <= 0.9 {
            score += 10.0;
            reasons.push("回踩量能收敛".into());
        } else if vr >= 1.4 && day_change_pct < 0.0 {
            score -= 8.0;
            risks.push("放量下跌，回踩质量差".into());
        }
    }

    if let Some(rsi) = snap.rsi14 {
        if (40.0..62.0).contains(&rsi) {
            score += 8.0;
            reasons.push(format!("RSI {rsi:.0} 中性偏强"));
        } else if rsi >= 72.0 {
            score -= 10.0;
            risks.push("RSI 偏热".into());
        }
    }

    match snap.regime.as_str() {
        "强势" | "偏强" => {
            score += 8.0;
            reasons.push(format!("技术面{}", snap.regime));
        }
        "防守" | "偏弱" => {
            score -= 12.0;
            risks.push(format!("技术面{}", snap.regime));
        }
        _ => {}
    }

    if day_change_pct <= -3.5 {
        score -= 8.0;
        risks.push("当日跌幅偏大".into());
    }

    score = score.clamp(0.0, 100.0);
    if score < 52.0 {
        return None;
    }

    let watch_low = (ma20 * 0.985).min(close * 0.97);
    let watch_high = close.max(ma20 * 1.02);

    Some(RadarHit {
        code: code.into(),
        name: name.into(),
        strategy: RadarStrategy::Pullback,
        score,
        close,
        change_pct: day_change_pct,
        rsi14: snap.rsi14,
        volume_ratio_20: snap.volume_ratio_20,
        regime: snap.regime.clone(),
        headline: format!(
            "回踩 · 分{:.0} · {}",
            score,
            reasons.first().map(|s| s.as_str()).unwrap_or("MA20 附近")
        ),
        reasons,
        risks,
        watch_low,
        watch_high,
    })
}

fn score_breakout(
    code: &str,
    name: &str,
    candles: &[Candle],
    snap: &AiSnapshot,
    day_change_pct: f64,
) -> Option<RadarHit> {
    let last = candles.last()?;
    let close = last.close;
    if close <= 0.0 {
        return None;
    }

    let high_20 = window_high(candles, 20)?;
    let high_60 = window_high(candles, 60.min(candles.len()));
    // 今日接近或突破 20 日高
    let near_20 = close >= high_20 * 0.995;
    if !near_20 && day_change_pct < 1.0 {
        return None;
    }

    let mut score: f64 = 42.0;
    let mut reasons = Vec::new();
    let mut risks = Vec::new();

    if close >= high_20 {
        score += 16.0;
        reasons.push("收盘突破/站上 20 日高".into());
    } else if near_20 {
        score += 10.0;
        reasons.push("逼近 20 日高".into());
    }

    if let Some(h60) = high_60 {
        if close >= h60 * 0.998 {
            score += 10.0;
            reasons.push("同步挑战 60 日高".into());
        }
    }

    if let Some(vr) = snap.volume_ratio_20 {
        if vr >= 1.25 {
            score += 14.0;
            reasons.push(format!("量比 {vr:.2} 放量确认"));
        } else if vr < 0.85 {
            score -= 10.0;
            risks.push("突破量能不足".into());
        }
    }

    if day_change_pct >= 2.0 {
        score += 8.0;
        reasons.push(format!("当日 {day_change_pct:+.1}%"));
    } else if day_change_pct < 0.0 {
        score -= 6.0;
    }

    if let Some(rsi) = snap.rsi14 {
        if rsi >= 78.0 {
            score -= 12.0;
            risks.push("RSI 过热，追高风险".into());
        } else if (55.0..72.0).contains(&rsi) {
            score += 6.0;
        }
    }

    if snap.near_20d_high && day_change_pct > 5.0 {
        risks.push("涨幅已大，注意隔日回吐".into());
        score -= 4.0;
    }

    match snap.regime.as_str() {
        "强势" | "偏强" => score += 6.0,
        "防守" => {
            score -= 14.0;
            risks.push("整体偏防守".into());
        }
        _ => {}
    }

    score = score.clamp(0.0, 100.0);
    if score < 55.0 {
        return None;
    }

    let watch_low = close * 0.97;
    let watch_high = close * 1.05;

    Some(RadarHit {
        code: code.into(),
        name: name.into(),
        strategy: RadarStrategy::Breakout,
        score,
        close,
        change_pct: day_change_pct,
        rsi14: snap.rsi14,
        volume_ratio_20: snap.volume_ratio_20,
        regime: snap.regime.clone(),
        headline: format!(
            "突破 · 分{:.0} · {}",
            score,
            reasons.first().map(|s| s.as_str()).unwrap_or("放量")
        ),
        reasons,
        risks,
        watch_low,
        watch_high,
    })
}

fn score_oversold(
    code: &str,
    name: &str,
    candles: &[Candle],
    snap: &AiSnapshot,
    day_change_pct: f64,
) -> Option<RadarHit> {
    let last = candles.last()?;
    let close = last.close;
    if close <= 0.0 {
        return None;
    }

    let rsi = snap.rsi14?;
    // 需要超卖或刚离开超卖
    if rsi > 38.0 {
        return None;
    }

    let mut score: f64 = 38.0;
    let mut reasons = Vec::new();
    let mut risks = Vec::new();

    reasons.push(format!("RSI {rsi:.0} 超卖区"));
    score += ((35.0 - rsi) * 0.8).clamp(0.0, 16.0);

    // 止跌：收阳或下影
    let body_up = last.close >= last.open;
    let lower_wick = (last.open.min(last.close) - last.low).max(0.0);
    let range = (last.high - last.low).max(1e-9);
    if body_up {
        score += 10.0;
        reasons.push("当日收阳止跌".into());
    } else if lower_wick / range >= 0.35 {
        score += 8.0;
        reasons.push("下影线偏长".into());
    }

    if let Some(vr) = snap.volume_ratio_20 {
        if vr >= 1.1 && body_up {
            score += 10.0;
            reasons.push("止跌放量".into());
        } else if vr >= 1.4 && !body_up {
            score -= 8.0;
            risks.push("仍在放量下跌".into());
        }
    }

    if day_change_pct > 0.0 && day_change_pct < 6.0 {
        score += 6.0;
        reasons.push(format!("反弹 {day_change_pct:+.1}%"));
    }

    // 超跌策略风险更高
    risks.push("博弈反弹，失败率偏高".into());
    if matches!(snap.regime.as_str(), "防守" | "偏弱") {
        score -= 4.0;
        risks.push(format!("技术面{}", snap.regime));
    }

    score = score.clamp(0.0, 100.0);
    if score < 50.0 {
        return None;
    }

    let low_20 = window_low(candles, 20).unwrap_or(close * 0.95);
    let watch_low = low_20.min(close * 0.97);
    let watch_high = close * 1.04;

    Some(RadarHit {
        code: code.into(),
        name: name.into(),
        strategy: RadarStrategy::OversoldBounce,
        score,
        close,
        change_pct: day_change_pct,
        rsi14: snap.rsi14,
        volume_ratio_20: snap.volume_ratio_20,
        regime: snap.regime.clone(),
        headline: format!("超跌 · 分{score:.0} · RSI {rsi:.0}"),
        reasons,
        risks,
        watch_low,
        watch_high,
    })
}

fn window_high(candles: &[Candle], n: usize) -> Option<f64> {
    let start = candles.len().saturating_sub(n);
    candles[start..]
        .iter()
        .map(|c| c.high)
        .filter(|v| v.is_finite())
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

fn window_low(candles: &[Candle], n: usize) -> Option<f64> {
    let start = candles.len().saturating_sub(n);
    candles[start..]
        .iter()
        .map(|c| c.low)
        .filter(|v| v.is_finite() && *v > 0.0)
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

pub fn sort_hits(hits: &mut [RadarHit]) {
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.change_pct
                    .partial_cmp(&a.change_pct)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
}

/// 本地整榜摘要。
pub fn local_summary(hits: &[RadarHit]) -> String {
    if hits.is_empty() {
        return "短线雷达未找到同时满足策略与技术条件的标的；可换时段再扫，或先看板块热度。\
                 结果仅供学习研究，不构成投资建议。"
            .into();
    }
    let mut pull = 0;
    let mut brk = 0;
    let mut ovs = 0;
    for h in hits {
        match h.strategy {
            RadarStrategy::Pullback => pull += 1,
            RadarStrategy::Breakout => brk += 1,
            RadarStrategy::OversoldBounce => ovs += 1,
        }
    }
    let top = hits
        .iter()
        .take(3)
        .map(|h| format!("{}({})", h.name, h.strategy.label(false)))
        .collect::<Vec<_>>()
        .join("、");
    format!(
        "短线雷达命中 {} 只：回踩 {pull} · 突破 {brk} · 超跌 {ovs}。靠前观察：{top}。\
         突破注意追高，超跌注意失败回落；仅供学习研究，不构成投资建议。",
        hits.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::shared;

    fn bar(close: f64, high: f64, low: f64, vol: u64) -> Candle {
        Candle {
            date: shared("2024-01-01"),
            open: close * 0.99,
            high,
            low,
            close,
            volume: vol,
        }
    }

    #[test]
    fn breakout_needs_enough_bars() {
        let candles: Vec<_> = (0..50)
            .map(|i| {
                let px = 10.0 + i as f64 * 0.1;
                bar(px, px * 1.01, px * 0.99, 100_000)
            })
            .collect();
        // last bar punches high with volume
        let mut c = candles;
        if let Some(last) = c.last_mut() {
            last.high = last.close * 1.05;
            last.close = last.high;
            last.volume = 500_000;
        }
        let hit = evaluate("000001", "测试", &c, 3.0);
        // may or may not hit depending on RSI/regime; just ensure no panic
        let _ = hit;
    }
}
