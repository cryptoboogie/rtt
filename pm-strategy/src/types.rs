use serde::{Deserialize, Serialize};

/// Side of an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// Order type for Polymarket CLOB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    GTC,
    GTD,
    FOK,
    FAK,
}

/// Trigger message produced by strategies and consumed by the executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerMessage {
    pub trigger_id: u64,
    pub token_id: String,
    pub side: Side,
    pub price: String,
    pub size: String,
    pub order_type: OrderType,
    pub timestamp_ns: u64,
}

/// A single price level in the order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: String,
    pub size: String,
}

/// Snapshot of the top-of-book for a given asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookSnapshot {
    pub asset_id: String,
    pub best_bid: Option<PriceLevel>,
    pub best_ask: Option<PriceLevel>,
    pub timestamp_ms: u64,
    pub hash: String,
}

/// A trade event from the market data feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeEvent {
    pub asset_id: String,
    pub price: String,
    pub size: String,
    pub side: Side,
    pub timestamp_ms: u64,
}
