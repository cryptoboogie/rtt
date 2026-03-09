use pm_strategy::backtest::BacktestRunner;
use pm_strategy::quote::{DesiredQuote, DesiredQuotes};
use pm_strategy::strategy::{
    IsolationPolicy, QuoteStrategy, StrategyDataRequirement, StrategyRequirements,
    StrategyRuntimeView,
};
use pm_strategy::threshold::ThresholdStrategy;
use pm_strategy::*;
use rtt_core::{
    AssetId, BookLevel, BookSnapshotUpdate, MarketId, MarketMeta, MarketStatus, MinOrderSize,
    NormalizedUpdate, NormalizedUpdatePayload, OutcomeSide, OutcomeToken, Price,
    ReferencePriceUpdate, SourceId, SourceKind, TickSize, UpdateKind, UpdateNotice,
};

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
            IsolationPolicy::SharedFeedAcceptable,
        )
    }

    fn on_update(&mut self, view: &StrategyRuntimeView) -> Option<DesiredQuotes> {
        let book = view.book("token_abc")?;
        let reference = view.reference("BTC-USD")?;
        Some(DesiredQuotes::single(DesiredQuote {
            asset_id: book.asset_id.as_str().to_string(),
            side: Side::Buy,
            price: reference.reference_price.as_ref()?.exact.clone(),
            size: "10".to_string(),
            order_type: OrderType::GTC,
        }))
    }

    fn name(&self) -> &str {
        "cross-feed-quote"
    }
}

#[test]
fn trigger_contract_notice_replay_matches_threshold_behavior() {
    let updates = vec![
        snapshot_update(1, "0.48"),
        snapshot_update(2, "0.44"),
        snapshot_update(3, "0.40"),
    ];

    let result = BacktestRunner::run_trigger_notice_replay(
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

    assert_eq!(result.total_events, 3);
    assert_eq!(result.total_snapshots, 3);
    assert_eq!(result.triggers.len(), 2);
    assert_eq!(result.triggers[0].price, "0.44");
    assert_eq!(result.triggers[1].price, "0.40");
}

#[test]
fn quote_contract_notice_replay_collects_desired_quotes() {
    let updates = vec![snapshot_update(1, "0.47"), reference_update(2, "0.46")];

    let result = BacktestRunner::run_quote_notice_replay(
        Box::new(CrossFeedQuoteStrategy),
        &[sample_market_meta()],
        &updates,
    );

    assert_eq!(result.total_events, 2);
    assert_eq!(result.total_views, 2);
    assert_eq!(result.desired_quotes.len(), 1);
    assert_eq!(result.desired_quotes[0].quotes[0].price, "0.46");
}
