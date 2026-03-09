use pm_strategy::quote::{DesiredQuote, DesiredQuotes, QuoteId};
use pm_strategy::runtime::{ProvisionedTopology, QuoteRuntime, TriggerRuntime};
use pm_strategy::strategy::{
    IsolationPolicy, QuoteStrategy, StrategyDataRequirement, StrategyRequirements,
    StrategyRuntimeView,
};
use pm_strategy::threshold::ThresholdStrategy;
use pm_strategy::*;
use rtt_core::{
    AssetId, BookLevel, BookSnapshotUpdate, HotStateStore, MarketId, MarketMeta, MarketStatus,
    MinOrderSize, NormalizedUpdate, NormalizedUpdatePayload, OutcomeSide, OutcomeToken, Price,
    ReferencePriceUpdate, SourceId, SourceKind, TickSize, UpdateKind, UpdateNotice,
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
                size: rtt_core::Size::new("100"),
            }],
            asks: vec![BookLevel {
                price: Price::new(ask),
                size: rtt_core::Size::new("100"),
            }],
            timestamp_ms: 1_700_000_000_000 + version,
            source_hash: Some(format!("hash-{version}")),
        }),
    }
}

fn reference_update(version: u64, price: &str) -> NormalizedUpdate {
    let source_id = SourceId::new("external-reference");
    NormalizedUpdate {
        notice: UpdateNotice {
            source_id: source_id.clone(),
            source_kind: SourceKind::ExternalReference,
            subject: rtt_core::InstrumentRef::symbol(source_id.clone(), "BTC-USD"),
            kind: UpdateKind::ReferencePrice,
            version,
            source_hash: Some(format!("ref-{version}")),
        },
        payload: NormalizedUpdatePayload::ReferencePrice(ReferencePriceUpdate {
            price: Price::new(price),
            notional: None,
            timestamp_ms: 1_700_000_100_000 + version,
        }),
    }
}

struct CrossFeedQuoteStrategy;

impl QuoteStrategy for CrossFeedQuoteStrategy {
    fn requirements(&self) -> StrategyRequirements {
        StrategyRequirements::quote(
            vec![
                StrategyDataRequirement::polymarket_bbo("token_abc"),
                StrategyDataRequirement::external_reference_price("BTC-USD"),
            ],
            IsolationPolicy::DedicatedRequired,
        )
    }

    fn on_update(&mut self, view: &StrategyRuntimeView) -> Option<DesiredQuotes> {
        let book = view.book("token_abc")?;
        let reference = view.reference("BTC-USD")?;

        Some(DesiredQuotes::single(DesiredQuote::new(
            QuoteId::new("quote-1"),
            book.asset_id.as_str(),
            Side::Buy,
            reference.reference_price.as_ref()?.exact.clone(),
            "25",
            OrderType::GTC,
        )))
    }

    fn name(&self) -> &str {
        "cross-feed-quote"
    }
}

#[tokio::test]
async fn trigger_runtime_keeps_existing_threshold_strategy_working() {
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
    let mut runtime = TriggerRuntime::new(Box::new(strategy), store.clone(), notice_rx, trigger_tx);

    assert_eq!(runtime.topology().inputs.len(), 1);
    assert_eq!(
        runtime.topology().inputs[0].topology,
        ProvisionedTopology::Shared
    );

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
    assert!(trigger_rx.try_recv().is_err());
}

#[tokio::test]
async fn quote_runtime_merges_cross_feed_state_into_desired_quotes() {
    let store = HotStateStore::new();
    store.register_market(&sample_market_meta());

    let (notice_tx, notice_rx) = mpsc::channel(16);
    let (quote_tx, mut quote_rx) = mpsc::channel(16);
    let mut runtime = QuoteRuntime::new(
        Box::new(CrossFeedQuoteStrategy),
        store.clone(),
        notice_rx,
        quote_tx,
    );

    assert_eq!(runtime.topology().inputs.len(), 2);
    assert!(runtime
        .topology()
        .inputs
        .iter()
        .all(|input| input.topology == ProvisionedTopology::Dedicated));

    let handle = tokio::spawn(async move {
        runtime.run().await;
    });

    let book = snapshot_update(1, "0.47");
    store.apply_update(&book);
    notice_tx.send(book.notice.clone()).await.unwrap();

    let reference = reference_update(2, "0.46");
    store.apply_update(&reference);
    notice_tx.send(reference.notice.clone()).await.unwrap();

    drop(notice_tx);
    handle.await.unwrap();

    let desired = quote_rx.recv().await.expect("desired quotes");
    assert_eq!(desired.quotes.len(), 1);
    assert_eq!(desired.quotes[0].asset_id, "token_abc");
    assert_eq!(desired.quotes[0].price, "0.46");
    assert!(quote_rx.try_recv().is_err());
}
