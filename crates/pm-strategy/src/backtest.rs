use crate::strategy::Strategy;
use crate::types::{OrderBookSnapshot, TriggerMessage};
use std::path::Path;

/// Result of a backtest run.
pub struct BacktestResult {
    pub triggers: Vec<TriggerMessage>,
    pub total_snapshots: usize,
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
        }
    }

    /// Load snapshots from a JSON file.
    pub fn load_snapshots(path: &Path) -> Result<Vec<OrderBookSnapshot>, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let snapshots: Vec<OrderBookSnapshot> = serde_json::from_str(&content)?;
        Ok(snapshots)
    }
}
