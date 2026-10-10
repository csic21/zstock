//! Recovery availability is a property of each source, never an empty balance.
//! Every test uses the existing isolated fixture harness and dummy financial data.

use gpui::{AppContext, Context, TestAppContext, VisualContext, VisualTestContext, Window};
use gpui_component::PixelsExt;

use super::{StockApp, layout_regression_tests::test_window};
use crate::domain::climate::NewEntryStance;
use crate::domain::decision::{DecisionOutcome, DecisionStepState};
use crate::domain::market::{Availability, Freshness, Market, QuoteRecord};
use crate::domain::money::Currency;
use crate::domain::position_sizing::PositionSizingError;
use crate::model::shared;
use crate::storage::RecoveryState;

fn update_app<R>(
    window: &mut VisualTestContext,
    update: impl FnOnce(&mut StockApp, &mut Window, &mut Context<StockApp>) -> R,
) -> R {
    let handle = window.window_handle();
    window
        .cx
        .update_window(handle, |view, window, cx| {
            view.downcast::<StockApp>()
                .unwrap()
                .update(cx, |app, cx| update(app, window, cx))
        })
        .unwrap()
}

fn seed(app: &mut StockApp) {
    app.ui_state.primary_task = super::state::PrimaryTask::Today;
    app.selected = shared("600519");
    app.loading = false;
    app.treasure_scanning = false;
    app.portfolio = serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "trades": [{"id":"fixture-trade","code":"600519","name":"fixture","side":"buy",
            "currency":"CNY","shares":100.0,"price":10.0,"fee":0.0,"time":"2026-01-01 09:30:00"}],
        "cash_balances": {"CNY":{"currency":"CNY","minor":123456}},
        "track_cash": true
    }))
    .unwrap();
    app.journal = serde_json::from_value(serde_json::json!({
        "schema_version":1,
        "entries":[{"id":"fixture-plan","code":"600519","name":"fixture","kind":"manual",
            "price":10.0,"target":12.0,"note":"fixture","created_at":"2026-01-01 09:30:00",
            "plan":{"id":"fixture-plan","code":"600519","created_on":"2026-01-01",
                "review_on":"2026-01-10","trigger":"fixture","observation_range":"9-10",
                "invalidation":"8","target":"12","risk_amount":null,"status":"due_for_review",
                "evidence":{"strategy_version":"fixture","data_as_of":"2026-01-01","source":"fixture","payload_json":"{}"},
                "executed":null,"exit_reason":null,"followed_plan":null},
            "outcomes":[{"plan_id":"fixture-plan","horizon_days":10,"return_pct":2.0,
                "maximum_favorable_excursion_pct":3.0,"maximum_adverse_excursion_pct":-1.0}]}]
    })).unwrap();
    let now = chrono::Utc::now();
    app.services.market.quotes_by_code.insert(
        "600519".into(),
        QuoteRecord {
            code: "600519".into(),
            market: Market::AShare,
            currency: Currency::Cny,
            name: "fixture".into(),
            price: Some(11.0),
            change_pct: Some(1.0),
            volume: Some(100),
            source: "fixture-quotes".into(),
            fetched_at: now.timestamp_millis(),
            market_time: Some(
                now.with_timezone(&crate::domain::market::exchange_offset())
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string(),
            ),
            availability: Availability::Available,
            freshness: Freshness::Live,
        },
    );
}

fn recovery() -> RecoveryState {
    RecoveryState {
        message: "fixture unreadable source".into(),
        backups: Vec::new(),
    }
}

