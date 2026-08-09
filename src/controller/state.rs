#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RequestState<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTicket {
    pub key: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestSlot<T> {
    active_key: Option<String>,
    generation: u64,
    pub state: RequestState<T>,
}

impl<T> Default for RequestSlot<T> {
    fn default() -> Self {
        Self {
            active_key: None,
            generation: 0,
            state: RequestState::Idle,
        }
    }
}

impl<T> RequestSlot<T> {
    pub fn begin(&mut self, key: impl Into<String>) -> RequestTicket {
        self.generation = self.generation.wrapping_add(1);
        let key = key.into();
        self.active_key = Some(key.clone());
        self.state = RequestState::Loading;
        RequestTicket {
            key,
            generation: self.generation,
        }
    }

    pub fn apply(&mut self, ticket: &RequestTicket, value: T) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.state = RequestState::Ready(value);
        true
    }

    pub fn fail(&mut self, ticket: &RequestTicket, message: impl Into<String>) -> bool {
        if !self.is_current(ticket) {
            return false;
        }
        self.state = RequestState::Failed(message.into());
        true
    }

    pub fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.active_key = None;
        self.state = RequestState::Idle;
    }

    pub fn is_current(&self, ticket: &RequestTicket) -> bool {
        self.generation == ticket.generation
            && self.active_key.as_deref() == Some(ticket.key.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_selection_drops_old_async_response() {
        let mut slot = RequestSlot::default();
        let old = slot.begin("600519");
        let current = slot.begin("00700");
        assert!(!slot.apply(&old, 1));
        assert!(slot.apply(&current, 2));
        assert_eq!(slot.state, RequestState::Ready(2));
    }

    #[test]
    fn cancellation_invalidates_in_flight_scan() {
        let mut slot = RequestSlot::default();
        let ticket = slot.begin("scan");
        slot.cancel();
        assert!(!slot.apply(&ticket, vec![1]));
        assert_eq!(slot.state, RequestState::Idle);
    }
}
