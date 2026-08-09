use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Planned,
    Executed,
    DueForReview,
    Reviewed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceSnapshot {
    pub strategy_version: String,
    pub data_as_of: String,
    pub source: String,
    pub payload_json: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionPlan {
    pub id: String,
    pub code: String,
    pub created_on: String,
    pub review_on: String,
    pub trigger: String,
    pub observation_range: String,
    pub invalidation: String,
    pub target: Option<String>,
    pub risk_amount: Option<String>,
    pub status: PlanStatus,
    pub evidence: EvidenceSnapshot,
    pub executed: Option<bool>,
    pub exit_reason: Option<String>,
    pub followed_plan: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanOutcome {
    pub plan_id: String,
    pub horizon_days: u16,
    pub return_pct: f64,
    pub maximum_favorable_excursion_pct: f64,
    pub maximum_adverse_excursion_pct: f64,
}

pub fn add_outcome_idempotently(outcomes: &mut Vec<PlanOutcome>, outcome: PlanOutcome) -> bool {
    if outcomes.iter().any(|existing| {
        existing.plan_id == outcome.plan_id && existing.horizon_days == outcome.horizon_days
    }) {
        return false;
    }
    outcomes.push(outcome);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_creation_is_idempotent() {
        let outcome = PlanOutcome {
            plan_id: "p1".into(),
            horizon_days: 5,
            return_pct: 1.0,
            maximum_favorable_excursion_pct: 2.0,
            maximum_adverse_excursion_pct: -1.0,
        };
        let mut values = Vec::new();
        assert!(add_outcome_idempotently(&mut values, outcome.clone()));
        assert!(!add_outcome_idempotently(&mut values, outcome));
    }
}
