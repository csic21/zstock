//! Multi-dimension review of an open position from cost vs last price.
//!
//! The review does not predict returns. It classifies the current lot against
//! local technical bands, the stop/target the user already set, and market
//! climate. AI may explain the result; it must not change stance or tones.

use serde::{Deserialize, Serialize};

const NEAR_BUY_HIGH_SLACK: f64 = 1.01;
const NEAR_BUY_LOW_SLACK: f64 = 0.98;
const NEAR_SELL_SLACK: f64 = 0.99;
const DEEP_LOSS_PCT: f64 = -12.0;
const SOFT_LOSS_PCT: f64 = -8.0;
const LARGE_GAIN_PCT: f64 = 15.0;
const TRIM_GAIN_PCT: f64 = 6.0;
const TIME_DECAY_DAYS: u32 = 10;
const TIME_DECAY_BAND_PCT: f64 = 3.0;
const ADD_SCORE_FLOOR: f64 = 55.0;
const WEAK_SCORE: f64 = 45.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionReviewStance {
    Protect,
    Hold,
    ReduceWatch,
    AddWatch,
}

impl PositionReviewStance {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Protect, false) => "优先防守",
            (Self::Protect, true) => "Protect",
            (Self::Hold, false) => "持有观望",
            (Self::Hold, true) => "Hold",
            (Self::ReduceWatch, false) => "观察减仓",
            (Self::ReduceWatch, true) => "Trim watch",
            (Self::AddWatch, false) => "观察补仓",
            (Self::AddWatch, true) => "Add watch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionTone {
    Support,
    Neutral,
    Caution,
    Blocked,
    Unknown,
}

