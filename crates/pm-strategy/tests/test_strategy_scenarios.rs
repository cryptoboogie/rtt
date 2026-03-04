//! # Strategy Scenario Tests
//!
//! These tests prove that the strategy layer correctly decides
//! WHEN to trade based on market conditions.
//!
//! The system currently supports ThresholdStrategy:
//!   "Buy when best_ask drops to X or below"
//!   "Sell when best_bid rises to X or above"
//!
//! WHY THIS MATTERS:
//! A false positive (fires when it shouldn't) = unwanted trade, lost money.
//! A false negative (doesn't fire when it should) = missed opportunity.
//! These tests cover both cases explicitly.

use pm_strategy::strategy::Strategy;
use pm_strategy::threshold::ThresholdStrategy;
use rtt_core::trigger::{OrderBookSnapshot, OrderType, PriceLevel, Side, TriggerMessage};

/// Helper: build a snapshot with given bid/ask prices for a specific asset.
fn make_snapshot(asset_id: &str, bid_price: &str, ask_price: &str) -> OrderBookSnapshot {
    OrderBookSnapshot {
        asset_id: asset_id.to_string(),
        best_bid: Some(PriceLevel {
            price: bid_price.to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: ask_price.to_string(),
            size: "100".to_string(),
        }),
        timestamp_ms: 1000,
        hash: "h".to_string(),
    }
}

/// TEST: Buy strategy fires when ask drops to threshold.
///
/// Scenario: Threshold = 0.45 (buy when ask <= 0.45)
///
/// Snapshot 1: ask = 0.50 -> no trigger (above threshold)
/// Snapshot 2: ask = 0.46 -> no trigger (still above)
/// Snapshot 3: ask = 0.45 -> TRIGGER FIRES (at threshold)
/// Snapshot 4: ask = 0.40 -> TRIGGER FIRES (below threshold)
///
/// This proves the strategy only fires at the right moment.
///
/// WHY THIS MATTERS:
/// The threshold boundary is critical. Off-by-one in the comparison
/// (< vs <=) would miss the exact threshold price. This test
/// catches that.
#[test]
fn buy_threshold_fires_when_ask_drops_to_target() {
    let mut strategy = ThresholdStrategy::new(
        "token_ABC".to_string(),
        Side::Buy,
        0.45, // Fire when ask <= 0.45
        "10".to_string(),
        OrderType::FOK,
    );

    // ask=0.50: above threshold, should NOT fire.
    let snap1 = make_snapshot("token_ABC", "0.44", "0.50");
    assert!(
        strategy.on_book_update(&snap1).is_none(),
        "ask=0.50 is above threshold 0.45 — should not fire"
    );

    // ask=0.46: still above, should NOT fire.
    let snap2 = make_snapshot("token_ABC", "0.44", "0.46");
    assert!(
        strategy.on_book_update(&snap2).is_none(),
        "ask=0.46 is above threshold 0.45 — should not fire"
    );

    // ask=0.45: exactly at threshold, SHOULD fire.
    let snap3 = make_snapshot("token_ABC", "0.44", "0.45");
    let trigger = strategy.on_book_update(&snap3);
    assert!(trigger.is_some(), "ask=0.45 is at threshold — should fire");
    let t = trigger.unwrap();
    assert_eq!(t.side, Side::Buy, "trigger side should be Buy");
    assert_eq!(t.price, "0.45", "trigger price should be the ask price");

    // ask=0.40: below threshold, SHOULD fire again.
    let snap4 = make_snapshot("token_ABC", "0.38", "0.40");
    let trigger = strategy.on_book_update(&snap4);
    assert!(trigger.is_some(), "ask=0.40 is below threshold — should fire");
    assert_eq!(trigger.unwrap().price, "0.40");
}

/// TEST: Strategy ignores snapshots for other assets.
///
/// Scenario: Strategy is configured for token "ABC".
/// A snapshot arrives for token "XYZ" with ask below threshold.
/// The strategy must NOT fire — wrong asset.
///
/// WHY THIS MATTERS:
/// In production we might monitor 10 markets but only trade 1.
/// Firing on the wrong market would buy the wrong thing.
#[test]
fn strategy_ignores_snapshots_for_other_assets() {
    let mut strategy = ThresholdStrategy::new(
        "token_ABC".to_string(),
        Side::Buy,
        0.45,
        "10".to_string(),
        OrderType::FOK,
    );

    // Snapshot for the WRONG asset, with ask below threshold.
    let snap_wrong = make_snapshot("token_XYZ", "0.30", "0.35");
    assert!(
        strategy.on_book_update(&snap_wrong).is_none(),
        "should not fire for wrong asset, even if price is below threshold"
    );

    // Snapshot for the CORRECT asset — should fire.
    let snap_right = make_snapshot("token_ABC", "0.40", "0.42");
    assert!(
        strategy.on_book_update(&snap_right).is_some(),
        "should fire for correct asset when below threshold"
    );
}

