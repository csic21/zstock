//! Exercise compact surfaces against canonical records rather than Symbol caches.
use super::{StockApp, layout_regression_tests::test_window};
use crate::domain::market::{Availability, Freshness, Market, QuoteRecord};
use crate::domain::money::Currency;
use crate::model::shared;
use gpui::{AppContext, TestAppContext, VisualContext};

#[gpui::test]
fn work_status_keeps_stale_evidence_time_instead_of_current_clock(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 1320.0, 860.0);
    let handle = window.window_handle();
    window
        .cx
        .update_window(handle, |view, _window, cx| {
            view.downcast::<StockApp>().unwrap().update(cx, |app, _cx| {
                app.selected = shared("600519");
                app.loading = false;
                app.treasure_scanning = false;
                app.quote_fail_streak = 0;
                app.services.market.quotes_by_code.clear();
                assert!(app.work_status_line().contains("unavailable"));
                let quote = QuoteRecord {
                    code: "600519".into(),
                    market: Market::AShare,
                    currency: Currency::Cny,
                    name: "fixture".into(),
                    price: Some(88.0),
                    change_pct: Some(1.0),
                    volume: Some(1),
                    source: "fixture-source".into(),
                    fetched_at: chrono::Utc::now().timestamp_millis(),
                    market_time: Some("2000-01-01 09:30:00".into()),
                    availability: Availability::Available,
                    freshness: Freshness::Live,
                };
                app.services
                    .market
                    .quotes_by_code
                    .insert("600519".into(), quote);
                let line = app.work_status_line();
                assert!(line.contains("stale"));
                assert!(line.contains("2000-01-01 09:30:00"));
                assert!(!line.contains("sync ok"));
                #[cfg(target_os = "macos")]
                {
                    let symbol = crate::model::Symbol {
                        code: "600519".into(),
                        name: shared("fixture"),
                        last: 9999.0,
                        change_pct: 99.0,
                        volume: 999,
                        board: crate::model::board_for_code("600519"),
                    };
                    for work in [false, true] {
                        app.work_mode = work;
                        for label in [
                            app.status_bar_title_for(&symbol),
                            app.status_bar_compact_for(&symbol),
                            app.status_bar_menu_label_for(&symbol),
                        ] {
                            assert!(label.contains("88"));
                            assert!(!label.contains("9999"));
                            assert!(label.contains("2000-01-01 09:30:00"));
                            assert!(label.contains(if work { "stale" } else { "过期" }));
                        }
                    }
                }
                app.services
                    .market
                    .quotes_by_code
                    .get_mut("600519")
                    .unwrap()
                    .market_time = None;
                assert!(app.work_status_line().contains("time unknown"));
            });
        })
        .unwrap();
}

fn quote_fixture() -> QuoteRecord {
    QuoteRecord {
        code: "600519".into(),
        market: Market::AShare,
        currency: Currency::Cny,
        name: "fixture".into(),
        price: Some(88.0),
        change_pct: Some(0.0),
        volume: None,
        source: "fixture-source".into(),
        fetched_at: 0,
        market_time: Some("2026-10-10 10:00:00".into()),
        availability: Availability::Available,
        freshness: Freshness::Live,
    }
}

#[test]
fn compact_quote_text_distinguishes_missing_from_real_zero_and_stale() {
    use super::helpers::palette_quote_text;
    let now = crate::domain::market::parse_market_timestamp("2026-10-10 10:00:10").unwrap();
    for work in [false, true] {
        let missing = palette_quote_text(None, work, now);
        assert_eq!(missing.price, "—");
        assert_eq!(missing.change, "—");
        assert_eq!(missing.up, None);
        assert!(
            missing
                .status
                .contains(if work { "unavailable" } else { "缺失" })
        );
        let mut quote = quote_fixture();
        let current = palette_quote_text(Some(&quote), work, now);
        assert!(current.price.starts_with("88"));
        assert_eq!(current.change, if work { "+0.00" } else { "+0.00%" });
        assert_eq!(current.up, Some(true));
        assert!(
            current
                .status
                .starts_with(if work { "live" } else { "实时" })
        );
        quote.change_pct = None;
        let unknown_change = palette_quote_text(Some(&quote), work, now);
        assert_eq!(unknown_change.change, "—");
        assert_eq!(unknown_change.up, None);
        assert!(unknown_change.price.starts_with("88"));
        quote.change_pct = Some(-2.5);
        let stale = palette_quote_text(Some(&quote), work, now + 600_000);
        assert!(
            stale
                .status
                .starts_with(if work { "stale" } else { "过期" })
        );
        assert!(stale.status.contains("2026-10-10 10:00:00"));
        assert!(stale.change.starts_with("-2.50"));
        quote.market_time = None;
        assert!(
            palette_quote_text(Some(&quote), work, now)
                .status
                .contains(if work { "time unknown" } else { "未知" })
        );
        quote.price = Some(0.0); // A zero stock price violates the canonical market contract.
        let invalid = palette_quote_text(Some(&quote), work, now);
        assert_eq!(invalid.price, "—");
        assert_eq!(invalid.change, "—");
        assert_eq!(invalid.up, None);
        assert!(
            invalid
                .status
                .starts_with(if work { "unavailable" } else { "缺失" })
        );
    }
}

#[test]
fn chart_change_needs_a_known_positive_previous_close() {
    use super::helpers::change_from_previous_close;
    assert_eq!(change_from_previous_close(88.0, 0.0), "—");
    assert_eq!(change_from_previous_close(88.0, f64::NAN), "—");
    assert_eq!(change_from_previous_close(0.0, 88.0), "—");
    assert_eq!(change_from_previous_close(88.0, 88.0), "+0.00%");
    assert_eq!(change_from_previous_close(90.0, 100.0), "-10.00%");
}

#[gpui::test]
fn missing_index_change_clears_previous_snapshot(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 800.0, 600.0);
    let handle = window.window_handle();
    window
        .cx
        .update_window(handle, |view, _window, cx| {
            view.downcast::<StockApp>().unwrap().update(cx, |app, _cx| {
                let tick = |change| vec![("000300".into(), "沪深300".into(), 3900.0, change)];
                assert!(app.apply_index_ticks(&tick(Some(0.0))));
                assert_eq!(app.index_hs300.as_ref().unwrap().change_pct, 0.0);
                assert!(app.apply_index_ticks(&tick(None)));
                assert!(app.index_hs300.is_none());
                assert!(!app.apply_index_ticks(&tick(None)));
            });
        })
        .unwrap();
}
