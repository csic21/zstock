//! AI-powered stock commentary.
//!
//! Two layers:
//!
//! 1. **Local rules** — a deterministic Chinese commentary generated from the
//!    strategy-radar snapshot plus a few extra pattern features (MA alignment,
//!    MACD cross, 60-day range position). Offline, instant, free.
//! 2. **Optional LLM** — the same compact numeric snapshot is sent to an
//!    OpenAI-compatible endpoint, speaking either the **Responses** or the
//!    **Chat Completions** protocol. Only pre-computed metrics leave the
//!    machine (never raw K-lines), keeping tokens small and the app's
//!    local-first privacy stance intact.
//!
//! The app always shows the local commentary first and upgrades it with the
//! LLM result when configured; a failed LLM call falls back to the local text.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::data::signals;
use crate::model::Candle;

/// LLM protocol used when talking to the configured provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AiKind {
    /// OpenAI Responses API (`POST /v1/responses`).
    #[default]
    Responses,
    /// OpenAI-compatible Chat Completions (`POST /v1/chat/completions`).
    Chat,
}

impl AiKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Responses => "Responses",
            Self::Chat => "Chat",
        }
    }

    pub fn all() -> [Self; 2] {
        [Self::Responses, Self::Chat]
    }
}

fn default_base_url() -> String {
    "https://api.openai.com/v1".into()
}

fn default_model() -> String {
    "gpt-5-mini".into()
}

fn default_timeout_secs() -> u64 {
    30
}

fn default_max_tokens() -> u32 {
    1000
}

/// User-configurable LLM settings. The API key is stored only in the local
/// config.json, never sent anywhere except the configured endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub kind: AiKind,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: AiKind::default(),
            base_url: default_base_url(),
            model: default_model(),
            api_key: String::new(),
            timeout_secs: default_timeout_secs(),
            max_tokens: default_max_tokens(),
        }
    }
}

impl AiConfig {
    pub fn is_configured(&self) -> bool {
        self.enabled
            && !self.base_url.trim().is_empty()
            && !self.model.trim().is_empty()
            && !self.api_key.trim().is_empty()
    }
}

/// Short description of the MA stack shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaAlignment {
    /// 多头排列：MA5 > MA10 > MA20 ( > MA60)
    Bullish,
    /// 空头排列：MA5 < MA10 < MA20 ( < MA60)
    Bearish,
    /// 缠绕 / 无明确排列
    Mixed,
}

impl MaAlignment {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bullish => "多头排列",
            Self::Bearish => "空头排列",
            Self::Mixed => "均线缠绕",
        }
    }
}

/// Most recent MACD cross (if any) and current histogram state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MacdSignal {
    None,
    Golden,
    Death,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct MacdSnapshot {
    pub signal: MacdSignal,
    /// Bars since the most recent cross (None when no cross in the window).
    pub cross_age: Option<usize>,
    /// Current `dif - dea`, the "histogram" per Chinese convention.
    pub histogram: Option<f64>,
}

/// Compact, serializable technical snapshot sent to the LLM.
#[derive(Debug, Clone, Serialize)]
pub struct AiSnapshot {
    pub code: String,
    pub name: String,
    /// Date of the last bar (`as_of`).
    pub as_of: String,
    pub close: f64,
    pub change_pct_1d: Option<f64>,
    /// 策略雷达 0–100 综合分。
    pub score: f64,
    pub regime: String,
    pub rsi14: Option<f64>,
    pub momentum_20_pct: Option<f64>,
    pub volatility_20_ann_pct: Option<f64>,
    pub max_drawdown_1y_pct: Option<f64>,
    pub volume_ratio_20: Option<f64>,
    pub confidence: f64,
    pub reasons: Vec<String>,
    pub ma_alignment: MaAlignment,
    /// MA20 slope over the last 5 bars (%).
    pub ma20_slope_5: Option<f64>,
    /// Close position within the last 60 closes, 0–100.
    pub range_position_60_pct: Option<f64>,
    pub macd: MacdSnapshot,
    /// Rising days among the last 5 bars.
    pub up_days_5: u8,
    /// Close within 1.5% of the 20-bar high.
    pub near_20d_high: bool,
}

