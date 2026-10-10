//! Portfolio trades, cash, and position AI advice.

use gpui::{Context, Timer, Window};

use crate::data::ai::{self};
use crate::data::portfolio::{
    LocalTradeForm, PortfolioSummary, TradeConfirmation, TradeSide, format_shares,
};
use crate::domain::money::Currency;
use crate::domain::portfolio::{PortfolioRiskView, RiskItem};
use crate::model::{Symbol, board_for_code, format_price, normalize_code, shared};
use crate::storage::{self, AppConfig};

use super::helpers::*;
use super::{AiCacheEntry, AiPanelState, AiSource, DetailTab, LeftTab, StockApp};

fn portfolio_advice_key(
    code: &str,
    daily: &[crate::model::Candle],
    position: Option<&crate::data::portfolio::Position>,
    quote: Option<&crate::domain::market::QuoteRecord>,
    now_millis: i64,
) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let latest = daily.last()?;
    let mut evidence = std::collections::hash_map::DefaultHasher::new();
    for candle in daily {
        candle.date.as_ref().hash(&mut evidence);
        candle.open.to_bits().hash(&mut evidence);
        candle.high.to_bits().hash(&mut evidence);
        candle.low.to_bits().hash(&mut evidence);
        candle.close.to_bits().hash(&mut evidence);
        candle.volume.hash(&mut evidence);
    }
    Some(format!(
        "pos:{code}@{}:{:x}:{:?}:{:?}:{:?}:{:?}:{:?}",
        latest.date,
        evidence.finish(),
        position.map(|position| (
            position.shares.to_bits(),
            position.avg_cost.to_bits(),
            position.realized_pnl.to_bits()
        )),
        quote.and_then(|quote| quote.price).map(f64::to_bits),
        quote.and_then(|quote| quote.change_pct).map(f64::to_bits),
        quote.map(|quote| quote.source.as_str()),
        quote.map(|quote| (quote.availability, quote.effective_freshness(now_millis)))
    ))
}

impl StockApp {
    pub(crate) fn prefill_position_sized_buy(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.decision_card_view_model().status
            != crate::domain::decision::DecisionStatus::MatchesStrategy
        {
            self.status = shared("当前结论不是“符合策略”，只保留观察/提醒，不预填买入记录");
            cx.notify();
            return;
        }
        let plan = match self.position_sizing_plan(cx) {
            Ok(plan) => plan,
            Err(error) => {
                self.status = shared(error.user_message());
                cx.notify();
                return;
            }
        };
        if !self.open_trade_form(TradeSide::Buy, window, cx) {
            return;
        }
        self.trade_shares_input.update(cx, |state, cx| {
            state.set_value(plan.shares.to_string(), window, cx);
        });
        self.trade_price_input.update(cx, |state, cx| {
            state.set_value(format_price(plan.entry_price), window, cx);
        });
        self.trade_note_input.update(cx, |state, cx| {
            state.set_value(
                format!(
                    "纪律仓位：失效价 {}，计划损失不超过约 {:.2}",
                    format_price(plan.invalidation_price),
                    plan.planned_loss
                ),
                window,
                cx,
            );
        });
        self.status = shared("已预填本地买入记录；不会自动下单，请按实际成交修改");
        cx.notify();
    }

    pub(crate) fn prefill_champion_stock_plan(
        &mut self,
        code: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (_, plans) = self.champion_stock_plans(cx);
        let Some(plan) = plans.into_iter().find(|plan| plan.code == code) else {
            self.status = shared("当前冠军策略还没有这只股票的买卖计划");
            cx.notify();
            return;
        };
        match plan.kind {
            crate::domain::strategy_application::StockPlanKind::Buy
            | crate::domain::strategy_application::StockPlanKind::Sell => {}
            _ => {
                self.status = shared("这只股票目前不是买入或卖出计划，不能预填成交");
                cx.notify();
                return;
            }
        }
        self.ensure_today_symbol(&plan.code);
        self.select_symbol(shared(plan.code.clone()), cx);
        let side = if plan.kind == crate::domain::strategy_application::StockPlanKind::Buy {
            TradeSide::Buy
        } else {
            TradeSide::Sell
        };
        if !self.open_trade_form(side, window, cx) {
            return;
        }
        self.trade_shares_input.update(cx, |state, cx| {
            state.set_value(plan.shares.to_string(), window, cx);
        });
        self.trade_price_input.update(cx, |state, cx| {
            state.set_value(format_price(plan.price), window, cx);
        });
        self.trade_note_input.update(cx, |state, cx| {
            state.set_value(
                format!(
                    "冠军策略「{}」：{} {} 股 @ {}。{}",
                    plan.strategy_name,
                    plan.kind.label(),
                    plan.shares,
                    format_price(plan.price),
                    plan.reason
                ),
                window,
                cx,
            );
        });
        self.status = shared("已按冠军策略预填本地记录；次日开盘成交，不会自动下单");
        cx.notify();
    }

