use crate::domain::climate::NewEntryStance;
use crate::domain::decision::{
    DecisionCard, DecisionInput, DecisionStep, DecisionStepState, DecisionTrace, Eligibility,
    FactorContributions, QualityEvidence,
};
use crate::domain::fundamentals::{
    QualityGate, REQUIRED_QUALITY_METRICS, metric_label, quality_gate,
};
use crate::domain::journal::{DecisionPlan, EvidenceSnapshot, PlanStatus};
use crate::domain::money::Currency;
use crate::domain::position_sizing::{
    PositionPlan, PositionSizingError, PositionSizingInput, calculate_position_plan,
};

use super::StockApp;

impl StockApp {
    pub(crate) fn decision_trace_view_model(&self, cx: &gpui::Context<Self>) -> DecisionTrace {
        use crate::controller::state::RequestState;

        let card = self.decision_card_view_model();
        let candles_current = self
            .candles_code
            .as_ref()
            .is_some_and(|code| code == self.selected.as_ref());
        let candle_count = if candles_current {
            self.candles.len()
        } else {
            0
        };
        let data_step = if (self.loading || self.refreshing) && candle_count == 0 {
            DecisionStep {
                title: "行情数据".into(),
                state: DecisionStepState::Running,
                summary: "正在读取当前标的日 K 与行情来源".into(),
            }
        } else if candle_count < 30 {
            DecisionStep {
                title: "行情数据".into(),
                state: DecisionStepState::Blocked,
                summary: format!("只有 {candle_count} 根日 K，至少需要 30 根"),
            }
        } else {
            DecisionStep {
                title: "行情数据".into(),
                state: DecisionStepState::Passed,
                summary: format!(
                    "{candle_count} 根日 K · {} · 截至 {}",
                    self.data_source, card.data_as_of
                ),
            }
        };

        let technical_step = if self.current_signal().is_none() {
            DecisionStep {
                title: "技术规则".into(),
                state: if self.loading {
                    DecisionStepState::Running
                } else {
                    DecisionStepState::Blocked
                },
                summary: "等待趋势、动量、量能与风险因子".into(),
            }
        } else if let Some(score) = card.score {
            DecisionStep {
                title: "技术规则".into(),
                state: if score >= 62.0 {
                    DecisionStepState::Passed
                } else {
                    DecisionStepState::Attention
                },
                summary: format!(
                    "策略匹配度 {score:.0} · 门槛 62 · {}",
                    card.supports
                        .first()
                        .map(String::as_str)
                        .unwrap_or("暂无主要支持因素")
                ),
            }
        } else {
            DecisionStep {
                title: "技术规则".into(),
                state: DecisionStepState::Attention,
                summary: "数据完整度不足，暂不输出强结论".into(),
            }
        };

        let evidence_date = card.data_as_of.as_str();
        let fundamental_step = match &self.analysis_state.fundamentals.state {
            RequestState::Idle | RequestState::Loading => DecisionStep {
                title: "基本面门槛".into(),
                state: DecisionStepState::Running,
                summary: "正在按公告日期读取质量证据".into(),
            },
            RequestState::Failed(message) => DecisionStep {
                title: "基本面门槛".into(),
                state: DecisionStepState::Blocked,
                summary: format!("数据不可用：{message}"),
            },
            RequestState::Ready(snapshot) => {
                let gate = quality_gate(&snapshot.metrics, evidence_date);
                if !gate.blockers.is_empty() {
                    DecisionStep {
                        title: "基本面门槛".into(),
                        state: DecisionStepState::Blocked,
                        summary: gate.blockers.join("；"),
                    }
                } else if !gate.unknown.is_empty() {
                    DecisionStep {
                        title: "基本面门槛".into(),
                        state: DecisionStepState::Attention,
                        summary: gate.unknown.join("；"),
                    }
                } else {
                    DecisionStep {
                        title: "基本面门槛".into(),
                        state: DecisionStepState::Passed,
                        summary: format!(
                            "质量门槛通过 · {} 项可追溯证据",
                            card.quality_evidence.len()
                        ),
                    }
                }
            }
        };

        let climate = self.market_climate_report();
        let climate_step = DecisionStep {
            title: "市场气候".into(),
            state: match climate.stance {
                NewEntryStance::Open => DecisionStepState::Passed,
                NewEntryStance::Selective => DecisionStepState::Attention,
                NewEntryStance::Freeze => DecisionStepState::Blocked,
            },
            summary: format!(
                "{} · {} · 新开仓风险 {:.0}%",
                climate.headline,
                climate.stance.label(),
                climate.risk_scale * 100.0
            ),
        };

        let levels_step = match self.current_levels() {
            Some(levels) => DecisionStep {
                title: "价位计划".into(),
                state: DecisionStepState::Passed,
                summary: format!(
                    "观察 {} · 失效 {:.2} · 目标 {}",
                    levels.buy_band_text(),
                    levels.buy_low,
                    levels.sell_band_text()
                ),
            },
            None => DecisionStep {
                title: "价位计划".into(),
                state: if self.loading {
                    DecisionStepState::Running
                } else {
                    DecisionStepState::Blocked
                },
                summary: "尚未形成可解释的观察、失效和目标价".into(),
            },
        };

        let sizing_result = self.position_sizing_plan(cx);
        let sizing_step = match &sizing_result {
            Ok(plan) => DecisionStep {
                title: "风险与仓位".into(),
                state: DecisionStepState::Passed,
                summary: format!(
                    "最多新买 {} 股 · 加仓后 {:.1}% · 失效损失约 {:.2}",
                    plan.shares, plan.capital_pct, plan.planned_loss
                ),
            },
            Err(error) => DecisionStep {
                title: "风险与仓位".into(),
                state: DecisionStepState::Blocked,
                summary: error.user_message().into(),
            },
        };

        let final_step = match (card.status, climate.stance, sizing_result.as_ref()) {
            (
                crate::domain::decision::DecisionStatus::MatchesStrategy,
                NewEntryStance::Freeze,
                _,
            ) => DecisionStep {
                title: "最终动作".into(),
                state: DecisionStepState::Attention,
                summary: "个股符合策略，但今日市场观望，不预填买入".into(),
            },
            (crate::domain::decision::DecisionStatus::MatchesStrategy, _, Err(error)) => {
                DecisionStep {
                    title: "最终动作".into(),
                    state: DecisionStepState::Attention,
                    summary: format!("策略条件符合，但暂不新增仓位：{}", error.user_message()),
                }
            }
            (crate::domain::decision::DecisionStatus::MatchesStrategy, _, Ok(_)) => DecisionStep {
                title: "最终动作".into(),
                state: DecisionStepState::Passed,
                summary: "可制定计划；等待进入观察区，不代表立即追价买入".into(),
            },
            (crate::domain::decision::DecisionStatus::Waiting, _, _) => DecisionStep {
                title: "最终动作".into(),
                state: DecisionStepState::Attention,
                summary: "继续观察，不预填买入；可设置价位提醒".into(),
            },
            (crate::domain::decision::DecisionStatus::NotEligible, _, _) => DecisionStep {
                title: "最终动作".into(),
                state: DecisionStepState::Blocked,
                summary: format!(
                    "不操作：{}",
                    card.risks
                        .first()
                        .map(String::as_str)
                        .unwrap_or("资格门槛未通过")
                ),
            },
            (crate::domain::decision::DecisionStatus::InsufficientEvidence, _, _) => DecisionStep {
                title: "最终动作".into(),
                state: if matches!(
                    self.analysis_state.fundamentals.state,
                    RequestState::Idle | RequestState::Loading
                ) || self.loading
                {
                    DecisionStepState::Running
                } else {
                    DecisionStepState::Blocked
                },
                summary: "证据不足，不做买入动作".into(),
            },
        };

        let trace_status = if card.status
            == crate::domain::decision::DecisionStatus::MatchesStrategy
            && (sizing_result.is_err() || climate.stance == NewEntryStance::Freeze)
        {
            crate::domain::decision::DecisionStatus::Waiting
        } else {
            card.status
        };

        DecisionTrace::build(
            self.selected.to_string(),
            trace_status,
            vec![
                data_step,
                climate_step,
                technical_step,
                fundamental_step,
                levels_step,
                sizing_step,
                final_step,
            ],
        )
    }

