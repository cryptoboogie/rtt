use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::orderbook::OrderBookManager;
use crate::types::{OrderBookSnapshot, WsMessage};
use crate::ws::WsClient;

/// Full pipeline: WS -> parse -> update order book -> notify via channel.
pub struct Pipeline {
    ws_client: WsClient,
    order_books: OrderBookManager,
    snapshot_tx: broadcast::Sender<OrderBookSnapshot>,
}

impl Pipeline {
    pub fn new(
        asset_ids: Vec<String>,
        ws_channel_capacity: usize,
        snapshot_channel_capacity: usize,
    ) -> Self {
        let ws_client = WsClient::new(asset_ids, ws_channel_capacity);
        let order_books = OrderBookManager::new();
        let (snapshot_tx, _) = broadcast::channel(snapshot_channel_capacity);
        Self {
            ws_client,
            order_books,
            snapshot_tx,
        }
    }

    /// Subscribe to OrderBookSnapshot notifications.
    pub fn subscribe_snapshots(&self) -> broadcast::Receiver<OrderBookSnapshot> {
        self.snapshot_tx.subscribe()
    }

    /// Get a clone of the OrderBookManager for direct reads.
    pub fn order_books(&self) -> OrderBookManager {
        self.order_books.clone()
    }

    /// Arc to the WsClient's last_message_at counter.
    pub fn ws_client_last_message_at(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.ws_client.last_message_at_arc()
    }

    /// Arc to the WsClient's reconnect counter.
    pub fn ws_client_reconnect_count(&self) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
        self.ws_client.reconnect_count_arc()
    }

    /// Run the pipeline. This spawns the WS client and processes messages.
    pub async fn run(&mut self) {
        let mut ws_rx = self.ws_client.subscribe();
        let order_books = self.order_books.clone();
        let snapshot_tx = self.snapshot_tx.clone();

        let processor = tokio::spawn(async move {
            loop {
                match ws_rx.recv().await {
                    Ok(msg) => {
                        process_message(&msg, &order_books, &snapshot_tx);
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Pipeline lagged, missed {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("WS channel closed, pipeline stopping");
                        break;
                    }
                }
            }
        });

        self.ws_client.run().await;
        let _ = processor.await;
    }

    /// Shutdown the pipeline.
    pub fn shutdown(&self) {
        self.ws_client.shutdown();
    }
}

fn process_message(
    msg: &WsMessage,
    order_books: &OrderBookManager,
    snapshot_tx: &broadcast::Sender<OrderBookSnapshot>,
) {
    match msg {
        WsMessage::Book(book) => {
            order_books.apply_book_update(book);
            if let Some(snap) = order_books.get_snapshot(&book.asset_id) {
                let _ = snapshot_tx.send(snap);
            }
        }
        WsMessage::PriceChange(pc) => {
            let ts: u64 = pc.timestamp.parse().unwrap_or(0);
            for entry in &pc.price_changes {
                order_books.apply_price_change(entry, ts);
                if let Some(snap) = order_books.get_snapshot(&entry.asset_id) {
                    let _ = snapshot_tx.send(snap);
                }
            }
        }
        WsMessage::Reconnected(_) => {
            info!("WS reconnected - clearing order book state");
            order_books.clear_all();
        }
        WsMessage::BestBidAsk(_) | WsMessage::LastTradePrice(_) | WsMessage::TickSizeChange(_) => {
            // These don't modify the order book, just informational
        }
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
        let order_books = OrderBookManager::new();
        let (snapshot_tx, mut snapshot_rx) = broadcast::channel(10);

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

        process_message(&msg, &order_books, &snapshot_tx);

        let snap = snapshot_rx.try_recv().unwrap();
        assert_eq!(snap.asset_id, "asset1");
        assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.55");
        assert_eq!(snap.best_ask.as_ref().unwrap().price, "0.56");
        assert_eq!(snap.hash, "hash1");
    }

    #[test]
    fn process_price_change_updates_and_notifies() {
        let order_books = OrderBookManager::new();
        let (snapshot_tx, mut snapshot_rx) = broadcast::channel(10);

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
        process_message(&book_msg, &order_books, &snapshot_tx);
        let _ = snapshot_rx.try_recv();

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
        process_message(&pc_msg, &order_books, &snapshot_tx);

        let snap = snapshot_rx.try_recv().unwrap();
        assert_eq!(snap.best_ask.as_ref().unwrap().price, "0.56");
        assert_eq!(snap.hash, "hash2");
    }

    #[test]
    fn process_informational_messages_do_not_modify_book() {
        let order_books = OrderBookManager::new();
        let (snapshot_tx, mut snapshot_rx) = broadcast::channel(10);

        let msg = WsMessage::LastTradePrice(crate::types::LastTradePriceEvent {
            asset_id: "asset1".to_string(),
            market: "0xmarket".to_string(),
            price: "0.55".to_string(),
            side: Some(Side::Buy),
            size: Some("50".to_string()),
            fee_rate_bps: None,
            timestamp: "1700000000000".to_string(),
        });
        process_message(&msg, &order_books, &snapshot_tx);

        assert!(snapshot_rx.try_recv().is_err());
        assert!(order_books.get_snapshot("asset1").is_none());
    }

    #[test]
    fn process_reconnected_clears_order_book() {
        let order_books = OrderBookManager::new();
        let (snapshot_tx, mut snapshot_rx) = broadcast::channel(10);

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
        process_message(&msg, &order_books, &snapshot_tx);
        let _ = snapshot_rx.try_recv();

        assert!(order_books.get_snapshot("asset1").is_some());

        process_message(&WsMessage::Reconnected(crate::types::ReconnectEvent {
            sequence: 1,
            timestamp_ms: 1_700_000_001_000,
        }), &order_books, &snapshot_tx);

        assert!(order_books.get_snapshot("asset1").is_none());
        assert_eq!(order_books.asset_count(), 0);
    }

    #[test]
    fn pipeline_construction() {
        let pipeline = Pipeline::new(vec!["asset1".to_string(), "asset2".to_string()], 100, 50);
        let _rx = pipeline.subscribe_snapshots();
        let books = pipeline.order_books();
        assert!(books.get_snapshot("asset1").is_none());
    }
}
