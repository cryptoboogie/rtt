use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    GTC,
    GTD,
    FOK,
    FAK,
}

/// Trigger message sent from strategy to executor.
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

/// Single price level in an order book.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: String,
    pub size: String,
}

/// Order book snapshot for a single asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBookSnapshot {
    pub asset_id: String,
    pub best_bid: Option<PriceLevel>,
    pub best_ask: Option<PriceLevel>,
    pub timestamp_ms: u64,
    pub hash: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_message_construction() {
        let msg = TriggerMessage {
            trigger_id: 1,
            token_id: "0xabc".to_string(),
            side: Side::Buy,
            price: "0.45".to_string(),
            size: "100".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 123456789,
        };
        assert_eq!(msg.trigger_id, 1);
        assert_eq!(msg.side, Side::Buy);
        assert_eq!(msg.order_type, OrderType::FOK);
    }

    #[test]
    fn trigger_message_serializes() {
        let msg = TriggerMessage {
            trigger_id: 42,
            token_id: "tok123".to_string(),
            side: Side::Sell,
            price: "0.75".to_string(),
            size: "50".to_string(),
            order_type: OrderType::GTC,
            timestamp_ns: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: TriggerMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.trigger_id, 42);
        assert_eq!(back.side, Side::Sell);
    }

    #[test]
    fn order_book_snapshot_construction() {
        let snap = OrderBookSnapshot {
            asset_id: "asset1".to_string(),
            best_bid: Some(PriceLevel {
                price: "0.40".to_string(),
                size: "200".to_string(),
            }),
            best_ask: Some(PriceLevel {
                price: "0.45".to_string(),
                size: "150".to_string(),
            }),
            timestamp_ms: 1000,
            hash: "abc123".to_string(),
        };
        assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.40");
        assert_eq!(snap.best_ask.as_ref().unwrap().price, "0.45");
    }

    #[test]
    fn side_and_order_type_variants() {
        assert_ne!(Side::Buy, Side::Sell);
        assert_ne!(OrderType::FOK, OrderType::GTC);
        assert_ne!(OrderType::GTD, OrderType::FAK);
    }
}
