//! 寻宝榜 → 批量「可买观察」筛分。
//!
//! 流程：取寻宝 Top-N → 再拉日 K → 结合低位标签 / 策略雷达 / 参考价位，
//! 给出可解释的 **buy_score** 与结论（可关注 / 观察 / 暂不），
//! 避免用户一只只点开看。
//!
//! 排序与入选由本地规则决定（可复现）；可选 LLM 只生成整榜摘要，不重排。

use serde::Serialize;

use crate::data::ai::{self, AiConfig, AiSnapshot};
use crate::data::levels::{self, ReferenceLevels};
use crate::data::treasure::{TreasureHit, TreasureTag};
use crate::model::Candle;

/// 从寻宝榜取多少只做二次深评（控制耗时与请求量）。
pub const SCOUT_CANDIDATE_N: usize = 20;
/// 最终展示的「可关注 + 观察」上限。
pub const SCOUT_RESULT_N: usize = 10;
/// buy_score ≥ 此值 → 可关注建仓观察。
pub const SCORE_BUY_WATCH: f64 = 62.0;
/// buy_score ≥ 此值 → 观察名单（未达可关注）。
pub const SCORE_WATCH: f64 = 48.0;

/// 筛分结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoutVerdict {
    /// 可关注：低位质量 + 技术面未明显恶化，有参考建仓带。
    BuyWatch,
    /// 观察：有亮点但风险或位置一般。
    Watch,
    /// 暂不：假低位 / ST / 过热等。
    Skip,
}

impl ScoutVerdict {
    pub fn label(self) -> &'static str {
        match self {
            Self::BuyWatch => "符合策略",
            Self::Watch => "等待触发",
            Self::Skip => "不符合",
        }
    }

    pub fn rank_key(self) -> u8 {
        match self {
            Self::BuyWatch => 0,
            Self::Watch => 1,
            Self::Skip => 2,
        }
    }
}

/// 单只股票的搜罗结果（可序列化，便于 LLM 摘要与缓存）。
#[derive(Debug, Clone, Serialize)]
pub struct ScoutPick {
    pub code: String,
    pub name: String,
    /// 寻宝低位分 0–100。
    pub treasure_score: f64,
    /// 可买观察分 0–100（本地规则）。
    pub buy_score: f64,
    pub verdict: ScoutVerdict,
    pub close: f64,
    pub buy_low: f64,
    pub buy_high: f64,
    pub sell_low: f64,
    pub sell_high: f64,
    pub regime: String,
    pub rsi14: Option<f64>,
    pub tags: Vec<String>,
    pub reasons: Vec<String>,
    pub risks: Vec<String>,
    /// 一行摘要（列表展示）。
    pub headline: String,
}

impl ScoutPick {
    pub fn buy_band_text(&self) -> String {
        format!("{} – {}", fmt_px(self.buy_low), fmt_px(self.buy_high))
    }

    pub fn sell_band_text(&self) -> String {
        format!("{} – {}", fmt_px(self.sell_low), fmt_px(self.sell_high))
    }
}

/// 对「寻宝命中 + 日 K」做可买观察评估。
pub fn evaluate(hit: &TreasureHit, candles: &[Candle]) -> Option<ScoutPick> {
    if candles.len() < 30 {
        return None;
    }
    let snap = ai::build_snapshot(candles, &hit.code, &hit.name)?;
    let levels = levels::compute(candles)?;
    Some(score_pick(hit, &snap, &levels))
}

