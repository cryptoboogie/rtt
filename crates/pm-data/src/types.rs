use rtt_core::polymarket::public_source_id as shared_polymarket_public_source_id;
use serde::Deserialize;

// Re-export shared types from rtt-core
pub use rtt_core::trigger::{OrderBookSnapshot, OrderType, PriceLevel, Side, TriggerMessage};
use rtt_core::{
    AssetId, BestBidAskUpdate, BookDeltaUpdate, BookLevel, BookSnapshotUpdate, InstrumentRef,
    MarketId, NormalizedUpdate, NormalizedUpdatePayload, Price, ReconnectUpdate, Size, SourceId,
    SourceKind, TickSize, TickSizeChangeUpdate, TradeTickUpdate, UpdateKind, UpdateNotice,
};

pub fn polymarket_public_source_id() -> SourceId {
    shared_polymarket_public_source_id()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectEvent {
    pub sequence: u64,
    pub timestamp_ms: u64,
}

// === WebSocket message types ===

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "event_type")]
pub enum WsMessage {
    #[serde(rename = "book")]
    Book(BookUpdate),
    #[serde(rename = "price_change")]
    PriceChange(PriceChangeEvent),
    #[serde(rename = "last_trade_price")]
    LastTradePrice(LastTradePriceEvent),
    #[serde(rename = "tick_size_change")]
    TickSizeChange(TickSizeChangeEvent),
    #[serde(rename = "best_bid_ask")]
    BestBidAsk(BestBidAskEvent),
    /// Emitted internally when the WS connection reconnects.
    /// Not deserialized from JSON — only produced by WsClient.
    #[serde(skip)]
    Reconnected(ReconnectEvent),
    /// Unknown or newly-added informational market event; ignored by current pipelines.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BookUpdate {
    pub asset_id: String,
    pub market: String,
    pub timestamp: String,
    #[serde(default)]
    pub bids: Vec<WsOrderBookLevel>,
    #[serde(default)]
    pub asks: Vec<WsOrderBookLevel>,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WsOrderBookLevel {
    pub price: String,
    pub size: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PriceChangeEvent {
    pub market: String,
    pub timestamp: String,
    #[serde(default)]
    pub price_changes: Vec<PriceChangeBatchEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PriceChangeBatchEntry {
    pub asset_id: String,
    pub price: String,
    pub size: Option<String>,
    pub side: Side,
    pub hash: Option<String>,
    pub best_bid: Option<String>,
    pub best_ask: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LastTradePriceEvent {
    pub asset_id: String,
    pub market: String,
    pub price: String,
    pub side: Option<Side>,
    pub size: Option<String>,
    pub fee_rate_bps: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TickSizeChangeEvent {
    pub asset_id: String,
    pub market: String,
    pub old_tick_size: String,
    pub new_tick_size: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BestBidAskEvent {
    pub asset_id: String,
    pub market: String,
    pub best_bid: String,
    pub best_ask: String,
    pub spread: String,
    pub timestamp: String,
}

fn parse_timestamp_ms(timestamp: &str) -> u64 {
    timestamp.parse().unwrap_or(0)
}

fn asset_notice(
    source_id: &SourceId,
    asset_id: &str,
    kind: UpdateKind,
    version: u64,
    source_hash: Option<String>,
) -> UpdateNotice {
    UpdateNotice {
        source_id: source_id.clone(),
        source_kind: SourceKind::PolymarketWs,
        subject: InstrumentRef::asset(source_id.clone(), asset_id),
        kind,
        version,
        source_hash,
    }
}

impl WsMessage {
    pub fn to_normalized_updates(&self) -> Vec<NormalizedUpdate> {
        let source_id = polymarket_public_source_id();

        match self {
            WsMessage::Book(book) => {
                let timestamp_ms = parse_timestamp_ms(&book.timestamp);
                let source_hash = book.hash.clone();
                vec![NormalizedUpdate {
                    notice: asset_notice(
                        &source_id,
                        &book.asset_id,
                        UpdateKind::BookSnapshot,
                        timestamp_ms,
                        source_hash.clone(),
                    ),
                    payload: NormalizedUpdatePayload::BookSnapshot(BookSnapshotUpdate {
                        market_id: MarketId::new(book.market.clone()),
                        asset_id: AssetId::new(book.asset_id.clone()),
                        bids: book
                            .bids
                            .iter()
                            .map(|level| BookLevel {
                                price: Price::new(level.price.clone()),
                                size: Size::new(level.size.clone()),
                            })
                            .collect(),
                        asks: book
                            .asks
                            .iter()
                            .map(|level| BookLevel {
                                price: Price::new(level.price.clone()),
                                size: Size::new(level.size.clone()),
                            })
                            .collect(),
                        timestamp_ms,
                        source_hash,
                    }),
                }]
            }
            WsMessage::PriceChange(price_change) => {
                let timestamp_ms = parse_timestamp_ms(&price_change.timestamp);
                price_change
                    .price_changes
                    .iter()
                    .map(|entry| {
                        let source_hash = entry.hash.clone();
                        NormalizedUpdate {
                            notice: asset_notice(
                                &source_id,
                                &entry.asset_id,
                                UpdateKind::BookDelta,
                                timestamp_ms,
                                source_hash.clone(),
                            ),
                            payload: NormalizedUpdatePayload::BookDelta(BookDeltaUpdate {
                                market_id: MarketId::new(price_change.market.clone()),
                                asset_id: AssetId::new(entry.asset_id.clone()),
                                price: Price::new(entry.price.clone()),
                                size: Size::new(
                                    entry.size.clone().unwrap_or_else(|| "0".to_string()),
                                ),
                                side: entry.side,
                                timestamp_ms,
                                best_bid: entry.best_bid.clone().map(Price::new),
                                best_ask: entry.best_ask.clone().map(Price::new),
                                source_hash,
                            }),
                        }
                    })
                    .collect()
            }
            WsMessage::LastTradePrice(last_trade) => {
                let timestamp_ms = parse_timestamp_ms(&last_trade.timestamp);
                vec![NormalizedUpdate {
                    notice: asset_notice(
                        &source_id,
                        &last_trade.asset_id,
                        UpdateKind::TradeTick,
                        timestamp_ms,
                        None,
                    ),
                    payload: NormalizedUpdatePayload::TradeTick(TradeTickUpdate {
                        market_id: Some(MarketId::new(last_trade.market.clone())),
                        asset_id: Some(AssetId::new(last_trade.asset_id.clone())),
                        price: Price::new(last_trade.price.clone()),
                        size: last_trade.size.clone().map(Size::new),
                        side: last_trade.side,
                        timestamp_ms,
                    }),
                }]
            }
            WsMessage::TickSizeChange(tick_size_change) => {
                let timestamp_ms = parse_timestamp_ms(&tick_size_change.timestamp);
                vec![NormalizedUpdate {
                    notice: asset_notice(
                        &source_id,
                        &tick_size_change.asset_id,
                        UpdateKind::TickSizeChange,
                        timestamp_ms,
                        None,
                    ),
                    payload: NormalizedUpdatePayload::TickSizeChange(TickSizeChangeUpdate {
                        market_id: MarketId::new(tick_size_change.market.clone()),
                        asset_id: AssetId::new(tick_size_change.asset_id.clone()),
                        old_tick_size: TickSize::new(tick_size_change.old_tick_size.clone()),
                        new_tick_size: TickSize::new(tick_size_change.new_tick_size.clone()),
                        timestamp_ms,
                    }),
                }]
            }
            WsMessage::BestBidAsk(best_bid_ask) => {
                let timestamp_ms = parse_timestamp_ms(&best_bid_ask.timestamp);
                vec![NormalizedUpdate {
                    notice: asset_notice(
                        &source_id,
                        &best_bid_ask.asset_id,
                        UpdateKind::BestBidAsk,
                        timestamp_ms,
                        None,
                    ),
                    payload: NormalizedUpdatePayload::BestBidAsk(BestBidAskUpdate {
                        market_id: MarketId::new(best_bid_ask.market.clone()),
                        asset_id: AssetId::new(best_bid_ask.asset_id.clone()),
                        best_bid: Price::new(best_bid_ask.best_bid.clone()),
                        best_ask: Price::new(best_bid_ask.best_ask.clone()),
                        spread: Some(Price::new(best_bid_ask.spread.clone())),
                        timestamp_ms,
                    }),
                }]
            }
            WsMessage::Unknown => Vec::new(),
            WsMessage::Reconnected(event) => vec![NormalizedUpdate {
                notice: UpdateNotice {
                    source_id: source_id.clone(),
                    source_kind: SourceKind::PolymarketWs,
                    subject: InstrumentRef::source(source_id),
                    kind: UpdateKind::Reconnect,
                    version: event.sequence,
                    source_hash: None,
                },
                payload: NormalizedUpdatePayload::Reconnect(ReconnectUpdate {
                    sequence: event.sequence,
                    timestamp_ms: event.timestamp_ms,
                }),
            }],
        }
    }
}
