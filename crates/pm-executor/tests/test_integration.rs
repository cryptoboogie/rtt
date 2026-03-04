use rtt_core::trigger::{OrderBookSnapshot, OrderType, PriceLevel, Side};

/// Verify that the config file parses correctly.
#[test]
fn example_config_parses() {
    let toml_str = include_str!("../../../config.toml");
    let config: toml::Value = toml::from_str(toml_str).unwrap();
    assert!(config.get("credentials").is_some());
    assert!(config.get("connection").is_some());
    assert!(config.get("websocket").is_some());
    assert!(config.get("strategy").is_some());
    assert!(config.get("execution").is_some());
    assert!(config.get("logging").is_some());
}

/// Verify strategy builds from the example config's strategy section.
#[test]
fn strategy_builds_from_example_config() {
    let toml_str = include_str!("../../../config.toml");
    let config: toml::Value = toml::from_str(toml_str).unwrap();
    let strategy_section = toml::to_string(config.get("strategy").unwrap()).unwrap();
    let strategy_config: pm_strategy::config::StrategyConfig =
        toml::from_str(&strategy_section).unwrap();
    let strategy = strategy_config.build_strategy().unwrap();
    assert_eq!(strategy.name(), "threshold");
}

/// Test the full mock data flow: snapshot → strategy → trigger.
#[tokio::test]
async fn mock_snapshot_to_trigger_flow() {
    use pm_strategy::strategy::Strategy;

    // Create a threshold strategy that fires when ask <= 0.45
    let mut strategy = pm_strategy::threshold::ThresholdStrategy::new(
        "asset1".to_string(),
        Side::Buy,
        0.45,
        "10".to_string(),
        OrderType::FOK,
    );

    // Snapshot with ask above threshold — should not fire
    let snap_no_fire = OrderBookSnapshot {
        asset_id: "asset1".to_string(),
        best_bid: Some(PriceLevel {
            price: "0.44".to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: "0.50".to_string(),
            size: "100".to_string(),
        }),
        timestamp_ms: 1000,
        hash: "h1".to_string(),
    };
    assert!(strategy.on_book_update(&snap_no_fire).is_none());

    // Snapshot with ask at threshold — should fire
    let snap_fire = OrderBookSnapshot {
        asset_id: "asset1".to_string(),
        best_bid: Some(PriceLevel {
            price: "0.44".to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: "0.45".to_string(),
            size: "100".to_string(),
        }),
        timestamp_ms: 2000,
        hash: "h2".to_string(),
    };
    let trigger = strategy.on_book_update(&snap_fire);
    assert!(trigger.is_some());
    let t = trigger.unwrap();
    assert_eq!(t.side, Side::Buy);
    assert_eq!(t.price, "0.45");
    assert_eq!(t.size, "10");
    assert_eq!(t.order_type, OrderType::FOK);
}

/// Test channel bridge: broadcast → mpsc → strategy → trigger.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn end_to_end_channel_flow() {
    use tokio::sync::{broadcast, mpsc, watch};

    let (_broadcast_tx, _) = broadcast::channel::<OrderBookSnapshot>(16);
    let (snapshot_mpsc_tx, snapshot_mpsc_rx) = mpsc::channel(16);
    let (trigger_mpsc_tx, mut trigger_mpsc_rx) = mpsc::channel(16);
    let (shutdown_tx, _) = watch::channel(false);

    // Create strategy that fires on any snapshot with ask <= 0.45
    let strategy: Box<dyn pm_strategy::strategy::Strategy> =
        Box::new(pm_strategy::threshold::ThresholdStrategy::new(
            "asset1".to_string(),
            Side::Buy,
            0.45,
            "10".to_string(),
            OrderType::FOK,
        ));

    // Start strategy runner
    let mut runner =
        pm_strategy::runner::StrategyRunner::new(strategy, snapshot_mpsc_rx, trigger_mpsc_tx);
    let runner_handle = tokio::spawn(async move {
        runner.run().await;
    });

    // Send a snapshot that should trigger
    let snap = OrderBookSnapshot {
        asset_id: "asset1".to_string(),
        best_bid: Some(PriceLevel {
            price: "0.44".to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: "0.40".to_string(),
            size: "50".to_string(),
        }),
        timestamp_ms: 1000,
        hash: "h".to_string(),
    };
    snapshot_mpsc_tx.send(snap).await.unwrap();

    // Receive the trigger
    let trigger = tokio::time::timeout(std::time::Duration::from_secs(2), trigger_mpsc_rx.recv())
        .await
        .expect("Timeout waiting for trigger")
        .expect("Channel closed");

    assert_eq!(trigger.side, Side::Buy);
    assert_eq!(trigger.price, "0.40");
    assert_eq!(trigger.token_id, "asset1");

    // Clean shutdown
    drop(snapshot_mpsc_tx);
    let _ = shutdown_tx.send(true);
    let _ = runner_handle.await;
}

/// Full pipeline smoke test — requires network + credentials.
#[tokio::test]
#[ignore]
async fn full_pipeline_smoke_test() {
    // This test requires:
    // 1. Network access to Polymarket WebSocket
    // 2. Valid credentials in environment variables
    // Run with: cargo test -p pm-executor --test test_integration full_pipeline -- --ignored
    eprintln!("Full pipeline smoke test would start here");
}
