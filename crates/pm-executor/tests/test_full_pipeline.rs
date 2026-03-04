//! # Full Pipeline Integration Tests
//!
//! These tests prove the entire system works when all crates
//! are wired together:
//!
//!   WebSocket data -> Order book -> Strategy -> Trigger -> Executor -> Order on wire
//!
//! This is the "system test" layer. Individual crates have their own
//! tests. This file tests that they COMPOSE correctly.
//!
//! The architecture:
//! +-------------+    +--------------+    +------------+    +--------------+
//! |   pm-data   | -> | pm-strategy  | -> |  bridge    | -> |   rtt-core   |
//! | (WebSocket) |    | (decisions)  |    | (channels) |    | (execution)  |
//! +-------------+    +--------------+    +------------+    +--------------+
//!
//! Channel types:
//!   broadcast<OrderBookSnapshot> -> mpsc<OrderBookSnapshot> -> mpsc<TriggerMessage> -> crossbeam<TriggerMessage>
//!
//! WHY THIS MATTERS:
//! Each crate works in isolation (unit tests prove that).
//! But do they work TOGETHER? Channel type mismatches, timing issues,
//! shutdown races — these only show up at integration level.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use rtt_core::trigger::{OrderBookSnapshot, OrderType, PriceLevel, Side};

/// Helper: build a test snapshot.
fn make_snapshot(asset_id: &str, bid: &str, ask: &str) -> OrderBookSnapshot {
    OrderBookSnapshot {
        asset_id: asset_id.to_string(),
        best_bid: Some(PriceLevel {
            price: bid.to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: ask.to_string(),
            size: "100".to_string(),
        }),
        timestamp_ms: 1000,
        hash: "h".to_string(),
    }
}

