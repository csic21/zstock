use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};

pub const LATEST_SCHEMA_VERSION: i64 = 5;

const MIGRATION_1: &str = r#"
CREATE TABLE instruments (
    instrument_key TEXT PRIMARY KEY,
    market TEXT NOT NULL,
    asset_type TEXT NOT NULL,
    code TEXT NOT NULL,
    UNIQUE(market, asset_type, code)
);

CREATE TABLE daily_bars (
    instrument_key TEXT NOT NULL REFERENCES instruments(instrument_key),
    trade_date TEXT NOT NULL,
    open REAL NOT NULL,
    high REAL NOT NULL,
    low REAL NOT NULL,
    close REAL NOT NULL,
    volume INTEGER NOT NULL CHECK(volume >= 0),
    source TEXT NOT NULL,
    adjustment TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(instrument_key, trade_date, source, adjustment)
);

CREATE TABLE dataset_manifests (
    dataset_id TEXT PRIMARY KEY,
    content_sha256 TEXT NOT NULL,
    created_at TEXT NOT NULL,
    manifest_json TEXT NOT NULL
);

CREATE TABLE dataset_members (
    dataset_id TEXT NOT NULL REFERENCES dataset_manifests(dataset_id) ON DELETE RESTRICT,
    instrument_key TEXT NOT NULL REFERENCES instruments(instrument_key),
    PRIMARY KEY(dataset_id, instrument_key)
);

-- Frozen copies preserve old experiments even when the live daily_bars cache changes.
CREATE TABLE dataset_bars (
    dataset_id TEXT NOT NULL REFERENCES dataset_manifests(dataset_id) ON DELETE RESTRICT,
    instrument_key TEXT NOT NULL REFERENCES instruments(instrument_key),
    trade_date TEXT NOT NULL,
    open REAL NOT NULL,
    high REAL NOT NULL,
    low REAL NOT NULL,
    close REAL NOT NULL,
    volume INTEGER NOT NULL CHECK(volume >= 0),
    source TEXT NOT NULL,
    adjustment TEXT NOT NULL,
    PRIMARY KEY(dataset_id, instrument_key, trade_date)
);

