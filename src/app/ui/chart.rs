//! Chart area, MA toggles, drawing mode, hover strip.

use gpui::{
    Bounds, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, ScrollWheelEvent, Styled, canvas, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, PixelsExt, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::chart::{ChartPaintData, chart_layout, index_from_x, paint_chart, price_from_y};
use crate::model::{
    MinutePeriod, QuoteSnapshot, TrendLine, format_pct, format_price, format_volume, shared,
};

use super::super::helpers::*;
use super::super::labels::L;
use super::super::{ChartKind, ChartRange, DetailTab, StockApp};
use gpui_component::skeleton::Skeleton;

impl StockApp {
    pub(crate) fn render_chart_area(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sym = self.current_symbol();
        // Only use candle snapshot when it belongs to the selected symbol
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let snap = if candles_match {
            match self.chart_kind {
                ChartKind::Intraday => self.minute.as_ref().and_then(|m| m.snapshot()),
                ChartKind::DayK | ChartKind::MinuteK(_) => {
                    QuoteSnapshot::from_candles(&self.candles)
                }
            }
        } else {
            None
        };
        let up = snap
            .as_ref()
            .map(|s| s.change_pct >= 0.0)
            .or_else(|| sym.map(|s| s.is_up()))
            .unwrap_or(true);
        let chg_color = self.chg_color(up, cx);
        let paint = self.chart_paint_data(cx);
        let work = self.work_mode;

        let code_show = self.display_code(self.selected.as_ref());
        let name_raw = sym.map(|s| s.name.as_ref().to_string()).unwrap_or_default();
        let name_show = if work {
            None
        } else if is_real_name(&name_raw, self.selected.as_ref()) {
            Some(shared(name_raw))
        } else {
            None
        };
        let board = if work {
            shared("metric")
        } else {
            sym.map(|s| s.board.clone()).unwrap_or_else(|| shared(""))
        };
        // Prefer live quote on the watchlist; fall back to last candle only if matched
        let close = sym
            .map(|s| s.last)
            .filter(|v| *v > 0.0)
            .or_else(|| snap.as_ref().map(|s| s.close))
            .unwrap_or(0.0);
        let chg = sym
            .map(|s| s.change_pct)
            .or_else(|| snap.as_ref().map(|s| s.change_pct))
            .unwrap_or(0.0);
        let close_disp = self.format_value(close);
        let chg_disp = self.format_change(chg);
        // Open position cost for header chip + dashed cost line on chart.
        let cost_mark = if work {
            None
        } else {
            self.portfolio
                .position_of(self.selected.as_ref())
                .filter(|p| p.is_open() && p.avg_cost.is_finite() && p.avg_cost > 0.0)
                .map(|p| {
                    let cost = p.avg_cost;
                    let pnl_pct = if close > 0.0 && cost > 0.0 {
                        (close - cost) / cost * 100.0
                    } else {
                        0.0
                    };
                    (cost, pnl_pct)
                })
        };

        // OHLC strip (merged into quote header to free chart vertical space)
        let ohlc_el = if candles_match {
            let o = snap.as_ref().map(|s| s.open).unwrap_or(0.0);
            let hi = snap.as_ref().map(|s| s.high).unwrap_or(0.0);
            let lo = snap.as_ref().map(|s| s.low).unwrap_or(0.0);
            let v = snap.as_ref().map(|s| s.volume).unwrap_or(0);
            if work {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("min {}", self.format_value(lo)))
                    .child(format!("max {}", self.format_value(hi)))
                    .child(format!("pts {}", format_volume(v)))
                    .into_any_element()
            } else {
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("开 {}", format_price(o)))
                    .child(format!("高 {}", format_price(hi)))
                    .child(format!("低 {}", format_price(lo)))
                    .child(format!("量 {}", format_volume(v)))
                    .into_any_element()
            }
        } else {
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if self.loading {
                    if matches!(self.chart_kind, ChartKind::Intraday) && !work {
                        "分时加载中…"
                    } else {
                        L::chart_loading(work)
                    }
                } else if self.refreshing {
                    L::chart_refreshing(work)
                } else if matches!(self.chart_kind, ChartKind::Intraday) && !work {
                    "暂无分时数据"
                } else {
                    L::chart_no_data(work)
                })
                .into_any_element()
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            // Used by the layout regression test; no-op outside test builds.
            .debug_selector(|| "chart-area-root".into())
            // Quote identity + price + OHLC（原两行合并为一行）
            .child(
                h_flex()
                    .id("chart-quote-header")
                    .h(px(48.))
                    .flex_shrink_0()
                    .px_4()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .debug_selector(|| "chart-quote-header".into())
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .min_w_0()
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(code_show),
                            )
                            .when_some(name_show, |row, n| {
                                row.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(n),
                                )
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(cx.theme().muted)
                                    .text_color(cx.theme().muted_foreground)
                                    .child(board),
                            )
                            .child(div().w(px(8.)))
                            .child(ohlc_el),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(close_disp),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .text_color(chg_color)
                                    .child(chg_disp),
                            )
                            .when_some(cost_mark, |row, (cost, pnl_pct)| {
                                let pnl_color = self.chg_color(pnl_pct >= 0.0, cx);
                                row.child(div().w(px(6.)))
                                    .child(
                                        div()
                                            .text_xs()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_full()
                                            .bg(cx.theme().muted)
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("成本 {}", format_price(cost))),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_medium()
                                            .text_color(pnl_color)
                                            .child(format!("{:+.2}%", pnl_pct)),
                                    )
                            })
                            .when(self.refreshing, |row| {
                                row.child(div().w(px(6.))).child(
                                    div()
                                        .text_xs()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_full()
                                        .bg(cx.theme().accent.opacity(0.18))
                                        .text_color(cx.theme().accent)
                                        .child(L::chart_refreshing(work)),
                                )
                            })
                            .when(self.loading && !self.refreshing, |row| {
                                row.child(div().w(px(6.))).child(
                                    div()
                                        .text_xs()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_full()
                                        .bg(cx.theme().muted)
                                        .text_color(cx.theme().muted_foreground)
                                        .child(L::loading_short(work)),
                                )
                            }),
                    ),
            )
            // Toolbar：周期 / 指标 / 画线（操作与行情分离）
            .child(
                h_flex()
                    .h(px(34.))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(self.kind_button(
                                if work { "Intraday" } else { "分时" },
                                ChartKind::Intraday,
                                cx,
                            ))
                            .child(self.kind_button(
                                if work { "Daily" } else { "日K" },
                                ChartKind::DayK,
                                cx,
                            ))
                            .child(self.kind_button(
                                if work { "Minute" } else { "分钟" },
                                ChartKind::MinuteK(self.current_minute_period()),
                                cx,
                            ))
                            .when(matches!(self.chart_kind, ChartKind::DayK), |row| {
                                row.child(div().w(px(6.)))
                                    .children(ChartRange::all().map(|range| {
                                        let active = self.range == range;
                                        Button::new(("range", range as u32))
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(range.label())
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.set_range(range, cx);
                                            }))
                                    }))
                            })
                            .when(matches!(self.chart_kind, ChartKind::MinuteK(_)), |row| {
                                row.child(div().w(px(6.)))
                                    .children(MinutePeriod::all().map(|p| {
                                        let active = self.chart_kind == ChartKind::MinuteK(p);
                                        Button::new(("mperiod", p as u32))
                                            .xsmall()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(p.label())
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.set_chart_kind(ChartKind::MinuteK(p), cx);
                                            }))
                                    }))
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .when(!matches!(self.chart_kind, ChartKind::Intraday), |row| {
                                row.child(self.ma_toggle(
                                    "ma5",
                                    if work { "L1" } else { "MA5" },
                                    self.show_ma5,
                                    cx,
                                ))
                                .child(self.ma_toggle(
                                    "ma10",
                                    if work { "L2" } else { "MA10" },
                                    self.show_ma10,
                                    cx,
                                ))
                                .child(self.ma_toggle(
                                    "ma20",
                                    if work { "L3" } else { "MA20" },
                                    self.show_ma20,
                                    cx,
                                ))
                                .child(self.ma_toggle(
                                    "ma60",
                                    if work { "L4" } else { "MA60" },
                                    self.show_ma60,
                                    cx,
                                ))
                                .when(!work, |row| {
                                    row.child(self.ma_toggle(
                                        "vol",
                                        "VOL",
                                        self.show_volume,
                                        cx,
                                    ))
                                    .child(self.ma_toggle(
                                        "macd",
                                        "MACD",
                                        self.show_macd,
                                        cx,
                                    ))
                                    .child(self.ma_toggle(
                                        "boll",
                                        "BOLL",
                                        self.show_boll,
                                        cx,
                                    ))
                                })
                            })
                            .when(!work, |row| {
                                row.child(div().w(px(6.)))
                                    .child(
                                        Button::new("draw-toggle")
                                            .xsmall()
                                            .when(self.drawing_mode, |b| b.primary())
                                            .when(!self.drawing_mode, |b| b.ghost())
                                            .label("画线")
                                            .tooltip(
                                                "画线模式：拖拽画趋势线，单击画水平价格线；Esc 退出",
                                            )
                                            .on_click(cx.listener(|this, _, _w, cx| {
                                                this.toggle_drawing_mode(cx);
                                            })),
                                    )
                                    .when(self.drawing_mode, |row| {
                                        row.child(
                                            Button::new("clear-lines")
                                                .xsmall()
                                                .ghost()
                                                .label("清除")
                                                .tooltip("清除当前标的的全部画线")
                                                .on_click(cx.listener(|this, _, _w, cx| {
                                                    this.clear_chart_lines(cx);
                                                })),
                                        )
                                    })
                            }),
                    ),
            )
            // hover strip
            .child(
                h_flex()
                    .h(px(26.))
                    .flex_shrink_0()
                    .px_3()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar.opacity(0.4))
                    .child(self.render_hover_strip(cx)),
            )
            // chart canvas
            .child({
                let entity = cx.entity().clone();
                // Move paint into the canvas (no second full series clone per frame).
                let show_skeleton =
                    self.loading && paint.candles.is_empty() && paint.minute.is_none();
                div()
                    .id("chart-body")
                    .flex_1()
                    .min_h_0()
                    .min_h(px(220.))
                    .p_2()
                    .child(
                        div()
                            .id("chart-surface")
                            .size_full()
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(cx.theme().radius)
                            .relative()
                            .overflow_hidden()
                            .child(
                                canvas(
                                    move |bounds, _, _| bounds,
                                    move |bounds, _, window, cx| {
                                        // Geometry only — never notify (update alone is silent).
                                        // Skip writes when unchanged to avoid thrashing state.
                                        entity.update(cx, |this, _| {
                                            let w = bounds.size.width.as_f32();
                                            let origin_changed = this.chart_origin.x != bounds.origin.x
                                                || this.chart_origin.y != bounds.origin.y;
                                            let size_changed = (this.chart_width - w).abs() > 0.5
                                                || (this.chart_bounds.size.height.as_f32()
                                                    - bounds.size.height.as_f32())
                                                    .abs()
                                                    > 0.5;
                                            if origin_changed || size_changed {
                                                this.chart_origin = bounds.origin;
                                                this.chart_bounds = bounds;
                                                this.chart_width = w;
                                            }
                                        });
                                        paint_chart(bounds, &paint, window);
                                    },
                                )
                                .size_full(),
                            )
                            .when(show_skeleton, |surface| {
                                surface.child(
                                    v_flex()
                                        .absolute()
                                        .inset_0()
                                        .p_6()
                                        .gap_3()
                                        .justify_center()
                                        .bg(cx.theme().background.opacity(0.55))
                                        .child(Skeleton::new().h_3().w_full())
                                        .child(Skeleton::new().secondary().h_3().w(px(280.)))
                                        .child(Skeleton::new().h_3().w(px(320.)))
                                        .child(Skeleton::new().secondary().h_3().w_full())
                                        .child(
                                            div()
                                                .mt_2()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(L::chart_loading(work)),
                                        ),
                                )
                            })
                            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _w, cx| {
                                let local_x =
                                    ev.position.x.as_f32() - this.chart_origin.x.as_f32();
                                let local_y =
                                    ev.position.y.as_f32() - this.chart_origin.y.as_f32();
                                if this.drawing_mode && this.drawing_anchor.is_some() {
                                    let paint = this.chart_paint_data(cx);
                                    let bounds = this.chart_bounds;
                                    if let Some((ix, price)) =
                                        this.anchor_from_local(&paint, bounds, local_x, local_y)
                                        && let Some((ax, ap)) = this.drawing_anchor {
                                            let color_ix = this.draw_color_ix;
                                            this.draft_line = Some(TrendLine::new(
                                                (ax, ap),
                                                (ix, price),
                                                color_ix,
                                            ));
                                            cx.notify();
                                        }
                                    return;
                                }
                                let (start, end) = this.chart_visible_range();
                                let vn = end.saturating_sub(start);
                                let local_ix = index_from_x(local_x, this.chart_width, vn);
                                let abs_ix = local_ix.map(|i| start + i);
                                if this.hover_ix != abs_ix {
                                    this.hover_ix = abs_ix;
                                    cx.notify();
                                }
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                                    if this.drawing_mode {
                                        if ev.click_count == 1 {
                                            let local_x = ev.position.x.as_f32()
                                                - this.chart_origin.x.as_f32();
                                            let local_y = ev.position.y.as_f32()
                                                - this.chart_origin.y.as_f32();
                                            let paint = this.chart_paint_data(cx);
                                            let bounds = this.chart_bounds;
                                            if let Some(anchor) = this.anchor_from_local(
                                                &paint,
                                                bounds,
                                                local_x,
                                                local_y,
                                            ) {
                                                this.drawing_anchor = Some(anchor);
                                                this.draft_line = Some(TrendLine::new(
                                                    anchor,
                                                    anchor,
                                                    this.draw_color_ix,
                                                ));
                                                this.hover_ix = None;
                                                cx.notify();
                                            }
                                        }
                                        return;
                                    }
                                    if ev.click_count >= 2 {
                                        this.reset_chart_view();
                                        this.hover_ix = None;
                                        this.status = shared(if this.work_mode {
                                            "zoom reset"
                                        } else {
                                            "已重置图表缩放"
                                        });
                                        cx.notify();
                                    }
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(move |this, _ev, _w, cx| {
                                    if this.drawing_mode {
                                        if let Some(draft) = this.draft_line.take() {
                                            let commit = {
                                                let from = draft.from;
                                                let to = draft.to;
                                                let same_bar = from.0 == to.0;
                                                let near_price = (to.1 - from.1).abs()
                                                    <= (from.1.abs() * 0.005).max(0.01);
                                                if same_bar && near_price {
                                                    // 单击 → 水平价格线，横跨当前可见区间。
                                                    let (vs, ve) = this.chart_visible_range();
                                                    let (a, b) =
                                                        (vs.min(ve.saturating_sub(1)), ve.saturating_sub(1));
                                                    Some(TrendLine::price_line(
                                                        a,
                                                        b,
                                                        from.1,
                                                        this.draw_color_ix,
                                                    ))
                                                } else {
                                                    Some(draft)
                                                }
                                            };
                                            if let Some(line) = commit {
                                                let selected = this.selected.to_string();
                                                this.chart_lines
                                                    .entry(selected)
                                                    .or_default()
                                                    .push(line);
                                                this.draw_color_ix = this.draw_color_ix.wrapping_add(1);
                                                this.status = shared(if this.work_mode {
                                                    "line added"
                                                } else {
                                                    "已添加画线"
                                                });
                                                this.schedule_persist(cx);
                                            }
                                        }
                                        this.drawing_anchor = None;
                                        this.draft_line = None;
                                        cx.notify();
                                    }
                                }),
                            )
                            .on_scroll_wheel(cx.listener(move |this, ev: &ScrollWheelEvent, _w, cx| {
                                this.on_chart_scroll(ev, cx);
                            })),
                    )
            })
    }

    pub(crate) fn ma_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        Button::new(id)
            .xsmall()
            .when(on, |b| b.primary())
            .when(!on, |b| b.ghost())
            .label(label)
            .on_click(cx.listener(move |this, _, _w, cx| {
                match id {
                    "ma5" => this.show_ma5 = !this.show_ma5,
                    "ma10" => this.show_ma10 = !this.show_ma10,
                    "ma20" => this.show_ma20 = !this.show_ma20,
                    "ma60" => this.show_ma60 = !this.show_ma60,
                    "vol" => this.show_volume = !this.show_volume,
                    "macd" => this.show_macd = !this.show_macd,
                    "boll" => this.show_boll = !this.show_boll,
                    _ => {}
                }
                this.schedule_persist(cx);
                cx.notify();
            }))
    }

    pub(crate) fn toggle_drawing_mode(&mut self, cx: &mut Context<Self>) {
        self.drawing_mode = !self.drawing_mode;
        self.drawing_anchor = None;
        self.draft_line = None;
        self.hover_ix = None;
        if self.drawing_mode {
            self.status = shared(if self.work_mode {
                "draw mode: drag on the chart to add a line"
            } else {
                "画线模式：在图上拖拽画趋势线；单击生成水平线；再点按钮退出"
            });
        } else {
            self.status = shared(if self.work_mode {
                "draw mode off"
            } else {
                "已退出画线模式"
            });
        }
        cx.notify();
    }

    /// 清空当前标的的全部画线。
    pub(crate) fn clear_chart_lines(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let removed = self.chart_lines.remove(&code).unwrap_or_default().len();
        self.draft_line = None;
        self.drawing_anchor = None;
        self.status = shared(
            (if self.work_mode {
                format!("removed {removed} line(s)")
            } else {
                format!("已清除 {removed} 条画线")
            })
            .to_string(),
        );
        self.schedule_persist(cx);
        cx.notify();
    }

    /// 把屏幕坐标转换为画线锚点（可见切片内的索引 + 价格）。
    pub(crate) fn anchor_from_local(
        &self,
        paint: &ChartPaintData,
        bounds: Bounds<Pixels>,
        local_x: f32,
        local_y: f32,
    ) -> Option<(usize, f64)> {
        if paint.candles.is_empty() {
            return None;
        }
        let layout = chart_layout(paint, bounds);
        let visible = paint.candles.len();
        let (start, end) = self.chart_visible_range();
        if start >= end {
            return None;
        }
        let local_ix = index_from_x(local_x, bounds.size.width.as_f32(), visible)?;
        let ix = start + local_ix;
        if ix >= end {
            return None;
        }
        // 只有价格窗格内的点击才落锚；副图区域忽略。
        if local_y < layout.plot_top || local_y > layout.plot_top + layout.price_h {
            return None;
        }
        let price = price_from_y(&layout, local_y);
        Some((ix, price))
    }

    /// The minute period to select when the 分钟K button is pressed.
    pub(crate) fn current_minute_period(&self) -> MinutePeriod {
        match self.chart_kind {
            ChartKind::MinuteK(p) => p,
            _ => MinutePeriod::M5,
        }
    }

    pub(crate) fn kind_button(
        &self,
        label: &'static str,
        kind: ChartKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.chart_kind == kind;
        Button::new(kind.to_label())
            .xsmall()
            .when(active, |b| b.primary())
            .when(!active, |b| b.ghost())
            .label(label)
            .on_click(cx.listener(move |this, _, _w, cx| {
                this.set_chart_kind(kind, cx);
            }))
    }

    pub(crate) fn render_hover_strip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let candles_match = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let work = self.work_mode;
        if candles_match
            && matches!(self.chart_kind, ChartKind::Intraday)
            && let Some(ix) = self.hover_ix
            && let (Some(m), Some(p)) = (
                self.minute.as_ref(),
                self.minute.as_ref().and_then(|m| m.points.get(ix)),
            )
        {
            let color = self.chg_color(p.price >= m.prev_close, cx);
            let vol = p.minute_volume(ix.checked_sub(1).map(|j| &m.points[j]));
            return h_flex()
                .gap_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    div()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(p.time.clone()),
                )
                .child(
                    div()
                        .text_color(color)
                        .child(format!("价 {}", format_price(p.price))),
                )
                .child(format!("均价 {}", format_price(p.avg_price())))
                .child(format!(
                    "涨跌 {}",
                    format_pct(if m.prev_close > 0.0 {
                        (p.price - m.prev_close) / m.prev_close * 100.0
                    } else {
                        0.0
                    })
                ))
                .child(format!("量 {}", format_volume(vol)))
                .into_any_element();
        }
        if candles_match
            && let Some(ix) = self.hover_ix
            && let Some(c) = self.candles.get(ix)
        {
            let color = self.chg_color(c.close >= c.open, cx);
            let (m5, m10, m20, m60) = self.ma.value_at(ix);
            let (dif, dea, hist) = self.macd.value_at(ix);
            let (b_up, b_mid, b_low) = self.boll.value_at(ix);
            let date_label = format_candle_date(c.date.as_ref());
            if work {
                return h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        div()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(date_label),
                    )
                    .child(format!("v {}", self.format_value(c.close)))
                    .child(format!("lo {}", self.format_value(c.low)))
                    .child(format!("hi {}", self.format_value(c.high)))
                    .when(m5.is_some(), |this| {
                        this.child(format!("L1 {}", self.format_value(m5.unwrap())))
                    })
                    .when(m10.is_some(), |this| {
                        this.child(format!("L2 {}", self.format_value(m10.unwrap())))
                    })
                    .when(m20.is_some(), |this| {
                        this.child(format!("L3 {}", self.format_value(m20.unwrap())))
                    })
                    .when(m60.is_some(), |this| {
                        this.child(format!("L4 {}", self.format_value(m60.unwrap())))
                    })
                    .into_any_element();
            }
            let row = h_flex()
                .gap_2()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    div()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(date_label),
                )
                .child(format!(
                    "开{} 高{} 低{}",
                    format_price(c.open),
                    format_price(c.high),
                    format_price(c.low)
                ))
                .child(
                    div()
                        .text_color(color)
                        .child(format!("收{}", format_price(c.close))),
                )
                .child(format!("量{}", format_volume(c.volume)))
                .when_some(
                    self.portfolio
                        .position_of(self.selected.as_ref())
                        .filter(|p| p.is_open() && p.avg_cost > 0.0)
                        .map(|p| p.avg_cost),
                    |this, cost| {
                        let vs = (c.close - cost) / cost * 100.0;
                        this.child(
                            div()
                                .text_color(self.chg_color(vs >= 0.0, cx))
                                .child(format!("成本{} · {:+.2}%", format_price(cost), vs)),
                        )
                    },
                )
                .when(m5.is_some(), |this| {
                    this.child(format!("MA5 {}", format_price(m5.unwrap())))
                })
                .when(m10.is_some(), |this| {
                    this.child(format!("MA10 {}", format_price(m10.unwrap())))
                })
                .when(m20.is_some(), |this| {
                    this.child(format!("MA20 {}", format_price(m20.unwrap())))
                })
                .when(m60.is_some(), |this| {
                    this.child(format!("MA60 {}", format_price(m60.unwrap())))
                })
                .when(
                    self.show_macd && dif.is_some() && dea.is_some() && hist.is_some(),
                    |this| {
                        this.child(format!(
                            "MACD {:.3}/{:.3}/{:.3}",
                            dif.unwrap(),
                            dea.unwrap(),
                            hist.unwrap()
                        ))
                    },
                )
                .when(
                    self.show_boll && b_up.is_some() && b_mid.is_some() && b_low.is_some(),
                    |this| {
                        this.child(format!(
                            "BOLL {:.2}/{:.2}/{:.2}",
                            b_up.unwrap(),
                            b_mid.unwrap(),
                            b_low.unwrap()
                        ))
                    },
                );
            return row.into_any_element();
        }
        let (vs, ve) = self.chart_visible_range();
        let zoom_hint = if self.drawing_mode && !work {
            "画线模式：拖拽画趋势线 · 单击水平线 · Esc 退出".to_string()
        } else if matches!(self.chart_kind, ChartKind::Intraday) {
            if let Some(m) = self.minute.as_ref() {
                let date = if m.date.len() >= 8 {
                    format!("{}-{}-{}", &m.date[..4], &m.date[4..6], &m.date[6..8])
                } else {
                    m.date.clone()
                };
                if work {
                    format!(
                        "intraday {date} · {} pts · scroll/pinch zoom · pan · dblclick reset",
                        m.points.len()
                    )
                } else {
                    format!(
                        "分时 {date} · {} 点 · 滚轮/捏合缩放 · 横向平移 · 双击重置",
                        m.points.len()
                    )
                }
            } else if work {
                "intraday · scroll/pinch zoom · pan · dblclick reset".into()
            } else {
                "分时 · 滚轮/捏合缩放 · 横向平移 · 双击重置".into()
            }
        } else if !self.candles.is_empty() && ve > vs {
            let first = self.candles.get(vs).map(|c| c.date.as_ref()).unwrap_or("?");
            let last = self
                .candles
                .get(ve - 1)
                .map(|c| c.date.as_ref())
                .unwrap_or("?");
            if work {
                format!(
                    "scroll/pinch zoom · pan · dblclick reset · {}…{} ({} pts)",
                    format_candle_date(first),
                    format_candle_date(last),
                    ve - vs
                )
            } else {
                format!(
                    "滚轮/捏合缩放 · 横向平移 · 双击重置 · 可见 {}～{}（{}根）",
                    format_candle_date(first),
                    format_candle_date(last),
                    ve - vs
                )
            }
        } else if work {
            "scroll/pinch zoom · pan · dblclick reset · hover for values".into()
        } else {
            "滚轮/捏合缩放 · 横向平移 · 双击重置 · 移动鼠标查看十字线".into()
        };
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(if self.loading {
                L::loading_short(work).to_string()
            } else if self.refreshing {
                L::chart_refreshing(work).to_string()
            } else if !candles_match || self.candles.is_empty() {
                if work {
                    "No series data".to_string()
                } else {
                    "暂无K线数据".to_string()
                }
            } else {
                zoom_hint
            })
            .into_any_element()
    }

    pub(crate) fn set_detail_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        if self.detail_tab == tab {
            return;
        }
        self.detail_tab = tab;
        self.schedule_persist(cx);
        cx.notify();
    }
}
