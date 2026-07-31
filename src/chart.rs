//! Interactive chart: candlesticks (normal) or area/line (work mode), with MA + volume + crosshair.

use gpui::{
    fill, point, px, size, Bounds, Hsla, PathBuilder, Pixels, Window,
};
use gpui_component::PixelsExt;

use crate::data::indicators::MaSeries;
use crate::model::Candle;

/// Visual style for the main series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartStyle {
    /// Classic OHLC candles (stock terminal look).
    #[default]
    Candles,
    /// Single close line + filled area (looks like a generic metric chart).
    Area,
}

#[derive(Clone)]
pub struct ChartPaintData {
    pub candles: Vec<Candle>,
    pub ma: MaSeries,
    /// Intraday (分时) overlay; when present the chart paints a time-share layout
    /// (price line + average line + prev-close baseline + per-minute volume).
    pub minute: Option<MinutePaintData>,
    pub show_ma5: bool,
    pub show_ma10: bool,
    pub show_ma20: bool,
    pub show_ma60: bool,
    /// Draw volume bars under the price pane.
    pub show_volume: bool,
    pub hover_ix: Option<usize>,
    pub style: ChartStyle,
    pub bullish: Hsla,
    pub bearish: Hsla,
    /// Primary series color for area/line style.
    pub line_color: Hsla,
    /// Soft fill under the area line.
    pub area_fill: Hsla,
    pub border: Hsla,
    pub ma5_color: Hsla,
    pub ma10_color: Hsla,
    pub ma20_color: Hsla,
    pub ma60_color: Hsla,
    pub crosshair: Hsla,
    /// Axis / tick label color (muted).
    pub axis_color: Hsla,
}

/// Paint data for the classic intraday (分时) chart.
#[derive(Clone)]
pub struct MinutePaintData {
    pub prices: Vec<f64>,
    /// Volume-weighted average price per minute.
    pub avg: Vec<f64>,
    /// Per-minute volume (手).
    pub volumes: Vec<u64>,
    pub prev_close: f64,
    pub hover_ix: Option<usize>,
    pub bullish: Hsla,
    pub bearish: Hsla,
    /// Average-price line color (classically yellow).
    pub avg_color: Hsla,
    pub border: Hsla,
    pub crosshair: Hsla,
    pub axis_color: Hsla,
}

/// Shared horizontal padding for price + volume panes (must match `index_from_x`).
const PAD_L: f32 = 8.0;
const PAD_R: f32 = 48.0; // room for right-side price ticks
const PAD_T: f32 = 12.0;
const PAD_B: f32 = 18.0;
/// Fraction of plot height reserved for volume (when enabled).
const VOL_FRAC: f32 = 0.20;
const VOL_GAP: f32 = 6.0;

/// Map local X (relative to chart surface left) → candle index.
pub fn index_from_x(local_x: f32, width: f32, n: usize) -> Option<usize> {
    if n == 0 || width <= 0.0 {
        return None;
    }
    let plot_w = (width - PAD_L - PAD_R).max(1.0);
    let x = (local_x - PAD_L).clamp(0.0, plot_w - 0.001);
    let ix = (x / plot_w * n as f32).floor() as usize;
    Some(ix.min(n - 1))
}

