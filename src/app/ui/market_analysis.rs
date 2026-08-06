//! Full-page market overview and A-share sector analysis.

use chrono::Local;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, canvas, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::chart::paint_sparkline;
use crate::data::market::SectorTick;
use crate::data::session::MarketId;
use crate::model::IndexSnap;

use super::super::{MarketRegion, StockApp};

impl StockApp {
    pub(crate) fn render_market_analysis(
        &self,
        _window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let region = self.market_analysis_region;
        let sectors = self.market_analysis_sectors.clone();
        let sector_total = sectors.len();
        let sector_advances = sectors.iter().filter(|s| s.change_pct > 0.0).count();
        let sector_declines = sectors.iter().filter(|s| s.change_pct < 0.0).count();
        let sector_unchanged = sector_total.saturating_sub(sector_advances + sector_declines);
        let stock_advances: u64 = sectors.iter().map(|s| s.advances).sum();
        let stock_declines: u64 = sectors.iter().map(|s| s.declines).sum();
        let stock_unchanged: u64 = sectors.iter().map(|s| s.unchanged).sum();
        let stock_total = stock_advances + stock_declines + stock_unchanged;
        let average_change = if sector_total == 0 {
            None
        } else {
            Some(sectors.iter().map(|s| s.change_pct).sum::<f64>() / sector_total as f64)
        };
        let strongest = sectors
            .iter()
            .max_by(|a, b| a.change_pct.total_cmp(&b.change_pct));
        let weakest = sectors
            .iter()
            .min_by(|a, b| a.change_pct.total_cmp(&b.change_pct));
        let market_open = MarketId::CnA.is_open_at(Local::now());
        let source = self.market_analysis_source.clone();
        let updated = self
            .market_analysis_updated
            .clone()
            .unwrap_or_else(|| "等待数据".into());
        let status_color = if market_open {
            cx.theme().green
        } else {
            cx.theme().muted_foreground
        };
        let total_for_meter = stock_total.max(1) as f32;
        let meter_w = 288.0;
        let up_w = (stock_advances as f32 / total_for_meter * meter_w).clamp(0.0, meter_w);
        let flat_w = (stock_unchanged as f32 / total_for_meter * meter_w).clamp(0.0, meter_w);
        let down_w = (stock_declines as f32 / total_for_meter * meter_w).clamp(0.0, meter_w);

        v_flex()
            .id("market-analysis-page")
            .debug_selector(|| "market-analysis-page-root".into())
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .h(px(52.))
                    .flex_shrink_0()
                    .px_4()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .child(
                        Button::new("market-analysis-back")
                            .ghost()
                            .xsmall()
                            .label("← 返回行情")
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.close_market_analysis(cx);
                            })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("市场分析"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("区域市场 · 指数与板块热度"),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(status_color)
                                    .child(if market_open {
                                        "● 交易中"
                                    } else {
                                        "○ 已收盘"
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("更新于 {updated}")),
                            )
                            .child(
                                Button::new("market-analysis-refresh")
                                    .xsmall()
                                    .when(self.market_analysis_loading, |b| b.primary())
                                    .when(!self.market_analysis_loading, |b| b.ghost())
                                    .label(if self.market_analysis_loading {
                                        "刷新中…"
                                    } else {
                                        "刷新"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.refresh_market_analysis(cx);
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .id("market-analysis-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .p_5()
                    .child(
                        v_flex()
                            .w_full()
                            .max_w(px(1280.))
                            .gap(px(16.))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("市场区域"),
                                    )
                                    .child(
                                        Button::new("market-region-a")
                                            .xsmall()
                                            .primary()
                                            .label(MarketRegion::AShare.label())
                                            .on_click(cx.listener(|this, _, _w, cx| {
                                                this.set_market_analysis_region(
                                                    MarketRegion::AShare,
                                                    cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new("market-region-hk")
                                            .xsmall()
                                            .ghost()
                                            .label(format!(
                                                "{}（接入中）",
                                                MarketRegion::Hk.label()
                                            ))
                                            .tooltip("港股市场分析即将接入"),
                                    )
                                    .child(
                                        Button::new("market-region-us")
                                            .xsmall()
                                            .ghost()
                                            .label(format!(
                                                "{}（接入中）",
                                                MarketRegion::Us.label()
                                            ))
                                            .tooltip("美股市场分析即将接入"),
                                    )
                                    .child(
                                        div()
                                            .ml_2()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(
                                                "{} · {} · 数据源 {}",
                                                Local::now().format("%Y-%m-%d"),
                                                region.label(),
                                                source
                                            )),
                                    ),
                            )
                            .child(
                                h_flex().gap(px(10.)).flex_wrap().children([
                                    self.render_analysis_stat(
                                        "市场状态",
                                        if market_open { "盘中" } else { "收盘后" },
                                        if market_open {
                                            "A股交易时段"
                                        } else {
                                            "等待下个交易时段"
                                        },
                                        status_color,
                                        cx,
                                    ),
                                    self.render_analysis_stat(
                                        "行业涨跌",
                                        &format!("↑ {} · ↓ {}", sector_advances, sector_declines),
                                        &format!(
                                            "共 {} 个行业板块，平 {}",
                                            sector_total, sector_unchanged
                                        ),
                                        cx.theme().foreground,
                                        cx,
                                    ),
                                    self.render_analysis_stat(
                                        "成分股涨跌",
                                        &format!("↑ {} · ↓ {}", stock_advances, stock_declines),
                                        &format!("接口合计 {} 只", stock_total),
                                        cx.theme().foreground,
                                        cx,
                                    ),
                                    self.render_analysis_stat(
                                        "板块平均",
                                        &average_change
                                            .map(|v| format!("{v:+.2}%"))
                                            .unwrap_or_else(|| "--".into()),
                                        "行业涨跌幅均值",
                                        average_change
                                            .map(|v| self.chg_color(v >= 0.0, cx))
                                            .unwrap_or(cx.theme().muted_foreground),
                                        cx,
                                    ),
                                    self.render_analysis_stat(
                                        "最强方向",
                                        strongest.map(|s| s.name.as_str()).unwrap_or("等待数据"),
                                        &strongest
                                            .map(|s| format!("{:+.2}%", s.change_pct))
                                            .unwrap_or_else(|| "板块数据加载中".into()),
                                        strongest
                                            .map(|s| self.chg_color(s.change_pct >= 0.0, cx))
                                            .unwrap_or(cx.theme().muted_foreground),
                                        cx,
                                    ),
                                ]),
                            )
                            .child(
                                v_flex()
                                    .gap_3()
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .justify_between()
                                            .child(self.render_analysis_section_title(
                                                "大盘指数",
                                                "主要指数最新快照",
                                                cx,
                                            ))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("上证 · 沪深300 · 创业板"),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .gap(px(10.))
                                            .flex_wrap()
                                            .child(self.render_index_card(
                                                "上证综指",
                                                "000001",
                                                self.index_sh,
                                                cx,
                                            ))
                                            .child(self.render_index_card(
                                                "沪深300",
                                                "000300",
                                                self.index_hs300,
                                                cx,
                                            ))
                                            .child(self.render_index_card(
                                                "创业板指",
                                                "399006",
                                                self.index_cyb,
                                                cx,
                                            )),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap(px(16.))
                                    .items_start()
                                    .flex_wrap()
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w(px(520.))
                                            .gap_3()
                                            .child(self.render_sector_panel(
                                                "板块热度",
                                                "行业涨跌幅排名 · 盘中实时",
                                                sectors.clone(),
                                                true,
                                                cx,
                                            ))
                                            .child(self.render_sector_panel(
                                                "弱势板块",
                                                "需要留意的回撤方向",
                                                sectors.clone(),
                                                false,
                                                cx,
                                            )),
                                    )
                                    .child(
                                        v_flex()
                                            .w(px(320.))
                                            .min_w(px(280.))
                                            .gap_3()
                                            .child(
                                                v_flex()
                                                    .gap_3()
                                                    .p_4()
                                                    .rounded(cx.theme().radius)
                                                    .border_1()
                                                    .border_color(cx.theme().border)
                                                    .bg(cx.theme().sidebar)
                                                    .child(self.render_analysis_section_title(
                                                        "市场宽度",
                                                        "行业成分股涨跌分布",
                                                        cx,
                                                    ))
                                                    .child(
                                                        div()
                                                            .h(px(10.))
                                                            .w_full()
                                                            .rounded_full()
                                                            .overflow_hidden()
                                                            .bg(cx.theme().muted)
                                                            .child(
                                                                h_flex()
                                                                    .h_full()
                                                                    .child(
                                                                        div()
                                                                            .h_full()
                                                                            .w(px(up_w))
                                                                            .bg(cx.theme().red),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .h_full()
                                                                            .w(px(flat_w))
                                                                            .bg(cx
                                                                                .theme()
                                                                                .muted_foreground
                                                                                .opacity(0.45)),
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .h_full()
                                                                            .w(px(down_w))
                                                                            .bg(cx.theme().green),
                                                                    ),
                                                            ),
                                                    )
                                                    .child(
                                                        h_flex()
                                                            .justify_between()
                                                            .text_xs()
                                                            .child(
                                                                div()
                                                                    .text_color(cx.theme().red)
                                                                    .child(format!(
                                                                        "上涨个股 {}",
                                                                        stock_advances
                                                                    )),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_color(
                                                                        cx.theme().muted_foreground,
                                                                    )
                                                                    .child(format!(
                                                                        "平盘 {}",
                                                                        stock_unchanged
                                                                    )),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_color(cx.theme().green)
                                                                    .child(format!(
                                                                        "下跌个股 {}",
                                                                        stock_declines
                                                                    )),
                                                            ),
                                                    )
                                                    .child(
                                                        v_flex()
                                                            .gap_2()
                                                            .pt_3()
                                                            .border_t_1()
                                                            .border_color(cx.theme().border)
                                                            .child(
                                                                self.render_analysis_metric_row(
                                                                    "强势行业",
                                                                    strongest
                                                                        .map(|s| s.name.as_str())
                                                                        .unwrap_or("--"),
                                                                    strongest
                                                                        .map(|s| {
                                                                            format!(
                                                                                "{:+.2}%",
                                                                                s.change_pct
                                                                            )
                                                                        })
                                                                        .unwrap_or_else(|| {
                                                                            "--".into()
                                                                        }),
                                                                    strongest
                                                                        .map(|s| {
                                                                            self.chg_color(
                                                                                s.change_pct >= 0.0,
                                                                                cx,
                                                                            )
                                                                        })
                                                                        .unwrap_or(
                                                                            cx.theme()
                                                                                .muted_foreground,
                                                                        ),
                                                                    cx,
                                                                ),
                                                            )
                                                            .child(
                                                                self.render_analysis_metric_row(
                                                                    "弱势行业",
                                                                    weakest
                                                                        .map(|s| s.name.as_str())
                                                                        .unwrap_or("--"),
                                                                    weakest
                                                                        .map(|s| {
                                                                            format!(
                                                                                "{:+.2}%",
                                                                                s.change_pct
                                                                            )
                                                                        })
                                                                        .unwrap_or_else(|| {
                                                                            "--".into()
                                                                        }),
                                                                    weakest
                                                                        .map(|s| {
                                                                            self.chg_color(
                                                                                s.change_pct >= 0.0,
                                                                                cx,
                                                                            )
                                                                        })
                                                                        .unwrap_or(
                                                                            cx.theme()
                                                                                .muted_foreground,
                                                                        ),
                                                                    cx,
                                                                ),
                                                            ),
                                                    ),
                                            )
                                            .child(
                                                v_flex()
                                                    .gap_3()
                                                    .p_4()
                                                    .rounded(cx.theme().radius)
                                                    .border_1()
                                                    .border_color(cx.theme().border)
                                                    .bg(cx.theme().sidebar)
                                                    .child(self.render_analysis_section_title(
                                                        "分析提示",
                                                        "基于行业与成分股快照的快速判断",
                                                        cx,
                                                    ))
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .text_color(cx.theme().foreground)
                                                            .child(self.analysis_summary(
                                                                average_change,
                                                                stock_advances,
                                                                stock_declines,
                                                                stock_total,
                                                                strongest,
                                                            )),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("仅作行情观察，不构成投资建议"),
                                                    ),
                                            ),
                                    ),
                            )
                            .when_some(self.market_analysis_error.clone(), |col, error| {
                                col.child(
                                    div()
                                        .p_3()
                                        .rounded(cx.theme().radius)
                                        .border_1()
                                        .border_color(cx.theme().red.opacity(0.35))
                                        .bg(cx.theme().red.opacity(0.08))
                                        .text_xs()
                                        .text_color(cx.theme().red)
                                        .child(error),
                                )
                            })
                            .when(self.market_analysis_loading && sectors.is_empty(), |col| {
                                col.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("正在读取 A 股行业板块数据…"),
                                )
                            }),
                    ),
            )
    }

    fn render_analysis_stat(
        &self,
        label: &str,
        value: &str,
        hint: &str,
        color: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        v_flex()
            .flex_1()
            .min_w(px(180.))
            .gap_1()
            .p_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .text_color(color)
                    .child(value.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint.to_string()),
            )
            .into_any_element()
    }

