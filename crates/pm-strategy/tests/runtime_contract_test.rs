use pm_strategy::liquidity_rewards::{LiquidityRewardsMarket, LiquidityRewardsParams, LiquidityRewardsStrategy};
use pm_strategy::quote::{DesiredQuote, DesiredQuotes, QuoteId};
use pm_strategy::runtime::{ProvisionedTopology, QuoteRuntime, TriggerRuntime};
use pm_strategy::strategy::{
    InventoryDelta, IsolationPolicy, QuoteStrategy, RequirementSelector, StrategyDataRequirement,
    StrategyDataRequirementKind, StrategyRequirements, StrategyRuntimeView,
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
        reward: Some(rtt_core::RewardParams {
            rate_bps: None,
            max_spread: Some(rtt_core::Price::new("0.04")),
            min_size: Some(rtt_core::Size::new("50")),
            min_notional: None,
            native_daily_rate: Some(rtt_core::Notional::new("5")),
            sponsored_daily_rate: None,
            total_daily_rate: Some(rtt_core::Notional::new("5")),
            market_competitiveness: Some("2".to_string()),
            fee_enabled: Some(false),
            updated_at_ms: Some(1_700_000_000_000),
            freshness: rtt_core::RewardFreshness::Fresh,
        }),
    }
}

fn snapshot_update_for_asset(version: u64, asset_id: &str, bid_1: &str, bid_2: &str, ask_1: &str, ask_2: &str) -> NormalizedUpdate {
    let source_id = SourceId::new("polymarket-public");
    NormalizedUpdate {
        notice: UpdateNotice {
            source_id: source_id.clone(),
            source_kind: SourceKind::PolymarketWs,
            subject: rtt_core::InstrumentRef::asset(source_id.clone(), asset_id),
            kind: UpdateKind::BookSnapshot,
            version,
            source_hash: Some(format!("hash-{version}")),
        },
        payload: NormalizedUpdatePayload::BookSnapshot(BookSnapshotUpdate {
            market_id: MarketId::new("market-1"),
            asset_id: AssetId::new(asset_id),
            bids: vec![BookLevel {
                price: Price::new(bid_1),
                size: rtt_core::Size::new("20"),
            }, BookLevel {
                price: Price::new(bid_2),
                size: rtt_core::Size::new("100"),
            }],
            asks: vec![BookLevel {
                price: Price::new(ask_1),
                size: rtt_core::Size::new("20"),
            }, BookLevel {
                price: Price::new(ask_2),
                size: rtt_core::Size::new("100"),
            }],
            timestamp_ms: 1_700_000_000_000 + version,
            source_hash: Some(format!("hash-{version}")),
        }),
    }
}

fn snapshot_update(version: u64, ask: &str) -> NormalizedUpdate {
    snapshot_update_for_asset(version, "token_abc", "0.40", "0.39", ask, "0.49")
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

struct InventoryAwareQuoteStrategy;

impl QuoteStrategy for InventoryAwareQuoteStrategy {
    fn requirements(&self) -> StrategyRequirements {
        StrategyRequirements::quote(
            vec![
                StrategyDataRequirement::polymarket_bbo("token_abc"),
                StrategyDataRequirement {
                    kind: StrategyDataRequirementKind::Inventory,
                    selector: RequirementSelector::Asset("token_abc".to_string()),
                },
            ],
            IsolationPolicy::SharedFeedAcceptable,
        )
    }

    fn on_update(&mut self, view: &StrategyRuntimeView) -> Option<DesiredQuotes> {
        let inventory = view.inventory("token_abc", Side::Buy)?;
        let book = view.book("token_abc")?;

        Some(DesiredQuotes::single(DesiredQuote::new(
            QuoteId::new("inventory-quote"),
            book.asset_id.as_str(),
            Side::Sell,
            book.best_bid.as_ref()?.price.exact.clone(),
            inventory.filled_size.clone(),
            OrderType::GTC,
        )))
    }

    fn name(&self) -> &str {
        "inventory-aware-quote"
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

#[test]
fn quote_runtime_surfaces_inventory_deltas_back_to_quote_strategies() {
    let store = HotStateStore::new();
    store.register_market(&sample_market_meta());

    let (_notice_tx, notice_rx) = mpsc::channel(16);
    let (quote_tx, _quote_rx) = mpsc::channel(16);
    let mut runtime = QuoteRuntime::new(
        Box::new(InventoryAwareQuoteStrategy),
        store.clone(),
        notice_rx,
        quote_tx,
    );

    runtime.apply_inventory_delta(InventoryDelta::new(
        "token_abc",
        Side::Buy,
        "7",
        "3.08",
        1_700_000_000_500,
    ));

    let book = snapshot_update(1, "0.47");
    store.apply_update(&book);

    let desired = runtime.handle_notice(&book.notice).expect("desired quote");
    assert_eq!(desired.quotes.len(), 1);
    assert_eq!(desired.quotes[0].quote_id, QuoteId::new("inventory-quote"));
    assert_eq!(desired.quotes[0].size, "7");
    assert_eq!(desired.quotes[0].price, "0.40");
}

#[test]
fn liquidity_rewards_quote_runtime_resolves_selected_yes_no_books_into_desired_quotes() {
    let store = HotStateStore::new();
    store.register_market(&sample_market_meta());

    let (_notice_tx, notice_rx) = mpsc::channel(16);
    let (quote_tx, _quote_rx) = mpsc::channel(16);
    let mut runtime = QuoteRuntime::new(
        Box::new(LiquidityRewardsStrategy::new(
            vec![LiquidityRewardsMarket {
                condition_id: "condition-1".to_string(),
                yes_asset_id: "token_abc".to_string(),
                no_asset_id: "token_xyz".to_string(),
                tick_size: "0.01".to_string(),
                min_order_size: Some("5".to_string()),
                reward_max_spread: "0.04".to_string(),
                reward_min_size: "50".to_string(),
                end_time_ms: Some(1_700_001_000_000),
            }],
            LiquidityRewardsParams {
                initial_bankroll_usd: 100.0,
                max_total_deployed_usd: 100.0,
                max_markets: 1,
                base_quote_size: 50.0,
                edge_buffer: 0.02,
                target_spread_cents: 2.0,
                quote_ttl_secs: 30,
                min_total_daily_rate: 1.0,
                max_market_competitiveness: 10.0,
                min_time_to_expiry_secs: 60,
                max_inventory_per_market: 100.0,
                max_unhedged_notional_per_market: 40.0,
            },
        )),
        store.clone(),
        notice_rx,
        quote_tx,
    );

    let yes = snapshot_update_for_asset(1, "token_abc", "0.49", "0.47", "0.51", "0.55");
    let no = snapshot_update_for_asset(2, "token_xyz", "0.48", "0.46", "0.52", "0.56");
    store.apply_update(&yes);
    store.apply_update(&no);

    assert!(runtime.handle_notice(&yes.notice).is_none());
    let desired = runtime.handle_notice(&no.notice).expect("desired quotes");

    assert_eq!(desired.quotes.len(), 2);
    assert_eq!(desired.quotes[0].quote_id, QuoteId::new("condition-1:yes:entry"));
    assert_eq!(desired.quotes[1].quote_id, QuoteId::new("condition-1:no:entry"));
}