/// Build the analysis snapshot for the given daily series.
///
/// Returns `None` when there is not enough data for the strategy radar
/// (signals requires at least 20 bars).
pub fn build_snapshot(candles: &[Candle], code: &str, name: &str) -> Option<AiSnapshot> {
    let sig = signals::analyze(candles)?;
    let last = candles.last()?;
    let prev = candles.get(candles.len().saturating_sub(2));
    let change_pct_1d = prev
        .filter(|p| p.close.is_finite() && p.close > 0.0)
        .map(|p| (last.close / p.close - 1.0) * 100.0);

    Some(AiSnapshot {
        code: code.to_string(),
        name: name.to_string(),
        as_of: last.date.to_string(),
        close: last.close,
        change_pct_1d,
        score: sig.score,
        regime: sig.regime.label().to_string(),
        rsi14: sig.rsi14,
        momentum_20_pct: sig.momentum_20_pct,
        volatility_20_ann_pct: sig.volatility_20_ann_pct,
        max_drawdown_1y_pct: sig.max_drawdown_1y_pct,
        volume_ratio_20: sig.volume_ratio_20,
        confidence: sig.confidence,
        reasons: sig.reasons.iter().map(|r| r.to_string()).collect(),
        ma_alignment: ma_alignment(candles),
        ma20_slope_5: sma_slope_pct(candles, 20, 5),
        range_position_60_pct: range_position_pct(candles, 60),
        macd: macd_snapshot(candles),
        up_days_5: up_days(candles, 5),
        near_20d_high: near_high(candles, 20, 0.015),
    })
}

/// Deterministic Chinese commentary from the snapshot. Works offline and is
/// the fallback whenever the LLM is unavailable.
pub fn local_commentary(snap: &AiSnapshot) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(10);

    lines.push(format!(
        "【综合】策略雷达 {} 分（{}）· 数据置信 {:.0}%。{}",
        snap.score.round() as i64,
        snap.regime,
        snap.confidence,
        regime_sentence(snap.regime.as_str())
    ));

    if !snap.reasons.is_empty() {
        lines.push(format!("【依据】{}。", snap.reasons.join("，")));
    }

    lines.push(format!(
        "【趋势】{}。{}",
        snap.ma_alignment.label(),
        match snap.ma20_slope_5 {
            Some(s) if s >= 0.3 => "MA20 近 5 日上行，中期动能偏强。",
            Some(s) if s <= -0.3 => "MA20 近 5 日下行，中期动能偏弱。",
            Some(_) => "MA20 近 5 日走平，方向待选择。",
            None => "中期均线数据不足。",
        }
    ));

    let mut momentum = Vec::new();
    if let Some(rsi) = snap.rsi14 {
        momentum.push(format!("RSI14={rsi:.1}"));
    }
    if let Some(mom) = snap.momentum_20_pct {
        momentum.push(format!("20日动量 {mom:+.1}%"));
    }
    if !momentum.is_empty() {
        lines.push(format!("【动量】{}。", momentum.join("，")));
    }

    if let Some(pos) = snap.range_position_60_pct {
        let pos_desc = match pos {
            p if p >= 85.0 => "接近区间高位",
            p if p <= 15.0 => "处于区间低位",
            p if p >= 55.0 => "区间中上沿",
            _ => "区间中下沿",
        };
        lines.push(format!("【位置】现价位于近 60 日区间的 {pos:.0}% 分位（{pos_desc}）。"));
    }

    if let Some(ratio) = snap.volume_ratio_20 {
        let dir = if snap.change_pct_1d.is_some_and(|c| c >= 0.0) {
            "上涨"
        } else {
            "下跌"
        };
        lines.push(format!("【量能】量能比 {ratio:.1}x，当日{dir}。"));
    }

    match snap.macd.signal {
        MacdSignal::Golden => lines.push(format!(
            "【MACD】最近金叉（{} 根前），动量指标转多。",
            snap.macd.cross_age.unwrap_or(0) + 1
        )),
        MacdSignal::Death => lines.push(format!(
            "【MACD】最近死叉（{} 根前），动量指标转空。",
            snap.macd.cross_age.unwrap_or(0) + 1
        )),
        MacdSignal::None => {}
    }

    let mut risks = Vec::new();
    if let Some(v) = snap.volatility_20_ann_pct {
        risks.push(format!("20日年化波动 {v:.0}%"));
    }
    if let Some(dd) = snap.max_drawdown_1y_pct {
        risks.push(format!("1年最大回撤 {dd:.0}%"));
    }
    if snap.near_20d_high {
        risks.push("贴近20日高点，短线追高需谨慎".to_string());
    }
    if !risks.is_empty() {
        lines.push(format!("【风险】{}。", risks.join("；")));
    }

    if snap.up_days_5 >= 4 {
        lines.push("【节奏】近 5 日多数收涨，短线情绪偏热。".to_string());
    } else if snap.up_days_5 <= 1 {
        lines.push("【节奏】近 5 日多数收跌，短线情绪偏弱。".to_string());
    }

    lines.push("以上为本地规则生成 · 仅供学习研究，不构成任何投资建议。".to_string());
    lines.join("\n")
}

