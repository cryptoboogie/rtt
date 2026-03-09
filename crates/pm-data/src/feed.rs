use std::sync::Arc;

use tokio::sync::broadcast;

use crate::orderbook::OrderBookManager;
use crate::reference_store::{ReferenceState, ReferenceStore};
use crate::types::{
    BookUpdate, OrderBookSnapshot, PriceChangeBatchEntry, WsMessage, WsOrderBookLevel,
};
use crate::ws::WsClient;
use rtt_core::{
    polymarket::public_source_id, BookLevel, NormalizedUpdate, NormalizedUpdatePayload, SourceId,
    SourceKind, UpdateKind, UpdateNotice,
};

pub const DEFAULT_UPDATE_CHANNEL_CAPACITY: usize = 1024;
pub const DEFAULT_NOTICE_CHANNEL_CAPACITY: usize = 1024;

pub trait FeedAdapter<Message> {
    fn source_id(&self) -> &SourceId;
    fn source_kind(&self) -> SourceKind;
    fn normalize(&self, message: &Message) -> Vec<NormalizedUpdate>;
}

#[derive(Debug, Clone)]
pub enum ResolvedFeedState {
    OrderBook(OrderBookSnapshot),
    Reference(ReferenceState),
}

#[derive(Clone, Default)]
pub struct FeedStores {
    order_books: OrderBookManager,
    reference_store: ReferenceStore,
}

#[derive(Clone)]
pub struct FeedOutputs {
    update_tx: broadcast::Sender<NormalizedUpdate>,
    notice_tx: broadcast::Sender<UpdateNotice>,
    snapshot_tx: broadcast::Sender<OrderBookSnapshot>,
}

pub struct ScopedPolymarketAdapter {
    source_id: SourceId,
}

pub struct PolymarketFeedManager {
    source_id: SourceId,
    asset_ids: Vec<String>,
    ws_channel_capacity: usize,
    ws_client: WsClient,
    stores: FeedStores,
    outputs: FeedOutputs,
}

impl FeedStores {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn order_books(&self) -> OrderBookManager {
        self.order_books.clone()
    }

    pub fn reference_store(&self) -> ReferenceStore {
        self.reference_store.clone()
    }

    pub fn resolve_notice(&self, notice: &UpdateNotice) -> Option<ResolvedFeedState> {
        match notice.kind {
            UpdateKind::BookSnapshot | UpdateKind::BookDelta => self
                .order_books
                .get_snapshot(&notice.subject.instrument_id)
                .map(ResolvedFeedState::OrderBook),
            _ => self
                .reference_store
                .resolve_subject(&notice.subject)
                .map(ResolvedFeedState::Reference),
        }
    }

    pub fn clear_source(&self, source_id: &SourceId) {
        self.order_books.clear_all();
        self.reference_store.clear_source(source_id);
    }
}

impl FeedOutputs {
    pub fn new(
        update_channel_capacity: usize,
        notice_channel_capacity: usize,
        snapshot_channel_capacity: usize,
    ) -> Self {
        let (update_tx, _) = broadcast::channel(update_channel_capacity);
        let (notice_tx, _) = broadcast::channel(notice_channel_capacity);
        let (snapshot_tx, _) = broadcast::channel(snapshot_channel_capacity);
        Self {
            update_tx,
            notice_tx,
            snapshot_tx,
        }
    }

    pub fn subscribe_updates(&self) -> broadcast::Receiver<NormalizedUpdate> {
        self.update_tx.subscribe()
    }

    pub fn subscribe_notices(&self) -> broadcast::Receiver<UpdateNotice> {
        self.notice_tx.subscribe()
    }

    pub fn subscribe_snapshots(&self) -> broadcast::Receiver<OrderBookSnapshot> {
        self.snapshot_tx.subscribe()
    }
}

impl ScopedPolymarketAdapter {
    pub fn new(source_id: SourceId) -> Self {
        Self { source_id }
    }
}

impl FeedAdapter<WsMessage> for ScopedPolymarketAdapter {
    fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    fn source_kind(&self) -> SourceKind {
        SourceKind::PolymarketWs
    }

