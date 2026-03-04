use serde::Deserialize;

// Re-export shared types from rtt-core
pub use rtt_core::trigger::{OrderBookSnapshot, OrderType, PriceLevel, Side, TriggerMessage};

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
