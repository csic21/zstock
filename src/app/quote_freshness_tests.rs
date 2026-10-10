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
