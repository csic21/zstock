//! Selected-symbol buy alert actions and quote-crossing evaluation.

use gpui::{Context, Window};

use crate::data::alerts::{self, BuyAlert, BuyAlertBasis};
use crate::model::{format_price, shared};
use crate::notifications;

use super::helpers::parse_f64;
use super::StockApp;

/// A newly fired alert, passed to the quote loop for status/UI handling.
#[derive(Debug, Clone)]
pub(crate) struct BuyAlertHit {
    pub code: String,
    pub name: String,
    pub target_price: f64,
    pub current_price: f64,
}

impl StockApp {
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
        self.buy_alerts
            .insert(code.clone(), BuyAlert::new(price, basis));
        self.schedule_persist(cx);
        self.status = shared(if self.work_mode {
            format!("Threshold armed · {}", format_price(price))
        } else {
            format!("已开启 {} · 目标 {} 元", basis.label(), format_price(price))
        });
        cx.notify();
    }

    pub(crate) fn clear_selected_buy_alert(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        if self.buy_alerts.remove(&code).is_some() {
            self.schedule_persist(cx);
            self.status = shared(if self.work_mode {
                "Threshold cleared"
            } else {
                "已关闭买入提醒"
            });
            cx.notify();
        }
    }

    pub(crate) fn reset_selected_buy_alert(&mut self, cx: &mut Context<Self>) {
        let code = self.selected.to_string();
        let Some(alert) = self.buy_alerts.get_mut(&code) else {
            return;
        };
        if alert.triggered {
            alert.triggered = false;
            self.schedule_persist(cx);
            self.status = shared(if self.work_mode {
                "Threshold re-armed"
            } else {
                "已重新武装买入提醒"
            });
            cx.notify();
        }
    }

    /// Apply quote transitions to all active alerts. A target fires once when
    /// price enters from above; it rearms after a 0.3% move back above target.
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
            if !alert.is_valid() {
                continue;
            }

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
                    name,
                    target_price: alert.target_price,
                    current_price: *current,
                });
            }
        }

        if dirty {
            self.schedule_persist(cx);
        }
        hits
    }

    /// Best-effort OS notifications for fired alerts. Work mode intentionally
    /// suppresses the stock identity to avoid leaking it through a toast.
    pub(crate) fn notify_buy_alert_hits(&self, hits: &[BuyAlertHit]) {
        if self.work_mode {
            return;
        }
        for hit in hits {
            notifications::send(
                "ZStock · 买入提醒".into(),
                format!(
                    "{} ({}) 已到目标价 {} 元，当前 {} 元",
                    hit.name,
                    hit.code,
                    format_price(hit.target_price),
                    format_price(hit.current_price)
                ),
            );
        }
    }

    pub(crate) fn format_buy_alert_status(&self, hits: &[BuyAlertHit]) -> String {
        if hits.is_empty() {
            return String::new();
        }
        if self.work_mode {
            format!("🔔 {} threshold(s) reached", hits.len())
        } else if hits.len() == 1 {
            let hit = &hits[0];
            format!(
                "🔔 {} 到达目标价 {} 元（现价 {}）",
                hit.name,
                format_price(hit.target_price),
                format_price(hit.current_price)
            )
        } else {
            format!("🔔 {} 只自选已到买入目标价", hits.len())
        }
    }
}
