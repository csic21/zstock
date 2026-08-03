//! AI-powered stock commentary.
//!
//! Two layers:
//!
//! 1. **Local rules** — a deterministic Chinese commentary generated from the
//!    strategy-radar snapshot plus a few extra pattern features (MA alignment,
//!    MACD cross, 60-day range position). Offline, instant, free.
//! 2. **Optional LLM** — the same compact numeric snapshot is sent either to an
//!    OpenAI-compatible **API** (Responses / Chat Completions) or through a local
//!    **CLI** (`grok` / `chatgpt`·`codex` / `opencode` / `claude`). Only
//!    pre-computed metrics leave the machine (never raw K-lines), keeping tokens
//!    small and the app's local-first privacy stance intact.
//!
//! The app always shows the local commentary first and upgrades it with the
//! LLM result when configured; a failed LLM call falls back to the local text.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::data::levels::{self, ReferenceLevels};
use crate::data::signals;
use crate::model::Candle;

/// How the optional LLM is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AiTransport {
    /// HTTP API (OpenAI-compatible).
    #[default]
    Api,
    /// Local CLI tool (uses the user's already-authenticated agent).
    Cli,
}

impl AiTransport {
    pub fn label(self) -> &'static str {
        match self {
            Self::Api => "API",
            Self::Cli => "CLI",
        }
    }

    pub fn all() -> [Self; 2] {
        [Self::Api, Self::Cli]
    }
}

/// Local CLI backend when [`AiTransport::Cli`] is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AiCliProvider {
    /// xAI Grok Build (`grok`).
    #[default]
    Grok,
    /// OpenAI ChatGPT / Codex (`chatgpt`, falls back to `codex`).
    Chatgpt,
    /// OpenCode (`opencode`).
    Opencode,
    /// Anthropic Claude Code (`claude`).
    Claude,
}

impl AiCliProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Grok => "Grok",
            Self::Chatgpt => "ChatGPT",
            Self::Opencode => "OpenCode",
            Self::Claude => "Claude",
        }
    }

    pub fn all() -> [Self; 4] {
        [Self::Grok, Self::Chatgpt, Self::Opencode, Self::Claude]
    }

    /// Default executable names to search, in preference order.
    pub fn default_bins(self) -> &'static [&'static str] {
        match self {
            Self::Grok => &["grok"],
            Self::Chatgpt => &["chatgpt", "codex"],
            Self::Opencode => &["opencode"],
            Self::Claude => &["claude"],
        }
    }
}

/// LLM protocol used when talking to the configured API provider.
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
/// config.json, never sent anywhere except the configured endpoint. CLI mode
/// reuses the login state of the installed agent binary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    #[serde(default)]
    pub enabled: bool,
    /// API HTTP vs local CLI.
    #[serde(default)]
    pub transport: AiTransport,
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
    /// Which local CLI to invoke when `transport == Cli`.
    #[serde(default)]
    pub cli_provider: AiCliProvider,
    /// Optional absolute/relative path or bare name overriding the default binary.
    #[serde(default)]
    pub cli_bin: String,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: AiTransport::default(),
            kind: AiKind::default(),
            base_url: default_base_url(),
            model: default_model(),
            api_key: String::new(),
            timeout_secs: default_timeout_secs(),
            max_tokens: default_max_tokens(),
            cli_provider: AiCliProvider::default(),
            cli_bin: String::new(),
        }
    }
}

impl AiConfig {
    /// Whether the optional LLM layer has enough settings to attempt a call.
    pub fn is_configured(&self) -> bool {
        if !self.enabled {
            return false;
        }
        match self.transport {
            AiTransport::Api => {
                !self.base_url.trim().is_empty()
                    && !self.model.trim().is_empty()
                    && !self.api_key.trim().is_empty()
            }
            // Binary presence is checked at call time (PATH may differ from the UI process).
            AiTransport::Cli => true,
        }
    }

