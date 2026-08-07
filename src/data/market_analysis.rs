//! Explainable market sentiment and button-triggered A-share recommendations.
//!
//! The sentiment index is deliberately transparent rather than pretending to
//! be an official indicator. It combines stock breadth, sector breadth, major
//! index momentum, and the average sector move. Candidate stocks are first
//! screened locally from a liquid A-share universe and then passed through the
//! existing technical snapshot engine; an optional LLM may summarize the
//! resulting, closed candidate set.

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::ai::{self, AiConfig, AiSnapshot};
use super::eastmoney::{self, QuoteTick};
use super::market::{self, SectorTick};

/// A transparent 0–100 market sentiment snapshot.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FearGreedIndex {
    pub score: f64,
    pub label: &'static str,
    /// Stock breadth: advances + half of unchanged, divided by all stocks.
    pub stock_breadth: f64,
    /// Industry-index breadth.
    pub sector_breadth: f64,
    /// Average major-index move mapped into a 0–100 band.
    pub index_momentum: f64,
    /// Average industry move mapped into a 0–100 band.
    pub sector_momentum: f64,
}

impl FearGreedIndex {
    pub fn label_for_score(score: f64) -> &'static str {
        match score.clamp(0.0, 100.0) {
            s if s >= 80.0 => "极度贪婪",
            s if s >= 60.0 => "贪婪",
            s if s > 40.0 => "中性",
            s if s > 20.0 => "恐惧",
            _ => "极度恐惧",
        }
    }

    pub fn is_greed(self) -> bool {
        self.score >= 60.0
    }

    pub fn is_fear(self) -> bool {
        self.score <= 40.0
    }
}

/// Calculate the index from the same values shown in the market-analysis UI.
pub fn fear_greed_index(
    stock_advances: u64,
    stock_declines: u64,
    stock_unchanged: u64,
    sector_advances: usize,
    sector_declines: usize,
    sector_unchanged: usize,
    sector_average_change: Option<f64>,
    index_changes: &[f64],
) -> FearGreedIndex {
    let stock_breadth = breadth_score(stock_advances, stock_declines, stock_unchanged);
    let sector_breadth = breadth_score(
        sector_advances as u64,
        sector_declines as u64,
        sector_unchanged as u64,
    );
    let index_momentum = signed_move_score(
        index_changes.iter().copied().sum::<f64>() / index_changes.len().max(1) as f64,
    );
    let sector_momentum = signed_move_score(sector_average_change.unwrap_or(0.0));
    let score = (stock_breadth * 0.45
        + sector_breadth * 0.25
        + index_momentum * 0.20
        + sector_momentum * 0.10)
        .clamp(0.0, 100.0);

    FearGreedIndex {
        score,
        label: FearGreedIndex::label_for_score(score),
        stock_breadth,
        sector_breadth,
        index_momentum,
        sector_momentum,
    }
}

fn breadth_score(up: u64, down: u64, flat: u64) -> f64 {
    let total = up + down + flat;
    if total == 0 {
        50.0
    } else {
        ((up as f64 + flat as f64 * 0.5) / total as f64 * 100.0).clamp(0.0, 100.0)
    }
}

fn signed_move_score(change_pct: f64) -> f64 {
    (50.0 + change_pct.clamp(-5.0, 5.0) * 10.0).clamp(0.0, 100.0)
}

/// One index point included in the market-AI context.
#[derive(Debug, Clone, Serialize)]
pub struct MarketIndexPoint {
    pub name: String,
    pub last: f64,
    pub change_pct: f64,
}

/// A locally screened candidate. The LLM is not allowed to invent candidates
/// outside this list.
#[derive(Debug, Clone, Serialize)]
pub struct MarketPick {
    pub code: String,
    pub name: String,
    pub last: f64,
    pub change_pct: f64,
    pub score: f64,
    pub regime: String,
    pub rsi14: Option<f64>,
    pub momentum_20_pct: Option<f64>,
    pub volume_ratio_20: Option<f64>,
    pub reasons: Vec<String>,
    pub risks: Vec<String>,
}

/// Compact context shown to the local summary and optionally sent to an LLM.
#[derive(Debug, Clone, Serialize)]
pub struct MarketAnalysisContext {
    pub generated_at: String,
    pub fear_greed: FearGreedIndex,
    pub sector_total: usize,
    pub sector_advances: usize,
    pub sector_declines: usize,
    pub sector_unchanged: usize,
    pub sector_average_change: Option<f64>,
    pub stock_advances: u64,
    pub stock_declines: u64,
    pub stock_unchanged: u64,
    pub indices: Vec<MarketIndexPoint>,
    pub picks: Vec<MarketPick>,
}