    fn render_analysis_section_title(
        &self,
        title: &str,
        subtitle: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(subtitle.to_string()),
            )
            .into_any_element()
    }

    fn render_index_card(
        &self,
        name: &str,
        code: &str,
        snap: Option<IndexSnap>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let up = snap.map(|s| s.change_pct >= 0.0).unwrap_or(true);
        let value = snap
            .map(|s| format!("{:.2}", s.last))
            .unwrap_or_else(|| "--".into());
        let change = snap
            .map(|s| format!("{:+.2}%", s.change_pct))
            .unwrap_or_else(|| "--".into());
        let color = snap
            .map(|_| self.chg_color(up, cx))
            .unwrap_or(cx.theme().muted_foreground);
        let meter_w = snap
            .map(|s| (s.change_pct.abs() as f32 * 18.0).clamp(8.0, 100.0))
            .unwrap_or(8.0);
        let trend = snap.map(index_sparkline);
        let border = cx.theme().border;
        let fill = color.opacity(0.12);

        v_flex()
            .flex_1()
            .min_w(px(260.))
            .gap_2()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(cx.theme().muted_foreground)
                            .child(code.to_string()),
                    ),
            )
            .child(
                h_flex()
                    .items_baseline()
                    .gap_2()
                    .child(
                        div()
                            .text_2xl()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(value),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .text_color(color)
                            .child(change),
                    ),
            )
            .child(
                div()
                    .h(px(5.))
                    .w_full()
                    .rounded_full()
                    .bg(cx.theme().muted)
                    .child(div().h_full().w(px(meter_w)).bg(color)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("最新点位"),
                    )
                    .child(match trend {
                        Some(values) => canvas(
                            move |bounds, _, _| bounds,
                            move |bounds, _, window, _cx| {
                                paint_sparkline(bounds, &values, color, fill, border, window);
                            },
                        )
                        .w(px(116.))
                        .h(px(28.))
                        .into_any_element(),
                        None => div()
                            .w(px(116.))
                            .h(px(28.))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("等待指数快照")
                            .into_any_element(),
                    }),
            )
            .into_any_element()
    }

    fn render_sector_panel(
        &self,
        title: &str,
        subtitle: &str,
        sectors: Vec<SectorTick>,
        strongest: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut rows = sectors;
        rows.sort_by(|a, b| {
            if strongest {
                b.change_pct.total_cmp(&a.change_pct)
            } else {
                a.change_pct.total_cmp(&b.change_pct)
            }
        });
        let sector_count = rows.len();
        let has_rows = !rows.is_empty();
        let row_elements: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(ix, sector)| self.render_sector_row(ix, sector, cx).into_any_element())
            .collect();
        let list_id = if strongest {
            "market-analysis-strong-list"
        } else {
            "market-analysis-weak-list"
        };
        let subtitle = format!("{subtitle} · 全部 {sector_count} 个");

        v_flex()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(self.render_analysis_section_title(title, &subtitle, cx))
            .child(
                div().id(list_id).max_h(px(520.)).overflow_y_scroll().child(
                    v_flex()
                        .gap_0()
                        .when(!has_rows, |col| {
                            col.child(
                                div()
                                    .py_4()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if self.market_analysis_loading {
                                        "板块数据加载中…"
                                    } else {
                                        "暂无板块快照，点击右上角刷新"
                                    }),
                            )
                        })
                        .children(row_elements),
                ),
            )
            .into_any_element()
    }

    fn render_sector_row(
        &self,
        ix: usize,
        sector: SectorTick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let up = sector.change_pct >= 0.0;
        let color = self.chg_color(up, cx);
        let breadth = sector.advances + sector.declines + sector.unchanged;
        let breadth_text = if breadth > 0 {
            format!(
                "↑{} ↓{} · {}",
                sector.advances,
                sector.declines,
                format_sector_amount(sector.amount)
            )
        } else {
            format_sector_amount(sector.amount)
        };
        div()
            .h(px(38.))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.35))
            .child(
                div()
                    .w(px(22.))
                    .text_xs()
                    .font_family("Menlo")
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{:02}", ix + 1)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .truncate()
                    .child(format!("{}  {}", sector.name, sector.code)),
            )
            .child(
                div()
                    .w(px(116.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(breadth_text),
            )
            .child(
                div()
                    .w(px(70.))
                    .text_sm()
                    .font_semibold()
                    .text_color(color)
                    .text_right()
                    .child(format!("{:+.2}%", sector.change_pct)),
            )
            .into_any_element()
    }

    fn render_analysis_metric_row(
        &self,
        label: &str,
        value: &str,
        detail: String,
        color: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .child(value.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(color)
                            .child(detail),
                    ),
            )
            .into_any_element()
    }

    fn analysis_summary(
        &self,
        average_change: Option<f64>,
        advances: u64,
        declines: u64,
        total: u64,
        strongest: Option<&SectorTick>,
    ) -> String {
        if total == 0 {
            return "市场宽度数据尚未返回，刷新后可查看成分股扩散与行业强弱方向。".into();
        }
        let bias = if advances > declines {
            "上涨个股占优，市场情绪偏积极"
        } else if declines > advances {
            "下跌个股占优，市场情绪偏谨慎"
        } else {
            "个股涨跌接近平衡，市场处于分化状态"
        };
        let avg = average_change
            .map(|v| format!("平均 {v:+.2}%"))
            .unwrap_or_else(|| "平均 --".into());
        let lead = strongest
            .map(|s| format!("领涨方向为{}（{:+.2}%）", s.name, s.change_pct))
            .unwrap_or_else(|| "领涨方向待定".into());
        format!("{bias}。{avg}，{lead}。")
    }
}

fn index_sparkline(snap: IndexSnap) -> Vec<f64> {
    // The quote endpoint exposes the latest point and day change. Reconstruct
    // the previous close so the tiny trace remains a factual two-point change
    // indicator instead of implying an unavailable intraday history.
    let last = snap.last.max(1.0);
    let ratio = snap.change_pct / 100.0;
    let previous = if (1.0 + ratio).abs() > 1e-9 {
        last / (1.0 + ratio)
    } else {
        last
    };
    vec![previous, last]
}

fn format_sector_amount(amount: f64) -> String {
    if amount >= 1.0e8 {
        format!("{:.1}亿", amount / 1.0e8)
    } else if amount >= 1.0e4 {
        format!("{:.0}万", amount / 1.0e4)
    } else if amount > 0.0 {
        format!("{amount:.0}")
    } else {
        "成交 --".into()
    }
}
