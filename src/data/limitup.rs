//! 连板梯队：首板 / 二连（二进一）/ 高板 / 炸板。
//!
//! 定位是**纪律化观察工具**，不是追涨按钮：
//! - 只用本地日 K + 实时快照推算，不预测明日涨跌；
//! - 高板、炸板、一字板、ST/退市、微成交额一律降权或直接跳过；
//! - 每个命中都给出观察价 / 失效价 / 目标价与盈亏比，方便接入现有
//!   “单笔亏损上限 + 失效价必须低于观察价”的仓位纪律；
//! - 打板天然盈亏比偏低（约 1.0），多数情况下**达不到**决策卡 1.5 的
//!   “符合策略”门槛——这是有意为之的诚实展示，不是 bug。
//!
//! 数据口径：
//! - 当日是否封板：优先用**不复权**的昨收 + 今开/高/低/收计算涨停价
//!   （四舍五入到分），收盘价贴近涨停价才算封住；
//! - 历史连板数：优先用 K 线自带的涨跌幅（不受前复权影响），回退到
//!   价格比较；前复权 high/low 会失真，炸板判断必须用不复权数据。

use serde::{Deserialize, Serialize};

/// 涨幅榜拉取行数（按涨跌幅降序，头部是今日封板，尾部覆盖昨日首板）。
pub const LIMITUP_UNIVERSE_N: usize = 150;
/// 单次扫描最多深评候选（每只 1 次不复权 K，控制耗时与请求量）。
pub const LIMITUP_PROBE_N: usize = 60;
/// 最终展示上限。
pub const LIMITUP_RESULT_N: usize = 20;
/// 拉取不复权日 K 根数（连板数 + 近5日炸板史够用）。
pub const LIMITUP_KLINE_LIMIT: usize = 90;

/// 一日行情（调用方负责提供**不复权**口径；历史可用涨跌幅兜底）。
#[derive(Debug, Clone, Default)]
pub struct RawDay {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// 当日涨跌幅（%），如 10.03。有它时连板判断不受复权影响。
    pub chg_pct: Option<f64>,
    /// 昨日收盘（不复权）。有它时可精确计算今日涨停价。
    pub prev_close: Option<f64>,
}

/// 当日实时快照（来自东财涨幅榜 clist，含昨收与高低开）。
#[derive(Debug, Clone, Default)]
pub struct TodayQuote {
    pub code: String,
    pub name: String,
    pub last: f64,
    pub change_pct: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub prev_close: f64,
    pub amount: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardDay {
    /// 收盘封住涨停。
    Sealed,
    /// 盘中触及涨停但收盘未封（炸板 / 冲高回落）。
    TouchedBroken,
    /// 未触板。
    NoBoard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitVerdict {
    /// 首板：刚封住第一板。
    FirstBoard,
    /// 二连板：昨日首板 + 今日继续封住（二进一成功）。
    SecondBoard,
    /// 三连及以上：情绪高位，只看不做。
    HigherBoard,
    /// 炸板：触板未封，当日不追。
    BrokenBoard,
    /// 二进一观察：昨日首板，今日尚未封住，等待确认。
    WatchSecondEntry,
    /// 跳过：ST/退/次新N字/成交额过小等。
    Skip,
}

impl LimitVerdict {
    pub fn label(self) -> &'static str {
        match self {
            Self::FirstBoard => "首板",
            Self::SecondBoard => "二连板",
            Self::HigherBoard => "高板",
            Self::BrokenBoard => "炸板",
            Self::WatchSecondEntry => "二进一观察",
            Self::Skip => "跳过",
        }
    }

    pub fn rank_key(self) -> u8 {
        match self {
            Self::SecondBoard => 0,
            Self::FirstBoard => 1,
            Self::WatchSecondEntry => 2,
            Self::HigherBoard => 3,
            Self::BrokenBoard => 4,
            Self::Skip => 5,
        }
    }

    /// 工作模式下的中性标签（不出现中文策略词）。
    pub fn label_work(self) -> &'static str {
        match self {
            Self::FirstBoard => "1st",
            Self::SecondBoard => "2nd",
            Self::HigherBoard => "High",
            Self::BrokenBoard => "Break",
            Self::WatchSecondEntry => "Watch",
            Self::Skip => "Skip",
        }
    }
}

