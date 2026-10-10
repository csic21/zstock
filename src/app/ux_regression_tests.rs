//! Synchronous, fixture-only regression coverage for high-frequency navigation.
//! The shared test harness isolates storage. These tests do not await network
//! responses or advance the background executor; all assertions use seeded state.

use gpui::VisualContext;
use gpui::{AppContext, Context, TestAppContext, VisualTestContext, Window};
use gpui_component::{PixelsExt, Root};

use super::layout_regression_tests::test_window as unwrapped_test_window;
use super::state::PrimaryTask;
use super::{ChartKind, ChartRange, DetailTab, StockApp};
use crate::controller::state::RequestState;
use crate::data::alerts::{BuyAlert, BuyAlertBasis};
use crate::data::groups::WatchTag;
use crate::data::limitup::{LimitUpHit, LimitVerdict};
use crate::data::radar::{RadarHit, RadarStrategy};
use crate::data::scout::{ScoutPick, ScoutVerdict};
use crate::data::treasure::TreasureHit;
use crate::model::{MinutePeriod, Symbol, TrendLine, board_for_code, shared};

/// The older layout fixture uses StockApp directly as its root. Input focus and
/// component overlays require the same Root wrapper used in the production app.
fn test_window(cx: &mut TestAppContext, width: f32, height: f32) -> VisualTestContext {
    let mut window = unwrapped_test_window(cx, width, height);
    let handle = window.window_handle();
    window
        .cx
        .update_window(handle, |view, window, cx| {
            window.replace_root(cx, |window, cx| Root::new(view, window, cx));
        })
        .expect("install component root");
    window
}

fn symbol(code: &str) -> Symbol {
    Symbol {
        code: code.into(),
        name: shared(format!("fixture-{code}")),
        last: 10.0,
        change_pct: 1.0,
        volume: 100,
        board: board_for_code(code),
    }
}

fn update_app<R>(
    window: &mut VisualTestContext,
    update: impl FnOnce(&mut StockApp, &mut Window, &mut Context<StockApp>) -> R,
) -> R {
    let handle = window.window_handle();
    window
        .cx
        .update_window(handle, |view, window, cx| {
            let app = view
                .downcast::<Root>()
                .expect("component root")
                .read(cx)
                .view()
                .clone()
                .downcast::<StockApp>()
                .expect("StockApp content");
            app.update(cx, |app, cx| update(app, window, cx))
        })
        .expect("update fixture window")
}

fn set_layout_preferences(app: &mut StockApp) {
    app.detail_tab = DetailTab::Indicators;
    app.range = ChartRange::Y1;
    app.chart_kind = ChartKind::MinuteK(MinutePeriod::M15);
    app.bottom_height = 157.0;
    app.dock.main_v = vec![333.0, 157.0];
}

fn assert_layout_preferences(app: &StockApp) {
    assert_eq!(app.detail_tab, DetailTab::Indicators);
    assert_eq!(app.range, ChartRange::Y1);
    assert_eq!(app.chart_kind, ChartKind::MinuteK(MinutePeriod::M15));
    assert_eq!(app.bottom_height, 157.0);
    assert_eq!(app.dock.main_v, vec![333.0, 157.0]);
}

#[gpui::test]
fn ordinary_selection_preserves_analysis_layout(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 920.0, 580.0);
    update_app(&mut window, |app, _window, cx| {
        app.symbols = vec![symbol("600519"), symbol("00700")];
        app.selected = shared("600519");
        set_layout_preferences(app);

        app.select_symbol(shared("600519"), cx);
        assert_layout_preferences(app);
        app.select_symbol(shared("00700"), cx);
        assert_eq!(app.selected.as_ref(), "00700");
        assert_layout_preferences(app);
    });
}