    /// Short label used in the AI panel / scout source line.
    pub fn source_label(&self) -> String {
        match self.transport {
            AiTransport::Api => {
                let m = self.model.trim();
                if m.is_empty() {
                    "LLM".into()
                } else {
                    format!("LLM · {m}")
                }
            }
            AiTransport::Cli => {
                let p = self.cli_provider.label();
                let m = self.model.trim();
                if m.is_empty() {
                    format!("CLI · {p}")
                } else {
                    format!("CLI · {p} · {m}")
                }
            }
        }
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
    /// 本地推算的参考建仓 / 减仓价位带（元）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub levels: Option<ReferenceLevels>,
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
        levels: levels::compute(candles),
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

    if let Some(lv) = &snap.levels {
        lines.push(format!(
            "【参考建仓带】{} 元 · 【参考减仓带】{} 元（技术位推算，非交易指令）。",
            lv.buy_band_text(),
            lv.sell_band_text()
        ));
    }

    lines.push("以上为本地规则生成 · 仅供学习研究，不构成任何投资建议。".to_string());
    lines.join("\n")
}

// ---- 持仓买卖建议 ----------------------------------------------------------

/// 本地/LLM 给出的持仓动作倾向（非交易指令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionAction {
    /// 尚无持仓，技术面可观察建仓。
    OpenWatch,
    /// 可考虑分批加仓。
    Add,
    /// 继续持有，观望为主。
    Hold,
    /// 可考虑分批减仓。
    Reduce,
    /// 风险偏高，观察减仓/清仓。
    ExitWatch,
}

impl PositionAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenWatch => "可观察建仓",
            Self::Add => "可考虑加仓",
            Self::Hold => "持有观望",
            Self::Reduce => "可考虑减仓",
            Self::ExitWatch => "观察减仓/清仓",
        }
    }
}

/// 持仓感知的分析快照（技术面 + 成本/盈亏）。
#[derive(Debug, Clone, Serialize)]
pub struct PositionAdviceSnap {
    pub tech: AiSnapshot,
    /// 当前持股（0 = 无持仓）。
    pub shares: f64,
    pub avg_cost: f64,
    pub last: f64,
    pub market_value: f64,
    pub unrealized_pnl: f64,
    pub unrealized_pnl_pct: f64,
    pub realized_pnl: f64,
    /// 现价相对成本的偏离（%）。
    pub price_vs_cost_pct: f64,
    /// 本地规则给出的动作倾向。
    pub action: PositionAction,
    /// 规则依据短句。
    pub action_reasons: Vec<String>,
}

/// 结合技术快照与持仓成本，生成买卖观察建议。
///
/// `shares == 0` 时按「空仓观察」处理。
pub fn build_position_advice(
    candles: &[Candle],
    code: &str,
    name: &str,
    shares: f64,
    avg_cost: f64,
    last: f64,
    realized_pnl: f64,
) -> Option<PositionAdviceSnap> {
    let tech = build_snapshot(candles, code, name)?;
    let last = if last.is_finite() && last > 0.0 {
        last
    } else {
        tech.close
    };
    let shares = if shares.is_finite() && shares > 0.0 {
        shares
    } else {
        0.0
    };
    let avg_cost = if avg_cost.is_finite() && avg_cost > 0.0 {
        avg_cost
    } else {
        0.0
    };
    let market_value = shares * last;
    let total_cost = shares * avg_cost;
    let unrealized_pnl = market_value - total_cost;
    let unrealized_pnl_pct = if total_cost > 1e-9 {
        unrealized_pnl / total_cost * 100.0
    } else {
        0.0
    };
    let price_vs_cost_pct = if avg_cost > 1e-9 {
        (last / avg_cost - 1.0) * 100.0
    } else {
        0.0
    };

    let (action, action_reasons) = decide_position_action(
        shares > 1e-9,
        &tech,
        unrealized_pnl_pct,
        price_vs_cost_pct,
        last,
        avg_cost,
    );

    Some(PositionAdviceSnap {
        tech,
        shares,
        avg_cost,
        last,
        market_value,
        unrealized_pnl,
        unrealized_pnl_pct,
        realized_pnl,
        price_vs_cost_pct,
        action,
        action_reasons,
    })
}