    pub(crate) fn position_sizing_plan(
        &self,
        cx: &gpui::Context<Self>,
    ) -> Result<PositionPlan, PositionSizingError> {
        let capital = super::helpers::parse_f64(&self.position_capital_input.read(cx).value())
            .unwrap_or_default();
        let climate = self.market_climate_report();
        if climate.stance == NewEntryStance::Freeze {
            return Err(PositionSizingError::NewEntriesRestricted);
        }
        let risk_pct = super::helpers::parse_f64(&self.position_risk_pct_input.read(cx).value())
            .unwrap_or_default()
            * climate.risk_scale;
        let levels = self
            .levels_cache
            .as_ref()
            .ok_or(PositionSizingError::InvalidEntry)?;
        let currency = Currency::for_code(self.selected.as_ref()).unwrap_or(Currency::Cny);
        let existing_shares = self
            .portfolio
            .position_of(self.selected.as_ref())
            .map(|position| position.shares.floor().max(0.0) as u64)
            .unwrap_or_default();
        let is_star_market = self.selected.starts_with("688") || self.selected.starts_with("689");
        calculate_position_plan(PositionSizingInput {
            capital,
            risk_pct,
            max_position_pct: 20.0,
            entry_price: levels.buy_high,
            invalidation_price: levels.buy_low,
            target_price: Some(levels.sell_low),
            existing_shares,
            lot_size: if is_star_market {
                1
            } else if currency == Currency::Cny {
                100
            } else {
                1
            },
            minimum_shares: if is_star_market {
                200
            } else if currency == Currency::Cny {
                100
            } else {
                1
            },
        })
    }

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
        let (fundamental_gate, fundamental_source, latest_evidence_date, quality_evidence) =
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
                                    metric.name == *name
                                        && metric.announced_on.as_str() <= data_as_of.as_str()
                                })
                                .max_by(|left, right| {
                                    (&left.reporting_period, &left.announced_on)
                                        .cmp(&(&right.reporting_period, &right.announced_on))
                                })?;
                            let value = metric.value?;
                            Some(QualityEvidence {
                                label: metric_label(name).into(),
                                value: match *name {
                                    "audit_risk_flag" if value < 1.0 => "标准无保留".into(),
                                    "audit_risk_flag" => "存在风险".into(),
                                    "dividend_continuity_years" => format!("{value:.0}"),
                                    _ => format!("{value:.2}"),
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
                "基本面质量门槛通过（证据截至 {}）",
                latest_evidence_date.as_deref().unwrap_or("—")
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
        if signal
            .as_ref()
            .is_some_and(|value| value.price_vs_ma20_pct >= 8.0)
        {
            risks.push("价格偏离 MA20 超过 8%，等待回踩而非追高".into());
        }
        let observation = levels.map(|value| format!("{} 元", value.buy_band_text()));
        let invalidation = levels.map(|value| format!("有效跌破 {:.2} 元", value.buy_low));
        let target = levels.map(|value| format!("{} 元", value.sell_band_text()));
        let risk_reward = levels.and_then(|value| {
            // Use the actual planned entry (upper edge of the observation band),
            // not the current close. This avoids showing an optimistic payoff
            // while position sizing uses a less favorable entry.
            let risk_distance = value.buy_high - value.buy_low;
            let reward_distance = value.sell_low - value.buy_high;
            (risk_distance > 0.0 && reward_distance > 0.0)
                .then_some(reward_distance / risk_distance)
        });
        if risk_reward.is_none_or(|ratio| ratio < crate::domain::decision::MINIMUM_PLAN_RISK_REWARD)
        {
            risks.push(format!(
                "计划盈亏比低于 {:.1}，不进入执行阶段",
                crate::domain::decision::MINIMUM_PLAN_RISK_REWARD
            ));
        }
        let evidence_grade = self
            .backtest_report
            .as_ref()
            .map(|report| report.verdict().label(false).to_string())
            .unwrap_or_else(|| {
                if self.candles.len() >= 120 {
                    "待运行样本外验证".into()
                } else {
                    "样本不足".into()
                }
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
            strategy_version: "technical-quality-gate-v4".into(),
            evidence_grade,
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
        let position_plan = self.position_sizing_plan(cx).ok();
        let plan = DecisionPlan {
            id: entry_id.clone(),
            code: code.clone(),
            created_on: created_on.to_string(),
            review_on: review_on.to_string(),
            trigger: trigger.clone(),
            observation_range: card.observation.clone().unwrap_or_else(|| "—".into()),
            invalidation: invalidation.clone(),
            target: card.target.clone(),
            risk_amount: position_plan.map(|position| {
                format!(
                    "计划新增 {} 股 / 加仓后 {} 股 · 金额 {:.2} · 失效损失约 {:.2} {}",
                    position.shares,
                    position.resulting_shares,
                    position.planned_notional,
                    position.planned_loss,
                    Currency::for_code(&code).unwrap_or(Currency::Cny).symbol()
                )
            }),
            status: PlanStatus::Planned,
            evidence: EvidenceSnapshot {
                strategy_version: card.strategy_version.clone(),
                data_as_of: card.data_as_of.clone(),
                source: card.source.clone(),
                payload_json: serde_json::to_string(&card).unwrap_or_else(|_| "{}".into()),
                score: card.score,
                regime: self
                    .current_signal()
                    .map(|signal| signal.regime.label().to_string()),
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
