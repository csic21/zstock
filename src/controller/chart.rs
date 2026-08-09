use crate::domain::market::KlineSeries;

use super::state::{RequestSlot, RequestTicket};

#[derive(Default)]
pub struct ChartController {
    pub series: RequestSlot<KlineSeries>,
}

impl ChartController {
    pub fn select(&mut self, code: &str) -> RequestTicket {
        self.series.begin(code)
    }

    pub fn apply(&mut self, ticket: &RequestTicket, series: KlineSeries) -> bool {
        self.series.apply(ticket, series)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::market::{Adjustment, Market};
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

    #[test]
    fn selection_change_rejects_old_series() {
        let mut controller = ChartController::default();
        let stale = controller.select("600519");
        let current = controller.select("000001");
        assert!(!controller.apply(&stale, series("600519")));
        assert!(controller.apply(&current, series("000001")));
    }
}
