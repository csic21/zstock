use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, MAIN_DB, OptionalExtension, Transaction, params};

use crate::domain::backtest::validation::{RobustnessReport, SealedTestResult};
use crate::domain::dataset::{
    DatasetManifest, FrozenDataset, FrozenSeries, dataset_content_sha256, validate_series,
};
use crate::domain::experiment::{ExperimentCandidate, ExperimentRecord};
use crate::domain::market::{Adjustment, AssetType, CandleRecord, InstrumentId, Market};
use crate::domain::paper::{PaperCandidate, PaperRunResult};
use crate::domain::strategy::{StrategySpec, normalized_json, strategy_id};
use crate::domain::strategy_library::{LibraryStatus, StrategyLibraryRecord};
use crate::services::backtest_repository::{
    BacktestRepository, StoredBacktestRun, StoredRunStatus,
};
use crate::services::dataset_repository::{DatasetRepository, FreezeDatasetRequest, IngestSummary};
use crate::services::experiment_repository::ExperimentRepository;
use crate::services::paper_trading::PaperTradingRepository;
use crate::services::strategy_library::StrategyLibraryRepository;
use crate::services::validation_repository::ValidationRepository;

use super::migrations;

pub struct SqliteLabStore {
    connection: Mutex<Connection>,
}

impl SqliteLabStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open strategy lab database {}", path.display()))?;
        Self::from_connection(connection, true)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?, false)
    }

    fn from_connection(mut connection: Connection, use_wal: bool) -> Result<Self> {
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        if use_wal {
            connection.pragma_update(None, "journal_mode", "WAL")?;
            connection.pragma_update(None, "synchronous", "NORMAL")?;
        }
        migrations::migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    pub fn backup_to(&self, path: &Path) -> Result<()> {
        let connection = self.connection.lock().expect("SQLite mutex poisoned");
        connection
            .backup(MAIN_DB, path, None)
            .with_context(|| format!("back up strategy lab database to {}", path.display()))
    }

    pub fn restore_from(&self, path: &Path) -> Result<()> {
        let mut connection = self.connection.lock().expect("SQLite mutex poisoned");
        connection
            .restore(MAIN_DB, path, None::<fn(rusqlite::backup::Progress)>)
            .with_context(|| format!("restore strategy lab database from {}", path.display()))?;
        migrations::migrate(&mut connection)
    }

    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.connection.lock().expect("SQLite mutex poisoned")
    }
}

