//! Real component asset lookups and production toolbar/settings layout.
//! Geometry tests do not assert SVG pixels: GPUI's TestAppContext deliberately
//! uses an empty asset source. Native rendering is verified separately.

use gpui::{
    AppContext, AssetSource, Context, Modifiers, TestAppContext, VisualContext, VisualTestContext,
    Window,
};
use gpui_component::{IconName, IconNamed, PixelsExt, Root, Theme, ThemeMode};

use crate::app::layout_regression_tests::test_window as unwrapped_test_window;
use crate::app::state::PrimaryTask;
use crate::app::{DetailTab, SettingsSection, StockApp, apply_zstock_theme};

#[test]
fn bundled_assets_resolve_every_toolbar_and_settings_icon() {
    let assets = gpui_component_assets::Assets;
    for icon in [
        IconName::Eye,
        IconName::Redo2,
        IconName::SquareTerminal,
        IconName::Search,
        IconName::Settings2,
        IconName::ArrowLeft,
        IconName::Check,
        IconName::WindowMinimize,
        IconName::WindowMaximize,
        IconName::WindowRestore,
        IconName::WindowClose,
    ] {
        let path = icon.path();
        let bytes = assets
            .load(path.as_ref())
            .unwrap_or_else(|error| panic!("missing bundled icon {path}: {error}"))
            .unwrap_or_else(|| panic!("empty bundled icon {path}"));
        let svg = std::str::from_utf8(&bytes).expect("SVG is UTF-8");
        assert!(svg.contains("<svg"), "{path} must resolve to SVG data");
    }
}

fn test_window(cx: &mut TestAppContext, width: f32, height: f32) -> VisualTestContext {
    let mut window = unwrapped_test_window(cx, width, height);
    let handle = window.window_handle();
    window
        .cx
        .update_window(handle, |view, window, cx| {
            Theme::change(ThemeMode::Dark, Some(window), cx);
            apply_zstock_theme(cx);
            window.replace_root(cx, |window, cx| Root::new(view, window, cx));
        })
        .expect("install production component root and theme");
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
    // Keep background network requests unpolled; these assertions need one frame.
    window.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
}

#[gpui::test]
fn toolbar_actions_fit_supported_window_widths(cx: &mut TestAppContext) {
    for (width, work) in [(1320., false), (1024., false), (800., false), (720., true)] {
        let mut window = test_window(cx, width, 860.);
        update_app(&mut window, |app, _, cx| {
            app.work_mode = work;
            app.settings_open = true;
            app.settings_section = SettingsSection::General;
            cx.notify();
        });
        draw(&mut window);
        let toolbar = window
            .debug_bounds("app-toolbar")
            .expect("production title bar");
        let actions = window
            .debug_bounds("toolbar-actions")
            .expect("action group");
        assert!(
            actions.origin.x >= toolbar.origin.x,
            "actions start inside title bar"
        );
        assert!(
            actions.right().as_f32() <= toolbar.right().as_f32() + 1.,
            "{width}px work={work}: toolbar actions must not cover native controls: {actions:?} / {toolbar:?}"
        );
        if !work {
            let tasks = window.debug_bounds("primary-task-tabs").expect("task tabs");
            assert!(
                tasks.right().as_f32() + 4. <= actions.origin.x.as_f32(),
                "{width}px: task tabs and actions must not overlap"
            );
        }
        for selector in [
            "toolbar-focus",
            "toolbar-refresh",
            "toolbar-search",
            "toolbar-settings",
        ] {
            let bounds = window
                .debug_bounds(selector)
                .expect("visible labeled action");
            assert!(
                bounds.size.height.as_f32() >= 28.,
                "{selector} hit target too short"
            );
            assert!(
                bounds.size.width.as_f32() >= 48.,
                "{selector} label collapsed"
            );
        }
    }
}

#[gpui::test]
fn settings_options_wrap_and_keep_readable_hit_targets(cx: &mut TestAppContext) {
    for width in [1320., 800., 640.] {
        let mut window = test_window(cx, width, 860.);
        update_app(&mut window, |app, _, cx| {
            app.settings_open = true;
            app.settings_section = SettingsSection::General;
            cx.notify();
        });
        draw(&mut window);
        let body = window.debug_bounds("settings-body").expect("settings body");
        for selector in [
            "settings-interval-1",
            "settings-interval-2",
            "settings-interval-3",
            "settings-interval-5",
            "settings-interval-8",
            "settings-interval-15",
            "settings-interval-30",
            "settings-interval-60",
        ] {
            let bounds = window.debug_bounds(selector).expect("interval choice");
            assert!(
                bounds.size.height.as_f32() >= 32.,
                "{selector} hit target too short"
            );
            assert!(bounds.origin.x >= body.origin.x);
            assert!(
                bounds.right().as_f32() <= body.right().as_f32() + 1.,
                "{width}px: option overflows settings body"
            );
        }
    }
}

