use crate::data::backtest::{BacktestReport, EvidenceVerdict};
use crate::data::levels::ReferenceLevels;
use crate::data::signals::SignalSnapshot;
use crate::domain::decision::{DecisionCard, DecisionStatus, MINIMUM_PLAN_RISK_REWARD};

pub(super) struct EvidenceDisplay {
    pub summary: String,
    pub evidence: String,
    pub execution: String,
}

impl EvidenceDisplay {
    pub fn from_report(report: &BacktestReport, work: bool) -> Self {
        Self {
            summary: report.summary_line(work),
            evidence: report.evidence_line(work),
            execution: report.notes.first().cloned().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GateState {
    Passed,
    Waiting,
    Blocked,
}

impl GateState {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::Passed, true) => "pass",
            (Self::Passed, false) => "通过",
            (Self::Waiting, true) => "wait",
            (Self::Waiting, false) => "等待",
            (Self::Blocked, true) => "block",
            (Self::Blocked, false) => "否决",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StrategyGate {
    pub title: &'static str,
    pub value: String,
    pub state: GateState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlaybookOutcome {
    NeedEvidence,
    NoAction,
    Wait,
    PlanReady,
}

impl PlaybookOutcome {
    pub fn label(self, work: bool) -> &'static str {
        match (self, work) {
            (Self::NeedEvidence, true) => "need evidence",
            (Self::NeedEvidence, false) => "证据不足",
            (Self::NoAction, true) => "no action",
            (Self::NoAction, false) => "不操作",
            (Self::Wait, true) => "wait for trigger",
            (Self::Wait, false) => "等待触发",
            (Self::PlanReady, true) => "plan ready",
            (Self::PlanReady, false) => "可制定计划",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StrategyPlaybook {
    pub outcome: PlaybookOutcome,
    pub summary: String,
    pub gates: Vec<StrategyGate>,
}

impl StrategyPlaybook {
    pub fn build(
        card: &DecisionCard,
        signal: Option<&SignalSnapshot>,
        levels: Option<&ReferenceLevels>,
        last_price: f64,
        report: Option<&BacktestReport>,
        work: bool,
    ) -> Self {
        let data_gate = if card.completeness_pct < 60.0
            || card.status == DecisionStatus::InsufficientEvidence
        {
            StrategyGate {
                title: if work { "data" } else { "数据可信" },
                value: if work {
                    format!("completeness {:.0}%", card.completeness_pct)
                } else {
                    format!("完整度 {:.0}% · 基本面/行情证据未齐", card.completeness_pct)
                },
                state: GateState::Blocked,
            }
        } else {
            StrategyGate {
                title: if work { "data" } else { "数据可信" },
                value: if work {
                    format!("completeness {:.0}%", card.completeness_pct)
                } else {
                    format!("完整度 {:.0}% · 质量门槛已计算", card.completeness_pct)
                },
                state: GateState::Passed,
            }
        };

        let setup_gate = match signal {
            None => StrategyGate {
                title: if work { "setup" } else { "趋势位置" },
                value: if work {
                    "no signal".into()
                } else {
                    "技术样本不足".into()
                },
                state: GateState::Blocked,
            },
            Some(signal) if signal.price_vs_ma20_pct >= 8.0 => StrategyGate {
                title: if work { "setup" } else { "趋势位置" },
                value: if work {
                    format!("MA20 drift +{:.1}%", signal.price_vs_ma20_pct)
                } else {
                    format!("偏离 MA20 +{:.1}% · 不追高", signal.price_vs_ma20_pct)
                },
                state: GateState::Waiting,
            },
            Some(signal) if signal.score >= 62.0 => StrategyGate {
                title: if work { "setup" } else { "趋势位置" },
                value: if work {
                    format!("score {:.0}", signal.score)
                } else {
                    format!("匹配度 {:.0} · 未过热", signal.score)
                },
                state: GateState::Passed,
            },
            Some(signal) => StrategyGate {
                title: if work { "setup" } else { "趋势位置" },
                value: if work {
                    format!("score {:.0} < 62", signal.score)
                } else {
                    format!("匹配度 {:.0} · 尚未达到 62", signal.score)
                },
                state: GateState::Waiting,
            },
        };

        let trigger_gate = match levels {
            None => StrategyGate {
                title: if work { "trigger" } else { "入场触发" },
                value: if work {
                    "no levels".into()
                } else {
                    "尚未形成观察区间".into()
                },
                state: GateState::Blocked,
            },
            Some(levels) => {
                let price = if last_price.is_finite() && last_price > 0.0 {
                    last_price
                } else {
                    levels.close
                };
                if price < levels.buy_low {
                    StrategyGate {
                        title: if work { "trigger" } else { "入场触发" },
                        value: if work {
                            format!("{price:.2} < invalidation {:.2}", levels.buy_low)
                        } else {
                            format!("现价 {price:.2} 已低于失效位 {:.2}", levels.buy_low)
                        },
                        state: GateState::Blocked,
                    }
                } else if price <= levels.buy_high {
                    StrategyGate {
                        title: if work { "trigger" } else { "入场触发" },
                        value: if work {
                            format!("{price:.2} in band")
                        } else {
                            format!("现价 {price:.2} 已进入观察区")
                        },
                        state: GateState::Passed,
                    }
                } else {
                    StrategyGate {
                        title: if work { "trigger" } else { "入场触发" },
                        value: if work {
                            format!("wait <= {:.2}", levels.buy_high)
                        } else {
                            format!("等待回到 {:.2} 以下，不追价", levels.buy_high)
                        },
                        state: GateState::Waiting,
                    }
                }
            }
        };

        let payoff_gate = match card.risk_reward {
            Some(ratio) if ratio >= MINIMUM_PLAN_RISK_REWARD => StrategyGate {
                title: if work { "payoff" } else { "风险收益" },
                value: format!("{ratio:.2} ≥ {MINIMUM_PLAN_RISK_REWARD:.1}"),
                state: GateState::Passed,
            },
            Some(ratio) => StrategyGate {
                title: if work { "payoff" } else { "风险收益" },
                value: format!("{ratio:.2} < {MINIMUM_PLAN_RISK_REWARD:.1}"),
                state: GateState::Blocked,
            },
            None => StrategyGate {
                title: if work { "payoff" } else { "风险收益" },
                value: if work {
                    "undefined".into()
                } else {
                    "赔率未定义".into()
                },
                state: GateState::Blocked,
            },
        };

        let evidence_gate = match report.map(BacktestReport::verdict) {
            Some(EvidenceVerdict::Candidate) => StrategyGate {
                title: if work { "evidence" } else { "样本外证据" },
                value: if work {
                    "candidate".into()
                } else {
                    "通过保守验证门槛".into()
                },
                state: GateState::Passed,
            },
            Some(EvidenceVerdict::Observe) => StrategyGate {
                title: if work { "evidence" } else { "样本外证据" },
                value: if work {
                    "observe".into()
                } else {
                    "有正向迹象，稳定性仍不足".into()
                },
                state: GateState::Waiting,
            },
            Some(EvidenceVerdict::Reject) => StrategyGate {
                title: if work { "evidence" } else { "样本外证据" },
                value: if work {
                    "unsupported".into()
                } else {
                    "扣费后或样本外表现不支持".into()
                },
                state: GateState::Blocked,
            },
            Some(EvidenceVerdict::Insufficient) | None => StrategyGate {
                title: if work { "evidence" } else { "样本外证据" },
                value: if work {
                    "insufficient".into()
                } else {
                    "交易数或样本外数量不足".into()
                },
                state: GateState::Waiting,
            },
        };

        let gates = vec![
            data_gate,
            setup_gate,
            trigger_gate,
            payoff_gate,
            evidence_gate,
        ];
        let outcome = resolve_outcome(card.status, &gates);
        let summary = match (outcome, work) {
            (PlaybookOutcome::NeedEvidence, true) => {
                "Complete market and quality evidence before acting."
            }
            (PlaybookOutcome::NeedEvidence, false) => {
                "行情或基本面证据未齐，停止把分数解读为买入信号。"
            }
            (PlaybookOutcome::NoAction, true) => "At least one hard gate failed. No new position.",
            (PlaybookOutcome::NoAction, false) => {
                "至少一道硬门槛未通过；不新增仓位，先处理失效原因。"
            }
            (PlaybookOutcome::Wait, true) => {
                "Setup is incomplete. Wait for price and evidence confirmation."
            }
            (PlaybookOutcome::Wait, false) => {
                "条件尚未共振；等待价格触发与样本外证据，不提前下注。"
            }
            (PlaybookOutcome::PlanReady, true) => {
                "All gates passed. Size by loss budget and execute next session."
            }
            (PlaybookOutcome::PlanReady, false) => {
                "五道门槛均通过；按损失预算定仓，仍需次日开盘验证。"
            }
        }
        .into();
        Self {
            outcome,
            summary,
            gates,
        }
    }
}

fn resolve_outcome(status: DecisionStatus, gates: &[StrategyGate]) -> PlaybookOutcome {
    if status == DecisionStatus::InsufficientEvidence {
        PlaybookOutcome::NeedEvidence
    } else if status == DecisionStatus::NotEligible
        || gates.iter().any(|gate| gate.state == GateState::Blocked)
    {
        PlaybookOutcome::NoAction
    } else if gates.iter().any(|gate| gate.state == GateState::Waiting) {
        PlaybookOutcome::Wait
    } else {
        PlaybookOutcome::PlanReady
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_hard_gate_prevents_a_ready_plan() {
        let gates = vec![
            StrategyGate {
                title: "a",
                value: "ok".into(),
                state: GateState::Passed,
            },
            StrategyGate {
                title: "b",
                value: "bad".into(),
                state: GateState::Blocked,
            },
        ];
        assert_eq!(
            resolve_outcome(DecisionStatus::MatchesStrategy, &gates),
            PlaybookOutcome::NoAction
        );
    }

    #[test]
    fn all_gates_must_pass_before_plan_ready() {
        let waiting = vec![StrategyGate {
            title: "a",
            value: "wait".into(),
            state: GateState::Waiting,
        }];
        assert_eq!(
            resolve_outcome(DecisionStatus::MatchesStrategy, &waiting),
            PlaybookOutcome::Wait
        );
        let passed = vec![StrategyGate {
            title: "a",
            value: "ok".into(),
            state: GateState::Passed,
        }];
        assert_eq!(
            resolve_outcome(DecisionStatus::MatchesStrategy, &passed),
            PlaybookOutcome::PlanReady
        );
    }
}
