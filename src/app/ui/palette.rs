//! Command palette overlay.

#![allow(unused_imports)]

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    canvas, div, point, px, size, App, AppContext, Bounds, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, Timer, Window, WindowBounds, WindowOptions,
    prelude::FluentBuilder,
};
use gpui_component::{
    button::{Button, ButtonVariants},
    h_flex,
    IconName,
    input::{Input, InputEvent, InputState},
    resizable::{h_resizable, resizable_panel, v_resizable, ResizableState},
    v_flex, ActiveTheme, Disableable, PixelsExt, Root, Sizable, StyledExt, Theme, ThemeMode,
    TitleBar, TITLE_BAR_HEIGHT,
};
use gpui_component::tooltip::Tooltip;

use crate::chart::{
    chart_layout, index_from_x, paint_chart, paint_sparkline, price_from_y, BollPaintData,
    ChartPaintData, ChartStyle, MacdPaintData, MinutePaintData,
};
use crate::data::ai::{self, AiCliProvider, AiConfig, AiKind, AiTransport};
use crate::data::levels;
use crate::data::portfolio::{
    self, format_money, format_shares, Portfolio, PortfolioSummary, TradeSide,
};
use crate::data::scout::{self, ScoutPick, ScoutVerdict, SCOUT_CANDIDATE_N};
use crate::data::treasure::{self, fmt_dd, fmt_pos, TreasureHit, TREASURE_KLINE_LIMIT};
use crate::data::universe::{self, FinFilter, TreasurePool, TREASURE_SCAN_CAP, TREASURE_TOP_N};
use crate::data::{
    indicators::{BollSeries, MaSeries, MacdSeries},
    market, session, signals,
};
use crate::data::market::Sourced;
use crate::data::session::{filter_codes_in_session, idle_delay_secs, open_markets_now, MarketSet};
use crate::model::{
    board_for_code, disguise_index, disguise_label, format_index, format_pct, format_price,
    format_volume, normalize_code, shared, Candle, IndexSnap, MinutePeriod, MinuteSeries,
    QuoteSnapshot, Symbol, TrendLine,
};
use crate::storage::{
    self, clamp_quote_interval_secs, normalize_status_bar, AppConfig, ColorScheme, DockLayout,
    WatchlistSort, STATUS_BAR_MAX_CODES,
};
use crate::update::{self, UpdateState};

use super::super::{
    AiCacheEntry, AiPanelState, AiSource, ChartKind, ChartRange, DetailTab, LeftTab, SettingsSection,
    StockApp, CHART_MIN_VISIBLE, QUOTE_INTERVAL_ERR_MAX, QUOTE_INTERVAL_PRESETS, TITLE_NORMAL,
    TITLE_WORK, TREASURE_SCAN_GAP,
};
use super::super::helpers::*;
use super::super::labels::L;



impl StockApp {
    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let local: Vec<(usize, Symbol)> = self
            .filtered_local
            .iter()
            .filter_map(|&i| self.symbols.get(i).cloned().map(|s| (i, s)))
            .collect();
        let remote = self.palette_hits.clone();
        let n_local = local.len();
        let highlight = self.palette_index;
        let work = self.work_mode;

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(72.))
            .bg(gpui::hsla(0., 0., 0., 0.55))
            // Same modal isolation as the settings overlay: don't let wheel
            // scrolling or hover styles reach the app behind the palette.
            .occlude()
            // Capture ↑↓ while the search input is focused (Input would otherwise
            // eat MoveUp/MoveDown for multi-line caret motion).
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _w, cx| {
                let key = event.keystroke.key.as_str();
                match key {
                    "up" => {
                        this.palette_move(-1, cx);
                        cx.stop_propagation();
                    }
                    "down" => {
                        this.palette_move(1, cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                v_flex()
                    .id("palette-panel")
                    .key_context("stock_palette")
                    .w(px(560.))
                    .max_h(px(480.))
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().popover)
                    .overflow_hidden()
                    .on_mouse_down_out(cx.listener(|this, _, _w, cx| {
                        this.palette_open = false;
                        cx.notify();
                    }))
                    .child(
                        h_flex()
                            .h(px(48.))
                            .px_3()
                            .items_center()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(div().flex_1().child(Input::new(&self.palette_query))),
                    )
                    .child({
                        let mut list = v_flex()
                            .id("palette-results")
                            .flex_1()
                            .overflow_y_scroll()
                            .p_1();
                        if !local.is_empty() {
                            list = list.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(L::palette_section_local(work)),
                            );
                            for (i, (_, sym)) in local.into_iter().enumerate() {
                                list = list.child(palette_row(
                                    sym,
                                    true,
                                    i as u64,
                                    highlight == i,
                                    self.color_scheme,
                                    self.work_mode,
                                    self.work_identity_reveal,
                                    cx,
                                ));
                            }
                        }
                        if !remote.is_empty() {
                            list = list.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(L::palette_section_remote(work)),
                            );
                            for (i, sym) in remote.into_iter().enumerate() {
                                let flat = n_local + i;
                                list = list.child(palette_row(
                                    sym,
                                    false,
                                    10_000 + i as u64,
                                    highlight == flat,
                                    self.color_scheme,
                                    self.work_mode,
                                    self.work_identity_reveal,
                                    cx,
                                ));
                            }
                        }
                        if self.filtered_local.is_empty() && self.palette_hits.is_empty() {
                            list = list.child(
                                div()
                                    .p_4()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(L::palette_empty(work)),
                            );
                        }
                        list
                    })
                    .child(
                        h_flex()
                            .h(px(28.))
                            .px_3()
                            .items_center()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(L::palette_footer(work)),
                            ),
                    ),
            )
    }
}
