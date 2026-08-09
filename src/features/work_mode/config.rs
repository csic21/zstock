use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkModeConfig {
    pub enabled: bool,
    pub reveal_identity: bool,
}