impl DatasetRepository for SqliteLabStore {
    fn upsert_series(&self, series: &FrozenSeries) -> Result<IngestSummary> {
        let issues = validate_series(series);
        if !issues.is_empty() {
            bail!(
                "invalid daily bar series: {}",
                serde_json::to_string(&issues)?
            );
        }
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        insert_instrument(&transaction, &series.instrument)?;
        let now = chrono::Utc::now().to_rfc3339();
        let mut summary = IngestSummary {
            inserted: 0,
            updated: 0,
            unchanged: 0,
        };
        for candle in &series.candles {
            let existing: Option<(u64, u64, u64, u64, u64)> = transaction
                .query_row(
                    "SELECT open, high, low, close, volume FROM daily_bars \
                     WHERE instrument_key=?1 AND trade_date=?2 AND source=?3 AND adjustment=?4",
                    params![
                        series.instrument.storage_key(),
                        candle.time,
                        series.source,
                        adjustment_name(series.adjustment),
                    ],
                    |row| {
                        Ok((
                            row.get::<_, f64>(0)?.to_bits(),
                            row.get::<_, f64>(1)?.to_bits(),
                            row.get::<_, f64>(2)?.to_bits(),
                            row.get::<_, f64>(3)?.to_bits(),
                            read_volume(row, 4)?,
                        ))
                    },
                )
                .optional()?;
            let incoming = (
                candle.open.to_bits(),
                candle.high.to_bits(),
                candle.low.to_bits(),
                candle.close.to_bits(),
                candle.volume,
            );
            match existing {
                None => summary.inserted += 1,
                Some(value) if value == incoming => summary.unchanged += 1,
                Some(_) => summary.updated += 1,
            }
            let volume = i64::try_from(candle.volume).context("daily volume exceeds SQLite i64")?;
            transaction.execute(
                r#"INSERT INTO daily_bars(
                    instrument_key, trade_date, open, high, low, close, volume, source, adjustment, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(instrument_key, trade_date, source, adjustment) DO UPDATE SET
                    open=excluded.open, high=excluded.high, low=excluded.low, close=excluded.close,
                    volume=excluded.volume, updated_at=excluded.updated_at"#,
                params![
                    series.instrument.storage_key(),
                    candle.time,
                    candle.open,
                    candle.high,
                    candle.low,
                    candle.close,
                    volume,
                    series.source,
                    adjustment_name(series.adjustment),
                    now,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(summary)
    }

    fn freeze_dataset(&self, request: &FreezeDatasetRequest) -> Result<DatasetManifest> {
        if request.instruments.is_empty() {
            bail!("cannot freeze an empty universe");
        }
        if request.interval.start > request.interval.end {
            bail!("dataset interval start must not be after end");
        }
        let mut connection = self.connection();
        let series = load_live_series(&connection, request)?;
        let mut quality_issues = Vec::new();
        for item in &series {
            quality_issues.extend(validate_series(item));
        }
        if !quality_issues.is_empty() {
            bail!(
                "cannot freeze invalid series: {}",
                serde_json::to_string(&quality_issues)?
            );
        }
        let content_sha256 = dataset_content_sha256(request.market, &series);
        let id = format!("dataset-sha256:{content_sha256}");
        if let Some(existing) = load_manifest_from_connection(&connection, &id)? {
            return Ok(existing);
        }
        let manifest = DatasetManifest {
            id: id.clone(),
            created_at: chrono::Utc::now().to_rfc3339(),
            market: request.market,
            adjustment: request.adjustment,
            source_versions: request.source_versions.clone(),
            instruments: series.iter().map(|item| item.instrument.clone()).collect(),
            interval: request.interval.clone(),
            content_sha256,
            known_biases: request.known_biases.clone(),
            quality_issues,
        };
        let transaction = connection.transaction()?;
        transaction.execute(
            r#"INSERT INTO dataset_manifests(dataset_id, content_sha256, created_at, manifest_json)
             VALUES (?1, ?2, ?3, ?4)"#,
            params![
                manifest.id,
                manifest.content_sha256,
                manifest.created_at,
                serde_json::to_string(&manifest)?,
            ],
        )?;
        for item in &series {
            insert_instrument(&transaction, &item.instrument)?;
            transaction.execute(
                "INSERT INTO dataset_members(dataset_id, instrument_key) VALUES (?1, ?2)",
                params![manifest.id, item.instrument.storage_key()],
            )?;
            for candle in &item.candles {
                let volume =
                    i64::try_from(candle.volume).context("daily volume exceeds SQLite i64")?;
                transaction.execute(
                    r#"INSERT INTO dataset_bars(
                        dataset_id, instrument_key, trade_date, open, high, low, close, volume, source, adjustment
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"#,
                    params![
                        manifest.id,
                        item.instrument.storage_key(),
                        candle.time,
                        candle.open,
                        candle.high,
                        candle.low,
                        candle.close,
                        volume,
                        item.source,
                        adjustment_name(item.adjustment),
                    ],
                )?;
            }
        }
        transaction.commit()?;
        Ok(manifest)
    }

    fn load_dataset(&self, id: &str) -> Result<Option<FrozenDataset>> {
        let connection = self.connection();
        let Some(manifest) = load_manifest_from_connection(&connection, id)? else {
            return Ok(None);
        };
        let mut series = Vec::with_capacity(manifest.instruments.len());
        for instrument in &manifest.instruments {
            let mut statement = connection.prepare(
                r#"SELECT trade_date, open, high, low, close, volume, source, adjustment
                 FROM dataset_bars WHERE dataset_id=?1 AND instrument_key=?2 ORDER BY trade_date"#,
            )?;
            let rows = statement.query_map(params![id, instrument.storage_key()], |row| {
                Ok((
                    CandleRecord {
                        time: row.get(0)?,
                        open: row.get(1)?,
                        high: row.get(2)?,
                        low: row.get(3)?,
                        close: row.get(4)?,
                        volume: read_volume(row, 5)?,
                    },
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            })?;
            let collected: Vec<_> = rows.collect::<rusqlite::Result<_>>()?;
            let Some((_, source, adjustment)) = collected.first() else {
                bail!(
                    "frozen dataset {id} is missing bars for {}",
                    instrument.storage_key()
                );
            };
            series.push(FrozenSeries {
                instrument: instrument.clone(),
                source: source.clone(),
                adjustment: parse_adjustment(adjustment)?,
                candles: collected.into_iter().map(|row| row.0).collect(),
            });
        }
        Ok(Some(FrozenDataset { manifest, series }))
    }

    fn load_observation_dataset(&self, id: &str, as_of: &str) -> Result<Option<FrozenDataset>> {
        let Some(mut dataset) = self.load_dataset(id)? else {
            return Ok(None);
        };
        let frozen_end = dataset.manifest.interval.end.clone();
        let connection = self.connection();
        for series in &mut dataset.series {
            let mut bars: BTreeMap<String, CandleRecord> = series
                .candles
                .drain(..)
                .map(|bar| (bar.time.clone(), bar))
                .collect();
            let mut statement = connection.prepare(
                r#"SELECT trade_date, open, high, low, close, volume
                   FROM daily_bars
                   WHERE instrument_key=?1 AND adjustment=?2 AND trade_date>?3 AND trade_date<=?4
                   ORDER BY trade_date, updated_at"#,
            )?;
            let live = statement.query_map(
                params![
                    series.instrument.storage_key(),
                    adjustment_name(series.adjustment),
                    frozen_end,
                    as_of,
                ],
                |row| {
                    Ok(CandleRecord {
                        time: row.get(0)?,
                        open: row.get(1)?,
                        high: row.get(2)?,
                        low: row.get(3)?,
                        close: row.get(4)?,
                        volume: read_volume(row, 5)?,
                    })
                },
            )?;
            for bar in live {
                let bar = bar?;
                bars.insert(bar.time.clone(), bar);
            }
            series.candles = bars.into_values().collect();
            series.source = format!("{}+forward-observation", series.source);
        }
        let hash = dataset_content_sha256(dataset.manifest.market, &dataset.series);
        dataset.manifest.id = format!("observation-sha256:{hash}");
        dataset.manifest.content_sha256 = hash;
        dataset.manifest.interval.end = as_of.into();
        dataset
            .manifest
            .known_biases
            .push("前向观察合并了冻结样本与运行时增量行情".into());
        Ok(Some(dataset))
    }

    fn list_manifests(&self) -> Result<Vec<DatasetManifest>> {
        let connection = self.connection();
        let mut statement = connection
            .prepare("SELECT manifest_json FROM dataset_manifests ORDER BY created_at DESC")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }
}

impl ExperimentRepository for SqliteLabStore {
    fn save_strategy(&self, spec: &StrategySpec) -> Result<String> {
        let id = strategy_id(spec);
        let connection = self.connection();
        connection.execute(
            r#"INSERT OR IGNORE INTO strategies(
                strategy_id, schema_version, normalized_json, spec_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)"#,
            params![
                id,
                spec.schema_version,
                normalized_json(spec),
                serde_json::to_string(spec)?,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(id)
    }

    fn load_strategy(&self, strategy_id: &str) -> Result<Option<StrategySpec>> {
        let connection = self.connection();
        let json: Option<String> = connection
            .query_row(
                "SELECT spec_json FROM strategies WHERE strategy_id=?1",
                [strategy_id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    fn save_experiment(
        &self,
        experiment: &ExperimentRecord,
        candidates: &[ExperimentCandidate],
    ) -> Result<()> {
        if candidates
            .iter()
            .any(|candidate| candidate.experiment_id != experiment.definition.id)
        {
            bail!("candidate belongs to a different experiment");
        }
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        let now = chrono::Utc::now().to_rfc3339();
        transaction.execute(
            r#"INSERT INTO experiments(
                experiment_id, status, dataset_id, created_at, updated_at, record_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(experiment_id) DO UPDATE SET
                status=excluded.status, dataset_id=excluded.dataset_id,
                updated_at=excluded.updated_at, record_json=excluded.record_json"#,
            params![
                experiment.definition.id,
                experiment.status.as_str(),
                experiment.definition.dataset_id,
                experiment.created_at,
                now,
                serde_json::to_string(experiment)?,
            ],
        )?;
        transaction.execute(
            "DELETE FROM experiment_candidates WHERE experiment_id=?1",
            [&experiment.definition.id],
        )?;
        for candidate in candidates {
            let ordinal = i64::try_from(candidate.ordinal).context("candidate ordinal overflow")?;
            transaction.execute(
                r#"INSERT INTO experiment_candidates(
                    experiment_id, ordinal, strategy_id, normalized_hash, candidate_json
                 ) VALUES (?1, ?2, ?3, ?4, ?5)"#,
                params![
                    candidate.experiment_id,
                    ordinal,
                    candidate.strategy_id,
                    candidate.normalized_hash,
                    serde_json::to_string(candidate)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn load_experiment(&self, id: &str) -> Result<Option<ExperimentRecord>> {
        let connection = self.connection();
        let json: Option<String> = connection
            .query_row(
                "SELECT record_json FROM experiments WHERE experiment_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    fn load_candidates(&self, experiment_id: &str) -> Result<Vec<ExperimentCandidate>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT candidate_json FROM experiment_candidates \
             WHERE experiment_id=?1 ORDER BY ordinal",
        )?;
        statement
            .query_map([experiment_id], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }

    fn list_experiments(&self) -> Result<Vec<ExperimentRecord>> {
        let connection = self.connection();
        let mut statement =
            connection.prepare("SELECT record_json FROM experiments ORDER BY created_at DESC")?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }
}

impl BacktestRepository for SqliteLabStore {
    fn save_run(&self, run: &StoredBacktestRun) -> Result<()> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute(
            r#"INSERT INTO backtest_runs(
                run_id, experiment_id, strategy_id, status, config_json, report_json,
                failure_message, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(run_id) DO UPDATE SET
                status=excluded.status, config_json=excluded.config_json,
                report_json=excluded.report_json, failure_message=excluded.failure_message,
                updated_at=excluded.updated_at"#,
            params![
                run.run_id,
                run.experiment_id,
                run.strategy_id,
                run.status.as_str(),
                serde_json::to_string(&run.config)?,
                run.report.as_ref().map(serde_json::to_string).transpose()?,
                run.failure_message,
                run.created_at,
                run.updated_at,
            ],
        )?;
        transaction.execute("DELETE FROM backtest_trades WHERE run_id=?1", [&run.run_id])?;
        transaction.execute("DELETE FROM daily_equity WHERE run_id=?1", [&run.run_id])?;
        if let Some(report) = &run.report {
            for (ordinal, trade) in report.trades.iter().enumerate() {
                let ordinal = i64::try_from(ordinal).context("trade ordinal overflow")?;
                transaction.execute(
                    "INSERT INTO backtest_trades(run_id, ordinal, trade_json) VALUES (?1, ?2, ?3)",
                    params![run.run_id, ordinal, serde_json::to_string(trade)?],
                )?;
            }
            for point in &report.daily_equity {
                transaction.execute(
                    "INSERT INTO daily_equity(run_id, trade_date, equity_json) VALUES (?1, ?2, ?3)",
                    params![run.run_id, point.date, serde_json::to_string(point)?],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    fn load_run(&self, run_id: &str) -> Result<Option<StoredBacktestRun>> {
        let connection = self.connection();
        connection
            .query_row(
                "SELECT run_id, experiment_id, strategy_id, status, config_json, report_json, \
                        failure_message, created_at, updated_at \
                 FROM backtest_runs WHERE run_id=?1",
                [run_id],
                stored_run_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    fn list_runs(&self, experiment_id: &str) -> Result<Vec<StoredBacktestRun>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT run_id, experiment_id, strategy_id, status, config_json, report_json, \
                    failure_message, created_at, updated_at \
             FROM backtest_runs WHERE experiment_id=?1 ORDER BY created_at, run_id",
        )?;
        statement
            .query_map([experiment_id], stored_run_from_row)?
            .collect::<rusqlite::Result<_>>()
            .map_err(Into::into)
    }
}

impl PaperTradingRepository for SqliteLabStore {
    fn save_candidate(&self, candidate: &PaperCandidate) -> Result<()> {
        let connection = self.connection();
        connection.execute(
            r#"INSERT INTO paper_candidates(
                candidate_id, strategy_id, dataset_id, experiment_id, status, created_at, record_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(candidate_id) DO UPDATE SET
                status=excluded.status, record_json=excluded.record_json"#,
            params![
                candidate.id,
                candidate.strategy_id,
                candidate.dataset_id,
                candidate.experiment_id,
                format!("{:?}", candidate.status).to_ascii_lowercase(),
                candidate.created_at,
                serde_json::to_string(candidate)?,
            ],
        )?;
        Ok(())
    }

    fn list_candidates(&self) -> Result<Vec<PaperCandidate>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT record_json FROM paper_candidates ORDER BY created_at DESC, candidate_id",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }

    fn save_paper_run(&self, result: &PaperRunResult) -> Result<()> {
        let mut connection = self.connection();
        let transaction = connection.transaction()?;
        transaction.execute(
            r#"INSERT INTO paper_runs(candidate_id, as_of, generated_at, result_json)
               VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT(candidate_id, as_of) DO UPDATE SET
                 generated_at=excluded.generated_at, result_json=excluded.result_json"#,
            params![
                result.candidate_id,
                result.as_of,
                result.generated_at,
                serde_json::to_string(result)?,
            ],
        )?;
        for signal in &result.signals {
            transaction.execute(
                r#"INSERT INTO paper_signals(
                    signal_id, strategy_id, instrument_key, signal_date, signal_kind, record_json
                   ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                   ON CONFLICT(signal_id) DO UPDATE SET record_json=excluded.record_json"#,
                params![
                    signal.id,
                    signal.strategy_id,
                    signal.instrument.storage_key(),
                    signal.signal_date,
                    format!("{:?}", signal.kind).to_ascii_lowercase(),
                    serde_json::to_string(signal)?,
                ],
            )?;
        }
        for trade in &result.trades {
            transaction.execute(
                r#"INSERT INTO paper_trades(trade_id, candidate_id, exit_date, record_json)
                   VALUES (?1, ?2, ?3, ?4)
                   ON CONFLICT(trade_id) DO UPDATE SET record_json=excluded.record_json"#,
                params![
                    trade.id,
                    trade.candidate_id,
                    trade.exit_date,
                    serde_json::to_string(trade)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn load_latest_run(&self, candidate_id: &str) -> Result<Option<PaperRunResult>> {
        let connection = self.connection();
        let json: Option<String> = connection
            .query_row(
                "SELECT result_json FROM paper_runs WHERE candidate_id=?1 ORDER BY as_of DESC LIMIT 1",
                [candidate_id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }
}

impl ValidationRepository for SqliteLabStore {
    fn save_robustness_report(&self, experiment_id: &str, report: &RobustnessReport) -> Result<()> {
        let connection = self.connection();
        connection.execute(
            r#"INSERT INTO robustness_reports(
                experiment_id, strategy_id, validation_version, report_json, updated_at
               ) VALUES (?1, ?2, ?3, ?4, ?5)
               ON CONFLICT(experiment_id, strategy_id, validation_version) DO UPDATE SET
                 report_json=excluded.report_json, updated_at=excluded.updated_at"#,
            params![
                experiment_id,
                report.strategy_id,
                report.validation_version,
                serde_json::to_string(report)?,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn list_robustness_reports(&self, experiment_id: &str) -> Result<Vec<RobustnessReport>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT report_json FROM robustness_reports WHERE experiment_id=?1 ORDER BY strategy_id",
        )?;
        statement
            .query_map([experiment_id], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }

    fn save_sealed_test(&self, experiment_id: &str, result: &SealedTestResult) -> Result<()> {
        let connection = self.connection();
        connection.execute(
            r#"INSERT INTO sealed_test_reports(
                experiment_id, strategy_id, consumed_at, result_json
               ) VALUES (?1, ?2, ?3, ?4)
               ON CONFLICT(experiment_id, strategy_id) DO NOTHING"#,
            params![
                experiment_id,
                result.strategy_id,
                result.consumed_at,
                serde_json::to_string(result)?,
            ],
        )?;
        Ok(())
    }

    fn list_sealed_tests(&self, experiment_id: &str) -> Result<Vec<SealedTestResult>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT result_json FROM sealed_test_reports WHERE experiment_id=?1 ORDER BY strategy_id",
        )?;
        statement
            .query_map([experiment_id], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }
}

impl StrategyLibraryRepository for SqliteLabStore {
    fn save_library_record(&self, record: &StrategyLibraryRecord) -> Result<()> {
        let connection = self.connection();
        connection.execute(
            r#"INSERT INTO strategy_library(
                record_id, experiment_id, strategy_id, dataset_id, status, win_rate_pct, retained_at, record_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(experiment_id, strategy_id) DO UPDATE SET
                dataset_id=excluded.dataset_id,
                win_rate_pct=excluded.win_rate_pct,
                retained_at=excluded.retained_at,
                record_json=excluded.record_json
             WHERE strategy_library.status = 'retained'"#,
            params![
                record.id,
                record.experiment_id,
                record.strategy_id,
                record.dataset_id,
                library_status_name(record.status),
                record.win_rate_pct,
                record.retained_at,
                serde_json::to_string(record)?,
            ],
        )?;
        Ok(())
    }

    fn list_library_records(&self) -> Result<Vec<StrategyLibraryRecord>> {
        let connection = self.connection();
        let mut statement = connection.prepare(
            "SELECT record_json FROM strategy_library WHERE status='retained' \
             ORDER BY win_rate_pct DESC, retained_at DESC, record_id",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }

    fn dismiss_library_record(&self, record_id: &str) -> Result<bool> {
        let connection = self.connection();
        let json: Option<String> = connection
            .query_row(
                "SELECT record_json FROM strategy_library WHERE record_id=?1",
                [record_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(json) = json else {
            return Ok(false);
        };
        let mut record: StrategyLibraryRecord = serde_json::from_str(&json)?;
        record.status = LibraryStatus::Dismissed;
        let updated = connection.execute(
            "UPDATE strategy_library SET status='dismissed', record_json=?1 WHERE record_id=?2",
            params![serde_json::to_string(&record)?, record_id],
        )?;
        Ok(updated > 0)
    }

    fn library_initialized(&self) -> Result<bool> {
        let connection = self.connection();
        let count: i64 =
            connection.query_row("SELECT COUNT(*) FROM strategy_library", [], |row| {
                row.get(0)
            })?;
        Ok(count > 0)
    }
}

fn library_status_name(status: LibraryStatus) -> &'static str {
    match status {
        LibraryStatus::Retained => "retained",
        LibraryStatus::Dismissed => "dismissed",
    }
}

fn insert_instrument(transaction: &Transaction<'_>, instrument: &InstrumentId) -> Result<()> {
    transaction.execute(
        r#"INSERT OR IGNORE INTO instruments(instrument_key, market, asset_type, code)
         VALUES (?1, ?2, ?3, ?4)"#,
        params![
            instrument.storage_key(),
            market_name(instrument.market),
            asset_type_name(instrument.asset_type),
            instrument.code,
        ],
    )?;
    Ok(())
}

fn load_live_series(
    connection: &Connection,
    request: &FreezeDatasetRequest,
) -> Result<Vec<FrozenSeries>> {
    let mut output = Vec::with_capacity(request.instruments.len());
    let mut instruments = request.instruments.clone();
    instruments.sort();
    instruments.dedup();
    for instrument in instruments {
        if instrument.market != request.market {
            bail!("instrument market does not match dataset market");
        }
        let mut statement = connection.prepare(
            r#"SELECT trade_date, open, high, low, close, volume, source
             FROM daily_bars WHERE instrument_key=?1 AND adjustment=?2
               AND trade_date>=?3 AND trade_date<=?4
             ORDER BY source, trade_date"#,
        )?;
        let rows = statement.query_map(
            params![
                instrument.storage_key(),
                adjustment_name(request.adjustment),
                request.interval.start,
                request.interval.end,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(6)?,
                    CandleRecord {
                        time: row.get(0)?,
                        open: row.get(1)?,
                        high: row.get(2)?,
                        low: row.get(3)?,
                        close: row.get(4)?,
                        volume: read_volume(row, 5)?,
                    },
                ))
            },
        )?;
        let mut sources: BTreeMap<String, Vec<CandleRecord>> = BTreeMap::new();
        for row in rows {
            let (source, candle) = row?;
            sources.entry(source).or_default().push(candle);
        }
        let selected_source = request
            .source_versions
            .iter()
            .find(|source| sources.contains_key(source.as_str()))
            .cloned()
            .or_else(|| sources.keys().next().cloned())
            .with_context(|| format!("no cached bars for {}", instrument.storage_key()))?;
        output.push(FrozenSeries {
            instrument,
            source: selected_source.clone(),
            adjustment: request.adjustment,
            candles: sources.remove(&selected_source).unwrap_or_default(),
        });
    }
    Ok(output)
}

fn load_manifest_from_connection(
    connection: &Connection,
    id: &str,
) -> Result<Option<DatasetManifest>> {
    let json: Option<String> = connection
        .query_row(
            "SELECT manifest_json FROM dataset_manifests WHERE dataset_id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    json.map(|json| serde_json::from_str(&json).map_err(Into::into))
        .transpose()
}

const fn market_name(market: Market) -> &'static str {
    match market {
        Market::AShare => "a_share",
        Market::HongKong => "hong_kong",
    }
}

const fn asset_type_name(asset_type: AssetType) -> &'static str {
    match asset_type {
        AssetType::Stock => "stock",
        AssetType::Index => "index",
    }
}

const fn adjustment_name(adjustment: Adjustment) -> &'static str {
    match adjustment {
        Adjustment::None => "none",
        Adjustment::Forward => "forward",
        Adjustment::Backward => "backward",
    }
}

fn parse_adjustment(value: &str) -> Result<Adjustment> {
    match value {
        "none" => Ok(Adjustment::None),
        "forward" => Ok(Adjustment::Forward),
        "backward" => Ok(Adjustment::Backward),
        _ => bail!("unknown adjustment {value}"),
    }
}

fn stored_run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredBacktestRun> {
    let status: String = row.get(3)?;
    let config_json: String = row.get(4)?;
    let report_json: Option<String> = row.get(5)?;
    Ok(StoredBacktestRun {
        run_id: row.get(0)?,
        experiment_id: row.get(1)?,
        strategy_id: row.get(2)?,
        status: parse_run_status(&status)?,
        config: serde_json::from_str(&config_json).map_err(to_from_sql_error)?,
        report: report_json
            .map(|json| serde_json::from_str(&json).map_err(to_from_sql_error))
            .transpose()?,
        failure_message: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn parse_run_status(value: &str) -> rusqlite::Result<StoredRunStatus> {
    match value {
        "pending" => Ok(StoredRunStatus::Pending),
        "running" => Ok(StoredRunStatus::Running),
        "completed" => Ok(StoredRunStatus::Completed),
        "cancelled" => Ok(StoredRunStatus::Cancelled),
        "failed" => Ok(StoredRunStatus::Failed),
        _ => Err(to_from_sql_error(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown backtest run status {value}"),
        ))),
    }
}

fn to_from_sql_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn read_volume(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::backtest::config::PortfolioBacktestConfig;
    use crate::domain::backtest::portfolio::run_portfolio_backtest;
    use crate::domain::dataset::DateInterval;
    use crate::domain::experiment::{
        CandidateSource, ExperimentDefinition, ExperimentStatus, GenerationAudit, RiskLimits,
    };
    use crate::domain::paper::{PaperCandidate, PaperCandidateStatus, run_paper_history};
    use crate::domain::strategy::{CompiledStrategy, LocalTemplate};

    fn instrument() -> InstrumentId {
        InstrumentId {
            market: Market::AShare,
            asset_type: AssetType::Stock,
            code: "600000".into(),
        }
    }

    fn series() -> FrozenSeries {
        FrozenSeries {
            instrument: instrument(),
            source: "fixture-v1".into(),
            adjustment: Adjustment::Forward,
            candles: (0..5)
                .map(|index| {
                    let close = 10.0 + index as f64;
                    CandleRecord {
                        time: format!("2026-01-{:02}", index + 1),
                        open: close,
                        high: close + 1.0,
                        low: close - 1.0,
                        close,
                        volume: 1_000 + index,
                    }
                })
                .collect(),
        }
    }

    fn freeze_request() -> FreezeDatasetRequest {
        FreezeDatasetRequest {
            market: Market::AShare,
            adjustment: Adjustment::Forward,
            source_versions: vec!["fixture-v1".into()],
            instruments: vec![instrument()],
            interval: DateInterval {
                start: "2026-01-01".into(),
                end: "2026-01-05".into(),
            },
            known_biases: vec!["survivorship bias unresolved".into()],
        }
    }

    fn seed_dataset(store: &SqliteLabStore) -> DatasetManifest {
        store.upsert_series(&series()).unwrap();
        store.freeze_dataset(&freeze_request()).unwrap()
    }

    fn experiment(dataset_id: &str, strategy_id: &str) -> ExperimentRecord {
        ExperimentRecord {
            definition: ExperimentDefinition {
                id: "experiment-fixture".into(),
                user_goal: "low drawdown".into(),
                risk_limits: RiskLimits {
                    max_drawdown_pct: 15.0,
                    max_turnover_pct: Some(200.0),
                    max_positions: 5,
                },
                generation: GenerationAudit {
                    model: "local".into(),
                    transport: "local-template".into(),
                    prompt_version: "v1".into(),
                    raw_candidate_count: 1,
                    validation_failure_count: 0,
                    raw_response_sha256: None,
                },
                strategy_ids: vec![strategy_id.into()],
                dataset_id: dataset_id.into(),
                universe_snapshot_id: dataset_id.into(),
                benchmark_version: "fixture-benchmark-v1".into(),
                cost_model_version: "fixture-costs-v1".into(),
                validation_config_version: "fixture-validation-v1".into(),
                parameter_attempts: 1,
                ranking_rule_version: "fixture-ranking-v1".into(),
            },
            status: ExperimentStatus::Completed,
            created_at: "2026-01-06T00:00:00Z".into(),
            started_at: Some("2026-01-06T00:00:01Z".into()),
            completed_at: Some("2026-01-06T00:00:02Z".into()),
            cancelled_at: None,
            failed_at: None,
            failure_message: None,
            test_consumed_at: None,
        }
    }

    #[test]
    fn repeated_ingest_is_idempotent() {
        let store = SqliteLabStore::open_in_memory().unwrap();
        let first = store.upsert_series(&series()).unwrap();
        let second = store.upsert_series(&series()).unwrap();
        assert_eq!(first.inserted, 5);
        assert_eq!(second.unchanged, 5);
        assert_eq!((second.inserted, second.updated), (0, 0));
    }

    #[test]
    fn changed_data_creates_new_dataset_without_mutating_old_snapshot() {
        let store = SqliteLabStore::open_in_memory().unwrap();
        let original = seed_dataset(&store);
        let mut changed = series();
        changed.candles[2].close = 12.5;
        changed.candles[2].high = 13.5;
        assert_eq!(store.upsert_series(&changed).unwrap().updated, 1);
        let replacement = store.freeze_dataset(&freeze_request()).unwrap();

        assert_ne!(original.id, replacement.id);
        let reopened_original = store.load_dataset(&original.id).unwrap().unwrap();
        let reopened_replacement = store.load_dataset(&replacement.id).unwrap().unwrap();
        assert_eq!(reopened_original.series[0].candles[2].close, 12.0);
        assert_eq!(reopened_replacement.series[0].candles[2].close, 12.5);
    }

    #[test]
    fn paper_observation_appends_new_bars_without_revising_frozen_history() {
        let store = SqliteLabStore::open_in_memory().unwrap();
        let frozen = seed_dataset(&store);
        let mut live = series();
        live.candles[2].close = 99.0;
        live.candles[2].high = 100.0;
        live.candles.push(CandleRecord {
            time: "2026-01-06".into(),
            open: 15.0,
            high: 16.0,
            low: 14.0,
            close: 15.5,
            volume: 2_000,
        });
        store.upsert_series(&live).unwrap();

        let observed = store
            .load_observation_dataset(&frozen.id, "2026-01-06")
            .unwrap()
            .unwrap();
        assert_eq!(observed.series[0].candles.len(), 6);
        assert_eq!(observed.series[0].candles[2].close, 12.0);
        assert_eq!(observed.series[0].candles[5].close, 15.5);
        assert!(observed.manifest.id.starts_with("observation-sha256:"));
    }

    #[test]
    fn experiment_transaction_rolls_back_and_completed_experiment_reopens_offline() {
        let path = unique_path("offline-reopen.sqlite3");
        let (expected, candidate) = {
            let store = SqliteLabStore::open(&path).unwrap();
            let manifest = seed_dataset(&store);
            let spec = LocalTemplate::NDayHighBreakout.build(&manifest.id);
            let strategy_id = store.save_strategy(&spec).unwrap();
            let expected = experiment(&manifest.id, &strategy_id);
            let candidate = ExperimentCandidate {
                experiment_id: expected.definition.id.clone(),
                ordinal: 0,
                strategy_id: Some(strategy_id.clone()),
                parent_strategy_id: None,
                source: CandidateSource::LocalTemplate,
                normalized_hash: Some(strategy_id),
                validation_errors: vec![],
            };
            store
                .save_experiment(&expected, std::slice::from_ref(&candidate))
                .unwrap();

            let mut invalid = expected.clone();
            invalid.definition.id = "must-rollback".into();
            let invalid_candidate = ExperimentCandidate {
                experiment_id: invalid.definition.id.clone(),
                ordinal: 0,
                strategy_id: Some("missing-strategy".into()),
                parent_strategy_id: None,
                source: CandidateSource::LocalTemplate,
                normalized_hash: None,
                validation_errors: vec![],
            };
            assert!(
                store
                    .save_experiment(&invalid, &[invalid_candidate])
                    .is_err()
            );
            assert!(store.load_experiment("must-rollback").unwrap().is_none());
            (expected, candidate)
        };

        let reopened = SqliteLabStore::open(&path).unwrap();
        assert_eq!(
            reopened.load_experiment(&expected.definition.id).unwrap(),
            Some(expected)
        );
        assert_eq!(
            reopened.load_candidates(&candidate.experiment_id).unwrap(),
            vec![candidate]
        );
        drop(reopened);
        std::fs::remove_file(&path).ok();
        std::fs::remove_file(path.with_extension("sqlite3-wal")).ok();
        std::fs::remove_file(path.with_extension("sqlite3-shm")).ok();
    }

    #[test]
    fn backup_and_restore_preserve_frozen_datasets() {
        let backup = unique_path("backup.sqlite3");
        let source = SqliteLabStore::open_in_memory().unwrap();
        let manifest = seed_dataset(&source);
        source.backup_to(&backup).unwrap();
        let restored = SqliteLabStore::open_in_memory().unwrap();
        restored.restore_from(&backup).unwrap();
        assert_eq!(
            restored
                .load_dataset(&manifest.id)
                .unwrap()
                .unwrap()
                .manifest,
            manifest
        );
        std::fs::remove_file(backup).ok();
    }

    #[test]
    fn completed_backtest_report_reopens_with_daily_equity_and_trades() {
        let store = SqliteLabStore::open_in_memory().unwrap();
        let manifest = seed_dataset(&store);
        let spec = LocalTemplate::NDayHighBreakout.build(&manifest.id);
        let strategy_id = store.save_strategy(&spec).unwrap();
        let experiment = experiment(&manifest.id, &strategy_id);
        let candidate = ExperimentCandidate {
            experiment_id: experiment.definition.id.clone(),
            ordinal: 0,
            strategy_id: Some(strategy_id.clone()),
            parent_strategy_id: None,
            source: CandidateSource::LocalTemplate,
            normalized_hash: Some(strategy_id.clone()),
            validation_errors: vec![],
        };
        store.save_experiment(&experiment, &[candidate]).unwrap();
        let dataset = store.load_dataset(&manifest.id).unwrap().unwrap();
        let compiled = CompiledStrategy::compile(spec).unwrap();
        let config = PortfolioBacktestConfig::default();
        let report = run_portfolio_backtest(&dataset, &compiled, &config).unwrap();
        let run = StoredBacktestRun {
            run_id: "run-fixture".into(),
            experiment_id: experiment.definition.id.clone(),
            strategy_id,
            status: StoredRunStatus::Completed,
            config,
            report: Some(report),
            failure_message: None,
            created_at: "2026-01-07T00:00:00Z".into(),
            updated_at: "2026-01-07T00:00:01Z".into(),
        };

        store.save_run(&run).unwrap();

        assert_eq!(store.load_run(&run.run_id).unwrap(), Some(run.clone()));
        assert_eq!(store.list_runs(&run.experiment_id).unwrap(), vec![run]);
    }

    #[test]
    fn repeated_paper_run_persists_idempotently() {
        let store = SqliteLabStore::open_in_memory().unwrap();
        let manifest = seed_dataset(&store);
        let spec = LocalTemplate::NDayHighBreakout.build(&manifest.id);
        let strategy_id = store.save_strategy(&spec).unwrap();
        let experiment = experiment(&manifest.id, &strategy_id);
        let experiment_candidate = ExperimentCandidate {
            experiment_id: experiment.definition.id.clone(),
            ordinal: 0,
            strategy_id: Some(strategy_id.clone()),
            parent_strategy_id: None,
            source: CandidateSource::LocalTemplate,
            normalized_hash: Some(strategy_id.clone()),
            validation_errors: vec![],
        };
        store
            .save_experiment(&experiment, &[experiment_candidate])
            .unwrap();
        let candidate = PaperCandidate {
            id: format!("paper:{strategy_id}"),
            strategy_id,
            dataset_id: manifest.id.clone(),
            experiment_id: experiment.definition.id,
            created_at: "2026-01-07T00:00:00Z".into(),
            status: PaperCandidateStatus::Observing,
        };
        store.save_candidate(&candidate).unwrap();
        store.save_candidate(&candidate).unwrap();
        let dataset = store.load_dataset(&manifest.id).unwrap().unwrap();
        let compiled = CompiledStrategy::compile(spec).unwrap();
        let result = run_paper_history(
            &candidate,
            &compiled,
            &dataset,
            "2026-01-05",
            "2026-01-06T00:00:00Z",
        );
        store.save_paper_run(&result).unwrap();
        store.save_paper_run(&result).unwrap();

        assert_eq!(store.list_candidates().unwrap(), vec![candidate.clone()]);
        assert_eq!(store.load_latest_run(&candidate.id).unwrap(), Some(result));
    }

    #[test]
    fn strategy_library_keeps_metrics_and_honors_dismiss() {
        use crate::domain::strategy_library::LibraryStatus;
        use crate::services::strategy_library::StrategyLibraryRepository;

        let store = SqliteLabStore::open_in_memory().unwrap();
        let manifest = seed_dataset(&store);
        let spec = LocalTemplate::NDayHighBreakout.build(&manifest.id);
        let strategy_id = store.save_strategy(&spec).unwrap();
        let experiment = experiment(&manifest.id, &strategy_id);
        let candidate = ExperimentCandidate {
            experiment_id: experiment.definition.id.clone(),
            ordinal: 0,
            strategy_id: Some(strategy_id.clone()),
            parent_strategy_id: None,
            source: CandidateSource::LocalTemplate,
            normalized_hash: Some(strategy_id.clone()),
            validation_errors: vec![],
        };
        store.save_experiment(&experiment, &[candidate]).unwrap();
        let record = StrategyLibraryRecord {
            id: StrategyLibraryRecord::id_for(&experiment.definition.id, &strategy_id),
            experiment_id: experiment.definition.id.clone(),
            strategy_id: strategy_id.clone(),
            dataset_id: manifest.id.clone(),
            strategy_name: spec.name,
            retained_at: "2026-08-14T00:00:00Z".into(),
            status: LibraryStatus::Retained,
            conclusion: None,
            evidence: "样本内探索".into(),
            win_rate_pct: 62.5,
            oos_win_rate_pct: None,
            total_return_pct: 8.0,
            excess_return_pct: 3.0,
            max_drawdown_pct: 12.0,
            trade_count: 16,
            payoff_ratio: 1.4,
            profit_factor: 1.2,
        };
        store.save_library_record(&record).unwrap();
        store.save_library_record(&record).unwrap();
        assert_eq!(store.list_library_records().unwrap(), vec![record.clone()]);
        assert!(store.dismiss_library_record(&record.id).unwrap());
        assert!(store.list_library_records().unwrap().is_empty());
        store.save_library_record(&record).unwrap();
        assert!(store.list_library_records().unwrap().is_empty());
        assert!(store.library_initialized().unwrap());
    }

    fn unique_path(suffix: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("zstock-{}-{nonce}-{suffix}", std::process::id()))
    }
}
