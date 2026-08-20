//! Cross-experiment strategy tournament.
//!
//! Ranking is a research score, not a live-trading recommendation. Promotion
//! gates stay authoritative: a high arena score cannot promote a rejected
//! strategy to paper, and win rate alone cannot crown a champion.
//! Bounded self-evolution only proposes parameter-neighborhood offspring of
//! the current champion; local backtests decide which children are kept.

use std::collections::BTreeSet;

use super::backtest::validation::{PromotionConclusion, parameter_neighbors};
use super::paper::{PaperBehaviorComparison, PaperRunResult};
use super::strategy::{CompiledStrategy, StrategySpec, strategy_id};
use super::strategy_library::{
    LibraryStatus, StrategyLibraryRecord, research_score, trading_fitness,
};

pub const ARENA_SCORE_VERSION: &str = "strategy-arena-v1";
pub const EVOLUTION_VERSION: &str = "strategy-evolution-v1";
pub const EVOLUTION_GENERATOR: &str = "arena-evolution-v1";
pub const MAX_LINEAGE_DEPTH: usize = 3;
pub const MAX_OFFSPRING_PER_CYCLE: usize = 4;
pub const MIN_OFFSPRING_TRADES: usize = 8;
pub const FITNESS_MARGIN: f64 = 1.5;
pub const DRAWDOWN_SLACK_PCT: f64 = 3.0;

const MIN_PRUNE_TRADES: usize = 25;
const DOMINANCE_EXCESS_MARGIN: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArenaRole {
    Champion,
    Challenger,
    Contender,
    PruneCandidate,
}

