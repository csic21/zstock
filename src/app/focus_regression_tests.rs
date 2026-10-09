//! Dispatch through the real keymap and rendered focus tree, not direct actions.
//! The fixture isolates storage; synchronous draws do not poll network tasks.

use gpui::{
    AppContext, Context, Focusable, Keystroke, TestAppContext, VisualContext, VisualTestContext,
    Window,
};
use gpui_component::Root;

use super::layout_regression_tests::test_window as unwrapped_test_window;
use super::state::PrimaryTask;
use super::{SettingsSection, StockApp, bind_app_keys};

fn test_window(cx: &mut TestAppContext) -> VisualTestContext {
    let mut window = unwrapped_test_window(cx, 1320., 860.);
    let handle = window.window_handle();
    window
        .cx
        .update_window(handle, |view, window, cx| {
            bind_app_keys(cx);
            window.replace_root(cx, |window, cx| Root::new(view, window, cx));
        })
        .expect("install production root and keymap");
    update_app(&mut window, |app, _, cx| {
        // The older layout fixture explicitly chooses Research. Normal launches
        // start on Today; do not manually repair focus in this fixture.
        app.ui_state.primary_task = PrimaryTask::Today;
        cx.notify();
    });
    draw(&mut window);
    window
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
        .expect("update app fixture")
}

fn draw(window: &mut VisualTestContext) {
    window.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
}

fn key(window: &mut VisualTestContext, keystroke: &str) {
    window.update(|window, cx| {
        window.dispatch_keystroke(Keystroke::parse(keystroke).expect("valid key"), cx);
    });
    draw(window);
}

fn shortcut(window: &mut VisualTestContext, key_name: &str) {
    let modifier = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    key(window, &format!("{modifier}-{key_name}"));
}

fn assert_app_focus(window: &mut VisualTestContext) {
    update_app(window, |app, window, _| {
        assert!(app.palette_focus.is_focused(window), "app root is focused");
        assert!(!app.app_focus_pending, "focus request was consumed once");
    });
}

#[gpui::test]
fn startup_shortcuts_dispatch_without_a_preceding_click(cx: &mut TestAppContext) {
    let mut window = test_window(cx);
    assert_app_focus(&mut window);
    shortcut(&mut window, ",");
    update_app(&mut window, |app, _, _| assert!(app.settings_open));
    shortcut(&mut window, ",");
    update_app(&mut window, |app, _, _| assert!(!app.settings_open));
    for (digit, expected) in [
        ("2", PrimaryTask::Research),
        ("3", PrimaryTask::Opportunities),
        ("4", PrimaryTask::Portfolio),
        ("1", PrimaryTask::Today),
    ] {
        shortcut(&mut window, digit);
        update_app(&mut window, |app, _, _| {
            assert_eq!(app.ui_state.primary_task, expected)
        });
        assert_app_focus(&mut window);
    }
}

