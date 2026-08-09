use std::path::PathBuf;

pub fn app_data_dir() -> PathBuf {
    let base = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("stock-analysis")
}

pub fn config() -> PathBuf {
    app_data_dir().join("config.json")
}

pub fn portfolio() -> PathBuf {
    app_data_dir().join("portfolio.json")
}

pub fn journal() -> PathBuf {
    app_data_dir().join("journal.json")
}

pub fn performance_report() -> PathBuf {
    app_data_dir().join("performance-report.json")
}
