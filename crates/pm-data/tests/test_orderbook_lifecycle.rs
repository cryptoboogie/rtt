//! # Order Book Lifecycle Tests
//!
//! These tests prove that the OrderBookManager correctly maintains
//! an in-memory mirror of the exchange's order book through a
//! realistic sequence of events:
//!
//!   1. Initial book snapshot (full state)
//!   2. Price changes (incremental updates)
//!   3. Multiple assets tracked simultaneously
//!   4. Thread-safe concurrent reads during writes
//!
//! WHY THIS MATTERS:
//! The order book is the strategy's input. The strategy looks at
//! best_bid and best_ask to decide whether to trade. If the order
//! book is wrong, the strategy makes wrong decisions.

use pm_data::types::{BookUpdate, PriceChangeBatchEntry, Side, WsOrderBookLevel};
use pm_data::OrderBookManager;

/// TEST: Full lifecycle of an order book through multiple updates.
///
/// Simulates a realistic sequence:
/// 1. Book snapshot arrives: bid=0.55, ask=0.56
/// 2. Price change: new ask at 0.54 (spread tightened)
/// 3. Price change: bid size increases (more liquidity)
/// 4. Verify best_bid and best_ask are correct at each step
///
/// WHY THIS MATTERS:
/// This is the scenario that runs continuously in production.
/// Every few milliseconds, price changes arrive and the book
/// must be updated correctly. One bug here means every trade
/// decision is based on wrong data.
#[test]
fn orderbook_tracks_full_lifecycle_of_updates() {
    let mgr = OrderBookManager::new();

    // --- Step 1: Initial book snapshot ---
    // The exchange sends a full snapshot when we first subscribe.
    let initial = BookUpdate {
        asset_id: "asset1".to_string(),
        market: "0xmarket".to_string(),
        timestamp: "1700000000000".to_string(),
        bids: vec![
            WsOrderBookLevel {
                price: "0.55".to_string(),
                size: "100".to_string(),
            },
            WsOrderBookLevel {
                price: "0.54".to_string(),
                size: "200".to_string(),
            },
            WsOrderBookLevel {
                price: "0.53".to_string(),
                size: "300".to_string(),
            },
        ],
        asks: vec![
            WsOrderBookLevel {
                price: "0.56".to_string(),
                size: "150".to_string(),
            },
            WsOrderBookLevel {
                price: "0.57".to_string(),
                size: "250".to_string(),
            },
        ],
        hash: Some("hash_snap".to_string()),
    };
    mgr.apply_book_update(&initial);

    let snap = mgr.get_snapshot("asset1").unwrap();
    assert_eq!(
        snap.best_bid.as_ref().unwrap().price,
        "0.55",
        "step 1: best bid"
    );
    assert_eq!(
        snap.best_bid.as_ref().unwrap().size,
        "100",
        "step 1: bid size"
    );
    assert_eq!(
        snap.best_ask.as_ref().unwrap().price,
        "0.56",
        "step 1: best ask"
    );
    assert_eq!(
        snap.best_ask.as_ref().unwrap().size,
        "150",
        "step 1: ask size"
    );
    assert_eq!(
        mgr.bid_count("asset1"),
        3,
        "step 1: should have 3 bid levels"
    );
    assert_eq!(
        mgr.ask_count("asset1"),
        2,
        "step 1: should have 2 ask levels"
    );

    // --- Step 2: Price change — new tighter ask at 0.545 ---
    // Someone placed a lower ask, tightening the spread.
    let tighter_ask = PriceChangeBatchEntry {
        asset_id: "asset1".to_string(),
        price: "0.545".to_string(),
        size: Some("80".to_string()),
        side: Side::Sell,
        hash: Some("hash_pc1".to_string()),
        best_bid: None,
        best_ask: None,
    };
    mgr.apply_price_change(&tighter_ask, 1700000001000);

    let snap = mgr.get_snapshot("asset1").unwrap();
    assert_eq!(
        snap.best_ask.as_ref().unwrap().price,
        "0.545",
        "step 2: ask tightened"
    );
    assert_eq!(
        snap.best_bid.as_ref().unwrap().price,
        "0.55",
        "step 2: bid unchanged"
    );
    assert_eq!(mgr.ask_count("asset1"), 3, "step 2: now 3 ask levels");

    // --- Step 3: Price change — bid size increases (liquidity added) ---
    let more_bid = PriceChangeBatchEntry {
        asset_id: "asset1".to_string(),
        price: "0.55".to_string(),
        size: Some("500".to_string()),
        side: Side::Buy,
        hash: Some("hash_pc2".to_string()),
        best_bid: None,
        best_ask: None,
    };
    mgr.apply_price_change(&more_bid, 1700000002000);

    let snap = mgr.get_snapshot("asset1").unwrap();
    assert_eq!(
        snap.best_bid.as_ref().unwrap().size,
        "500",
        "step 3: bid size increased"
    );
    assert_eq!(snap.hash, "hash_pc2", "step 3: hash updated");

    // --- Step 4: Price change — top bid removed (size=0) ---
    let remove_top_bid = PriceChangeBatchEntry {
        asset_id: "asset1".to_string(),
        price: "0.55".to_string(),
        size: Some("0".to_string()),
        side: Side::Buy,
        hash: Some("hash_pc3".to_string()),
        best_bid: None,
        best_ask: None,
    };
    mgr.apply_price_change(&remove_top_bid, 1700000003000);

    let snap = mgr.get_snapshot("asset1").unwrap();
    assert_eq!(
        snap.best_bid.as_ref().unwrap().price,
        "0.54",
        "step 4: best bid dropped to 0.54"
    );
    assert_eq!(
        mgr.bid_count("asset1"),
        2,
        "step 4: only 2 bid levels remain"
    );

    // --- Step 5: Full snapshot replaces everything ---
    let new_snap = BookUpdate {
        asset_id: "asset1".to_string(),
        market: "0xmarket".to_string(),
        timestamp: "1700000010000".to_string(),
        bids: vec![WsOrderBookLevel {
            price: "0.60".to_string(),
            size: "1000".to_string(),
        }],
        asks: vec![WsOrderBookLevel {
            price: "0.61".to_string(),
            size: "900".to_string(),
        }],
        hash: Some("hash_new".to_string()),
    };
    mgr.apply_book_update(&new_snap);

    let snap = mgr.get_snapshot("asset1").unwrap();
    assert_eq!(
        snap.best_bid.as_ref().unwrap().price,
        "0.60",
        "step 5: full reset"
    );
    assert_eq!(
        snap.best_ask.as_ref().unwrap().price,
        "0.61",
        "step 5: full reset"
    );
    assert_eq!(mgr.bid_count("asset1"), 1, "step 5: only 1 bid after reset");
    assert_eq!(mgr.ask_count("asset1"), 1, "step 5: only 1 ask after reset");
}

