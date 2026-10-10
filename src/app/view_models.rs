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
use crate::domain::position_review::{PositionReview, PositionReviewInput, analyze_position};
use crate::domain::position_sizing::{
    PositionPlan, PositionSizingError, PositionSizingInput, calculate_position_plan,
};

use super::StockApp;

impl StockApp {
    pub(crate) fn decision_trace_view_model(&self, cx: &gpui::Context<Self>) -> DecisionTrace {
        use crate::controller::state::RequestState;

        let card = self.decision_card_view_model();
        let candle_count = self.current_daily_candles().len();
        let daily_loading = matches!(self.chart_state.daily.request.state, RequestState::Loading);
        let data_step = if daily_loading && candle_count == 0 {
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
                    self.daily_analysis_source(),
                    card.data_as_of
                ),
            }
        };

        let technical_step = if self.current_signal().is_none() {
            DecisionStep {
                title: "技术规则".into(),
                state: if daily_loading {
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
        let climate_step = if self.financial_recovery_required() {
            DecisionStep {
                title: "市场气候".into(),
                state: DecisionStepState::Blocked,
                summary: "行情研究仍可用；本地数据待恢复，组合风控与新仓额度暂不可用".into(),
            }
        } else {
            DecisionStep {
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
            }
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
                state: if daily_loading {
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

        let final_step = if self.financial_recovery_required() {
            DecisionStep {
                title: "最终动作".into(),
                state: DecisionStepState::Blocked,
                summary: PositionSizingError::FinancialDataUnavailable
                    .user_message()
                    .into(),
            }
        } else {
            match (card.status, climate.stance, sizing_result.as_ref()) {
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
                (crate::domain::decision::DecisionStatus::MatchesStrategy, _, Ok(_)) => {
                    DecisionStep {
                        title: "最终动作".into(),
                        state: DecisionStepState::Passed,
                        summary: "可制定计划；等待进入观察区，不代表立即追价买入".into(),
                    }
                }
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
                (crate::domain::decision::DecisionStatus::InsufficientEvidence, _, _) => {
                    DecisionStep {
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
                    }
                }
            }
        };

        let trace_status = if self.financial_recovery_required() {
            crate::domain::decision::DecisionStatus::InsufficientEvidence
        } else if card.status == crate::domain::decision::DecisionStatus::MatchesStrategy
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
        if self.financial_recovery_required() {
            return Err(PositionSizingError::FinancialDataUnavailable);
        }
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
            .current_levels()
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
        let current_levels = self.current_levels();
        let levels = current_levels.as_ref();
        let quote_evidence = CanonicalQuoteEvidence::from_quote(
            self.selected.as_ref(),
            self.quote_for_code(self.selected.as_ref()),
        );
        let data_as_of = self
            .current_daily_candles()
            .last()
            .map(|candle| candle.date.to_string())
            .unwrap_or_else(|| "日线时间未知".into());
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
        let mut unknown = fundamental_gate.unknown.clone();
        if quote_evidence.name.is_none() {
            unknown.push("标的名称未由行情核验，无法确认 ST 风险门槛".into());
        }
        if let Some((code, error)) = &self.chart_state.daily.error
            && code == self.selected.as_ref()
        {
            unknown.push(format!("日线刷新失败：{error}"));
        }
        match self.quote_for_code(self.selected.as_ref()) {
            Some(quote)
                if quote.usable()
                    && quote.effective_freshness(chrono::Utc::now().timestamp_millis())
                        == crate::domain::market::Freshness::Live => {}
            Some(quote) => unknown.push(format!(
                "当前行情{}（{}）",
                quote.freshness_label(),
                quote.as_of_label()
            )),
            None => unknown.push("当前行情缺失".into()),
        }
        let mut blockers = Vec::new();
        if quote_evidence.st_risk {
            blockers.push("ST 风险门槛".into());
        }
        if self.current_daily_candles().len() < 30 {
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
                if self.current_daily_candles().len() >= 120 {
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
                unknown,
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
            source: format!(
                "日线 {}；基本面 {fundamental_source}",
                self.daily_analysis_source()
            ),
            adjustment: "前复权".into(),
            sample_size: self.current_daily_candles().len(),
            strategy_version: "technical-quality-gate-v4".into(),
            evidence_grade,
        })
    }

    pub(crate) fn record_decision_plan_from_card(&mut self, cx: &mut gpui::Context<Self>) {
        if !self.require_journal_writable(cx) {
            return;
        }
        use crate::data::journal::{self, JournalEntry, JournalKind};

        // Re-evaluate safety evidence at the save boundary, independently of preview metadata.
        let card = self.decision_card_view_model();
        let code = self.selected.to_string();
        let quote_evidence = CanonicalQuoteEvidence::from_quote(&code, self.quote_for_code(&code));
        let name = quote_evidence
            .name
            .clone()
            .unwrap_or_else(|| format!("{code}（名称未知）"));
        let price = quote_evidence.price;
        let quote_note = self
            .quote_for_code(&code)
            .map(|quote| {
                format!(
                    "{} · {} · {}",
                    quote.source,
                    quote.as_of_label(),
                    quote.freshness_label()
                )
            })
            .unwrap_or_else(|| "行情缺失，参考价格未知".into());
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
            "计划 · {trigger} · 失效：{invalidation} · 复盘：{review_on} · {} · {quote_note}",
            card.strategy_version
        );
        let target = self.current_levels().map(|levels| levels.sell_low);
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
        self.status = crate::model::shared("已创建计划，正在保存当时证据快照");
        cx.notify();
    }

    /// Cost-vs-last review of the selected open lot. Always local and
    /// deterministic; missing quotes or K-lines only mark those dimensions
    /// unknown instead of inventing a stance.
    pub(crate) fn position_review_view_model(&self) -> Option<PositionReview> {
        if self.financial_recovery_required() {
            return None;
        }
        let code = self.selected.to_string();
        let position = self.portfolio.position_of(&code)?;
        let quote = self.quote_for_code(&code)?;
        let last = quote
            .price
            .filter(|price| price.is_finite() && *price > 0.0)?;
        let mark = crate::data::portfolio::PositionMark::from_position(position, last, 0.0);
        let signal = self.current_signal();
        let current_levels = self.current_levels();
        let levels = current_levels.as_ref();
        let alert = self.buy_alerts.get(&code);
        let climate = self.market_climate_report();
        let quote_stale = !quote.usable()
            || quote.effective_freshness(chrono::Utc::now().timestamp_millis())
                != crate::domain::market::Freshness::Live;
        let weight = self
            .portfolio_risk_view(&self.portfolio_summary())
            .items
            .iter()
            .find(|item| item.code == code)
            .map(|item| item.position_weight_pct);
        let open_plan = self.journal.for_code(&code).into_iter().find_map(|entry| {
            entry.plan.as_ref().filter(|plan| {
                matches!(
                    plan.status,
                    PlanStatus::Planned | PlanStatus::Executed | PlanStatus::DueForReview
                )
            })
        });
        let as_of = chrono::Utc::now()
            .with_timezone(&crate::domain::market::exchange_offset())
            .date_naive()
            .to_string();
        let range_position_60_pct = levels.and_then(|item| {
            let low = item.low_60?;
            let high = item.high_60?;
            (high > low).then_some((last - low) / (high - low) * 100.0)
        });
        analyze_position(&PositionReviewInput {
            code,
            shares: mark.position.shares,
            avg_cost: mark.position.avg_cost,
            last,
            unrealized_pnl: mark.unrealized_pnl?,
            unrealized_pnl_pct: mark.unrealized_pnl_pct?,
            realized_pnl: mark.position.realized_pnl,
            held_calendar_days: held_calendar_days(
                self.portfolio
                    .open_lot_opened_on(&mark.position.code)
                    .as_deref(),
                &as_of,
            ),
            trade_count: mark.position.trade_count,
            quote_stale,
            regime_label: signal.as_ref().map(|item| item.regime.label().to_string()),
            score: signal.as_ref().map(|item| item.score),
            rsi14: signal.as_ref().and_then(|item| item.rsi14),
            price_vs_ma20_pct: signal.as_ref().map(|item| item.price_vs_ma20_pct),
            range_position_60_pct,
            atr14: levels.and_then(|item| item.atr14),
            buy_low: levels.map(|item| item.buy_low),
            buy_high: levels.map(|item| item.buy_high),
            sell_low: levels.map(|item| item.sell_low),
            sell_high: levels.map(|item| item.sell_high),
            stop_price: alert.and_then(|item| item.stop_price),
            take_profit: alert.and_then(|item| item.sell_price),
            stop_triggered: alert.is_some_and(|item| item.stop_triggered),
            take_profit_triggered: alert.is_some_and(|item| item.sell_triggered),
            position_weight_pct: weight,
            climate_label: Some(climate.climate.label().into()),
            new_entries_frozen: climate.stance == NewEntryStance::Freeze,
            has_open_plan: open_plan.is_some(),
            plan_invalidation: open_plan.as_ref().map(|plan| plan.invalidation.clone()),
            plan_target: open_plan.and_then(|plan| plan.target.clone()),
        })
    }
}

fn held_calendar_days(opened_on: Option<&str>, as_of: &str) -> Option<u32> {
    let opened = opened_on?.get(..10)?;
    let as_of = as_of.get(..10)?;
    let start = chrono::NaiveDate::parse_from_str(opened, "%Y-%m-%d").ok()?;
    let end = chrono::NaiveDate::parse_from_str(as_of, "%Y-%m-%d").ok()?;
    let days = end.signed_duration_since(start).num_days();
    if days < 0 {
        Some(0)
    } else {
        u32::try_from(days).ok()
    }
}

/// Canonical evidence at safety and plan-save boundaries; preview values are display hints only.
#[derive(Debug, PartialEq)]
struct CanonicalQuoteEvidence {
    name: Option<String>,
    price: Option<f64>,
    st_risk: bool,
}

impl CanonicalQuoteEvidence {
    fn from_quote(code: &str, quote: Option<&crate::domain::market::QuoteRecord>) -> Self {
        let quote = quote.filter(|quote| quote.code == code);
        let name = quote
            .map(|quote| quote.name.trim())
            .filter(|name| super::helpers::is_real_name(name, code))
            .map(str::to_string);
        let st_risk = name
            .as_ref()
            .is_some_and(|name| name.to_ascii_uppercase().contains("ST"));
        let price = quote
            .filter(|quote| quote.usable())
            .and_then(|quote| quote.price);
        Self {
            name,
            price,
            st_risk,
        }
    }
}

#[cfg(test)]
mod canonical_quote_evidence_tests {
    use super::CanonicalQuoteEvidence;
    use crate::domain::market::{Availability, Freshness, Market, QuoteRecord};
    use crate::domain::money::Currency;

    fn fixture(name: &str, price: Option<f64>) -> QuoteRecord {
        QuoteRecord {
            code: "600519".into(),
            market: Market::AShare,
            currency: Currency::Cny,
            name: name.into(),
            price,
            change_pct: None,
            volume: None,
            source: "offline fixture".into(),
            fetched_at: 0,
            market_time: None,
            availability: Availability::Available,
            freshness: Freshness::Unknown,
        }
    }

    #[test]
    fn canonical_st_name_controls_safety_even_without_watchlist_or_preview_name() {
        let quote = fixture("*ST离线样本", Some(12.0));
        let evidence = CanonicalQuoteEvidence::from_quote("600519", Some(&quote));
        assert_eq!(evidence.name.as_deref(), Some("*ST离线样本"));
        assert!(evidence.st_risk);
        assert_eq!(evidence.price, Some(12.0));
    }

    #[test]
    fn missing_invalid_or_other_instrument_quote_never_saves_zero_as_plan_price() {
        let missing = CanonicalQuoteEvidence::from_quote("600519", None);
        assert_eq!(missing.name, None);
        assert_eq!(missing.price, None);
        for price in [None, Some(0.0), Some(-1.0), Some(f64::NAN)] {
            let quote = fixture("600519", price);
            let evidence = CanonicalQuoteEvidence::from_quote("600519", Some(&quote));
            assert_eq!(evidence.name, None);
            assert_eq!(evidence.price, None);
        }
        let other =
            CanonicalQuoteEvidence::from_quote("000001", Some(&fixture("样本", Some(12.0))));
        assert_eq!(other.name, None);
        assert_eq!(other.price, None);
    }
}
