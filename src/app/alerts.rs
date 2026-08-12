//! Selected-symbol multi-leg alerts and quote-crossing evaluation.

use gpui::{Context, Window};

use crate::data::alerts::{self, AlertLeg, BuyAlert, BuyAlertBasis};
use crate::model::{format_price, shared};
use crate::notifications;

use super::StockApp;
use super::helpers::parse_f64;

/// A newly fired alert, passed to the quote loop for status/UI handling.
#[derive(Debug, Clone)]
pub(crate) struct BuyAlertHit {
    pub code: String,
    pub name: String,
    pub target_price: f64,
    pub current_price: f64,
    pub leg: AlertLeg,
}

impl StockApp {
    pub(crate) fn install_execution_alerts(&mut self, cx: &mut Context<Self>) {
        let Some(levels) = self.current_levels() else {
            self.status = shared("加载至少 30 根日 K 后才能生成三道提醒");
            cx.notify();
            return;
        };
        let code = self.selected.to_string();
        let mut alert = BuyAlert::new(levels.buy_high, BuyAlertBasis::Technical);
        alert.sell_price = Some(levels.sell_low);
        alert.stop_price = Some(levels.buy_low);
        self.buy_alerts.insert(code, alert);
        self.schedule_persist(cx);
        self.status = shared(format!(
            "三道提醒已开启 · 观察 {} · 失效 {} · 目标 {}",
            format_price(levels.buy_high),
            format_price(levels.buy_low),
            format_price(levels.sell_low)
        ));
        cx.notify();
    }

    pub(crate) fn selected_buy_alert(&self) -> Option<BuyAlert> {
        self.buy_alerts.get(self.selected.as_ref()).cloned()
    }

    pub(crate) fn selected_recommended_buy_price(&self) -> Option<f64> {
        if !matches!(self.chart_kind, super::ChartKind::DayK) {
            return None;
        }
        self.current_levels()
            .map(|levels| levels.buy_high)
            .filter(|price| price.is_finite() && *price > 0.0)
    }

    pub(crate) fn selected_recommended_sell_price(&self) -> Option<f64> {
        if !matches!(self.chart_kind, super::ChartKind::DayK) {
            return None;
        }
        self.current_levels()
            .map(|levels| levels.sell_low)
            .filter(|price| price.is_finite() && *price > 0.0)
    }

    pub(crate) fn set_manual_buy_alert(&mut self, cx: &mut Context<Self>) {
        let raw = self.alert_price_input.read(cx).value().to_string();
        let Some(price) = parse_f64(&raw).filter(|v| *v > 0.0) else {
            self.status = shared(if self.work_mode {
                "Invalid threshold"
            } else {
                "目标价须为大于 0 的数字"
            });
            cx.notify();
            return;
        };
        self.install_buy_alert(price, BuyAlertBasis::Manual, cx);
    }

    pub(crate) fn set_recommended_buy_alert(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(price) = self.selected_recommended_buy_price() else {
            self.status = shared(if self.work_mode {
                "Need daily bars for a threshold"
            } else {
                "加载至少 30 根日 K 后才能生成参考建仓价"
            });
            cx.notify();
            return;
        };
        self.alert_price_input.update(cx, |input, cx| {
            input.set_value(format_price(price), window, cx);
        });
        self.install_buy_alert(price, BuyAlertBasis::Technical, cx);
    }

    fn install_buy_alert(&mut self, price: f64, basis: BuyAlertBasis, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let mut alert = self
            .buy_alerts
            .get(&code)
            .cloned()
            .unwrap_or_else(|| BuyAlert::new(price, basis));
        alert.target_price = price;
        alert.basis = basis;
        alert.triggered = false;
        self.buy_alerts.insert(code, alert);
        self.schedule_persist(cx);
        self.status = shared(if self.work_mode {
            format!("Buy zone · {}", format_price(price))
        } else {
            format!(
                "已开启买入观察 · {} 元 · {}",
                format_price(price),
                basis.label()
            )
        });
        cx.notify();
    }

    pub(crate) fn set_sell_alert_from_levels(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(price) = self.selected_recommended_sell_price() else {
            self.status = shared(if self.work_mode {
                "Need daily bars for take-profit"
            } else {
                "加载日 K 后才能生成参考减仓价"
            });
            cx.notify();
            return;
        };
        self.set_sell_alert(price, window, cx);
    }

    pub(crate) fn set_sell_alert_manual(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.alert_price_input.read(cx).value().to_string();
        let Some(price) = parse_f64(&raw).filter(|v| *v > 0.0) else {
            self.status = shared(if self.work_mode {
                "Enter a sell target first"
            } else {
                "请先在输入框填写止盈价"
            });
            cx.notify();
            return;
        };
        self.set_sell_alert(price, window, cx);
    }

    fn set_sell_alert(&mut self, price: f64, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let mut alert = self.buy_alerts.get(&code).cloned().unwrap_or_else(|| {
            // 若尚无买入腿，用现价占位一个极低买价（不触发），只启用卖出腿
            let last = self
                .symbols
                .iter()
                .find(|s| s.code == code)
                .map(|s| s.last)
                .filter(|v| *v > 0.0)
                .unwrap_or(price * 0.5);
            BuyAlert::new(last.min(price * 0.5).max(0.01), BuyAlertBasis::Manual)
        });
        alert.sell_price = Some(price);
        alert.sell_triggered = false;
        self.buy_alerts.insert(code, alert);
        self.alert_price_input.update(cx, |input, cx| {
            input.set_value(format_price(price), window, cx);
        });
        self.schedule_persist(cx);
        self.status = shared(if self.work_mode {
            format!("Take-profit · {}", format_price(price))
        } else {
            format!("已设置止盈/减仓观察 · {} 元", format_price(price))
        });
        cx.notify();
    }

