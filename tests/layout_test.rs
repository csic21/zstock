use gpui::{
    AnyWindowHandle, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, TestAppContext, VisualTestContext, Window, div, px, size,
};
use gpui_component::{
    PixelsExt,
    resizable::{h_resizable, resizable_panel, v_resizable},
    v_flex,
};

/// Minimal reproduction of the app's root layout:
/// a fixed-height title bar plus a resizable horizontal split
/// (left sidebar panel + main column).
struct TestApp;

impl Render for TestApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                div()
                    .h(px(34.))
                    .w_full()
                    .flex_shrink_0()
                    .debug_selector(|| "title-bar".into()),
            )
            .child(
                div().flex_1().min_h_0().w_full().overflow_hidden().child(
                    h_resizable("main-h")
                        .child(
                            resizable_panel()
                                .size(px(280.))
                                .size_range(px(200.)..px(440.))
                                .child(div().size_full().debug_selector(|| "left-content".into())),
                        )
                        .child(
                            resizable_panel()
                                .child(div().size_full().debug_selector(|| "main-content".into())),
                        ),
                ),
            )
    }
}

/// Closer replica of the real left sidebar + main column contents.
struct RichApp;

impl Render for RichApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                div()
                    .h(px(34.))
                    .w_full()
                    .flex_shrink_0()
                    .debug_selector(|| "title-bar".into()),
            )
            .child(
                div().flex_1().min_h_0().w_full().overflow_hidden().child(
                    h_resizable("main-h")
                        .child(
                            resizable_panel()
                                .size(px(280.))
                                .size_range(px(200.)..px(440.))
                                .child(rich_left_panel()),
                        )
                        .child(
                            resizable_panel().child(
                                v_resizable("main-v")
                                    .child(
                                        resizable_panel().child(
                                            v_flex()
                                                .size_full()
                                                .child(div().h(px(90.)).w_full())
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_h_0()
                                                        .w_full()
                                                        .debug_selector(|| "chart".into()),
                                                ),
                                        ),
                                    )
                                    .child(
                                        resizable_panel()
                                            .size(px(411.))
                                            .size_range(px(140.)..px(420.))
                                            .child(
                                                div()
                                                    .size_full()
                                                    .debug_selector(|| "details".into()),
                                            ),
                                    ),
                            ),
                        ),
                ),
            )
    }
}

fn rich_left_panel() -> impl IntoElement {
    v_flex()
        .size_full()
        .child(
            div()
                .h(px(36.))
                .w_full()
                .debug_selector(|| "left-tabs".into()),
        )
        .child(
            v_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(div().h(px(28.)).w_full())
                .child(div().h(px(26.)).w_full())
                .child(
                    v_flex()
                        .id("watchlist-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .children(
                            (0..6usize).map(|ix| div().h(px(48.)).w_full().id(("wl-row", ix))),
                        ),
                )
                .child(div().h(px(32.)).w_full()),
        )
}

#[gpui::test]
fn resizable_panels_should_fill_the_available_height(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.add_window(|_window, _cx| TestApp);
    let window: AnyWindowHandle = window.into();
    let mut window = VisualTestContext::from_window(window, cx);
    window.simulate_resize(size(px(1320.), px(860.)));
    window.run_until_parked();

    let left = window.debug_bounds("left-content").expect("left bounds");
    let main = window.debug_bounds("main-content").expect("main bounds");

    eprintln!("title bar -> left: {left:?}, main: {main:?}");

    // Below a 34px title bar, both panels should start at y=34 and span to the
    // bottom of the 860px-tall window (height 826).
    assert!(
        (left.origin.y.as_f32() - 34.0).abs() < 2.0,
        "left panel should start right below the title bar, got y={}",
        left.origin.y.as_f32()
    );
    assert!(
        (left.size.height.as_f32() - 826.0).abs() < 2.0,
        "left panel should be full height, got {}",
        left.size.height.as_f32()
    );
    assert!(
        (main.origin.y.as_f32() - 34.0).abs() < 2.0,
        "main panel should start right below the title bar, got y={}",
        main.origin.y.as_f32()
    );
}

#[gpui::test]
fn rich_content_panels_should_fill_the_available_height(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.add_window(|_window, _cx| RichApp);
    let window: AnyWindowHandle = window.into();
    let mut window = VisualTestContext::from_window(window, cx);
    window.simulate_resize(size(px(1320.), px(860.)));
    window.run_until_parked();

    let tabs = window.debug_bounds("left-tabs").expect("tabs bounds");
    let chart = window.debug_bounds("chart").expect("chart bounds");
    let details = window.debug_bounds("details").expect("details bounds");

    eprintln!("tabs: {tabs:?}, chart: {chart:?}, details: {details:?}");

    assert!(
        (tabs.origin.y.as_f32() - 34.0).abs() < 2.0,
        "left sidebar tabs should start right below the title bar, got y={}",
        tabs.origin.y.as_f32()
    );
    assert!(
        (tabs.size.height.as_f32() - 36.0).abs() < 2.0,
        "tabs row should be 36px tall, got {}",
        tabs.size.height.as_f32()
    );
    assert!(
        (details.origin.y.as_f32() + details.size.height.as_f32() - 860.0).abs() < 2.0,
        "details panel should reach the bottom of the window, got {details:?}"
    );
}

#[gpui::test]
fn reference_window_sizes_keep_all_primary_regions_paintable(cx: &mut TestAppContext) {
    cx.update(gpui_component::init);
    for (width, height) in [(720.0, 440.0), (920.0, 580.0), (1280.0, 800.0)] {
        let window = cx.add_window(|_window, _cx| RichApp);
        let window: AnyWindowHandle = window.into();
        let mut window = VisualTestContext::from_window(window, cx);
        window.simulate_resize(size(px(width), px(height)));
        window.run_until_parked();

        for selector in ["left-tabs", "chart", "details"] {
            let bounds = window.debug_bounds(selector).expect("reference region");
            assert!(
                bounds.size.width.as_f32() > 0.0 && bounds.size.height.as_f32() > 0.0,
                "{selector} collapsed at {width}x{height}: {bounds:?}"
            );
        }
    }
}