fn regime_sentence(regime: &str) -> &'static str {
    match regime {
        "强势" => "当前技术面处于强势区间。",
        "偏强" => "当前技术面略偏积极。",
        "中性" => "当前技术面方向不明。",
        "偏弱" => "当前技术面略偏谨慎。",
        "防守" => "当前技术面偏弱，建议以防守为主。",
        _ => "",
    }
}

/// Request a commentary from the configured LLM. Only the compact snapshot is
/// sent; the returned text is expected to be plain Chinese prose.
pub fn llm_commentary(cfg: &AiConfig, snap: &AiSnapshot) -> Result<String> {
    if !cfg.enabled {
        bail!("AI 分析未开启（设置 → AI 分析）");
    }
    if cfg.api_key.trim().is_empty() {
        bail!("未配置 API Key（设置 → AI 分析）");
    }
    let base = cfg.base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        bail!("未配置 API 地址（设置 → AI 分析）");
    }
    let model = cfg.model.trim();
    if model.is_empty() {
        bail!("未配置模型名称（设置 → AI 分析）");
    }

    let body = serde_json::to_string(snap).context("序列化分析快照失败")?;
    let user_prompt = format!(
        "请基于以下本地计算好的 A 股技术面量化快照进行分析：\n```json\n{body}\n```\n\
         要求：输出一段结构化中文点评（趋势 / 动量 / 量能 / 位置 / 风险），\
         不超过 400 字，不要编造快照之外的数据，结尾必须包含“不构成投资建议”提示。"
    );

    let timeout = Duration::from_secs(cfg.timeout_secs.clamp(5, 120));
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build();

    let (path, payload) = match cfg.kind {
        AiKind::Responses => (
            format!("{base}/responses"),
            serde_json::json!({
                "model": model,
                "instructions": SYSTEM_PROMPT,
                "input": user_prompt,
                "max_output_tokens": cfg.max_tokens,
            })
            .to_string(),
        ),
        AiKind::Chat => (
            format!("{base}/chat/completions"),
            serde_json::json!({
                "model": model,
                "messages": [
                    { "role": "system", "content": SYSTEM_PROMPT },
                    { "role": "user", "content": user_prompt },
                ],
                "temperature": 0.3,
                "max_tokens": cfg.max_tokens,
            })
            .to_string(),
        ),
    };

    let response = agent
        .post(&path)
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {}", cfg.api_key.trim()))
        .send_string(&payload)
        .map_err(friendly_http_error)?;
    let text = response
        .into_string()
        .map_err(|e| anyhow!("读取 LLM 响应失败：{e}"))?;

    let out = match cfg.kind {
        AiKind::Responses => parse_responses(&text)?,
        AiKind::Chat => parse_chat(&text)?,
    };
    let trimmed = out.trim();
    if trimmed.is_empty() {
        bail!("LLM 返回了空内容");
    }
    Ok(trimmed.to_string())
}

const SYSTEM_PROMPT: &str = "你是一名严谨的 A 股技术面分析助手。\
你只会获得一份由本地程序计算好的量化快照 JSON（技术指标与形态特征，不含原始行情）。\
请：1) 基于快照写一段客观、结构化的中文点评，覆盖趋势、动量、量能与风险；\
2) 指出这些数据仅代表技术面统计，不代表基本面；\
3) 结尾必须包含“不构成投资建议”提示；\
4) 全文不超过 400 字；\
5) 不得编造快照之外的数据，数值必须与快照一致。";

