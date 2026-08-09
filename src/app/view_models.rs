use crate::domain::decision::{
    DecisionCard, DecisionInput, Eligibility, FactorContributions, QualityEvidence,
};
use crate::domain::fundamentals::{
    QualityGate, REQUIRED_QUALITY_METRICS, metric_label, quality_gate,
};
use crate::domain::journal::{DecisionPlan, EvidenceSnapshot, PlanStatus};

use super::StockApp;

impl StockApp {
    pub(crate) fn decision_card_view_model(&self) -> DecisionCard {
        let signal = self.current_signal();
        let levels = self.levels_cache.as_ref();
        let symbol = self.current_symbol();
        let name = symbol.map(|value| value.name.as_ref()).unwrap_or_default();
        let data_as_of = self
            .candles
            .last()
            .map(|candle| candle.date.to_string())
            .unwrap_or_else(|| chrono::Local::now().date_naive().to_string());
        let (fundamental_gate, fundamental_source, latest_financial_notice, quality_evidence) =
            match &self.analysis_state.fundamentals.state {
                crate::controller::state::RequestState::Ready(snapshot)
                    if snapshot.code == self.selected.as_ref() =>
                {
                    let notice = snapshot
                        .metrics
                        .iter()
                        .filter(|metric| metric.available_on(&data_as_of))
                        .map(|metric| metric.announced_on.as_str())
                        .max()
                        .map(str::to_string);
                    let evidence = REQUIRED_QUALITY_METRICS
                        .iter()
                        .filter_map(|name| {
                            let metric = snapshot
                                .metrics
                                .iter()
                                .filter(|metric| {
                                    metric.name == *name && metric.available_on(&data_as_of)
                                })
                                .max_by(|left, right| {
                                    (&left.reporting_period, &left.announced_on)
                                        .cmp(&(&right.reporting_period, &right.announced_on))
                                })?;
                            let value = metric.value?;
                            Some(QualityEvidence {
                                label: metric_label(name).into(),
                                value: if *name == "audit_risk_flag" {
                                    if value < 1.0 {
                                        "标准无保留".into()
                                    } else {
                                        "存在风险".into()
                                    }
                                } else {
                                    format!("{value:.2}")
                                },
                                unit: metric.unit.clone(),
                                reporting_period: metric.reporting_period.clone(),
                                announced_on: metric.announced_on.clone(),
                                source: metric.source.clone(),
                            })
                        })
                        .collect();
                    (
                        quality_gate(&snapshot.metrics, &data_as_of),
                        snapshot.source.clone(),
                        notice,
                        evidence,
                    )
                }
                crate::controller::state::RequestState::Failed(message) => (
                    QualityGate {
                        passed: false,
                        blockers: Vec::new(),
                        unknown: vec![format!("基本面数据不可用：{message}")],
                    },
                    "基本面数据不可用".into(),
                    None,
                    Vec::new(),
                ),
                crate::controller::state::RequestState::Loading => (
                    QualityGate {
                        passed: false,
                        blockers: Vec::new(),
                        unknown: vec!["基本面质量数据加载中".into()],
                    },
                    "基本面加载中".into(),
                    None,
                    Vec::new(),
                ),
                _ => (
                    QualityGate {
                        passed: false,
                        blockers: Vec::new(),
                        unknown: vec!["基本面质量数据未知（未默认通过）".into()],
                    },
                    "基本面未知".into(),
                    None,
                    Vec::new(),
                ),
            };
        let mut blockers = Vec::new();
        if name.to_ascii_uppercase().contains("ST") {
            blockers.push("ST 风险门槛".into());
        }
        if self.candles.len() < 30 {
            blockers.push("历史样本不足 30 根".into());
        }
        let completeness = signal.as_ref().map(|value| value.confidence).unwrap_or(0.0);
        let technical_score = signal.as_ref().map(|value| value.score).unwrap_or(0.0);
        let risk = signal
            .as_ref()
            .map(|value| {
                let volatility = value
                    .volatility_20_ann_pct
                    .map(|number| (number - 25.0).max(0.0) / 3.0)
                    .unwrap_or(8.0);
                let drawdown = value
                    .max_drawdown_1y_pct
                    .map(|number| (-number - 20.0).max(0.0) / 3.0)
                    .unwrap_or(8.0);
                (volatility + drawdown).clamp(0.0, 30.0)
            })
            .unwrap_or(30.0);
        let positive = (technical_score + risk).clamp(0.0, 70.0);
        let factors = FactorContributions {
            position: (positive * 0.35).clamp(0.0, 25.0),
            trend: (positive * 0.30).clamp(0.0, 20.0),
            momentum: (positive * 0.20).clamp(0.0, 15.0),
            volume: (positive * 0.15).clamp(0.0, 10.0),
            risk,
        };
        let mut supports: Vec<String> = signal
            .as_ref()
            .map(|value| {
                value
                    .reasons
                    .iter()
                    .map(|reason| (*reason).to_string())
                    .collect()
            })
            .unwrap_or_default();
        if fundamental_gate.passed {
            supports.push(format!(
                "基本面质量门槛通过（公告截至 {}）",
                latest_financial_notice.as_deref().unwrap_or("—")
            ));
        }
        let mut risks = Vec::new();
        if signal
            .as_ref()
            .and_then(|value| value.volatility_20_ann_pct)
            .is_some_and(|value| value >= 60.0)
        {
            risks.push("短期波动较高".into());
        }
        if signal
            .as_ref()
            .and_then(|value| value.max_drawdown_1y_pct)
            .is_some_and(|value| value <= -35.0)
        {
            risks.push("历史回撤较深".into());
        }
        let observation = levels.map(|value| format!("{} 元", value.buy_band_text()));
        let invalidation = levels.map(|value| format!("有效跌破 {:.2} 元", value.buy_low));
        let target = levels.map(|value| format!("{} 元", value.sell_band_text()));
        let risk_reward = levels.and_then(|value| {
            let risk_distance = value.close - value.buy_low;
            let reward_distance = value.sell_low - value.close;
            (risk_distance > 0.0 && reward_distance > 0.0)
                .then_some(reward_distance / risk_distance)
        });
        DecisionCard::build(DecisionInput {
            eligibility: Eligibility {
                passed: blockers.is_empty() && fundamental_gate.passed,
                blockers: blockers
                    .into_iter()
                    .chain(fundamental_gate.blockers)
                    .collect(),
                unknown: fundamental_gate.unknown,
            },
            factors,
            completeness_pct: completeness,
            supports,
            risks,
            quality_evidence,
            observation,
            invalidation,
            target,
            risk_reward,
            data_as_of,
            source: format!("行情 {}；基本面 {fundamental_source}", self.data_source),
            adjustment: "前复权".into(),
            sample_size: self.candles.len(),
            strategy_version: "technical-quality-gate-v2".into(),
            evidence_grade: if self.candles.len() >= 120 {
                "样本内探索".into()
            } else {
                "样本不足".into()
            },
        })
    }

