//! Interactive candlestick chart with MA overlays and crosshair.

use gpui::{
    fill, point, px, size, Bounds, Hsla, PathBuilder, Pixels, Window,
};
use gpui_component::PixelsExt;

use crate::data::indicators::MaSeries;
use crate::model::Candle;

#[derive(Clone)]
pub struct ChartPaintData {
    pub candles: Vec<Candle>,
    pub ma: MaSeries,
    pub show_ma5: bool,
    pub show_ma10: bool,
    pub show_ma20: bool,
    pub hover_ix: Option<usize>,
    pub bullish: Hsla,
    pub bearish: Hsla,
    pub border: Hsla,
    pub ma5_color: Hsla,
    pub ma10_color: Hsla,
    pub ma20_color: Hsla,
    pub crosshair: Hsla,
}

/// Map local X (relative to chart surface left) → candle index.
pub fn index_from_x(local_x: f32, width: f32, n: usize) -> Option<usize> {
    if n == 0 || width <= 0.0 {
        return None;
    }
    let pad_l = 8.0;
    let pad_r = 8.0;
    let plot_w = (width - pad_l - pad_r).max(1.0);
    let x = (local_x - pad_l).clamp(0.0, plot_w - 0.001);
    let ix = (x / plot_w * n as f32).floor() as usize;
    Some(ix.min(n - 1))
}

pub fn paint_chart(bounds: Bounds<Pixels>, data: &ChartPaintData, window: &mut Window) {
    let candles = &data.candles;
    if candles.is_empty() {
        return;
    }

    let width = bounds.size.width.as_f32().max(1.0);
    let height = bounds.size.height.as_f32().max(1.0);
    let pad_l = 8.0;
    let pad_r = 8.0;
    let pad_t = 12.0;
    let pad_b = 22.0;
    let plot_w = (width - pad_l - pad_r).max(1.0);
    let plot_h = (height - pad_t - pad_b).max(1.0);
    let origin = bounds.origin;

    let mut y_min = candles.iter().map(|c| c.low).fold(f64::MAX, f64::min);
    let mut y_max = candles.iter().map(|c| c.high).fold(f64::MIN, f64::max);
    for series in [
        data.ma.ma5.as_slice(),
        data.ma.ma10.as_slice(),
        data.ma.ma20.as_slice(),
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
        pad_t + plot_h * (1.0 - t)
    };
    let x_center = |i: usize| -> f32 { pad_l + slot * (i as f32 + 0.5) };

    // Horizontal grid
    for g in 0..5 {
        let gy = pad_t + plot_h * (g as f32 / 4.0);
        let mut path = PathBuilder::stroke(px(1.));
        path.move_to(point(origin.x + px(pad_l), origin.y + px(gy)));
        path.line_to(point(origin.x + px(pad_l + plot_w), origin.y + px(gy)));
        if let Ok(p) = path.build() {
            window.paint_path(p, data.border.opacity(0.35));
        }
    }

    // MA lines
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

    // Candles
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

    // Crosshair
    if let Some(ix) = data.hover_ix {
        if ix < candles.len() {
            let cx = x_center(ix);
            let c = &candles[ix];
            let cy = y_of(c.close);

            let mut vline = PathBuilder::stroke(px(1.));
            vline.move_to(point(origin.x + px(cx), origin.y + px(pad_t)));
            vline.line_to(point(origin.x + px(cx), origin.y + px(pad_t + plot_h)));
            if let Ok(p) = vline.build() {
                window.paint_path(p, data.crosshair);
            }

            let mut hline = PathBuilder::stroke(px(1.));
            hline.move_to(point(origin.x + px(pad_l), origin.y + px(cy)));
            hline.line_to(point(origin.x + px(pad_l + plot_w), origin.y + px(cy)));
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