/// TEST: A mock snapshot flows through the entire channel pipeline.
///
/// No network needed. We manually inject a snapshot at the start
/// and verify a trigger comes out the other end.
///
///   inject snapshot -> broadcast -> mpsc bridge -> strategy runner
///   -> trigger out -> mpsc -> crossbeam bridge -> trigger received
///
/// WHY THIS MATTERS:
/// This proves the channel wiring is correct. Each bridge
/// (broadcast->mpsc, mpsc->crossbeam) correctly forwards messages
/// and the strategy processes them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn snapshot_flows_through_entire_channel_pipeline_to_trigger() {
    use tokio::sync::{broadcast, mpsc, watch};

    // Set up the full channel pipeline:
    //   broadcast -> [bridge] -> mpsc -> StrategyRunner -> mpsc -> [bridge] -> crossbeam

    // 1. broadcast<OrderBookSnapshot> (what Pipeline produces)
    let (broadcast_tx, mut broadcast_rx) = broadcast::channel::<OrderBookSnapshot>(16);

    // 2. mpsc<OrderBookSnapshot> (what StrategyRunner consumes)
    let (snapshot_mpsc_tx, snapshot_mpsc_rx) = mpsc::channel::<OrderBookSnapshot>(16);

    // 3. mpsc<TriggerMessage> (what StrategyRunner produces)
    let (trigger_mpsc_tx, mut trigger_mpsc_rx) = mpsc::channel(16);

    // 4. crossbeam<TriggerMessage> (what the executor consumes)
    let (crossbeam_tx, crossbeam_rx) = crossbeam_channel::bounded(16);

    // Shutdown signal.
    let (shutdown_tx, _) = watch::channel(false);

    // --- Start bridge: broadcast -> mpsc ---
    let mut shutdown_rx1 = shutdown_tx.subscribe();
    let bridge1_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = broadcast_rx.recv() => {
                    match result {
                        Ok(snap) => { if snapshot_mpsc_tx.send(snap).await.is_err() { break; } }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(_) => continue,
                    }
                }
                _ = shutdown_rx1.changed() => { if *shutdown_rx1.borrow() { break; } }
            }
        }
    });

    // --- Start strategy runner ---
    let strategy: Box<dyn pm_strategy::strategy::Strategy> =
        Box::new(pm_strategy::threshold::ThresholdStrategy::new(
            "asset1".to_string(),
            Side::Buy,
            0.45,
            "10".to_string(),
            OrderType::FOK,
        ));
    let mut runner =
        pm_strategy::runner::StrategyRunner::new(strategy, snapshot_mpsc_rx, trigger_mpsc_tx);
    let runner_handle = tokio::spawn(async move { runner.run().await });

    // --- Start bridge: mpsc -> crossbeam ---
    let mut shutdown_rx2 = shutdown_tx.subscribe();
    let bridge2_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = trigger_mpsc_rx.recv() => {
                    match msg {
                        Some(mut trigger) => {
                            trigger.timestamp_ns = rtt_core::clock::now_ns();
                            if crossbeam_tx.send(trigger).is_err() { break; }
                        }
                        None => break,
                    }
                }
                _ = shutdown_rx2.changed() => { if *shutdown_rx2.borrow() { break; } }
            }
        }
    });

    // --- Inject test snapshots ---
    // Snapshot 1: ask=0.50, above threshold 0.45 -> no trigger.
    broadcast_tx.send(make_snapshot("asset1", "0.44", "0.50")).unwrap();

    // Snapshot 2: ask=0.45, at threshold -> should trigger!
    broadcast_tx.send(make_snapshot("asset1", "0.44", "0.45")).unwrap();

    // --- Receive the trigger from the crossbeam end ---
    let trigger = tokio::task::spawn_blocking(move || {
        crossbeam_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("timeout waiting for trigger at crossbeam end")
    })
    .await
    .unwrap();

    assert_eq!(trigger.side, Side::Buy, "trigger side");
    assert_eq!(trigger.price, "0.45", "trigger price");
    assert_eq!(trigger.token_id, "asset1", "trigger token_id");
    assert_eq!(trigger.order_type, OrderType::FOK, "trigger order_type");
    // timestamp_ns is set by the bridge via clock::now_ns(). On the very first
    // call it may be 0 (epoch just initialized), so we don't assert > 0.

    // --- Shutdown ---
    let _ = shutdown_tx.send(true);
    drop(broadcast_tx);
    let _ = bridge1_handle.await;
    let _ = runner_handle.await;
    let _ = bridge2_handle.await;
}

/// TEST: Configuration loads and all components can be constructed.
///
/// Load config.toml, build the strategy from it, verify all
/// channel types are compatible.
///
/// WHY THIS MATTERS:
/// A typo in config.toml or a type mismatch between crates
/// would prevent the system from starting. This catches that.
#[test]
fn config_loads_and_all_components_construct() {
    // Parse the example config.toml shipped with the project.
    let toml_str = include_str!("../../../config.toml");
    let config: toml::Value = toml::from_str(toml_str).expect("config.toml should parse");

    // Verify all required sections exist.
    assert!(config.get("credentials").is_some(), "missing [credentials]");
    assert!(config.get("connection").is_some(), "missing [connection]");
    assert!(config.get("websocket").is_some(), "missing [websocket]");
    assert!(config.get("strategy").is_some(), "missing [strategy]");
    assert!(config.get("execution").is_some(), "missing [execution]");
    assert!(config.get("logging").is_some(), "missing [logging]");

    // Build the strategy from the config's strategy section.
    let strategy_section = toml::to_string(config.get("strategy").unwrap()).unwrap();
    let strategy_config: pm_strategy::config::StrategyConfig =
        toml::from_str(&strategy_section).expect("strategy config should parse");
    let strategy = strategy_config
        .build_strategy()
        .expect("strategy should build from config");
    assert_eq!(strategy.name(), "threshold");

    // Verify channel types are compatible by constructing them.
    // This is a compile-time check that the generic parameters match.
    let (_broadcast_tx, _broadcast_rx) =
        tokio::sync::broadcast::channel::<OrderBookSnapshot>(16);
    let (_mpsc_snap_tx, _mpsc_snap_rx) =
        tokio::sync::mpsc::channel::<OrderBookSnapshot>(16);
    let (_mpsc_trig_tx, _mpsc_trig_rx) =
        tokio::sync::mpsc::channel::<rtt_core::trigger::TriggerMessage>(16);
    let (_crossbeam_tx, _crossbeam_rx) =
        crossbeam_channel::bounded::<rtt_core::trigger::TriggerMessage>(16);
}