/// 单只连板命中。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitUpHit {
    pub code: String,
    pub name: String,
    pub close: f64,
    pub change_pct: f64,
    /// 含今日在内的连续封板数。
    pub streak: usize,
    /// 昨日收盘时的连续封板数（用于判断二进一）。
    pub yesterday_streak: usize,
    pub verdict: LimitVerdict,
    /// 0–100 封板质量分（越高封得越“干净”，不是明日上涨概率）。
    pub score: f64,
    pub is_one_word: bool,
    pub amount: f64,
    /// 建议观察价（今日收盘附近，次日不追高）。
    pub observation: f64,
    /// 建议失效价（跌破即认错）。
    pub invalidation: f64,
    /// 参考目标价（再冲一板的理论位置）。
    pub target: f64,
    pub risk_reward: Option<f64>,
    pub limit_pct: f64,
    pub limit_price_today: f64,
    pub reasons: Vec<String>,
    pub risks: Vec<String>,
    pub headline: String,
}

impl LimitUpHit {
    pub fn price_band_text(&self) -> String {
        format!(
            "观察 {} · 失效 {} · 目标 {}",
            fmt_px(self.observation),
            fmt_px(self.invalidation),
            fmt_px(self.target)
        )
    }
}

// —— 涨停规则 ——

/// 是否 ST（含 *ST / ST / S*ST，不区分大小写）。
pub fn is_st(name: &str) -> bool {
    name.to_ascii_uppercase().contains("ST")
}

pub fn is_delisted_or_risk(name: &str) -> bool {
    name.contains('退') || name.contains("摘牌")
}

/// 是否次新未开板常见前缀（N/C 开头）。
pub fn is_new_listing_prefix(name: &str) -> bool {
    name.starts_with('N') || name.starts_with('n') || name.starts_with('C') || name.starts_with('c')
}

/// 该标的适用的涨停幅度（小数，如 0.10）。
///
/// - 主板：10%；ST 主板：5%
/// - 创业板 300/301、科创板 688/689：20%（含 ST，规则如此）
/// - 北交所 4/8 开头：30%
/// - 港股/其他：返回 0.10 仅作展示，调用方不应把港股纳入连板池
pub fn limit_pct_for(code: &str, name: &str) -> f64 {
    let st = is_st(name);
    if code.starts_with("300") || code.starts_with("301") {
        return 0.20;
    }
    if code.starts_with("688") || code.starts_with("689") {
        return 0.20;
    }
    if code.starts_with('4') || code.starts_with('8') || code.starts_with("92") {
        // 北交所：30%（含 43/83/87/92 段）
        return 0.30;
    }
    if st {
        return 0.05;
    }
    0.10
}

/// 涨停价：昨收 × (1+幅度)，四舍五入到分。
pub fn limit_price(prev_close: f64, limit_pct: f64) -> f64 {
    if !prev_close.is_finite() || prev_close <= 0.0 {
        return 0.0;
    }
    ((prev_close * (1.0 + limit_pct)) * 100.0).round() / 100.0
}

/// 涨跌幅是否达到封板线（含 0.5 个百分点容差，应对四舍五入）。
pub fn is_limit_chg(chg_pct: f64, limit_pct: f64) -> bool {
    chg_pct.is_finite() && chg_pct >= limit_pct * 100.0 - 0.5
}

