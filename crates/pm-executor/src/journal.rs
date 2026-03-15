use crossbeam_channel::{unbounded, Receiver, Sender};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};

#[derive(Debug, Clone, PartialEq)]
pub struct Btc5mJournalRecord {
    pub recorded_at_ms: u64,
    pub market_slug: String,
    pub market_id: String,
    pub market_open_ts: u64,
    pub market_close_ts: u64,
    pub up_token_id: String,
    pub down_token_id: String,
    pub dry_run: bool,
    pub action_kind: String,
    pub action_status: String,
    pub purpose: Option<String>,
    pub mode_candidate: Option<String>,
    pub outcome: Option<String>,
    pub side: Option<String>,
    pub token_id: Option<String>,
    pub size: Option<String>,
    pub price: Option<String>,
    pub cost_usd: Option<f64>,
    pub pair_sum: Option<f64>,
    pub expected_profit_usd: Option<f64>,
    pub cleanup_reason: Option<String>,
    pub cleanup_loss_usd: Option<f64>,
    pub session_cleanup_loss_usd: Option<f64>,
    pub binance_open_price: Option<f64>,
    pub binance_latest_price: Option<f64>,
    pub binance_signed_move_bps: Option<f64>,
    pub binance_recent_move_bps: Option<f64>,
    pub order_id: Option<String>,
    pub order_status: Option<String>,
    pub transaction_hashes_json: String,
    pub trade_ids_json: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Btc5mJournal {
    tx: Sender<Btc5mJournalRecord>,
}

pub struct Btc5mJournalWorker {
    join_handle: Option<JoinHandle<Result<(), String>>>,
}

impl Btc5mJournal {
    pub fn start(path: impl AsRef<Path>) -> Result<(Self, Btc5mJournalWorker), String> {
        let (tx, rx) = unbounded();
        let path = path.as_ref().to_path_buf();
        let join_handle = thread::Builder::new()
            .name("btc5m-journal".to_string())
            .spawn(move || run_worker(path, rx))
            .map_err(|err| format!("failed to spawn btc5m journal thread: {err}"))?;

        Ok((
            Self { tx },
            Btc5mJournalWorker {
                join_handle: Some(join_handle),
            },
        ))
    }

    pub fn record(&self, record: Btc5mJournalRecord) {
        if let Err(error) = self.tx.send(record) {
            tracing::error!(error = %error, "BTC 5m journal writer is unavailable");
        }
    }
}

impl Btc5mJournalWorker {
    pub fn join(mut self) -> Result<(), String> {
        let handle = self
            .join_handle
            .take()
            .ok_or_else(|| "btc5m journal worker already joined".to_string())?;
        handle
            .join()
            .map_err(|_| "btc5m journal worker panicked".to_string())?
    }
}

fn run_worker(path: PathBuf, rx: Receiver<Btc5mJournalRecord>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create journal dir {}: {err}", parent.display()))?;
        }
    }

    let conn = Connection::open(&path)
        .map_err(|err| format!("failed to open journal db {}: {err}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|err| format!("failed to enable WAL for {}: {err}", path.display()))?;
    init_schema(&conn)?;

    for record in rx {
        insert_record(&conn, &record)?;
    }

    Ok(())
}

fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
CREATE TABLE IF NOT EXISTS btc5m_order_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at_ms INTEGER NOT NULL,
    market_slug TEXT NOT NULL,
    market_id TEXT NOT NULL,
    market_open_ts INTEGER NOT NULL,
    market_close_ts INTEGER NOT NULL,
    up_token_id TEXT NOT NULL,
    down_token_id TEXT NOT NULL,
    dry_run INTEGER NOT NULL,
    action_kind TEXT NOT NULL,
    action_status TEXT NOT NULL,
    purpose TEXT,
    mode_candidate TEXT,
    outcome TEXT,
    side TEXT,
    token_id TEXT,
    size TEXT,
    price TEXT,
    cost_usd REAL,
    pair_sum REAL,
    expected_profit_usd REAL,
    cleanup_reason TEXT,
    cleanup_loss_usd REAL,
    session_cleanup_loss_usd REAL,
    binance_open_price REAL,
    binance_latest_price REAL,
    binance_signed_move_bps REAL,
    binance_recent_move_bps REAL,
    order_id TEXT,
    order_status TEXT,
    transaction_hashes_json TEXT NOT NULL,
    trade_ids_json TEXT NOT NULL,
    error_message TEXT
);
CREATE INDEX IF NOT EXISTS idx_btc5m_order_events_market
    ON btc5m_order_events (market_open_ts, market_slug, recorded_at_ms);
"#,
    )
    .map_err(|err| format!("failed to initialize btc5m journal schema: {err}"))
}