#[gpui::test]
fn clicking_current_task_exits_settings_without_resetting_research(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 1320., 860.);
    update_app(&mut window, |app, _, cx| {
        app.ui_state.primary_task = PrimaryTask::Research;
        app.detail_tab = DetailTab::Indicators;
        app.settings_open = true;
        cx.notify();
    });
    draw(&mut window);
    let tab = window.debug_bounds("task-research").expect("Research tab");
    window.simulate_click(tab.center(), Modifiers::default());
    update_app(&mut window, |app, _, _| {
        assert!(!app.settings_open);
        assert_eq!(app.ui_state.primary_task, PrimaryTask::Research);
        assert_eq!(app.detail_tab, DetailTab::Indicators);
    });
}

#[gpui::test]
fn returning_to_research_keeps_analysis_tab_and_reselect_closes_market(cx: &mut TestAppContext) {
    let mut window = test_window(cx, 1320., 860.);
    update_app(&mut window, |app, _, cx| {
        app.ui_state.primary_task = PrimaryTask::Research;
        app.detail_tab = DetailTab::Indicators;
        app.set_primary_task(PrimaryTask::Today, cx);
        app.set_primary_task(PrimaryTask::Research, cx);
        assert_eq!(app.detail_tab, DetailTab::Indicators);
        app.market_analysis_open = true;
        app.market_heatmap_fullscreen = true;
        app.set_primary_task(PrimaryTask::Research, cx);
        assert!(!app.market_analysis_open);
        assert!(!app.market_heatmap_fullscreen);
        assert_eq!(app.detail_tab, DetailTab::Indicators);
    });
}

/// Observe the text style inherited by a real painted button child. This tests
/// the renderer path that formerly discarded hover_style.fg, not just its helper.
#[gpui::test]
fn hovered_buttons_keep_the_variant_foreground(cx: &mut TestAppContext) {
    use gpui::{
        Hsla, InteractiveElement, IntoElement, MouseMoveEvent, ParentElement, Render, Styled,
        canvas, div, hsla, point, px,
    };
    use gpui_component::{
        ActiveTheme,
        button::{Button, ButtonCustomVariant, ButtonVariant, ButtonVariants},
    };
    use std::{cell::Cell, rc::Rc};

    struct HoverProbe {
        variant: ButtonVariant,
        observed: Rc<Cell<Option<Hsla>>>,
    }

    impl Render for HoverProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let observed = self.observed.clone();
            div().size_full().p_4().child(
                Button::new("hover-probe")
                    .with_variant(self.variant)
                    .label("Hover")
                    .debug_selector(|| "hover-probe".into())
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |_, _, window, _| {
                                observed.set(Some(window.text_style().color));
                            },
                        )
                        .size(px(1.)),
                    ),
            )
        }
    }

    cx.update(gpui_component::init);
    let variants = cx.update(|cx| {
        let custom_foreground = hsla(0.43, 0.8, 0.7, 1.);
        [
            (ButtonVariant::Primary, cx.theme().primary_foreground),
            (ButtonVariant::Secondary, cx.theme().secondary_foreground),
            // GPUI 0.5.1 shares secondary_foreground for Ghost and Secondary.
            (ButtonVariant::Ghost, cx.theme().secondary_foreground),
            (
                ButtonVariant::Custom(ButtonCustomVariant::new(cx).foreground(custom_foreground)),
                custom_foreground,
            ),
        ]
    });
    for (variant, expected) in variants {
        let observed = Rc::new(Cell::new(None));
        let capture = observed.clone();
        let handle = cx.add_window(|_, _| HoverProbe {
            variant,
            observed: capture,
        });
        let mut window = VisualTestContext::from_window(handle.into(), cx);
        window.simulate_event(MouseMoveEvent {
            position: point(px(300.), px(200.)),
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        draw(&mut window);
        assert_eq!(observed.get(), Some(expected), "normal foreground");
        let bounds = window.debug_bounds("hover-probe").expect("painted button");
        window.simulate_event(MouseMoveEvent {
            position: bounds.center(),
            pressed_button: None,
            modifiers: Modifiers::default(),
        });
        draw(&mut window);
        assert_eq!(
            observed.get(),
            Some(expected),
            "hover must retain variant foreground"
        );
    }
}
