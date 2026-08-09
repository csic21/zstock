use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    InsufficientEvidence,
    NotEligible,
    Waiting,
    MatchesStrategy,
}

impl DecisionStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::InsufficientEvidence => "证据不足",
            Self::NotEligible => "不符合",
            Self::Waiting => "等待触发",
            Self::MatchesStrategy => "符合策略",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Eligibility {
    pub passed: bool,
    pub blockers: Vec<String>,
    pub unknown: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FactorContributions {
    pub position: f64,
    pub trend: f64,
    pub momentum: f64,
    pub volume: f64,
    pub risk: f64,
}

impl FactorContributions {
    pub fn calibrated_score(self) -> f64 {
        let position = self.position.clamp(0.0, 25.0);
        let trend = self.trend.clamp(0.0, 20.0);
        let momentum = self.momentum.clamp(0.0, 15.0);
        let volume = self.volume.clamp(0.0, 10.0);
        let risk = self.risk.clamp(0.0, 30.0);
        (position + trend + momentum + volume - risk).clamp(0.0, 100.0)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionInput {
    pub eligibility: Eligibility,
    pub factors: FactorContributions,
    pub completeness_pct: f64,
    pub supports: Vec<String>,
    pub risks: Vec<String>,
    pub observation: Option<String>,
    pub invalidation: Option<String>,
    pub target: Option<String>,
    pub risk_reward: Option<f64>,
    pub data_as_of: String,
    pub source: String,
    pub adjustment: String,
    pub sample_size: usize,
    pub strategy_version: String,
    pub evidence_grade: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionCard {
    pub status: DecisionStatus,
    pub score: Option<f64>,
    pub completeness_pct: f64,
    pub supports: Vec<String>,
    pub risks: Vec<String>,
    pub observation: Option<String>,
    pub invalidation: Option<String>,
    pub target: Option<String>,
    pub risk_reward: Option<f64>,
    pub data_as_of: String,
    pub source: String,
    pub adjustment: String,
    pub sample_size: usize,
    pub strategy_version: String,
    pub evidence_grade: String,
}

impl DecisionCard {
    pub fn build(input: DecisionInput) -> Self {
        let completeness_pct = input.completeness_pct.clamp(0.0, 100.0);
        let score = input.factors.calibrated_score();
        let status = if completeness_pct < 60.0 || !input.eligibility.unknown.is_empty() {
            DecisionStatus::InsufficientEvidence
        } else if !input.eligibility.passed || !input.eligibility.blockers.is_empty() {
            DecisionStatus::NotEligible
        } else if input.invalidation.is_none() {
            DecisionStatus::Waiting
        } else if score >= 62.0 {
            DecisionStatus::MatchesStrategy
        } else {
            DecisionStatus::Waiting
        };
        let mut risks = input.risks;
        risks.extend(input.eligibility.blockers);
        risks.extend(input.eligibility.unknown);
        risks.truncate(2);
        let mut supports = input.supports;
        supports.truncate(3);
        Self {
            status,
            score: (status != DecisionStatus::InsufficientEvidence).then_some(score),
            completeness_pct,
            supports,
            risks,
            observation: input.observation,
            invalidation: input.invalidation,
            target: input.target,
            risk_reward: input.risk_reward,
            data_as_of: input.data_as_of,
            source: input.source,
            adjustment: input.adjustment,
            sample_size: input.sample_size,
            strategy_version: input.strategy_version,
            evidence_grade: input.evidence_grade,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> DecisionInput {
        DecisionInput {
            eligibility: Eligibility {
                passed: true,
                blockers: Vec::new(),
                unknown: Vec::new(),
            },
            factors: FactorContributions {
                position: 25.0,
                trend: 20.0,
                momentum: 15.0,
                volume: 10.0,
                risk: 0.0,
            },
            completeness_pct: 100.0,
            supports: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            risks: vec![],
            observation: Some("observe".into()),
            invalidation: Some("invalid".into()),
            target: None,
            risk_reward: None,
            data_as_of: "now".into(),
            source: "fixture".into(),
            adjustment: "forward".into(),
            sample_size: 100,
            strategy_version: "v1".into(),
            evidence_grade: "sample".into(),
        }
    }

    #[test]
    fn blockers_cannot_be_outscored() {
        let mut value = input();
        value.eligibility.blockers.push("ST".into());
        assert_eq!(
            DecisionCard::build(value).status,
            DecisionStatus::NotEligible
        );
    }

    #[test]
    fn missing_invalidation_never_matches() {
        let mut value = input();
        value.invalidation = None;
        assert_eq!(DecisionCard::build(value).status, DecisionStatus::Waiting);
    }

    #[test]
    fn more_risk_never_improves_status() {
        let low_risk = DecisionCard::build(input());
        let mut high = input();
        high.factors.risk = 30.0;
        let high_risk = DecisionCard::build(high);
        assert!(high_risk.status <= low_risk.status);
    }

    #[test]
    fn missing_quality_evidence_is_unknown_and_cannot_match() {
        let mut value = input();
        value.eligibility.unknown.push("基本面质量数据未知".into());
        let card = DecisionCard::build(value);
        assert_eq!(card.status, DecisionStatus::InsufficientEvidence);
        assert!(card.risks.iter().any(|risk| risk.contains("未知")));
    }

    #[test]
    fn lowering_completeness_never_improves_status() {
        let complete = DecisionCard::build(input());
        let mut missing = input();
        missing.completeness_pct = 30.0;
        assert!(DecisionCard::build(missing).status <= complete.status);
    }

    #[test]
    fn calibrated_fixture_avoids_endpoint_saturation_and_keeps_top_twenty_distinct() {
        let mut scores = (0..1_000)
            .map(|index| {
                let phase = index as f64 / 999.0;
                FactorContributions {
                    position: 5.0 + phase * 19.0,
                    trend: 4.0 + (phase * 17.0).min(15.5),
                    momentum: 3.0 + ((index * 37 % 997) as f64 / 997.0) * 11.0,
                    volume: 2.0 + ((index * 53 % 991) as f64 / 991.0) * 7.0,
                    risk: 2.0 + ((index * 29 % 983) as f64 / 983.0) * 20.0,
                }
                .calibrated_score()
            })
            .collect::<Vec<_>>();
        let saturated = scores
            .iter()
            .filter(|score| **score <= 0.01 || **score >= 99.99)
            .count();
        assert!(saturated * 100 < scores.len() * 2);
        scores.sort_by(|left, right| right.total_cmp(left));
        let mut top = scores[..20].to_vec();
        top.dedup_by(|left, right| (*left - *right).abs() < 0.001);
        assert!(top.len() >= 18, "distinct top scores={}", top.len());
    }

    #[test]
    fn risk_monotonicity_holds_across_the_full_factor_range() {
        let mut previous = f64::INFINITY;
        for risk in 0..=30 {
            let score = FactorContributions {
                position: 22.0,
                trend: 18.0,
                momentum: 13.0,
                volume: 9.0,
                risk: f64::from(risk),
            }
            .calibrated_score();
            assert!(score <= previous);
            previous = score;
        }
    }
}
