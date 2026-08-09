use super::state::{RequestSlot, RequestTicket};

#[derive(Default)]
pub struct DiscoveryController<T> {
    pub scan: RequestSlot<Vec<T>>,
}

impl<T> DiscoveryController<T> {
    pub fn begin(&mut self, strategy: &str) -> RequestTicket {
        self.scan.begin(strategy)
    }

    pub fn finish(&mut self, ticket: &RequestTicket, results: Vec<T>) -> bool {
        self.scan.apply(ticket, results)
    }

    pub fn fail(&mut self, ticket: &RequestTicket, message: impl Into<String>) -> bool {
        self.scan.fail(ticket, message)
    }

    pub fn cancel(&mut self) {
        self.scan.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::state::RequestState;

    #[test]
    fn cancellation_rejects_scan_completion() {
        let mut controller = DiscoveryController::default();
        let ticket = controller.begin("long-term");
        controller.cancel();
        assert!(!controller.finish(&ticket, vec!["600519"]));
        assert_eq!(controller.scan.state, RequestState::Idle);
    }
}