CREATE TABLE strategies (
    strategy_id TEXT PRIMARY KEY,
    schema_version INTEGER NOT NULL,
    normalized_json TEXT NOT NULL,
    spec_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE experiments (
    experiment_id TEXT PRIMARY KEY,
    status TEXT NOT NULL,
    dataset_id TEXT NOT NULL REFERENCES dataset_manifests(dataset_id) ON DELETE RESTRICT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    record_json TEXT NOT NULL
);

CREATE TABLE experiment_candidates (
    experiment_id TEXT NOT NULL REFERENCES experiments(experiment_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    strategy_id TEXT REFERENCES strategies(strategy_id) ON DELETE RESTRICT,
    normalized_hash TEXT,
    candidate_json TEXT NOT NULL,
    PRIMARY KEY(experiment_id, ordinal)
);

CREATE TABLE backtest_runs (
    run_id TEXT PRIMARY KEY,
    experiment_id TEXT NOT NULL REFERENCES experiments(experiment_id) ON DELETE CASCADE,
    strategy_id TEXT NOT NULL REFERENCES strategies(strategy_id) ON DELETE RESTRICT,
    status TEXT NOT NULL,
    config_json TEXT NOT NULL,
    report_json TEXT,
    failure_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE backtest_trades (
    run_id TEXT NOT NULL REFERENCES backtest_runs(run_id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    trade_json TEXT NOT NULL,
    PRIMARY KEY(run_id, ordinal)
);

CREATE TABLE daily_equity (
    run_id TEXT NOT NULL REFERENCES backtest_runs(run_id) ON DELETE CASCADE,
    trade_date TEXT NOT NULL,
    equity_json TEXT NOT NULL,
    PRIMARY KEY(run_id, trade_date)
);

CREATE TABLE paper_signals (
    signal_id TEXT PRIMARY KEY,
    strategy_id TEXT NOT NULL REFERENCES strategies(strategy_id) ON DELETE RESTRICT,
    instrument_key TEXT NOT NULL REFERENCES instruments(instrument_key),
    signal_date TEXT NOT NULL,
    signal_kind TEXT NOT NULL,
    record_json TEXT NOT NULL,
    UNIQUE(strategy_id, instrument_key, signal_date, signal_kind)
);

CREATE INDEX daily_bars_lookup
    ON daily_bars(instrument_key, adjustment, trade_date);
CREATE INDEX dataset_bars_lookup
    ON dataset_bars(dataset_id, instrument_key, trade_date);
CREATE INDEX experiments_created
    ON experiments(created_at DESC);
CREATE INDEX runs_experiment
    ON backtest_runs(experiment_id, strategy_id);
"#;

const MIGRATION_2: &str = r#"
CREATE TABLE paper_candidates (
    candidate_id TEXT PRIMARY KEY,
    strategy_id TEXT NOT NULL REFERENCES strategies(strategy_id) ON DELETE RESTRICT,
    dataset_id TEXT NOT NULL REFERENCES dataset_manifests(dataset_id) ON DELETE RESTRICT,
    experiment_id TEXT NOT NULL REFERENCES experiments(experiment_id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    record_json TEXT NOT NULL
);

CREATE TABLE paper_runs (
    candidate_id TEXT NOT NULL REFERENCES paper_candidates(candidate_id) ON DELETE CASCADE,
    as_of TEXT NOT NULL,
    generated_at TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(candidate_id, as_of)
);

CREATE TABLE paper_trades (
    trade_id TEXT PRIMARY KEY,
    candidate_id TEXT NOT NULL REFERENCES paper_candidates(candidate_id) ON DELETE CASCADE,
    exit_date TEXT NOT NULL,
    record_json TEXT NOT NULL
);

CREATE INDEX paper_candidates_status ON paper_candidates(status, created_at);
CREATE INDEX paper_runs_latest ON paper_runs(candidate_id, as_of DESC);
CREATE INDEX paper_trades_candidate ON paper_trades(candidate_id, exit_date);
"#;

const MIGRATION_3: &str = r#"
CREATE TABLE robustness_reports (
    experiment_id TEXT NOT NULL REFERENCES experiments(experiment_id) ON DELETE CASCADE,
    strategy_id TEXT NOT NULL REFERENCES strategies(strategy_id) ON DELETE RESTRICT,
    validation_version TEXT NOT NULL,
    report_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(experiment_id, strategy_id, validation_version)
);

CREATE INDEX robustness_experiment
    ON robustness_reports(experiment_id, strategy_id);
"#;

const MIGRATION_4: &str = r#"
CREATE TABLE sealed_test_reports (
    experiment_id TEXT NOT NULL REFERENCES experiments(experiment_id) ON DELETE CASCADE,
    strategy_id TEXT NOT NULL REFERENCES strategies(strategy_id) ON DELETE RESTRICT,
    consumed_at TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY(experiment_id, strategy_id)
);
"#;

const MIGRATION_5: &str = r#"
CREATE TABLE strategy_library (
    record_id TEXT PRIMARY KEY,
    experiment_id TEXT NOT NULL REFERENCES experiments(experiment_id) ON DELETE CASCADE,
    strategy_id TEXT NOT NULL REFERENCES strategies(strategy_id) ON DELETE RESTRICT,
    dataset_id TEXT NOT NULL,
    status TEXT NOT NULL,
    win_rate_pct REAL NOT NULL,
    retained_at TEXT NOT NULL,
    record_json TEXT NOT NULL,
    UNIQUE(experiment_id, strategy_id)
);

CREATE INDEX strategy_library_status_win_rate
    ON strategy_library(status, win_rate_pct DESC, retained_at DESC);
"#;

pub fn migrate(connection: &mut Connection) -> Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (\
            version INTEGER PRIMARY KEY,\
            applied_at TEXT NOT NULL\
        );",
    )?;
    let current: i64 = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    if current > LATEST_SCHEMA_VERSION {
        bail!("database schema {current} is newer than supported {LATEST_SCHEMA_VERSION}");
    }
    for version in (current + 1)..=LATEST_SCHEMA_VERSION {
        let sql = match version {
            1 => MIGRATION_1,
            2 => MIGRATION_2,
            3 => MIGRATION_3,
            4 => MIGRATION_4,
            5 => MIGRATION_5,
            _ => bail!("missing migration {version}"),
        };
        apply_one(connection, version, sql)?;
    }
    Ok(())
}

fn apply_one(connection: &mut Connection, version: i64, sql: &str) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction
        .execute_batch(sql)
        .with_context(|| format!("apply SQLite migration {version}"))?;
    transaction.execute(
        "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
        params![version, chrono::Utc::now().to_rfc3339()],
    )?;
    transaction.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_migration_rolls_back_every_statement() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
            )
            .unwrap();
        let error = apply_one(
            &mut connection,
            99,
            "CREATE TABLE should_rollback(id INTEGER); INVALID SQL;",
        )
        .unwrap_err();
        assert!(error.to_string().contains("migration 99"));
        let exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'should_rollback'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0);
    }
}
