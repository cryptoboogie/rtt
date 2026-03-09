use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use crate::types::{
    BookUpdate, OrderBookSnapshot, PriceChangeBatchEntry, PriceLevel, Side, WsOrderBookLevel,
};

/// Ordered price → size for one side of the book.
/// Bids: sorted descending (highest first). Asks: sorted ascending (lowest first).
/// We store price as a string key for exact decimal matching.
#[derive(Debug, Clone, Default)]
struct PriceLadder {
    levels: BTreeMap<String, String>,
}

impl PriceLadder {
    fn apply_levels(&mut self, levels: &[WsOrderBookLevel]) {
        self.levels.clear();
        for level in levels {
            self.levels.insert(level.price.clone(), level.size.clone());
        }
    }

    fn upsert(&mut self, price: &str, size: &str) {
        if size == "0" || size == "0.0" || size == "0.00" {
            self.levels.remove(price);
        } else {
            self.levels.insert(price.to_string(), size.to_string());
        }
    }

    fn best_bid(&self) -> Option<PriceLevel> {
        // Bids: highest price first → last entry in BTreeMap
        self.levels.iter().next_back().map(|(p, s)| PriceLevel {
            price: p.clone(),
            size: s.clone(),
        })
    }

    fn best_ask(&self) -> Option<PriceLevel> {
        // Asks: lowest price first → first entry in BTreeMap
        self.levels.iter().next().map(|(p, s)| PriceLevel {
            price: p.clone(),
            size: s.clone(),
        })
    }

    fn len(&self) -> usize {
        self.levels.len()
    }
}

#[derive(Debug, Clone)]
struct BookState {
    asset_id: String,
    bids: PriceLadder,
    asks: PriceLadder,
    hash: String,
    timestamp_ms: u64,
}

impl BookState {
    fn new(asset_id: String) -> Self {
        Self {
            asset_id,
            bids: PriceLadder::default(),
            asks: PriceLadder::default(),
            hash: String::new(),
            timestamp_ms: 0,
        }
    }

    fn apply_snapshot(&mut self, update: &BookUpdate) {
        self.bids.apply_levels(&update.bids);
        self.asks.apply_levels(&update.asks);
        if let Some(h) = &update.hash {
            self.hash = h.clone();
        }
        self.timestamp_ms = update.timestamp.parse().unwrap_or(0);
    }

    fn apply_delta(&mut self, entry: &PriceChangeBatchEntry, timestamp_ms: u64) {
        let size = entry.size.as_deref().unwrap_or("0");
        match entry.side {
            Side::Buy => self.bids.upsert(&entry.price, size),
            Side::Sell => self.asks.upsert(&entry.price, size),
        }
        if let Some(h) = &entry.hash {
            self.hash = h.clone();
        }
        self.timestamp_ms = timestamp_ms;
    }

    fn to_snapshot(&self) -> OrderBookSnapshot {
        OrderBookSnapshot {
            asset_id: self.asset_id.clone(),
            best_bid: self.bids.best_bid(),
            best_ask: self.asks.best_ask(),
            timestamp_ms: self.timestamp_ms,
            hash: self.hash.clone(),
        }
    }
}

/// Thread-safe order book manager for multiple assets.
#[derive(Clone)]
pub struct OrderBookManager {
    books: Arc<RwLock<std::collections::HashMap<String, BookState>>>,
}