impl ArenaRole {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Champion => "冠军",
            Self::Challenger => "挑战者",
            Self::Contender => "在册",
            Self::PruneCandidate => "建议淘汰",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneReason {
    NoTrades,
    RejectedWeak,
    NegativeEdge,
    Dominated { by_name: String },
}

impl PruneReason {
    pub fn label(&self) -> String {
        match self {
            Self::NoTrades => "没有完成交易，研究价值不足".into(),
            Self::RejectedWeak => "已淘汰且没有成本后优势".into(),
            Self::NegativeEdge => "样本足够但超额为负、回撤偏大".into(),
            Self::Dominated { by_name } => format!("被「{by_name}」全面占优"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiveObservation {
    pub strategy_id: String,
    pub missed_signal_pct: f64,
    pub minimum_observation_met: bool,
    pub warning_count: usize,
    pub paper_win_rate_pct: Option<f64>,
    pub paper_trade_count: usize,
    pub paper_expectancy_pct: Option<f64>,
}

impl LiveObservation {
    pub fn from_paper(run: &PaperRunResult, comparison: Option<&PaperBehaviorComparison>) -> Self {
        let wins = run
            .trades
            .iter()
            .filter(|trade| trade.gross_return_pct > 0.0)
            .count();
        let paper_win_rate_pct =
            (!run.trades.is_empty()).then_some(wins as f64 / run.trades.len() as f64 * 100.0);
        let paper_expectancy_pct = (!run.trades.is_empty()).then_some(
            run.trades
                .iter()
                .map(|trade| trade.gross_return_pct)
                .sum::<f64>()
                / run.trades.len() as f64,
        );
        Self {
            strategy_id: run.strategy_id.clone(),
            missed_signal_pct: comparison.map(|item| item.missed_signal_pct).unwrap_or(0.0),
            minimum_observation_met: comparison
                .map(|item| item.minimum_observation_met)
                .unwrap_or(false),
            warning_count: comparison.map(|item| item.warnings.len()).unwrap_or(0),
            paper_win_rate_pct,
            paper_trade_count: run.trades.len(),
            paper_expectancy_pct,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArenaStanding {
    pub rank: usize,
    pub score: f64,
    pub role: ArenaRole,
    pub record: StrategyLibraryRecord,
    pub reasons: Vec<String>,
    pub prune_reason: Option<PruneReason>,
    pub live_adjustment: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArenaSnapshot {
    pub version: &'static str,
    pub as_of: String,
    pub champion: Option<ArenaStanding>,
    pub standings: Vec<ArenaStanding>,
    pub headline: String,
}

pub fn live_adjustment(
    record: &StrategyLibraryRecord,
    live: Option<&LiveObservation>,
) -> (f64, Vec<String>) {
    let Some(live) = live else {
        return (0.0, Vec::new());
    };
    let mut adjustment = 0.0;
    let mut reasons = Vec::new();
    if live.minimum_observation_met {
        adjustment += 4.0;
        reasons.push("每日观察已满最短窗口".into());
    }
    if live.missed_signal_pct > 0.0 {
        let penalty = (live.missed_signal_pct / 100.0) * 6.0;
        adjustment -= penalty;
        if live.missed_signal_pct >= 20.0 {
            reasons.push(format!(
                "模拟无法成交 {missed:.0}%",
                missed = live.missed_signal_pct
            ));
        }
    }
    adjustment -= (live.warning_count.min(3) as f64) * 1.0;
    if let Some(paper_win) = live
        .paper_win_rate_pct
        .filter(|_| live.paper_trade_count >= 8)
    {
        let backtest_win = record.oos_win_rate_pct.unwrap_or(record.win_rate_pct);
        let drift = ((paper_win - backtest_win) / 20.0).clamp(-8.0, 6.0);
        adjustment += drift;
        if drift <= -2.0 {
            reasons.push(format!(
                "每日胜率 {paper_win:.0}% 低于回测 {backtest_win:.0}%"
            ));
        } else if drift >= 2.0 {
            reasons.push(format!("每日胜率 {paper_win:.0}% 好于回测"));
        }
    }
    if live.paper_trade_count >= 8 {
        match live.paper_expectancy_pct {
            Some(expectancy) if expectancy > 0.0 => {
                adjustment += 2.0;
                reasons.push(format!("每日期望 {expectancy:+.2}%"));
            }
            Some(expectancy) if expectancy < 0.0 => {
                adjustment -= 3.0;
                reasons.push(format!("每日期望 {expectancy:+.2}%"));
            }
            _ => {}
        }
    }
    (adjustment, reasons)
}

struct ScoredRow {
    record: StrategyLibraryRecord,
    score: f64,
    live_adjustment: f64,
    reasons: Vec<String>,
    prune: Option<PruneReason>,
}

pub fn evaluate_arena(
    records: &[StrategyLibraryRecord],
    live: &[LiveObservation],
    as_of: &str,
) -> ArenaSnapshot {
    let retained: Vec<_> = records
        .iter()
        .filter(|record| record.status == LibraryStatus::Retained)
        .cloned()
        .collect();
    let mut scored: Vec<ScoredRow> = retained
        .iter()
        .map(|record| {
            let live = live
                .iter()
                .find(|item| item.strategy_id == record.strategy_id);
            let (adjustment, mut live_reasons) = live_adjustment(record, live);
            let score = research_score(record) + adjustment;
            let mut reasons = research_reasons(record);
            reasons.append(&mut live_reasons);
            let prune = prune_reason(record, &retained);
            if let Some(reason) = &prune {
                reasons.push(reason.label());
            }
            ScoredRow {
                record: record.clone(),
                score,
                live_adjustment: adjustment,
                reasons,
                prune,
            }
        })
        .collect();
    scored.sort_by(|left, right| {
        left.prune
            .is_some()
            .cmp(&right.prune.is_some())
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| right.record.trade_count.cmp(&left.record.trade_count))
            .then_with(|| left.record.strategy_id.cmp(&right.record.strategy_id))
    });

    let eligible = scored.iter().filter(|item| item.prune.is_none()).count();
    let mut standings = Vec::with_capacity(scored.len());
    let mut eligible_index = 0usize;
    for (index, row) in scored.into_iter().enumerate() {
        let role = if row.prune.is_some() {
            ArenaRole::PruneCandidate
        } else {
            eligible_index += 1;
            match eligible_index {
                1 => ArenaRole::Champion,
                2 | 3 => ArenaRole::Challenger,
                _ => ArenaRole::Contender,
            }
        };
        standings.push(ArenaStanding {
            rank: index + 1,
            score: row.score,
            role,
            record: row.record,
            reasons: row.reasons,
            prune_reason: row.prune,
            live_adjustment: row.live_adjustment,
        });
    }

    let champion = standings
        .iter()
        .find(|row| row.role == ArenaRole::Champion)
        .cloned();
    let prune_count = standings
        .iter()
        .filter(|row| row.prune_reason.is_some())
        .count();
    let headline = headline(
        as_of,
        retained.len(),
        eligible,
        prune_count,
        champion.as_ref(),
    );
    ArenaSnapshot {
        version: ARENA_SCORE_VERSION,
        as_of: as_of.into(),
        champion,
        standings,
        headline,
    }
}

fn research_reasons(record: &StrategyLibraryRecord) -> Vec<String> {
    let mut reasons = Vec::new();
    match record.conclusion {
        Some(PromotionConclusion::PaperCandidate) => reasons.push("已过模拟盘硬门槛".into()),
        Some(PromotionConclusion::ContinueResearch) => {
            reasons.push("继续研究，未过全部门槛".into())
        }
        Some(PromotionConclusion::Rejected) => reasons.push("稳健性门槛未通过".into()),
        None => reasons.push("尚未生成稳健性结论".into()),
    }
    let win = record.oos_win_rate_pct.unwrap_or(record.win_rate_pct);
    if record.oos_win_rate_pct.is_some() {
        reasons.push(format!("样本外胜率 {win:.0}%"));
    } else {
        reasons.push(format!(
            "样本内胜率 {win:.0}% · {trades} 笔",
            trades = record.trade_count
        ));
    }
    reasons.push(format!(
        "成本后超额 {excess:+.1}% · 回撤 {dd:.1}%",
        excess = record.excess_return_pct,
        dd = record.max_drawdown_pct
    ));
    reasons
}

fn prune_reason(
    record: &StrategyLibraryRecord,
    pool: &[StrategyLibraryRecord],
) -> Option<PruneReason> {
    if record.conclusion == Some(PromotionConclusion::PaperCandidate) {
        return None;
    }
    if record.trade_count == 0 {
        return Some(PruneReason::NoTrades);
    }
    if let Some(other) = pool.iter().find(|other| dominates(other, record)) {
        return Some(PruneReason::Dominated {
            by_name: other.strategy_name.clone(),
        });
    }
    if record.trade_count < MIN_PRUNE_TRADES {
        return None;
    }
    let win = record.oos_win_rate_pct.unwrap_or(record.win_rate_pct);
    if record.conclusion == Some(PromotionConclusion::Rejected) && record.excess_return_pct <= 0.0 {
        return Some(PruneReason::RejectedWeak);
    }
    if record.excess_return_pct < 0.0 && win < 45.0 && record.max_drawdown_pct >= 18.0 {
        return Some(PruneReason::NegativeEdge);
    }
    None
}

fn dominates(other: &StrategyLibraryRecord, this: &StrategyLibraryRecord) -> bool {
    if other.id == this.id || other.status != LibraryStatus::Retained {
        return false;
    }
    if this.conclusion == Some(PromotionConclusion::PaperCandidate) {
        return false;
    }
    if other.conclusion == Some(PromotionConclusion::Rejected)
        && this.conclusion != Some(PromotionConclusion::Rejected)
    {
        return false;
    }
    let other_win = other.oos_win_rate_pct.unwrap_or(other.win_rate_pct);
    let this_win = this.oos_win_rate_pct.unwrap_or(this.win_rate_pct);
    other.excess_return_pct >= this.excess_return_pct + DOMINANCE_EXCESS_MARGIN
        && other.max_drawdown_pct <= this.max_drawdown_pct + 1e-9
        && other_win + 1e-9 >= this_win
        && other.trade_count >= this.trade_count
        && other.profit_factor + 1e-9 >= this.profit_factor
}

fn headline(
    as_of: &str,
    retained: usize,
    eligible: usize,
    prune_count: usize,
    champion: Option<&ArenaStanding>,
) -> String {
    if retained == 0 {
        return "策略库还是空的。完成一次批量回测后，会按稳健分角逐最强策略。".into();
    }
    let prune_note = if prune_count == 0 {
        String::new()
    } else {
        format!("；{prune_count} 个建议淘汰")
    };
    match champion {
        Some(champion) => format!(
            "{as_of} 当前冠军：{} · 稳健分 {:.1} · {}{prune_note}。每日观察会校正排名，并从冠军派生有界邻域变体。",
            champion.record.strategy_name,
            champion.score,
            champion
                .record
                .conclusion
                .map(conclusion_short)
                .unwrap_or("尚未评级")
        ),
        None if eligible == 0 => {
            format!("{as_of} 在册 {retained} 个策略，但还没有稳定领先者{prune_note}。")
        }
        None => format!("{as_of} 在册 {retained} 个策略，角逐尚未产生冠军{prune_note}。"),
    }
}

const fn conclusion_short(conclusion: PromotionConclusion) -> &'static str {
    match conclusion {
        PromotionConclusion::Rejected => "未过门槛",
        PromotionConclusion::ContinueResearch => "继续研究",
        PromotionConclusion::PaperCandidate => "模拟盘候选",
    }
}

pub fn evolution_root_name(name: &str) -> String {
    name.split(" · 进化")
        .next()
        .unwrap_or(name)
        .trim()
        .to_string()
}

pub fn propose_offspring(
    parent: &StrategySpec,
    generation: usize,
    existing_strategy_ids: &BTreeSet<String>,
) -> Vec<StrategySpec> {
    if generation >= MAX_LINEAGE_DEPTH {
        return Vec::new();
    }
    let root_name = evolution_root_name(&parent.name);
    let mut output = Vec::new();
    for mut spec in parameter_neighbors(parent) {
        spec.metadata.generator = EVOLUTION_GENERATOR.into();
        spec.metadata.prompt_version = EVOLUTION_VERSION.into();
        spec.name = format!("{} · 进化{}", root_name, generation + 1);
        spec.hypothesis = format!(
            "由「{}」在白名单参数邻域内进化；只有本地回测证明更稳健才会保留",
            parent.name
        );
        if CompiledStrategy::compile(spec.clone()).is_err() {
            continue;
        }
        let id = strategy_id(&spec);
        if existing_strategy_ids.contains(&id) || id == strategy_id(parent) {
            continue;
        }
        output.push(spec);
        if output.len() >= MAX_OFFSPRING_PER_CYCLE {
            break;
        }
    }
    output
}

pub fn should_keep_offspring(
    parent: &StrategyLibraryRecord,
    child: &StrategyLibraryRecord,
) -> bool {
    child.status == LibraryStatus::Retained
        && child.trade_count >= MIN_OFFSPRING_TRADES
        && child.max_drawdown_pct <= parent.max_drawdown_pct + DRAWDOWN_SLACK_PCT
        && trading_fitness(child) >= trading_fitness(parent) + FITNESS_MARGIN
}

pub fn evolution_headline(
    parent_name: &str,
    generation: usize,
    considered: usize,
    kept: usize,
    skipped: usize,
) -> String {
    if considered == 0 && skipped == 0 {
        format!("「{parent_name}」已到第 {generation} 代，达到进化上限，改为每日观察校正。")
    } else if kept == 0 {
        format!("从「{parent_name}」派生 {considered} 个邻域变体，没有更强的子代被保留。")
    } else {
        format!(
            "自我进化：从「{parent_name}」第 {} 代保留 {kept} 个更强变体（评估 {considered} 个）。",
            generation + 1
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::strategy_library::LibraryStatus;

    fn record(
        strategy_id: &str,
        name: &str,
        win_rate: f64,
        trades: usize,
        excess: f64,
        drawdown: f64,
        conclusion: Option<PromotionConclusion>,
    ) -> StrategyLibraryRecord {
        StrategyLibraryRecord {
            id: format!("library:exp:{strategy_id}"),
            experiment_id: "exp".into(),
            strategy_id: strategy_id.into(),
            dataset_id: "dataset".into(),
            strategy_name: name.into(),
            retained_at: "2026-08-20T00:00:00Z".into(),
            status: LibraryStatus::Retained,
            conclusion,
            evidence: "fixture".into(),
            win_rate_pct: win_rate,
            oos_win_rate_pct: Some(win_rate),
            total_return_pct: excess,
            excess_return_pct: excess,
            max_drawdown_pct: drawdown,
            trade_count: trades,
            payoff_ratio: 1.3,
            profit_factor: 1.2,
        }
    }

    #[test]
    fn paper_candidate_with_balanced_metrics_beats_high_win_rate_reject() {
        let rejected = record(
            "hot",
            "高胜率已淘汰",
            92.0,
            80,
            -3.0,
            24.0,
            Some(PromotionConclusion::Rejected),
        );
        let paper = record(
            "steady",
            "稳健回踩",
            56.0,
            70,
            9.0,
            8.0,
            Some(PromotionConclusion::PaperCandidate),
        );
        let arena = evaluate_arena(&[rejected, paper], &[], "2026-08-20");
        assert_eq!(
            arena
                .champion
                .as_ref()
                .map(|row| row.record.strategy_id.as_str()),
            Some("steady")
        );
        assert_eq!(arena.standings[0].role, ArenaRole::Champion);
        assert!(arena.standings.iter().any(|row| {
            row.record.strategy_id == "hot" && row.role == ArenaRole::PruneCandidate
        }));
    }

    #[test]
    fn dominated_and_zero_trade_strategies_are_prune_candidates() {
        let leader = record(
            "leader",
            "放量突破",
            60.0,
            40,
            12.0,
            7.0,
            Some(PromotionConclusion::ContinueResearch),
        );
        let mut shadow = leader.clone();
        shadow.id = "library:exp:shadow".into();
        shadow.strategy_id = "shadow".into();
        shadow.strategy_name = "弱化突破".into();
        shadow.excess_return_pct = 4.0;
        shadow.win_rate_pct = 50.0;
        shadow.oos_win_rate_pct = Some(50.0);
        shadow.profit_factor = 1.0;
        let empty = record(
            "empty",
            "空转策略",
            0.0,
            0,
            0.0,
            0.0,
            Some(PromotionConclusion::ContinueResearch),
        );
        let arena = evaluate_arena(&[leader, shadow, empty], &[], "2026-08-20");
        let prune: Vec<_> = arena
            .standings
            .iter()
            .filter(|row| row.prune_reason.is_some())
            .map(|row| row.record.strategy_id.as_str())
            .collect();
        assert!(prune.contains(&"shadow"));
        assert!(prune.contains(&"empty"));
        assert_eq!(
            arena
                .champion
                .as_ref()
                .map(|row| row.record.strategy_id.as_str()),
            Some("leader")
        );
    }

    #[test]
    fn daily_observation_can_dethrone_a_backtest_leader() {
        let fading = record(
            "fade",
            "回测领先",
            62.0,
            50,
            10.0,
            9.0,
            Some(PromotionConclusion::PaperCandidate),
        );
        let rising = record(
            "rise",
            "每日转强",
            54.0,
            48,
            7.0,
            8.0,
            Some(PromotionConclusion::PaperCandidate),
        );
        let before = evaluate_arena(&[fading.clone(), rising.clone()], &[], "2026-08-20");
        assert_eq!(
            before
                .champion
                .as_ref()
                .map(|row| row.record.strategy_id.as_str()),
            Some("fade")
        );

        let live = vec![
            LiveObservation {
                strategy_id: "fade".into(),
                missed_signal_pct: 40.0,
                minimum_observation_met: true,
                warning_count: 3,
                paper_win_rate_pct: Some(28.0),
                paper_trade_count: 12,
                paper_expectancy_pct: Some(-1.4),
            },
            LiveObservation {
                strategy_id: "rise".into(),
                missed_signal_pct: 5.0,
                minimum_observation_met: true,
                warning_count: 0,
                paper_win_rate_pct: Some(68.0),
                paper_trade_count: 12,
                paper_expectancy_pct: Some(1.8),
            },
        ];
        let after = evaluate_arena(&[fading, rising], &live, "2026-08-21");
        assert_eq!(
            after
                .champion
                .as_ref()
                .map(|row| row.record.strategy_id.as_str()),
            Some("rise")
        );
        assert!(
            after
                .champion
                .as_ref()
                .is_some_and(|row| row.live_adjustment > 0.0)
        );
    }

    #[test]
    fn paper_candidates_are_never_auto_pruned() {
        let paper = record(
            "paper",
            "模拟候选",
            40.0,
            80,
            -4.0,
            22.0,
            Some(PromotionConclusion::PaperCandidate),
        );
        let other = record(
            "other",
            "对照",
            58.0,
            80,
            6.0,
            10.0,
            Some(PromotionConclusion::ContinueResearch),
        );
        let arena = evaluate_arena(&[paper, other], &[], "2026-08-20");
        assert!(
            arena
                .standings
                .iter()
                .find(|row| row.record.strategy_id == "paper")
                .is_some_and(|row| row.prune_reason.is_none())
        );
    }

    #[test]
    fn dismissed_records_never_enter_the_arena() {
        let mut gone = record(
            "gone",
            "已删除",
            99.0,
            90,
            20.0,
            3.0,
            Some(PromotionConclusion::PaperCandidate),
        );
        gone.status = LibraryStatus::Dismissed;
        let keep = record(
            "keep",
            "留用",
            50.0,
            20,
            2.0,
            12.0,
            Some(PromotionConclusion::ContinueResearch),
        );
        let arena = evaluate_arena(&[gone, keep], &[], "2026-08-20");
        assert_eq!(arena.standings.len(), 1);
        assert_eq!(arena.standings[0].record.strategy_id, "keep");
    }

    #[test]
    fn offspring_are_new_versions_with_parent_and_generation_cap() {
        use crate::domain::strategy::LocalTemplate;

        let parent = LocalTemplate::MaTrendPullback.build("fixture");
        let parent_id = crate::domain::strategy::strategy_id(&parent);
        let offspring = propose_offspring(&parent, 0, &BTreeSet::new());
        assert!(!offspring.is_empty());
        assert!(offspring.len() <= MAX_OFFSPRING_PER_CYCLE);
        for child in &offspring {
            assert_eq!(child.metadata.generator, EVOLUTION_GENERATOR);
            assert_eq!(
                child.metadata.parent_strategy_id.as_deref(),
                Some(parent_id.as_str())
            );
            assert!(child.name.contains("进化1"));
            assert_ne!(crate::domain::strategy::strategy_id(child), parent_id);
        }
        assert!(propose_offspring(&parent, MAX_LINEAGE_DEPTH, &BTreeSet::new()).is_empty());

        let existing: BTreeSet<_> = offspring
            .iter()
            .map(crate::domain::strategy::strategy_id)
            .collect();
        let again = propose_offspring(&parent, 0, &existing);
        assert!(
            again
                .iter()
                .all(|spec| !existing.contains(&crate::domain::strategy::strategy_id(spec)))
        );
    }

    #[test]
    fn only_fitter_offspring_are_kept() {
        let parent = record(
            "parent",
            "父代",
            55.0,
            40,
            6.0,
            10.0,
            Some(PromotionConclusion::ContinueResearch),
        );
        let mut better = parent.clone();
        better.id = "library:exp:child".into();
        better.strategy_id = "child".into();
        better.win_rate_pct = 62.0;
        better.oos_win_rate_pct = Some(62.0);
        better.excess_return_pct = 11.0;
        better.max_drawdown_pct = 8.0;
        better.profit_factor = 1.6;
        assert!(should_keep_offspring(&parent, &better));

        let mut weaker = better.clone();
        weaker.win_rate_pct = 50.0;
        weaker.oos_win_rate_pct = Some(50.0);
        weaker.excess_return_pct = 4.0;
        weaker.max_drawdown_pct = 16.0;
        assert!(!should_keep_offspring(&parent, &weaker));

        let mut thin = better;
        thin.trade_count = 3;
        assert!(!should_keep_offspring(&parent, &thin));
    }
}