impl DimensionTone {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Support, false) => "通过",
            (Self::Support, true) => "Pass",
            (Self::Neutral, false) => "中性",
            (Self::Neutral, true) => "Neutral",
            (Self::Caution, false) => "注意",
            (Self::Caution, true) => "Watch",
            (Self::Blocked, false) => "拦截",
            (Self::Blocked, true) => "Block",
            (Self::Unknown, false) => "证据不足",
            (Self::Unknown, true) => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewDimension {
    pub id: String,
    pub title: String,
    pub work_title: String,
    pub tone: DimensionTone,
    pub headline: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionReview {
    pub code: String,
    pub stance: PositionReviewStance,
    pub headline: String,
    pub cost: f64,
    pub last: f64,
    pub price_vs_cost_pct: f64,
    pub atr_from_cost: Option<f64>,
    pub dimensions: Vec<ReviewDimension>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PositionReviewInput {
    pub code: String,
    pub shares: f64,
    pub avg_cost: f64,
    pub last: f64,
    pub unrealized_pnl: f64,
    pub unrealized_pnl_pct: f64,
    pub realized_pnl: f64,
    pub held_calendar_days: Option<u32>,
    pub trade_count: usize,
    pub quote_stale: bool,
    pub regime_label: Option<String>,
    pub score: Option<f64>,
    pub rsi14: Option<f64>,
    pub price_vs_ma20_pct: Option<f64>,
    pub range_position_60_pct: Option<f64>,
    pub atr14: Option<f64>,
    pub buy_low: Option<f64>,
    pub buy_high: Option<f64>,
    pub sell_low: Option<f64>,
    pub sell_high: Option<f64>,
    pub stop_price: Option<f64>,
    pub take_profit: Option<f64>,
    pub stop_triggered: bool,
    pub take_profit_triggered: bool,
    pub position_weight_pct: Option<f64>,
    pub climate_label: Option<String>,
    pub new_entries_frozen: bool,
    pub has_open_plan: bool,
    pub plan_invalidation: Option<String>,
    pub plan_target: Option<String>,
}

pub fn analyze_position(input: &PositionReviewInput) -> Option<PositionReview> {
    if input.shares <= 1e-9 || !input.avg_cost.is_finite() || input.avg_cost <= 0.0 {
        return None;
    }
    let last_ok = input.last.is_finite() && input.last > 0.0;
    let last = if last_ok { input.last } else { 0.0 };
    let price_vs_cost_pct = if last_ok {
        (last / input.avg_cost - 1.0) * 100.0
    } else {
        0.0
    };
    let atr_from_cost = last_ok
        .then(|| input.atr14.filter(|atr| atr.is_finite() && *atr > 0.0))
        .flatten()
        .map(|atr| (last - input.avg_cost) / atr);

    let pnl = pnl_dimension(input, last_ok, price_vs_cost_pct, atr_from_cost);
    let location = location_dimension(input, last_ok, atr_from_cost);
    let trend = trend_dimension(input, last_ok);
    let risk = risk_dimension(input, last_ok);
    let discipline = discipline_dimension(input);
    let time = time_dimension(input, last_ok, price_vs_cost_pct);

    let stance = decide_stance(input, last_ok, price_vs_cost_pct);
    let headline = stance_headline(stance, input, last_ok, price_vs_cost_pct);

    Some(PositionReview {
        code: input.code.clone(),
        stance,
        headline,
        cost: input.avg_cost,
        last,
        price_vs_cost_pct,
        atr_from_cost,
        dimensions: vec![pnl, location, trend, risk, discipline, time],
    })
}

fn decide_stance(
    input: &PositionReviewInput,
    last_ok: bool,
    price_vs_cost_pct: f64,
) -> PositionReviewStance {
    if !last_ok {
        return PositionReviewStance::Hold;
    }
    let last = input.last;
    let stop_hit = input.stop_triggered
        || input
            .stop_price
            .is_some_and(|stop| stop.is_finite() && stop > 0.0 && last <= stop);
    if stop_hit {
        return PositionReviewStance::Protect;
    }

    let weak = input.regime_label.as_deref().is_some_and(is_weak_regime)
        || input.score.is_some_and(|score| score < WEAK_SCORE);
    if input.unrealized_pnl_pct <= DEEP_LOSS_PCT && (weak || input.score.is_none()) {
        return PositionReviewStance::Protect;
    }
    if input.new_entries_frozen && input.unrealized_pnl_pct <= SOFT_LOSS_PCT {
        return PositionReviewStance::Protect;
    }

    let near_sell = near_sell_band(input, last);
    let hot = input.rsi14.is_some_and(|rsi| rsi >= 70.0)
        || input
            .range_position_60_pct
            .is_some_and(|position| position >= 85.0);
    if (near_sell && input.unrealized_pnl_pct >= TRIM_GAIN_PCT)
        || (input.unrealized_pnl_pct >= LARGE_GAIN_PCT && hot)
    {
        return PositionReviewStance::ReduceWatch;
    }

    let can_add = !input.new_entries_frozen
        && !input
            .regime_label
            .as_deref()
            .is_some_and(is_defensive_regime)
        && input.score.is_some_and(|score| score >= ADD_SCORE_FLOOR)
        && input.unrealized_pnl_pct > DEEP_LOSS_PCT
        && last < input.avg_cost
        && near_buy_band(input, last);
    if can_add {
        return PositionReviewStance::AddWatch;
    }

    let _ = price_vs_cost_pct;
    PositionReviewStance::Hold
}

fn stance_headline(
    stance: PositionReviewStance,
    input: &PositionReviewInput,
    last_ok: bool,
    price_vs_cost_pct: f64,
) -> String {
    if !last_ok {
        return "现价不可用，先核对行情再看盈亏与价位".into();
    }
    match stance {
        PositionReviewStance::Protect => {
            if input.stop_triggered
                || input
                    .stop_price
                    .is_some_and(|stop| input.last <= stop && stop > 0.0)
            {
                "现价已触及失效价，先处理风险，不再加仓".into()
            } else if input.new_entries_frozen {
                format!(
                    "浮亏 {:+.1}% 且市场观望，先防守持仓，不补仓",
                    input.unrealized_pnl_pct
                )
            } else {
                format!(
                    "浮亏 {:+.1}% 且结构偏弱，优先防守而不是摊低成本",
                    input.unrealized_pnl_pct
                )
            }
        }
        PositionReviewStance::ReduceWatch => {
            "现价相对成本已有利润，且靠近减仓带或短线偏热，可观察兑现".into()
        }
        PositionReviewStance::AddWatch => format!(
            "现价低于成本 {:+.1}%，仍靠近建仓带，只观察小步补仓",
            price_vs_cost_pct
        ),
        PositionReviewStance::Hold => {
            if input.unrealized_pnl_pct.abs() < TIME_DECAY_BAND_PCT {
                "现价贴着成本，等待更清晰的价位或计划，不急着加减".into()
            } else if input.unrealized_pnl_pct > 0.0 {
                format!(
                    "浮盈 {:+.1}%，趋势与纪律未要求减仓，继续按计划持有",
                    input.unrealized_pnl_pct
                )
            } else {
                format!(
                    "浮亏 {:+.1}%，尚未跌破防守条件，继续观察而不是补仓",
                    input.unrealized_pnl_pct
                )
            }
        }
    }
}

fn pnl_dimension(
    input: &PositionReviewInput,
    last_ok: bool,
    price_vs_cost_pct: f64,
    atr_from_cost: Option<f64>,
) -> ReviewDimension {
    if !last_ok || input.quote_stale {
        return dimension(
            "pnl",
            "盈亏",
            "P&L",
            DimensionTone::Unknown,
            "现价不足，浮盈亏不可靠",
            "行情缺失或过期时，不以成本对比作为行动依据",
        );
    }
    let atr_text = atr_from_cost
        .map(|value| format!("，相当成本外侧 {value:+.1} 倍 ATR"))
        .unwrap_or_default();
    let realized = if input.realized_pnl.abs() > 1e-6 {
        format!("；该票已实现 {:+.2}", input.realized_pnl)
    } else {
        String::new()
    };
    if input.unrealized_pnl_pct <= DEEP_LOSS_PCT {
        dimension(
            "pnl",
            "盈亏",
            "P&L",
            DimensionTone::Blocked,
            format!("浮亏 {:+.1}%", input.unrealized_pnl_pct),
            format!(
                "成本 {:.2} → 现价 {:.2}{atr_text}。回本还需 {:.1}%{realized}",
                input.avg_cost,
                input.last,
                (-price_vs_cost_pct).max(0.0)
            ),
        )
    } else if input.unrealized_pnl_pct <= SOFT_LOSS_PCT {
        dimension(
            "pnl",
            "盈亏",
            "P&L",
            DimensionTone::Caution,
            format!("浮亏 {:+.1}%", input.unrealized_pnl_pct),
            format!(
                "成本 {:.2} → 现价 {:.2}{atr_text}。先看失效价，不按亏损幅度补仓{realized}",
                input.avg_cost, input.last
            ),
        )
    } else if input.unrealized_pnl_pct >= LARGE_GAIN_PCT {
        dimension(
            "pnl",
            "盈亏",
            "P&L",
            DimensionTone::Caution,
            format!("浮盈 {:+.1}%", input.unrealized_pnl_pct),
            format!(
                "成本 {:.2} → 现价 {:.2}{atr_text}。利润已厚，核对减仓带与是否设了止盈{realized}",
                input.avg_cost, input.last
            ),
        )
    } else if input.unrealized_pnl_pct >= TRIM_GAIN_PCT {
        dimension(
            "pnl",
            "盈亏",
            "P&L",
            DimensionTone::Support,
            format!("浮盈 {:+.1}%", input.unrealized_pnl_pct),
            format!(
                "成本 {:.2} → 现价 {:.2}{atr_text}。成本已被现价覆盖{realized}",
                input.avg_cost, input.last
            ),
        )
    } else if input.unrealized_pnl_pct >= 0.0 {
        dimension(
            "pnl",
            "盈亏",
            "P&L",
            DimensionTone::Neutral,
            format!("浮盈 {:+.1}%", input.unrealized_pnl_pct),
            format!(
                "成本 {:.2} → 现价 {:.2}{atr_text}。刚离开成本区{realized}",
                input.avg_cost, input.last
            ),
        )
    } else {
        dimension(
            "pnl",
            "盈亏",
            "P&L",
            DimensionTone::Neutral,
            format!("浮亏 {:+.1}%", input.unrealized_pnl_pct),
            format!(
                "成本 {:.2} → 现价 {:.2}{atr_text}。回本还需 {:.1}%{realized}",
                input.avg_cost,
                input.last,
                (-price_vs_cost_pct).max(0.0)
            ),
        )
    }
}

fn location_dimension(
    input: &PositionReviewInput,
    last_ok: bool,
    atr_from_cost: Option<f64>,
) -> ReviewDimension {
    if !last_ok {
        return dimension(
            "location",
            "价位",
            "Location",
            DimensionTone::Unknown,
            "没有有效现价",
            "无法比较现价、成本与建仓/减仓带",
        );
    }
    let Some(buy_low) = input.buy_low.filter(|value| *value > 0.0) else {
        return dimension(
            "location",
            "价位",
            "Location",
            DimensionTone::Unknown,
            "参考价位带未就绪",
            format!(
                "现价 {:.2}，成本 {:.2}。等待日 K 计算出建仓/减仓带",
                input.last, input.avg_cost
            ),
        );
    };
    let buy_high = input.buy_high.unwrap_or(buy_low);
    let sell_low = input.sell_low.unwrap_or(buy_high);
    let last = input.last;
    let cost = input.avg_cost;
    let cost_vs_band = if cost <= buy_high {
        "成本落在/低于建仓带上沿"
    } else if cost >= sell_low {
        "成本偏高、靠近减仓带"
    } else {
        "成本介于建仓与减仓带之间"
    };
    let atr_text = atr_from_cost
        .map(|value| format!("；现价距成本 {value:+.1} 倍 ATR"))
        .unwrap_or_default();
    if near_sell_band(input, last) {
        dimension(
            "location",
            "价位",
            "Location",
            DimensionTone::Caution,
            "现价靠近参考减仓带",
            format!(
                "{cost_vs_band}。建仓 {buy_low:.2}–{buy_high:.2}，减仓约 {sell_low:.2}{atr_text}"
            ),
        )
    } else if near_buy_band(input, last) && last < cost {
        dimension(
            "location",
            "价位",
            "Location",
            DimensionTone::Neutral,
            "现价低于成本，仍在建仓带附近",
            format!("{cost_vs_band}。只说明位置，不自动等于补仓{atr_text}"),
        )
    } else if last < buy_low {
        dimension(
            "location",
            "价位",
            "Location",
            DimensionTone::Caution,
            "现价已低于建仓带下沿",
            format!("{cost_vs_band}。先核对失效价，而不是把更低的价格当成便宜{atr_text}"),
        )
    } else {
        dimension(
            "location",
            "价位",
            "Location",
            DimensionTone::Support,
            "现价仍在观察带内",
            format!("{cost_vs_band}。建仓 {buy_low:.2}–{buy_high:.2}{atr_text}"),
        )
    }
}

fn trend_dimension(input: &PositionReviewInput, last_ok: bool) -> ReviewDimension {
    let Some(score) = input.score.filter(|value| value.is_finite()) else {
        return dimension(
            "trend",
            "趋势",
            "Trend",
            DimensionTone::Unknown,
            "技术规则未就绪",
            "至少需要约 20 根日 K 才能判断趋势是否还支撑这笔成本",
        );
    };
    let regime = input.regime_label.as_deref().unwrap_or("未知");
    let ma_text = input
        .price_vs_ma20_pct
        .map(|value| format!("；现价相对 MA20 {value:+.1}%"))
        .unwrap_or_default();
    if !last_ok {
        return dimension(
            "trend",
            "趋势",
            "Trend",
            DimensionTone::Unknown,
            format!("{regime} · {score:.0} 分"),
            format!("现价缺失，趋势只能作背景{ma_text}"),
        );
    }
    if is_weak_regime(regime) || score < WEAK_SCORE {
        let tone = if input.unrealized_pnl_pct <= SOFT_LOSS_PCT {
            DimensionTone::Blocked
        } else {
            DimensionTone::Caution
        };
        dimension(
            "trend",
            "趋势",
            "Trend",
            tone,
            format!("{regime} · {score:.0} 分，不再支撑摊低成本"),
            format!("弱结构里继续加仓会放大回撤{ma_text}"),
        )
    } else if is_strong_regime(regime) && score >= 62.0 && input.unrealized_pnl_pct >= 0.0 {
        dimension(
            "trend",
            "趋势",
            "Trend",
            DimensionTone::Support,
            format!("{regime} · {score:.0} 分，仍覆盖成本"),
            format!("趋势尚未否定这笔持仓{ma_text}"),
        )
    } else {
        dimension(
            "trend",
            "趋势",
            "Trend",
            DimensionTone::Neutral,
            format!("{regime} · {score:.0} 分"),
            format!("方向不够鲜明，持仓以观望为主{ma_text}"),
        )
    }
}

fn risk_dimension(input: &PositionReviewInput, last_ok: bool) -> ReviewDimension {
    let climate = input.climate_label.as_deref().unwrap_or("未知");
    let weight = input
        .position_weight_pct
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| format!("；组合内约 {value:.1}%"))
        .unwrap_or_default();
    if input.stop_triggered
        || (last_ok
            && input
                .stop_price
                .is_some_and(|stop| stop > 0.0 && input.last <= stop))
    {
        return dimension(
            "risk",
            "风险",
            "Risk",
            DimensionTone::Blocked,
            "已触及失效/止损观察价",
            format!("市场{climate}。先处理这笔持仓，不再加仓{weight}"),
        );
    }
    if let Some(stop) = input.stop_price.filter(|value| *value > 0.0)
        && last_ok
    {
        let room_pct = (input.last / stop - 1.0) * 100.0;
        let atr_room = input
            .atr14
            .filter(|atr| *atr > 0.0)
            .map(|atr| format!("，约 {:.1} 倍 ATR", (input.last - stop) / atr))
            .unwrap_or_default();
        let tone = if room_pct <= 2.5 {
            DimensionTone::Caution
        } else {
            DimensionTone::Support
        };
        return dimension(
            "risk",
            "风险",
            "Risk",
            tone,
            format!("距失效价还有 {room_pct:.1}%"),
            format!("失效价 {stop:.2}{atr_room}。市场{climate}{weight}"),
        );
    }
    if input.new_entries_frozen {
        return dimension(
            "risk",
            "风险",
            "Risk",
            DimensionTone::Caution,
            format!("市场{climate}，今日不宜再开新风险"),
            format!("未设置失效价时，浮亏只能按纪律处理{weight}"),
        );
    }
    dimension(
        "risk",
        "风险",
        "Risk",
        DimensionTone::Caution,
        "尚未设置失效价",
        format!("没有止损观察价，就无法按计划衡量这笔持仓还剩多少风险。市场{climate}{weight}"),
    )
}

fn discipline_dimension(input: &PositionReviewInput) -> ReviewDimension {
    let mut missing = Vec::new();
    if input.stop_price.is_none_or(|value| value <= 0.0) {
        missing.push("失效/止损");
    }
    if input.take_profit.is_none_or(|value| value <= 0.0) {
        missing.push("止盈");
    }
    if !input.has_open_plan {
        missing.push("决策计划");
    }
    if input.take_profit_triggered {
        return dimension(
            "discipline",
            "纪律",
            "Plan",
            DimensionTone::Caution,
            "止盈观察已触发",
            "到价后应复盘是否按计划减仓，而不是凭感觉继续拿",
        );
    }
    if missing.is_empty() {
        let plan = match (
            input.plan_invalidation.as_deref(),
            input.plan_target.as_deref(),
        ) {
            (Some(invalidation), Some(target)) => {
                format!("计划失效 {invalidation}，目标 {target}")
            }
            _ => "买/止盈/止损三腿与计划齐全".into(),
        };
        return dimension(
            "discipline",
            "纪律",
            "Plan",
            DimensionTone::Support,
            "计划与提醒齐全",
            plan,
        );
    }
    let tone = if missing.contains(&"失效/止损") {
        DimensionTone::Caution
    } else {
        DimensionTone::Neutral
    };
    dimension(
        "discipline",
        "纪律",
        "Plan",
        tone,
        format!("还缺 {}", missing.join("、")),
        "提醒和计划由本地规则执行；AI 只解释，不会改价位",
    )
}

fn time_dimension(
    input: &PositionReviewInput,
    last_ok: bool,
    price_vs_cost_pct: f64,
) -> ReviewDimension {
    let Some(days) = input.held_calendar_days else {
        return dimension(
            "time",
            "时间",
            "Time",
            DimensionTone::Unknown,
            "无法判断持有多久",
            format!("已有 {} 笔成交，但缺少本轮开仓日期", input.trade_count),
        );
    };
    let stuck = last_ok && price_vs_cost_pct.abs() < TIME_DECAY_BAND_PCT;
    if days >= TIME_DECAY_DAYS && stuck {
        dimension(
            "time",
            "时间",
            "Time",
            DimensionTone::Caution,
            format!("已持有 {days} 日，价格仍贴着成本"),
            "超过常用 10 日观察窗仍未离开成本区，应到期复盘，而不是靠拖变成盈利",
        )
    } else if days >= 28 && !input.has_open_plan {
        dimension(
            "time",
            "时间",
            "Time",
            DimensionTone::Caution,
            format!("已持有 {days} 日，尚无到期复盘计划"),
            "长持仓更需要事先写明失效与目标",
        )
    } else if days < 3 {
        dimension(
            "time",
            "时间",
            "Time",
            DimensionTone::Neutral,
            format!("本轮持有 {days} 日"),
            "刚开仓，优先看计划是否成立，不急着加减",
        )
    } else {
        dimension(
            "time",
            "时间",
            "Time",
            DimensionTone::Support,
            format!("本轮持有 {days} 日"),
            "仍在观察窗内，继续对照成本和失效价",
        )
    }
}

fn near_buy_band(input: &PositionReviewInput, last: f64) -> bool {
    match (input.buy_low, input.buy_high) {
        (Some(low), Some(high)) if low > 0.0 && high > 0.0 => {
            last <= high * NEAR_BUY_HIGH_SLACK && last >= low * NEAR_BUY_LOW_SLACK
        }
        _ => false,
    }
}

fn near_sell_band(input: &PositionReviewInput, last: f64) -> bool {
    input
        .sell_low
        .is_some_and(|low| low > 0.0 && last >= low * NEAR_SELL_SLACK)
}

fn is_weak_regime(label: &str) -> bool {
    matches!(label, "防守" | "偏弱" | "Defensive" | "Weak")
}

fn is_defensive_regime(label: &str) -> bool {
    matches!(label, "防守" | "Defensive")
}

fn is_strong_regime(label: &str) -> bool {
    matches!(label, "强势" | "偏强" | "Strong" | "Constructive")
}

fn dimension(
    id: &str,
    title: &str,
    work_title: &str,
    tone: DimensionTone,
    headline: impl Into<String>,
    detail: impl Into<String>,
) -> ReviewDimension {
    ReviewDimension {
        id: id.into(),
        title: title.into(),
        work_title: work_title.into(),
        tone,
        headline: headline.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PositionReviewInput {
        PositionReviewInput {
            code: "600519".into(),
            shares: 100.0,
            avg_cost: 10.0,
            last: 10.4,
            unrealized_pnl: 40.0,
            unrealized_pnl_pct: 4.0,
            realized_pnl: 0.0,
            held_calendar_days: Some(6),
            trade_count: 1,
            quote_stale: false,
            regime_label: Some("偏强".into()),
            score: Some(66.0),
            rsi14: Some(55.0),
            price_vs_ma20_pct: Some(1.2),
            range_position_60_pct: Some(60.0),
            atr14: Some(0.25),
            buy_low: Some(9.4),
            buy_high: Some(10.1),
            sell_low: Some(11.2),
            sell_high: Some(11.8),
            stop_price: Some(9.2),
            take_profit: Some(11.2),
            stop_triggered: false,
            take_profit_triggered: false,
            position_weight_pct: Some(12.0),
            climate_label: Some("精选".into()),
            new_entries_frozen: false,
            has_open_plan: true,
            plan_invalidation: Some("9.20".into()),
            plan_target: Some("11.20".into()),
        }
    }

    fn dim<'a>(review: &'a PositionReview, id: &str) -> &'a ReviewDimension {
        review
            .dimensions
            .iter()
            .find(|item| item.id == id)
            .expect("dimension")
    }

    #[test]
    fn empty_or_invalid_lot_is_not_reviewed() {
        let mut input = base();
        input.shares = 0.0;
        assert!(analyze_position(&input).is_none());
        input = base();
        input.avg_cost = 0.0;
        assert!(analyze_position(&input).is_none());
    }

    #[test]
    fn profitable_hold_near_cost_stays_hold() {
        let review = analyze_position(&base()).unwrap();
        assert_eq!(review.stance, PositionReviewStance::Hold);
        assert!((review.price_vs_cost_pct - 4.0).abs() < 1e-9);
        assert!((review.atr_from_cost.unwrap() - 1.6).abs() < 1e-9);
        assert_eq!(review.dimensions.len(), 6);
        assert_eq!(dim(&review, "pnl").tone, DimensionTone::Neutral);
        assert_eq!(dim(&review, "discipline").tone, DimensionTone::Support);
    }

    #[test]
    fn stop_hit_forces_protect() {
        let mut input = base();
        input.last = 9.1;
        input.unrealized_pnl_pct = -9.0;
        let review = analyze_position(&input).unwrap();
        assert_eq!(review.stance, PositionReviewStance::Protect);
        assert_eq!(dim(&review, "risk").tone, DimensionTone::Blocked);
    }

    #[test]
    fn deep_loss_and_weak_tape_is_protect_not_add() {
        let mut input = base();
        input.last = 8.6;
        input.unrealized_pnl_pct = -14.0;
        input.regime_label = Some("偏弱".into());
        input.score = Some(38.0);
        input.buy_low = Some(8.4);
        input.buy_high = Some(8.8);
        let review = analyze_position(&input).unwrap();
        assert_eq!(review.stance, PositionReviewStance::Protect);
        assert_ne!(review.stance, PositionReviewStance::AddWatch);
        assert_eq!(dim(&review, "trend").tone, DimensionTone::Blocked);
    }

    #[test]
    fn below_cost_in_buy_band_can_watch_add() {
        let mut input = base();
        input.last = 9.8;
        input.unrealized_pnl = -20.0;
        input.unrealized_pnl_pct = -2.0;
        let review = analyze_position(&input).unwrap();
        assert_eq!(review.stance, PositionReviewStance::AddWatch);
        assert_eq!(dim(&review, "location").tone, DimensionTone::Neutral);
    }

    #[test]
    fn frozen_climate_blocks_add_on_a_losing_lot() {
        let mut input = base();
        input.last = 9.8;
        input.unrealized_pnl_pct = -8.5;
        input.new_entries_frozen = true;
        input.climate_label = Some("观望".into());
        let review = analyze_position(&input).unwrap();
        assert_eq!(review.stance, PositionReviewStance::Protect);
    }

    #[test]
    fn large_gain_near_sell_band_is_reduce_watch() {
        let mut input = base();
        input.last = 11.3;
        input.unrealized_pnl_pct = 13.0;
        input.rsi14 = Some(72.0);
        let review = analyze_position(&input).unwrap();
        assert_eq!(review.stance, PositionReviewStance::ReduceWatch);
        assert_eq!(dim(&review, "location").tone, DimensionTone::Caution);
    }

    #[test]
    fn missing_stop_is_discipline_caution() {
        let mut input = base();
        input.stop_price = None;
        input.take_profit = None;
        input.has_open_plan = false;
        let review = analyze_position(&input).unwrap();
        assert_eq!(dim(&review, "discipline").tone, DimensionTone::Caution);
        assert!(dim(&review, "discipline").headline.contains("失效"));
        assert_eq!(dim(&review, "risk").tone, DimensionTone::Caution);
    }

    #[test]
    fn time_decay_when_price_hugs_cost() {
        let mut input = base();
        input.held_calendar_days = Some(14);
        input.last = 10.1;
        input.unrealized_pnl_pct = 1.0;
        input.sell_low = Some(12.0);
        let review = analyze_position(&input).unwrap();
        assert_eq!(dim(&review, "time").tone, DimensionTone::Caution);
        assert!(dim(&review, "time").headline.contains("贴着成本"));
    }

    #[test]
    fn stale_quote_marks_pnl_unknown() {
        let mut input = base();
        input.quote_stale = true;
        let review = analyze_position(&input).unwrap();
        assert_eq!(dim(&review, "pnl").tone, DimensionTone::Unknown);
    }
}