fn assert_unavailable(app: &StockApp, cx: &Context<StockApp>, portfolio: bool, journal: bool) {
    assert!(app.financial_recovery_required());
    assert_eq!(app.recovery_banner_height().as_f32(), 56.0);
    assert!(app.work_status_line().contains("data recovery required"));
    assert!(app.today_dashboard_view_model().is_none());
    assert_eq!(app.rule_ledger_view_model().is_none(), journal);
    assert_eq!(
        app.position_sizing_plan(cx),
        Err(PositionSizingError::FinancialDataUnavailable)
    );
    assert!(app.position_review_view_model().is_none());
    assert!(app.champion_stock_plans(cx).1.is_empty());
    let climate = app.market_climate_report();
    assert_eq!(climate.stance, NewEntryStance::Freeze);
    assert_eq!(climate.risk_scale, 0.0);
    assert!(climate.detail.contains("不可用"));
    let trace = app.decision_trace_view_model(cx);
    assert_ne!(trace.outcome, DecisionOutcome::PlanReady);
    let final_step = trace
        .steps
        .iter()
        .find(|step| step.title == "最终动作")
        .unwrap();
    assert_eq!(final_step.state, DecisionStepState::Blocked);
    assert!(final_step.summary.contains("恢复模式"));
    // Even a decoded-but-untrusted source must not feed a zero/all-clear view.
    assert_eq!(app.portfolio_recovery.is_some(), portfolio);
    assert_eq!(app.journal_recovery.is_some(), journal);
    assert_eq!(app.quote_for_code("600519").unwrap().price, Some(11.0));
    assert!(app.quote_for_code("600519").unwrap().usable());
}

fn check_mode(cx: &mut TestAppContext, portfolio: bool, journal: bool) {
    let mut window = test_window(cx, 1320.0, 860.0);
    update_app(&mut window, |app, _window, cx| {
        seed(app);
        app.portfolio_recovery = portfolio.then(recovery);
        app.journal_recovery = journal.then(recovery);
        assert_unavailable(app, cx, portfolio, journal);
        // Empty render fallbacks obey the same unavailable contract.
        if portfolio {
            app.portfolio = Default::default();
        }
        if journal {
            app.journal = Default::default();
        }
        assert_unavailable(app, cx, portfolio, journal);
        cx.notify();
    });
    window.run_until_parked();
    assert!(window.debug_bounds("financial-recovery-banner").is_some());
    assert!(window.debug_bounds("today-recovery-unavailable").is_some());
    assert!(window.debug_bounds("today-dashboard-root").is_none());
}

#[gpui::test]
fn portfolio_only_recovery_does_not_become_zero_holdings(cx: &mut TestAppContext) {
    check_mode(cx, true, false);
}

#[gpui::test]
fn journal_only_recovery_does_not_become_no_due_reviews(cx: &mut TestAppContext) {
    check_mode(cx, false, true);
}

#[gpui::test]
fn both_recovery_sources_block_personal_allocation_but_keep_quotes(cx: &mut TestAppContext) {
    check_mode(cx, true, true);
}

#[gpui::test]
fn completed_recovery_restores_projections_and_removes_warning(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 1320.0, 860.0);
    update_app(&mut window, |app, _window, cx| {
        seed(app);
        app.portfolio_recovery = Some(recovery());
        app.journal_recovery = Some(recovery());
        assert_unavailable(app, cx, true, true);
        cx.notify();
    });
    window.run_until_parked();
    assert!(window.debug_bounds("financial-recovery-banner").is_some());
    assert!(window.debug_bounds("today-recovery-unavailable").is_some());
    assert!(window.debug_bounds("today-dashboard-root").is_none());
    update_app(&mut window, |app, _window, cx| {
        // Successful validated recovery installs these values then clears locks.
        app.portfolio_recovery = None;
        app.journal_recovery = None;
        app.recovery_busy = false;
        assert!(!app.financial_recovery_required());
        assert_eq!(app.recovery_banner_height().as_f32(), 0.0);
        assert!(!app.work_status_line().contains("data recovery required"));
        let dashboard = app.today_dashboard_view_model().unwrap();
        assert_eq!(dashboard.open_positions, 1);
        assert_eq!(dashboard.due_reviews, 1);
        assert_eq!(app.rule_ledger_view_model().unwrap().sample, 1);
        assert_ne!(
            app.position_sizing_plan(cx),
            Err(PositionSizingError::FinancialDataUnavailable)
        );
        assert_eq!(app.quote_for_code("600519").unwrap().price, Some(11.0));
        cx.notify();
    });
    window.run_until_parked();
    assert!(window.debug_bounds("financial-recovery-banner").is_none());
    assert!(window.debug_bounds("today-recovery-unavailable").is_none());
    assert!(window.debug_bounds("today-dashboard-root").is_some());
}