    /// Queue the latest config snapshot without disk or credential IO on GPUI.
    pub(crate) fn persist(&mut self) {
        let mut dock = self.dock.clone();
        dock.window = self.window_bounds;
        let cfg = AppConfig {
            schema_version: crate::storage::CONFIG_SCHEMA_VERSION,
            watchlist: self.symbols.iter().map(|s| s.code.clone()).collect(),
            selected: self.selected.to_string(),
            range: self.range.label().into(),
            chart_kind: self.chart_kind.to_label().into(),
            show_ma5: self.show_ma5,
            show_ma10: self.show_ma10,
            show_ma20: self.show_ma20,
            show_ma60: self.show_ma60,
            show_volume: self.show_volume,
            show_macd: self.show_macd,
            show_boll: self.show_boll,
            dock,
            left_width: self.left_width,
            bottom_height: self.bottom_height,
            color_scheme: self.color_scheme,
            work_mode: self.work_mode,
            work_density: self.work_density,
            work_right_width: self.work_right_width,
            work_aliases: self.work_aliases.clone(),
            quote_interval_secs: self.quote_interval_secs,
            watchlist_sort: self.watchlist_sort,
            ai_api: self.ai_config.clone(),
            buy_alerts: self.buy_alerts.clone(),
            chart_lines: self.chart_lines.clone(),
            treasure_pool: self.treasure_pool.id().into(),
            treasure_fin: self.treasure_fin.id().into(),
            status_bar_enabled: self.status_bar_enabled,
            status_bar_codes: self.status_bar_codes.clone(),
            status_bar_active: self.status_bar_active.clone(),
            detail_tab: self.detail_tab.to_label().into(),
            left_tab: self.left_tab.to_label().into(),
            find_mode: self.find_mode.id().into(),
            watch_tags: self
                .watch_tags
                .iter()
                .filter(|(_, t)| **t != crate::data::groups::WatchTag::None)
                .map(|(k, t)| (k.clone(), t.id().into()))
                .collect(),
            watch_filter: self.watch_filter.id().into(),
        };
        if self.ai_api_key_dirty.get() {
            let states = storage::persistence_worker().status();
            let pending_state = self.ai_api_key_pending.and_then(|(revision, generation)| {
                (generation == self.ai_api_key_edit_gen)
                    .then(|| (revision, states.get(&storage::Slot::Credential)))
            });
            match pending_state {
                Some((revision, Some(storage::SaveState::Saved(current))))
                    if revision == *current =>
                {
                    self.ai_api_key_dirty.set(false);
                    self.ai_api_key_save_error = None;
                    self.ai_api_key_pending = None;
                }
                Some((revision, Some(storage::SaveState::Pending(current))))
                    if revision == *current => {}
                _ => {
                    let revision = storage::enqueue_api_key(self.ai_config.api_key.clone());
                    self.ai_api_key_pending = Some((revision, self.ai_api_key_edit_gen));
                    self.ai_api_key_save_error = None;
                }
            }
        }
        storage::enqueue_config(cfg);
    }

