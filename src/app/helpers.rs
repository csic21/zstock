//! Pure UI/string helpers shared across app modules.

use gpui::{
    App, Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, div, prelude::FluentBuilder, px,
};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme, StyledExt, h_flex, v_flex};

use crate::model::{Symbol, disguise_label, format_pct, format_price, shared};
use crate::storage::ColorScheme;

use super::StockApp;

pub(crate) fn format_candle_date(raw: &str) -> String {
    let s = raw.trim();
    if s.len() >= 10 && s.as_bytes().get(4) == Some(&b'-') {
        if s.len() > 10 && s.as_bytes().get(10) == Some(&b' ') {
            // minute bar: `2026-07-31 15:00` → `07-31 15:00`
            format!("{}{}", &s[5..10], &s[10..])
        } else {
            // 2026-07-29 → keep ISO; also accept already short forms
            s[..10].to_string()
        }
    } else if s.len() == 5 && s.contains('/') {
        // legacy MM/DD — cannot recover year
        s.to_string()
    } else {
        s.to_string()
    }
}

/// Line color palette for user-drawn chart lines (cycles by `color_ix`).
pub(crate) fn chart_line_palette(theme: &gpui_component::Theme) -> Vec<gpui::Hsla> {
    vec![
        theme.yellow,
        theme.cyan,
        theme.magenta,
        theme.blue,
        theme.green,
        theme.red,
        theme.accent,
    ]
}

pub(crate) struct PaletteRowOptions {
    pub(crate) in_watchlist: bool,
    pub(crate) row_id: u64,
    pub(crate) highlighted: bool,
    pub(crate) color_scheme: ColorScheme,
    pub(crate) work_mode: bool,
    pub(crate) reveal_identity: bool,
}

pub(crate) fn palette_row(
    sym: Symbol,
    options: PaletteRowOptions,
    cx: &mut Context<StockApp>,
) -> impl IntoElement {
    use super::labels::L;
    let PaletteRowOptions {
        in_watchlist,
        row_id,
        highlighted,
        color_scheme,
        work_mode,
        reveal_identity,
    } = options;
    let code = sym.code.clone();
    let name = sym.name.to_string();
    let code_width = if work_mode && reveal_identity {
        180.0
    } else {
        64.0
    };
    let code_show = if work_mode && reveal_identity {
        if is_real_name(sym.name.as_ref(), &sym.code) {
            format!("{} · {}", sym.name, sym.code)
        } else {
            sym.code.clone()
        }
    } else if work_mode {
        disguise_label(&sym.code, sym.name.as_ref())
    } else {
        sym.code.clone()
    };
    let name_show = if work_mode {
        shared("series")
    } else {
        sym.name.clone()
    };
    let board = if work_mode {
        shared("svc")
    } else {
        sym.board.clone()
    };
    let last = if work_mode {
        if sym.last > 0.0 {
            format!("{}ms", format_price(sym.last))
        } else {
            "--".into()
        }
    } else {
        format_price(sym.last)
    };
    let chg = if work_mode {
        format!("{:+.2}", sym.change_pct)
    } else {
        format_pct(sym.change_pct)
    };
    let up = sym.is_up();
    let chg_color = if work_mode {
        if up {
            cx.theme().muted_foreground
        } else {
            cx.theme().muted_foreground.opacity(0.65)
        }
    } else {
        match color_scheme {
            ColorScheme::Cn => {
                if up {
                    cx.theme().red
                } else {
                    cx.theme().green
                }
            }
            ColorScheme::Us => {
                if up {
                    cx.theme().green
                } else {
                    cx.theme().red
                }
            }
        }
    };

    div()
        .id(("palette-item", row_id))
        .h(px(40.))
        .px_3()
        .rounded(cx.theme().radius)
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .when(highlighted, |this| this.bg(cx.theme().accent.opacity(0.22)))
        .hover(|this| this.bg(cx.theme().accent.opacity(0.15)))
        .on_click(cx.listener(move |this, _, window, cx| {
            if in_watchlist {
                this.select_symbol(shared(code.clone()), cx);
            } else {
                this.add_symbol(code.clone(), name.clone(), window, cx);
            }
        }))
        .child(
            div()
                .w(px(code_width))
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(code_show),
        )
        .child(
            div()
                .flex_1()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .truncate()
                .child(name_show),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(board),
        )
        .when(in_watchlist, |this| {
            this.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(last),
            )
            .child(
                div()
                    .w(px(64.))
                    .text_right()
                    .text_xs()
                    .text_color(chg_color)
                    .child(chg),
            )
        })
        .when(!in_watchlist, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().accent)
                    .child(L::palette_add(work_mode)),
            )
        })
}

