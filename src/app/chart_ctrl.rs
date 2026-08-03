//! Chart paint data, formatting helpers, drawing anchors.

#![allow(unused_imports)]

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    canvas, div, point, px, size, App, AppContext, Bounds, Context, Entity, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent, SharedString,
    StatefulInteractiveElement, Styled, Timer, Window, WindowBounds, WindowOptions,
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

use super::{
    AiCacheEntry, AiPanelState, AiSource, ChartKind, ChartRange, DetailTab, LeftTab, SettingsSection,
    StockApp, CHART_MIN_VISIBLE, QUOTE_INTERVAL_ERR_MAX, QUOTE_INTERVAL_PRESETS, TITLE_NORMAL,
    TITLE_WORK, TREASURE_SCAN_GAP,
};
use super::helpers::*;



impl StockApp {
    pub(crate) fn chart_paint_data(&self, cx: &App) -> ChartPaintData {
        let theme = cx.theme();
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        let minute_matched = matches!(self.chart_kind, ChartKind::Intraday)
            && self
                .minute_code
                .as_ref()
                .is_some_and(|c| c == self.selected.as_ref());
        // While loading a new series, keep painting the previous candles to avoid a blank flash.
        let show_series = if matches!(self.chart_kind, ChartKind::Intraday) {
            minute_matched && matched
        } else {
            matched || (self.loading && !self.candles.is_empty())
        };
        let (start, end) = if show_series {
            self.chart_visible_range()
        } else {
            (0, 0)
        };
        let candles = if show_series && end > start {
            self.candles[start..end].to_vec()
        } else {
            Vec::new()
        };
        let ma = if show_series && end > start {
            self.ma.slice(start, end)
        } else {
            MaSeries::default()
        };
        let work = self.work_mode;
        let is_intraday = matches!(self.chart_kind, ChartKind::Intraday);
        let macd = if show_series && end > start && !is_intraday && self.show_macd && !work {
            let s = self.macd.slice(start, end);
            Some(MacdPaintData {
                dif: s.dif,
                dea: s.dea,
                hist: s.hist,
                dif_color: theme.foreground,
                dea_color: theme.blue,
                axis_color: theme.muted_foreground,
                bullish: self.chg_color(true, cx),
                bearish: self.chg_color(false, cx),
            })
        } else {
            None
        };
        let boll = if show_series && end > start && !is_intraday && self.show_boll && !work {
            let s = self.boll.slice(start, end);
            Some(BollPaintData {
                upper: s.upper,
                mid: s.mid,
                lower: s.lower,
                upper_color: theme.cyan.opacity(0.9),
                mid_color: theme.muted_foreground.opacity(0.8),
                lower_color: theme.magenta.opacity(0.9),
            })
        } else {
            None
        };
        // 画线：当前标的的线（裁剪到可见区间，保持锚点索引为切片内坐标）。
        let mut lines = Vec::new();
        if show_series && end > start && !work {
            let owned = self
                .chart_lines
                .get(self.selected.as_ref())
                .cloned()
                .unwrap_or_default();
            for line in owned {
                if line.from.0 < start || line.to.0 < start {
                    continue;
                }
                if line.from.0 >= end || line.to.0 >= end {
                    continue;
                }
                lines.push(TrendLine {
                    from: (line.from.0 - start, line.from.1),
                    to: (line.to.0 - start, line.to.1),
                    color_ix: line.color_ix,
                });
            }
            if let Some(draft) = self.draft_line {
                if draft.from.0 >= start && draft.from.0 < end && draft.to.0 >= start && draft.to.0 < end {
                    lines.push(TrendLine {
                        from: (draft.from.0 - start, draft.from.1),
                        to: (draft.to.0 - start, draft.to.1),
                        color_ix: draft.color_ix,
                    });
                }
            }
        }
        // Open position average cost (hidden in work mode).
        let cost_line = if show_series && !work {
            self.portfolio
                .position_of(self.selected.as_ref())
                .filter(|p| p.is_open() && p.avg_cost.is_finite() && p.avg_cost > 0.0)
                .map(|p| p.avg_cost)
        } else {
            None
        };
        let cost_line_color = theme.yellow.opacity(0.85);
        let hover_ix = if matched {
            self.hover_ix.and_then(|ix| {
                if ix >= start && ix < end {
                    Some(ix - start)
                } else {
                    None
                }
            })
        } else {
            None
        };
        let minute = if matches!(self.chart_kind, ChartKind::Intraday) {
            self.minute_paint_data(cx)
        } else {
            None
        };
        ChartPaintData {
            candles,
            ma,
            macd,
            boll,
            lines,
            line_colors: chart_line_palette(theme),
            cost_line,
            cost_line_color,
            minute,
            show_ma5: self.show_ma5,
            show_ma10: self.show_ma10,
            show_ma20: self.show_ma20,
            show_ma60: self.show_ma60,
            show_volume: self.show_volume && !work,
            hover_ix,
            style: if work {
                ChartStyle::Area
            } else {
                ChartStyle::Candles
            },
            bullish: self.chg_color(true, cx),
            bearish: self.chg_color(false, cx),
            line_color: if work {
                theme.blue
            } else {
                theme.foreground
            },
            area_fill: theme.blue.opacity(0.18),
            border: theme.border,
            ma5_color: if work {
                theme.muted_foreground.opacity(0.85)
            } else {
                theme.yellow
            },
            ma10_color: if work {
                theme.muted_foreground.opacity(0.65)
            } else {
                theme.blue
            },
            ma20_color: if work {
                theme.muted_foreground.opacity(0.45)
            } else {
                theme.magenta
            },
            ma60_color: if work {
                theme.muted_foreground.opacity(0.35)
            } else {
                theme.cyan
            },
            crosshair: theme.muted_foreground.opacity(0.7),
            axis_color: theme.muted_foreground,
        }
    }