pub fn paint_chart(bounds: Bounds<Pixels>, data: &ChartPaintData, window: &mut Window) {
    if let Some(minute) = &data.minute {
        paint_minute_chart(bounds, minute, window);
        return;
    }
    let candles = &data.candles;
    if candles.is_empty() {
        return;
    }

    let width = bounds.size.width.as_f32().max(1.0);
    let height = bounds.size.height.as_f32().max(1.0);
    let plot_w = (width - PAD_L - PAD_R).max(1.0);
    let usable_h = (height - PAD_T - PAD_B).max(1.0);
    let show_vol = data.show_volume;
    let vol_h = if show_vol {
        (usable_h * VOL_FRAC).clamp(28.0, usable_h * 0.32)
    } else {
        0.0
    };
    let price_h = if show_vol {
        (usable_h - vol_h - VOL_GAP).max(40.0)
    } else {
        usable_h
    };
    let origin = bounds.origin;

    let mut y_min = match data.style {
        ChartStyle::Candles => candles.iter().map(|c| c.low).fold(f64::MAX, f64::min),
        ChartStyle::Area => candles.iter().map(|c| c.close).fold(f64::MAX, f64::min),
    };
    let mut y_max = match data.style {
        ChartStyle::Candles => candles.iter().map(|c| c.high).fold(f64::MIN, f64::max),
        ChartStyle::Area => candles.iter().map(|c| c.close).fold(f64::MIN, f64::max),
    };
    for series in [
        data.ma.ma5.as_slice(),
        data.ma.ma10.as_slice(),
        data.ma.ma20.as_slice(),
        data.ma.ma60.as_slice(),
    ] {
        for v in series.iter().flatten() {
            y_min = y_min.min(*v);
            y_max = y_max.max(*v);
        }
    }
    if (y_max - y_min).abs() < 1e-6 {
        y_max += 1.0;
        y_min -= 1.0;
    }
    let y_pad = (y_max - y_min) * 0.05;
    y_min -= y_pad;
    y_max += y_pad;

    let n = candles.len() as f32;
    let slot = plot_w / n;
    let body_w = (slot * 0.65).clamp(1.0, 14.0);

    let y_of = |price: f64| -> f32 {
        let t = ((price - y_min) / (y_max - y_min)) as f32;
        PAD_T + price_h * (1.0 - t)
    };
    let x_center = |i: usize| -> f32 { PAD_L + slot * (i as f32 + 0.5) };

    // Horizontal grid + right-side price ticks (as short ticks; labels via compact bars)
    for g in 0..5 {
        let gy = PAD_T + price_h * (g as f32 / 4.0);
        let mut path = PathBuilder::stroke(px(1.));
        path.move_to(point(origin.x + px(PAD_L), origin.y + px(gy)));
        path.line_to(point(origin.x + px(PAD_L + plot_w), origin.y + px(gy)));
        if let Ok(p) = path.build() {
            window.paint_path(p, data.border.opacity(0.35));
        }
        // Right edge tick mark (visual price guide without font path)
        let mut tick = PathBuilder::stroke(px(1.));
        tick.move_to(point(origin.x + px(PAD_L + plot_w), origin.y + px(gy)));
        tick.line_to(point(origin.x + px(PAD_L + plot_w + 5.0), origin.y + px(gy)));
        if let Ok(p) = tick.build() {
            window.paint_path(p, data.axis_color.opacity(0.7));
        }
    }

    // Mini price level markers (solid dots at high/mid/low on the right gutter)
    for (frac, alpha) in [(0.0f32, 0.9), (0.5, 0.55), (1.0, 0.9)] {
        let gy = PAD_T + price_h * frac;
        window.paint_quad(fill(
            Bounds {
                origin: point(origin.x + px(PAD_L + plot_w + 6.0), origin.y + px(gy - 1.5)),
                size: size(px(3.), px(3.)),
            },
            data.axis_color.opacity(alpha),
        ));
    }

    match data.style {
        ChartStyle::Area => {
            paint_area_series(candles, origin, &x_center, &y_of, PAD_T + price_h, data, window);
        }
        ChartStyle::Candles => {
            paint_candles(candles, origin, &x_center, &y_of, body_w, data, window);
        }
    }

    // MA / overlay lines (drawn after series so they sit on top in area mode)
    let draw_ma = |values: &[Option<f64>], color: Hsla, window: &mut Window| {
        let mut path = PathBuilder::stroke(px(1.5));
        let mut started = false;
        for (i, v) in values.iter().enumerate() {
            let Some(v) = v else {
                started = false;
                continue;
            };
            let x = origin.x + px(x_center(i));
            let y = origin.y + px(y_of(*v));
            if !started {
                path.move_to(point(x, y));
                started = true;
            } else {
                path.line_to(point(x, y));
            }
        }
        if let Ok(p) = path.build() {
            window.paint_path(p, color);
        }
    };
    if data.show_ma5 {
        draw_ma(&data.ma.ma5, data.ma5_color, window);
    }
    if data.show_ma10 {
        draw_ma(&data.ma.ma10, data.ma10_color, window);
    }
    if data.show_ma20 {
        draw_ma(&data.ma.ma20, data.ma20_color, window);
    }
    if data.show_ma60 {
        draw_ma(&data.ma.ma60, data.ma60_color, window);
    }

    // Volume pane
    if show_vol {
        let vol_top = PAD_T + price_h + VOL_GAP;
        // Separator
        let mut sep = PathBuilder::stroke(px(1.));
        sep.move_to(point(origin.x + px(PAD_L), origin.y + px(vol_top - VOL_GAP * 0.5)));
        sep.line_to(point(
            origin.x + px(PAD_L + plot_w),
            origin.y + px(vol_top - VOL_GAP * 0.5),
        ));
        if let Ok(p) = sep.build() {
            window.paint_path(p, data.border.opacity(0.55));
        }

        let max_vol = candles
            .iter()
            .map(|c| c.volume)
            .max()
            .unwrap_or(0)
            .max(1) as f64;

        for (i, c) in candles.iter().enumerate() {
            let cx = x_center(i);
            let color = if c.close >= c.open {
                data.bullish.opacity(0.55)
            } else {
                data.bearish.opacity(0.55)
            };
            let h = ((c.volume as f64 / max_vol) as f32 * vol_h).max(1.0);
            let top = vol_top + vol_h - h;
            let left = cx - body_w / 2.0;
            window.paint_quad(fill(
                Bounds {
                    origin: point(origin.x + px(left), origin.y + px(top)),
                    size: size(px(body_w), px(h)),
                },
                color,
            ));
        }
    }

    // Crosshair (full height including volume)
    if let Some(ix) = data.hover_ix {
        if ix < candles.len() {
            let cx = x_center(ix);
            let c = &candles[ix];
            let cy = y_of(c.close);
            let full_bottom = if show_vol {
                PAD_T + price_h + VOL_GAP + vol_h
            } else {
                PAD_T + price_h
            };

            let mut vline = PathBuilder::stroke(px(1.));
            vline.move_to(point(origin.x + px(cx), origin.y + px(PAD_T)));
            vline.line_to(point(origin.x + px(cx), origin.y + px(full_bottom)));
            if let Ok(p) = vline.build() {
                window.paint_path(p, data.crosshair);
            }

            let mut hline = PathBuilder::stroke(px(1.));
            hline.move_to(point(origin.x + px(PAD_L), origin.y + px(cy)));
            hline.line_to(point(origin.x + px(PAD_L + plot_w), origin.y + px(cy)));
            if let Ok(p) = hline.build() {
                window.paint_path(p, data.crosshair);
            }

            window.paint_quad(fill(
                Bounds {
                    origin: point(origin.x + px(cx - 2.5), origin.y + px(cy - 2.5)),
                    size: size(px(5.), px(5.)),
                },
                data.crosshair,
            ));
        }
    }
}

