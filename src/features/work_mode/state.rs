#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkModeState {
    pub enabled: bool,
    pub identity_visible: bool,
    pub hide_generation: u64,
}

impl WorkModeState {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.identity_visible = false;
        }
    }
}
