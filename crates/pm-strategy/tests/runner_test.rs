use pm_strategy::runner::StrategyRunner;
use pm_strategy::spread::SpreadStrategy;
use pm_strategy::threshold::ThresholdStrategy;
use pm_strategy::*;
use tokio::sync::mpsc;

fn make_snapshot(asset: &str, bid: &str, ask: &str, ts: u64) -> OrderBookSnapshot {
    OrderBookSnapshot {
        asset_id: asset.to_string(),
        best_bid: Some(PriceLevel {
            price: bid.to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: ask.to_string(),
            size: "100".to_string(),
        }),
        timestamp_ms: ts,
        hash: "h".to_string(),
    }
}

#[tokio::test]
async fn runner_forwards_trigger_from_threshold() {
    let (snap_tx, snap_rx) = mpsc::channel::<OrderBookSnapshot>(16);
    let (trigger_tx, mut trigger_rx) = mpsc::channel::<TriggerMessage>(16);

    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.40,
        "50".to_string(),
        OrderType::FOK,
    );

    let mut runner = StrategyRunner::new(Box::new(strategy), snap_rx, trigger_tx);

    // Spawn runner in background
    let handle = tokio::spawn(async move {
        runner.run().await;
    });

    // Send a snapshot that should NOT trigger (ask=0.45 > 0.40)
    snap_tx
        .send(make_snapshot("token_abc", "0.44", "0.45", 1))
        .await
        .unwrap();
    // Send a snapshot that SHOULD trigger (ask=0.39 <= 0.40)
    snap_tx
        .send(make_snapshot("token_abc", "0.38", "0.39", 2))
        .await
        .unwrap();

    // Drop sender to signal runner to stop
    drop(snap_tx);
    handle.await.unwrap();

    // Should have received exactly one trigger
    let t = trigger_rx.recv().await.unwrap();
    assert_eq!(t.side, Side::Buy);
    assert_eq!(t.price, "0.39");
    assert_eq!(t.size, "50");

    // No more triggers
    assert!(trigger_rx.try_recv().is_err());
}

#[tokio::test]
async fn runner_forwards_multiple_triggers_from_spread() {
    let (snap_tx, snap_rx) = mpsc::channel::<OrderBookSnapshot>(16);
    let (trigger_tx, mut trigger_rx) = mpsc::channel::<TriggerMessage>(16);

    let strategy = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Sell,
        0.03,
        "25".to_string(),
        OrderType::GTC,
    );

    let mut runner = StrategyRunner::new(Box::new(strategy), snap_rx, trigger_tx);

    let handle = tokio::spawn(async move {
        runner.run().await;
    });

    // Spread=0.04, no fire
    snap_tx
        .send(make_snapshot("token_abc", "0.46", "0.50", 1))
        .await
        .unwrap();
    // Spread=0.01, fires
    snap_tx
        .send(make_snapshot("token_abc", "0.495", "0.505", 2))
        .await
        .unwrap();
    // Spread=0.02, fires
    snap_tx
        .send(make_snapshot("token_abc", "0.49", "0.51", 3))
        .await
        .unwrap();

    drop(snap_tx);
    handle.await.unwrap();

    let t1 = trigger_rx.recv().await.unwrap();
    let t2 = trigger_rx.recv().await.unwrap();
    assert_eq!(t1.side, Side::Sell);
    assert_eq!(t2.side, Side::Sell);
    assert_eq!(t1.trigger_id + 1, t2.trigger_id);

    assert!(trigger_rx.try_recv().is_err());
}

#[tokio::test]
async fn runner_handles_empty_channel() {
    let (_snap_tx, snap_rx) = mpsc::channel::<OrderBookSnapshot>(16);
    let (trigger_tx, mut trigger_rx) = mpsc::channel::<TriggerMessage>(16);

    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.40,
        "50".to_string(),
        OrderType::FOK,
    );

    let mut runner = StrategyRunner::new(Box::new(strategy), snap_rx, trigger_tx);

    // Drop sender immediately — runner should exit cleanly
    drop(_snap_tx);

    let handle = tokio::spawn(async move {
        runner.run().await;
    });

    handle.await.unwrap();
    assert!(trigger_rx.try_recv().is_err());
}

#[tokio::test]
async fn runner_processes_sequence_of_mock_snapshots() {
    let (snap_tx, snap_rx) = mpsc::channel::<OrderBookSnapshot>(32);
    let (trigger_tx, mut trigger_rx) = mpsc::channel::<TriggerMessage>(32);

    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.50,
        "10".to_string(),
        OrderType::FOK,
    );

    let mut runner = StrategyRunner::new(Box::new(strategy), snap_rx, trigger_tx);

    let handle = tokio::spawn(async move {
        runner.run().await;
    });

    // Send 10 snapshots, all below threshold → 10 triggers
    for i in 0..10 {
        snap_tx
            .send(make_snapshot("token_abc", "0.40", "0.45", i))
            .await
            .unwrap();
    }

    drop(snap_tx);
    handle.await.unwrap();

    let mut count = 0;
    while trigger_rx.try_recv().is_ok() {
        count += 1;
    }
    assert_eq!(count, 10);
}
