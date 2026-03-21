use pm_strategy::runtime::NoticeDrivenRuntime;
use pm_strategy::threshold::ThresholdStrategy;
use pm_strategy::*;
use rtt_core::{
    AssetId, BookLevel, BookSnapshotUpdate, HotStateStore, MarketId, MarketMeta, MarketStatus,
    MinOrderSize, NormalizedUpdate, NormalizedUpdatePayload, OutcomeSide, OutcomeToken, Price,
    Size, SourceId, SourceKind, TickSize, UpdateKind, UpdateNotice,
};
use tokio::sync::mpsc;

fn sample_market_meta() -> MarketMeta {
    MarketMeta {
        market_id: MarketId::new("market-1"),
        yes_asset: OutcomeToken::new(AssetId::new("token_abc"), OutcomeSide::Yes),
        no_asset: OutcomeToken::new(AssetId::new("token_xyz"), OutcomeSide::No),
        condition_id: Some("condition-1".to_string()),
        tick_size: TickSize::new("0.01"),
        min_order_size: Some(MinOrderSize::new("5")),
        status: MarketStatus::Active,
        reward: None,
    }
}

fn snapshot_update(version: u64, ask: &str) -> NormalizedUpdate {
    let source_id = SourceId::new("polymarket-public");
    NormalizedUpdate {
        notice: UpdateNotice {
            source_id: source_id.clone(),
            source_kind: SourceKind::PolymarketWs,
            subject: rtt_core::InstrumentRef::asset(source_id.clone(), "token_abc"),
            kind: UpdateKind::BookSnapshot,
            version,
            source_hash: Some(format!("hash-{version}")),
        },
        payload: NormalizedUpdatePayload::BookSnapshot(BookSnapshotUpdate {
            market_id: MarketId::new("market-1"),
            asset_id: AssetId::new("token_abc"),
            bids: vec![BookLevel {
                price: Price::new("0.40"),
                size: Size::new("100"),
            }],
            asks: vec![BookLevel {
                price: Price::new(ask),
                size: Size::new("100"),
            }],
            timestamp_ms: 1_700_000_000_000 + version,
            source_hash: Some(format!("hash-{version}")),
        }),
    }
}

#[tokio::test]
async fn notice_runtime_projects_hot_state_into_strategy_triggers() {
    let store = HotStateStore::new();
    store.register_market(&sample_market_meta());

    let (notice_tx, notice_rx) = mpsc::channel(16);
    let (trigger_tx, mut trigger_rx) = mpsc::channel(16);
    let strategy = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.45,
        "50".to_string(),
        OrderType::FOK,
    );
    let mut runtime =
        NoticeDrivenRuntime::new(Box::new(strategy), store.clone(), notice_rx, trigger_tx);

    let handle = tokio::spawn(async move {
        runtime.run().await;
    });

    let first = snapshot_update(1, "0.48");
    store.apply_update(&first);
    notice_tx.send(first.notice.clone()).await.unwrap();

    let second = snapshot_update(2, "0.44");
    store.apply_update(&second);
    notice_tx.send(second.notice.clone()).await.unwrap();

    drop(notice_tx);
    handle.await.unwrap();

    let trigger = trigger_rx.recv().await.expect("trigger");
    assert_eq!(trigger.price, "0.44");
    assert_eq!(trigger.side, Side::Buy);
    assert!(trigger_rx.try_recv().is_err());
}