fn paint_minute_chart(bounds: Bounds<Pixels>, data: &MinutePaintData, window: &mut Window) {
    let n = data.prices.len();
    if n == 0 {
        return;
    }

    let width = bounds.size.width.as_f32().max(1.0);
    let height = bounds.size.height.as_f32().max(1.0);
    let plot_w = (width - PAD_L - PAD_R).max(1.0);
    let usable_h = (height - PAD_T - PAD_B).max(1.0);
    let vol_h = (usable_h * VOL_FRAC).clamp(28.0, usable_h * 0.32);
    let price_h = (usable_h - vol_h - VOL_GAP).max(40.0);
    let origin = bounds.origin;

    // Symmetric scale around prev-close so the baseline sits mid-chart.
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for i in 0..n {
        lo = lo.min(data.prices[i]).min(data.avg[i]);
        hi = hi.max(data.prices[i]).max(data.avg[i]);
    }
    if !lo.is_finite() || !hi.is_finite() {
        lo = data.prev_close - 1.0;
        hi = data.prev_close + 1.0;
    }
    let mut range = (data.prev_close - lo).max(hi - data.prev_close);
    if range < 1e-9 {
        range = 1.0;
    }
    range *= 1.06;
    let y_min = data.prev_close - range;
    let y_max = data.prev_close + range;

    let y_of = |price: f64| -> f32 {
        let t = ((price - y_min) / (y_max - y_min)) as f32;
        PAD_T + price_h * (1.0 - t)
    };
    let x_of = |i: usize| -> f32 { PAD_L + plot_w * (i as f32 / (n - 1) as f32) };

    // Horizontal grid + right-side price ticks
    for g in 0..5 {
        let gy = PAD_T + price_h * (g as f32 / 4.0);
        let mut path = PathBuilder::stroke(px(1.));
        path.move_to(point(origin.x + px(PAD_L), origin.y + px(gy)));
        path.line_to(point(origin.x + px(PAD_L + plot_w), origin.y + px(gy)));
        if let Ok(p) = path.build() {
            window.paint_path(p, data.border.opacity(0.35));
        }
        let mut tick = PathBuilder::stroke(px(1.));
        tick.move_to(point(origin.x + px(PAD_L + plot_w), origin.y + px(gy)));
        tick.line_to(point(origin.x + px(PAD_L + plot_w + 5.0), origin.y + px(gy)));
        if let Ok(p) = tick.build() {
            window.paint_path(p, data.axis_color.opacity(0.7));
        }
    }

    // Prev-close dashed baseline
    paint_dashed_hline(
        origin,
        PAD_L,
        PAD_L + plot_w,
        y_of(data.prev_close),
        data.axis_color.opacity(0.9),
        window,
    );

    // Price line (per-segment color vs prev close)
    for i in 0..n.saturating_sub(1) {
        let up = data.prices[i + 1] >= data.prev_close;
        let color = if up { data.bullish } else { data.bearish };
        let mut seg = PathBuilder::stroke(px(1.5));
        seg.move_to(point(origin.x + px(x_of(i)), origin.y + px(y_of(data.prices[i]))));
        seg.line_to(point(
            origin.x + px(x_of(i + 1)),
            origin.y + px(y_of(data.prices[i + 1])),
        ));
        if let Ok(p) = seg.build() {
            window.paint_path(p, color);
        }
    }

    // Average-price dashed line
    {
        let mut path = PathBuilder::stroke(px(1.));
        let mut started = false;
        for i in 0..n {
            let x = origin.x + px(x_of(i));
            let y = origin.y + px(y_of(data.avg[i]));
            if !started {
                path.move_to(point(x, y));
                started = true;
            } else {
                path.line_to(point(x, y));
            }
        }
        if let Ok(p) = path.build() {
            window.paint_path(p, data.avg_color.opacity(0.9));
        }
    }

    // Volume pane: per-minute bars colored vs prev close
    let vol_top = PAD_T + price_h + VOL_GAP;
    let mut sep = PathBuilder::stroke(px(1.));
    sep.move_to(point(origin.x + px(PAD_L), origin.y + px(vol_top - VOL_GAP * 0.5)));
    sep.line_to(point(
        origin.x + px(PAD_L + plot_w),
        origin.y + px(vol_top - VOL_GAP * 0.5),
    ));
    if let Ok(p) = sep.build() {
        window.paint_path(p, data.border.opacity(0.55));
    }
    let max_vol = data.volumes.iter().copied().max().unwrap_or(0).max(1) as f64;
    let slot = plot_w / n as f32;
    let body_w = (slot * 0.6).clamp(1.0, 8.0);
    for i in 0..n {
        let color = if data.prices[i] >= data.prev_close {
            data.bullish.opacity(0.55)
        } else {
            data.bearish.opacity(0.55)
        };
        let h = ((data.volumes[i] as f64 / max_vol) as f32 * vol_h).max(1.0);
        let top = vol_top + vol_h - h;
        let left = x_of(i) - body_w / 2.0;
        window.paint_quad(fill(
            Bounds {
                origin: point(origin.x + px(left), origin.y + px(top)),
                size: size(px(body_w), px(h)),
            },
            color,
        ));
    }

    // Crosshair
    if let Some(ix) = data.hover_ix {
        if ix < n {
            let cx = x_of(ix);
            let cy = y_of(data.prices[ix]);
            let full_bottom = PAD_T + price_h + VOL_GAP + vol_h;

            let mut vline = PathBuilder::stroke(px(1.));
            vline.move_to(point(origin.x + px(cx), origin.y + px(PAD_T)));
            vline.line_to(point(origin.x + px(cx), origin.y + px(full_bottom)));
            if let Ok(p) = vline.build() {
                window.paint_path(p, data.crosshair);
            }

            let mut hline = PathBuilder::stroke(px(1.));
            hline.move_to(point(origin.x + px(PAD_L), origin.y + px(cy)));
            hline.line_to(point(origin.x + px(PAD_L + plot_w), origin.y + px(cy)));
            if let Ok(p) = hline.build() {
                window.paint_path(p, data.crosshair);
            }

            window.paint_quad(fill(
                Bounds {
                    origin: point(origin.x + px(cx - 2.5), origin.y + px(cy - 2.5)),
                    size: size(px(5.), px(5.)),
                },
                data.crosshair,
            ));
        }
    }
}

