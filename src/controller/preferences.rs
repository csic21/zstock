use crate::services::repositories::ConfigRepository;
use crate::storage::AppConfig;

pub struct PreferencesController<R> {
    repository: R,
}

impl<R: ConfigRepository> PreferencesController<R> {
    pub fn new(repository: R) -> Self {
        Self { repository }
    }

    pub fn load(&self) -> anyhow::Result<AppConfig> {
        self.repository.load()
    }

    pub fn save(&self, config: &AppConfig) -> anyhow::Result<()> {
        self.repository.save(config)
    }
}
