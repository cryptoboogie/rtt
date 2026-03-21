use std::path::Path;

use rusqlite::{params, Connection};

#[derive(Debug)]
pub struct AnalysisStore {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisOperation {
    pub timestamp_ms: u64,
    pub operation_type: String,
    pub condition_id: Option<String>,
    pub asset_id: Option<String>,
    pub quote_id: Option<String>,
    pub client_order_id: Option<String>,
    pub exchange_order_id: Option<String>,
    pub side: Option<String>,
    pub requested_price: Option<String>,
    pub requested_size: Option<String>,
    pub result_status: String,
    pub error_text: Option<String>,
    pub capital_before_usd: Option<f64>,
    pub capital_after_usd: Option<f64>,
    pub reward_share: Option<f64>,
    pub payload_json: Option<String>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl AnalysisStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS operations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp_ms INTEGER NOT NULL,
                operation_type TEXT NOT NULL,
                condition_id TEXT,
                asset_id TEXT,
                quote_id TEXT,
                client_order_id TEXT,
                exchange_order_id TEXT,
                side TEXT,
                requested_price TEXT,
                requested_size TEXT,
                result_status TEXT NOT NULL,
                error_text TEXT,
                capital_before_usd REAL,
                capital_after_usd REAL,
                reward_share REAL,
                payload_json TEXT
            );
            "#,
        )?;

        Ok(Self { conn })
    }

    pub fn append(&self, operation: &AnalysisOperation) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            r#"
            INSERT INTO operations (
                timestamp_ms,
                operation_type,
                condition_id,
                asset_id,
                quote_id,
                client_order_id,
                exchange_order_id,
                side,
                requested_price,
                requested_size,
                result_status,
                error_text,
                capital_before_usd,
                capital_after_usd,
                reward_share,
                payload_json
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            params![
                operation.timestamp_ms as i64,
                operation.operation_type,
                operation.condition_id,
                operation.asset_id,
                operation.quote_id,
                operation.client_order_id,
                operation.exchange_order_id,
                operation.side,
                operation.requested_price,
                operation.requested_size,
                operation.result_status,
                operation.error_text,
                operation.capital_before_usd,
                operation.capital_after_usd,
                operation.reward_share,
                operation.payload_json,
            ],
        )?;

        Ok(())
    }

    pub fn operation_count(&self) -> Result<u64, rusqlite::Error> {
        self.conn
            .query_row("SELECT COUNT(*) FROM operations", [], |row| {
                row.get::<_, u64>(0)
            })
    }

    pub fn load_operations(&self) -> Result<Vec<AnalysisOperation>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                timestamp_ms,
                operation_type,
                condition_id,
                asset_id,
                quote_id,
                client_order_id,
                exchange_order_id,
                side,
                requested_price,
                requested_size,
                result_status,
                error_text,
                capital_before_usd,
                capital_after_usd,
                reward_share,
                payload_json
            FROM operations
            ORDER BY id ASC
            "#,
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(AnalysisOperation {
                timestamp_ms: row.get::<_, i64>(0)? as u64,
                operation_type: row.get(1)?,
                condition_id: row.get(2)?,
                asset_id: row.get(3)?,
                quote_id: row.get(4)?,
                client_order_id: row.get(5)?,
                exchange_order_id: row.get(6)?,
                side: row.get(7)?,
                requested_price: row.get(8)?,
                requested_size: row.get(9)?,
                result_status: row.get(10)?,
                error_text: row.get(11)?,
                capital_before_usd: row.get(12)?,
                capital_after_usd: row.get(13)?,
                reward_share: row.get(14)?,
                payload_json: row.get(15)?,
            })
        })?;

        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_sqlite_path() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("pm-executor-analysis-{unique}.sqlite"))
    }

    #[test]
    fn analysis_store_appends_material_operations() {
        let path = temp_sqlite_path();
        let store = AnalysisStore::open(&path).expect("sqlite store");

        store
            .append(&AnalysisOperation {
                timestamp_ms: 1_700_000_000_000,
                operation_type: "quote_set_emitted".to_string(),
                condition_id: Some("condition-1".to_string()),
                asset_id: Some("asset-yes".to_string()),
                quote_id: Some("condition-1:yes:entry".to_string()),
                client_order_id: Some("client-1".to_string()),
                exchange_order_id: Some("exchange-1".to_string()),
                side: Some("Buy".to_string()),
                requested_price: Some("0.45".to_string()),
                requested_size: Some("50".to_string()),
                result_status: "accepted".to_string(),
                error_text: None,
                capital_before_usd: Some(10.0),
                capital_after_usd: Some(32.5),
                reward_share: Some(12.5),
                payload_json: Some("{\"lanes\":2}".to_string()),
            })
            .expect("append");

        assert_eq!(store.operation_count().unwrap(), 1);

        let operations = store.load_operations().unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].operation_type, "quote_set_emitted");
        assert_eq!(
            operations[0].quote_id.as_deref(),
            Some("condition-1:yes:entry")
        );
        assert_eq!(operations[0].capital_after_usd, Some(32.5));

        let _ = std::fs::remove_file(path);
    }
}