/// Host metric bar: shows `value_text` (e.g. +0.52%), bar fill 0–100, hover → real index.
pub(crate) fn sys_gauge(
    id: u64,
    label: &str,
    value_text: String,
    bar_pct: f64,
    tooltip_text: String,
    cx: &App,
) -> impl IntoElement {
    let bar_pct = bar_pct.clamp(0.0, 100.0);
    let fill_w = (bar_pct / 100.0 * 140.0) as f32;
    let tip = tooltip_text.clone();
    h_flex()
        .id(("sys-gauge", id))
        .w_full()
        .items_center()
        .gap_2()
        .cursor_default()
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(
            div()
                .w(px(36.))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .h(px(8.))
                .rounded_full()
                .bg(cx.theme().muted.opacity(0.55))
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(px(fill_w))
                        .rounded_full()
                        .bg(cx.theme().blue.opacity(0.85)),
                ),
        )
        .child(
            div()
                .w(px(112.))
                .whitespace_nowrap()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(value_text),
        )
}

pub(crate) fn detail_row(label: &str, value: &str, cx: &App) -> impl IntoElement {
    detail_kv(label, value, cx)
}

/// Label/value row with room for Chinese multi-char keys.
pub(crate) fn detail_kv(label: &str, value: &str, cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            div()
                .w(px(88.))
                .flex_shrink_0()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(value.to_string()),
        )
}

pub(crate) fn section_title(text: &str, cx: &App) -> impl IntoElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
}

/// 解析用户输入的金额/股数（允许千分位逗号、空白）。
pub(crate) fn parse_f64(raw: &str) -> Option<f64> {
    let s = raw.trim().replace([',', '，'], "");
    if s.is_empty() {
        return None;
    }
    let v: f64 = s.parse().ok()?;
    if v.is_finite() { Some(v) } else { None }
}

/// Compact metric pill used on the overview / treasure dashboards.
pub(crate) fn metric_chip(label: &str, value: &str, cx: &App) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .px_2()
        .py_1()
        .min_w(px(72.))
        .rounded(cx.theme().radius)
        .bg(cx.theme().background)
        .border_1()
        .border_color(cx.theme().border)
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(cx.theme().foreground)
                .child(value.to_string()),
        )
}

/// 是否为可用的中文/展示名称（排除空、占位、与代码相同）。
pub(crate) fn is_real_name(name: &str, code: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n != "--" && n != code && n != code.trim_start_matches('0')
}

pub(crate) fn display_name_str(name: &str, code: &str) -> String {
    if is_real_name(name, code) {
        name.trim().to_string()
    } else {
        String::new()
    }
}

/// Strip common fund/ETF suffixes so long names can be shortened cleanly.
/// e.g. `华泰柏瑞沪深300ETF` → `华泰柏瑞沪深300`
pub(crate) fn strip_fund_suffix(name: &str) -> &str {
    let n = name.trim();
    for suffix in ["ETF联接", "ETF", "LOF", "基金"] {
        if let Some(rest) = n.strip_suffix(suffix) {
            let rest = rest.trim_end();
            if !rest.is_empty() {
                return rest;
            }
        }
    }
    n
}

/// Compact label for the macOS menu bar (space is tight).
/// Keeps a real Chinese/name fragment even for long ETF titles — never
/// collapses a known name to the bare code (that produced `588710 588710`).
pub(crate) fn short_status_name(name: &str, code: &str) -> String {
    if !is_real_name(name, code) {
        return code.to_string();
    }
    let n = strip_fund_suffix(name);
    let chars: Vec<char> = n.chars().collect();
    // ≤6: show full (most stocks + short fund nicknames).
    // Longer: keep first 4 chars (e.g. 华泰柏瑞沪深300 → 华泰柏瑞).
    if chars.len() <= 6 {
        n.to_string()
    } else {
        chars.into_iter().take(4).collect()
    }
}

#[cfg(test)]
mod name_label_tests {
    use super::{is_real_name, short_status_name, strip_fund_suffix};

    #[test]
    fn strip_etf_suffix() {
        assert_eq!(strip_fund_suffix("华泰柏瑞沪深300ETF"), "华泰柏瑞沪深300");
        assert_eq!(strip_fund_suffix("科创板50ETF"), "科创板50");
        assert_eq!(strip_fund_suffix("比亚迪"), "比亚迪");
    }

    #[test]
    fn short_name_keeps_etf_fragment() {
        // Long ETF names must NOT collapse to bare code (was: "588710 588710").
        let s = short_status_name("华泰柏瑞沪深300ETF", "510300");
        assert_ne!(s, "510300");
        assert!(s.chars().count() <= 6);
        assert!(!s.is_empty());
    }

    #[test]
    fn short_name_keeps_stock() {
        assert_eq!(short_status_name("比亚迪", "002594"), "比亚迪");
        assert_eq!(short_status_name("贵州茅台", "600519"), "贵州茅台");
    }

    #[test]
    fn missing_name_falls_back_to_code() {
        assert_eq!(short_status_name("", "588710"), "588710");
        assert_eq!(short_status_name("588710", "588710"), "588710");
        assert!(!is_real_name("588710", "588710"));
    }
}