fn friendly_http_error(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            let snippet = body.trim();
            if snippet.is_empty() {
                anyhow!("HTTP {code}")
            } else {
                anyhow!("HTTP {code}：{}", truncate(snippet, 180))
            }
        }
        ureq::Error::Transport(t) => anyhow!("网络错误：{t}"),
    }
}

fn truncate(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push('…');
    }
    out
}

fn parse_responses(text: &str) -> Result<String> {
    let v: serde_json::Value =
        serde_json::from_str(text).context("解析 Responses 响应失败（返回的不是 JSON）")?;
    if let Some(msg) = extract_api_error(&v) {
        bail!("接口返回错误：{msg}");
    }
    let mut parts = Vec::new();
    if let Some(output) = v.get("output").and_then(|o| o.as_array()) {
        for item in output {
            if item.get("type").and_then(|t| t.as_str()) != Some("message") {
                continue;
            }
            if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
                for c in content {
                    if c.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                        if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                            parts.push(t);
                        }
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        bail!("Responses 响应中没有找到文本内容");
    }
    Ok(parts.join("\n"))
}

fn parse_chat(text: &str) -> Result<String> {
    let v: serde_json::Value =
        serde_json::from_str(text).context("解析 Chat 响应失败（返回的不是 JSON）")?;
    if let Some(msg) = extract_api_error(&v) {
        bail!("接口返回错误：{msg}");
    }
    match v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
    {
        Some(s) if !s.trim().is_empty() => Ok(s.to_string()),
        _ => bail!("Chat 响应中没有文本内容（可能是配额或限流错误）"),
    }
}

fn extract_api_error(v: &serde_json::Value) -> Option<String> {
    v.get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .map(|s| truncate(s, 200))
}

// ---- pattern helpers -------------------------------------------------------

fn closes(candles: &[Candle]) -> Vec<f64> {
    candles
        .iter()
        .map(|c| c.close)
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect()
}

fn sma(values: &[f64], period: usize) -> Option<f64> {
    if period == 0 || values.len() < period {
        return None;
    }
    let window = &values[values.len() - period..];
    Some(window.iter().sum::<f64>() / window.len() as f64)
}

fn ma_alignment(candles: &[Candle]) -> MaAlignment {
    let values = closes(candles);
    if values.len() < 20 {
        return MaAlignment::Mixed;
    }
    let Some(ma5) = sma(&values, 5) else {
        return MaAlignment::Mixed;
    };
    let Some(ma10) = sma(&values, 10) else {
        return MaAlignment::Mixed;
    };
    let Some(ma20) = sma(&values, 20) else {
        return MaAlignment::Mixed;
    };
    let ma60 = sma(&values, 60);
    let bullish_short = ma5 > ma10 && ma10 > ma20;
    let bearish_short = ma5 < ma10 && ma10 < ma20;
    match ma60 {
        Some(ma60) if ma20 > ma60 && bullish_short => MaAlignment::Bullish,
        Some(ma60) if ma20 < ma60 && bearish_short => MaAlignment::Bearish,
        None if bullish_short => MaAlignment::Bullish,
        None if bearish_short => MaAlignment::Bearish,
        _ => MaAlignment::Mixed,
    }
}

fn sma_slope_pct(candles: &[Candle], period: usize, window: usize) -> Option<f64> {
    let values = closes(candles);
    if values.len() < period + window {
        return None;
    }
    let now = sma(&values, period)?;
    let prev_values = &values[..values.len() - window];
    let prev = sma(prev_values, period)?;
    if prev <= 0.0 {
        return None;
    }
    Some((now / prev - 1.0) * 100.0)
}

fn range_position_pct(candles: &[Candle], period: usize) -> Option<f64> {
    let values = closes(candles);
    if values.len() < period {
        return None;
    }
    let window = &values[values.len() - period..];
    let min = window.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = window.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if max <= min {
        return Some(50.0);
    }
    let last = *values.last()?;
    Some(((last - min) / (max - min) * 100.0).clamp(0.0, 100.0))
}

fn macd_snapshot(candles: &[Candle]) -> MacdSnapshot {
    const WINDOW: usize = 8;
    let values = closes(candles);
    // Need 26 (EMA26 warm-up) + 9 (DEA) + a lookback window.
    if values.len() < 26 + 9 + WINDOW {
        return MacdSnapshot {
            signal: MacdSignal::None,
            cross_age: None,
            histogram: None,
        };
    }

    let ema12 = ema_series(&values, 12);
    let ema26 = ema_series(&values, 26);
    let dif: Vec<f64> = ema12
        .iter()
        .zip(ema26.iter())
        .map(|(a, b)| a - b)
        .collect();
    let dea = ema_series(&dif, 9);
    let hist: Vec<f64> = dif
        .iter()
        .zip(dea.iter())
        .map(|(d, e)| d - e)
        .collect();

    let cur = hist.last().copied();
    let recent = &hist[hist.len().saturating_sub(WINDOW + 1)..];
    let mut cross_age = None;
    let mut signal = MacdSignal::None;
    for (i, pair) in recent.windows(2).rev().enumerate() {
        let (a, b) = (pair[0], pair[1]);
        if (a <= 0.0 && b > 0.0) || (a >= 0.0 && b < 0.0) {
            signal = if b > 0.0 {
                MacdSignal::Golden
            } else {
                MacdSignal::Death
            };
            cross_age = Some(i);
            break;
        }
    }
    MacdSnapshot {
        signal,
        cross_age,
        histogram: cur,
    }
}

fn ema_series(values: &[f64], period: usize) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }
    let k = 2.0 / (period as f64 + 1.0);
    let mut out = Vec::with_capacity(values.len());
    let mut prev = values[0];
    out.push(prev);
    for &v in &values[1..] {
        prev = v * k + prev * (1.0 - k);
        out.push(prev);
    }
    out
}