/// 单日分类。`prev_close` 必须是不复权昨收。
pub fn classify_day(
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    chg_pct: Option<f64>,
    prev_close: f64,
    limit_pct: f64,
) -> BoardDay {
    let target = limit_price(prev_close, limit_pct);
    if !(target.is_finite() && target > 0.0) {
        return BoardDay::NoBoard;
    }
    // 收盘封住：价格贴近涨停价，或涨跌幅达线且收盘≈全天最高（前复权兜底）。
    let sealed_by_price =
        close.is_finite() && close > 0.0 && close >= target * 0.998 && close <= target * 1.005;
    let sealed_by_chg = chg_pct.is_some_and(|c| is_limit_chg(c, limit_pct))
        && close.is_finite()
        && high.is_finite()
        && close >= high * 0.998;
    if sealed_by_price || sealed_by_chg {
        return BoardDay::Sealed;
    }
    // 炸板：最高触及涨停但收盘未封。
    if high.is_finite() && high >= target * 0.995 {
        return BoardDay::TouchedBroken;
    }
    // 前复权兜底：涨跌幅达线但 high 失真时，至少标为触板，避免漏掉炸板提示。
    if chg_pct.is_some_and(|c| c >= limit_pct * 100.0 - 0.5)
        && !(open.is_finite() && high.is_finite() && low.is_finite())
    {
        return BoardDay::TouchedBroken;
    }
    let _ = open;
    let _ = low;
    BoardDay::NoBoard
}

/// 尾部连续封板数（days 按时间从旧到新，含今日在最后一位）。
pub fn trailing_streak(days: &[BoardDay]) -> usize {
    days.iter()
        .rev()
        .take_while(|d| **d == BoardDay::Sealed)
        .count()
}

fn classify_history(history: &[RawDay], code: &str, name: &str) -> Vec<BoardDay> {
    let limit_pct = limit_pct_for(code, name);
    let mut out = Vec::with_capacity(history.len());
    let mut prev = history.first().and_then(|d| d.prev_close).unwrap_or(0.0);
    for (i, day) in history.iter().enumerate() {
        // 优先用本行自带的 prev_close，否则用前一根收盘（不复权假设下成立）。
        let pc = if day.prev_close.is_some_and(|v| v > 0.0) {
            day.prev_close.unwrap()
        } else if i == 0 {
            prev
        } else {
            let p = history[i - 1].close;
            if p > 0.0 { p } else { prev }
        };
        if pc > 0.0 {
            prev = pc;
        }
        out.push(classify_day(
            day.open,
            day.high,
            day.low,
            day.close,
            day.chg_pct,
            pc,
            limit_pct,
        ));
    }
    out
}

// —— 评估 ——

