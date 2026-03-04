use pm_strategy::*;

#[test]
fn trigger_message_roundtrip_json() {
    let msg = TriggerMessage {
        trigger_id: 42,
        token_id: "0xabc123".to_string(),
        side: Side::Buy,
        price: "0.45".to_string(),
        size: "100".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: 1_000_000,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: TriggerMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.trigger_id, 42);
    assert_eq!(parsed.token_id, "0xabc123");
    assert_eq!(parsed.side, Side::Buy);
    assert_eq!(parsed.price, "0.45");
    assert_eq!(parsed.size, "100");
    assert_eq!(parsed.order_type, OrderType::FOK);
    assert_eq!(parsed.timestamp_ns, 1_000_000);
}

#[test]
fn order_book_snapshot_with_levels() {
    let snap = OrderBookSnapshot {
        asset_id: "asset_1".to_string(),
        best_bid: Some(PriceLevel {
            price: "0.44".to_string(),
            size: "200".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: "0.46".to_string(),
            size: "150".to_string(),
        }),
        timestamp_ms: 1700000000000,
        hash: "abc".to_string(),
    };
    assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.44");
    assert_eq!(snap.best_ask.as_ref().unwrap().price, "0.46");
}

#[test]
fn order_book_snapshot_empty_book() {
    let snap = OrderBookSnapshot {
        asset_id: "asset_2".to_string(),
        best_bid: None,
        best_ask: None,
        timestamp_ms: 0,
        hash: "".to_string(),
    };
    assert!(snap.best_bid.is_none());
    assert!(snap.best_ask.is_none());
}

#[test]
fn trade_event_serde() {
    let trade = TradeEvent {
        asset_id: "asset_1".to_string(),
        price: "0.50".to_string(),
        size: "10".to_string(),
        side: Side::Sell,
        timestamp_ms: 1700000000000,
    };
    let json = serde_json::to_string(&trade).unwrap();
    let parsed: TradeEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.side, Side::Sell);
    assert_eq!(parsed.price, "0.50");
}

#[test]
fn side_and_order_type_variants() {
    assert_ne!(Side::Buy, Side::Sell);
    assert_ne!(OrderType::GTC, OrderType::FOK);
    assert_ne!(OrderType::GTD, OrderType::FAK);
}