fn score_pick(hit: &TreasureHit, snap: &AiSnapshot, levels: &ReferenceLevels) -> ScoutPick {
    let mut score = hit.score * 0.45;
    let mut reasons = Vec::new();
    let mut risks = Vec::new();

    // —— 低位质量标签 ——
    if hit.tags.contains(&TreasureTag::MultiYearLow) {
        score += 14.0;
        reasons.push("多年低位".into());
    }
    if hit.tags.contains(&TreasureTag::DualLow) {
        score += 8.0;
        reasons.push("1–3年双低".into());
    }
    if hit.tags.contains(&TreasureTag::DeepDrawdown) {
        score += 5.0;
        reasons.push("深回撤".into());
    }
    if hit.tags.contains(&TreasureTag::UptrendPullback) {
        score -= 22.0;
        risks.push("上行中继回撤（假低位风险）".into());
    }
    if hit.tags.contains(&TreasureTag::StRisk) {
        score -= 28.0;
        risks.push("ST 风险".into());
    }
    if hit.tags.contains(&TreasureTag::ThinLiquidity) {
        score -= 12.0;
        risks.push("流动性偏弱".into());
    }
    if hit.tags.contains(&TreasureTag::SampleShort) {
        score -= 8.0;
        risks.push("样本偏短".into());
    }

    // —— 策略雷达 / 动量 ——
    match snap.regime.as_str() {
        "强势" | "偏强" => {
            score += 6.0;
            reasons.push(format!("技术面{}", snap.regime));
        }
        "中性" => {
            score += 2.0;
        }
        "偏弱" => {
            score -= 4.0;
            risks.push("技术面偏弱".into());
        }
        "防守" => {
            score -= 10.0;
            risks.push("技术面防守".into());
        }
        _ => {}
    }

    if let Some(rsi) = snap.rsi14 {
        if rsi <= 35.0 {
            score += 8.0;
            reasons.push(format!("RSI 偏低({rsi:.0})"));
        } else if rsi <= 45.0 {
            score += 3.0;
        } else if rsi >= 70.0 {
            score -= 12.0;
            risks.push(format!("RSI 偏高({rsi:.0})"));
        } else if rsi >= 60.0 {
            score -= 4.0;
        }
    }

    // 价格相对参考建仓带
    let close = levels.close;
    let mid_buy = (levels.buy_low + levels.buy_high) / 2.0;
    if close <= levels.buy_high * 1.01 {
        score += 10.0;
        reasons.push("现价贴近参考建仓带".into());
    } else if close <= levels.buy_high * 1.04 {
        score += 4.0;
        reasons.push("现价略高于建仓上沿".into());
    } else if close > mid_buy * 1.12 {
        score -= 8.0;
        risks.push("现价明显高于建仓带".into());
    }

    // 近端位置：60 日区间低位更好（寻宝场景）
    if let Some(pos) = snap.range_position_60_pct {
        if pos <= 25.0 {
            score += 6.0;
            reasons.push("近60日仍处低位".into());
        } else if pos >= 80.0 {
            score -= 8.0;
            risks.push("近60日已处高位".into());
        }
    }

    if snap.near_20d_high {
        score -= 6.0;
        risks.push("贴近20日高点".into());
    }

    // 均线：空头排列略减分（低位抄底允许，但不给加成）
    match snap.ma_alignment {
        ai::MaAlignment::Bullish => {
            score += 3.0;
            reasons.push("均线偏多".into());
        }
        ai::MaAlignment::Bearish => {
            score -= 3.0;
        }
        ai::MaAlignment::Mixed => {}
    }

    let buy_score = score.clamp(0.0, 100.0);
    let verdict = if hit.tags.contains(&TreasureTag::StRisk) {
        ScoutVerdict::Skip
    } else if buy_score >= SCORE_BUY_WATCH && !hit.tags.contains(&TreasureTag::UptrendPullback) {
        ScoutVerdict::BuyWatch
    } else if buy_score >= SCORE_WATCH {
        ScoutVerdict::Watch
    } else {
        ScoutVerdict::Skip
    };

    // 上行中继即使分数够也最多「观察」
    let verdict =
        if hit.tags.contains(&TreasureTag::UptrendPullback) && verdict == ScoutVerdict::BuyWatch {
            ScoutVerdict::Watch
        } else {
            verdict
        };

    let tags: Vec<String> = hit.tags.iter().map(|t| t.label().to_string()).collect();
    let headline = match verdict {
        ScoutVerdict::BuyWatch => format!(
            "符合策略 · 参考观察区间 {} 元 · 位置{:.0}/匹配{:.0}",
            format_band(levels.buy_low, levels.buy_high),
            hit.score,
            buy_score
        ),
        ScoutVerdict::Watch => format!(
            "等待触发 · 参考观察区间 {} 元 · 位置{:.0}/匹配{:.0}",
            format_band(levels.buy_low, levels.buy_high),
            hit.score,
            buy_score
        ),
        ScoutVerdict::Skip => format!(
            "不符合 · 位置{:.0}/匹配{:.0}{}",
            hit.score,
            buy_score,
            risks.first().map(|r| format!(" · {r}")).unwrap_or_default()
        ),
    };

    if reasons.is_empty() {
        reasons.push("综合低位与技术快照".into());
    }

    ScoutPick {
        code: hit.code.clone(),
        name: hit.name.clone(),
        treasure_score: hit.score,
        buy_score,
        verdict,
        close: levels.close,
        buy_low: levels.buy_low,
        buy_high: levels.buy_high,
        sell_low: levels.sell_low,
        sell_high: levels.sell_high,
        regime: snap.regime.clone(),
        rsi14: snap.rsi14,
        tags,
        reasons,
        risks,
        headline,
    }
}

