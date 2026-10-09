use crate::domain::market::{KlineSeries, SeriesIdentity};

use super::state::{RequestSlot, RequestTicket};

#[derive(Default)]
pub struct ChartController {
    pub series: RequestSlot<KlineSeries>,
    selected: Option<SeriesIdentity>,
    pub visible_identity: Option<SeriesIdentity>,
}

impl ChartController {
    pub fn select(&mut self, identity: SeriesIdentity) -> RequestTicket {
        let ticket = self.series.begin(identity.storage_key());
        self.selected = Some(identity);
        ticket
    }

    pub fn matches_visible(&self, identity: &SeriesIdentity) -> bool {
        self.visible_identity.as_ref() == Some(identity)
    }

    pub fn apply(&mut self, ticket: &RequestTicket, series: KlineSeries) -> bool {
        let Some(identity) = self.selected.as_ref() else {
            return false;
        };
        if series.code != identity.instrument.code
            || series.market != identity.instrument.market
            || series.adjustment != identity.adjustment
            || !self.series.is_current(ticket)
        {
            return false;
        }
        self.visible_identity = Some(identity.clone());
        self.series.apply(ticket, series)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{Adjustment, BarKind, Market};
    use crate::domain::money::Currency;

    fn series(code: &str) -> KlineSeries {
        KlineSeries {
            code: code.into(),
            market: Market::AShare,
            currency: Currency::Cny,
            source: "fixture".into(),
            as_of: 1,
            market_time: None,
            adjustment: Adjustment::Forward,
            candles: Vec::new(),
        }
    }

    fn identity(code: &str, bars: BarKind) -> SeriesIdentity {
        SeriesIdentity::stock(
            code,
            bars,
            if bars == BarKind::Daily {
                Adjustment::Forward
            } else {
                Adjustment::None
            },
        )
        .unwrap()
    }

    #[test]
    fn selection_change_rejects_old_series() {
        let mut controller = ChartController::default();
        let stale = controller.select(identity("600519", BarKind::Daily));
        let current = controller.select(identity("000001", BarKind::Daily));
        assert!(!controller.apply(&stale, series("600519")));
        assert!(controller.apply(&current, series("000001")));
    }
    #[test]
    fn failed_interval_switch_never_relabels_old_daily_bars() {
        let mut controller = ChartController::default();
        let daily = identity("600519", BarKind::Daily);
        let ticket = controller.select(daily.clone());
        assert!(controller.apply(&ticket, series("600519")));
        let five_minute = identity("600519", BarKind::Minute(5));
        let ticket = controller.select(five_minute.clone());
        assert!(controller.series.fail(&ticket, "fixture timeout"));
        assert!(!controller.matches_visible(&five_minute));
        assert!(controller.matches_visible(&daily));
        assert!(!controller.matches_visible(&identity("000001", BarKind::Daily)));
    }

    #[test]
    fn fast_a_b_a_switch_rejects_late_a_response_and_wrong_adjustment() {
        let mut controller = ChartController::default();
        let old = controller.select(identity("600519", BarKind::Daily));
        controller.select(identity("000001", BarKind::Daily));
        let new = controller.select(identity("600519", BarKind::Daily));
        assert!(!controller.apply(&old, series("600519")));
        let mut wrong = series("600519");
        wrong.adjustment = Adjustment::None;
        assert!(!controller.apply(&new, wrong));
        assert!(controller.apply(&new, series("600519")));
    }
}