    pub(crate) fn minute_paint_data(&self, cx: &App) -> Option<MinutePaintData> {
        let theme = cx.theme();
        let matched = self
            .minute_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref())
            && self
                .candles_code
                .as_ref()
                .is_some_and(|c| c == self.selected.as_ref());
        if !matched {
            return None;
        }
        let m = self.minute.as_ref()?;
        if m.is_empty() {
            return None;
        }
        let (start, end) = self.chart_visible_range();
        if start >= end || end > m.points.len() {
            return None;
        }
        let mut prices = Vec::with_capacity(end - start);
        let mut avg = Vec::with_capacity(end - start);
        let mut volumes = Vec::with_capacity(end - start);
        for i in start..end {
            let p = &m.points[i];
            prices.push(p.price);
            avg.push(p.avg_price());
            volumes.push(p.minute_volume(i.checked_sub(1).map(|j| &m.points[j])));
        }
        let hover_ix = self.hover_ix.and_then(|ix| {
            if ix >= start && ix < end {
                Some(ix - start)
            } else {
                None
            }
        });
        Some(MinutePaintData {
            prices,
            avg,
            volumes,
            prev_close: m.prev_close,
            hover_ix,
            bullish: self.chg_color(true, cx),
            bearish: self.chg_color(false, cx),
            avg_color: if self.work_mode {
                theme.muted_foreground.opacity(0.85)
            } else {
                theme.yellow
            },
            border: theme.border,
            crosshair: theme.muted_foreground.opacity(0.7),
            axis_color: theme.muted_foreground,
        })
    }

    /// Display id: real code, or stable camouflage label in work mode.
    pub(crate) fn display_code(&self, code: &str) -> String {
        if self.work_mode {
            let name = self
                .symbols
                .iter()
                .find(|s| s.code == code)
                .map(|s| s.name.as_ref())
                .unwrap_or("");
            if self.work_identity_reveal {
                if is_real_name(name, code) {
                    format!("{name} · {code}")
                } else {
                    code.to_string()
                }
            } else {
                disguise_label(code, name)
            }
        } else {
            code.to_string()
        }
    }

    pub(crate) fn apply_index_ticks(&mut self, ticks: &[(String, String, f64, f64)]) {
        for (code, name, last, change_pct) in ticks {
            let snap = IndexSnap {
                last: *last,
                change_pct: *change_pct,
            };
            let n = name.as_str();
            if n.contains("上证") || (*code == "000001" && n.contains("指数")) {
                self.index_sh = Some(snap);
            } else if n.contains("沪深300") || code == "000300" {
                self.index_hs300 = Some(snap);
            } else if n.contains("创业板") || code == "399006" {
                self.index_cyb = Some(snap);
            } else if code == "000001" && *last > 1000.0 {
                // 上证点位通常 >1000；个股平安银行不会这么高
                self.index_sh = Some(snap);
            }
        }
    }

    /// Price base for index rebased display (first visible close, else last).
    pub(crate) fn price_base(&self) -> f64 {
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        if matched {
            let (start, end) = self.chart_visible_range();
            if end > start {
                if let Some(c) = self.candles.get(start) {
                    if c.close > 0.0 {
                        return c.close;
                    }
                }
            }
            if let Some(c) = self.candles.last() {
                if c.close > 0.0 {
                    return c.close;
                }
            }
        }
        self.current_symbol()
            .map(|s| s.last)
            .filter(|v| *v > 0.0)
            .unwrap_or(1.0)
    }

    pub(crate) fn format_value(&self, price: f64) -> String {
        if self.work_mode {
            format_index(disguise_index(price, self.price_base()))
        } else {
            format_price(price)
        }
    }

    pub(crate) fn format_change(&self, pct: f64) -> String {
        if self.work_mode {
            // Show as index points vs 100, not a stock-style %.
            format!("{pct:+.2}")
        } else {
            format_pct(pct)
        }
    }

    /// Work-mode status never mentions quotes / vendors / Chinese stock jargon.
    pub(crate) fn work_status_line(&self) -> String {
        if self.loading {
            return "loading series…".into();
        }
        if self.treasure_scanning {
            return format!("job {}/{}", self.treasure_done, self.treasure_total);
        }
        let t = chrono::Local::now().format("%H:%M:%S");
        if self.quote_fail_streak > 0 {
            return format!("sync retry · {t}");
        }
        format!("sync ok · src-a · {t}")
    }

    pub(crate) fn max_watchlist_volume(&self) -> u64 {
        self.symbols.iter().map(|s| s.volume).max().unwrap_or(0)
    }

    /// 0..1 volume share vs busiest row (looks like load, not 手/万).
    pub(crate) fn load_factor(volume: u64, max_vol: u64) -> f64 {
        if max_vol == 0 {
            0.0
        } else {
            (volume as f64 / max_vol as f64).clamp(0.0, 1.0)
        }
    }

    /// |涨跌幅| → CPU%（波动大 = 更忙）。约 0%→8%，5%→48%，10%→88%。
    pub(crate) fn sys_cpu_pct(change_pct: f64) -> f64 {
        (8.0 + change_pct.abs() * 8.0).clamp(3.0, 96.0)
    }

    /// 成交量 → 假网速 MB/s（看起来像网络吞吐）。
    pub(crate) fn sys_net_mbs(volume: u64, max_vol: u64) -> f64 {
        let load = Self::load_factor(volume, max_vol);
        (0.4 + load * 24.0 + (volume % 97) as f64 * 0.02).clamp(0.2, 48.0)
    }

    /// RSS MB from volume + stable code salt.
    pub(crate) fn sys_rss_mb(code: &str, volume: u64, max_vol: u64) -> u32 {
        let salt = code.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
        let base = 48 + (salt % 180);
        let vol = (Self::load_factor(volume, max_vol) * 720.0) as u32;
        base + vol
    }

    pub(crate) fn current_signal(&self) -> Option<signals::SignalSnapshot> {
        self.candles_code
            .as_ref()
            .is_some_and(|code| code == self.selected.as_ref())
            .then(|| signals::analyze(&self.candles))
            .flatten()
    }

    pub(crate) fn spark_closes(&self) -> Vec<f64> {
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == self.selected.as_ref());
        if !matched || self.candles.is_empty() {
            return Vec::new();
        }
        let (start, end) = self.chart_visible_range();
        let slice = if end > start {
            &self.candles[start..end]
        } else {
            self.candles.as_slice()
        };
        // Cap points so the spark stays readable.
        let n = slice.len();
        let step = (n / 80).max(1);
        slice
            .iter()
            .step_by(step)
            .map(|c| c.close)
            .collect()
    }

}