/// Dashed horizontal line: 5px dash, 4px gap.
fn paint_dashed_hline(
    origin: gpui::Point<Pixels>,
    x0: f32,
    x1: f32,
    y: f32,
    color: Hsla,
    window: &mut Window,
) {
    const DASH: f32 = 5.0;
    const GAP: f32 = 4.0;
    let mut x = x0;
    while x < x1 {
        let seg_end = (x + DASH).min(x1);
        let mut path = PathBuilder::stroke(px(1.));
        path.move_to(point(origin.x + px(x), origin.y + px(y)));
        path.line_to(point(origin.x + px(seg_end), origin.y + px(y)));
        if let Ok(p) = path.build() {
            window.paint_path(p, color);
        }
        x = seg_end + GAP;
    }
}

fn paint_candles(
    candles: &[Candle],
    origin: gpui::Point<Pixels>,
    x_center: &dyn Fn(usize) -> f32,
    y_of: &dyn Fn(f64) -> f32,
    body_w: f32,
    data: &ChartPaintData,
    window: &mut Window,
) {
    for (i, c) in candles.iter().enumerate() {
        let cx = x_center(i);
        let color = if c.close >= c.open {
            data.bullish
        } else {
            data.bearish
        };
        let high_y = y_of(c.high);
        let low_y = y_of(c.low);
        let open_y = y_of(c.open);
        let close_y = y_of(c.close);

        let mut wick = PathBuilder::stroke(px(1.));
        wick.move_to(point(origin.x + px(cx), origin.y + px(high_y)));
        wick.line_to(point(origin.x + px(cx), origin.y + px(low_y)));
        if let Ok(p) = wick.build() {
            window.paint_path(p, color);
        }

        let top = open_y.min(close_y);
        let bot = open_y.max(close_y);
        let body_h = (bot - top).max(1.0);
        let left = cx - body_w / 2.0;
        window.paint_quad(fill(
            Bounds {
                origin: point(origin.x + px(left), origin.y + px(top)),
                size: size(px(body_w), px(body_h)),
            },
            color,
        ));
    }
}