    fn normalize(&self, message: &WsMessage) -> Vec<NormalizedUpdate> {
        message
            .to_normalized_updates()
            .into_iter()
            .map(|update| scope_update_to_source(update, &self.source_id, self.source_kind()))
            .collect()
    }
}

impl PolymarketFeedManager {
    pub fn new(
        source_id: SourceId,
        asset_ids: Vec<String>,
        ws_channel_capacity: usize,
        update_channel_capacity: usize,
        notice_channel_capacity: usize,
        snapshot_channel_capacity: usize,
    ) -> Self {
        Self {
            source_id,
            asset_ids: asset_ids.clone(),
            ws_channel_capacity,
            ws_client: WsClient::new(asset_ids, ws_channel_capacity),
            stores: FeedStores::new(),
            outputs: FeedOutputs::new(
                update_channel_capacity,
                notice_channel_capacity,
                snapshot_channel_capacity,
            ),
        }
    }

    pub fn shared(
        asset_ids: Vec<String>,
        ws_channel_capacity: usize,
        snapshot_channel_capacity: usize,
    ) -> Self {
        Self::new(
            public_source_id(),
            asset_ids,
            ws_channel_capacity,
            DEFAULT_UPDATE_CHANNEL_CAPACITY,
            DEFAULT_NOTICE_CHANNEL_CAPACITY,
            snapshot_channel_capacity,
        )
    }

    pub fn subscribe_updates(&self) -> broadcast::Receiver<NormalizedUpdate> {
        self.outputs.subscribe_updates()
    }

    pub fn subscribe_notices(&self) -> broadcast::Receiver<UpdateNotice> {
        self.outputs.subscribe_notices()
    }

    pub fn subscribe_snapshots(&self) -> broadcast::Receiver<OrderBookSnapshot> {
        self.outputs.subscribe_snapshots()
    }

    pub fn order_books(&self) -> OrderBookManager {
        self.stores.order_books()
    }

    pub fn reference_store(&self) -> ReferenceStore {
        self.stores.reference_store()
    }

    pub fn stores(&self) -> FeedStores {
        self.stores.clone()
    }

    pub fn source_id(&self) -> &SourceId {
        &self.source_id
    }

    pub fn asset_ids(&self) -> &[String] {
        &self.asset_ids
    }

    pub fn ws_client_last_message_at(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.ws_client.last_message_at_arc()
    }

    pub fn ws_client_reconnect_count(&self) -> Arc<std::sync::atomic::AtomicU64> {
        self.ws_client.reconnect_count_arc()
    }

    pub fn process_message(&self, _message: &WsMessage) -> usize {
        let adapter = ScopedPolymarketAdapter::new(self.source_id.clone());
        process_adapter_message(&adapter, _message, &self.stores, &self.outputs)
    }

    pub fn reconfigure_assets(&mut self, asset_ids: Vec<String>) -> bool {
        if self.asset_ids == asset_ids {
            return false;
        }

        self.stores.clear_source(&self.source_id);
        self.asset_ids = asset_ids.clone();
        self.ws_client = WsClient::new(asset_ids, self.ws_channel_capacity);
        true
    }

