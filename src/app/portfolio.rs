//! Portfolio trades, cash, and position AI advice.

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
    /// Write config.json immediately (structural changes: add/remove symbol, trades).
    pub(crate) fn persist(&self) {
        let mut dock = self.dock.clone();
        dock.window = self.window_bounds;
        let cfg = AppConfig {
            watchlist: self.symbols.iter().map(|s| s.code.clone()).collect(),
            selected: self.selected.to_string(),
            range: self.range.label().into(),
            chart_kind: self.chart_kind.to_label().into(),
            show_ma5: self.show_ma5,
            show_ma10: self.show_ma10,
            show_ma20: self.show_ma20,
            show_ma60: self.show_ma60,
            show_volume: self.show_volume,
            show_macd: self.show_macd,
            show_boll: self.show_boll,
            dock,
            left_width: self.left_width,
            bottom_height: self.bottom_height,
            color_scheme: self.color_scheme,
            work_mode: self.work_mode,
            work_density: self.work_density,
            work_right_width: self.work_right_width,
            work_aliases: self.work_aliases.clone(),
            quote_interval_secs: self.quote_interval_secs,
            watchlist_sort: self.watchlist_sort,
            ai_api: self.ai_config.clone(),
            buy_alerts: self.buy_alerts.clone(),
            chart_lines: self.chart_lines.clone(),
            treasure_pool: self.treasure_pool.id().into(),
            treasure_fin: self.treasure_fin.id().into(),
            status_bar_enabled: self.status_bar_enabled,
            status_bar_codes: self.status_bar_codes.clone(),
            status_bar_active: self.status_bar_active.clone(),
            detail_tab: self.detail_tab.to_label().into(),
            left_tab: self.left_tab.to_label().into(),
        };
        let _ = storage::save_config(&cfg);
    }

    /// Debounced config write — collapses rapid UI thrash (resize, typing, tab flips).
    pub(crate) fn schedule_persist(&mut self, cx: &mut Context<Self>) {
        self.persist_gen = self.persist_gen.wrapping_add(1);
        let token = self.persist_gen;
        cx.spawn(async move |this, cx| {
            Timer::after(super::PERSIST_DEBOUNCE).await;
            let _ = this.update(cx, |app, _cx| {
                if app.persist_gen == token {
                    app.persist();
                }
            });
        })
        .detach();
    }

    pub(crate) fn persist_portfolio(&self) {
        let _ = storage::save_portfolio(&self.portfolio);
    }

    /// 组合汇总：现价优先取自选行情，否则用成本占位。
    pub(crate) fn portfolio_summary(&self) -> PortfolioSummary {
        self.portfolio.summarize_with(|code| {
            if let Some(sym) = self.symbols.iter().find(|s| s.code == code) {
                (
                    sym.last,
                    sym.change_pct,
                    sym.name.to_string(),
                )
            } else {
                (0.0, 0.0, String::new())
            }
        })
    }

    /// 确保代码在自选中（持仓需要行情）。
    pub(crate) fn ensure_in_watchlist(&mut self, code: &str, name: &str, last: f64) {
        let code = normalize_code(code).unwrap_or_else(|| code.trim().to_string());
        if code.is_empty() {
            return;
        }
        if self.symbols.iter().any(|s| s.code == code) {
            if let Some(sym) = self.symbols.iter_mut().find(|s| s.code == code) {
                if is_real_name(name, &code) && !is_real_name(sym.name.as_ref(), &code) {
                    sym.name = shared(name.to_string());
                }
                if last > 0.0 && sym.last <= 0.0 {
                    sym.last = last;
                }
            }
            return;
        }
        self.symbols.push(Symbol {
            code: code.clone(),
            name: shared(if is_real_name(name, &code) {
                name.to_string()
            } else {
                code.clone()
            }),
            last,
            change_pct: 0.0,
            volume: 0,
            board: board_for_code(&code),
        });
        self.filtered_local = (0..self.symbols.len()).collect();
    }

    pub(crate) fn dismiss_overlay(&mut self, cx: &mut Context<Self>) {
        if self.work_alias_editing {
            self.cancel_work_alias_edit(cx);
            return;
        }
        if self.palette_open {
            self.palette_open = false;
            cx.notify();
            return;
        }
        if self.trade_form_side.is_some() {
            self.trade_form_side = None;
            cx.notify();
            return;
        }
        if self.settings_open {
            self.close_settings(cx);
            return;
        }
        if self.market_analysis_open {
            self.close_market_analysis(cx);
            return;
        }
        if self.drawing_mode {
            self.drawing_mode = false;
            self.drawing_anchor = None;
            self.draft_line = None;
            self.status = shared(if self.work_mode {
                "draw mode off"
            } else {
                "已退出画线模式"
            });
            cx.notify();
        }
    }

    /// 打开买入/卖出表单，默认填充现价与（卖出时）全部可卖股数。
    pub(crate) fn open_trade_form(&mut self, side: TradeSide, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let last = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.last)
            .filter(|p| *p > 0.0)
            .or_else(|| {
                self.candles
                    .last()
                    .filter(|_| {
                        self.candles_code
                            .as_ref()
                            .is_some_and(|c| c == code.as_str())
                    })
                    .map(|c| c.close)
            })
            .unwrap_or(0.0);
        let held = self
            .portfolio
            .position_of(&code)
            .map(|p| p.shares)
            .unwrap_or(0.0);

        let price_s = if last > 0.0 {
            format_price(last)
        } else {
            String::new()
        };
        let shares_s = match side {
            TradeSide::Sell if held > 0.0 => format_shares(held),
            _ => String::new(),
        };

        self.trade_price_input.update(cx, |s, cx| {
            s.set_value(price_s, window, cx);
        });
        self.trade_shares_input.update(cx, |s, cx| {
            s.set_value(shares_s, window, cx);
        });
        self.trade_fee_input.update(cx, |s, cx| {
            s.set_value("0", window, cx);
        });
        self.trade_note_input.update(cx, |s, cx| {
            s.set_value("", window, cx);
        });
        self.trade_form_side = Some(side);
        self.left_tab = LeftTab::Portfolio;
        self.persist();
        cx.notify();
    }

    pub(crate) fn submit_trade(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(side) = self.trade_form_side else {
            return;
        };
        let code = self.selected.to_string();
        let name = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.name.to_string())
            .unwrap_or_else(|| code.clone());

        let shares = parse_f64(&self.trade_shares_input.read(cx).value());
        let price = parse_f64(&self.trade_price_input.read(cx).value());
        let fee = parse_f64(&self.trade_fee_input.read(cx).value()).unwrap_or(0.0);
        let note = self.trade_note_input.read(cx).value().to_string();

        let (Some(shares), Some(price)) = (shares, price) else {
            self.status = shared(if self.work_mode {
                "Invalid shares/price"
            } else {
                "请填写有效的股数与价格"
            });
            cx.notify();
            return;
        };

        match self.portfolio.record_trade(
            &code,
            &name,
            side,
            shares,
            price,
            fee,
            &note,
            None,
        ) {
            Ok(_) => {
                self.ensure_in_watchlist(&code, &name, price);
                self.persist_portfolio();
                self.persist();
                self.trade_form_side = None;
                self.status = shared(if self.work_mode {
                    format!(
                        "{} {} × {} @ {}",
                        side.label_work(),
                        code,
                        format_shares(shares),
                        format_price(price)
                    )
                } else {
                    format!(
                        "{} {} × {} 股 @ {} 元",
                        side.label(),
                        code,
                        format_shares(shares),
                        format_price(price)
                    )
                });
                self.detail_tab = DetailTab::Portfolio;
                self.persist(); // immediate: trade is structural
                // 清空表单
                self.trade_shares_input.update(cx, |s, cx| {
                    s.set_value("", window, cx);
                });
                self.trade_note_input.update(cx, |s, cx| {
                    s.set_value("", window, cx);
                });
            }
            Err(e) => {
                self.status = shared(e.to_string());
            }
        }
        cx.notify();
    }

    pub(crate) fn close_selected_position(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let price = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.last)
            .filter(|p| *p > 0.0)
            .or_else(|| {
                self.candles.last().map(|c| c.close).filter(|p| *p > 0.0)
            })
            .unwrap_or(0.0);
        if price <= 0.0 {
            self.status = shared(if self.work_mode {
                "No price for close"
            } else {
                "无法清仓：缺少现价"
            });
            cx.notify();
            return;
        }
        match self.portfolio.close_position(&code, price, 0.0, "清仓") {
            Ok(Some(_)) => {
                self.persist_portfolio();
                self.status = shared(if self.work_mode {
                    format!("Closed {code} @ {}", format_price(price))
                } else {
                    format!("已清仓 {code} @ {} 元", format_price(price))
                });
            }
            Ok(None) => {
                self.status = shared(if self.work_mode {
                    "No position"
                } else {
                    "当前无持仓"
                });
            }
            Err(e) => {
                self.status = shared(e.to_string());
            }
        }
        cx.notify();
    }

    pub(crate) fn undo_last_trade_for_selected(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let id = self
            .portfolio
            .trades
            .iter()
            .rev()
            .find(|t| t.code == code)
            .map(|t| t.id.clone());
        let Some(id) = id else {
            self.status = shared(if self.work_mode {
                "No trade to undo"
            } else {
                "没有可撤销的成交"
            });
            cx.notify();
            return;
        };
        if self.portfolio.remove_trade(&id) {
            self.persist_portfolio();
            self.status = shared(if self.work_mode {
                "Trade undone"
            } else {
                "已撤销最近一笔成交"
            });
        } else {
            self.status = shared(if self.work_mode {
                "Cannot undo (would break history)"
            } else {
                "无法撤销：会破坏后续卖出流水"
            });
        }
        cx.notify();
    }

    pub(crate) fn apply_portfolio_cash(&mut self, cx: &mut Context<Self>) {
        let raw = self.portfolio_cash_input.read(cx).value();
        let Some(v) = parse_f64(&raw) else {
            self.status = shared(if self.work_mode {
                "Invalid cash"
            } else {
                "现金金额无效"
            });
            cx.notify();
            return;
        };
        if v < 0.0 {
            self.status = shared(if self.work_mode {
                "Cash cannot be negative"
            } else {
                "现金不能为负"
            });
            cx.notify();
            return;
        }
        self.portfolio.cash = v;
        self.persist_portfolio();
        self.status = shared(if self.work_mode {
            format!("Cash = {v:.2}")
        } else {
            format!("现金已设为 {v:.2} 元")
        });
        cx.notify();
    }

    pub(crate) fn toggle_track_cash(&mut self, cx: &mut Context<Self>) {
        self.portfolio.track_cash = !self.portfolio.track_cash;
        self.persist_portfolio();
        cx.notify();
    }

    pub(crate) fn request_portfolio_ai(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let matched = self
            .candles_code
            .as_ref()
            .is_some_and(|c| c == code.as_str());
        if !matched || self.candles.is_empty() {
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: shared(if self.work_mode {
                    "Load daily chart first."
                } else {
                    "请先加载该标的日 K 数据。"
                }),
                source: AiSource::Local,
                note: None,
            };
            cx.notify();
            return;
        }
        let name = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.name.to_string())
            .unwrap_or_default();
        let pos = self.portfolio.position_state_of(&code);
        let (shares, avg_cost, realized) = pos
            .map(|p| (p.shares, p.avg_cost, p.realized_pnl))
            .unwrap_or((0.0, 0.0, 0.0));
        let last = self
            .symbols
            .iter()
            .find(|s| s.code == code)
            .map(|s| s.last)
            .filter(|p| *p > 0.0)
            .unwrap_or_else(|| self.candles.last().map(|c| c.close).unwrap_or(0.0));
        let date = self.candles.last().map(|c| c.date.to_string()).unwrap_or_default();
        let cache_key = format!(
            "pos:{}@{}:{:.4}:{:.4}",
            code,
            date,
            shares,
            avg_cost
        );

        if let Some(hit) = self.portfolio_ai_cache.get(&cache_key).cloned() {
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: hit.text.into(),
                source: hit.source,
                note: None,
            };
            self.portfolio_ai_key = Some(cache_key);
            cx.notify();
            return;
        }

        let Some(snap) = ai::build_position_advice(
            &self.candles,
            &code,
            &name,
            shares,
            avg_cost,
            last,
            realized,
        ) else {
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: shared("数据不足：需要至少 20 根有效日 K。"),
                source: AiSource::Local,
                note: None,
            };
            self.portfolio_ai_key = Some(cache_key);
            cx.notify();
            return;
        };

        let local = ai::local_position_advice(&snap);
        super::types::insert_ai_cache(
            &mut self.portfolio_ai_cache,
            cache_key.clone(),
            AiCacheEntry {
                text: local.clone(),
                source: AiSource::Local,
            },
        );
        self.portfolio_ai_key = Some(cache_key.clone());

        if !self.ai_config.enabled {
            self.portfolio_ai_panel = AiPanelState::Ready {
                text: local.into(),
                source: AiSource::Local,
                note: None,
            };
            cx.notify();
            return;
        }

        self.portfolio_ai_panel = AiPanelState::Loading {
            text: local.clone().into(),
        };
        self.portfolio_ai_gen = self.portfolio_ai_gen.wrapping_add(1);
        let req_id = self.portfolio_ai_gen;
        let cfg = self.ai_config.clone();
        let source_label = cfg.source_label();
        cx.spawn(async move |this, cx| {
            let res = smol::unblock(move || ai::llm_position_advice(&cfg, &snap)).await;
            let _ = this.update(cx, |app, cx| {
                if app.portfolio_ai_gen != req_id {
                    return;
                }
                match res {
                    Ok(text) if !text.trim().is_empty() => {
                        let source = AiSource::Llm {
                            label: source_label.clone(),
                        };
                        super::types::insert_ai_cache(
                            &mut app.portfolio_ai_cache,
                            cache_key.clone(),
                            AiCacheEntry {
                                text: text.clone(),
                                source: source.clone(),
                            },
                        );
                        app.portfolio_ai_panel = AiPanelState::Ready {
                            text: text.into(),
                            source,
                            note: None,
                        };
                    }
                    Ok(_) => {
                        app.portfolio_ai_panel = AiPanelState::Ready {
                            text: local.clone().into(),
                            source: AiSource::Local,
                            note: Some(shared("LLM 返回了空内容")),
                        };
                    }
                    Err(e) => {
                        app.portfolio_ai_panel = AiPanelState::Ready {
                            text: local.clone().into(),
                            source: AiSource::Local,
                            note: Some(shared(format!("LLM 请求失败：{e}"))),
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

}