/// Compact sparkline from close prices (work-mode metrics strip).
pub fn paint_sparkline(
    bounds: Bounds<Pixels>,
    closes: &[f64],
    line: Hsla,
    fill: Hsla,
    border: Hsla,
    window: &mut Window,
) {
    if closes.len() < 2 {
        return;
    }
    let width = bounds.size.width.as_f32().max(1.0);
    let height = bounds.size.height.as_f32().max(1.0);
    let pad_x = 4.0;
    let pad_y = 4.0;
    let plot_w = (width - pad_x * 2.0).max(1.0);
    let plot_h = (height - pad_y * 2.0).max(1.0);
    let origin = bounds.origin;

    let mut y_min = closes.iter().copied().fold(f64::MAX, f64::min);
    let mut y_max = closes.iter().copied().fold(f64::MIN, f64::max);
    if (y_max - y_min).abs() < 1e-9 {
        y_max += 1.0;
        y_min -= 1.0;
    }
    let y_of = |v: f64| -> f32 {
        let t = ((v - y_min) / (y_max - y_min)) as f32;
        pad_y + plot_h * (1.0 - t)
    };
    let x_of = |i: usize| -> f32 {
        pad_x + plot_w * (i as f32 / (closes.len() - 1) as f32)
    };

    // faint baseline
    let mut grid = PathBuilder::stroke(px(1.));
    let mid = pad_y + plot_h * 0.5;
    grid.move_to(point(origin.x + px(pad_x), origin.y + px(mid)));
    grid.line_to(point(origin.x + px(pad_x + plot_w), origin.y + px(mid)));
    if let Ok(p) = grid.build() {
        window.paint_path(p, border.opacity(0.25));
    }

    let mut fill_path = PathBuilder::fill();
    let base_y = pad_y + plot_h;
    fill_path.move_to(point(origin.x + px(x_of(0)), origin.y + px(base_y)));
    for (i, &v) in closes.iter().enumerate() {
        fill_path.line_to(point(origin.x + px(x_of(i)), origin.y + px(y_of(v))));
    }
    fill_path.line_to(point(
        origin.x + px(x_of(closes.len() - 1)),
        origin.y + px(base_y),
    ));
    fill_path.close();
    if let Ok(p) = fill_path.build() {
        window.paint_path(p, fill);
    }

    let mut line_path = PathBuilder::stroke(px(1.5));
    for (i, &v) in closes.iter().enumerate() {
        let pt = point(origin.x + px(x_of(i)), origin.y + px(y_of(v)));
        if i == 0 {
            line_path.move_to(pt);
        } else {
            line_path.line_to(pt);
        }
    }
    if let Ok(p) = line_path.build() {
        window.paint_path(p, line);
    }
}