fn decide_position_action(
    has_pos: bool,
    tech: &AiSnapshot,
    unrealized_pct: f64,
    price_vs_cost_pct: f64,
    last: f64,
    avg_cost: f64,
) -> (PositionAction, Vec<String>) {
    let mut reasons = Vec::new();
    let regime = tech.regime.as_str();
    let score = tech.score;
    let rsi = tech.rsi14;
    let range_pos = tech.range_position_60_pct;

    // 相对参考价位
    let near_buy_band = tech.levels.as_ref().is_some_and(|lv| {
        last <= lv.buy_high * 1.01 && last >= lv.buy_low * 0.98
    });
    let near_sell_band = tech.levels.as_ref().is_some_and(|lv| last >= lv.sell_low * 0.99);
    let below_cost = has_pos && avg_cost > 0.0 && last < avg_cost;

    if !has_pos {
        if score >= 62.0 && matches!(regime, "强势" | "偏强") && near_buy_band {
            reasons.push("技术面偏强且现价靠近参考建仓带".into());
            return (PositionAction::OpenWatch, reasons);
        }
        if score >= 55.0 && near_buy_band && rsi.is_none_or(|r| r < 70.0) {
            reasons.push("现价在参考建仓带附近，可观察分批".into());
            return (PositionAction::OpenWatch, reasons);
        }
        if score < 40.0 || matches!(regime, "防守" | "偏弱") {
            reasons.push("技术面偏弱，空仓宜继续观察".into());
            return (PositionAction::Hold, reasons);
        }
        reasons.push("无持仓且信号不鲜明，继续观察".into());
        return (PositionAction::Hold, reasons);
    }

    // —— 有持仓 ——
    if unrealized_pct <= -12.0 && (score < 45.0 || matches!(regime, "防守" | "偏弱")) {
        reasons.push(format!("浮亏 {unrealized_pct:.1}% 且技术面偏弱"));
        return (PositionAction::ExitWatch, reasons);
    }
    if near_sell_band && (unrealized_pct >= 8.0 || range_pos.is_some_and(|p| p >= 85.0)) {
        reasons.push("现价靠近参考减仓带，且已有浮盈/处于区间高位".into());
        return (PositionAction::Reduce, reasons);
    }
    if unrealized_pct >= 15.0 && (rsi.is_some_and(|r| r >= 70.0) || tech.near_20d_high) {
        reasons.push(format!("浮盈 {unrealized_pct:.1}% 且短线偏热"));
        return (PositionAction::Reduce, reasons);
    }
    if below_cost
        && near_buy_band
        && score >= 55.0
        && !matches!(regime, "防守")
        && unrealized_pct > -15.0
    {
        reasons.push(format!(
            "现价低于成本 {price_vs_cost_pct:.1}% 且靠近建仓带，可观察补仓"
        ));
        return (PositionAction::Add, reasons);
    }
    if score >= 65.0
        && matches!(regime, "强势" | "偏强")
        && near_buy_band
        && unrealized_pct > -5.0
        && unrealized_pct < 12.0
    {
        reasons.push("趋势偏强且仍在建仓带附近，可观察加仓".into());
        return (PositionAction::Add, reasons);
    }
    if score < 38.0 || matches!(regime, "防守") {
        reasons.push("技术面转弱，持仓宜以防守/减仓观察".into());
        return (PositionAction::ExitWatch, reasons);
    }
    if unrealized_pct <= -8.0 {
        reasons.push(format!("浮亏 {unrealized_pct:.1}%，优先观察而非盲目加仓"));
        return (PositionAction::Hold, reasons);
    }
    reasons.push("盈亏与技术面中性，以持有观望为主".into());
    (PositionAction::Hold, reasons)
}