#[gpui::test]
fn candidate_preview_keeps_membership_tags_and_layout(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 920.0, 580.0);
    update_app(&mut window, |app, _window, cx| {
        app.symbols = vec![symbol("600519")];
        // Same selected code avoids data-fetch work; each old candidate handler
        // still used to save/tag this code and reset its layout in this case.
        app.selected = shared("00700");
        app.watch_tags.clear();
        app.watch_tags.insert("600519".into(), WatchTag::Watch);
        set_layout_preferences(app);
        let membership = serde_json::to_value(&app.symbols).unwrap();
        let tags = app.watch_tags.clone();

        app.select_treasure_hit(&treasure_fixture(), cx);
        assert_layout_preferences(app);
        assert_eq!(serde_json::to_value(&app.symbols).unwrap(), membership);
        assert_eq!(app.watch_tags, tags);
        app.select_scout_pick(&scout_fixture(), cx);
        assert_layout_preferences(app);
        assert_eq!(serde_json::to_value(&app.symbols).unwrap(), membership);
        assert_eq!(app.watch_tags, tags);
        app.select_radar_hit(&radar_fixture(), cx);
        assert_layout_preferences(app);
        assert_eq!(serde_json::to_value(&app.symbols).unwrap(), membership);
        assert_eq!(app.watch_tags, tags);
        app.select_limitup_hit(&limitup_fixture(), cx);
        assert_layout_preferences(app);
        assert_eq!(serde_json::to_value(&app.symbols).unwrap(), membership);
        assert_eq!(app.watch_tags, tags);
        assert_eq!(app.current_symbol().unwrap().code, "00700");

        app.add_pick_to_group("00700", "fixture", 10.0, WatchTag::Long, cx);
        assert_eq!(app.symbols.len(), 2);
        assert_eq!(app.tag_for("00700"), WatchTag::Long);
    });
}

#[gpui::test]
fn removing_last_membership_retains_research_data_and_undo(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 720.0, 440.0);
    update_app(&mut window, |app, _window, cx| {
        let code = "00700";
        app.symbols = vec![symbol(code)];
        app.selected = shared(code);
        app.buy_alerts.clear();
        app.buy_alerts
            .insert(code.into(), BuyAlert::new(9.0, BuyAlertBasis::Manual));
        app.chart_lines.clear();
        app.chart_lines
            .insert(code.into(), vec![TrendLine::price_line(0, 10, 9.5, 1)]);
        app.work_aliases.clear();
        app.work_aliases
            .insert(code.into(), "fixture-service".into());
        app.watch_tags.clear();
        app.watch_tags.insert(code.into(), WatchTag::Long);
        app.status_bar_enabled = true;
        app.status_bar_codes = vec![code.into()];
        app.status_bar_active = code.into();
        let alerts = serde_json::to_value(&app.buy_alerts).unwrap();
        let lines = serde_json::to_value(&app.chart_lines).unwrap();
        let aliases = app.work_aliases.clone();
        let tags = app.watch_tags.clone();

        app.remove_symbol(code, cx);
        assert!(app.symbols.is_empty());
        assert_eq!(app.selected.as_ref(), code);
        assert_eq!(app.current_symbol().unwrap().code, code);
        assert_eq!(serde_json::to_value(&app.buy_alerts).unwrap(), alerts);
        assert_eq!(serde_json::to_value(&app.chart_lines).unwrap(), lines);
        assert_eq!(app.work_aliases, aliases);
        assert_eq!(app.watch_tags, tags);
        assert!(app.status_bar_codes.is_empty());
        assert!(app.watchlist_undo.is_some());
        app.select_adjacent_symbol(1, cx);
        assert_eq!(app.selected.as_ref(), code);

        // Explicitly verify the persisted empty list, only inside the harness's
        // isolated temporary directory. Never resolve a developer's real files.
        let fixture_dir = crate::infrastructure::storage::paths::app_data_dir();
        assert!(fixture_dir.starts_with(std::env::temp_dir()));
        app.persist();
        crate::storage::persistence_worker().flush().unwrap();
        let saved = crate::storage::load_config();
        assert!(saved.watchlist.is_empty());
        assert_eq!(serde_json::to_value(saved.buy_alerts).unwrap(), alerts);
        assert_eq!(serde_json::to_value(saved.chart_lines).unwrap(), lines);
        assert_eq!(saved.work_aliases, aliases);
        assert_eq!(saved.watch_tags.get(code).map(String::as_str), Some("long"));

        app.undo_remove_symbol(cx);
        assert_eq!(app.symbols.len(), 1);
        assert_eq!(app.symbols[0].code, code);
        assert_eq!(app.status_bar_codes, vec![code.to_string()]);
        assert_eq!(app.status_bar_active, code);
        assert!(app.watchlist_undo.is_none());
        assert_eq!(serde_json::to_value(&app.buy_alerts).unwrap(), alerts);
        assert_eq!(serde_json::to_value(&app.chart_lines).unwrap(), lines);
    });
}