    pub async fn run(&mut self) {
        let mut ws_rx = self.ws_client.subscribe();
        let stores = self.stores.clone();
        let outputs = self.outputs.clone();
        let source_id = self.source_id.clone();

        let processor = tokio::spawn(async move {
            let adapter = ScopedPolymarketAdapter::new(source_id);
            loop {
                match ws_rx.recv().await {
                    Ok(message) => {
                        process_adapter_message(&adapter, &message, &stores, &outputs);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        self.ws_client.run().await;
        let _ = processor.await;
    }

    pub fn shutdown(&self) {
        self.ws_client.shutdown();
    }
}

pub fn process_adapter_message<Message, Adapter>(
    adapter: &Adapter,
    message: &Message,
    stores: &FeedStores,
    outputs: &FeedOutputs,
) -> usize
where
    Adapter: FeedAdapter<Message>,
{
    let updates = adapter.normalize(message);

    for update in &updates {
        apply_update_to_stores(stores, update);
        let _ = outputs.update_tx.send(update.clone());
        let _ = outputs.notice_tx.send(update.notice.clone());

        if let Some(snapshot) = snapshot_from_update(stores, update) {
            let _ = outputs.snapshot_tx.send(snapshot);
        }
    }

    updates.len()
}

fn scope_update_to_source(
    mut update: NormalizedUpdate,
    source_id: &SourceId,
    source_kind: SourceKind,
) -> NormalizedUpdate {
    update.notice.source_id = source_id.clone();
    update.notice.source_kind = source_kind;
    update.notice.subject.source_id = source_id.clone();
    update
}

fn apply_update_to_stores(stores: &FeedStores, update: &NormalizedUpdate) {
    match &update.payload {
        NormalizedUpdatePayload::BookSnapshot(snapshot) => {
            let book = BookUpdate {
                asset_id: snapshot.asset_id.as_str().to_string(),
                market: snapshot.market_id.as_str().to_string(),
                timestamp: snapshot.timestamp_ms.to_string(),
                bids: snapshot.bids.iter().map(book_level_to_ws_level).collect(),
                asks: snapshot.asks.iter().map(book_level_to_ws_level).collect(),
                hash: snapshot.source_hash.clone(),
            };
            stores.order_books.apply_book_update(&book);
        }
        NormalizedUpdatePayload::BookDelta(delta) => {
            let change = PriceChangeBatchEntry {
                asset_id: delta.asset_id.as_str().to_string(),
                price: delta.price.as_str().to_string(),
                size: Some(delta.size.as_str().to_string()),
                side: delta.side,
                hash: delta.source_hash.clone(),
                best_bid: delta
                    .best_bid
                    .as_ref()
                    .map(|price| price.as_str().to_string()),
                best_ask: delta
                    .best_ask
                    .as_ref()
                    .map(|price| price.as_str().to_string()),
            };
            stores
                .order_books
                .apply_price_change(&change, delta.timestamp_ms);
        }
        NormalizedUpdatePayload::Reconnect(_) => {
            stores.order_books.clear_all();
            stores.reference_store.apply_update(update);
        }
        NormalizedUpdatePayload::BestBidAsk(_)
        | NormalizedUpdatePayload::TradeTick(_)
        | NormalizedUpdatePayload::ReferencePrice(_)
        | NormalizedUpdatePayload::TickSizeChange(_)
        | NormalizedUpdatePayload::SourceStatus(_) => {
            stores.reference_store.apply_update(update);
        }
    }
}

fn snapshot_from_update(
    stores: &FeedStores,
    update: &NormalizedUpdate,
) -> Option<OrderBookSnapshot> {
    match &update.payload {
        NormalizedUpdatePayload::BookSnapshot(snapshot) => {
            stores.order_books.get_snapshot(snapshot.asset_id.as_str())
        }
        NormalizedUpdatePayload::BookDelta(delta) => {
            stores.order_books.get_snapshot(delta.asset_id.as_str())
        }
        _ => None,
    }
}

fn book_level_to_ws_level(level: &BookLevel) -> WsOrderBookLevel {
    WsOrderBookLevel {
        price: level.price.as_str().to_string(),
        size: level.size.as_str().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        BestBidAskEvent, BookUpdate, LastTradePriceEvent, PriceChangeBatchEntry, PriceChangeEvent,
        ReconnectEvent, Side, WsOrderBookLevel,
    };
    use rtt_core::{
        feed_source::{InstrumentKind, InstrumentRef},
        market::Price,
        public_event::{NormalizedUpdate, NormalizedUpdatePayload, ReferencePriceUpdate},
        UpdateKind,
    };

    struct PassthroughReferenceAdapter {
        source_id: SourceId,
    }

    impl FeedAdapter<NormalizedUpdate> for PassthroughReferenceAdapter {
        fn source_id(&self) -> &SourceId {
            &self.source_id
        }

        fn source_kind(&self) -> SourceKind {
            SourceKind::ExternalReference
        }

        fn normalize(&self, message: &NormalizedUpdate) -> Vec<NormalizedUpdate> {
            vec![message.clone()]
        }
    }

    #[test]
    fn scoped_polymarket_adapter_rewrites_source_identity_per_manager() {
        let adapter = ScopedPolymarketAdapter::new(SourceId::new("polymarket-dedicated"));
        let message = WsMessage::BestBidAsk(BestBidAskEvent {
            asset_id: "asset-1".to_string(),
            market: "market-1".to_string(),
            best_bid: "0.44".to_string(),
            best_ask: "0.45".to_string(),
            spread: "0.01".to_string(),
            timestamp: "1700000000000".to_string(),
        });

        let updates = adapter.normalize(&message);
        let update = updates.first().expect("one normalized update");

        assert_eq!(update.notice.source_id.as_str(), "polymarket-dedicated");
        assert_eq!(
            update.notice.subject.source_id.as_str(),
            "polymarket-dedicated"
        );
        assert_eq!(update.notice.source_kind, SourceKind::PolymarketWs);
    }

    #[test]
    fn process_adapter_message_emits_notices_and_legacy_snapshots() {
        let adapter = ScopedPolymarketAdapter::new(SourceId::new("polymarket-shared"));
        let stores = FeedStores::new();
        let outputs = FeedOutputs::new(16, 16, 16);
        let mut update_rx = outputs.subscribe_updates();
        let mut notice_rx = outputs.subscribe_notices();
        let mut snapshot_rx = outputs.subscribe_snapshots();

        let book = WsMessage::Book(BookUpdate {
            asset_id: "asset-1".to_string(),
            market: "market-1".to_string(),
            timestamp: "1700000000000".to_string(),
            bids: vec![WsOrderBookLevel {
                price: "0.44".to_string(),
                size: "100".to_string(),
            }],
            asks: vec![WsOrderBookLevel {
                price: "0.45".to_string(),
                size: "150".to_string(),
            }],
            hash: Some("hash-1".to_string()),
        });
        let delta = WsMessage::PriceChange(PriceChangeEvent {
            market: "market-1".to_string(),
            timestamp: "1700000001000".to_string(),
            price_changes: vec![PriceChangeBatchEntry {
                asset_id: "asset-1".to_string(),
                price: "0.43".to_string(),
                size: Some("200".to_string()),
                side: Side::Buy,
                hash: Some("hash-2".to_string()),
                best_bid: Some("0.44".to_string()),
                best_ask: Some("0.45".to_string()),
            }],
        });

        assert_eq!(
            process_adapter_message(&adapter, &book, &stores, &outputs),
            1
        );
        assert_eq!(
            process_adapter_message(&adapter, &delta, &stores, &outputs),
            1
        );

        let _book_update = update_rx.try_recv().expect("book update");
        let delta_update = update_rx.try_recv().expect("delta update");
        assert_eq!(delta_update.notice.kind, UpdateKind::BookDelta);

        let delta_notice = notice_rx.try_recv().expect("book notice");
        assert_eq!(delta_notice.subject.instrument_id, "asset-1");
        let _book_notice = notice_rx.try_recv().expect("delta notice");

        let snapshot = snapshot_rx.try_recv().expect("snapshot");
        assert_eq!(snapshot.asset_id, "asset-1");

        match stores
            .resolve_notice(&delta_notice)
            .expect("resolved state")
        {
            ResolvedFeedState::OrderBook(snapshot) => {
                assert_eq!(snapshot.best_bid.expect("best bid").price, "0.44");
            }
            other => panic!("expected order book state, got {other:?}"),
        }
    }

    #[test]
    fn informational_updates_survive_notice_path_and_resolve_from_reference_store() {
        let stores = FeedStores::new();
        let outputs = FeedOutputs::new(16, 16, 16);
        let adapter = PassthroughReferenceAdapter {
            source_id: SourceId::new("reference-mid"),
        };
        let mut update_rx = outputs.subscribe_updates();
        let mut notice_rx = outputs.subscribe_notices();

        let subject = InstrumentRef {
            source_id: adapter.source_id().clone(),
            kind: InstrumentKind::Symbol,
            instrument_id: "BTC-USD".to_string(),
        };
        let update = NormalizedUpdate {
            notice: UpdateNotice {
                source_id: adapter.source_id().clone(),
                source_kind: SourceKind::ExternalReference,
                subject: subject.clone(),
                kind: UpdateKind::ReferencePrice,
                version: 7,
                source_hash: None,
            },
            payload: NormalizedUpdatePayload::ReferencePrice(ReferencePriceUpdate {
                price: Price::new("62345.12"),
                notional: None,
                timestamp_ms: 1_700_000_000_123,
            }),
        };

        assert_eq!(
            process_adapter_message(&adapter, &update, &stores, &outputs),
            1
        );

        let emitted_update = update_rx.try_recv().expect("update");
        assert_eq!(emitted_update.notice.kind, UpdateKind::ReferencePrice);
        let notice = notice_rx.try_recv().expect("notice");

        match stores.resolve_notice(&notice).expect("resolved state") {
            ResolvedFeedState::Reference(state) => {
                assert_eq!(
                    state
                        .last_reference_price
                        .expect("reference price")
                        .price
                        .as_str(),
                    "62345.12"
                );
            }
            other => panic!("expected reference state, got {other:?}"),
        }
    }

    #[test]
    fn reconfigure_assets_clears_authoritative_state_before_swapping_subscription_set() {
        let mut manager = PolymarketFeedManager::shared(vec!["asset-1".to_string()], 16, 16);
        let book = WsMessage::Book(BookUpdate {
            asset_id: "asset-1".to_string(),
            market: "market-1".to_string(),
            timestamp: "1700000000000".to_string(),
            bids: vec![WsOrderBookLevel {
                price: "0.44".to_string(),
                size: "100".to_string(),
            }],
            asks: vec![WsOrderBookLevel {
                price: "0.45".to_string(),
                size: "150".to_string(),
            }],
            hash: Some("hash-1".to_string()),
        });
        manager.process_message(&book);
        assert!(manager.order_books().get_snapshot("asset-1").is_some());

        assert!(manager.reconfigure_assets(vec!["asset-2".to_string()]));
        assert_eq!(manager.asset_ids(), &["asset-2".to_string()]);
        assert!(manager.order_books().get_snapshot("asset-1").is_none());
    }

    #[test]
    fn reconnect_messages_clear_books_and_record_source_reset_notice() {
        let manager = PolymarketFeedManager::shared(vec!["asset-1".to_string()], 16, 16);
        let book = WsMessage::Book(BookUpdate {
            asset_id: "asset-1".to_string(),
            market: "market-1".to_string(),
            timestamp: "1700000000000".to_string(),
            bids: vec![WsOrderBookLevel {
                price: "0.44".to_string(),
                size: "100".to_string(),
            }],
            asks: vec![WsOrderBookLevel {
                price: "0.45".to_string(),
                size: "150".to_string(),
            }],
            hash: Some("hash-1".to_string()),
        });
        manager.process_message(&book);

        let reconnect = WsMessage::Reconnected(ReconnectEvent {
            sequence: 3,
            timestamp_ms: 1_700_000_000_555,
        });
        manager.process_message(&reconnect);

        assert!(manager.order_books().get_snapshot("asset-1").is_none());
        let subject = InstrumentRef::source(manager.source_id().clone());
        let state = manager
            .reference_store()
            .resolve_subject(&subject)
            .expect("source state");
        assert_eq!(state.last_reconnect.expect("reconnect").sequence, 3);
    }

    #[test]
    fn informational_polymarket_events_do_not_emit_legacy_snapshots() {
        let manager = PolymarketFeedManager::shared(vec!["asset-1".to_string()], 16, 16);
        let mut snapshot_rx = manager.subscribe_snapshots();

        manager.process_message(&WsMessage::LastTradePrice(LastTradePriceEvent {
            asset_id: "asset-1".to_string(),
            market: "market-1".to_string(),
            price: "0.44".to_string(),
            side: Some(Side::Buy),
            size: Some("50".to_string()),
            fee_rate_bps: None,
            timestamp: "1700000000000".to_string(),
        }));

        assert!(snapshot_rx.try_recv().is_err());
    }
}
