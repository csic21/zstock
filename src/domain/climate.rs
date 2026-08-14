//! Market climate playbook.
//!
//! This module does not predict returns. It only decides whether the current
//! tape supports *new* entries, which playbooks remain eligible, and how much
//! of the user's loss budget should stay armed. Existing stop / review work
//! is never suppressed.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MarketClimate {
    Attack,
    Select,
    Defend,
    StandAside,
}

impl MarketClimate {
    pub fn label(self) -> &'static str {
        match self {
            Self::Attack => "进攻",
            Self::Select => "精选",
            Self::Defend => "防守",
            Self::StandAside => "观望",
        }
    }

    pub fn work_label(self) -> &'static str {
        match self {
            Self::Attack => "scale-up",
            Self::Select => "selective",
            Self::Defend => "defensive",
            Self::StandAside => "hold",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewEntryStance {
    Open,
    Selective,
    Freeze,
}

impl NewEntryStance {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "可按计划新开仓",
            Self::Selective => "只保留高质量计划",
            Self::Freeze => "不宜新开仓",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybookKind {
    LowPosition,
    Pullback,
    Breakout,
    OversoldBounce,
}

impl PlaybookKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::LowPosition => "低位策略",
            Self::Pullback => "强势回踩",
            Self::Breakout => "放量突破",
            Self::OversoldBounce => "超跌反弹",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexMove {
    pub name: String,
    pub change_pct: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClimateEvidence {
    pub indices: Vec<IndexMove>,
    pub stock_advances: Option<u64>,
    pub stock_declines: Option<u64>,
    pub stock_unchanged: Option<u64>,
    pub sector_advances: Option<u64>,
    pub sector_declines: Option<u64>,
    pub sector_unchanged: Option<u64>,
    pub sector_average_change: Option<f64>,
    pub open_positions: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClimateReport {
    pub climate: MarketClimate,
    pub stance: NewEntryStance,
    pub risk_scale: f64,
    pub completeness_pct: f64,
    pub fear_greed_score: Option<f64>,
    pub headline: String,
    pub detail: String,
    pub reasons: Vec<String>,
}

impl Default for ClimateReport {
    fn default() -> Self {
        assess_market_climate(&ClimateEvidence::default())
    }
}

impl ClimateReport {
    pub fn sizing_note(&self) -> Option<String> {
        if self.stance == NewEntryStance::Freeze {
            return Some("今日市场观望，不新增仓位；先处理持仓风险与到期复盘".into());
        }
        if self.risk_scale < 0.999 {
            return Some(format!(
                "市场{}，单笔亏损上限已按 {:.0}% 缩放，避免在弱结构里把预算打满",
                self.climate.label(),
                self.risk_scale * 100.0
            ));
        }
        None
    }
}

pub fn assess_market_climate(evidence: &ClimateEvidence) -> ClimateReport {
    let index_changes: Vec<f64> = evidence
        .indices
        .iter()
        .map(|item| item.change_pct)
        .filter(|value| value.is_finite())
        .collect();
    let stock_total = sum3(
        evidence.stock_advances,
        evidence.stock_declines,
        evidence.stock_unchanged,
    );
    let sector_total = sum3(
        evidence.sector_advances,
        evidence.sector_declines,
        evidence.sector_unchanged,
    );
    let stock_breadth = breadth_score(
        evidence.stock_advances,
        evidence.stock_declines,
        evidence.stock_unchanged,
    );
    let sector_breadth = breadth_score(
        evidence.sector_advances,
        evidence.sector_declines,
        evidence.sector_unchanged,
    );
    let completeness_pct = completeness(
        !index_changes.is_empty(),
        stock_total.is_some_and(|total| total > 0),
        sector_total.is_some_and(|total| total > 0),
    );
    let index_mean = mean(&index_changes);
    let down_indices = index_changes
        .iter()
        .filter(|value| **value <= -0.40)
        .count();
    let up_indices = index_changes.iter().filter(|value| **value >= 0.30).count();
    let fear_greed_score = (completeness_pct >= 40.0).then(|| {
        composite_tape_score(
            stock_breadth,
            sector_breadth,
            index_mean,
            evidence.sector_average_change,
        )
    });

    let mut reasons = Vec::new();
    if let Some(mean) = index_mean {
        reasons.push(format!(
            "主要指数 {} 下跌 / {} 上涨，均值 {:+.2}%",
            down_indices, up_indices, mean
        ));
        for index in evidence.indices.iter().take(3) {
            if index.change_pct.is_finite() {
                reasons.push(format!("{} {:+.2}%", index.name, index.change_pct));
            }
        }
    }
    if let Some(breadth) = stock_breadth {
        reasons.push(format!("上涨家数占比 {:.0}%", breadth));
    }
    if let Some(breadth) = sector_breadth {
        reasons.push(format!("上涨行业占比 {:.0}%", breadth));
    }
    if evidence.open_positions >= 5 {
        reasons.push(format!(
            "已有 {} 笔持仓，组合热度偏高，新开仓更应克制",
            evidence.open_positions
        ));
    }

    let mut climate = if completeness_pct < 40.0 {
        reasons.insert(0, "指数或市场宽度证据不足，默认按精选处理".into());
        MarketClimate::Select
    } else {
        classify_tape(stock_breadth, down_indices, up_indices, index_mean)
    };

    if evidence.open_positions >= 8 {
        climate = more_defensive(climate, MarketClimate::Defend);
        reasons.push("持仓数量达到 8 笔，气候上限压到防守".into());
    } else if evidence.open_positions >= 5 && climate == MarketClimate::Attack {
        climate = MarketClimate::Select;
        reasons.push("持仓已偏多，即使宽度配合也不再按进攻处理".into());
    }

    let (stance, risk_scale) = match climate {
        MarketClimate::Attack => (NewEntryStance::Open, 1.0),
        MarketClimate::Select => (NewEntryStance::Selective, 0.70),
        MarketClimate::Defend => (NewEntryStance::Selective, 0.40),
        MarketClimate::StandAside => (NewEntryStance::Freeze, 0.0),
    };
    let headline = match (climate, completeness_pct < 40.0) {
        (_, true) => "市场证据不足，按精选处理".into(),
        (MarketClimate::Attack, _) => "市场宽度配合，允许按计划进攻".into(),
        (MarketClimate::Select, _) => "结构分化，只做高质量回踩与低位".into(),
        (MarketClimate::Defend, _) => "宽度偏弱，先防守，不扩散新仓".into(),
        (MarketClimate::StandAside, _) => "普跌或宽度恶化，今日不宜新开仓".into(),
    };
    let detail = match climate {
        MarketClimate::Attack => "突破也可看，但仍先处理失效价；涨幅已大的不追。".into(),
        MarketClimate::Select => {
            "突破和超跌默认等待；只有回踩/低位且匹配度够高才保留为符合观察条件。".into()
        }
        MarketClimate::Defend => {
            "不追突破、不抄超跌；只保留高质量回踩或低位，并把单笔风险缩小。".into()
        }
        MarketClimate::StandAside => {
            "今日候选全部暂缓新开仓。优先核对持仓失效、集中度和到期复盘。".into()
        }
    };
    reasons.truncate(5);

    ClimateReport {
        climate,
        stance,
        risk_scale,
        completeness_pct,
        fear_greed_score,
        headline,
        detail,
        reasons,
    }
}

/// Returns `Some(reason)` when a technically ready setup should not stay ready.
pub fn gate_playbook(report: &ClimateReport, kind: PlaybookKind, score: f64) -> Option<String> {
    let score = if score.is_finite() { score } else { 0.0 };
    match report.climate {
        MarketClimate::StandAside => Some(format!("市场{}：今日不新开仓", report.climate.label())),
        MarketClimate::Attack => match kind {
            PlaybookKind::OversoldBounce if score < 68.0 => {
                Some("即使市场偏强，超跌也只保留匹配度很高的博弈".into())
            }
            PlaybookKind::Breakout if score < 58.0 => {
                Some("突破匹配度不足，避免在强势里追劣质放量".into())
            }
            _ => None,
        },
        MarketClimate::Select => match kind {
            PlaybookKind::Breakout if score < 78.0 => Some("精选日不追突破，除非匹配度很高".into()),
            PlaybookKind::OversoldBounce => Some("精选日不做超跌博弈".into()),
            PlaybookKind::Pullback if score < 58.0 => Some("精选日只保留质量足够的回踩".into()),
            PlaybookKind::LowPosition if score < 62.0 => {
                Some("精选日只保留达到关注门槛的低位".into())
            }
            _ => None,
        },
        MarketClimate::Defend => match kind {
            PlaybookKind::Breakout => Some("防守日不追突破".into()),
            PlaybookKind::OversoldBounce => Some("防守日不抄超跌".into()),
            PlaybookKind::Pullback if score < 70.0 => Some("防守日只保留高质量回踩".into()),
            PlaybookKind::LowPosition if score < 68.0 => Some("防守日只保留高质量低位".into()),
            _ => None,
        },
    }
}

fn classify_tape(
    stock_breadth: Option<f64>,
    down_indices: usize,
    up_indices: usize,
    index_mean: Option<f64>,
) -> MarketClimate {
    let breadth = stock_breadth.unwrap_or(50.0);
    let mean = index_mean.unwrap_or(0.0);
    if (down_indices >= 2 && mean <= -0.80) || breadth < 28.0 {
        MarketClimate::StandAside
    } else if down_indices >= 2 || breadth < 40.0 || mean <= -0.50 {
        MarketClimate::Defend
    } else if breadth >= 86.0 && mean >= 1.20 {
        // Extreme breadth is usually a chase day, not a high-quality entry day.
        MarketClimate::Select
    } else if up_indices >= 2 && breadth >= 58.0 && mean >= 0.25 {
        MarketClimate::Attack
    } else {
        MarketClimate::Select
    }
}

fn more_defensive(current: MarketClimate, floor: MarketClimate) -> MarketClimate {
    current.max(floor)
}

fn completeness(has_index: bool, has_stock_breadth: bool, has_sector_breadth: bool) -> f64 {
    let mut score = 0.0;
    if has_index {
        score += 40.0;
    }
    if has_stock_breadth {
        score += 40.0;
    }
    if has_sector_breadth {
        score += 20.0;
    }
    score
}

fn composite_tape_score(
    stock_breadth: Option<f64>,
    sector_breadth: Option<f64>,
    index_mean: Option<f64>,
    sector_average_change: Option<f64>,
) -> f64 {
    let stock = stock_breadth.unwrap_or(50.0);
    let sector = sector_breadth.unwrap_or(50.0);
    let index = signed_move_score(index_mean.unwrap_or(0.0));
    let sector_move = signed_move_score(sector_average_change.unwrap_or(0.0));
    (stock * 0.45 + sector * 0.25 + index * 0.20 + sector_move * 0.10).clamp(0.0, 100.0)
}

fn signed_move_score(change_pct: f64) -> f64 {
    (50.0 + change_pct.clamp(-5.0, 5.0) * 10.0).clamp(0.0, 100.0)
}

fn breadth_score(up: Option<u64>, down: Option<u64>, flat: Option<u64>) -> Option<f64> {
    let total = sum3(up, down, flat)?;
    if total == 0 {
        return None;
    }
    Some(
        ((up.unwrap_or(0) as f64 + flat.unwrap_or(0) as f64 * 0.5) / total as f64 * 100.0)
            .clamp(0.0, 100.0),
    )
}

fn sum3(a: Option<u64>, b: Option<u64>, c: Option<u64>) -> Option<u64> {
    match (a, b, c) {
        (None, None, None) => None,
        _ => Some(a.unwrap_or(0) + b.unwrap_or(0) + c.unwrap_or(0)),
    }
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indices(changes: &[(&str, f64)]) -> Vec<IndexMove> {
        changes
            .iter()
            .map(|(name, change)| IndexMove {
                name: (*name).into(),
                change_pct: *change,
            })
            .collect()
    }

    fn broad(up: u64, down: u64) -> ClimateEvidence {
        ClimateEvidence {
            indices: indices(&[("上证综指", 0.2), ("沪深300", 0.1), ("创业板指", 0.0)]),
            stock_advances: Some(up),
            stock_declines: Some(down),
            stock_unchanged: Some(50),
            sector_advances: Some(40),
            sector_declines: Some(40),
            sector_unchanged: Some(6),
            sector_average_change: Some(0.0),
            open_positions: 1,
        }
    }

    #[test]
    fn missing_tape_defaults_to_selective_not_attack() {
        let report = assess_market_climate(&ClimateEvidence::default());
        assert_eq!(report.climate, MarketClimate::Select);
        assert_eq!(report.stance, NewEntryStance::Selective);
        assert!(report.risk_scale < 1.0);
        assert!(report.completeness_pct < 40.0);
    }

    #[test]
    fn broad_selloff_freezes_new_entries() {
        let mut evidence = broad(180, 820);
        evidence.indices = indices(&[("上证综指", -1.2), ("沪深300", -1.4), ("创业板指", -1.8)]);
        evidence.sector_average_change = Some(-1.6);
        let report = assess_market_climate(&evidence);
        assert_eq!(report.climate, MarketClimate::StandAside);
        assert_eq!(report.stance, NewEntryStance::Freeze);
        assert_eq!(report.risk_scale, 0.0);
        assert!(gate_playbook(&report, PlaybookKind::Pullback, 90.0).is_some());
    }

    #[test]
    fn constructive_tape_allows_attack_but_heat_caps_it() {
        let mut evidence = broad(720, 180);
        evidence.indices = indices(&[("上证综指", 0.8), ("沪深300", 0.6), ("创业板指", 1.1)]);
        evidence.sector_advances = Some(70);
        evidence.sector_declines = Some(14);
        evidence.sector_average_change = Some(1.0);
        let attack = assess_market_climate(&evidence);
        assert_eq!(attack.climate, MarketClimate::Attack);
        assert_eq!(attack.stance, NewEntryStance::Open);

        evidence.open_positions = 6;
        let heated = assess_market_climate(&evidence);
        assert_eq!(heated.climate, MarketClimate::Select);
    }

    #[test]
    fn extreme_breadth_does_not_become_a_chase_day() {
        let mut evidence = broad(940, 20);
        evidence.indices = indices(&[("上证综指", 1.4), ("沪深300", 1.2), ("创业板指", 1.8)]);
        evidence.sector_advances = Some(86);
        evidence.sector_declines = Some(4);
        evidence.sector_average_change = Some(2.2);
        let report = assess_market_climate(&evidence);
        assert_eq!(report.climate, MarketClimate::Select);
        assert!(gate_playbook(&report, PlaybookKind::Breakout, 70.0).is_some());
        assert!(gate_playbook(&report, PlaybookKind::Pullback, 72.0).is_none());
    }

    #[test]
    fn defend_keeps_only_high_quality_mean_reversion() {
        let mut evidence = broad(360, 620);
        evidence.indices = indices(&[("上证综指", -0.6), ("沪深300", -0.4), ("创业板指", -0.2)]);
        let report = assess_market_climate(&evidence);
        assert_eq!(report.climate, MarketClimate::Defend);
        assert!(gate_playbook(&report, PlaybookKind::Breakout, 88.0).is_some());
        assert!(gate_playbook(&report, PlaybookKind::OversoldBounce, 80.0).is_some());
        assert!(gate_playbook(&report, PlaybookKind::Pullback, 64.0).is_some());
        assert!(gate_playbook(&report, PlaybookKind::Pullback, 76.0).is_none());
        assert!(gate_playbook(&report, PlaybookKind::LowPosition, 72.0).is_none());
    }

    #[test]
    fn more_positions_never_makes_climate_more_aggressive() {
        let mut evidence = broad(700, 200);
        evidence.indices = indices(&[("上证综指", 0.7), ("沪深300", 0.5), ("创业板指", 0.4)]);
        evidence.sector_advances = Some(66);
        evidence.sector_declines = Some(18);
        let few = assess_market_climate(&evidence);
        evidence.open_positions = 9;
        let many = assess_market_climate(&evidence);
        assert!(many.climate >= few.climate);
        assert!(many.risk_scale <= few.risk_scale);
    }
}