    pub(crate) fn record_decision_plan_from_card(&mut self, cx: &mut gpui::Context<Self>) {
        use crate::data::journal::{self, JournalEntry, JournalKind};

        let card = self
            .analysis_state
            .decision_card
            .clone()
            .unwrap_or_else(|| self.decision_card_view_model());
        let code = self.selected.to_string();
        let name = self
            .current_symbol()
            .map(|symbol| symbol.name.to_string())
            .unwrap_or_else(|| code.clone());
        let entry_id = journal::new_id();
        let created_on = chrono::Local::now().date_naive();
        let review_on = created_on + chrono::Duration::days(28);
        let trigger = card
            .observation
            .clone()
            .map(|range| format!("进入参考观察区间 {range}"))
            .unwrap_or_else(|| "等待证据补充".into());
        let invalidation = card
            .invalidation
            .clone()
            .unwrap_or_else(|| "尚未定义，不能执行".into());
        let plan = DecisionPlan {
            id: entry_id.clone(),
            code: code.clone(),
            created_on: created_on.to_string(),
            review_on: review_on.to_string(),
            trigger: trigger.clone(),
            observation_range: card.observation.clone().unwrap_or_else(|| "—".into()),
            invalidation: invalidation.clone(),
            target: card.target.clone(),
            risk_amount: None,
            status: PlanStatus::Planned,
            evidence: EvidenceSnapshot {
                strategy_version: card.strategy_version.clone(),
                data_as_of: card.data_as_of.clone(),
                source: card.source.clone(),
                payload_json: serde_json::to_string(&card).unwrap_or_else(|_| "{}".into()),
            },
            executed: None,
            exit_reason: None,
            followed_plan: None,
        };
        let note = format!(
            "计划 · {trigger} · 失效：{invalidation} · 复盘：{review_on} · {}",
            card.strategy_version
        );
        let price = self.current_symbol().map(|symbol| symbol.last);
        let target = self.levels_cache.as_ref().map(|levels| levels.sell_low);
        self.journal.push(JournalEntry {
            id: entry_id,
            code,
            name,
            kind: JournalKind::FromPick,
            price,
            target,
            note,
            created_at: journal::now_stamp(),
            plan: Some(plan),
            outcomes: Vec::new(),
        });
        self.persist_journal();
        self.status = crate::model::shared("已创建计划，并保存当时证据快照");
        cx.notify();
    }
}
