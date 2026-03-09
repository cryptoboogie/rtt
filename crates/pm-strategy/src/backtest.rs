use crate::strategy::Strategy;
use crate::types::{OrderBookSnapshot, TriggerMessage};
use rtt_core::{HotStateStore, MarketMeta, NormalizedUpdate};
use std::path::Path;

/// Result of a backtest run.
pub struct BacktestResult {
    pub triggers: Vec<TriggerMessage>,
    pub total_snapshots: usize,
    pub total_events: usize,
}

/// Replays saved order book snapshots through a strategy.
pub struct BacktestRunner;

impl BacktestRunner {
    /// Run a strategy against a slice of snapshots and collect triggers.
    pub fn run(mut strategy: Box<dyn Strategy>, snapshots: &[OrderBookSnapshot]) -> BacktestResult {
        let mut triggers = Vec::new();
        for snapshot in snapshots {
            if let Some(trigger) = strategy.on_book_update(snapshot) {
                triggers.push(trigger);
            }
        }
        BacktestResult {
            triggers,
            total_snapshots: snapshots.len(),
            total_events: snapshots.len(),
        }
    }

    /// Replays normalized updates through the hot-state store, then resolves the
    /// current snapshot view from each notice before invoking the existing strategy trait.
    pub fn run_notice_replay(
        mut strategy: Box<dyn Strategy>,
        markets: &[MarketMeta],
        updates: &[NormalizedUpdate],
    ) -> BacktestResult {
        let store = HotStateStore::new();
        for market in markets {
            store.register_market(market);
        }

        let mut triggers = Vec::new();
        let mut total_snapshots = 0;
        for update in updates {
            store.apply_update(update);
            let Some(snapshot) = store.project_snapshot(&update.notice) else {
                continue;
            };

            total_snapshots += 1;
            if let Some(trigger) = strategy.on_book_update(&snapshot) {
                triggers.push(trigger);
            }
        }

        BacktestResult {
            triggers,
            total_snapshots,
            total_events: updates.len(),
        }
    }

    /// Load snapshots from a JSON file.
    pub fn load_snapshots(
        path: &Path,
    ) -> Result<Vec<OrderBookSnapshot>, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let snapshots: Vec<OrderBookSnapshot> = serde_json::from_str(&content)?;
        Ok(snapshots)
    }
}