fn insert_record(conn: &Connection, record: &Btc5mJournalRecord) -> Result<(), String> {
    conn.execute(
        r#"
INSERT INTO btc5m_order_events (
    recorded_at_ms,
    market_slug,
    market_id,
    market_open_ts,
    market_close_ts,
    up_token_id,
    down_token_id,
    dry_run,
    action_kind,
    action_status,
    purpose,
    mode_candidate,
    outcome,
    side,
    token_id,
    size,
    price,
    cost_usd,
    pair_sum,
    expected_profit_usd,
    cleanup_reason,
    cleanup_loss_usd,
    session_cleanup_loss_usd,
    binance_open_price,
    binance_latest_price,
    binance_signed_move_bps,
    binance_recent_move_bps,
    order_id,
    order_status,
    transaction_hashes_json,
    trade_ids_json,
    error_message
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32)
"#,
        params![
            u64_to_i64(record.recorded_at_ms)?,
            record.market_slug,
            record.market_id,
            u64_to_i64(record.market_open_ts)?,
            u64_to_i64(record.market_close_ts)?,
            record.up_token_id,
            record.down_token_id,
            if record.dry_run { 1_i64 } else { 0_i64 },
            record.action_kind,
            record.action_status,
            record.purpose,
            record.mode_candidate,
            record.outcome,
            record.side,
            record.token_id,
            record.size,
            record.price,
            record.cost_usd,
            record.pair_sum,
            record.expected_profit_usd,
            record.cleanup_reason,
            record.cleanup_loss_usd,
            record.session_cleanup_loss_usd,
            record.binance_open_price,
            record.binance_latest_price,
            record.binance_signed_move_bps,
            record.binance_recent_move_bps,
            record.order_id,
            record.order_status,
            record.transaction_hashes_json,
            record.trade_ids_json,
            record.error_message,
        ],
    )
    .map_err(|err| format!("failed to insert btc5m journal row: {err}"))?;

    Ok(())
}

fn u64_to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| format!("value {value} exceeds SQLite integer range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rtt_btc5m_journal_{}_{}_test.sqlite3",
            name,
            std::process::id()
        ))
    }

    fn sample_record() -> Btc5mJournalRecord {
        Btc5mJournalRecord {
            recorded_at_ms: 1_773_499_205_123,
            market_slug: "btc-updown-5m-1773499200".to_string(),
            market_id: "market-123".to_string(),
            market_open_ts: 1_773_499_200,
            market_close_ts: 1_773_499_500,
            up_token_id: "up-token".to_string(),
            down_token_id: "down-token".to_string(),
            dry_run: false,
            action_kind: "first_leg".to_string(),
            action_status: "matched".to_string(),
            purpose: Some("probe".to_string()),
            mode_candidate: Some("paired_no_sells".to_string()),
            outcome: Some("up".to_string()),
            side: Some("buy".to_string()),
            token_id: Some("up-token".to_string()),
            size: Some("5".to_string()),
            price: Some("0.48".to_string()),
            cost_usd: Some(2.4),
            pair_sum: Some(0.95),
            expected_profit_usd: Some(0.25),
            cleanup_reason: None,
            cleanup_loss_usd: None,
            session_cleanup_loss_usd: None,
            binance_open_price: Some(82_500.0),
            binance_latest_price: Some(82_510.0),
            binance_signed_move_bps: Some(1.21),
            binance_recent_move_bps: Some(0.33),
            order_id: Some("order-1".to_string()),
            order_status: Some("matched".to_string()),
            transaction_hashes_json: "[\"0xabc\"]".to_string(),
            trade_ids_json: "[\"trade-1\"]".to_string(),
            error_message: None,
        }
    }

    #[test]
    fn journal_persists_rows_for_later_market_reconstruction() {
        let path = temp_db_path("persist_rows");
        let _ = std::fs::remove_file(&path);

        let (journal, worker) = Btc5mJournal::start(&path).unwrap();
        journal.record(sample_record());
        drop(journal);
        worker.join().unwrap();

        let conn = Connection::open(&path).unwrap();
        let row = conn
            .query_row(
                r#"
SELECT market_slug, action_kind, action_status, order_id, transaction_hashes_json, trade_ids_json
FROM btc5m_order_events
LIMIT 1
"#,
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap();

        assert_eq!(row.0, "btc-updown-5m-1773499200");
        assert_eq!(row.1, "first_leg");
        assert_eq!(row.2, "matched");
        assert_eq!(row.3.as_deref(), Some("order-1"));
        assert_eq!(row.4, "[\"0xabc\"]");
        assert_eq!(row.5, "[\"trade-1\"]");

        let _ = std::fs::remove_file(&path);
    }
}