/// 排序：可关注 > 观察 > 暂不，同分按 buy_score、treasure_score。
pub fn sort_picks(picks: &mut [ScoutPick]) {
    picks.sort_by(|a, b| {
        a.verdict
            .rank_key()
            .cmp(&b.verdict.rank_key())
            .then(
                b.buy_score
                    .partial_cmp(&a.buy_score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(
                b.treasure_score
                    .partial_cmp(&a.treasure_score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
}

/// 只保留可关注 + 观察，截断到 `SCOUT_RESULT_N`。
pub fn finalize_results(mut picks: Vec<ScoutPick>) -> Vec<ScoutPick> {
    sort_picks(&mut picks);
    picks.retain(|p| p.verdict != ScoutVerdict::Skip);
    if picks.len() > SCOUT_RESULT_N {
        picks.truncate(SCOUT_RESULT_N);
    }
    picks
}

/// 本地整榜摘要（无 LLM 时使用）。列表已展示明细，摘要保持短可扫。
pub fn local_summary(picks: &[ScoutPick]) -> String {
    if picks.is_empty() {
        return "本轮未筛出可关注/观察：多为上行中继、ST 或过热。可换池重扫或放宽财务过滤。".into();
    }
    let buy_n = picks
        .iter()
        .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
        .count();
    let watch_n = picks
        .iter()
        .filter(|p| p.verdict == ScoutVerdict::Watch)
        .count();
    // 优先点名可关注 Top3（一行一条，方便菜单栏式扫读）
    let tops: Vec<String> = picks
        .iter()
        .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
        .take(3)
        .map(|p| {
            format!(
                "{} {} 建仓{}",
                p.code,
                short_name(&p.name, &p.code),
                p.buy_band_text()
            )
        })
        .collect();
    let head = if tops.is_empty() {
        format!("暂无「可关注」· {watch_n} 只观察（可切「全部」查看）")
    } else {
        format!("优先：{}", tops.join("；"))
    };
    format!(
        "{head}\n可关注 {buy_n} · 观察 {watch_n} · 点列表看图与价位 · 仅供学习研究，不构成投资建议。"
    )
}

/// 可选：一次 LLM 调用生成整榜中文摘要（不改排序）。
pub fn llm_summary(cfg: &AiConfig, picks: &[ScoutPick]) -> anyhow::Result<String> {
    if picks.is_empty() {
        anyhow::bail!("没有可摘要的候选");
    }
    // 紧凑载荷，控制 token
    #[derive(Serialize)]
    struct Row<'a> {
        code: &'a str,
        name: &'a str,
        verdict: &'a str,
        buy_score: f64,
        treasure_score: f64,
        close: f64,
        buy_low: f64,
        buy_high: f64,
        sell_low: f64,
        sell_high: f64,
        regime: &'a str,
        reasons: &'a [String],
        risks: &'a [String],
    }
    let rows: Vec<Row<'_>> = picks
        .iter()
        .map(|p| Row {
            code: &p.code,
            name: &p.name,
            verdict: p.verdict.label(),
            buy_score: (p.buy_score * 10.0).round() / 10.0,
            treasure_score: (p.treasure_score * 10.0).round() / 10.0,
            close: p.close,
            buy_low: p.buy_low,
            buy_high: p.buy_high,
            sell_low: p.sell_low,
            sell_high: p.sell_high,
            regime: &p.regime,
            reasons: &p.reasons,
            risks: &p.risks,
        })
        .collect();
    let body = serde_json::to_string(&rows)?;
    let user = format!(
        "以下是本地程序从 A 股历史低位池中筛出的「可买观察」清单 JSON（已按优先级排好，请勿重排）：\n\
         ```json\n{body}\n```\n\
         请用中文写一份搜罗简报：\n\
         1) 先点名最多 3 只最值得优先关注的代码及参考建仓价（用 JSON 里的 buy_low–buy_high）；\n\
         2) 概括共性风险（假低位、流动性、过热等）；\n\
         3) 不要编造清单外的股票或数据；\n\
         4) 不超过 350 字；结尾必须含“不构成投资建议”。"
    );

    const SYSTEM: &str = "你是 A 股技术面搜罗助手。你只根据给定 JSON 清单写简报，\
不编造标的与数值，不给出具体仓位或收益承诺，强调观察价位非交易指令。";

    // 与个股点评共用 API / CLI 完成层。
    ai::llm_complete(cfg, SYSTEM, &user)
}

fn format_band(lo: f64, hi: f64) -> String {
    format!("{}–{}", fmt_px(lo), fmt_px(hi))
}

fn fmt_px(v: f64) -> String {
    if v >= 1000.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

fn short_name(name: &str, code: &str) -> String {
    if name.is_empty() || name == code {
        return code.to_string();
    }
    name.chars().take(6).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::treasure;
    use crate::model::shared;

    fn c(close: f64, high: f64, low: f64) -> Candle {
        Candle {
            date: shared("d"),
            open: close,
            high,
            low,
            close,
            volume: 200_000,
        }
    }

    /// 阴跌至低位：应更容易进入可关注/观察。
    fn multi_year_low_series() -> Vec<Candle> {
        let mut v = Vec::new();
        for i in 0..500 {
            let px = 100.0 - (i as f64) * (70.0 / 499.0);
            v.push(c(px, px * 1.01, px * 0.99));
        }
        // 近端磨底
        for i in 0..80 {
            let px = 30.0 + (i as f64 % 5.0) * 0.2;
            v.push(c(px, px * 1.01, px * 0.99));
        }
        v
    }

    fn uptrend_pullback_series() -> Vec<Candle> {
        let mut v = Vec::new();
        for i in 0..600 {
            let px = 10.0 + (i as f64) * (90.0 / 599.0);
            v.push(c(px, px * 1.01, px * 0.99));
        }
        for i in 0..120 {
            let px = 100.0 - (i as f64) * (32.0 / 119.0);
            v.push(c(px, px * 1.01, px * 0.99));
        }
        v
    }

    #[test]
    fn multi_year_low_outranks_pullback() {
        let low = multi_year_low_series();
        let pb = uptrend_pullback_series();
        let hit_low = treasure::analyze("000001", "测试低位", &low, "test").unwrap();
        let hit_pb = treasure::analyze("000002", "测试中继", &pb, "test").unwrap();
        let pick_low = evaluate(&hit_low, &low).unwrap();
        let pick_pb = evaluate(&hit_pb, &pb).unwrap();
        assert!(
            pick_low.buy_score > pick_pb.buy_score,
            "low={} pb={}",
            pick_low.buy_score,
            pick_pb.buy_score
        );
        assert_ne!(pick_pb.verdict, ScoutVerdict::BuyWatch);
    }

    #[test]
    fn finalize_drops_skips_and_limits() {
        let mut picks = vec![
            ScoutPick {
                code: "1".into(),
                name: "a".into(),
                treasure_score: 80.0,
                buy_score: 70.0,
                verdict: ScoutVerdict::BuyWatch,
                close: 10.0,
                buy_low: 9.0,
                buy_high: 9.5,
                sell_low: 11.0,
                sell_high: 12.0,
                regime: "中性".into(),
                rsi14: Some(40.0),
                tags: vec![],
                reasons: vec![],
                risks: vec![],
                headline: String::new(),
            },
            ScoutPick {
                code: "2".into(),
                name: "b".into(),
                treasure_score: 50.0,
                buy_score: 30.0,
                verdict: ScoutVerdict::Skip,
                close: 10.0,
                buy_low: 9.0,
                buy_high: 9.5,
                sell_low: 11.0,
                sell_high: 12.0,
                regime: "防守".into(),
                rsi14: None,
                tags: vec![],
                reasons: vec![],
                risks: vec![],
                headline: String::new(),
            },
        ];
        let out = finalize_results(std::mem::take(&mut picks));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].code, "1");
    }

    #[test]
    fn local_summary_mentions_disclaimer() {
        let text = local_summary(&[]);
        assert!(text.contains("未筛出") || text.contains("不构成"));
    }
}
