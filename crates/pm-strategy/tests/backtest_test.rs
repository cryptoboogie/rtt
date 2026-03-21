use pm_strategy::backtest::BacktestRunner;
use pm_strategy::spread::SpreadStrategy;
use pm_strategy::threshold::ThresholdStrategy;
use pm_strategy::*;
use rtt_core::{
    AssetId, BookLevel, BookSnapshotUpdate, MarketId, MarketMeta, MarketStatus, MinOrderSize,
    NormalizedUpdate, NormalizedUpdatePayload, OutcomeSide, OutcomeToken, Price, Size, SourceId,
    SourceKind, TickSize, UpdateKind, UpdateNotice,
};

fn sample_snapshots() -> Vec<OrderBookSnapshot> {
    vec![
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel {
                price: "0.50".to_string(),
                size: "100".to_string(),
            }),
            best_ask: Some(PriceLevel {
                price: "0.55".to_string(),
                size: "100".to_string(),
            }),
            timestamp_ms: 1000,
            hash: "h1".to_string(),
        },
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel {
                price: "0.48".to_string(),
                size: "100".to_string(),
            }),
            best_ask: Some(PriceLevel {
                price: "0.52".to_string(),
                size: "100".to_string(),
            }),
            timestamp_ms: 2000,
            hash: "h2".to_string(),
        },
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel {
                price: "0.42".to_string(),
                size: "100".to_string(),
            }),
            best_ask: Some(PriceLevel {
                price: "0.44".to_string(),
                size: "100".to_string(),
            }),
            timestamp_ms: 3000,
            hash: "h3".to_string(),
        },
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel {
                price: "0.38".to_string(),
                size: "100".to_string(),
            }),
            best_ask: Some(PriceLevel {
                price: "0.40".to_string(),
                size: "100".to_string(),
            }),
            timestamp_ms: 4000,
            hash: "h4".to_string(),
        },
        OrderBookSnapshot {
            asset_id: "token_abc".to_string(),
            best_bid: Some(PriceLevel {
                price: "0.35".to_string(),
                size: "100".to_string(),
            }),
            best_ask: Some(PriceLevel {
                price: "0.37".to_string(),
                size: "100".to_string(),
            }),
            timestamp_ms: 5000,
            hash: "h5".to_string(),
        },
    ]
}

fn sample_market_meta() -> MarketMeta {
    MarketMeta {
        market_id: MarketId::new("market-1"),
        yes_asset: OutcomeToken::new(AssetId::new("token_abc"), OutcomeSide::Yes),
        no_asset: OutcomeToken::new(AssetId::new("token_xyz"), OutcomeSide::No),
        condition_id: Some("condition-1".to_string()),
        neg_risk: false,
        tick_size: TickSize::new("0.01"),
        min_order_size: Some(MinOrderSize::new("5")),
        status: MarketStatus::Active,
        reward: None,
    }
}

fn sample_normalized_updates() -> Vec<NormalizedUpdate> {
    let source_id = SourceId::new("polymarket-public");
    sample_snapshots()
        .into_iter()
        .enumerate()
        .map(|(index, snapshot)| NormalizedUpdate {
            notice: UpdateNotice {
                source_id: source_id.clone(),
                source_kind: SourceKind::PolymarketWs,
                subject: rtt_core::InstrumentRef::asset(
                    source_id.clone(),
                    snapshot.asset_id.clone(),
                ),
                kind: UpdateKind::BookSnapshot,
                version: (index as u64) + 1,
                source_hash: Some(snapshot.hash.clone()),
            },
            payload: NormalizedUpdatePayload::BookSnapshot(BookSnapshotUpdate {
                market_id: MarketId::new("market-1"),
                asset_id: AssetId::new(snapshot.asset_id),
                bids: snapshot
                    .best_bid
                    .into_iter()
                    .map(|level| BookLevel {
                        price: Price::new(level.price),
                        size: Size::new(level.size),
                    })
                    .collect(),
                asks: snapshot
                    .best_ask
                    .into_iter()
                    .map(|level| BookLevel {
                        price: Price::new(level.price),
                        size: Size::new(level.size),
                    })
                    .collect(),
                timestamp_ms: snapshot.timestamp_ms,
                source_hash: Some(snapshot.hash),
            }),
        })
        .collect()
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

#[test]
fn notice_replay_matches_snapshot_backtest_for_threshold_strategy() {
    let snapshots = sample_snapshots();
    let updates = sample_normalized_updates();

    let snapshot_result = BacktestRunner::run(
        Box::new(ThresholdStrategy::new(
            "token_abc".to_string(),
            Side::Buy,
            0.45,
            "50".to_string(),
            OrderType::FOK,
        )),
        &snapshots,
    );

    let notice_result = BacktestRunner::run_notice_replay(
        Box::new(ThresholdStrategy::new(
            "token_abc".to_string(),
            Side::Buy,
            0.45,
            "50".to_string(),
            OrderType::FOK,
        )),
        &[sample_market_meta()],
        &updates,
    );

    assert_eq!(notice_result.total_events, updates.len());
    assert_eq!(
        notice_result.total_snapshots,
        snapshot_result.total_snapshots
    );
    assert_eq!(notice_result.triggers.len(), snapshot_result.triggers.len());
    assert_eq!(notice_result.triggers[0].price, "0.44");
    assert_eq!(notice_result.triggers[2].price, "0.37");
}