    pub(crate) fn set_stop_alert_manual(&mut self, cx: &mut Context<Self>) {
        let raw = self.alert_price_input.read(cx).value().to_string();
        let Some(price) = parse_f64(&raw).filter(|v| *v > 0.0) else {
            self.status = shared(if self.work_mode {
                "Enter a stop price first"
            } else {
                "请先在输入框填写止损价"
            });
            cx.notify();
            return;
        };
        let code = self.selected.to_string();
        let mut alert = self
            .buy_alerts
            .get(&code)
            .cloned()
            .unwrap_or_else(|| BuyAlert::new(price * 1.05, BuyAlertBasis::Manual));
        alert.stop_price = Some(price);
        alert.stop_triggered = false;
        self.buy_alerts.insert(code, alert);
        self.schedule_persist(cx);
        self.status = shared(if self.work_mode {
            format!("Stop armed · {}", format_price(price))
        } else {
            format!("已设置止损观察 · {} 元", format_price(price))
        });
        cx.notify();
    }

    pub(crate) fn clear_selected_buy_alert(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        if self.buy_alerts.remove(&code).is_some() {
            self.schedule_persist(cx);
            self.status = shared(if self.work_mode {
                "Alerts cleared"
            } else {
                "已关闭该标的全部价位提醒"
            });
            cx.notify();
        }
    }

    pub(crate) fn reset_selected_buy_alert(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let Some(alert) = self.buy_alerts.get_mut(&code) else {
            return;
        };
        let mut changed = false;
        if alert.triggered {
            alert.triggered = false;
            changed = true;
        }
        if alert.sell_triggered {
            alert.sell_triggered = false;
            changed = true;
        }
        if alert.stop_triggered {
            alert.stop_triggered = false;
            changed = true;
        }
        if changed {
            self.schedule_persist(cx);
            self.status = shared(if self.work_mode {
                "All legs re-armed"
            } else {
                "已重新武装全部提醒腿"
            });
            cx.notify();
        }
    }

    /// Apply quote transitions to all active multi-leg alerts.
    pub(crate) fn evaluate_buy_alerts(
        &mut self,
        transitions: &[(String, f64, f64)],
        cx: &mut Context<Self>,
    ) -> Vec<BuyAlertHit> {
        let mut hits = Vec::new();
        let mut dirty = false;

        for (code, previous, current) in transitions {
            let name = self
                .symbols
                .iter()
                .find(|symbol| symbol.code == *code)
                .map(|symbol| symbol.name.to_string())
                .unwrap_or_else(|| code.clone());
            let Some(alert) = self.buy_alerts.get_mut(code) else {
                continue;
            };

            // —— 买入腿 ——
            if alert.is_valid() {
                if alert.triggered {
                    if alerts::should_rearm(*current, alert.target_price) {
                        alert.triggered = false;
                        dirty = true;
                    }
                } else if alerts::crossed_down(*previous, *current, alert.target_price) {
                    alert.triggered = true;
                    dirty = true;
                    hits.push(BuyAlertHit {
                        code: code.clone(),
                        name: name.clone(),
                        target_price: alert.target_price,
                        current_price: *current,
                        leg: AlertLeg::Buy,
                    });
                }
            }

            // —— 止盈腿 ——
            if let Some(sell) = alert.sell_price.filter(|p| p.is_finite() && *p > 0.0) {
                if alert.sell_triggered {
                    if alerts::should_rearm_sell(*current, sell) {
                        alert.sell_triggered = false;
                        dirty = true;
                    }
                } else if alerts::crossed_up(*previous, *current, sell) {
                    alert.sell_triggered = true;
                    dirty = true;
                    hits.push(BuyAlertHit {
                        code: code.clone(),
                        name: name.clone(),
                        target_price: sell,
                        current_price: *current,
                        leg: AlertLeg::Sell,
                    });
                }
            }

            // —— 止损腿 ——
            if let Some(stop) = alert.stop_price.filter(|p| p.is_finite() && *p > 0.0) {
                if alert.stop_triggered {
                    if alerts::should_rearm_stop(*current, stop) {
                        alert.stop_triggered = false;
                        dirty = true;
                    }
                } else if alerts::crossed_down(*previous, *current, stop) {
                    alert.stop_triggered = true;
                    dirty = true;
                    hits.push(BuyAlertHit {
                        code: code.clone(),
                        name: name.clone(),
                        target_price: stop,
                        current_price: *current,
                        leg: AlertLeg::Stop,
                    });
                }
            }
        }

        if dirty {
            self.schedule_persist(cx);
        }
        hits
    }

    pub(crate) fn notify_buy_alert_hits(&mut self, hits: &[BuyAlertHit]) {
        // 到价自动记日记，便于复盘（先于通知，确保落盘）
        self.record_alert_journal_hits(hits);
        for hit in hits {
            let leg = hit.leg.label(false);
            let title = format!("ZStock · {leg}");
            let body = format!(
                "{} {} · 目标 {} · 现价 {}",
                hit.code,
                hit.name,
                format_price(hit.target_price),
                format_price(hit.current_price)
            );
            notifications::send(title, body);
        }
    }

    pub(crate) fn format_buy_alert_status(&self, hits: &[BuyAlertHit]) -> String {
        if hits.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = hits
            .iter()
            .map(|h| {
                format!(
                    "{} {}@{}",
                    h.leg.label(self.work_mode),
                    h.code,
                    format_price(h.target_price)
                )
            })
            .collect();
        if self.work_mode {
            format!("Alert · {}", parts.join(" · "))
        } else {
            format!("🔔 {}", parts.join(" · "))
        }
    }
}
