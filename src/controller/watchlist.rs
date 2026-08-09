use std::collections::HashSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchlistController {
    codes: Vec<String>,
}

impl WatchlistController {
    pub fn replace(&mut self, codes: impl IntoIterator<Item = String>) {
        let mut seen = HashSet::new();
        self.codes = codes
            .into_iter()
            .filter(|code| seen.insert(code.clone()))
            .collect();
    }

    pub fn codes(&self) -> &[String] {
        &self.codes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_preserves_first_seen_order_and_removes_duplicates() {
        let mut controller = WatchlistController::default();
        controller.replace(["00700".into(), "600519".into(), "00700".into()]);
        assert_eq!(controller.codes(), ["00700", "600519"]);
    }
}
