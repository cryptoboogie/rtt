use tokio::sync::broadcast;

use crate::feed::PolymarketFeedManager;
use crate::orderbook::OrderBookManager;
use crate::reference_store::ReferenceStore;
use crate::types::{OrderBookSnapshot, WsMessage};
use rtt_core::{NormalizedUpdate, UpdateNotice};

/// Transitional runtime seam that preserves the legacy snapshot path while exposing
/// the 11c feed-manager notice/update streams underneath it.
pub struct Pipeline {
    feed_manager: PolymarketFeedManager,
}

impl Pipeline {
    pub fn new(
        asset_ids: Vec<String>,
        ws_channel_capacity: usize,
        snapshot_channel_capacity: usize,
    ) -> Self {
        Self {
            feed_manager: PolymarketFeedManager::shared(
                asset_ids,
                ws_channel_capacity,
                snapshot_channel_capacity,
            ),
        }
    }

    /// Subscribe to OrderBookSnapshot notifications.
    pub fn subscribe_snapshots(&self) -> broadcast::Receiver<OrderBookSnapshot> {
        self.feed_manager.subscribe_snapshots()
    }

    /// Subscribe to normalized updates emitted by the feed manager.
    pub fn subscribe_updates(&self) -> broadcast::Receiver<NormalizedUpdate> {
        self.feed_manager.subscribe_updates()
    }

    /// Subscribe to small change notices emitted by the feed manager.
    pub fn subscribe_notices(&self) -> broadcast::Receiver<UpdateNotice> {
        self.feed_manager.subscribe_notices()
    }

    /// Get a clone of the OrderBookManager for direct reads.
    pub fn order_books(&self) -> OrderBookManager {
        self.feed_manager.order_books()
    }

    /// Get a clone of the non-depth store for notice resolution.
    pub fn reference_store(&self) -> ReferenceStore {
        self.feed_manager.reference_store()
    }

    /// Test seam and transition helper for direct message processing.
    pub fn process_message(&self, message: &WsMessage) -> usize {
        self.feed_manager.process_message(message)
    }

    /// Conservative full-asset-set reset/reconfigure path for later registry integration.
    pub fn reconfigure_assets(&self, asset_ids: Vec<String>) -> bool {
        self.feed_manager.reconfigure_assets(asset_ids)
    }

    /// Arc to the WsClient's last_message_at counter.
    pub fn ws_client_last_message_at(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.feed_manager.ws_client_last_message_at()
    }

    /// Arc to the WsClient's reconnect counter.
    pub fn ws_client_reconnect_count(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.feed_manager.ws_client_reconnect_count()
    }

    /// Run the pipeline. This spawns the WS client and processes messages.
    pub async fn run(&self) {
        self.feed_manager.run().await;
    }

    /// Shutdown the pipeline.
    pub fn shutdown(&self) {
        self.feed_manager.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        BookUpdate, PriceChangeBatchEntry, PriceChangeEvent, Side, WsOrderBookLevel,
    };

    #[test]
    fn process_book_message_updates_orderbook_and_notifies() {
        let pipeline = Pipeline::new(vec!["asset1".to_string()], 10, 10);
        let mut snapshot_rx = pipeline.subscribe_snapshots();

        let msg = WsMessage::Book(BookUpdate {
            asset_id: "asset1".to_string(),
            market: "0xmarket".to_string(),
            timestamp: "1700000000000".to_string(),
            bids: vec![WsOrderBookLevel {
                price: "0.55".to_string(),
                size: "100".to_string(),
            }],
            asks: vec![WsOrderBookLevel {
                price: "0.56".to_string(),
                size: "150".to_string(),
            }],
            hash: Some("hash1".to_string()),
        });

        pipeline.process_message(&msg);

        let snap = snapshot_rx.try_recv().unwrap();
        assert_eq!(snap.asset_id, "asset1");
        assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.55");
        assert_eq!(snap.best_ask.as_ref().unwrap().price, "0.56");
        assert_eq!(snap.hash, "hash1");
        assert!(pipeline.order_books().get_snapshot("asset1").is_some());
    }

    #[test]
    fn process_price_change_updates_and_notifies() {
        let pipeline = Pipeline::new(vec!["asset1".to_string()], 10, 10);
        let mut snapshot_rx = pipeline.subscribe_snapshots();

        // First set up the book
        let book_msg = WsMessage::Book(BookUpdate {
            asset_id: "asset1".to_string(),
            market: "0xmarket".to_string(),
            timestamp: "1700000000000".to_string(),
            bids: vec![WsOrderBookLevel {
                price: "0.55".to_string(),
                size: "100".to_string(),
            }],
            asks: vec![WsOrderBookLevel {
                price: "0.56".to_string(),
                size: "150".to_string(),
            }],
            hash: Some("hash1".to_string()),
        });
        pipeline.process_message(&book_msg);
        let _ = snapshot_rx.try_recv(); // consume book notification

        // Now apply a price change
        let pc_msg = WsMessage::PriceChange(PriceChangeEvent {
            market: "0xmarket".to_string(),
            timestamp: "1700000001000".to_string(),
            price_changes: vec![PriceChangeBatchEntry {
                asset_id: "asset1".to_string(),
                price: "0.57".to_string(),
                size: Some("200".to_string()),
                side: Side::Sell,
                hash: Some("hash2".to_string()),
                best_bid: None,
                best_ask: None,
            }],
        });
        pipeline.process_message(&pc_msg);

        let snap = snapshot_rx.try_recv().unwrap();
        assert_eq!(snap.best_ask.as_ref().unwrap().price, "0.56"); // 0.56 < 0.57
        assert_eq!(snap.hash, "hash2");
    }