/// TEST: Graceful shutdown — no panics, no hangs.
///
/// Start all components, send shutdown signal, verify everything
/// stops within 5 seconds without panicking.
///
/// WHY THIS MATTERS:
/// In production, Ctrl+C must stop cleanly. If a thread hangs
/// or a channel deadlocks, the process becomes a zombie that
/// might still have open orders or connections.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_shutdown_completes_within_timeout() {
    use tokio::sync::{broadcast, mpsc, watch};
    use tokio::time::{timeout, Duration};

    // Set up channels and components.
    let (broadcast_tx, broadcast_rx) = broadcast::channel::<OrderBookSnapshot>(16);
    let (snapshot_tx, snapshot_rx) = mpsc::channel(16);
    let (trigger_tx, trigger_rx) = mpsc::channel(16);
    let (crossbeam_tx, _crossbeam_rx) = crossbeam_channel::bounded(16);
    let (shutdown_tx, _) = watch::channel(false);

    // Start bridge 1.
    let mut shutdown_rx1 = shutdown_tx.subscribe();
    let bridge1 = tokio::spawn(async move {
        let mut rx = broadcast_rx;
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(snap) => { let _ = snapshot_tx.send(snap).await; }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(_) => continue,
                    }
                }
                _ = shutdown_rx1.changed() => { if *shutdown_rx1.borrow() { break; } }
            }
        }
    });

    // Start strategy runner.
    let strategy: Box<dyn pm_strategy::strategy::Strategy> =
        Box::new(pm_strategy::threshold::ThresholdStrategy::new(
            "asset1".to_string(), Side::Buy, 0.45, "10".to_string(), OrderType::FOK,
        ));
    let mut runner = pm_strategy::runner::StrategyRunner::new(strategy, snapshot_rx, trigger_tx);
    let runner_task = tokio::spawn(async move { runner.run().await });

    // Start bridge 2.
    let mut shutdown_rx2 = shutdown_tx.subscribe();
    let bridge2 = tokio::spawn(async move {
        let mut rx = trigger_rx;
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(t) => { let _ = crossbeam_tx.send(t); }
                        None => break,
                    }
                }
                _ = shutdown_rx2.changed() => { if *shutdown_rx2.borrow() { break; } }
            }
        }
    });

    // Let components run briefly, then shut down.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = shutdown_tx.send(true);
    drop(broadcast_tx); // close the broadcast channel to unblock bridge1

    // All components should stop within 5 seconds.
    let result = timeout(Duration::from_secs(5), async {
        let _ = bridge1.await;
        let _ = runner_task.await;
        let _ = bridge2.await;
    })
    .await;

    assert!(
        result.is_ok(),
        "all components should stop within 5 seconds of shutdown signal"
    );
}