fn up_days(candles: &[Candle], n: usize) -> u8 {
    if candles.len() < n + 1 {
        return 0;
    }
    candles[candles.len() - n - 1..]
        .windows(2)
        .filter(|pair| pair[1].close > pair[0].close)
        .count() as u8
}

fn near_high(candles: &[Candle], period: usize, tolerance: f64) -> bool {
    let values = closes(candles);
    if values.len() < period {
        return false;
    }
    let window = &values[values.len() - period..];
    let max = window.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let Some(&last) = values.last() else {
        return false;
    };
    max > 0.0 && last >= max * (1.0 - tolerance)
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
                    high: close * 1.01,
                    low: close * 0.99,
                    close,
                    volume: 100_000 + i as u64 * 100,
                }
            })
            .collect()
    }

    #[test]
    fn snapshot_bullish_on_rising_series() {
        let candles = series(10.0, 0.006, 120);
        let snap = build_snapshot(&candles, "600519", "测试").unwrap();
        assert_eq!(snap.ma_alignment, MaAlignment::Bullish);
        assert!(snap.score > 50.0);
        assert!(snap.range_position_60_pct.unwrap() > 80.0);
        assert!(!snap.reasons.is_empty());
        assert_eq!(snap.code, "600519");
        assert_eq!(snap.as_of, "d119");
    }

    #[test]
    fn local_commentary_is_structured_and_has_disclaimer() {
        let candles = series(10.0, 0.006, 120);
        let snap = build_snapshot(&candles, "600519", "测试").unwrap();
        let text = local_commentary(&snap);
        assert!(text.contains("【综合】"));
        assert!(text.contains("不构成任何投资建议"));
        assert!(text.contains(snap.regime.as_str()));
    }

    #[test]
    fn parse_chat_extracts_content() {
        let raw = r#"{"choices":[{"message":{"content":"趋势向好。不构成投资建议"}}]}"#;
        assert_eq!(parse_chat(raw).unwrap(), "趋势向好。不构成投资建议");
    }

    #[test]
    fn parse_responses_extracts_output_text() {
        let raw = r#"{
            "output": [
                {"type": "message", "content": [
                    {"type": "output_text", "text": "第一段"},
                    {"type": "output_text", "text": "第二段"}
                ]},
                {"type": "reasoning", "summary": []}
            ]
        }"#;
        assert_eq!(parse_responses(raw).unwrap(), "第一段\n第二段");
    }

    #[test]
    fn api_error_is_surfaced() {
        let raw = r#"{"error":{"message":"Invalid API key"}}"#;
        assert!(parse_chat(raw).unwrap_err().to_string().contains("Invalid API key"));
    }

    #[test]
    fn config_defaults_and_serde() {
        let cfg: AiConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.kind, AiKind::Responses);
        assert_eq!(cfg.timeout_secs, 30);
        assert!(!cfg.is_configured());
    }
}