#[gpui::test]
fn global_palette_confirm_reveals_research_without_saving(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 720.0, 440.0);
    update_app(&mut window, |app, window, cx| {
        app.symbols = vec![symbol("600519")];
        app.selected = shared("00700");
        // Input is fixture-controlled; prevent queued Change events from
        // replacing seeded search results or starting a provider request.
        app._subscriptions.clear();
        app.palette_query
            .update(cx, |input, cx| input.set_value("", window, cx));
        let membership = serde_json::to_value(&app.symbols).unwrap();
        for (task, settings, market) in [
            (PrimaryTask::Today, false, false),
            (PrimaryTask::StrategyLab, false, false),
            (PrimaryTask::Research, true, false),
            (PrimaryTask::Today, false, true),
        ] {
            set_layout_preferences(app);
            app.ui_state.primary_task = task;
            app.settings_open = settings;
            app.market_analysis_open = market;
            app.market_heatmap_fullscreen = market;
            app.palette_open = true;
            app.filtered_local.clear();
            app.palette_hits = vec![symbol("00700")];
            app.palette_index = 0;
            app.palette_search.state = RequestState::Ready(());

            app.palette_confirm(window, cx);
            assert_eq!(app.ui_state.primary_task, PrimaryTask::Research);
            assert_eq!(app.selected.as_ref(), "00700");
            assert!(!app.settings_open && !app.market_analysis_open);
            assert!(!app.market_heatmap_fullscreen && !app.palette_open);
            assert_eq!(serde_json::to_value(&app.symbols).unwrap(), membership);
            assert_layout_preferences(app);
        }
    });
}

#[gpui::test]
fn palette_is_visible_and_bounded_over_compact_settings_and_market(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 720.0, 440.0);
    for settings in [true, false] {
        update_app(&mut window, |app, _window, cx| {
            app.settings_open = settings;
            app.market_analysis_open = !settings;
            app.palette_open = true;
            app.filtered_local.clear();
            app.palette_hits.clear();
            cx.notify();
        });
        // Draw synchronously; do not run background network tasks to get a frame.
        window.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        let panel = window
            .debug_bounds("palette-panel")
            .expect("global palette is visible");
        assert!(panel.size.width.as_f32() > 100.0);
        assert!(panel.size.height.as_f32() > 80.0);
        assert!(panel.origin.x.as_f32() >= 0.0);
        assert!(panel.origin.y.as_f32() >= 0.0);
        assert!(panel.origin.x.as_f32() + panel.size.width.as_f32() <= 720.0);
        assert!(panel.origin.y.as_f32() + panel.size.height.as_f32() <= 440.0);
    }
}

#[gpui::test]
fn palette_clear_close_reopen_and_invalid_text_are_safe(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 720.0, 440.0);
    update_app(&mut window, |app, window, cx| {
        app._subscriptions.clear();
        app.symbols = vec![symbol("600519")];
        let membership = serde_json::to_value(&app.symbols).unwrap();
        app.palette_open = true;
        app.palette_hits = vec![symbol("00700")];
        let old = app.palette_search.begin("old");
        app.on_palette_query_changed("", cx);
        assert!(app.palette_hits.is_empty());
        assert!(!app.palette_search.apply(&old, ()));
        let closing = app.palette_search.begin("closing");
        app.close_palette(cx);
        assert!(!app.palette_search.fail(&closing, "late failure"));
        app.toggle_palette(window, cx);
        assert!(app.palette_open);
        assert!(app.palette_hits.is_empty());
        assert!(!app.palette_search.apply(&closing, ()));

        app.palette_query
            .update(cx, |input, cx| input.set_value("not-a-code", window, cx));
        app.filtered_local.clear();
        app.palette_search.state = RequestState::Ready(());
        app.palette_confirm(window, cx);
        assert!(app.palette_open);
        assert!(matches!(app.palette_search.state, RequestState::Failed(_)));
        assert_eq!(serde_json::to_value(&app.symbols).unwrap(), membership);
    });
}

