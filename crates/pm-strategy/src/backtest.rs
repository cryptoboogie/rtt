use crate::quote::DesiredQuotes;
use crate::runtime::SharedRuntimeScaffold;
use crate::strategy::{QuoteStrategy, Strategy, TriggerStrategy};
use crate::types::{OrderBookSnapshot, TriggerMessage};
use rtt_core::{HotStateStore, MarketMeta, NormalizedUpdate};
use std::path::Path;

/// Result of a trigger-strategy backtest run.
pub struct BacktestResult {
    pub triggers: Vec<TriggerMessage>,
    pub total_snapshots: usize,
    pub total_events: usize,
}

/// Result of a quote-strategy backtest run.
pub struct QuoteBacktestResult {
    pub desired_quotes: Vec<DesiredQuotes>,
    pub total_views: usize,
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

    /// Replays normalized updates through the shared 12b runtime scaffold using
    /// the explicit trigger-strategy contract.
    pub fn run_trigger_notice_replay(
        mut strategy: Box<dyn TriggerStrategy>,
        markets: &[MarketMeta],
        updates: &[NormalizedUpdate],
    ) -> BacktestResult {
        let store = HotStateStore::new();
        for market in markets {
            store.register_market(market);
        }

        let mut scaffold = SharedRuntimeScaffold::new(store.clone(), strategy.requirements());
        let mut triggers = Vec::new();
        let mut total_snapshots = 0;
        for update in updates {
            store.apply_update(update);
            let Some(view) = scaffold.resolve_view(&update.notice) else {
                continue;
            };

            total_snapshots += 1;
            if let Some(trigger) = strategy.on_update(&view) {
                triggers.push(trigger);
            }
        }

        BacktestResult {
            triggers,
            total_snapshots,
            total_events: updates.len(),
        }
    }

    /// Replays normalized updates through the shared 12b runtime scaffold using
    /// the explicit quote-strategy contract.
    pub fn run_quote_notice_replay(
        mut strategy: Box<dyn QuoteStrategy>,
        markets: &[MarketMeta],
        updates: &[NormalizedUpdate],
    ) -> QuoteBacktestResult {
        let store = HotStateStore::new();
        for market in markets {
            store.register_market(market);
        }

        let mut scaffold = SharedRuntimeScaffold::new(store.clone(), strategy.requirements());
        let mut desired_quotes = Vec::new();
        let mut total_views = 0;
        for update in updates {
            store.apply_update(update);
            let Some(view) = scaffold.resolve_view(&update.notice) else {
                continue;
            };

            total_views += 1;
            if let Some(next_quotes) = strategy.on_update(&view) {
                desired_quotes.push(next_quotes);
            }
        }

        QuoteBacktestResult {
            desired_quotes,
            total_views,
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
