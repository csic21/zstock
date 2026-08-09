use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};

use crate::app::StockApp;
use crate::app::helpers::*;
use crate::app::labels::L;
use crate::data::freshness::{self, Freshness};
use crate::data::groups::{FindMode, WatchTag};
use crate::data::radar::RadarStrategy;
use crate::data::scout::ScoutVerdict;
use crate::data::treasure::{self, fmt_dd, fmt_pos};
use crate::data::universe::{FinFilter, TREASURE_SCAN_CAP, TREASURE_TOP_N, TreasurePool};
use crate::model::disguise_label;

impl StockApp {
    pub(crate) fn render_treasure_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let long_active = self.find_mode == FindMode::Long;
        let short_active = self.find_mode == FindMode::Short;

        // 顶部统一入口：长线 / 短线
        let header = v_flex()
            .px_2()
            .py_2()
            .gap_1()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(if work { "Opportunities" } else { "机会" }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("find-mode-long")
                            .xsmall()
                            .when(long_active, |b| b.primary())
                            .when(!long_active, |b| b.ghost())
                            .label(L::find_long(work))
                            .tooltip(FindMode::Long.headline(work))
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_find_mode(FindMode::Long, cx);
                            })),
                    )
                    .child(
                        Button::new("find-mode-short")
                            .xsmall()
                            .when(short_active, |b| b.primary())
                            .when(!short_active, |b| b.ghost())
                            .label(L::find_short(work))
                            .tooltip(FindMode::Short.headline(work))
                            .on_click(cx.listener(|this, _, _w, cx| {
                                this.set_find_mode(FindMode::Short, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.find_mode.headline(work)),
            )
            .child(self.render_find_freshness_banner(cx));

        v_flex().flex_1().min_h_0().w_full().child(header).child(
            if self.find_mode == FindMode::Short {
                self.render_radar_body(cx).into_any_element()
            } else {
                self.render_long_find_body(cx).into_any_element()
            },
        )
    }

    fn render_find_freshness_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let (stamp, scanning) = match self.find_mode {
            FindMode::Long => (
                self.treasure_updated_at.as_str(),
                self.treasure_scanning || self.scout_running,
            ),
            FindMode::Short => (self.radar_updated_at.as_str(), self.radar_scanning),
        };
        let fresh = if scanning {
            Freshness::Fresh
        } else {
            freshness::classify(stamp)
        };
        let color = match fresh {
            Freshness::Fresh => cx.theme().green,
            Freshness::Aging => cx.theme().yellow,
            Freshness::Stale => cx.theme().red,
            Freshness::Unknown => cx.theme().muted_foreground,
        };
        let silent = scanning && self.treasure_scan_silent && self.find_mode == FindMode::Long;
        let text = if scanning {
            if silent {
                if work {
                    "Background refresh…".into()
                } else {
                    "后台静默更新中（可继续看盘）…".into()
                }
            } else if work {
                "Scanning…".into()
            } else {
                "扫描进行中…".into()
            }
        } else {
            freshness::banner_text(stamp, work)
        };
        h_flex()
            .px_1()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(cx.theme().radius)
            .bg(color.opacity(0.10))
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(color)
                    .child(if work { "Cache" } else { "时效" }),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(text),
            )
    }

    fn render_radar_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let work = self.work_mode;
        let selected = self.selected.clone();
        let busy = self.radar_scanning;
        let has_hits = !self.radar_hits.is_empty();
        let visible = self.visible_radar_hits();

        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                v_flex()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_wrap()
                            .child(
                                Button::new("radar-scan")
                                    .xsmall()
                                    .when(!busy && !has_hits, |b| b.primary())
                                    .when(busy || has_hits, |b| b.ghost())
                                    .label(if busy {
                                        if work {
                                            "Scanning…"
                                        } else {
                                            "扫描中…"
                                        }
                                    } else if has_hits {
                                        if work {
                                            "Rescan"
                                        } else {
                                            "重新扫描"
                                        }
                                    } else if work {
                                        "Run radar"
                                    } else {
                                        "开始短线扫描"
                                    })
                                    .disabled(busy)
                                    .tooltip(if work {
                                        "Pullback / breakout / oversold on liquid A-shares"
                                    } else {
                                        "扫描强势回踩、放量突破、超跌反弹"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_radar_scan(cx);
                                    })),
                            )
                            .when(busy, |row| {
                                row.child(
                                    Button::new("radar-cancel")
                                        .xsmall()
                                        .ghost()
                                        .label(if work { "Cancel" } else { "取消" })
                                        .on_click(cx.listener(|this, _, _w, cx| {
                                            this.cancel_radar_scan(cx);
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if busy {
                                        format!("{}/{}", self.radar_done, self.radar_total)
                                    } else {
                                        format!("{} 只", visible.len())
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.radar_status.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(
                                Button::new("radar-f-all")
                                    .xsmall()
                                    .when(self.radar_filter.is_none(), |b| b.primary())
                                    .when(self.radar_filter.is_some(), |b| b.ghost())
                                    .label(if work { "All" } else { "全部" })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.set_radar_filter(None, cx);
                                    })),
                            )
                            .children(RadarStrategy::all().into_iter().enumerate().map(
                                |(ix, st)| {
                                    let active = self.radar_filter == Some(st);
                                    Button::new(("radar-f", ix as u32))
                                        .xsmall()
                                        .when(active, |b| b.primary())
                                        .when(!active, |b| b.ghost())
                                        .label(st.label(work))
                                        .tooltip(st.hint())
                                        .on_click(cx.listener(move |this, _, _w, cx| {
                                            this.set_radar_filter(Some(st), cx);
                                        }))
                                },
                            )),
                    ),
            )
            .when(
                !self.radar_summary.as_ref().is_empty() && !busy,
                |col| {
                    col.child(
                        div()
                            .px_3()
                            .py_2()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.radar_summary.clone()),
                    )
                },
            )
            .child(
                v_flex()
                    .id("radar-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(visible.is_empty() && !busy, |col| {
                        col.child(
                            div()
                                .px_3()
                                .py_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if work {
                                    "No short-term hits yet · run radar"
                                } else {
                                    "还没有短线命中 · 点上方「开始短线扫描」\n或先去「市场分析」看板块主线"
                                }),
                        )
                    })
                    .children(visible.into_iter().enumerate().map(|(ix, hit)| {
                        let is_sel = hit.code == selected.as_ref();
                        let hit_c = hit.clone();
                        let code_l = hit.code.clone();
                        let name_l = hit.name.clone();
                        let last_l = hit.close;
                        let code_s = hit.code.clone();
                        let name_s = hit.name.clone();
                        let last_s = hit.close;
                        let chg_color = self.chg_color(hit.change_pct >= 0.0, cx);
                        v_flex()
                            .id(("radar-row", ix))
                            .px_3()
                            .py_2()
                            .gap_1()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.35))
                            .cursor_pointer()
                            .when(is_sel, |r| r.bg(cx.theme().accent.opacity(0.16)))
                            .hover(|r| r.bg(cx.theme().accent.opacity(0.08)))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.select_radar_hit(&hit_c, cx);
                            }))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().foreground)
                                            .child(if work {
                                                hit.code.clone()
                                            } else {
                                                format!("{}  {}", hit.code, hit.name)
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .px_1()
                                            .rounded(cx.theme().radius)
                                            .bg(cx.theme().muted)
                                            .text_color(cx.theme().muted_foreground)
                                            .child(hit.strategy.label(work)),
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_semibold()
                                            .text_color(cx.theme().accent)
                                            .child(format!("{:.0}", hit.score)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(chg_color)
                                            .child(format!("{:+.1}%", hit.change_pct)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if work {
                                        hit.headline.clone()
                                    } else {
                                        format!(
                                            "{} · 观察带 {}",
                                            hit.headline,
                                            hit.watch_band_text()
                                        )
                                    }),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new(("radar-long", ix as u32))
                                            .xsmall()
                                            .ghost()
                                            .label(if work { "+Long" } else { "+长线池" })
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.add_pick_to_group(
                                                    &code_l, &name_l, last_l, WatchTag::Long, cx,
                                                );
                                            })),
                                    )
                                    .child(
                                        Button::new(("radar-short", ix as u32))
                                            .xsmall()
                                            .ghost()
                                            .label(if work { "+Short" } else { "+短线池" })
                                            .on_click(cx.listener(move |this, _, _w, cx| {
                                                this.add_pick_to_group(
                                                    &code_s, &name_s, last_s, WatchTag::Short, cx,
                                                );
                                            })),
                                    ),
                            )
                    })),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.8))
                    .child(if work {
                        "Local rules · not advice"
                    } else {
                        "本地规则排序 · 仅供学习研究，不构成投资建议"
                    }),
            )
    }

    fn render_long_find_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        let work = self.work_mode;
        // 主按钮：空榜 → 搜罗；有榜无清单/重跑 → 筛可买；两边都不抢 primary。
        let has_hits = !self.treasure_hits.is_empty();
        let has_picks = !self.scout_picks.is_empty();
        let busy = self.treasure_scanning || self.scout_running;
        let scan_is_primary = !busy && !has_hits;
        let pick_is_primary = !busy && has_hits && !has_picks;
        let show_full_list = !has_picks || self.treasure_list_expanded;

        v_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                v_flex()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_wrap()
                            .child(
                                Button::new("treasure-scan")
                                    .xsmall()
                                    .when(scan_is_primary, |b| b.primary())
                                    .when(!scan_is_primary, |b| b.ghost())
                                    .label(if self.treasure_scanning {
                                        if work {
                                            "① Running…"
                                        } else {
                                            "① 扫描中…"
                                        }
                                    } else if work {
                                        "① Scout pool"
                                    } else {
                                        "① 开始搜罗"
                                    })
                                    .disabled(busy)
                                    .tooltip(if work {
                                        "Scan historical lows (then auto AI picks)"
                                    } else {
                                        "扫描历史低位；完成后自动运行规则筛选"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_treasure_scan(cx);
                                    })),
                            )
                            .child(
                                Button::new("treasure-ai-scout")
                                    .xsmall()
                                    .when(pick_is_primary, |b| b.primary())
                                    .when(!pick_is_primary, |b| b.ghost())
                                    .label(if self.scout_running {
                                        if work {
                                            "② Picking…"
                                        } else {
                                            "② 规则筛选中…"
                                        }
                                    } else if has_picks {
                                        if work {
                                            "② Re-pick"
                                        } else {
                                            "② 重新规则筛选"
                                        }
                                    } else if work {
                                        "② AI picks"
                                    } else {
                                        "② 规则筛选"
                                    })
                                    .disabled(busy || !has_hits)
                                    .tooltip(if work {
                                        "Batch-rank buy-watch names from the scan list"
                                    } else {
                                        "从机会候选批量生成策略匹配度与参考观察区间"
                                    })
                                    .on_click(cx.listener(|this, _, _w, cx| {
                                        this.start_scout_picks(cx);
                                    })),
                            )
                            .when(busy, |row| {
                                row.child(
                                    Button::new("treasure-cancel")
                                        .xsmall()
                                        .ghost()
                                        .label(if work { "Cancel" } else { "取消" })
                                        .on_click(cx.listener(|this, _, _w, cx| {
                                            if this.scout_running {
                                                this.cancel_scout_picks(cx);
                                            } else {
                                                this.cancel_treasure_scan(cx);
                                            }
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.treasure_status.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.85))
                            .child(if work {
                                format!(
                                    "{} · {} · ≤{TREASURE_SCAN_CAP} · Top{TREASURE_TOP_N}",
                                    self.treasure_pool.label(),
                                    self.treasure_fin.label(),
                                )
                            } else {
                                format!(
                                    "流程：①低位策略 → ②规则筛选（自动）→ 点候选查看证据 · {}池 · {} · Top{TREASURE_TOP_N}",
                                    self.treasure_pool.label(),
                                    self.treasure_fin.label(),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(TreasurePool::all().into_iter().enumerate().map(
                                |(ix, p)| {
                                    let active = self.treasure_pool == p;
                                    Button::new(("tpool", ix as u32))
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(p.label())
                                    .tooltip(if p == TreasurePool::Mcap {
                                        "东财按总市值取沪深 A（默认）"
                                    } else {
                                        "指数成分动态拉取（新浪）"
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_treasure_pool(p, cx);
                                    }))
                                },
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(FinFilter::all().into_iter().enumerate().map(
                                |(ix, f)| {
                                    let active = self.treasure_fin == f;
                                    Button::new(("tfin", ix as u32))
                                    .xsmall()
                                    .when(active, |b| b.primary())
                                    .when(!active, |b| b.ghost())
                                    .label(f.label())
                                    .tooltip(match f {
                                        FinFilter::Off => "不做财务过滤",
                                        FinFilter::Pe => "保留池内 PE 分位 ≤ 50%",
                                        FinFilter::Pb => "保留池内 PB 分位 ≤ 50%",
                                        FinFilter::Value => "PE 与 PB 分位均 ≤ 50%",
                                    })
                                    .on_click(cx.listener(move |this, _, _w, cx| {
                                        this.set_treasure_fin(f, cx);
                                    }))
                                },
                            )),
                    ),
            )
            .child(
                v_flex()
                    .id("treasure-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    // —— 可买观察清单（批量筛结果，优先展示）——
                    .when(
                        !self.scout_picks.is_empty()
                            || self.scout_running
                            || !self.scout_summary.as_ref().is_empty(),
                        |el| {
                            let visible = self.visible_scout_picks();
                            let buy_n = self
                                .scout_picks
                                .iter()
                                .filter(|p| p.verdict == ScoutVerdict::BuyWatch)
                                .count();
                            let watch_n = self
                                .scout_picks
                                .iter()
                                .filter(|p| p.verdict == ScoutVerdict::Watch)
                                .count();
                            let only_buy = self.scout_only_buy_watch;
                            let count_label = if self.scout_running {
                                format!("{}/{}", self.scout_done, self.scout_total)
                            } else if only_buy {
                                format!("{}/{} 可关注", visible.len(), self.scout_picks.len())
                            } else {
                                format!("{} 只", self.scout_picks.len())
                            };

                            el.child(
                                v_flex()
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .child(
                                        h_flex()
                                            .h(px(26.))
                                            .px_3()
                                            .items_center()
                                            .gap_2()
                                            .bg(cx.theme().accent.opacity(0.08))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .child(if work {
                                                        "Buy watchlist"
                                                    } else {
                                                        "候选观察"
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(count_label),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .px_3()
                                            .py_1()
                                            .gap_1()
                                            .items_center()
                                            .border_b_1()
                                            .border_color(cx.theme().border.opacity(0.5))
                                            .child(
                                                Button::new("scout-filter-all")
                                                    .xsmall()
                                                    .when(!only_buy, |b| b.primary())
                                                    .when(only_buy, |b| b.ghost())
                                                    .label(if work {
                                                        "All"
                                                    } else {
                                                        "全部"
                                                    })
                                                    .tooltip(if work {
                                                        "Show BuyWatch + Watch"
                                                    } else {
                                                        "显示可关注与观察"
                                                    })
                                                    .on_click(cx.listener(|this, _, _w, cx| {
                                                        this.set_scout_only_buy_watch(false, cx);
                                                    })),
                                            )
                                            .child(
                                                Button::new("scout-filter-buy")
                                                    .xsmall()
                                                    .when(only_buy, |b| b.primary())
                                                    .when(!only_buy, |b| b.ghost())
                                                    .label(if work {
                                                        "Buy only"
                                                    } else {
                                                        "仅可关注"
                                                    })
                                                    .tooltip(if work {
                                                        "Hide Watch rows"
                                                    } else {
                                                        "只显示可关注，隐藏观察"
                                                    })
                                                    .on_click(cx.listener(|this, _, _w, cx| {
                                                        this.set_scout_only_buy_watch(true, cx);
                                                    })),
                                            )
                                            .when(!self.scout_running && !work, |row| {
                                                row.child(
                                                    div()
                                                        .ml_1()
                                                        .text_xs()
                                                        .text_color(
                                                            cx.theme().muted_foreground.opacity(0.85),
                                                        )
                                                        .child(format!(
                                                            "可关注 {buy_n} · 观察 {watch_n}"
                                                        )),
                                                )
                                            }),
                                    )
                                    .when(!self.scout_summary.as_ref().is_empty(), |c| {
                                        c.child(
                                            div()
                                                .px_3()
                                                .py_2()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(self.scout_summary.clone()),
                                        )
                                        .when(!self.scout_source.as_ref().is_empty(), |c2| {
                                            c2.child(
                                                div()
                                                    .px_3()
                                                    .pb_1()
                                                    .text_xs()
                                                    .text_color(
                                                        cx.theme().muted_foreground.opacity(0.8),
                                                    )
                                                    .child(self.scout_source.clone()),
                                            )
                                        })
                                    })
                                    .when(
                                        visible.is_empty()
                                            && !self.scout_running
                                            && !self.scout_picks.is_empty()
                                            && only_buy,
                                        |c| {
                                            c.child(
                                                div()
                                                    .px_3()
                                                    .py_2()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(if work {
                                                        "No BuyWatch rows. Switch to All to see Watch."
                                                    } else {
                                                        "当前没有「可关注」；点「全部」可查看观察名单。"
                                                    }),
                                            )
                                        },
                                    )
                                    .children(visible.into_iter().enumerate().map(
                                        |(ix, pick)| {
                                            let is_selected =
                                                pick.code == selected.as_ref();
                                            let pick_owned = pick.clone();
                                            let code_label = if work {
                                                disguise_label(&pick.code, &pick.name)
                                            } else {
                                                pick.code.clone()
                                            };
                                            let name =
                                                display_name_str(&pick.name, &pick.code);
                                            let show_name =
                                                !work && is_real_name(&name, &pick.code);
                                            let verdict_color = match pick.verdict {
                                                ScoutVerdict::BuyWatch => {
                                                    cx.theme().success
                                                }
                                                ScoutVerdict::Watch => {
                                                    cx.theme().warning
                                                }
                                                ScoutVerdict::Skip => {
                                                    cx.theme().muted_foreground
                                                }
                                            };
                                            let band = format!(
                                                "建仓 {} · 减仓 {}",
                                                pick.buy_band_text(),
                                                pick.sell_band_text()
                                            );
                                            let why = pick
                                                .reasons
                                                .iter()
                                                .take(2)
                                                .cloned()
                                                .collect::<Vec<_>>()
                                                .join(" · ");

                                            div()
                                                .id(("scout-row", ix as u64))
                                                .px_3()
                                                .py_2()
                                                .flex()
                                                .items_start()
                                                .gap_2()
                                                .cursor_pointer()
                                                .border_b_1()
                                                .border_color(
                                                    cx.theme().border.opacity(0.35),
                                                )
                                                .when(is_selected, |this| {
                                                    this.bg(cx.theme().accent.opacity(0.18))
                                                })
                                                .hover(|this| {
                                                    this.bg(cx.theme().accent.opacity(0.10))
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, _w, cx| {
                                                        this.select_scout_pick(
                                                            &pick_owned, cx,
                                                        );
                                                    },
                                                ))
                                                .child(
                                                    v_flex()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .gap_1()
                                                        .child(
                                                            h_flex()
                                                                .gap_2()
                                                                .items_center()
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .font_semibold()
                                                                        .text_color(
                                                                            cx.theme()
                                                                                .foreground,
                                                                        )
                                                                        .child(code_label),
                                                                )
                                                                .when(show_name, |row| {
                                                                    row.child(
                                                                        div()
                                                                            .text_xs()
                                                                            .text_color(
                                                                                cx.theme()
                                                                                    .muted_foreground,
                                                                            )
                                                                            .child(name),
                                                                    )
                                                                })
                                                                .child(
                                                                    div()
                                                                        .text_xs()
                                                                        .font_semibold()
                                                                        .text_color(
                                                                            verdict_color,
                                                                        )
                                                                        .child(
                                                                            pick.verdict
                                                                                .label(),
                                                                        ),
                                                                ),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().foreground
                                                                        .opacity(0.9),
                                                                )
                                                                .child(if work {
                                                                    format!(
                                                                        "buy {} / sell {}",
                                                                        pick.buy_band_text(),
                                                                        pick.sell_band_text()
                                                                    )
                                                                } else {
                                                                    band
                                                                }),
                                                        )
                                                        .when(!why.is_empty() && !work, |r| {
                                                            r.child(
                                                                div()
                                                                    .text_xs()
                                                                    .text_color(
                                                                        cx.theme()
                                                                            .muted_foreground,
                                                                    )
                                                                    .child(why),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_semibold()
                                                        .text_color(verdict_color)
                                                        .child(format!(
                                                            "{:.0}",
                                                            pick.buy_score
                                                        )),
                                                )
                                        },
                                    )),
                            )
                        },
                    )
                    // 完整寻宝榜：有可买清单时默认折叠，避免抢注意力
                    .when(has_picks, |el| {
                        el.child(
                            h_flex()
                                .id("treasure-list-fold")
                                .h(px(28.))
                                .px_3()
                                .items_center()
                                .gap_2()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .cursor_pointer()
                                .hover(|this| this.bg(cx.theme().accent.opacity(0.06)))
                                .on_click(cx.listener(|this, _, _w, cx| {
                                    let next = !this.treasure_list_expanded;
                                    this.set_treasure_list_expanded(next, cx);
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work {
                                            format!(
                                                "{} full list ({})",
                                                if self.treasure_list_expanded {
                                                    "▾"
                                                } else {
                                                    "▸"
                                                },
                                                self.treasure_hits.len()
                                            )
                                        } else {
                                            format!(
                                                "{} 完整机会榜（{} 只，参考用）",
                                                if self.treasure_list_expanded {
                                                    "▾"
                                                } else {
                                                    "▸"
                                                },
                                                self.treasure_hits.len()
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if self.treasure_list_expanded {
                                            if work {
                                                "Hide"
                                            } else {
                                                "收起"
                                            }
                                        } else if work {
                                            "Show"
                                        } else {
                                            "展开"
                                        }),
                                ),
                        )
                    })
                    .when(!has_picks, |el| {
                        el.child(
                            h_flex()
                                .h(px(26.))
                                .px_3()
                                .items_center()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work {
                                            "Scan list / position"
                                        } else {
                                            "机会榜 / 位置（规则筛选后会出现上方候选）"
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if work { "Scr" } else { "分" }),
                                ),
                        )
                    })
                    .when(self.treasure_hits.is_empty() && !self.treasure_scanning, |el| {
                        el.child(
                            div()
                                .p_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "三步：①「开始搜罗」扫历史低位（≤{TREASURE_SCAN_CAP}）→ \
                                     ②自动筛可买清单与建仓价 → ③点「可关注」看图。\
                                     有缓存榜时可直接点「② 筛可买」。"
                                )),
                        )
                    })
                    .when(show_full_list, |el| {
                        el.children(self.treasure_hits.iter().enumerate().map(|(ix, hit)| {
                        let is_selected = hit.code == selected.as_ref();
                        let hit_owned = hit.clone();
                        let code_label = if work {
                            disguise_label(&hit.code, &hit.name)
                        } else {
                            hit.code.clone()
                        };
                        let name = display_name_str(&hit.name, &hit.code);
                        let show_name = !work && is_real_name(&name, &hit.code);
                        let score = format!("{:.0}", hit.score);
                        let pos_line = format!(
                            "1Y {} · 3Y {} · 全 {}",
                            fmt_pos(hit.pos_1y),
                            fmt_pos(hit.pos_3y),
                            fmt_pos(hit.pos_all)
                        );
                        let dd_line = format!("回撤全 {}", fmt_dd(hit.dd_all));
                        let tags: String = hit
                            .tags
                            .iter()
                            .take(3)
                            .map(|t| t.label())
                            .collect::<Vec<_>>()
                            .join(" · ");
                        let is_pullback = hit
                            .tags
                            .iter()
                            .any(|t| matches!(t, treasure::TreasureTag::UptrendPullback));

                        div()
                            .id(("treasure-row", ix as u64))
                            .px_3()
                            .py_2()
                            .flex()
                            .items_start()
                            .gap_2()
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.35))
                            .when(is_selected, |this| {
                                this.bg(cx.theme().accent.opacity(0.18))
                            })
                            .hover(|this| this.bg(cx.theme().accent.opacity(0.10)))
                            .on_click(cx.listener(move |this, _, _w, cx| {
                                this.select_treasure_hit(&hit_owned, cx);
                            }))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_semibold()
                                                    .text_color(cx.theme().foreground)
                                                    .child(code_label),
                                            )
                                            .when(show_name, |row| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .truncate()
                                                        .child(name),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(pos_line),
                                    )
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(dd_line),
                                            )
                                            .when(!tags.is_empty(), |row| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(if is_pullback {
                                                            cx.theme().yellow
                                                        } else {
                                                            cx.theme().blue
                                                        })
                                                        .truncate()
                                                        .child(tags),
                                                )
                                            }),
                                    ),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(if is_pullback {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().yellow
                                    })
                                    .child(score),
                            )
                        }))
                    }),
            )
    }
}