#[gpui::test]
fn palette_dismissal_restores_settings_and_task_shortcuts(cx: &mut TestAppContext) {
    let mut window = test_window(cx);
    shortcut(&mut window, ",");
    for _ in 0..2 {
        shortcut(&mut window, "k");
        update_app(&mut window, |app, window, cx| {
            assert!(app.palette_open && app.settings_open);
            assert!(
                app.palette_query
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
            cx.notify();
        });
        draw(&mut window);
        update_app(&mut window, |app, window, cx| {
            assert!(
                app.palette_query
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
            );
        });
        key(&mut window, "escape");
        update_app(&mut window, |app, _, _| {
            assert!(!app.palette_open && app.settings_open);
        });
        assert_app_focus(&mut window);
    }
    key(&mut window, "escape");
    update_app(&mut window, |app, _, _| assert!(!app.settings_open));
    shortcut(&mut window, "4");
    update_app(&mut window, |app, _, _| {
        assert_eq!(app.ui_state.primary_task, PrimaryTask::Portfolio)
    });
}

#[gpui::test]
fn palette_reopened_before_paint_keeps_its_visible_input_focus(cx: &mut TestAppContext) {
    let mut window = test_window(cx);
    shortcut(&mut window, "k");
    update_app(&mut window, |app, window, cx| {
        app.close_palette(cx);
        assert!(app.app_focus_pending);
        app.toggle_palette(window, cx);
    });
    draw(&mut window);
    update_app(&mut window, |app, window, cx| {
        assert!(app.palette_open);
        assert!(!app.app_focus_pending);
        assert!(
            app.palette_query
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    key(&mut window, "escape");
    assert_app_focus(&mut window);
    shortcut(&mut window, "3");
    update_app(&mut window, |app, _, _| {
        assert_eq!(app.ui_state.primary_task, PrimaryTask::Opportunities)
    });
}

#[gpui::test]
fn visible_settings_input_keeps_edit_keys_until_its_panel_is_hidden(cx: &mut TestAppContext) {
    let mut window = test_window(cx);
    shortcut(&mut window, ",");
    update_app(&mut window, |app, _, cx| {
        app.set_settings_section(SettingsSection::Ai, cx)
    });
    draw(&mut window);
    let (selected, codes) = update_app(&mut window, |app, window, cx| {
        app.ai_model_input.update(cx, |input, cx| {
            input.set_value("abcd", window, cx);
            input.focus(window, cx);
        });
        (
            app.selected.clone(),
            app.symbols
                .iter()
                .map(|symbol| symbol.code.clone())
                .collect::<Vec<_>>(),
        )
    });
    draw(&mut window);
    for keystroke in ["left", "backspace", "delete", "0"] {
        key(&mut window, keystroke);
    }
    update_app(&mut window, |app, window, cx| {
        assert_eq!(app.ai_model_input.read(cx).value().as_ref(), "ab0");
        assert!(
            app.ai_model_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
        cx.notify();
    });
    draw(&mut window);
    for keystroke in ["up", "down"] {
        key(&mut window, keystroke);
    }
    shortcut(&mut window, "3");
    update_app(&mut window, |app, window, cx| {
        assert_eq!(
            app.ui_state.primary_task,
            PrimaryTask::Today,
            "task keys yield to Input"
        );
        assert_eq!(app.selected, selected);
        assert_eq!(
            app.symbols
                .iter()
                .map(|symbol| symbol.code.clone())
                .collect::<Vec<_>>(),
            codes
        );
        assert!(
            app.ai_model_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        );
    });
    key(&mut window, "escape");
    update_app(&mut window, |app, _, _| assert!(!app.settings_open));
    assert_app_focus(&mut window);
    shortcut(&mut window, "3");
    update_app(&mut window, |app, _, _| {
        assert_eq!(app.ui_state.primary_task, PrimaryTask::Opportunities)
    });
}

#[gpui::test]
fn changing_settings_section_or_task_releases_hidden_input_focus(cx: &mut TestAppContext) {
    let mut window = test_window(cx);
    for change_task in [false, true] {
        update_app(&mut window, |app, _, cx| {
            if !app.settings_open {
                app.toggle_settings(cx);
            }
            app.set_settings_section(SettingsSection::Ai, cx);
        });
        draw(&mut window);
        update_app(&mut window, |app, window, cx| {
            app.ai_model_input
                .update(cx, |input, cx| input.focus(window, cx));
        });
        draw(&mut window);
        update_app(&mut window, |app, _, cx| {
            if change_task {
                app.set_primary_task(PrimaryTask::Today, cx);
            } else {
                app.set_settings_section(SettingsSection::General, cx);
            }
        });
        draw(&mut window);
        assert_app_focus(&mut window);
    }
    shortcut(&mut window, "2");
    update_app(&mut window, |app, _, _| {
        assert_eq!(app.ui_state.primary_task, PrimaryTask::Research)
    });
}
