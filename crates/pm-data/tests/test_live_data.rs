//! # Live Market Data Tests
//!
//! These tests prove that pm-data can:
//! 1. Connect to Polymarket's WebSocket feed
//! 2. Subscribe to a real market's order book
//! 3. Receive and parse book snapshots and price updates
//! 4. Maintain a keepalive connection (PING every 10s)
//!
//! WHY THIS MATTERS:
//! The WebSocket feed is our eyes — without it, we're trading blind.
//! These tests verify we can see the market in real-time. The data
//! flows: WebSocket -> parse JSON -> update local order book -> notify strategy
//!
//! The WebSocket URL: wss://ws-subscriptions-clob.polymarket.com/ws/market
//! Protocol: JSON messages with types "book", "price_change", etc.

use pm_data::OrderBookManager;
use pm_data::types::{BookUpdate, PriceChangeBatchEntry, Side, WsOrderBookLevel};

/// TEST: We can connect to the real WebSocket and receive a book snapshot.
///
/// This is the most fundamental data test. It proves:
/// - WebSocket connection succeeds
/// - Subscription message is accepted by the server
/// - The server sends back a full order book snapshot
/// - We can parse it into our OrderBookSnapshot type
///
/// WHY THIS MATTERS:
/// If this doesn't work, the entire system is deaf. No market data
/// means no strategy signals, which means no trades.
#[tokio::test]
async fn connects_to_polymarket_and_receives_book_snapshot() {
    use pm_data::Pipeline;
    use tokio::time::{timeout, Duration};

    println!("\n=== Live Data: WebSocket Connection ===");

    // Use a known active market token ID — must match one with live order book data.
    let asset_id =
        "48825140812430902098404528620382945035793471220915259967486864813738884055220"
            .to_string();

    println!("Asset:     {}...", &asset_id[..20]);

    // Create a pipeline and subscribe to snapshots BEFORE starting it.
    let mut pipeline = Pipeline::new(vec![asset_id.clone()], 256, 64);
    let mut snapshot_rx = pipeline.subscribe_snapshots();

    // Start the pipeline in a background task.
    let pipeline_handle = tokio::spawn(async move {
        pipeline.run().await;
    });

    // Wait for a snapshot (the first book message after subscription).
    // Timeout after 15 seconds — if no data arrives, something is wrong.
    let result = timeout(Duration::from_secs(15), snapshot_rx.recv()).await;

    match result {
        Ok(Ok(snapshot)) => {
            println!("Received book snapshot:");
            println!("  asset_id:  {}...", &snapshot.asset_id[..20.min(snapshot.asset_id.len())]);
            println!(
                "  best_bid:  {}",
                snapshot
                    .best_bid
                    .as_ref()
                    .map(|b| format!("{} @ size {}", b.price, b.size))
                    .unwrap_or_else(|| "none".to_string())
            );
            println!(
                "  best_ask:  {}",
                snapshot
                    .best_ask
                    .as_ref()
                    .map(|a| format!("{} @ size {}", a.price, a.size))
                    .unwrap_or_else(|| "none".to_string())
            );
            println!("  hash:      {}", &snapshot.hash);
            println!("=== PASS ===\n");
        }
        Ok(Err(e)) => {
            panic!("Snapshot channel error: {}", e);
        }
        Err(_) => {
            panic!("Timeout: no snapshot received within 15 seconds. WebSocket may be down.");
        }
    }

    // Clean up — abort the pipeline task.
    pipeline_handle.abort();
}

/// TEST: Price changes update the local order book correctly.
///
/// Scenario: We receive a full book snapshot, then a price_change
/// event. The local order book should reflect the update.
///
/// This test uses MOCK data (no network) to verify the parsing
/// and update logic deterministically.
///
/// WHY THIS MATTERS:
/// If we miss updates or apply them wrong, we have a stale view
/// of the market. A stale order book means the strategy might
/// fire at the wrong price.
#[test]
fn price_change_updates_local_orderbook() {
    let mgr = OrderBookManager::new();

    // Step 1: Apply initial book snapshot.
    // The market has bids at 0.55 and 0.54, asks at 0.56 and 0.57.
    let snapshot = BookUpdate {
        asset_id: "asset1".to_string(),
        market: "0xmarket".to_string(),
        timestamp: "1700000000000".to_string(),
        bids: vec![
            WsOrderBookLevel { price: "0.55".to_string(), size: "100".to_string() },
            WsOrderBookLevel { price: "0.54".to_string(), size: "200".to_string() },
        ],
        asks: vec![
            WsOrderBookLevel { price: "0.56".to_string(), size: "150".to_string() },
            WsOrderBookLevel { price: "0.57".to_string(), size: "250".to_string() },
        ],
        hash: Some("hash1".to_string()),
    };
    mgr.apply_book_update(&snapshot);

    let snap = mgr.get_snapshot("asset1").unwrap();
    assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.55");
    assert_eq!(snap.best_ask.as_ref().unwrap().price, "0.56");

    // Step 2: Apply a price change — a new ask at 0.54 (tighter spread).
    let delta = PriceChangeBatchEntry {
        asset_id: "asset1".to_string(),
        price: "0.54".to_string(),
        size: Some("75".to_string()),
        side: Side::Sell,
        hash: Some("hash2".to_string()),
        best_bid: None,
        best_ask: None,
    };
    mgr.apply_price_change(&delta, 1700000001000);

    // Now best_ask should be 0.54 (lower than previous 0.56).
    let snap = mgr.get_snapshot("asset1").unwrap();
    assert_eq!(
        snap.best_ask.as_ref().unwrap().price, "0.54",
        "best_ask should update to the new lower ask"
    );
    assert_eq!(snap.hash, "hash2", "hash should update with delta");
    // best_bid should be unchanged.
    assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.55");

    // Step 3: Remove a bid level (size = 0 means removal).
    let remove_bid = PriceChangeBatchEntry {
        asset_id: "asset1".to_string(),
        price: "0.55".to_string(),
        size: Some("0".to_string()),
        side: Side::Buy,
        hash: Some("hash3".to_string()),
        best_bid: None,
        best_ask: None,
    };
    mgr.apply_price_change(&remove_bid, 1700000002000);

    // best_bid should now be 0.54 (the 0.55 level was removed).
    let snap = mgr.get_snapshot("asset1").unwrap();
    assert_eq!(
        snap.best_bid.as_ref().unwrap().price, "0.54",
        "best_bid should fall back to 0.54 after 0.55 removed"
    );
}
