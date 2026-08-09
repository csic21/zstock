use crate::domain::market::QuoteRecord;

use super::state::{RequestSlot, RequestTicket};

#[derive(Default)]
pub struct MarketController {
    pub quotes: RequestSlot<Vec<QuoteRecord>>,
}

impl MarketController {
    pub fn begin_refresh(&mut self, codes: &[String]) -> RequestTicket {
        let key = codes.join(",");
        self.quotes.begin(key)
    }

    pub fn apply_refresh(&mut self, ticket: &RequestTicket, records: Vec<QuoteRecord>) -> bool {
        self.quotes.apply(ticket, records)
    }

    pub fn fail_refresh(&mut self, ticket: &RequestTicket, message: impl Into<String>) -> bool {
        self.quotes.fail(ticket, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::state::RequestState;
    use crate::domain::market::{Availability, Freshness, Market};
    use crate::domain::money::Currency;

    fn quote(code: &str) -> QuoteRecord {
        QuoteRecord {
            code: code.into(),
            market: Market::AShare,
            currency: Currency::Cny,
            name: code.into(),
            price: Some(10.0),
            change_pct: Some(0.0),
            volume: Some(1),
            source: "fixture".into(),
            fetched_at: 1,
            market_time: Some("2026-08-09 10:00:00".into()),
            availability: Availability::Available,
            freshness: Freshness::Live,
        }
    }

    #[test]
    fn stale_quote_refresh_cannot_replace_current_request() {
        let mut controller = MarketController::default();
        let stale = controller.begin_refresh(&["600519".into()]);
        let current = controller.begin_refresh(&["000001".into()]);
        assert!(!controller.apply_refresh(&stale, vec![quote("600519")]));
        assert!(controller.apply_refresh(&current, vec![quote("000001")]));
        assert!(matches!(controller.quotes.state, RequestState::Ready(_)));
    }
}