/// TEST: Multiple assets are tracked independently.
///
/// We might monitor 3 markets simultaneously. Updates to one
/// must not affect the others.
///
/// WHY THIS MATTERS:
/// In production we might watch many markets but trade only one.
/// Cross-contamination between books would mean wrong prices for
/// every market except the first.
#[test]
fn multiple_assets_tracked_independently() {
    let mgr = OrderBookManager::new();

    // Set up two independent order books.
    let book_a = BookUpdate {
        asset_id: "market_A".to_string(),
        market: "0xA".to_string(),
        timestamp: "1000".to_string(),
        bids: vec![WsOrderBookLevel {
            price: "0.40".to_string(),
            size: "100".to_string(),
        }],
        asks: vec![WsOrderBookLevel {
            price: "0.45".to_string(),
            size: "200".to_string(),
        }],
        hash: Some("hashA".to_string()),
    };
    let book_b = BookUpdate {
        asset_id: "market_B".to_string(),
        market: "0xB".to_string(),
        timestamp: "1000".to_string(),
        bids: vec![WsOrderBookLevel {
            price: "0.70".to_string(),
            size: "500".to_string(),
        }],
        asks: vec![WsOrderBookLevel {
            price: "0.75".to_string(),
            size: "600".to_string(),
        }],
        hash: Some("hashB".to_string()),
    };

    mgr.apply_book_update(&book_a);
    mgr.apply_book_update(&book_b);

    // Verify they are independent.
    let snap_a = mgr.get_snapshot("market_A").unwrap();
    let snap_b = mgr.get_snapshot("market_B").unwrap();
    assert_eq!(snap_a.best_bid.as_ref().unwrap().price, "0.40");
    assert_eq!(snap_b.best_bid.as_ref().unwrap().price, "0.70");

    // Update market_A only.
    let delta = PriceChangeBatchEntry {
        asset_id: "market_A".to_string(),
        price: "0.42".to_string(),
        size: Some("150".to_string()),
        side: Side::Buy,
        hash: Some("hashA2".to_string()),
        best_bid: None,
        best_ask: None,
    };
    mgr.apply_price_change(&delta, 2000);

    // market_A should be updated.
    let snap_a = mgr.get_snapshot("market_A").unwrap();
    assert_eq!(
        mgr.bid_count("market_A"),
        2,
        "market_A should have 2 bids now"
    );
    assert_eq!(snap_a.hash, "hashA2");

    // market_B should be untouched.
    let snap_b = mgr.get_snapshot("market_B").unwrap();
    assert_eq!(
        snap_b.best_bid.as_ref().unwrap().price,
        "0.70",
        "market_B should be unchanged"
    );
    assert_eq!(snap_b.hash, "hashB", "market_B hash should be unchanged");

    // Nonexistent market returns None.
    assert!(mgr.get_snapshot("market_C").is_none());
}