    /// Debounced config write — collapses rapid UI thrash (resize, typing, tab flips).
    pub(crate) fn schedule_persist(&mut self, cx: &mut Context<Self>) {
        self.persist_gen = self.persist_gen.wrapping_add(1);
        let token = self.persist_gen;
        cx.spawn(async move |this, cx| {
            Timer::after(super::PERSIST_DEBOUNCE).await;
            let _ = this.update(cx, |app, cx| {
                if app.persist_gen == token {
                    app.persist();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn persist_portfolio(&self) {
        if self.portfolio_recovery.is_none() && !self.recovery_busy {
            storage::enqueue_portfolio(self.portfolio.clone());
        }
    }

    pub(crate) fn require_portfolio_writable(&mut self, cx: &mut Context<Self>) -> bool {
        if self.shutdown_pending {
            self.status = shared("正在保存并退出，暂时不能修改持仓");
            cx.notify();
            return false;
        }
        if self.portfolio_recovery.is_some() || self.recovery_busy {
            self.status = shared("持仓处于只读恢复模式，请先到设置 → 通用恢复数据");
            self.trade_feedback = Some(self.status.clone());
            cx.notify();
            return false;
        }
        true
    }

    pub(crate) fn require_journal_writable(&mut self, cx: &mut Context<Self>) -> bool {
        if self.shutdown_pending {
            self.status = shared("正在保存并退出，暂时不能修改日记");
            cx.notify();
            return false;
        }
        if self.journal_recovery.is_some() || self.recovery_busy {
            self.status = shared("日记处于只读恢复模式，请先到设置 → 通用恢复数据");
            cx.notify();
            return false;
        }
        true
    }

    pub(crate) fn start_persistence_observer(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(std::time::Duration::from_millis(150)).await;
                if this
                    .update(cx, |app, cx| {
                        app.observe_persistence(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn observe_persistence(&mut self, cx: &mut Context<Self>) {
        let status = storage::persistence_worker().status();
        if self.persistence_status == status {
            return;
        }
        let previous = std::mem::replace(&mut self.persistence_status, status.clone());
        for (slot, state) in &status {
            if previous.get(slot) == Some(state) {
                continue;
            }
            match state {
                storage::SaveState::Failed(_, error) => {
                    let message = format!("{}尚未保存：{error}", persistence_label(*slot));
                    self.status = shared(message.clone());
                    if *slot == storage::Slot::Credential {
                        self.ai_api_key_save_error = Some(shared(message));
                    }
                    if *slot == storage::Slot::Portfolio {
                        self.portfolio_recovery = crate::infrastructure::storage::recovery::state(
                            &storage::portfolio_path(),
                        );
                        self.trade_feedback = Some(self.status.clone());
                    }
                    if *slot == storage::Slot::Journal {
                        self.journal_recovery = crate::infrastructure::storage::recovery::state(
                            &storage::journal_path(),
                        );
                    }
                }
                storage::SaveState::Saved(revision) => {
                    if *slot == storage::Slot::Credential {
                        if self.ai_api_key_pending == Some((*revision, self.ai_api_key_edit_gen)) {
                            self.ai_api_key_dirty.set(false);
                            self.ai_api_key_save_error = None;
                            self.ai_api_key_pending = None;
                            self.status = shared("API Key 更改已安全写入系统凭据库");
                        }
                    } else if matches!(slot, storage::Slot::Portfolio | storage::Slot::Journal) {
                        self.status = shared(format!("{}已保存到本地", persistence_label(*slot)));
                        if *slot == storage::Slot::Portfolio {
                            self.trade_feedback = Some(self.status.clone());
                        }
                    }
                }
                storage::SaveState::Pending(_) => {}
            }
        }
        // Errors remain visible even when an unrelated save completes.
        if let Some((slot, storage::SaveState::Failed(_, error))) = status
            .iter()
            .find(|(_, state)| matches!(state, storage::SaveState::Failed(_, _)))
        {
            self.status = shared(format!("{}尚未保存：{error}", persistence_label(*slot)));
        }
        if self.financial_recovery_required() {
            self.portfolio_ai_gen = self.portfolio_ai_gen.wrapping_add(1);
            self.portfolio_ai_key = None;
            self.portfolio_ai_cache.clear();
        }
        self.sync_status_bar();
        cx.notify();
    }

    pub(crate) fn financial_recovery_required(&self) -> bool {
        self.portfolio_recovery.is_some() || self.journal_recovery.is_some() || self.recovery_busy
    }

    pub(crate) fn recovery_banner_height(&self) -> gpui::Pixels {
        gpui::px(if self.financial_recovery_required() {
            56.0
        } else {
            0.0
        })
    }

    pub(crate) fn persistence_summary(&self) -> String {
        if self.financial_recovery_required() {
            return "本地数据只读恢复模式".into();
        }
        if let Some((slot, storage::SaveState::Failed(_, error))) = self
            .persistence_status
            .iter()
            .find(|(_, state)| matches!(state, storage::SaveState::Failed(_, _)))
        {
            return format!("{}未保存：{error}", persistence_label(*slot));
        }
        if self.ai_api_key_dirty.get()
            || storage::persistence_worker()
                .status()
                .values()
                .any(|state| matches!(state, storage::SaveState::Pending(_)))
        {
            "正在保存更改…".into()
        } else {
            "更改已保存".into()
        }
    }

    /// Quotes belong to instruments, not the user's watchlist.
    pub(crate) fn portfolio_summary(&self) -> PortfolioSummary {
        self.portfolio
            .summarize_with(|code| self.quote_for_code(code).cloned())
    }

    /// A risk projection built from current holdings and deterministic alert
    /// invalidation prices. Weights are calculated inside each currency group;
    /// there is deliberately no cross-currency total without an FX contract.
    pub(crate) fn portfolio_risk_view(&self, summary: &PortfolioSummary) -> PortfolioRiskView {
        let items = summary
            .positions
            .iter()
            .map(|mark| {
                let currency = mark.position.currency;
                let group_value = summary
                    .by_currency
                    .get(&currency)
                    .map(|totals| totals.total_market_value)
                    .unwrap_or_default();
                let position_weight_pct = if group_value > 0.0 {
                    mark.market_value.unwrap_or(0.0) / group_value * 100.0
                } else {
                    0.0
                };
                let raw_quote = mark.last;
                let quote_stale = mark.quote_is_uncertain();
                let stop = self
                    .buy_alerts
                    .get(&mark.position.code)
                    .and_then(|alert| alert.stop_price)
                    .filter(|price| price.is_finite() && *price > 0.0);
                let risk_amount = raw_quote.zip(stop).and_then(|(last, stop)| {
                    crate::domain::money::Money::from_major(
                        currency,
                        (last - stop).max(0.0) * mark.position.shares,
                    )
                });
                RiskItem {
                    code: mark.position.code.clone(),
                    currency,
                    position_weight_pct,
                    risk_amount,
                    // No point-in-time sector contract is available yet. Keeping
                    // this unknown is safer than deriving a false concentration.
                    industry: None,
                    quote_stale,
                    invalidation_breached: raw_quote
                        .zip(stop)
                        .is_some_and(|(last, stop)| last <= stop),
                }
            })
            .collect();
        PortfolioRiskView::from_items(items)
    }

    /// Explicitly add a code to the watchlist; holdings receive quotes independently.
    pub(crate) fn ensure_in_watchlist(&mut self, code: &str, name: &str, last: f64) {
        let code = normalize_code(code).unwrap_or_else(|| code.trim().to_string());
        if code.is_empty() {
            return;
        }
        if self.symbols.iter().any(|s| s.code == code) {
            if let Some(sym) = self.symbols.iter_mut().find(|s| s.code == code) {
                if is_real_name(name, &code) && !is_real_name(sym.name.as_ref(), &code) {
                    sym.name = shared(name.to_string());
                }
                if last > 0.0 && sym.last <= 0.0 {
                    sym.last = last;
                }
            }
            return;
        }
        self.symbols.push(Symbol {
            code: code.clone(),
            name: shared(if is_real_name(name, &code) {
                name.to_string()
            } else {
                code.clone()
            }),
            last,
            change_pct: 0.0,
            volume: 0,
            board: board_for_code(&code),
        });
        self.filtered_local = (0..self.symbols.len()).collect();
    }

    pub(crate) fn dismiss_overlay(&mut self, cx: &mut Context<Self>) {
        if self.work_alias_editing {
            self.cancel_work_alias_edit(cx);
            return;
        }
        if self.palette_open {
            self.close_palette(cx);
            return;
        }
        if self.trade_form.is_some() {
            self.trade_form = None;
            self.app_focus_pending = true;
            self.trade_feedback = None;
            cx.notify();
            return;
        }
        if self.settings_open {
            self.close_settings(cx);
            return;
        }
        if self.market_analysis_open {
            if self.market_heatmap_fullscreen {
                self.toggle_market_heatmap_fullscreen(cx);
                return;
            }
            if self.market_heatmap_can_go_back() {
                self.back_market_heatmap(cx);
                return;
            }
            self.close_market_analysis(cx);
            return;
        }
        if self.drawing_mode {
            self.drawing_mode = false;
            self.drawing_anchor = None;
            self.draft_line = None;
            self.status = shared(if self.work_mode {
                "draw mode off"
            } else {
                "已退出画线模式"
            });
            cx.notify();
        }
    }

    /// Open an editable local record, binding its identity before any input.
    /// A quote is only a reference; recording a trade never sends a broker order.
    pub(crate) fn open_trade_form(
        &mut self,
        side: TradeSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.require_portfolio_writable(cx) {
            return false;
        }
        if let Some(form) = &self.trade_form {
            self.trade_feedback = Some(shared(format!(
                "{} {} {} · 请先完成或取消当前本地记录",
                form.name,
                form.code,
                form.currency.symbol()
            )));
            self.left_tab = LeftTab::Portfolio;
            cx.notify();
            return false;
        }
        let code = self.selected.to_string();
        let position = self.portfolio.position_of(&code);
        let quote = self.quote_for_code(&code).filter(|quote| quote.usable());
        let name = quote
            .map(|quote| quote.name.clone())
            .filter(|name| is_real_name(name, &code))
            .or_else(|| position.as_ref().map(|position| position.name.clone()))
            .or_else(|| self.current_symbol().map(|symbol| symbol.name.to_string()))
            .unwrap_or_else(|| code.clone());
        let mut form = match LocalTradeForm::new(&code, &name, side) {
            Ok(form) => form,
            Err(error) => {
                self.trade_feedback = Some(shared(error.to_string()));
                self.left_tab = LeftTab::Portfolio;
                cx.notify();
                return false;
            }
        };
        let price_s = quote
            .and_then(|quote| quote.price)
            .map(format_price)
            .unwrap_or_default();
        form.reference_hint = if let Some(quote) = quote {
            format!(
                "参考行情：{} · {} · {}；请按实际成交修改价格与费用",
                quote.source,
                quote.freshness_label(),
                quote.as_of_label()
            )
        } else {
            "行情不可用，请填写实际成交价格与费用".into()
        };
        let shares_s = if side == TradeSide::Sell {
            position
                .as_ref()
                .map(|position| position.shares.to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.trade_price_input
            .update(cx, |state, cx| state.set_value(price_s, window, cx));
        self.trade_shares_input
            .update(cx, |state, cx| state.set_value(shares_s, window, cx));
        self.trade_fee_input
            .update(cx, |state, cx| state.set_value("0", window, cx));
        self.trade_note_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.trade_form = Some(form);
        self.trade_feedback = None;
        self.left_tab = LeftTab::Portfolio;
        self.persist();
        cx.notify();
        true
    }

    pub(crate) fn review_trade(&mut self, cx: &mut Context<Self>) {
        if self.trade_form.is_none() {
            return;
        }
        let shares = parse_f64(&self.trade_shares_input.read(cx).value());
        let price = parse_f64(&self.trade_price_input.read(cx).value());
        let raw_fee = self.trade_fee_input.read(cx).value();
        let fee = if raw_fee.trim().is_empty() {
            Some(0.0)
        } else {
            parse_f64(&raw_fee)
        };
        let note = self.trade_note_input.read(cx).value().to_string();
        let confirmation = shares
            .ok_or(crate::data::portfolio::TradeError::InvalidShares)
            .and_then(|shares| {
                let price = price.ok_or(crate::data::portfolio::TradeError::InvalidPrice)?;
                let fee = fee.ok_or(crate::data::portfolio::TradeError::InvalidFee)?;
                TradeConfirmation::new(shares, price, fee, note)
            });
        match confirmation {
            Ok(confirmation) => {
                if let Some(form) = &mut self.trade_form {
                    form.confirmation = Some(confirmation);
                }
                self.trade_feedback = None;
            }
            Err(error) => self.trade_feedback = Some(shared(error.to_string())),
        }
        cx.notify();
    }

    pub(crate) fn submit_trade(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.require_portfolio_writable(cx) {
            return;
        }
        let Some(form) = self.trade_form.clone() else {
            return;
        };
        let Some(draft) = form.confirmed_draft() else {
            // A click on the editing screen cannot write an unreviewed record.
            return;
        };
        let confirmation = form.confirmation.as_ref().expect("confirmed draft");
        match self.portfolio.record_trade(draft) {
            Ok(_) => {
                self.persist_portfolio();
                self.trade_form = None;
                self.app_focus_pending = true;
                let message = shared(format!(
                    "本地{}记录待保存：{} {} · {} · {} 股 @ {} · 费用 {:.2} · 合计 {:.2}（未连接券商）",
                    form.side.label(),
                    form.name,
                    form.code,
                    form.currency.symbol(),
                    format_shares(confirmation.shares),
                    format_price(confirmation.price),
                    confirmation.fee,
                    confirmation.total(form.side)
                ));
                self.status = message.clone();
                self.trade_feedback = Some(message);
                self.detail_tab = DetailTab::Portfolio;
                self.persist();
                self.trade_shares_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
                self.trade_note_input
                    .update(cx, |state, cx| state.set_value("", window, cx));
            }
            Err(error) => {
                let message = shared(error.to_string());
                self.status = message.clone();
                self.trade_feedback = Some(message);
            }
        }
        cx.notify();
    }

    /// Full exit only prepares a sell record. Price and fees remain editable,
    /// and the same review/confirmation step is required as every local record.
    pub(crate) fn close_selected_position(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.portfolio.position_of(self.selected.as_ref()).is_none() {
            self.trade_feedback = Some(shared(if self.work_mode {
                "No position"
            } else {
                "当前无持仓"
            }));
            cx.notify();
            return;
        }
        if self.trade_form.is_some() {
            self.open_trade_form(TradeSide::Sell, window, cx);
            return;
        }
        if !self.open_trade_form(TradeSide::Sell, window, cx) {
            return;
        }
        self.trade_note_input
            .update(cx, |state, cx| state.set_value("清仓记录", window, cx));
    }

    pub(crate) fn undo_last_trade_for_selected(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        self.undo_last_trade_for_code(&code, cx);
    }

    pub(crate) fn undo_last_trade_for_code(&mut self, code: &str, cx: &mut Context<Self>) {
        if !self.require_portfolio_writable(cx) {
            return;
        }
        let id = self
            .portfolio
            .trades
            .iter()
            .rev()
            .find(|t| t.code == code)
            .map(|t| t.id.clone());
        let Some(id) = id else {
            self.status = shared(if self.work_mode {
                "No trade to undo"
            } else {
                "没有可撤销的成交"
            });
            self.trade_feedback = Some(self.status.clone());
            cx.notify();
            return;
        };
        if self.portfolio.remove_trade(&id) {
            self.persist_portfolio();
            self.status = shared(if self.work_mode {
                "Trade undo pending save"
            } else {
                "已撤销最近一笔成交，正在保存"
            });
        } else {
            self.status = shared(if self.work_mode {
                "Cannot undo (would break history)"
            } else {
                "无法撤销：会破坏后续卖出流水"
            });
        }
        self.trade_feedback = Some(shared(format!("{code} · {}", self.status)));
        cx.notify();
    }

    pub(crate) fn apply_portfolio_cash(&mut self, cx: &mut Context<Self>) {
        if !self.require_portfolio_writable(cx) {
            return;
        }
        let raw = self.portfolio_cash_input.read(cx).value();
        let Some(v) = parse_f64(&raw) else {
            self.status = shared(if self.work_mode {
                "Invalid cash"
            } else {
                "现金金额无效"
            });
            cx.notify();
            return;
        };
        if v < 0.0 {
            self.status = shared(if self.work_mode {
                "Cash cannot be negative"
            } else {
                "现金不能为负"
            });
            cx.notify();
            return;
        }
        let currency = Currency::for_code(self.selected.as_ref()).unwrap_or(Currency::Cny);
        if self.portfolio.set_cash(currency, v).is_err() {
            self.status = shared("现金金额超出范围");
            cx.notify();
            return;
        }
        self.persist_portfolio();
        self.status = shared(if self.work_mode {
            format!("Saving cash = {v:.2} {}", currency.symbol())
        } else {
            format!("现金设为 {v:.2} {}，正在保存", currency.symbol())
        });
        cx.notify();
    }

    pub(crate) fn toggle_track_cash(&mut self, cx: &mut Context<Self>) {
        if !self.require_portfolio_writable(cx) {
            return;
        }
        self.portfolio.track_cash = !self.portfolio.track_cash;
        self.persist_portfolio();
        cx.notify();
    }

    /// The producer and display use the same daily/quote identity. Intraday
    /// chart selections never participate in a daily position-advice cache key.
    pub(crate) fn portfolio_advice_cache_key(&self) -> Option<String> {
        if self.financial_recovery_required() {
            return None;
        }
        let code = self.selected.as_ref();
        portfolio_advice_key(
            code,
            self.current_daily_candles(),
            self.portfolio.position_state_of(code).as_ref(),
            self.quote_for_code(code),
            chrono::Utc::now().timestamp_millis(),
        )
    }

    pub(crate) fn request_portfolio_ai(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        // Also invalidate an in-flight response when a new request is blocked.
        self.portfolio_ai_gen = self.portfolio_ai_gen.wrapping_add(1);
        let req_id = self.portfolio_ai_gen;
        if self.financial_recovery_required() {
            self.portfolio_ai_key = None;
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: shared("本地金融数据不可用，恢复前暂停持仓与买卖建议。"),
                source: AiSource::Local,
                note: None,
            };
            cx.notify();
            return;
        }
        let Some(cache_key) = self.portfolio_advice_cache_key() else {
            self.portfolio_ai_key = None;
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: shared(if self.work_mode {
                    "Load daily evidence first."
                } else {
                    "请先加载该标的日线证据。"
                }),
                source: AiSource::Local,
                note: None,
            };
            cx.notify();
            return;
        };
        self.portfolio_ai_key = Some(cache_key.clone());
        let name = self
            .quote_for_code(&code)
            .map(|quote| quote.name.clone())
            .filter(|name| is_real_name(name, &code))
            .or_else(|| self.current_symbol().map(|symbol| symbol.name.to_string()))
            .unwrap_or_default();
        let pos = self.portfolio.position_state_of(&code);
        let (shares, avg_cost, realized) = pos
            .map(|p| (p.shares, p.avg_cost, p.realized_pnl))
            .unwrap_or((0.0, 0.0, 0.0));
        let quote = self.quote_for_code(&code).filter(|quote| {
            quote.usable()
                && matches!(
                    quote.effective_freshness(chrono::Utc::now().timestamp_millis()),
                    crate::domain::market::Freshness::Live
                        | crate::domain::market::Freshness::Delayed
                )
        });
        let Some(last) = quote.and_then(|quote| quote.price) else {
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: shared("当前持仓行情缺失、过期或时间未知，暂不生成基于现价的建议。"),
                source: AiSource::Local,
                note: None,
            };
            cx.notify();
            return;
        };
        let quote_note = quote.map(|quote| {
            shared(format!(
                "行情快照：{} · {} · {}",
                quote.source,
                quote.freshness_label(),
                quote.as_of_label()
            ))
        });

        if let Some(hit) = self.portfolio_ai_cache.get(&cache_key).cloned() {
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: hit.text.into(),
                source: hit.source,
                note: quote_note.clone(),
            };
            self.portfolio_ai_key = Some(cache_key);
            cx.notify();
            return;
        }

        let Some(mut snap) = ai::build_position_advice(
            self.current_daily_candles(),
            &code,
            &name,
            shares,
            avg_cost,
            last,
            realized,
        ) else {
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: shared("数据不足：需要至少 20 根有效日 K。"),
                source: AiSource::Local,
                note: None,
            };
            self.portfolio_ai_key = Some(cache_key);
            cx.notify();
            return;
        };
        snap.review = self.position_review_view_model();

        let local = ai::local_position_advice(&snap);
        super::types::insert_ai_cache(
            &mut self.portfolio_ai_cache,
            cache_key.clone(),
            AiCacheEntry {
                text: local.clone(),
                source: AiSource::Local,
            },
        );
        self.portfolio_ai_key = Some(cache_key.clone());

        if !self.ai_config.enabled {
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: local.into(),
                source: AiSource::Local,
                note: quote_note.clone(),
            };
            cx.notify();
            return;
        }

        self.portfolio_ai_panel = AiPanelState::Loading {
            text: local.clone().into(),
        };
        let cfg = self.ai_config.clone();
        let source_label = cfg.source_label();
        cx.spawn(async move |this, cx| {
            let res = smol::unblock(move || ai::llm_position_advice(&cfg, &snap)).await;
            let _ = this.update(cx, |app, cx| {
                if app.portfolio_ai_gen != req_id
                    || app.portfolio_advice_cache_key().as_ref() != Some(&cache_key)
                {
                    return;
                }
                match res {
                    Ok(text) if !text.trim().is_empty() => {
                        let source = AiSource::Llm {
                            label: source_label.clone(),
                        };
                        super::types::insert_ai_cache(
                            &mut app.portfolio_ai_cache,
                            cache_key.clone(),
                            AiCacheEntry {
                                text: text.clone(),
                                source: source.clone(),
                            },
                        );
                        app.portfolio_ai_panel = AiPanelState::Ready {
                            text: text.into(),
                            source,
                            note: quote_note.clone(),
                        };
                    }
                    Ok(_) => {
                        app.portfolio_ai_panel = AiPanelState::Ready {
                            text: local.clone().into(),
                            source: AiSource::Local,
                            note: Some(shared("LLM 返回了空内容")),
                        };
                    }
                    Err(e) => {
                        app.portfolio_ai_panel = AiPanelState::Ready {
                            text: local.clone().into(),
                            source: AiSource::Local,
                            note: Some(shared(format!("LLM 请求失败：{e}"))),
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

fn persistence_label(slot: storage::Slot) -> &'static str {
    match slot {
        storage::Slot::Config => "配置",
        storage::Slot::Credential => "API Key",
        storage::Slot::Portfolio => "持仓",
        storage::Slot::Journal => "日记",
        storage::Slot::Treasure => "机会缓存",
        storage::Slot::Radar => "扫描缓存",
        storage::Slot::Recovery => "数据恢复",
    }
}

impl StockApp {
    pub(crate) fn confirm_financial_recovery(
        &mut self,
        slot: storage::Slot,
        backup: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if self.recovery_busy {
            return;
        }
        if !matches!(slot, storage::Slot::Portfolio | storage::Slot::Journal) {
            return;
        }
        let selection = (slot, backup.clone());
        if self.recovery_confirm.as_ref() != Some(&selection) {
            self.recovery_confirm = Some(selection);
            self.status =
                shared("请再次点击确认：将先永久保留原文件副本，再恢复所选备份或重置为空");
            cx.notify();
            return;
        }
        self.recovery_confirm = None;
        self.recovery_busy = true;
        self.status = shared("正在校验和恢复本地数据；完成前保持只读…");
        let path = if slot == storage::Slot::Portfolio {
            storage::portfolio_path()
        } else {
            storage::journal_path()
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        storage::persistence_worker().submit(storage::Slot::Recovery, move || {
            use crate::infrastructure::storage::recovery;
            let result = if slot == storage::Slot::Portfolio {
                match backup {
                    Some(backup) => {
                        recovery::restore::<crate::data::portfolio::Portfolio>(&path, &backup)
                    }
                    None => recovery::reset::<crate::data::portfolio::Portfolio>(&path),
                }
                .map(RecoveredDocument::Portfolio)
            } else {
                match backup {
                    Some(backup) => {
                        recovery::restore::<crate::data::journal::Journal>(&path, &backup)
                    }
                    None => recovery::reset::<crate::data::journal::Journal>(&path),
                }
                .map(RecoveredDocument::Journal)
            };
            let error = result.as_ref().err().map(|error| format!("{error:#}"));
            let _ = sender.send(result.map_err(|error| format!("{error:#}")));
            match error {
                Some(error) => Err(anyhow::anyhow!(error)),
                None => Ok(()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || receiver.recv()).await;
            let _ = this.update(cx, |app, cx| {
                app.recovery_busy = false;
                app.portfolio_ai_gen = app.portfolio_ai_gen.wrapping_add(1);
                app.portfolio_ai_key = None;
                app.portfolio_ai_cache.clear();
                match result {
                    Ok(Ok(RecoveredDocument::Portfolio(value))) => {
                        storage::persistence_worker()
                            .clear_recovered_error(storage::Slot::Portfolio);
                        app.portfolio = value;
                        app.portfolio_recovery = None;
                        app.trade_form = None;
                        app.trade_feedback = None;
                        app.status = shared("持仓恢复完成；恢复前原文件已保留为独立副本");
                    }
                    Ok(Ok(RecoveredDocument::Journal(value))) => {
                        storage::persistence_worker().clear_recovered_error(storage::Slot::Journal);
                        app.journal = value;
                        app.journal_recovery = None;
                        app.status = shared("日记恢复完成；恢复前原文件已保留为独立副本");
                    }
                    Ok(Err(error)) => app.status = shared(format!("恢复失败，仍保持只读：{error}")),
                    Err(_) => app.status = shared("恢复任务未完成，仍保持只读"),
                }
                app.sync_status_bar();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

enum RecoveredDocument {
    Portfolio(crate::data::portfolio::Portfolio),
    Journal(crate::data::journal::Journal),
}

impl StockApp {
    pub(crate) fn retry_persistence(&mut self) {
        let states = storage::persistence_worker().status();
        self.persist();
        if matches!(
            states.get(&storage::Slot::Portfolio),
            Some(storage::SaveState::Failed(_, _))
        ) {
            self.persist_portfolio();
        }
        if matches!(
            states.get(&storage::Slot::Journal),
            Some(storage::SaveState::Failed(_, _))
        ) {
            self.persist_journal();
        }
    }

    pub(crate) fn request_safe_quit(&mut self, cx: &mut Context<Self>) {
        self.request_safe_close(None, cx);
    }

    pub(crate) fn request_safe_close(
        &mut self,
        window: Option<gpui::AnyWindowHandle>,
        cx: &mut Context<Self>,
    ) {
        if self.update_installing
            || (matches!(
                self.update_state,
                crate::update::UpdateState::Downloading(_)
            ) && self.update_relaunch.is_none())
        {
            self.status = shared("安全更新正在执行，请等待安装完成后再退出");
            cx.notify();
            return;
        }
        if self.shutdown_pending {
            return;
        }
        self.shutdown_pending = true;
        self.retry_persistence();
        let generation = self.persist_gen;
        self.status = shared("正在保存更改，完成后退出…");
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (result, completed) = smol::unblock(|| {
                let result = storage::persistence_worker().flush();
                (result, storage::persistence_worker().status())
            })
            .await;
            let _ = this.update(cx, |app, cx| {
                app.shutdown_pending = false;
                match result {
                    Err(error) => {
                        app.observe_persistence(cx);
                        app.status = shared(format!(
                            "尚有更改未保存，已取消退出：{error:#}。请检查磁盘/凭据库后重试。"
                        ));
                        cx.notify();
                    }
                    Ok(()) => {
                        if app.update_installing
                            || (matches!(
                                app.update_state,
                                crate::update::UpdateState::Downloading(_)
                            ) && app.update_relaunch.is_none())
                        {
                            app.status = shared("安全更新正在执行，已暂停退出，等待安装完成");
                            cx.notify();
                            return;
                        }
                        if app.persist_gen != generation
                            || completed
                                .values()
                                .any(|state| matches!(state, storage::SaveState::Pending(_)))
                            || storage::persistence_worker().status() != completed
                        {
                            app.request_safe_close(window, cx);
                            return;
                        }
                        if let Some(path) = app.update_relaunch.as_ref()
                            && let Err(error) = crate::update::relaunch(path)
                        {
                            app.status = shared(format!(
                                "更新已安装，但重新启动失败：{error}。当前窗口仍保持打开。"
                            ));
                            cx.notify();
                            return;
                        }
                        app.shutdown_complete = true;
                        if let Some(window) = window {
                            let _ = window.update(cx, |_, window, _cx| window.remove_window());
                        } else {
                            #[cfg(target_os = "macos")]
                            crate::mac_status_bar::allow_termination();
                            cx.quit();
                        }
                    }
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod advice_identity_tests {
    use super::portfolio_advice_key;
    use crate::domain::market::{
        Availability, Freshness, Market, QuoteRecord, parse_market_timestamp,
    };
    use crate::domain::money::Currency;
    use crate::model::Candle;

    fn fixtures() -> (Vec<Candle>, QuoteRecord, i64) {
        let time = "2026-10-09 10:00:00";
        let now = parse_market_timestamp(time).unwrap();
        let daily = vec![Candle {
            date: "2026-10-09".into(),
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 100,
        }];
        let quote = QuoteRecord {
            code: "600519".into(),
            market: Market::AShare,
            currency: Currency::Cny,
            name: "Fixture".into(),
            price: Some(12.0),
            change_pct: Some(1.0),
            volume: Some(100),
            source: "fixture".into(),
            fetched_at: now,
            market_time: Some(time.into()),
            availability: Availability::Available,
            freshness: Freshness::Live,
        };
        (daily, quote, now)
    }

    #[test]
    fn advice_key_rejects_empty_daily_evidence_and_tracks_instrument_and_daily_revisions() {
        let (mut daily, quote, now) = fixtures();
        assert!(portfolio_advice_key("600519", &[], None, Some(&quote), now).is_none());
        let original = portfolio_advice_key("600519", &daily, None, Some(&quote), now);
        assert_ne!(
            original,
            portfolio_advice_key("000001", &daily, None, Some(&quote), now)
        );
        daily[0].high = 13.0;
        assert_ne!(
            original,
            portfolio_advice_key("600519", &daily, None, Some(&quote), now)
        );
    }

    #[test]
    fn advice_key_changes_with_price_or_staleness_but_not_flat_fetch_time() {
        let (daily, mut quote, now) = fixtures();
        let original = portfolio_advice_key("600519", &daily, None, Some(&quote), now);
        quote.fetched_at += 1000;
        quote.market_time = Some("2026-10-09 10:00:01".into());
        assert_eq!(
            original,
            portfolio_advice_key("600519", &daily, None, Some(&quote), now + 1000)
        );
        assert_ne!(
            original,
            portfolio_advice_key("600519", &daily, None, Some(&quote), now + 400_000)
        );
        quote.price = Some(13.0);
        assert_ne!(
            original,
            portfolio_advice_key("600519", &daily, None, Some(&quote), now + 1000)
        );
    }
}