    #[test]
    fn process_informational_messages_do_not_modify_book() {
        let pipeline = Pipeline::new(vec!["asset1".to_string()], 10, 10);
        let mut snapshot_rx = pipeline.subscribe_snapshots();

        let msg = WsMessage::LastTradePrice(crate::types::LastTradePriceEvent {
            asset_id: "asset1".to_string(),
            market: "0xmarket".to_string(),
            price: "0.55".to_string(),
            side: Some(Side::Buy),
            size: Some("50".to_string()),
            fee_rate_bps: None,
            timestamp: "1700000000000".to_string(),
        });
        pipeline.process_message(&msg);

        // No snapshot should be emitted
        assert!(snapshot_rx.try_recv().is_err());
        // No book should exist, but the informational event is preserved in the reference store.
        assert!(pipeline.order_books().get_snapshot("asset1").is_none());
        assert_eq!(pipeline.reference_store().len(), 1);
    }

    #[test]
    fn process_reconnected_clears_order_book() {
        let pipeline = Pipeline::new(vec!["asset1".to_string()], 10, 10);
        let mut snapshot_rx = pipeline.subscribe_snapshots();

        // First populate the book
        let msg = WsMessage::Book(BookUpdate {
            asset_id: "asset1".to_string(),
            market: "0xmarket".to_string(),
            timestamp: "1700000000000".to_string(),
            bids: vec![WsOrderBookLevel {
                price: "0.55".to_string(),
                size: "100".to_string(),
            }],
            asks: vec![WsOrderBookLevel {
                price: "0.56".to_string(),
                size: "150".to_string(),
            }],
            hash: Some("hash1".to_string()),
        });
        pipeline.process_message(&msg);
        let _ = snapshot_rx.try_recv(); // consume

        assert!(pipeline.order_books().get_snapshot("asset1").is_some());

        // Send Reconnected — should clear all books
        pipeline.process_message(&WsMessage::Reconnected(crate::types::ReconnectEvent {
            sequence: 1,
            timestamp_ms: 1_700_000_001_000,
        }));

        assert!(pipeline.order_books().get_snapshot("asset1").is_none());
        assert_eq!(pipeline.order_books().asset_count(), 0);
    }

    #[test]
    fn pipeline_construction() {
        let pipeline = Pipeline::new(vec!["asset1".to_string(), "asset2".to_string()], 100, 50);
        let _rx = pipeline.subscribe_snapshots();
        let books = pipeline.order_books();
        assert!(books.get_snapshot("asset1").is_none());
    }

    #[test]
    fn pipeline_exposes_notice_and_update_subscriptions() {
        let pipeline = Pipeline::new(vec!["asset1".to_string()], 100, 50);
        let mut updates = pipeline.subscribe_updates();
        let mut notices = pipeline.subscribe_notices();

        let msg = WsMessage::BestBidAsk(crate::types::BestBidAskEvent {
            asset_id: "asset1".to_string(),
            market: "0xmarket".to_string(),
            best_bid: "0.55".to_string(),
            best_ask: "0.56".to_string(),
            spread: "0.01".to_string(),
            timestamp: "1700000000000".to_string(),
        });

        pipeline.process_message(&msg);

        assert_eq!(
            updates.try_recv().unwrap().notice.kind,
            rtt_core::UpdateKind::BestBidAsk
        );
        assert_eq!(
            notices.try_recv().unwrap().kind,
            rtt_core::UpdateKind::BestBidAsk
        );
    }

    #[test]
    fn pipeline_reconfigure_resets_legacy_state_and_updates_asset_set() {
        let pipeline = Pipeline::new(vec!["asset1".to_string()], 100, 50);
        let msg = WsMessage::Book(BookUpdate {
            asset_id: "asset1".to_string(),
            market: "0xmarket".to_string(),
            timestamp: "1700000000000".to_string(),
            bids: vec![WsOrderBookLevel {
                price: "0.55".to_string(),
                size: "100".to_string(),
            }],
            asks: vec![WsOrderBookLevel {
                price: "0.56".to_string(),
                size: "150".to_string(),
            }],
            hash: Some("hash1".to_string()),
        });

        pipeline.process_message(&msg);
        assert!(pipeline.order_books().get_snapshot("asset1").is_some());

        assert!(pipeline.reconfigure_assets(vec!["asset2".to_string()]));
        assert!(pipeline.order_books().get_snapshot("asset1").is_none());
    }

    #[test]
    fn pipeline_run_future_can_be_created_from_shared_reference() {
        let pipeline = Pipeline::new(vec!["asset1".to_string()], 100, 50);
        let _future = pipeline.run();
    }
}