#[gpui::test]
fn only_explicit_credential_edits_persist_through_the_test_secret_store(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 920.0, 580.0);
    update_app(&mut window, |app, window, cx| {
        assert!(
            !app.ai_api_key_dirty.get(),
            "loading config must not edit credentials"
        );
        let initial = app.ai_config.api_key.clone();
        app.ai_api_key_input
            .update(cx, |input, cx| input.set_value(initial, window, cx));
    });
    // Separate updates flush InputEvent::Change subscriptions between assertions.
    update_app(&mut window, |app, window, cx| {
        assert!(
            !app.ai_api_key_dirty.get(),
            "unchanged input must stay clean"
        );
        app.ai_api_key_input.update(cx, |input, cx| {
            input.set_value("fixture-api-key", window, cx)
        });
    });
    update_app(&mut window, |app, window, cx| {
        assert!(
            app.ai_api_key_dirty.get(),
            "explicit input change must be tracked"
        );
        let fixture_dir = crate::infrastructure::storage::paths::app_data_dir();
        assert!(fixture_dir.starts_with(std::env::temp_dir()));
        // Ordinary test builds inject an in-memory store. This cannot touch the
        // developer's native keychain, and the config remains fixture-only.
        app.persist();
        crate::storage::persistence_worker().flush().unwrap();
        app.observe_persistence(cx);
        assert!(!app.ai_api_key_dirty.get());
        assert_eq!(
            crate::storage::load_config().ai_api.api_key,
            "fixture-api-key"
        );
        app.ai_api_key_input
            .update(cx, |input, cx| input.set_value("", window, cx));
    });
    update_app(&mut window, |app, _window, cx| {
        assert!(app.ai_api_key_dirty.get(), "explicit clear must be tracked");
        app.persist();
        crate::storage::persistence_worker().flush().unwrap();
        app.observe_persistence(cx);
        assert!(!app.ai_api_key_dirty.get());
        assert!(crate::storage::load_config().ai_api.api_key.is_empty());
    });
}

#[gpui::test]
fn credential_save_failure_is_visible_and_recovers_without_losing_config(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 920.0, 580.0);
    let broken = b"{ fixture interrupted config";
    let (path, original) = update_app(&mut window, |app, window, cx| {
        assert!(!app.ai_api_key_dirty.get());
        let fixture_dir = crate::infrastructure::storage::paths::app_data_dir();
        assert!(fixture_dir.starts_with(std::env::temp_dir()));
        let path = fixture_dir.join("config.json");
        let original = std::fs::read(&path).expect("read valid fixture config");
        std::fs::write(&path, broken).expect("seed interrupted fixture config");
        app.ai_api_key_input.update(cx, |input, cx| {
            input.set_value("fixture-retry-api-key", window, cx);
        });
        (path, original)
    });
    update_app(&mut window, |app, _window, cx| {
        assert!(app.ai_api_key_dirty.get());
        app.persist();
        assert!(crate::storage::persistence_worker().flush().is_err());
        app.observe_persistence(cx);
        assert!(app.ai_api_key_dirty.get(), "failed key write stays pending");
        assert!(
            app.ai_api_key_save_error
                .as_ref()
                .is_some_and(|error| error.contains("API Key尚未保存"))
        );
        assert!(
            app.status.contains("配置尚未保存"),
            "failure must be visible during the session"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            broken,
            "failed save must preserve the interrupted original"
        );
    });

    std::fs::write(&path, original).expect("restore valid fixture config");
    update_app(&mut window, |app, window, cx| {
        app.persist();
        crate::storage::persistence_worker().flush().unwrap();
        app.observe_persistence(cx);
        assert!(!app.ai_api_key_dirty.get());
        assert!(app.ai_api_key_save_error.is_none());
        assert_eq!(
            crate::storage::load_config().ai_api.api_key,
            "fixture-retry-api-key"
        );
        app.ai_api_key_input
            .update(cx, |input, cx| input.set_value("", window, cx));
    });
    update_app(&mut window, |app, _window, cx| {
        app.persist();
        crate::storage::persistence_worker().flush().unwrap();
        app.observe_persistence(cx);
        assert!(!app.ai_api_key_dirty.get());
        assert!(app.ai_api_key_save_error.is_none());
        assert!(crate::storage::load_config().ai_api.api_key.is_empty());
    });
}