/// 本地规则生成的持仓买卖建议文案。
pub fn local_position_advice(snap: &PositionAdviceSnap) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(12);
    let held = snap.shares > 1e-9;

    lines.push(format!(
        "【建议倾向】{} · 策略雷达 {:.0} 分（{}）",
        snap.action.label(),
        snap.tech.score,
        snap.tech.regime
    ));

    if held {
        lines.push(format!(
            "【持仓】{} 股 · 成本 {} 元 · 现价 {} 元 · 浮盈亏 {} ({:+.2}%)",
            crate::data::portfolio::format_shares(snap.shares),
            crate::model::format_price(snap.avg_cost),
            crate::model::format_price(snap.last),
            crate::data::portfolio::format_money(snap.unrealized_pnl),
            snap.unrealized_pnl_pct
        ));
        if snap.realized_pnl.abs() > 1e-6 {
            lines.push(format!(
                "【已实现盈亏】{} 元",
                crate::data::portfolio::format_money(snap.realized_pnl)
            ));
        }
    } else {
        lines.push(format!(
            "【仓位】当前无持仓 · 现价 {} 元",
            crate::model::format_price(snap.last)
        ));
    }

    if !snap.action_reasons.is_empty() {
        lines.push(format!("【依据】{}。", snap.action_reasons.join("；")));
    }

    // 复用部分技术点评
    lines.push(format!(
        "【趋势】{}。{}",
        snap.tech.ma_alignment.label(),
        match snap.tech.ma20_slope_5 {
            Some(s) if s >= 0.3 => "MA20 近 5 日上行。",
            Some(s) if s <= -0.3 => "MA20 近 5 日下行。",
            Some(_) => "MA20 近 5 日走平。",
            None => "中期均线数据不足。",
        }
    ));

    if let Some(rsi) = snap.tech.rsi14 {
        lines.push(format!(
            "【动量】RSI14={rsi:.1}{}",
            snap.tech
                .momentum_20_pct
                .map(|m| format!("，20日动量 {m:+.1}%"))
                .unwrap_or_default()
        ));
    }

    if let Some(lv) = &snap.tech.levels {
        lines.push(format!(
            "【参考建仓带】{} 元 · 【参考减仓带】{} 元",
            lv.buy_band_text(),
            lv.sell_band_text()
        ));
        if held && snap.avg_cost > 0.0 {
            let vs_buy = if snap.avg_cost <= lv.buy_high {
                "成本落在/低于建仓带上沿附近"
            } else if snap.avg_cost >= lv.sell_low {
                "成本偏高、靠近减仓带"
            } else {
                "成本介于建仓与减仓带之间"
            };
            lines.push(format!("【成本位置】{vs_buy}。"));
        }
    }

    match snap.action {
        PositionAction::OpenWatch => {
            lines.push("【操作观察】若计划建仓，可优先参考建仓带分批，不宜追高一次性重仓。".into());
        }
        PositionAction::Add => {
            lines.push("【操作观察】加仓宜小步分批，并设定个人可接受的最大回撤。".into());
        }
        PositionAction::Hold => {
            lines.push("【操作观察】维持现有仓位，等待更清晰的价位或信号。".into());
        }
        PositionAction::Reduce => {
            lines.push("【操作观察】可考虑分批兑现部分利润，保留底仓跟踪趋势。".into());
        }
        PositionAction::ExitWatch => {
            lines.push("【操作观察】风险偏好偏低时可逐步降低仓位，避免情绪化清仓。".into());
        }
    }

    lines.push("以上为本地规则生成 · 仅供学习研究，不构成任何投资建议。".into());
    lines.join("\n")
}

/// LLM 持仓买卖建议（仅上传压缩快照）。
pub fn llm_position_advice(cfg: &AiConfig, snap: &PositionAdviceSnap) -> Result<String> {
    let body = serde_json::to_string(snap).context("序列化持仓建议快照失败")?;
    let user_prompt = format!(
        "请基于以下「持仓 + 技术面」量化快照给出买卖观察建议：\n```json\n{body}\n```\n\
         要求：1) 明确回应当前建议倾向（可观察建仓/加仓/持有/减仓/观察清仓）及理由；\
         2) 结合成本价、浮盈亏%、现价与 levels 参考带，用「约 X–Y 元」写出观察价；\
         3) 区分「有持仓」与「空仓」场景；4) 不超过 480 字；\
         5) 不得编造快照外数据；结尾必须含“不构成投资建议”。"
    );
    llm_complete(cfg, POSITION_SYSTEM_PROMPT, &user_prompt)
}

