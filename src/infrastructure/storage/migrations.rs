use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::domain::money::Currency;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const PORTFOLIO_SCHEMA_VERSION: u32 = 1;
pub const JOURNAL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Config,
    Portfolio,
    Journal,
}

impl DocumentKind {
    pub const fn current_version(self) -> u32 {
        match self {
            Self::Config => CONFIG_SCHEMA_VERSION,
            Self::Portfolio => PORTFOLIO_SCHEMA_VERSION,
            Self::Journal => JOURNAL_SCHEMA_VERSION,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MigrationResult {
    pub value: Value,
    pub from_version: u32,
    pub migrated: bool,
    /// Config v0 only. The caller must persist this in SecretStore before replacing the file.
    pub legacy_api_key: Option<String>,
}

pub fn migrate(kind: DocumentKind, value: Value) -> Result<MigrationResult> {
    let mut object = value
        .as_object()
        .cloned()
        .context("document root must be a JSON object")?;
    let current = kind.current_version();
    let from_version = schema_version(&object)?;
    if from_version > current {
        bail!("future schema version {from_version}; supported version is {current}");
    }
    let mut version = from_version;
    let mut legacy_api_key = None;
    while version < current {
        match (kind, version) {
            (DocumentKind::Config, 0) => {
                legacy_api_key = migrate_config_v0(&mut object);
            }
            (DocumentKind::Portfolio, 0) => migrate_portfolio_v0(&mut object)?,
            (DocumentKind::Journal, 0) => {}
            _ => bail!("no migration for {kind:?} schema {version}"),
        }
        version += 1;
        object.insert("schema_version".into(), Value::from(version));
    }
    Ok(MigrationResult {
        value: Value::Object(object),
        from_version,
        migrated: from_version != current,
        legacy_api_key,
    })
}

fn schema_version(object: &Map<String, Value>) -> Result<u32> {
    match object.get("schema_version") {
        None => Ok(0),
        Some(value) => value
            .as_u64()
            .and_then(|version| u32::try_from(version).ok())
            .context("schema_version must be a non-negative 32-bit integer"),
    }
}

fn migrate_config_v0(object: &mut Map<String, Value>) -> Option<String> {
    object
        .get_mut("ai_api")
        .and_then(Value::as_object_mut)
        .and_then(|ai| ai.remove("api_key"))
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|value| !value.trim().is_empty())
}

fn migrate_portfolio_v0(object: &mut Map<String, Value>) -> Result<()> {
    let mut pending = Vec::new();
    if let Some(trades) = object.get_mut("trades").and_then(Value::as_array_mut) {
        for trade in trades {
            let trade = trade.as_object_mut().context("trade must be an object")?;
            if trade.contains_key("currency") {
                continue;
            }
            let code = trade
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match Currency::for_code(code) {
                Some(currency) => {
                    trade.insert("currency".into(), serde_json::to_value(currency)?);
                }
                None => pending.push(code.to_string()),
            }
        }
    }
    let legacy_cash = object.remove("cash").and_then(|value| value.as_f64());
    if !object.contains_key("cash_balances") {
        let mut balances = Map::new();
        if let Some(cash) = legacy_cash {
            balances.insert(
                "CNY".into(),
                serde_json::json!({ "currency": "CNY", "minor": (cash * 100.0).round() as i64 }),
            );
        }
        object.insert("cash_balances".into(), Value::Object(balances));
    }
    object.insert(
        "pending_currency_codes".into(),
        serde_json::to_value(pending)?,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent_and_strip_plaintext_secret() {
        let first = migrate(
            DocumentKind::Config,
            serde_json::json!({"watchlist": [], "ai_api": {"api_key": "sk-test"}}),
        )
        .unwrap();
        assert_eq!(first.legacy_api_key.as_deref(), Some("sk-test"));
        assert!(first.value.pointer("/ai_api/api_key").is_none());
        let second = migrate(DocumentKind::Config, first.value.clone()).unwrap();
        assert_eq!(second.value, first.value);
        assert!(!second.migrated);
    }

    #[test]
    fn portfolio_migration_infers_a_h_currency_and_preserves_trade_fields() {
        let original = serde_json::json!({
            "trades": [
                {"id":"a", "code":"600519", "shares":100.0, "price":10.0},
                {"id":"h", "code":"00700", "shares":200.0, "price":20.0}
            ],
            "cash": 12.34
        });
        let migrated = migrate(DocumentKind::Portfolio, original).unwrap().value;
        assert_eq!(
            migrated.pointer("/trades/0/currency"),
            Some(&Value::from("CNY"))
        );
        assert_eq!(
            migrated.pointer("/trades/1/currency"),
            Some(&Value::from("HKD"))
        );
        assert_eq!(
            migrated.pointer("/trades/1/shares").and_then(Value::as_f64),
            Some(200.0)
        );
        assert_eq!(
            migrated
                .pointer("/cash_balances/CNY/minor")
                .and_then(Value::as_i64),
            Some(1234)
        );
    }

    #[test]
    fn rejects_future_schema() {
        let error = migrate(
            DocumentKind::Journal,
            serde_json::json!({"schema_version": 99}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("future schema"));
    }
}