/// 评估一只今日候选。`history` 为不含今日的历史（旧→新），`today` 为今日快照。
pub fn evaluate(today: &TodayQuote, history: &[RawDay]) -> Option<LimitUpHit> {
    if today.last <= 0.0 || !today.last.is_finite() {
        return None;
    }
    let limit_pct = limit_pct_for(&today.code, &today.name);
    let limit_price_today = limit_price(today.prev_close, limit_pct);

    let mut reasons = Vec::new();
    let mut risks = Vec::new();

    // —— 硬过滤：问题股直接 Skip ——
    if is_delisted_or_risk(&today.name) {
        return Some(skip_hit(
            today,
            limit_pct,
            limit_price_today,
            0,
            0,
            "退市/风险警示整理",
        ));
    }
    if is_st(&today.name) {
        return Some(skip_hit(
            today,
            limit_pct,
            limit_price_today,
            0,
            0,
            "ST 涨跌幅受限且流动性差，不纳入连板池",
        ));
    }
    if is_new_listing_prefix(&today.name) {
        return Some(skip_hit(
            today,
            limit_pct,
            limit_price_today,
            0,
            0,
            "次新未开板（N/C字），波动极大",
        ));
    }
    // 港股无涨跌停，不纳入。
    if today.code.len() == 5 && today.code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    // —— 今日分类 ——
    let today_kind = classify_day(
        today.open,
        today.high,
        today.low,
        today.last,
        Some(today.change_pct),
        today.prev_close,
        limit_pct,
    );

    // —— 历史连板 ——
    let hist_kinds = classify_history(history, &today.code, &today.name);
    let yesterday_streak = trailing_streak(&hist_kinds);

    let mut chain = hist_kinds;
    chain.push(today_kind);
    let streak = trailing_streak(&chain);

    // 一字板：开=高=低=收且封住（散户基本排不上）。
    let is_one_word = today_kind == BoardDay::Sealed
        && today.open > 0.0
        && (today.open - today.high).abs() <= 0.005
        && (today.high - today.low).abs() <= 0.01
        && (today.low - today.last).abs() <= 0.01;

    // —— 基础分：封板质量 ——
    let mut score = 50.0;
    match today_kind {
        BoardDay::Sealed => {
            score += 12.0;
            reasons.push(format!("今日封住涨停（{:.0}%板）", limit_pct * 100.0));
        }
        BoardDay::TouchedBroken => {
            score -= 18.0;
            risks.push("今日炸板：触及涨停未封住".into());
        }
        BoardDay::NoBoard => {
            // 未封住但昨日首板 → 二进一观察
            if yesterday_streak == 1 {
                score += 2.0;
                reasons.push("昨日首板，今日待确认".into());
            } else {
                return None;
            }
        }
    }

    match streak {
        1 => {
            score += 6.0;
            reasons.push("首板".into());
        }
        2 => {
            score += 10.0;
            reasons.push("二连板（二进一成功）".into());
        }
        3 => {
            score += 2.0;
            risks.push("三连板：情绪高位，次日分歧概率大".into());
        }
        4.. => {
            score -= 10.0;
            risks.push(format!("{streak}连板：高位接力风险极大，只看不做"));
        }
        _ => {}
    }

    if is_one_word {
        score -= 8.0;
        risks.push("一字板：无换手，散户基本排不上，次日开板即风险".into());
    }

    // 成交额：太小封不住也出不来，太大可能是尾盘偷袭。
    if today.amount > 0.0 && today.amount < 100_000_000.0 {
        score -= 10.0;
        risks.push("成交额不足1亿：封板质量与流动性存疑".into());
    } else if today.amount >= 100_000_000.0 {
        score += 4.0;
        reasons.push("成交额过亿，有换手".into());
    }

    // 炸板史：近5日出现过炸板则减分。
    let recent_breaks = chain
        .iter()
        .rev()
        .skip(if today_kind == BoardDay::Sealed { 1 } else { 0 })
        .take(5)
        .filter(|d| **d == BoardDay::TouchedBroken)
        .count();
    if recent_breaks > 0 {
        score -= 6.0 * recent_breaks as f64;
        risks.push(format!("近5日出现过{recent_breaks}次炸板"));
    }

    // 昨日是炸板今日反包封住：质量存疑。
    if today_kind == BoardDay::Sealed
        && chain
            .get(chain.len().saturating_sub(2))
            .is_some_and(|d| *d == BoardDay::TouchedBroken)
    {
        score -= 5.0;
        risks.push("昨日炸板今日反包：分歧较大".into());
    }

    // —— 三价（纪律用，非买卖建议） ——
    let observation = today.last;
    // 失效：昨日收盘与今日最低的较高者（跌破即认错）；兜底 -1×板幅一半。
    let mut invalidation = today.prev_close.max(today.low * 0.99);
    if !(invalidation.is_finite() && invalidation > 0.0 && invalidation < observation) {
        invalidation = observation * (1.0 - limit_pct / 2.0);
    }
    let target = limit_price(observation, limit_pct);
    let risk_reward = if observation > invalidation && target > observation {
        Some((target - observation) / (observation - invalidation))
    } else {
        None
    };
    if risk_reward.is_some_and(|rr| rr < 1.5) {
        risks.push("盈亏比不足1.5：打板天然风险大收益薄".into());
    }

    // —— 结论 ——
    let mut verdict = match (today_kind, streak, yesterday_streak) {
        (BoardDay::TouchedBroken, _, _) => LimitVerdict::BrokenBoard,
        (BoardDay::Sealed, 1, _) => LimitVerdict::FirstBoard,
        (BoardDay::Sealed, 2, _) => LimitVerdict::SecondBoard,
        (BoardDay::Sealed, _, _) => LimitVerdict::HigherBoard,
        (BoardDay::NoBoard, _, 1) => LimitVerdict::WatchSecondEntry,
        _ => LimitVerdict::Skip,
    };
    // 高板强制只看不做；炸板不给观察结论。
    if verdict == LimitVerdict::HigherBoard && streak >= 4 {
        risks.push("4连板以上不参与接力".into());
    }
    if today.amount > 0.0 && today.amount < 50_000_000.0 {
        verdict = LimitVerdict::Skip;
        risks.push("成交额不足5000万，直接跳过".into());
    }

    score = score.clamp(0.0, 100.0);
    let headline = match verdict {
        LimitVerdict::FirstBoard => {
            format!("首板 · {} · {streak}连 · 分{score:.0}", fmt_px(today.last))
        }
        LimitVerdict::SecondBoard => format!(
            "二连板 · {} · 二进一成功 · 分{score:.0}",
            fmt_px(today.last)
        ),
        LimitVerdict::HigherBoard => format!(
            "{streak}连板 · 高位 · 分{score:.0} · 只看不做",
            score = score
        ),
        LimitVerdict::BrokenBoard => {
            format!("炸板 · {} · 当日不追 · 分{score:.0}", fmt_px(today.last))
        }
        LimitVerdict::WatchSecondEntry => format!(
            "二进一观察 · 昨日首板 · 现价{} · 等确认",
            fmt_px(today.last)
        ),
        LimitVerdict::Skip => format!("跳过 · 分{score:.0}"),
    };

    if reasons.is_empty() {
        reasons.push("连板形态与封板质量综合".into());
    }

    Some(LimitUpHit {
        code: today.code.clone(),
        name: today.name.clone(),
        close: today.last,
        change_pct: today.change_pct,
        streak,
        yesterday_streak,
        verdict,
        score,
        is_one_word,
        amount: today.amount,
        observation: round_px(observation),
        invalidation: round_px(invalidation),
        target: round_px(target),
        risk_reward,
        limit_pct,
        limit_price_today: round_px(limit_price_today),
        reasons,
        risks,
        headline,
    })
}