const POSITION_SYSTEM_PROMPT: &str = "你是一名严谨的 A 股持仓助手。\
你只会获得本地计算的技术快照 + 用户持仓成本/股数/浮盈亏（无原始 K 线、无基本面新闻）。\
请：1) 在快照 action 倾向基础上做可解释的中文建议，可微调但需说明依据；\
2) 给出观察性的加仓/减仓价位带（优先用 levels），强调非交易指令；\
3) 对深浮亏避免鼓吹死扛或报复性加仓；对大浮盈提醒兑现纪律；\
4) 全文不超过 480 字；结尾必须“不构成投资建议”；不得编造数值。";

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
    let body = serde_json::to_string(snap).context("序列化分析快照失败")?;
    let user_prompt = format!(
        "请基于以下本地计算好的 A 股技术面量化快照进行分析：\n```json\n{body}\n```\n\
         要求：输出结构化中文点评，覆盖趋势 / 动量 / 量能 / 位置 / 风险；\
         若快照含 levels（参考建仓带 buy_low–buy_high、减仓带 sell_low–sell_high），\
         必须用「约 X–Y 元」明确写出参考买入观察价与减仓观察价，并说明仅为技术位、非买卖指令；\
         不超过 450 字，不要编造快照之外的数据或新闻基本面，结尾必须包含“不构成投资建议”提示。"
    );
    llm_complete(cfg, SYSTEM_PROMPT, &user_prompt)
}

/// Generic completion used by stock commentary and scout summary.
///
/// Routes to the HTTP API or a local CLI based on [`AiConfig::transport`].
pub fn llm_complete(cfg: &AiConfig, system: &str, user: &str) -> Result<String> {
    if !cfg.enabled {
        bail!("AI 分析未开启（设置 → AI 分析）");
    }
    let out = match cfg.transport {
        AiTransport::Api => api_complete(cfg, system, user)?,
        AiTransport::Cli => cli_complete(cfg, system, user)?,
    };
    let trimmed = out.trim();
    if trimmed.is_empty() {
        bail!("LLM 返回了空内容");
    }
    Ok(trimmed.to_string())
}

const SYSTEM_PROMPT: &str = "你是一名严谨的 A 股技术面分析助手。\
你只会获得一份由本地程序计算好的量化快照 JSON（技术指标、形态特征与可选参考价位带，不含原始行情）。\
请：1) 基于快照写一段客观、结构化的中文点评，覆盖趋势、动量、量能与风险；\
2) 若有 levels 字段，用其中 buy_low/buy_high、sell_low/sell_high 给出「参考建仓带 / 参考减仓带」元价位，并强调只是技术观察位；\
3) 指出这些数据仅代表技术面统计，不代表基本面；\
4) 结尾必须包含“不构成投资建议”提示；\
5) 全文不超过 450 字；\
6) 不得编造快照之外的数据，数值必须与快照一致。";

// ---- HTTP API --------------------------------------------------------------

fn api_complete(cfg: &AiConfig, system: &str, user: &str) -> Result<String> {
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
                "instructions": system,
                "input": user,
                "max_output_tokens": cfg.max_tokens,
            })
            .to_string(),
        ),
        AiKind::Chat => (
            format!("{base}/chat/completions"),
            serde_json::json!({
                "model": model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user },
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

    match cfg.kind {
        AiKind::Responses => parse_responses(&text),
        AiKind::Chat => parse_chat(&text),
    }
}

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

// ---- Local CLI -------------------------------------------------------------

fn cli_complete(cfg: &AiConfig, system: &str, user: &str) -> Result<String> {
    let bin = resolve_cli_bin(cfg)?;
    // Agent CLIs are often slower than a raw HTTP call (auth / first token).
    let timeout = Duration::from_secs(cfg.timeout_secs.clamp(60, 600));
    let model = cfg.model.trim();
    let model = if model.is_empty() { None } else { Some(model) };

    match cfg.cli_provider {
        AiCliProvider::Grok => run_grok(&bin, system, user, model, timeout),
        AiCliProvider::Chatgpt => {
            let name = bin
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if name == "codex" || name.starts_with("codex") {
                run_codex(&bin, system, user, model, timeout)
            } else {
                run_chatgpt_generic(&bin, system, user, model, timeout)
            }
        }
        AiCliProvider::Opencode => run_opencode(&bin, system, user, model, timeout),
        AiCliProvider::Claude => run_claude(&bin, system, user, model, timeout),
    }
}

fn resolve_cli_bin(cfg: &AiConfig) -> Result<PathBuf> {
    let custom = cfg.cli_bin.trim();
    if !custom.is_empty() {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return Ok(p);
        }
        if let Some(found) = which_bin(custom) {
            return Ok(found);
        }
        bail!(
            "找不到 CLI「{}」（请确认已安装，或在设置中填写绝对路径）",
            custom
        );
    }
    for name in cfg.cli_provider.default_bins() {
        if let Some(found) = which_bin(name) {
            return Ok(found);
        }
    }
    let names = cfg.cli_provider.default_bins().join(" / ");
    bail!(
        "未找到 {} CLI（已搜索 {}；可安装后重试，或在设置中填写 CLI 路径）",
        cfg.cli_provider.label(),
        names
    );
}