/// Build one consistent context for both the UI sentiment card and the AI
/// prompt, so the displayed numbers and the generated explanation cannot drift.
pub fn build_context(
    generated_at: impl Into<String>,
    sectors: &[SectorTick],
    indices: Vec<MarketIndexPoint>,
    picks: Vec<MarketPick>,
) -> MarketAnalysisContext {
    let sector_total = sectors.len();
    let sector_advances = sectors.iter().filter(|s| s.change_pct > 0.0).count();
    let sector_declines = sectors.iter().filter(|s| s.change_pct < 0.0).count();
    let sector_unchanged = sector_total.saturating_sub(sector_advances + sector_declines);
    let stock_advances: u64 = sectors.iter().map(|s| s.advances).sum();
    let stock_declines: u64 = sectors.iter().map(|s| s.declines).sum();
    let stock_unchanged: u64 = sectors.iter().map(|s| s.unchanged).sum();
    let sector_average_change = if sector_total == 0 {
        None
    } else {
        Some(sectors.iter().map(|s| s.change_pct).sum::<f64>() / sector_total as f64)
    };
    let index_changes: Vec<f64> = indices.iter().map(|i| i.change_pct).collect();
    let fear_greed = fear_greed_index(
        stock_advances,
        stock_declines,
        stock_unchanged,
        sector_advances,
        sector_declines,
        sector_unchanged,
        sector_average_change,
        &index_changes,
    );

    MarketAnalysisContext {
        generated_at: generated_at.into(),
        fear_greed,
        sector_total,
        sector_advances,
        sector_declines,
        sector_unchanged,
        sector_average_change,
        stock_advances,
        stock_declines,
        stock_unchanged,
        indices,
        picks,
    }
}

