use std::collections::HashMap;

use crate::domain::market::{Freshness, QuoteRecord};

use super::state::{RequestSlot, RequestTicket};

#[derive(Default)]
pub struct MarketController {
    pub quotes: RequestSlot<Vec<QuoteRecord>>,
    /// Retained per instrument across partial market batches and loading states.
    pub quotes_by_code: HashMap<String, QuoteRecord>,
    pub errors: Vec<String>,
}

impl MarketController {
    pub fn begin_refresh(&mut self, codes: &[String]) -> RequestTicket {
        let key = codes.join(",");
        self.quotes.begin(key)
    }

    pub fn apply_refresh(&mut self, ticket: &RequestTicket, records: Vec<QuoteRecord>) -> bool {
        self.apply_refresh_with_errors(ticket, records, Vec::new())
    }

    pub fn age_quotes(&mut self, now_millis: i64) -> bool {
        let mut changed = false;
        for record in self.quotes_by_code.values_mut() {
            let freshness = record.effective_freshness(now_millis);
            changed |= record.freshness != freshness;
            record.freshness = freshness;
        }
        changed
    }

    pub fn quote_for(&self, code: &str) -> Option<&QuoteRecord> {
        self.quotes_by_code.get(code)
    }

    pub fn apply_refresh_with_errors(
        &mut self,
        ticket: &RequestTicket,
        records: Vec<QuoteRecord>,
        errors: Vec<String>,
    ) -> bool {
        if !self.quotes.is_current(ticket) {
            return false;
        }
        for record in &records {
            self.quotes_by_code
                .insert(record.code.clone(), record.clone());
        }
        // Bound retained instruments while keeping the current batch intact.
        if self.quotes_by_code.len() > 512 {
            let requested: std::collections::HashSet<_> =
                records.iter().map(|r| r.code.as_str()).collect();
            let mut old: Vec<_> = self
                .quotes_by_code
                .values()
                .filter(|record| !requested.contains(record.code.as_str()))
                .map(|record| (record.fetched_at, record.code.clone()))
                .collect();
            old.sort();
            for (_, code) in old
                .into_iter()
                .take(self.quotes_by_code.len().saturating_sub(512))
            {
                self.quotes_by_code.remove(&code);
            }
        }
        self.errors = errors;
        self.quotes.apply(ticket, records)
    }

    pub fn fail_refresh(&mut self, ticket: &RequestTicket, message: impl Into<String>) -> bool {
        if !self.quotes.is_current(ticket) {
            return false;
        }
        let message = message.into();
        for code in ticket.key.split(',') {
            if let Some(record) = self.quotes_by_code.get_mut(code) {
                record.freshness = Freshness::Stale;
            }
        }
        self.errors = vec![message.clone()];
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
    #[test]
    fn separate_market_batches_preserve_unwatched_holdings_and_metadata_changes() {
        let mut controller = MarketController::default();
        let a = controller.begin_refresh(&["600519".into()]);
        assert!(controller.apply_refresh(&a, vec![quote("600519")]));
        let hk = controller.begin_refresh(&["00700".into()]);
        assert!(controller.quote_for("600519").is_some());
        assert!(controller.apply_refresh(&hk, vec![quote("00700")]));
        assert!(controller.quote_for("600519").is_some());
        let a = controller.begin_refresh(&["600519".into()]);
        let mut stale = quote("600519");
        stale.freshness = Freshness::Stale;
        assert!(controller.apply_refresh_with_errors(&a, vec![stale], vec!["timeout".into()]));
        assert_eq!(controller.quote_for("600519").unwrap().price, Some(10.0));
        assert_eq!(
            controller.quote_for("600519").unwrap().freshness,
            Freshness::Stale
        );
        assert_eq!(controller.errors, ["timeout"]);
        let late = a;
        let current = controller.begin_refresh(&["00700".into()]);
        assert!(!controller.fail_refresh(&late, "late failure"));
        assert!(controller.apply_refresh(&current, vec![quote("00700")]));
        assert_eq!(
            controller.quote_for("600519").unwrap().freshness,
            Freshness::Stale
        );
    }
    #[test]
    fn elapsed_time_alone_notifies_stale_transition_without_price_change() {
        let mut controller = MarketController::default();
        let mut record = quote("600519");
        let now = crate::domain::market::parse_market_timestamp("2026-10-09 10:00:00").unwrap();
        record.market_time = Some("2026-10-09 10:00:00".into());
        let ticket = controller.begin_refresh(&["600519".into()]);
        controller.apply_refresh(&ticket, vec![record]);
        assert!(!controller.age_quotes(now));
        assert!(controller.age_quotes(now + 31_000));
        assert_eq!(
            controller.quote_for("600519").unwrap().freshness,
            Freshness::Delayed
        );
        assert!(controller.age_quotes(now + 301_000));
        assert_eq!(controller.quote_for("600519").unwrap().price, Some(10.0));
        assert_eq!(
            controller.quote_for("600519").unwrap().freshness,
            Freshness::Stale
        );
        assert!(!controller.age_quotes(now + 302_000));
    }
}