fn paint_area_series(
    candles: &[Candle],
    origin: gpui::Point<Pixels>,
    x_center: &dyn Fn(usize) -> f32,
    y_of: &dyn Fn(f64) -> f32,
    baseline_y: f32,
    data: &ChartPaintData,
    window: &mut Window,
) {
    if candles.is_empty() {
        return;
    }

    // Filled area under close line
    let mut fill_path = PathBuilder::fill();
    let first_x = x_center(0);
    let first_y = y_of(candles[0].close);
    fill_path.move_to(point(origin.x + px(first_x), origin.y + px(baseline_y)));
    fill_path.line_to(point(origin.x + px(first_x), origin.y + px(first_y)));
    for (i, c) in candles.iter().enumerate().skip(1) {
        let x = x_center(i);
        let y = y_of(c.close);
        fill_path.line_to(point(origin.x + px(x), origin.y + px(y)));
    }
    let last_x = x_center(candles.len() - 1);
    fill_path.line_to(point(origin.x + px(last_x), origin.y + px(baseline_y)));
    fill_path.close();
    if let Ok(p) = fill_path.build() {
        window.paint_path(p, data.area_fill);
    }

    // Close line on top
    let mut line = PathBuilder::stroke(px(2.));
    for (i, c) in candles.iter().enumerate() {
        let x = origin.x + px(x_center(i));
        let y = origin.y + px(y_of(c.close));
        if i == 0 {
            line.move_to(point(x, y));
        } else {
            line.line_to(point(x, y));
        }
    }
    if let Ok(p) = line.build() {
        window.paint_path(p, data.line_color);
    }
}
