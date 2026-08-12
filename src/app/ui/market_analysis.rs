//! Full-page market overview and A-share sector analysis.

use std::sync::Arc;

use chrono::Local;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, canvas, div, prelude::FluentBuilder, px, relative,
};
use gpui_component::tooltip::Tooltip;
use gpui_component::{
    ActiveTheme, Disableable, IconName, PixelsExt, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::chart::paint_sparkline;
use crate::data::eastmoney::{IndustryHeatmapSector, IndustryStockGroup, QuoteTick};
use crate::data::market::SectorTick;
use crate::data::market_analysis::{self as analysis, FearGreedIndex, MarketPick};
use crate::data::session::MarketId;
use crate::model::IndexSnap;

use super::super::treemap::squarified_treemap;
use super::super::{AiPanelState, MarketRegion, StockApp};

const HEATMAP_DEFAULT_HEIGHT: f32 = 440.0;
const HEATMAP_FULLSCREEN_RESERVED_HEIGHT: f32 = 148.0;
const HEATMAP_SECTOR_HEADER_HEIGHT: f32 = 20.0;
const HEATMAP_INDUSTRY_HEADER_HEIGHT: f32 = 16.0;
#[derive(Clone)]
enum HeatmapAction {
    Stock {
        code: String,
        name: String,
        last: f64,
    },
}

#[derive(Clone)]
struct HeatmapTile {
    name: String,
    code: String,
    change_pct: f64,
    amount: f64,
    tooltip: String,
    action: HeatmapAction,
}

impl StockApp {
    pub(crate) fn render_market_analysis(
        &self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let region = self.market_analysis_region;
        let sectors = self.market_analysis_sectors.clone();
        let heatmap_sectors = self.market_heatmap_sectors.clone();
        let sentiment_context =
            analysis::build_context("", &sectors, self.market_index_points(), Vec::new());
        let fear_greed = sentiment_context.fear_greed;
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
        let fear_greed_color = if fear_greed.is_greed() {
            cx.theme().red
        } else if fear_greed.is_fear() {
            cx.theme().green
        } else {
            cx.theme().muted_foreground
        };
        let window_width = window.bounds().size.width.as_f32();
        let window_height = window.bounds().size.height.as_f32();
        let heatmap_surface_width = (window_width - 72.0).clamp(320.0, 1248.0);
        let fullscreen_surface_width = (window_width - 56.0).max(320.0);
        let fullscreen_surface_height =
            (window_height - HEATMAP_FULLSCREEN_RESERVED_HEIGHT).clamp(440.0, 1400.0);

        if self.market_heatmap_fullscreen {
            return v_flex()
                .id("market-analysis-heatmap-fullscreen")
                .debug_selector(|| "market-analysis-heatmap-fullscreen".into())
                .size_full()
                .min_h_0()
                .overflow_hidden()
                .p_3()
                .bg(cx.theme().background)
                .child(self.render_market_heatmap(
                    heatmap_sectors,
                    fullscreen_surface_width,
                    fullscreen_surface_height,
                    true,
                    cx,
                ));
        }

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
                            .label("← 返回今日")
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
                                        "贪婪恐惧",
                                        &format!("{:.0} · {}", fear_greed.score, fear_greed.label),
                                        "0 恐惧 · 50 中性 · 100 贪婪",
                                        fear_greed_color,
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
                            .child(self.render_fear_greed_panel(fear_greed, cx))
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
                            .child(self.render_market_heatmap(
                                heatmap_sectors,
                                heatmap_surface_width,
                                HEATMAP_DEFAULT_HEIGHT,
                                false,
                                cx,
                            ))
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
                                                "强势榜",
                                                "涨幅领先 · 点击查看成分股",
                                                sectors.clone(),
                                                true,
                                                cx,
                                            ))
                                            .child(self.render_sector_panel(
                                                "弱势榜",
                                                "回撤靠前 · 点击查看成分股",
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
                            })
                            .child(self.render_market_ai_panel(cx)),
                    ),
            )
    }

    fn render_fear_greed_panel(&self, index: FearGreedIndex, cx: &mut Context<Self>) -> AnyElement {
        let color = if index.is_greed() {
            cx.theme().red
        } else if index.is_fear() {
            cx.theme().green
        } else {
            cx.theme().muted_foreground
        };
        let meter_fraction = (index.score as f32 / 100.0).clamp(0.0, 1.0);
        v_flex()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(self.render_analysis_section_title(
                        "市场贪婪恐惧指数",
                        "本地可解释模型 · 非官方指标",
                        cx,
                    ))
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(color)
                            .child(format!("{:.0} · {}", index.score, index.label)),
                    ),
            )
            .child(
                div()
                    .h(px(10.))
                    .w_full()
                    .rounded_full()
                    .overflow_hidden()
                    .bg(cx.theme().muted)
                    .child(div().h_full().w(relative(meter_fraction)).bg(color)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .children([
                        self.render_analysis_metric_row(
                            "个股扩散",
                            &format!("{:.0}", index.stock_breadth),
                            "上涨/平盘/下跌".to_string(),
                            color,
                            cx,
                        ),
                        self.render_analysis_metric_row(
                            "行业扩散",
                            &format!("{:.0}", index.sector_breadth),
                            "行业指数涨跌".to_string(),
                            color,
                            cx,
                        ),
                        self.render_analysis_metric_row(
                            "指数动量",
                            &format!("{:.0}", index.index_momentum),
                            "三大指数".to_string(),
                            color,
                            cx,
                        ),
                        self.render_analysis_metric_row(
                            "行业动量",
                            &format!("{:.0}", index.sector_momentum),
                            "行业平均涨跌".to_string(),
                            color,
                            cx,
                        ),
                    ])
                    .into_any_element(),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("指数 20% · 个股扩散 45% · 行业扩散 25% · 行业动量 10%"),
            )
            .into_any_element()
    }

    fn render_market_ai_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let loading = matches!(&self.market_ai_panel, AiPanelState::Loading { .. });
        let has_data = !self.market_analysis_sectors.is_empty();
        let mut panel = v_flex()
            .gap_3()
            .w_full()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(self.render_analysis_section_title(
                        "AI 大盘分析",
                        "按钮触发 · 自动筛选当日候选观察股",
                        cx,
                    ))
                    .child(
                        Button::new("market-ai-analyze")
                            .xsmall()
                            .when(!loading && has_data, |b| b.primary())
                            .when(loading || !has_data, |b| b.ghost())
                            .label(if loading {
                                "分析中…"
                            } else if self.market_ai_picks.is_empty() {
                                "AI 分析大盘"
                            } else {
                                "重新分析"
                            })
                            .disabled(loading || !has_data)
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.request_market_ai_analysis(cx);
                            })),
                    ),
            );

        match &self.market_ai_panel {
            AiPanelState::Loading { text } => {
                panel = panel
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(text.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("本地候选扫描完成后，已配置的 LLM 会继续生成大盘简报…"),
                    );
            }
            AiPanelState::Ready { text, source, note } => {
                let source_color = if source.is_llm() {
                    cx.theme().accent
                } else {
                    cx.theme().muted_foreground
                };
                panel = panel
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("来源"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(source_color)
                                    .child(source.label(false)),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(text.clone()),
                    );
                if let Some(note) = note {
                    panel = panel.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(note.clone()),
                    );
                }
            }
            AiPanelState::Idle => {
                panel = panel.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if has_data {
                            "点击按钮读取候选股并生成当日市场分析。"
                        } else {
                            "等待行业数据加载完成后可开始分析。"
                        }),
                );
            }
        }

        if !self.market_ai_picks.is_empty() {
            panel = panel
                .child(self.render_analysis_section_title(
                    "今日候选观察",
                    "实时上涨 + 技术快照综合排序，不代表明日必涨",
                    cx,
                ))
                .child(
                    v_flex().gap_0().children(
                        self.market_ai_picks
                            .iter()
                            .enumerate()
                            .map(|(ix, pick)| self.render_market_pick_row(ix, pick, cx)),
                    ),
                );
        }
        panel.into_any_element()
    }

    fn render_market_pick_row(
        &self,
        ix: usize,
        pick: &MarketPick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let score_color = if pick.score >= 65.0 {
            cx.theme().red
        } else {
            cx.theme().foreground
        };
        let detail = format!(
            "{:+.2}% · {:.0}分 · {}{}",
            pick.change_pct,
            pick.score,
            pick.regime,
            pick.rsi14
                .map(|rsi| format!(" · RSI {:.0}", rsi))
                .unwrap_or_default()
        );
        let reason = pick
            .reasons
            .first()
            .map(String::as_str)
            .unwrap_or("实时行情与技术快照");
        let risk = pick
            .risks
            .first()
            .map(|risk| format!(" · 风险：{risk}"))
            .unwrap_or_default();
        h_flex()
            .h(px(44.))
            .w_full()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.35))
            .child(
                div()
                    .w(px(24.))
                    .text_xs()
                    .font_family("Menlo")
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{:02}", ix + 1)),
            )
            .child(
                h_flex()
                    .w(px(190.))
                    .flex_shrink_0()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .truncate()
                            .child(pick.name.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(cx.theme().foreground)
                            .child(pick.code.clone()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .truncate()
                    .child(format!("现价 {:.2} · {}{}", pick.last, reason, risk)),
            )
            .child(
                div()
                    .w(px(190.))
                    .text_xs()
                    .text_color(score_color)
                    .text_right()
                    .child(detail),
            )
            .child(
                div()
                    .w(px(66.))
                    .text_sm()
                    .font_semibold()
                    .text_color(self.chg_color(pick.change_pct >= 0.0, cx))
                    .text_right()
                    .child(format!("{:+.2}%", pick.change_pct)),
            )
            .into_any_element()
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

    fn render_market_heatmap(
        &self,
        heatmap_sectors: Arc<Vec<IndustryHeatmapSector>>,
        surface_width: f32,
        surface_height: f32,
        fullscreen: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.sector_drill_code.is_none() {
            return self.render_full_market_heatmap(
                heatmap_sectors,
                surface_width,
                surface_height,
                fullscreen,
                cx,
            );
        }
        let drill_name = self
            .sector_drill_name
            .as_ref()
            .map(|name| name.to_string())
            .unwrap_or_else(|| "板块成分".into());

        let (
            title,
            subtitle,
            treemap_tiles,
            list_tiles,
            loading,
            empty_message,
            advances,
            declines,
            unchanged,
        ) = {
            let mut list_tiles = self
                .sector_drill_quotes
                .iter()
                .map(stock_heatmap_tile)
                .collect::<Vec<_>>();
            list_tiles.sort_by(|left, right| {
                right
                    .amount
                    .total_cmp(&left.amount)
                    .then_with(|| left.code.cmp(&right.code))
            });
            let advances = list_tiles
                .iter()
                .filter(|tile| tile.change_pct > 0.0)
                .count();
            let declines = list_tiles
                .iter()
                .filter(|tile| tile.change_pct < 0.0)
                .count();
            let unchanged = list_tiles.len().saturating_sub(advances + declines);
            let treemap_tiles = list_tiles.clone();
            (
                format!("行业热力图 / {drill_name}"),
                "全部成分股 · 面积代表成交额 · 点击个股打开图表".to_string(),
                treemap_tiles,
                list_tiles,
                self.sector_drill_loading,
                self.sector_drill_error
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "暂无成分股快照".into()),
                advances,
                declines,
                unchanged,
            )
        };
        let has_data = !list_tiles.is_empty();
        let down_color = self.chg_color(false, cx);
        let up_color = self.chg_color(true, cx);
        let can_go_back = self.market_heatmap_can_go_back();
        let body = if self.market_heatmap_list {
            self.render_heatmap_list(list_tiles, surface_height, loading, &empty_message, cx)
        } else {
            self.render_heatmap_treemap(
                treemap_tiles,
                surface_width,
                surface_height,
                loading,
                &empty_message,
                cx,
            )
        };

        v_flex()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .when(can_go_back, |header| {
                        header.child(
                            Button::new("market-heatmap-back")
                                .xsmall()
                                .ghost()
                                .label("← 返回")
                                .on_click(cx.listener(|this, _, _window, cx| {
                                    this.back_market_heatmap(cx);
                                })),
                        )
                    })
                    .child(self.render_analysis_section_title(&title, &subtitle, cx))
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .child(div().text_color(up_color).child(format!("涨 {advances}")))
                            .child(div().text_color(cx.theme().muted_foreground).child("·"))
                            .child(div().text_color(down_color).child(format!("跌 {declines}")))
                            .when(unchanged > 0, |counts| {
                                counts
                                    .child(div().text_color(cx.theme().muted_foreground).child("·"))
                                    .child(
                                        div()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("平 {unchanged}")),
                                    )
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("跌"),
                            )
                            .children(
                                [
                                    down_color.opacity(0.72),
                                    down_color.opacity(0.42),
                                    cx.theme().muted,
                                    up_color.opacity(0.42),
                                    up_color.opacity(0.72),
                                ]
                                .into_iter()
                                .map(|color| div().w(px(18.)).h(px(8.)).rounded_sm().bg(color)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("涨"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("market-heatmap-treemap-view")
                                    .xsmall()
                                    .when(!self.market_heatmap_list, |button| button.primary())
                                    .when(self.market_heatmap_list, |button| button.ghost())
                                    .label("热力图")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.set_market_heatmap_list(false, cx);
                                    })),
                            )
                            .child(
                                Button::new("market-heatmap-list-view")
                                    .xsmall()
                                    .when(self.market_heatmap_list, |button| button.primary())
                                    .when(!self.market_heatmap_list, |button| button.ghost())
                                    .label("列表")
                                    .on_click(cx.listener(|this, _, _window, cx| {
                                        this.set_market_heatmap_list(true, cx);
                                    })),
                            ),
                    )
                    .child(
                        Button::new("market-heatmap-fullscreen-toggle")
                            .xsmall()
                            .ghost()
                            .icon(if fullscreen {
                                IconName::Minimize
                            } else {
                                IconName::Maximize
                            })
                            .label(if fullscreen { "退出全屏" } else { "全屏" })
                            .tooltip(if fullscreen {
                                "退出热力图全屏（Esc）"
                            } else {
                                "全屏查看热力图"
                            })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.toggle_market_heatmap_fullscreen(cx);
                            })),
                    ),
            )
            .when(has_data || loading || !empty_message.is_empty(), |panel| {
                panel.child(body)
            })
            .into_any_element()
    }

    fn render_full_market_heatmap(
        &self,
        sectors: Arc<Vec<IndustryHeatmapSector>>,
        surface_width: f32,
        surface_height: f32,
        fullscreen: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let sector_count = sectors.len();
        let industry_count: usize = sectors.iter().map(|group| group.industries.len()).sum();
        let stock_count: usize = sectors
            .iter()
            .flat_map(|group| &group.industries)
            .map(|industry| industry.stocks.len())
            .sum();
        let advances = sectors
            .iter()
            .flat_map(|group| &group.industries)
            .flat_map(|industry| &industry.stocks)
            .filter(|stock| stock.change_pct > 0.0)
            .count();
        let declines = sectors
            .iter()
            .flat_map(|group| &group.industries)
            .flat_map(|industry| &industry.stocks)
            .filter(|stock| stock.change_pct < 0.0)
            .count();
        let unchanged = stock_count.saturating_sub(advances + declines);
        let has_data = stock_count > 0;
        let down_color = self.chg_color(false, cx);
        let up_color = self.chg_color(true, cx);
        let empty_message = self
            .market_heatmap_error
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "暂无全景数据，点击右上角刷新".into());
        let body = self.render_grouped_heatmap_treemap(
            sectors,
            surface_width,
            surface_height,
            self.market_heatmap_loading,
            &empty_message,
            cx,
        );

        v_flex()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(self.render_analysis_section_title(
                        "A股全景热力图（申万行业）",
                        &format!(
                            "{sector_count} 个一级行业 · {industry_count} 个二级行业 · {stock_count} 只个股"
                        ),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .child(div().text_color(up_color).child(format!("涨 {advances}")))
                            .child(div().text_color(cx.theme().muted_foreground).child("·"))
                            .child(div().text_color(down_color).child(format!("跌 {declines}")))
                            .when(unchanged > 0, |counts| {
                                counts
                                    .child(div().text_color(cx.theme().muted_foreground).child("·"))
                                    .child(
                                        div()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("平 {unchanged}")),
                                    )
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("跌"),
                            )
                            .children(
                                [
                                    down_color.opacity(0.88),
                                    down_color.opacity(0.48),
                                    cx.theme().muted,
                                    up_color.opacity(0.48),
                                    up_color.opacity(0.88),
                                ]
                                .into_iter()
                                .map(|color| div().w(px(18.)).h(px(8.)).rounded_sm().bg(color)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("涨"),
                            ),
                    )
                    .child(
                        Button::new("market-heatmap-fullscreen-toggle")
                            .xsmall()
                            .ghost()
                            .icon(if fullscreen {
                                IconName::Minimize
                            } else {
                                IconName::Maximize
                            })
                            .label(if fullscreen { "退出全屏" } else { "全屏" })
                            .tooltip(if fullscreen {
                                "退出热力图全屏（Esc）"
                            } else {
                                "全屏查看热力图"
                            })
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.toggle_market_heatmap_fullscreen(cx);
                            })),
                    ),
            )
            .when(has_data || self.market_heatmap_loading || !empty_message.is_empty(), |panel| {
                panel.child(body)
            })
            .into_any_element()
    }

    fn render_grouped_heatmap_treemap(
        &self,
        sectors: Arc<Vec<IndustryHeatmapSector>>,
        surface_width: f32,
        surface_height: f32,
        loading: bool,
        empty_message: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weights = sectors
            .iter()
            .map(|group| {
                let stock_amount: f64 = group.industries.iter().map(|item| item.amount).sum();
                stock_amount.max(group.sector.amount)
            })
            .collect::<Vec<_>>();
        let cells = squarified_treemap(&weights, f64::from(surface_width / surface_height));
        let mut next_stock_id = 0u32;
        let mut sector_elements = Vec::with_capacity(cells.len());
        for cell in cells {
            if let Some(group) = sectors.get(cell.index) {
                sector_elements.push(self.render_heatmap_sector_group(
                    cell.rect,
                    group,
                    surface_width,
                    surface_height,
                    &mut next_stock_id,
                    cx,
                ));
            }
        }
        let has_data = next_stock_id > 0;

        div()
            .id("market-sector-heatmap")
            .debug_selector(|| "market-sector-heatmap".into())
            .relative()
            .w_full()
            .h(px(surface_height))
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .when(!has_data, |surface| {
                surface.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(if loading {
                            cx.theme().muted_foreground
                        } else {
                            cx.theme().red
                        })
                        .child(if loading {
                            "正在加载全部 31 个一级行业及其个股…".to_string()
                        } else {
                            empty_message.to_string()
                        }),
                )
            })
            .children(sector_elements)
            .into_any_element()
    }

    fn render_heatmap_sector_group(
        &self,
        rect: super::super::treemap::TreemapRect,
        group: &IndustryHeatmapSector,
        surface_width: f32,
        surface_height: f32,
        next_stock_id: &mut u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group_width = rect.width * surface_width;
        let group_height = rect.height * surface_height;
        let show_header = group_width >= 52.0 && group_height >= 34.0;
        let header_height = if show_header {
            HEATMAP_SECTOR_HEADER_HEIGHT.min(group_height * 0.24)
        } else {
            0.0
        };
        let body_height = (group_height - header_height).max(1.0);
        let weights = group
            .industries
            .iter()
            .map(|industry| industry.amount)
            .collect::<Vec<_>>();
        let cells = squarified_treemap(&weights, f64::from(group_width / body_height));
        let mut industries = Vec::with_capacity(cells.len());
        for cell in cells {
            if let Some(industry) = group.industries.get(cell.index) {
                industries.push(self.render_heatmap_industry_group(
                    cell.rect,
                    industry,
                    group_width,
                    body_height,
                    next_stock_id,
                    cx,
                ));
            }
        }
        let code = group.sector.code.clone();
        let name = group.sector.name.clone();
        let tooltip_name = name.clone();
        let stock_count: usize = group
            .industries
            .iter()
            .map(|industry| industry.stocks.len())
            .sum();

        div()
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .overflow_hidden()
            .border_2()
            .border_color(cx.theme().background)
            .bg(cx.theme().sidebar)
            .when(show_header, |sector| {
                sector.child(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "heatmap-sector-{}",
                            group.sector.code
                        )))
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(header_height))
                        .px_1()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .bg(cx.theme().background.opacity(0.94))
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .truncate()
                        .tooltip(move |window, cx| {
                            Tooltip::new(format!(
                                "{tooltip_name} · {stock_count} 只个股\n点击放大该行业"
                            ))
                            .build(window, cx)
                        })
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            this.open_sector_drill(code.clone(), name.clone(), cx);
                        }))
                        .child(group.sector.name.clone()),
                )
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(header_height))
                    .bottom_0()
                    .overflow_hidden()
                    .children(industries),
            )
            .into_any_element()
    }

    fn render_heatmap_industry_group(
        &self,
        rect: super::super::treemap::TreemapRect,
        industry: &IndustryStockGroup,
        surface_width: f32,
        surface_height: f32,
        next_stock_id: &mut u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let group_width = rect.width * surface_width;
        let group_height = rect.height * surface_height;
        let show_header = group_width >= 44.0 && group_height >= 28.0;
        let header_height = if show_header {
            HEATMAP_INDUSTRY_HEADER_HEIGHT.min(group_height * 0.28)
        } else {
            0.0
        };
        let stock_height = (group_height - header_height).max(1.0);
        let weights = industry
            .stocks
            .iter()
            .map(|stock| stock.amount)
            .collect::<Vec<_>>();
        let cells = squarified_treemap(&weights, f64::from(group_width / stock_height));
        let mut stocks = Vec::with_capacity(cells.len());
        for cell in cells {
            if let Some(stock) = industry.stocks.get(cell.index) {
                let id = *next_stock_id;
                *next_stock_id = next_stock_id.saturating_add(1);
                stocks.push(self.render_heatmap_tile(
                    id as usize,
                    cell.rect,
                    stock_heatmap_tile(stock),
                    group_width,
                    stock_height,
                    cx,
                ));
            }
        }
        let stock_count = industry.stocks.len();
        let label = industry.name.clone();

        div()
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .overflow_hidden()
            .border_1()
            .border_color(cx.theme().background.opacity(0.92))
            .bg(cx.theme().muted.opacity(0.3))
            .when(show_header, |group| {
                group.child(
                    div()
                        .id(gpui::SharedString::from(format!(
                            "heatmap-industry-{}",
                            industry.name
                        )))
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(header_height))
                        .px_1()
                        .flex()
                        .items_center()
                        .bg(cx.theme().sidebar.opacity(0.96))
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().foreground.opacity(0.92))
                        .truncate()
                        .tooltip(move |window, cx| {
                            Tooltip::new(format!("{label} · {stock_count} 只个股"))
                                .build(window, cx)
                        })
                        .child(industry.name.clone()),
                )
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(header_height))
                    .bottom_0()
                    .overflow_hidden()
                    .children(stocks),
            )
            .into_any_element()
    }

    fn render_heatmap_treemap(
        &self,
        tiles: Vec<HeatmapTile>,
        surface_width: f32,
        surface_height: f32,
        loading: bool,
        empty_message: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let weights: Vec<f64> = tiles.iter().map(|tile| tile.amount).collect();
        let cells = squarified_treemap(&weights, f64::from(surface_width / surface_height));
        let tile_elements: Vec<_> = cells
            .into_iter()
            .filter_map(|cell| {
                tiles.get(cell.index).cloned().map(|tile| {
                    self.render_heatmap_tile(
                        cell.index,
                        cell.rect,
                        tile,
                        surface_width,
                        surface_height,
                        cx,
                    )
                })
            })
            .collect();
        let has_data = !tile_elements.is_empty();

        div()
            .id("market-sector-heatmap")
            .debug_selector(|| "market-sector-heatmap".into())
            .relative()
            .w_full()
            .h(px(surface_height))
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.35))
            .when(!has_data, |surface| {
                surface.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(if loading {
                            cx.theme().muted_foreground
                        } else {
                            cx.theme().red
                        })
                        .child(if loading {
                            "热力图加载中…".to_string()
                        } else {
                            empty_message.to_string()
                        }),
                )
            })
            .children(tile_elements)
            .into_any_element()
    }

    fn render_heatmap_tile(
        &self,
        index: usize,
        rect: super::super::treemap::TreemapRect,
        tile: HeatmapTile,
        surface_width: f32,
        surface_height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let estimated_width = rect.width * surface_width;
        let estimated_height = rect.height * surface_height;
        let show_name = estimated_width >= 48.0 && estimated_height >= 24.0;
        let show_change = estimated_width >= 68.0 && estimated_height >= 42.0;
        let show_amount = estimated_width >= 118.0 && estimated_height >= 68.0;
        let move_color = self.chg_color(tile.change_pct >= 0.0, cx);
        let opacity = heatmap_opacity(tile.change_pct, self.work_mode);
        let fill = if tile.change_pct.abs() < 0.05 {
            cx.theme().muted
        } else {
            move_color.opacity(opacity)
        };
        let tooltip = tile.tooltip.clone();
        let action = tile.action.clone();

        div()
            .id(("market-heatmap-tile", index as u32))
            .when(index == 0, |tile| {
                tile.debug_selector(|| "market-heatmap-first-stock".into())
            })
            .absolute()
            .left(relative(rect.x))
            .top(relative(rect.y))
            .w(relative(rect.width))
            .h(relative(rect.height))
            .p_1()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .cursor_pointer()
            .border_1()
            .border_color(cx.theme().background.opacity(0.72))
            .bg(fill)
            .hover(|cell| {
                cell.border_2()
                    .border_color(cx.theme().foreground.opacity(0.8))
            })
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.activate_heatmap_tile(action.clone(), cx);
            }))
            .when(show_name, |cell| {
                cell.child(
                    v_flex()
                        .max_w_full()
                        .items_center()
                        .gap_0()
                        .text_center()
                        .text_color(cx.theme().foreground)
                        .child(
                            div()
                                .max_w_full()
                                .truncate()
                                .when(show_change, |name| name.text_sm())
                                .when(!show_change, |name| name.text_xs())
                                .font_semibold()
                                .child(tile.name.clone()),
                        )
                        .when(show_change, |content| {
                            content.child(
                                div()
                                    .text_xs()
                                    .font_family("Menlo")
                                    .font_semibold()
                                    .child(format!("{:+.2}%", tile.change_pct)),
                            )
                        })
                        .when(show_amount, |content| {
                            content.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().foreground.opacity(0.72))
                                    .child(format_sector_amount(tile.amount)),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    fn render_heatmap_list(
        &self,
        tiles: Vec<HeatmapTile>,
        surface_height: f32,
        loading: bool,
        empty_message: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_data = !tiles.is_empty();
        v_flex()
            .id("market-sector-heatmap-list")
            .w_full()
            .h(px(surface_height))
            .overflow_y_scroll()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted.opacity(0.2))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .when(!has_data, |list| {
                list.child(
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if loading {
                            "列表加载中…".to_string()
                        } else {
                            empty_message.to_string()
                        }),
                )
            })
            .children(tiles.into_iter().enumerate().map(|(index, tile)| {
                let action = tile.action.clone();
                let color = self.chg_color(tile.change_pct >= 0.0, cx);
                div()
                    .id(("market-heatmap-list-row", index as u32))
                    .h(px(40.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.35))
                    .hover(|row| row.bg(cx.theme().accent.opacity(0.08)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.activate_heatmap_tile(action.clone(), cx);
                    }))
                    .child(
                        div()
                            .w(px(30.))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{:02}", index + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .truncate()
                            .child(tile.name),
                    )
                    .child(
                        div()
                            .w(px(82.))
                            .text_xs()
                            .font_family("Menlo")
                            .text_color(cx.theme().muted_foreground)
                            .child(tile.code),
                    )
                    .child(
                        div()
                            .w(px(84.))
                            .text_xs()
                            .text_right()
                            .text_color(cx.theme().muted_foreground)
                            .child(format_sector_amount(tile.amount)),
                    )
                    .child(
                        div()
                            .w(px(70.))
                            .text_sm()
                            .font_semibold()
                            .text_right()
                            .text_color(color)
                            .child(format!("{:+.2}%", tile.change_pct)),
                    )
            }))
            .into_any_element()
    }

    fn activate_heatmap_tile(&mut self, action: HeatmapAction, cx: &mut Context<Self>) {
        match action {
            HeatmapAction::Stock { code, name, last } => {
                self.select_sector_constituent(code, name, last, cx)
            }
        }
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
        rows.truncate(5);
        let visible_count = rows.len();
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
        let subtitle = format!("{subtitle} · 前 {visible_count}/{sector_count}");

        v_flex()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(self.render_analysis_section_title(title, &subtitle, cx))
            .child(
                div()
                    .id(list_id)
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    // GPUI bubbles wheel events through nested scroll containers. Keep the
                    // page from scrolling when the pointer is over one of these lists.
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(
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
        let code = sector.code.clone();
        let name = sector.name.clone();
        let selected = self
            .sector_drill_code
            .as_ref()
            .is_some_and(|c| c == &sector.code);
        div()
            .id(("sector-row", ix as u32))
            .h(px(38.))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.35))
            .when(selected, |r| r.bg(cx.theme().accent.opacity(0.14)))
            .hover(|r| r.bg(cx.theme().accent.opacity(0.08)))
            .on_click(cx.listener(move |this, _, _w, cx| {
                this.open_sector_drill(code.clone(), name.clone(), cx);
            }))
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

fn stock_heatmap_tile(quote: &QuoteTick) -> HeatmapTile {
    let amount = if quote.amount > 0.0 {
        quote.amount
    } else {
        quote.last.max(0.0) * quote.volume as f64
    };
    HeatmapTile {
        name: quote.name.clone(),
        code: quote.code.clone(),
        change_pct: quote.change_pct,
        amount,
        tooltip: format!(
            "{} · {}\n现价 {:.2} · 涨跌 {:+.2}%\n成交 {} · 成交量 {}\n点击打开个股图表",
            quote.name,
            quote.code,
            quote.last,
            quote.change_pct,
            format_sector_amount(amount),
            quote.volume
        ),
        action: HeatmapAction::Stock {
            code: quote.code.clone(),
            name: quote.name.clone(),
            last: quote.last,
        },
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

fn heatmap_opacity(change_pct: f64, work_mode: bool) -> f32 {
    let normalized = (change_pct.abs() / 5.0).clamp(0.0, 1.0).sqrt() as f32;
    if work_mode {
        0.08 + normalized * 0.20
    } else {
        0.24 + normalized * 0.64
    }
}