impl OrderBookManager {
    pub fn new() -> Self {
        Self {
            books: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }

    pub fn apply_book_update(&self, update: &BookUpdate) {
        let mut books = self.books.write().unwrap();
        let book = books
            .entry(update.asset_id.clone())
            .or_insert_with(|| BookState::new(update.asset_id.clone()));
        book.apply_snapshot(update);
    }

    pub fn apply_price_change(&self, entry: &PriceChangeBatchEntry, timestamp_ms: u64) {
        let mut books = self.books.write().unwrap();
        let book = books
            .entry(entry.asset_id.clone())
            .or_insert_with(|| BookState::new(entry.asset_id.clone()));
        book.apply_delta(entry, timestamp_ms);
    }

    pub fn get_snapshot(&self, asset_id: &str) -> Option<OrderBookSnapshot> {
        let books = self.books.read().unwrap();
        books.get(asset_id).map(|b| b.to_snapshot())
    }

    pub fn get_all_snapshots(&self) -> Vec<OrderBookSnapshot> {
        let books = self.books.read().unwrap();
        books.values().map(|b| b.to_snapshot()).collect()
    }

    pub fn get_hash(&self, asset_id: &str) -> Option<String> {
        let books = self.books.read().unwrap();
        books.get(asset_id).map(|b| b.hash.clone())
    }

    pub fn bid_count(&self, asset_id: &str) -> usize {
        let books = self.books.read().unwrap();
        books.get(asset_id).map(|b| b.bids.len()).unwrap_or(0)
    }

    pub fn ask_count(&self, asset_id: &str) -> usize {
        let books = self.books.read().unwrap();
        books.get(asset_id).map(|b| b.asks.len()).unwrap_or(0)
    }

    /// Clear all order book state. Used on WS reconnect to avoid stale data.
    pub fn clear_all(&self) {
        let mut books = self.books.write().unwrap();
        books.clear();
    }

    /// Returns the number of tracked assets.
    pub fn asset_count(&self) -> usize {
        let books = self.books.read().unwrap();
        books.len()
    }
}

impl Default for OrderBookManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BookUpdate, PriceChangeBatchEntry, Side, WsOrderBookLevel};

    fn make_book_update(
        asset_id: &str,
        bids: &[(&str, &str)],
        asks: &[(&str, &str)],
    ) -> BookUpdate {
        BookUpdate {
            asset_id: asset_id.to_string(),
            market: "0xmarket".to_string(),
            timestamp: "1700000000000".to_string(),
            bids: bids
                .iter()
                .map(|(p, s)| WsOrderBookLevel {
                    price: p.to_string(),
                    size: s.to_string(),
                })
                .collect(),
            asks: asks
                .iter()
                .map(|(p, s)| WsOrderBookLevel {
                    price: p.to_string(),
                    size: s.to_string(),
                })
                .collect(),
            hash: Some("hash1".to_string()),
        }
    }

    #[test]
    fn apply_snapshot_sets_bids_and_asks() {
        let mgr = OrderBookManager::new();
        let update = make_book_update(
            "asset1",
            &[("0.55", "100"), ("0.54", "200")],
            &[("0.56", "150"), ("0.57", "250")],
        );
        mgr.apply_book_update(&update);

        let snap = mgr.get_snapshot("asset1").unwrap();
        assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.55");
        assert_eq!(snap.best_bid.as_ref().unwrap().size, "100");
        assert_eq!(snap.best_ask.as_ref().unwrap().price, "0.56");
        assert_eq!(snap.best_ask.as_ref().unwrap().size, "150");
        assert_eq!(snap.hash, "hash1");
        assert_eq!(mgr.bid_count("asset1"), 2);
        assert_eq!(mgr.ask_count("asset1"), 2);
    }

    #[test]
    fn apply_snapshot_replaces_previous() {
        let mgr = OrderBookManager::new();
        let update1 = make_book_update(
            "asset1",
            &[("0.55", "100"), ("0.54", "200"), ("0.53", "300")],
            &[("0.56", "150")],
        );
        mgr.apply_book_update(&update1);
        assert_eq!(mgr.bid_count("asset1"), 3);

        // Second snapshot should replace
        let update2 = make_book_update("asset1", &[("0.50", "50")], &[("0.60", "60")]);
        mgr.apply_book_update(&update2);
        assert_eq!(mgr.bid_count("asset1"), 1);
        let snap = mgr.get_snapshot("asset1").unwrap();
        assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.50");
    }

    #[test]
    fn apply_delta_upserts_level() {
        let mgr = OrderBookManager::new();
        let update = make_book_update("asset1", &[("0.55", "100")], &[("0.56", "150")]);
        mgr.apply_book_update(&update);

        // Add a new bid level
        let delta = PriceChangeBatchEntry {
            asset_id: "asset1".to_string(),
            price: "0.545".to_string(),
            size: Some("75".to_string()),
            side: Side::Buy,
            hash: Some("hash2".to_string()),
            best_bid: None,
            best_ask: None,
        };
        mgr.apply_price_change(&delta, 1700000001000);

        assert_eq!(mgr.bid_count("asset1"), 2);
        let snap = mgr.get_snapshot("asset1").unwrap();
        // 0.55 > 0.545 so best bid is still 0.55
        assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.55");
        assert_eq!(snap.hash, "hash2");
    }

    #[test]
    fn apply_delta_removes_level_on_zero_size() {
        let mgr = OrderBookManager::new();
        let update = make_book_update(
            "asset1",
            &[("0.55", "100"), ("0.54", "200")],
            &[("0.56", "150")],
        );
        mgr.apply_book_update(&update);

        // Remove the 0.55 bid
        let delta = PriceChangeBatchEntry {
            asset_id: "asset1".to_string(),
            price: "0.55".to_string(),
            size: Some("0".to_string()),
            side: Side::Buy,
            hash: None,
            best_bid: None,
            best_ask: None,
        };
        mgr.apply_price_change(&delta, 1700000001000);

        assert_eq!(mgr.bid_count("asset1"), 1);
        let snap = mgr.get_snapshot("asset1").unwrap();
        assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.54");
    }

    #[test]
    fn apply_delta_updates_existing_level() {
        let mgr = OrderBookManager::new();
        let update = make_book_update("asset1", &[("0.55", "100")], &[]);
        mgr.apply_book_update(&update);

        let delta = PriceChangeBatchEntry {
            asset_id: "asset1".to_string(),
            price: "0.55".to_string(),
            size: Some("250".to_string()),
            side: Side::Buy,
            hash: None,
            best_bid: None,
            best_ask: None,
        };
        mgr.apply_price_change(&delta, 1700000001000);

        assert_eq!(mgr.bid_count("asset1"), 1);
        let snap = mgr.get_snapshot("asset1").unwrap();
        assert_eq!(snap.best_bid.as_ref().unwrap().size, "250");
    }

    #[test]
    fn multiple_assets_independent() {
        let mgr = OrderBookManager::new();
        let update1 = make_book_update("asset1", &[("0.55", "100")], &[("0.56", "150")]);
        let update2 = make_book_update("asset2", &[("0.30", "500")], &[("0.35", "600")]);
        mgr.apply_book_update(&update1);
        mgr.apply_book_update(&update2);

        let snap1 = mgr.get_snapshot("asset1").unwrap();
        let snap2 = mgr.get_snapshot("asset2").unwrap();
        assert_eq!(snap1.best_bid.as_ref().unwrap().price, "0.55");
        assert_eq!(snap2.best_bid.as_ref().unwrap().price, "0.30");
    }

    #[test]
    fn get_snapshot_nonexistent_returns_none() {
        let mgr = OrderBookManager::new();
        assert!(mgr.get_snapshot("nonexistent").is_none());
    }

    #[test]
    fn hash_tracking() {
        let mgr = OrderBookManager::new();
        let update = make_book_update("asset1", &[("0.55", "100")], &[]);
        mgr.apply_book_update(&update);
        assert_eq!(mgr.get_hash("asset1"), Some("hash1".to_string()));

        let delta = PriceChangeBatchEntry {
            asset_id: "asset1".to_string(),
            price: "0.54".to_string(),
            size: Some("50".to_string()),
            side: Side::Buy,
            hash: Some("hash2".to_string()),
            best_bid: None,
            best_ask: None,
        };
        mgr.apply_price_change(&delta, 1700000001000);
        assert_eq!(mgr.get_hash("asset1"), Some("hash2".to_string()));
    }

    #[test]
    fn clear_all_empties_order_book() {
        let mgr = OrderBookManager::new();
        let update1 = make_book_update("asset1", &[("0.55", "100")], &[("0.56", "150")]);
        let update2 = make_book_update("asset2", &[("0.30", "500")], &[("0.35", "600")]);
        mgr.apply_book_update(&update1);
        mgr.apply_book_update(&update2);
        assert_eq!(mgr.asset_count(), 2);

        mgr.clear_all();

        assert_eq!(mgr.asset_count(), 0);
        assert!(mgr.get_snapshot("asset1").is_none());
        assert!(mgr.get_snapshot("asset2").is_none());
    }

    #[test]
    fn concurrent_read_write() {
        use std::sync::Arc;
        use std::thread;

        let mgr = Arc::new(OrderBookManager::new());
        let update = make_book_update("asset1", &[("0.55", "100")], &[("0.56", "150")]);
        mgr.apply_book_update(&update);

        let mgr_reader = Arc::clone(&mgr);
        let reader = thread::spawn(move || {
            for _ in 0..100 {
                let _ = mgr_reader.get_snapshot("asset1");
            }
        });

        let mgr_writer = Arc::clone(&mgr);
        let writer = thread::spawn(move || {
            for i in 0..100 {
                let delta = PriceChangeBatchEntry {
                    asset_id: "asset1".to_string(),
                    price: format!("0.{}", 40 + (i % 20)),
                    size: Some(format!("{}", 10 + i)),
                    side: Side::Buy,
                    hash: None,
                    best_bid: None,
                    best_ask: None,
                };
                mgr_writer.apply_price_change(&delta, 1700000000000 + i);
            }
        });

        reader.join().unwrap();
        writer.join().unwrap();
        // Should not panic — RwLock provides safety
        assert!(mgr.get_snapshot("asset1").is_some());
    }
}
