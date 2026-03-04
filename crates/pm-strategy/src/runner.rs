use crate::strategy::Strategy;
use crate::types::{OrderBookSnapshot, TriggerMessage};
use tokio::sync::mpsc;

/// Receives order book snapshots from a channel, runs them through the active
/// strategy, and forwards any resulting triggers to the executor channel.
pub struct StrategyRunner {
    strategy: Box<dyn Strategy>,
    snapshot_rx: mpsc::Receiver<OrderBookSnapshot>,
    trigger_tx: mpsc::Sender<TriggerMessage>,
}

impl StrategyRunner {
    pub fn new(
        strategy: Box<dyn Strategy>,
        snapshot_rx: mpsc::Receiver<OrderBookSnapshot>,
        trigger_tx: mpsc::Sender<TriggerMessage>,
    ) -> Self {
        Self {
            strategy,
            snapshot_rx,
            trigger_tx,
        }
    }

    /// Run the strategy loop until the snapshot channel closes.
    pub async fn run(&mut self) {
        while let Some(snapshot) = self.snapshot_rx.recv().await {
            if let Some(trigger) = self.strategy.on_book_update(&snapshot) {
                // If the trigger channel is closed, stop.
                if self.trigger_tx.send(trigger).await.is_err() {
                    break;
                }
            }
        }
    }
}