/// TEST: Snapshot → Strategy → Trigger → Execution loop (dry run).
///
/// This tests the COMPLETE pipeline including the execution loop.
/// A mock snapshot triggers the strategy, which produces a trigger
/// that flows through to the execution thread (dry-run mode).
///
/// WHY THIS MATTERS:
/// Session 6 wires the execution loop. This proves that triggers
/// produced by the strategy actually reach the execution thread
/// and are processed in dry-run mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trigger_reaches_dry_run_execution_loop() {
    use tokio::sync::{broadcast, mpsc, watch};
    use tokio::time::{timeout, Duration};

    // Track how many triggers the execution "loop" processes
    let trigger_count = Arc::new(AtomicU64::new(0));
    let trigger_count_clone = trigger_count.clone();

    // Full channel pipeline
    let (broadcast_tx, mut broadcast_rx) = broadcast::channel::<OrderBookSnapshot>(16);
    let (snapshot_mpsc_tx, snapshot_mpsc_rx) = mpsc::channel::<OrderBookSnapshot>(16);
    let (trigger_mpsc_tx, mut trigger_mpsc_rx) = mpsc::channel(16);
    let (crossbeam_tx, crossbeam_rx) = crossbeam_channel::bounded(16);
    let (shutdown_tx, _) = watch::channel(false);

    // Bridge: broadcast -> mpsc
    let mut shutdown_rx1 = shutdown_tx.subscribe();
    let bridge1 = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = broadcast_rx.recv() => {
                    match result {
                        Ok(snap) => { if snapshot_mpsc_tx.send(snap).await.is_err() { break; } }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(_) => continue,
                    }
                }
                _ = shutdown_rx1.changed() => { if *shutdown_rx1.borrow() { break; } }
            }
        }
    });

    // Strategy runner
    let strategy: Box<dyn pm_strategy::strategy::Strategy> =
        Box::new(pm_strategy::threshold::ThresholdStrategy::new(
            "asset1".to_string(),
            Side::Buy,
            0.45,
            "10".to_string(),
            OrderType::FOK,
        ));
    let mut runner =
        pm_strategy::runner::StrategyRunner::new(strategy, snapshot_mpsc_rx, trigger_mpsc_tx);
    let runner_handle = tokio::spawn(async move { runner.run().await });

    // Bridge: mpsc -> crossbeam
    let mut shutdown_rx2 = shutdown_tx.subscribe();
    let bridge2 = tokio::spawn(async move {
        loop {
            tokio::select! {
                msg = trigger_mpsc_rx.recv() => {
                    match msg {
                        Some(mut trigger) => {
                            trigger.timestamp_ns = rtt_core::clock::now_ns();
                            if crossbeam_tx.send(trigger).is_err() { break; }
                        }
                        None => break,
                    }
                }
                _ = shutdown_rx2.changed() => { if *shutdown_rx2.borrow() { break; } }
            }
        }
    });

    // Execution thread (dry-run) — replicates the pattern from execution::run_execution_loop
    let exec_shutdown = Arc::new(AtomicBool::new(false));
    let exec_shutdown_clone = exec_shutdown.clone();
    let exec_handle = std::thread::spawn(move || {
        while !exec_shutdown_clone.load(Ordering::Relaxed) {
            match crossbeam_rx.try_recv() {
                Ok(trigger) => {
                    // Dry-run: just count the trigger
                    assert_eq!(trigger.side, Side::Buy);
                    assert_eq!(trigger.price, "0.45");
                    trigger_count_clone.fetch_add(1, Ordering::Relaxed);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    std::thread::yield_now();
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => break,
            }
        }
    });

    // Inject snapshots: first above threshold (no fire), then at threshold (fire!)
    broadcast_tx
        .send(make_snapshot("asset1", "0.44", "0.50"))
        .unwrap();
    broadcast_tx
        .send(make_snapshot("asset1", "0.44", "0.45"))
        .unwrap();

    // Wait for the execution thread to process the trigger
    let result = timeout(Duration::from_secs(5), async {
        loop {
            if trigger_count.load(Ordering::Relaxed) >= 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;

    assert!(result.is_ok(), "execution loop should process trigger");
    assert_eq!(trigger_count.load(Ordering::Relaxed), 1);

    // Shutdown
    let _ = shutdown_tx.send(true);
    exec_shutdown.store(true, Ordering::Relaxed);
    drop(broadcast_tx);
    let _ = bridge1.await;
    let _ = runner_handle.await;
    let _ = bridge2.await;
    let _ = exec_handle.join();
}

/// TEST: Full pipeline with real WebSocket + real data (dry run).
///
/// This is the ultimate integration test. It starts the real
/// WebSocket connection, receives real market data, runs the
/// strategy, and verifies triggers are produced (dry run — no
/// actual orders sent).
///
/// Run with: cargo test -p pm-executor --test test_full_pipeline full_pipeline_live -- --ignored --nocapture
///
/// WHY THIS MATTERS:
/// This is as close to production as we can get without spending
/// money. If this passes, we have high confidence the system works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_pipeline_live_dry_run() {
    use tokio::sync::{mpsc, watch};
    use tokio::time::{timeout, Duration};

    println!("\n=== Full Pipeline: Live Dry Run ===");

    // Use a known active market.
    let asset_id =
        "48825140812430902098404528620382945035793471220915259967486864813738884055220"
            .to_string();
    println!("Asset: {}...", &asset_id[..20]);

    // Create the data pipeline.
    let mut pipeline = pm_data::Pipeline::new(
        vec![asset_id.clone()],
        1024,
        256,
    );
    let snapshot_rx = pipeline.subscribe_snapshots();

    // Start WebSocket pipeline in background.
    let pipeline_handle = tokio::spawn(async move {
        pipeline.run().await;
    });

    // Bridge: broadcast -> mpsc.
    let (snapshot_mpsc_tx, snapshot_mpsc_rx) = mpsc::channel(256);
    let (shutdown_tx, _) = watch::channel(false);
    let mut shutdown_rx1 = shutdown_tx.subscribe();
    let bridge_handle = tokio::spawn(async move {
        let mut rx = snapshot_rx;
        loop {
            tokio::select! {
                result = rx.recv() => {
                    match result {
                        Ok(snap) => {
                            if snapshot_mpsc_tx.send(snap).await.is_err() { break; }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(_) => continue,
                    }
                }
                _ = shutdown_rx1.changed() => { if *shutdown_rx1.borrow() { break; } }
            }
        }
    });

    // Strategy: use a very aggressive threshold so it fires on real data.
    // Set threshold = 1.0 (buy when ask <= 1.0) — will fire on almost any market
    // since prices are between 0 and 1.
    let strategy: Box<dyn pm_strategy::strategy::Strategy> =
        Box::new(pm_strategy::threshold::ThresholdStrategy::new(
            asset_id.clone(),
            Side::Buy,
            1.0, // aggressive threshold to ensure we get a trigger from real data
            "5".to_string(),
            OrderType::FOK,
        ));
    let (trigger_tx, mut trigger_rx) = mpsc::channel(256);
    let mut runner = pm_strategy::runner::StrategyRunner::new(
        strategy,
        snapshot_mpsc_rx,
        trigger_tx,
    );
    let runner_handle = tokio::spawn(async move { runner.run().await });

    // Wait for a trigger from real market data (timeout 30s).
    println!("Waiting for market data and strategy trigger...");
    let result = timeout(Duration::from_secs(30), trigger_rx.recv()).await;

    match result {
        Ok(Some(trigger)) => {
            println!("Trigger received! [DRY RUN — no order sent]");
            println!("  trigger_id: {}", trigger.trigger_id);
            println!("  token_id:   {}...", &trigger.token_id[..20.min(trigger.token_id.len())]);
            println!("  side:       {:?}", trigger.side);
            println!("  price:      {}", trigger.price);
            println!("  size:       {}", trigger.size);
            println!("  order_type: {:?}", trigger.order_type);
            assert_eq!(trigger.side, Side::Buy);
            assert_eq!(trigger.order_type, OrderType::FOK);
            println!("=== PASS ===\n");
        }
        Ok(None) => {
            panic!("Trigger channel closed unexpectedly");
        }
        Err(_) => {
            panic!("Timeout: no trigger received within 30 seconds. WebSocket may be down or market is inactive.");
        }
    }

    // Clean up.
    let _ = shutdown_tx.send(true);
    pipeline_handle.abort();
    bridge_handle.abort();
    runner_handle.abort();
}
