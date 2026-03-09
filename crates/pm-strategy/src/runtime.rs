use tokio::sync::mpsc;

use crate::strategy::Strategy;
use crate::types::{OrderBookSnapshot, TriggerMessage};
use rtt_core::{HotStateStore, UpdateNotice};

/// Consumes small notices and resolves the current runtime view from `HotStateStore`.
/// This preserves the existing snapshot-based strategy trait during the 12a migration.
pub struct NoticeDrivenRuntime {
    strategy: Box<dyn Strategy>,
    store: HotStateStore,
    notice_rx: mpsc::Receiver<UpdateNotice>,
    trigger_tx: mpsc::Sender<TriggerMessage>,
}

impl NoticeDrivenRuntime {
    pub fn new(
        strategy: Box<dyn Strategy>,
        store: HotStateStore,
        notice_rx: mpsc::Receiver<UpdateNotice>,
        trigger_tx: mpsc::Sender<TriggerMessage>,
    ) -> Self {
        Self {
            strategy,
            store,
            notice_rx,
            trigger_tx,
        }
    }

    pub fn resolve_snapshot(&self, notice: &UpdateNotice) -> Option<OrderBookSnapshot> {
        self.store.project_snapshot(notice)
    }

    pub fn handle_notice(&mut self, notice: &UpdateNotice) -> Option<TriggerMessage> {
        let snapshot = self.resolve_snapshot(notice)?;
        self.strategy.on_book_update(&snapshot)
    }

    pub async fn run(&mut self) {
        while let Some(notice) = self.notice_rx.recv().await {
            let Some(trigger) = self.handle_notice(&notice) else {
                continue;
            };

            if self.trigger_tx.send(trigger).await.is_err() {
                break;
            }
        }
    }
}
