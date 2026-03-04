use pm_strategy::*;
use pm_strategy::backtest::BacktestRunner;
use pm_strategy::threshold::ThresholdStrategy;
use pm_strategy::spread::SpreadStrategy;

fn sample_snapshots() -> Vec<OrderBookSnapshot> {
    vec![
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel { price: "0.50".to_string(), size: "100".to_string() }),
            best_ask: Some(PriceLevel { price: "0.55".to_string(), size: "100".to_string() }),
            timestamp_ms: 1000,
            hash: "h1".to_string(),
        },
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel { price: "0.48".to_string(), size: "100".to_string() }),
            best_ask: Some(PriceLevel { price: "0.52".to_string(), size: "100".to_string() }),
            timestamp_ms: 2000,
            hash: "h2".to_string(),
        },
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel { price: "0.42".to_string(), size: "100".to_string() }),
            best_ask: Some(PriceLevel { price: "0.44".to_string(), size: "100".to_string() }),
            timestamp_ms: 3000,
            hash: "h3".to_string(),
        },
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel { price: "0.38".to_string(), size: "100".to_string() }),
            best_ask: Some(PriceLevel { price: "0.40".to_string(), size: "100".to_string() }),
            timestamp_ms: 4000,
            hash: "h4".to_string(),
        },
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel { price: "0.35".to_string(), size: "100".to_string() }),
            best_ask: Some(PriceLevel { price: "0.37".to_string(), size: "100".to_string() }),
            timestamp_ms: 5000,
            hash: "h5".to_string(),
        },
    ]
}

#[test]
fn backtest_threshold_finds_triggers() {
    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.45,
        "50".to_string(),
        OrderType::FOK,
    );

    let snapshots = sample_snapshots();
    let result = BacktestRunner::run(Box::new(strategy), &snapshots);

    // Snapshots 3,4,5 have ask <= 0.45 (0.44, 0.40, 0.37)
    assert_eq!(result.triggers.len(), 3);
    assert_eq!(result.total_snapshots, 5);
    assert_eq!(result.triggers[0].price, "0.44");
    assert_eq!(result.triggers[1].price, "0.40");
    assert_eq!(result.triggers[2].price, "0.37");
}

#[test]
fn backtest_spread_finds_triggers() {
    let strategy = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.03,
        "25".to_string(),
        OrderType::GTC,
    );

    let snapshots = sample_snapshots();
    let result = BacktestRunner::run(Box::new(strategy), &snapshots);

    // Spreads: 0.05, 0.04, 0.02, 0.02, 0.02 — last three are < 0.03
    assert_eq!(result.triggers.len(), 3);
}

#[test]
fn backtest_no_triggers_on_empty_snapshots() {
    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.10,
        "50".to_string(),
        OrderType::FOK,
    );

    let result = BacktestRunner::run(Box::new(strategy), &[]);
    assert_eq!(result.triggers.len(), 0);
    assert_eq!(result.total_snapshots, 0);
}

#[test]
fn backtest_no_triggers_when_none_match() {
    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.10,
        "50".to_string(),
        OrderType::FOK,
    );

    let snapshots = sample_snapshots();
    let result = BacktestRunner::run(Box::new(strategy), &snapshots);

    // No ask price is <= 0.10
    assert_eq!(result.triggers.len(), 0);
    assert_eq!(result.total_snapshots, 5);
}

#[test]
fn backtest_load_snapshots_from_json() {
    use std::io::Write;

    let snaps = sample_snapshots();
    let json = serde_json::to_string_pretty(&snaps).unwrap();

    let dir = std::env::temp_dir().join("pm_backtest_test");
    std::fs::create_dir_all(&dir).ok();
    let path = dir.join("snapshots.json");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(json.as_bytes()).unwrap();

    let loaded = BacktestRunner::load_snapshots(&path).unwrap();
    assert_eq!(loaded.len(), 5);
    assert_eq!(loaded[0].asset_id, "token_abc");

    std::fs::remove_file(&path).ok();
}

#[test]
fn backtest_result_trigger_ids_sequential() {
    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.45,
        "50".to_string(),
        OrderType::FOK,
    );

    let snapshots = sample_snapshots();
    let result = BacktestRunner::run(Box::new(strategy), &snapshots);

    for (i, trigger) in result.triggers.iter().enumerate() {
        assert_eq!(trigger.trigger_id, (i as u64) + 1);
    }
}