/// TEST: Order book can be read while being written to.
///
/// The WebSocket thread writes updates. The strategy thread reads
/// snapshots. They must not block each other or corrupt data.
///
/// WHY THIS MATTERS:
/// If reading blocks writing, we miss WebSocket updates.
/// If writing blocks reading, the strategy stalls.
/// Both are unacceptable for real-time trading.
#[test]
fn concurrent_read_during_write_is_safe() {
    use std::sync::Arc;
    use std::thread;

    let mgr = Arc::new(OrderBookManager::new());

    // Set up initial book.
    let initial = BookUpdate {
        asset_id: "asset1".to_string(),
        market: "0xmarket".to_string(),
        timestamp: "1000".to_string(),
        bids: vec![WsOrderBookLevel {
            price: "0.50".to_string(),
            size: "100".to_string(),
        }],
        asks: vec![WsOrderBookLevel {
            price: "0.55".to_string(),
            size: "100".to_string(),
        }],
        hash: Some("h0".to_string()),
    };
    mgr.apply_book_update(&initial);

    // Spawn a writer thread: rapidly applies 200 price changes.
    let mgr_writer = Arc::clone(&mgr);
    let writer = thread::spawn(move || {
        for i in 0..200u64 {
            let delta = PriceChangeBatchEntry {
                asset_id: "asset1".to_string(),
                price: format!("0.{}", 40 + (i % 20)),
                size: Some(format!("{}", 50 + i)),
                side: Side::Buy,
                hash: None,
                best_bid: None,
                best_ask: None,
            };
            mgr_writer.apply_price_change(&delta, 1000 + i);
        }
    });

    // Spawn a reader thread: rapidly reads snapshots.
    let mgr_reader = Arc::clone(&mgr);
    let reader = thread::spawn(move || {
        let mut read_count = 0u64;
        for _ in 0..200 {
            if let Some(snap) = mgr_reader.get_snapshot("asset1") {
                // Every snapshot should have a valid best_bid.
                assert!(snap.best_bid.is_some(), "bid should always be present");
                read_count += 1;
            }
        }
        read_count
    });

    // Both threads should complete without panics or deadlocks.
    writer.join().expect("writer thread panicked");
    let reads = reader.join().expect("reader thread panicked");

    // Verify the book is still consistent after concurrent access.
    let final_snap = mgr.get_snapshot("asset1").unwrap();
    assert!(final_snap.best_bid.is_some(), "book should still have bids");
    assert!(reads > 0, "reader should have read at least one snapshot");
}