/// Locate an executable by bare name: `$PATH` first, then common install dirs
/// (GUI apps on macOS often inherit a stripped PATH).
fn which_bin(name: &str) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        let p = PathBuf::from(name);
        return p.is_file().then_some(p);
    }

    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }

    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".grok/bin"));
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
        dirs.push(home.join("bin"));
        dirs.push(home.join(".cargo/bin"));
    }
    for dir in dirs {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn combined_prompt(system: &str, user: &str) -> String {
    format!("{system}\n\n---\n\n{user}")
}

fn write_temp_prompt(content: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "zstock-ai-{}-{}.txt",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    std::fs::write(&path, content).with_context(|| format!("写入临时提示失败：{}", path.display()))?;
    Ok(path)
}

fn run_grok(
    bin: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
) -> Result<String> {
    let prompt_file = write_temp_prompt(user)?;
    let mut cmd = Command::new(bin);
    cmd.arg("--prompt-file")
        .arg(&prompt_file)
        .arg("--output-format")
        .arg("plain")
        .arg("--system-prompt-override")
        .arg(system)
        .arg("--max-turns")
        .arg("1")
        .arg("--no-subagents")
        .arg("--disable-web-search")
        .arg("--permission-mode")
        .arg("dontAsk");
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    let result = run_command(cmd, timeout);
    let _ = std::fs::remove_file(&prompt_file);
    result
}

fn run_claude(
    bin: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
) -> Result<String> {
    let mut cmd = Command::new(bin);
    cmd.arg("-p")
        .arg("--output-format")
        .arg("text")
        .arg("--system-prompt")
        .arg(system)
        // Empty tool set: pure completion, no interactive permission prompts.
        .arg("--tools")
        .arg("")
        .arg("--bare")
        .arg("--permission-mode")
        .arg("dontAsk");
    if let Some(m) = model {
        cmd.arg("--model").arg(m);
    }
    cmd.arg("--").arg(user);
    run_command(cmd, timeout)
}

fn run_opencode(
    bin: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
) -> Result<String> {
    let prompt = combined_prompt(system, user);
    let mut cmd = Command::new(bin);
    cmd.arg("run").arg("--format").arg("default");
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    // Positional message after options.
    cmd.arg("--").arg(prompt);
    run_command(cmd, timeout)
}

fn run_codex(
    bin: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
) -> Result<String> {
    let prompt = combined_prompt(system, user);
    let out_path = std::env::temp_dir().join(format!(
        "zstock-codex-{}-{}.txt",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    let mut cmd = Command::new(bin);
    cmd.arg("exec")
        .arg("--ephemeral")
        .arg("--skip-git-repo-check")
        .arg("-s")
        .arg("read-only")
        .arg("-o")
        .arg(&out_path);
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    cmd.arg(prompt);
    let run = run_command(cmd, timeout);
    let file_out = std::fs::read_to_string(&out_path).ok();
    let _ = std::fs::remove_file(&out_path);
    match run {
        Ok(stdout) => {
            if let Some(text) = file_out.filter(|s| !s.trim().is_empty()) {
                Ok(text)
            } else {
                Ok(stdout)
            }
        }
        Err(e) => {
            if let Some(text) = file_out.filter(|s| !s.trim().is_empty()) {
                Ok(text)
            } else {
                Err(e)
            }
        }
    }
}

/// Generic `chatgpt` (and similar) one-shot CLIs: pass the full prompt as the
/// sole argument; optional `-m MODEL` when configured.
fn run_chatgpt_generic(
    bin: &Path,
    system: &str,
    user: &str,
    model: Option<&str>,
    timeout: Duration,
) -> Result<String> {
    let prompt = combined_prompt(system, user);
    let mut cmd = Command::new(bin);
    if let Some(m) = model {
        cmd.arg("-m").arg(m);
    }
    cmd.arg(prompt);
    run_command(cmd, timeout)
}

fn run_command(mut cmd: Command, timeout: Duration) -> Result<String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        // Avoid accidental interactive prompts when PATH-less GUI spawns shells.
        .env("CI", "1");

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("启动 CLI 失败：{e}（请确认二进制在 PATH 中或已填写绝对路径）"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_handle = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut r) = stdout {
            let _ = r.read_to_string(&mut s);
        }
        s
    });
    let err_handle = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut r) = stderr {
            let _ = r.read_to_string(&mut s);
        }
        s
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_handle.join();
                let _ = err_handle.join();
                bail!("CLI 超时（{}s）", timeout.as_secs());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => {
                let _ = out_handle.join();
                let _ = err_handle.join();
                bail!("等待 CLI 失败：{e}");
            }
        }
    };

    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();

    if !status.success() {
        let detail = if !stderr.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };
        let code = status.code().unwrap_or(-1);
        if detail.is_empty() {
            bail!("CLI 退出码 {code}");
        }
        bail!("CLI 退出码 {code}：{}", truncate(detail, 240));
    }

    let text = stdout.trim();
    if text.is_empty() {
        // Some CLIs print the answer to stderr in edge cases.
        let err = stderr.trim();
        if !err.is_empty() {
            return Ok(err.to_string());
        }
        bail!("CLI 返回了空内容");
    }
    Ok(text.to_string())
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
        assert!(snap.levels.is_some(), "levels expected on long series");
        assert!(text.contains("【参考建仓带】"));
        assert!(text.contains("【参考减仓带】"));
    }

    #[test]
    fn position_advice_includes_cost_and_action() {
        let candles = series(10.0, 0.006, 120);
        let last = candles.last().unwrap().close;
        let snap = build_position_advice(
            &candles,
            "600519",
            "测试",
            100.0,
            last * 0.9, // 成本低于现价 → 有浮盈
            last,
            0.0,
        )
        .unwrap();
        assert!(snap.shares > 0.0);
        assert!(snap.unrealized_pnl > 0.0);
        let text = local_position_advice(&snap);
        assert!(text.contains("【建议倾向】"));
        assert!(text.contains("【持仓】"));
        assert!(text.contains("不构成任何投资建议"));
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
        assert_eq!(cfg.transport, AiTransport::Api);
        assert_eq!(cfg.cli_provider, AiCliProvider::Grok);
        assert!(cfg.cli_bin.is_empty());
        assert_eq!(cfg.timeout_secs, 30);
        assert!(!cfg.is_configured());
    }

    #[test]
    fn cli_transport_is_configured_when_enabled() {
        let mut cfg = AiConfig::default();
        cfg.enabled = true;
        cfg.transport = AiTransport::Cli;
        cfg.model.clear();
        assert!(cfg.is_configured());
        assert_eq!(cfg.source_label(), "CLI · Grok");
        cfg.model = "grok-4.5".into();
        assert_eq!(cfg.source_label(), "CLI · Grok · grok-4.5");
    }

    #[test]
    fn api_transport_requires_key_and_model() {
        let mut cfg = AiConfig::default();
        cfg.enabled = true;
        cfg.transport = AiTransport::Api;
        assert!(!cfg.is_configured());
        cfg.api_key = "sk-test".into();
        assert!(cfg.is_configured());
        assert_eq!(cfg.source_label(), "LLM · gpt-5-mini");
    }

    #[test]
    fn combined_prompt_joins_system_and_user() {
        let p = combined_prompt("SYS", "USER");
        assert!(p.contains("SYS"));
        assert!(p.contains("USER"));
        assert!(p.contains("---"));
    }
}