fn treasure_fixture() -> TreasureHit {
    TreasureHit {
        code: "00700".into(),
        name: "fixture".into(),
        close: 10.0,
        bars: 252,
        as_of: "2026-10-08".into(),
        pos_1y: None,
        pos_3y: None,
        pos_all: None,
        pctile_1y: None,
        pctile_3y: None,
        pctile_all: None,
        dd_1y: None,
        dd_3y: None,
        dd_all: None,
        avg_vol_20: None,
        score: 60.0,
        tags: vec![],
        source: "fixture".into(),
    }
}

fn scout_fixture() -> ScoutPick {
    ScoutPick {
        code: "00700".into(),
        name: "fixture".into(),
        treasure_score: 60.0,
        buy_score: 60.0,
        verdict: ScoutVerdict::Watch,
        close: 10.0,
        buy_low: 9.0,
        buy_high: 10.0,
        sell_low: 11.0,
        sell_high: 12.0,
        regime: "fixture".into(),
        rsi14: None,
        tags: vec![],
        reasons: vec![],
        risks: vec![],
        headline: "fixture".into(),
    }
}

fn radar_fixture() -> RadarHit {
    RadarHit {
        code: "00700".into(),
        name: "fixture".into(),
        strategy: RadarStrategy::Pullback,
        score: 60.0,
        close: 10.0,
        change_pct: 1.0,
        rsi14: None,
        volume_ratio_20: None,
        regime: "fixture".into(),
        reasons: vec![],
        risks: vec![],
        headline: "fixture".into(),
        watch_low: 9.0,
        watch_high: 10.0,
    }
}

fn limitup_fixture() -> LimitUpHit {
    LimitUpHit {
        code: "00700".into(),
        name: "fixture".into(),
        close: 10.0,
        change_pct: 1.0,
        streak: 1,
        yesterday_streak: 0,
        verdict: LimitVerdict::FirstBoard,
        score: 60.0,
        is_one_word: false,
        amount: 1_000.0,
        observation: 10.0,
        invalidation: 9.0,
        target: 11.0,
        risk_reward: Some(1.0),
        limit_pct: 0.1,
        limit_price_today: 10.0,
        reasons: vec![],
        risks: vec![],
        headline: "fixture".into(),
    }
}

#[gpui::test]
fn older_credential_completion_cannot_clear_a_newer_edit(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 920.0, 580.0);
    update_app(&mut window, |app, _window, cx| {
        let revision = crate::storage::persistence_worker()
            .submit(crate::storage::Slot::Credential, || Ok(()));
        app.ai_api_key_edit_gen = 10;
        app.ai_api_key_pending = Some((revision, 10));
        app.ai_api_key_dirty.set(true);
        // A second edit has happened but its debounce has not submitted yet.
        app.ai_api_key_edit_gen = 11;
        app.ai_config.api_key = "fixture-newer-edit".into();
        crate::storage::persistence_worker().flush().unwrap();
        app.observe_persistence(cx);
        assert!(app.ai_api_key_dirty.get());
        app.persist();
        crate::storage::persistence_worker().flush().unwrap();
        app.observe_persistence(cx);
        assert!(!app.ai_api_key_dirty.get());
        assert_eq!(
            crate::storage::load_config().ai_api.api_key,
            "fixture-newer-edit"
        );
        app.ai_api_key_edit_gen += 1;
        app.ai_api_key_dirty.set(true);
        app.ai_config.api_key.clear();
        app.persist();
        crate::storage::persistence_worker().flush().unwrap();
        app.observe_persistence(cx);
    });
}