/// TEST: Full flow — snapshots through strategy runner produce triggers.
///
/// This tests the StrategyRunner (the component that connects
/// the data pipeline to the strategy to the executor):
///
///   mpsc channel -> StrategyRunner -> strategy.on_book_update() -> trigger out
///
/// We send 3 snapshots: 2 that don't trigger, 1 that does.
/// Verify exactly 1 trigger comes out with the correct parameters.
///
/// WHY THIS MATTERS:
/// This is the integration point between pm-data and pm-strategy.
/// If the runner drops snapshots, misroutes triggers, or corrupts
/// the trigger parameters, trades will be wrong.
#[tokio::test]
async fn strategy_runner_produces_trigger_from_snapshot_stream() {
    use pm_strategy::runner::StrategyRunner;
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

    // Create channels.
    let (snapshot_tx, snapshot_rx) = mpsc::channel(16);
    let (trigger_tx, mut trigger_rx) = mpsc::channel(16);

    // Create a threshold strategy: fire when ask <= 0.45 for token_ABC.
    let strategy: Box<dyn Strategy> = Box::new(ThresholdStrategy::new(
        "token_ABC".to_string(),
        Side::Buy,
        0.45,
        "10".to_string(),
        OrderType::FOK,
    ));

    // Start the strategy runner in a background task.
    let mut runner = StrategyRunner::new(strategy, snapshot_rx, trigger_tx);
    let runner_handle = tokio::spawn(async move { runner.run().await });

    // Send 3 snapshots: 2 above threshold, 1 at threshold.
    let snap1 = make_snapshot("token_ABC", "0.44", "0.50"); // above -> no fire
    let snap2 = make_snapshot("token_ABC", "0.44", "0.48"); // above -> no fire
    let snap3 = make_snapshot("token_ABC", "0.44", "0.45"); // at threshold -> FIRE

    snapshot_tx.send(snap1).await.unwrap();
    snapshot_tx.send(snap2).await.unwrap();
    snapshot_tx.send(snap3).await.unwrap();

    // Expect exactly 1 trigger.
    let trigger = timeout(Duration::from_secs(2), trigger_rx.recv())
        .await
        .expect("timeout waiting for trigger")
        .expect("trigger channel closed");

    assert_eq!(trigger.side, Side::Buy);
    assert_eq!(trigger.price, "0.45");
    assert_eq!(trigger.size, "10");
    assert_eq!(trigger.order_type, OrderType::FOK);
    assert_eq!(trigger.token_id, "token_ABC");

    // Clean up.
    drop(snapshot_tx);
    let _ = runner_handle.await;
}

/// TEST: Strategy converts trigger with correct order parameters.
///
/// When the strategy fires, the TriggerMessage must contain:
/// - The correct token_id (what to trade)
/// - The correct side (Buy/Sell)
/// - The correct price (what the market is at)
/// - The correct size (how much to trade)
/// - The correct order_type (FOK/GTC/etc.)
///
/// All of these come from the strategy configuration, not the snapshot.
/// The snapshot only determines WHETHER to fire, not the order details
/// (except the price, which is the current market price at trigger time).
///
/// WHY THIS MATTERS:
/// Wrong token_id = trading the wrong asset.
/// Wrong side = buying when you meant to sell.
/// Wrong size = trading 10x too much.
/// Any of these is a costly bug.
#[test]
fn trigger_contains_correct_order_parameters() {
    // Configure: sell token_XYZ when bid >= 0.70, size=50, GTC order.
    let mut strategy = ThresholdStrategy::new(
        "token_XYZ".to_string(),
        Side::Sell,
        0.70,
        "50".to_string(),
        OrderType::GTC,
    );

    // Send a snapshot that fires the sell trigger.
    let snap = make_snapshot("token_XYZ", "0.72", "0.75");
    let trigger = strategy
        .on_book_update(&snap)
        .expect("should fire: bid 0.72 >= threshold 0.70");

    // Verify every field in the trigger.
    assert_eq!(trigger.token_id, "token_XYZ", "token_id from config");
    assert_eq!(trigger.side, Side::Sell, "side from config");
    assert_eq!(trigger.price, "0.72", "price from market (best_bid)");
    assert_eq!(trigger.size, "50", "size from config");
    assert_eq!(trigger.order_type, OrderType::GTC, "order_type from config");
    assert!(trigger.trigger_id > 0, "trigger_id should be assigned");
    assert!(trigger.timestamp_ns > 0, "timestamp should be set");
}