/// Fetch and score a small, explainable set of current candidates.
pub fn fetch_market_picks(max_picks: usize) -> Result<Vec<MarketPick>> {
    let max_picks = max_picks.clamp(3, 8);
    let universe = eastmoney::fetch_liquid_a_shares(180).context("读取 A 股候选池失败")?;
    let codes: Vec<String> = universe.iter().map(|row| row.code.clone()).collect();
    let names: std::collections::HashMap<String, String> = universe
        .into_iter()
        .map(|row| (row.code, row.name))
        .collect();
    let sourced = market::fetch_quotes(&codes).context("读取候选股实时行情失败")?;
    let mut quotes: Vec<QuoteTick> = sourced
        .data
        .into_iter()
        .filter(|q| q.last > 0.0 && q.change_pct.is_finite() && q.change_pct > 0.0)
        .filter(|q| !q.name.to_ascii_uppercase().contains("ST"))
        .collect();

    quotes.sort_by(|a, b| {
        b.change_pct
            .partial_cmp(&a.change_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Probe a few more than the final display count so technical filtering can
    // remove overbought / incomplete candidates without leaving the panel thin.
    let probe_n = (max_picks * 3).clamp(10, 20);
    let mut picks = Vec::with_capacity(max_picks);
    for quote in quotes.into_iter().take(probe_n) {
        let code = quote.code.clone();
        let Ok(sourced) = market::fetch_klines_adjusted(&code, 180) else {
            continue;
        };
        let (_returned_code, returned_name, candles) = sourced.data;
        let name = if returned_name.trim().is_empty() || returned_name == "--" {
            names
                .get(&code)
                .cloned()
                .unwrap_or_else(|| quote.name.clone())
        } else {
            returned_name
        };
        let Some(snapshot) = ai::build_snapshot(&candles, &code, &name) else {
            continue;
        };
        picks.push(score_market_pick(&quote, &name, &snapshot));
    }

    picks.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    picks.truncate(max_picks);
    if picks.is_empty() {
        bail!("今日没有同时满足实时上涨与技术数据要求的候选股");
    }
    Ok(picks)
}

fn score_market_pick(quote: &QuoteTick, name: &str, snapshot: &AiSnapshot) -> MarketPick {
    let daily_score = ((quote.change_pct + 1.0) / 4.0 * 100.0).clamp(0.0, 100.0);
    let volume_score = snapshot
        .volume_ratio_20
        .map(|ratio| ((ratio - 0.6) / 1.4 * 100.0).clamp(0.0, 100.0))
        .unwrap_or(50.0);
    let mut score = snapshot.score * 0.60 + daily_score * 0.25 + volume_score * 0.15;
    let mut reasons: Vec<String> = snapshot.reasons.iter().take(3).cloned().collect();
    let mut risks = Vec::new();

    if quote.change_pct >= 1.0 {
        reasons.push(format!("当日涨幅 {:+.2}%", quote.change_pct));
    }
    if snapshot.ma_alignment == ai::MaAlignment::Bullish {
        reasons.push("均线偏多".into());
    }
    if snapshot.volume_ratio_20.is_some_and(|ratio| ratio >= 1.2) {
        reasons.push("量能高于近20日均值".into());
    }
    if snapshot.rsi14.is_some_and(|rsi| rsi >= 72.0) {
        score -= 8.0;
        risks.push("短线偏热".into());
    }
    if snapshot.near_20d_high {
        score -= 5.0;
        risks.push("接近20日高点，追高风险".into());
    }
    if snapshot.regime == "偏弱" || snapshot.regime == "防守" {
        risks.push(format!("技术面{}", snapshot.regime));
    }
    if reasons.is_empty() {
        reasons.push("实时动量与技术快照综合".into());
    }

    MarketPick {
        code: quote.code.clone(),
        name: name.to_string(),
        last: quote.last,
        change_pct: quote.change_pct,
        score: score.clamp(0.0, 100.0),
        regime: snapshot.regime.clone(),
        rsi14: snapshot.rsi14,
        momentum_20_pct: snapshot.momentum_20_pct,
        volume_ratio_20: snapshot.volume_ratio_20,
        reasons,
        risks,
    }
}

/// Local-first market brief. It is intentionally explicit about its source.
pub fn local_market_summary(context: &MarketAnalysisContext) -> String {
    let sentiment = context.fear_greed;
    let index_text = if context.indices.is_empty() {
        "指数快照暂缺".to_string()
    } else {
        context
            .indices
            .iter()
            .map(|index| format!("{} {:+.2}%", index.name, index.change_pct))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let picks_text = if context.picks.is_empty() {
        "暂无满足条件的候选股".to_string()
    } else {
        context
            .picks
            .iter()
            .take(3)
            .map(|pick| {
                format!(
                    "{} {}（{:.0}分，{:+.2}%）",
                    pick.code, pick.name, pick.score, pick.change_pct
                )
            })
            .collect::<Vec<_>>()
            .join("；")
    };
    let tone = match sentiment.score {
        s if s >= 80.0 => "情绪很热，优先防追高和冲高回落。",
        s if s >= 60.0 => "风险偏好偏积极，但仍需区分趋势与短线过热。",
        s if s > 40.0 => "多空力量接近，适合等待更清晰的方向。",
        s if s > 20.0 => "风险偏好偏弱，候选股宜降低仓位预期。",
        _ => "市场处于高压区，优先控制风险、减少追涨。",
    };

    format!(
        "【大盘判断】贪婪恐惧指数 {:.0}/100 · {}。{}\n\
         【市场扩散】行业上涨 {}/{}；成分股上涨 {}、下跌 {}、平盘 {}。\n\
         【指数表现】{}。\n\
         【候选观察】{}。候选按当日动量、策略雷达与量能综合筛选，不代表明日必涨。\n\
         以上为本地规则生成，仅供学习研究，不构成投资建议。",
        sentiment.score,
        sentiment.label,
        tone,
        context.sector_advances,
        context.sector_total,
        context.stock_advances,
        context.stock_declines,
        context.stock_unchanged,
        index_text,
        picks_text,
    )
}

/// Optional LLM pass over the local context. The model may explain and rank,
/// but is explicitly forbidden from inventing symbols or numbers.
pub fn llm_market_summary(cfg: &AiConfig, context: &MarketAnalysisContext) -> Result<String> {
    let body = serde_json::to_string(context).context("序列化市场分析快照失败")?;
    let prompt = format!(
        "请基于以下本地计算的 A 股市场分析快照，生成一份中文大盘简报：\n```json\n{body}\n```\n\
         要求：1) 解释贪婪恐惧指数和市场宽度；2) 结合指数与行业扩散判断当日风格；\
         3) 只能从 picks 数组中挑选最多 3 只候选股，必须使用其中的代码、名称和数值；\
         4) 为每只候选写出理由与主要风险；5) 不得编造新闻、基本面或候选列表之外的股票；\
         6) 不超过 600 字，结尾必须包含“不构成投资建议”。"
    );
    const SYSTEM: &str = "你是严谨的 A 股市场分析助手。只根据给定的本地量化快照回答，\
不编造标的、价格、新闻或基本面结论；候选股只是技术面观察，不是买卖指令。";
    ai::llm_complete(cfg, SYSTEM, &prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inputs_are_neutral() {
        let index = fear_greed_index(0, 0, 0, 0, 0, 0, None, &[]);
        assert_eq!(index.score, 50.0);
        assert_eq!(index.label, "中性");
        assert!(!index.is_greed());
        assert!(!index.is_fear());
    }

    #[test]
    fn sentiment_thresholds_match_labels() {
        assert!(!fear_greed_index(0, 0, 0, 0, 0, 0, None, &[]).is_greed());

        let greed = FearGreedIndex {
            score: 60.0,
            label: FearGreedIndex::label_for_score(60.0),
            stock_breadth: 0.0,
            sector_breadth: 0.0,
            index_momentum: 0.0,
            sector_momentum: 0.0,
        };
        assert!(greed.is_greed());
        assert_eq!(greed.label, "贪婪");

        let fear = FearGreedIndex {
            score: 40.0,
            label: FearGreedIndex::label_for_score(40.0),
            stock_breadth: 0.0,
            sector_breadth: 0.0,
            index_momentum: 0.0,
            sector_momentum: 0.0,
        };
        assert!(fear.is_fear());
        assert_eq!(fear.label, "恐惧");
    }

    #[test]
    fn broad_rally_is_greedier_than_broad_selloff() {
        let greed = fear_greed_index(900, 50, 50, 90, 5, 5, Some(2.0), &[1.0, 0.8, 0.5]);
        let fear = fear_greed_index(50, 900, 50, 5, 90, 5, Some(-2.0), &[-1.0, -0.8, -0.5]);
        assert!(greed.score > fear.score);
        assert!(greed.score > 70.0);
        assert!(fear.score < 30.0);
    }
}