#[gpui::test]
fn recovery_mode_blocks_financial_mutations_and_never_reports_saved(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 920.0, 580.0);
    update_app(&mut window, |app, window, cx| {
        let fixture_dir = crate::infrastructure::storage::paths::app_data_dir();
        assert!(fixture_dir.starts_with(std::env::temp_dir()));
        let portfolio_path = fixture_dir.join("portfolio.json");
        let journal_path = fixture_dir.join("journal.json");
        let original = b"interrupted fixture bytes";
        std::fs::write(&portfolio_path, original).unwrap();
        std::fs::write(&journal_path, original).unwrap();
        app.portfolio_recovery = crate::storage::load_portfolio().recovery;
        app.journal_recovery = crate::storage::load_journal().recovery;
        assert!(app.portfolio_recovery.is_some() && app.journal_recovery.is_some());
        let portfolio = serde_json::to_value(&app.portfolio).unwrap();
        let journal = serde_json::to_value(&app.journal).unwrap();
        assert!(!app.open_trade_form(crate::data::portfolio::TradeSide::Buy, window, cx));
        app.toggle_track_cash(cx);
        app.apply_portfolio_cash(cx);
        app.undo_last_trade_for_code("600519", cx);
        app.add_manual_journal_note(window, cx);
        app.record_decision_plan_from_card(cx);
        app.review_journal_plan("fixture", true, cx);
        app.remove_journal_entry("fixture", cx);
        app.persist_portfolio();
        app.persist_journal();
        crate::storage::persistence_worker().flush().unwrap();
        assert_eq!(serde_json::to_value(&app.portfolio).unwrap(), portfolio);
        assert_eq!(serde_json::to_value(&app.journal).unwrap(), journal);
        assert_eq!(std::fs::read(&portfolio_path).unwrap(), original);
        assert_eq!(std::fs::read(&journal_path).unwrap(), original);
        assert!(app.status.contains("只读恢复模式"));
    });
}

#[gpui::test]
fn close_and_quit_cannot_discard_an_active_update(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 1320.0, 860.0);
    let handle = window.window_handle();
    update_app(&mut window, |app, _, cx| {
        app.update_state = crate::update::UpdateState::Downloading("99.0.0".into());
        app.update_relaunch = None;
        app.request_safe_close(Some(handle), cx);
        assert!(!app.shutdown_pending);
        assert!(!app.shutdown_complete);
        assert!(app.status.contains("更新正在执行"));
        app.request_safe_quit(cx);
        assert!(!app.shutdown_pending);
        assert!(!app.shutdown_complete);
        app.check_for_updates(true, cx);
        assert!(matches!(
            app.update_state,
            crate::update::UpdateState::Downloading(_)
        ));
        app.update_state = crate::update::UpdateState::Error("fixture failure".into());
    });
}

#[gpui::test]
fn pending_shutdown_blocks_update_and_late_checks_cannot_hide_installation(
    cx: &mut TestAppContext,
) {
    let mut window = test_window(cx, 1320.0, 860.0);
    update_app(&mut window, |app, _, cx| {
        app.update_state = crate::update::UpdateState::Available(crate::update::test_update_info());
        app.shutdown_pending = true;
        app.start_update(cx);
        assert!(!app.update_installing);
        assert!(matches!(
            app.update_state,
            crate::update::UpdateState::Available(_)
        ));
        app.shutdown_pending = false;
        app.shutdown_complete = true;
        app.start_update(cx);
        assert!(!app.update_installing);
        app.shutdown_complete = false;
        app.update_installing = true;
        app.update_state = crate::update::UpdateState::Downloading("99.0.0".into());
        app.apply_update_check_result(Ok(Some(crate::update::test_update_info())), true);
        assert!(matches!(
            app.update_state,
            crate::update::UpdateState::Downloading(_)
        ));
        app.apply_update_check_result(Err("late fixture failure".into()), true);
        assert!(matches!(
            app.update_state,
            crate::update::UpdateState::Downloading(_)
        ));
        app.update_installing = false;
        app.update_state = crate::update::UpdateState::Idle;
    });
}