fn skip_hit(
    today: &TodayQuote,
    limit_pct: f64,
    limit_price_today: f64,
    streak: usize,
    yesterday_streak: usize,
    reason: &str,
) -> LimitUpHit {
    LimitUpHit {
        code: today.code.clone(),
        name: today.name.clone(),
        close: today.last,
        change_pct: today.change_pct,
        streak,
        yesterday_streak,
        verdict: LimitVerdict::Skip,
        score: 0.0,
        is_one_word: false,
        amount: today.amount,
        observation: round_px(today.last),
        invalidation: round_px(today.last * 0.95),
        target: round_px(today.last * 1.05),
        risk_reward: None,
        limit_pct,
        limit_price_today: round_px(limit_price_today),
        reasons: vec![],
        risks: vec![reason.to_string()],
        headline: format!("跳过 · {reason}"),
    }
}

/// 按结论排序：二连 > 首板 > 二进一观察 > 高板 > 炸板 > 跳过，同级按分。
pub fn sort_hits(hits: &mut [LimitUpHit]) {
    hits.sort_by(|a, b| {
        a.verdict
            .rank_key()
            .cmp(&b.verdict.rank_key())
            .then(
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then_with(|| b.streak.cmp(&a.streak))
    });
}

/// 本地摘要（列表为空时也给出纪律提示）。
pub fn local_summary(hits: &[LimitUpHit]) -> String {
    if hits.is_empty() {
        return "今日未发现符合纪律的连板候选：可能无封板、或多为ST/炸板/微成交额。\
                 连板次日低开即弱，不追高；仅供学习研究，不构成投资建议。"
            .into();
    }
    let first = hits
        .iter()
        .filter(|h| h.verdict == LimitVerdict::FirstBoard)
        .count();
    let second = hits
        .iter()
        .filter(|h| h.verdict == LimitVerdict::SecondBoard)
        .count();
    let watch = hits
        .iter()
        .filter(|h| h.verdict == LimitVerdict::WatchSecondEntry)
        .count();
    let top = hits
        .iter()
        .take(3)
        .map(|h| format!("{}({})", h.name, h.verdict.label()))
        .collect::<Vec<_>>()
        .join("、");
    format!(
        "连板梯队 {} 只：首板 {first} · 二连 {second} · 二进一观察 {watch}。\
          靠前：{top}。高板与炸板只看不做，打板必须先设失效价；仅供学习研究，不构成投资建议。",
        hits.len()
    )
}

fn round_px(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn fmt_px(v: f64) -> String {
    if v >= 1000.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(close: f64, prev: f64) -> RawDay {
        let chg = (close / prev - 1.0) * 100.0;
        RawDay {
            open: prev,
            high: close.max(prev),
            low: close.min(prev),
            close,
            chg_pct: Some(chg),
            prev_close: Some(prev),
        }
    }

    fn quote(code: &str, name: &str, last: f64, prev: f64) -> TodayQuote {
        TodayQuote {
            code: code.into(),
            name: name.into(),
            last,
            change_pct: (last / prev - 1.0) * 100.0,
            open: prev,
            high: last,
            low: prev,
            prev_close: prev,
            amount: 300_000_000.0,
        }
    }

    #[test]
    fn limit_pct_covers_main_chinext_star_bse_and_st() {
        assert_eq!(limit_pct_for("600519", "贵州茅台"), 0.10);
        assert_eq!(limit_pct_for("000001", "平安银行"), 0.10);
        assert_eq!(limit_pct_for("300750", "宁德时代"), 0.20);
        assert_eq!(limit_pct_for("688981", "中芯国际"), 0.20);
        assert_eq!(limit_pct_for("430047", "北交所示例"), 0.30);
        assert_eq!(limit_pct_for("600001", "*ST大药"), 0.05);
        // 创业板 ST 仍是 20% 板
        assert_eq!(limit_pct_for("300001", "ST特锐德"), 0.20);
    }

    #[test]
    fn limit_price_rounds_to_fen() {
        // 10.03 × 1.1 = 11.033 → 11.03
        assert_eq!(limit_price(10.03, 0.10), 11.03);
        assert_eq!(limit_price(10.00, 0.10), 11.00);
        assert_eq!(limit_price(20.00, 0.20), 24.00);
    }

    #[test]
    fn first_board_detected() {
        let hist = vec![day(10.00, 10.05), day(10.10, 10.00)];
        let today = quote("600001", "测试", 11.11, 10.10);
        let hit = evaluate(&today, &hist).unwrap();
        assert_eq!(hit.verdict, LimitVerdict::FirstBoard);
        assert_eq!(hit.streak, 1);
        assert_eq!(hit.yesterday_streak, 0);
        assert!(hit.score > 50.0);
    }

    #[test]
    fn second_board_is_two_in_a_row() {
        // 昨日首板 + 今日继续封住 = 二连
        let hist = vec![day(10.00, 10.05), day(11.00, 10.00)];
        let today = quote("600002", "测试二", 12.10, 11.00);
        let hit = evaluate(&today, &hist).unwrap();
        assert_eq!(hit.streak, 2);
        assert_eq!(hit.yesterday_streak, 1);
        assert_eq!(hit.verdict, LimitVerdict::SecondBoard);
    }

    #[test]
    fn watch_second_entry_when_yesterday_first_board_but_today_open() {
        let hist = vec![day(10.00, 10.05), day(11.00, 10.00)];
        let mut today = quote("600003", "测试三", 11.30, 11.00);
        today.high = 11.40;
        today.low = 10.90;
        let hit = evaluate(&today, &hist).unwrap();
        assert_eq!(hit.verdict, LimitVerdict::WatchSecondEntry);
        assert_eq!(hit.yesterday_streak, 1);
        assert_eq!(hit.streak, 0);
    }

    #[test]
    fn broken_board_is_flagged_and_not_chased() {
        let hist = vec![day(10.00, 10.05)];
        let mut today = quote("600004", "测试四", 10.50, 10.00);
        // 最高触及涨停 11.00 但收回 10.50
        today.high = 11.00;
        today.low = 10.00;
        let hit = evaluate(&today, &hist).unwrap();
        assert_eq!(hit.verdict, LimitVerdict::BrokenBoard);
        assert!(hit.risks.iter().any(|r| r.contains("炸板")));
    }

    #[test]
    fn high_board_is_downgraded() {
        let mut hist = vec![day(10.00, 10.10)];
        let mut prev = 10.0;
        // 构造 4 连板历史
        for _ in 0..4 {
            let close = limit_price(prev, 0.10);
            hist.push(day(close, prev));
            prev = close;
        }
        let today = quote("600005", "高标", limit_price(prev, 0.10), prev);
        let hit = evaluate(&today, &hist).unwrap();
        assert_eq!(hit.verdict, LimitVerdict::HigherBoard);
        assert!(hit.streak >= 5);
        assert!(
            hit.risks
                .iter()
                .any(|r| r.contains("只看") || r.contains("高位"))
        );
    }

    #[test]
    fn st_is_skipped() {
        let hist = vec![day(2.00, 2.00)];
        let today = quote("600006", "*ST测试", 2.10, 2.00);
        let hit = evaluate(&today, &hist).unwrap();
        assert_eq!(hit.verdict, LimitVerdict::Skip);
        assert_eq!(hit.score, 0.0);
    }

    #[test]
    fn tiny_amount_is_skipped() {
        let hist = vec![day(10.00, 10.05)];
        let mut today = quote("600007", "小票", 11.00, 10.00);
        today.amount = 30_000_000.0;
        let hit = evaluate(&today, &hist).unwrap();
        assert_eq!(hit.verdict, LimitVerdict::Skip);
    }

    #[test]
    fn one_word_board_warns_unfillable() {
        let hist = vec![day(10.00, 10.05)];
        let mut today = quote("600008", "一字", 11.00, 10.00);
        today.open = 11.00;
        today.high = 11.00;
        today.low = 11.00;
        let hit = evaluate(&today, &hist).unwrap();
        assert!(hit.is_one_word);
        assert!(hit.risks.iter().any(|r| r.contains("一字板")));
    }

    #[test]
    fn risk_reward_is_honest_about_thin_payoff() {
        let hist = vec![day(10.00, 10.05)];
        let today = quote("600009", "盈亏", 11.00, 10.00);
        let hit = evaluate(&today, &hist).unwrap();
        // 10% 风险距离 vs 10% 目标 → 盈亏比约 1.0，不足 1.5，要如实提示
        let rr = hit.risk_reward.unwrap();
        assert!(rr < 1.5, "rr={rr}");
        assert!(hit.risks.iter().any(|r| r.contains("盈亏比")));
    }

    #[test]
    fn sort_puts_second_before_first_before_watch() {
        let mk = |verdict, score| LimitUpHit {
            code: "x".into(),
            name: "x".into(),
            close: 10.0,
            change_pct: 10.0,
            streak: 1,
            yesterday_streak: 0,
            verdict,
            score,
            is_one_word: false,
            amount: 0.0,
            observation: 10.0,
            invalidation: 9.0,
            target: 11.0,
            risk_reward: None,
            limit_pct: 0.10,
            limit_price_today: 11.0,
            reasons: vec![],
            risks: vec![],
            headline: String::new(),
        };
        let mut hits = vec![
            mk(LimitVerdict::WatchSecondEntry, 90.0),
            mk(LimitVerdict::FirstBoard, 60.0),
            mk(LimitVerdict::SecondBoard, 70.0),
        ];
        sort_hits(&mut hits);
        assert_eq!(hits[0].verdict, LimitVerdict::SecondBoard);
        assert_eq!(hits[1].verdict, LimitVerdict::FirstBoard);
        assert_eq!(hits[2].verdict, LimitVerdict::WatchSecondEntry);
    }

    #[test]
    fn summary_mentions_disclaimer() {
        assert!(local_summary(&[]).contains("不构成投资建议"));
    }
}
