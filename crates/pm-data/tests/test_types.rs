use pm_data::types::*;
use rtt_core::public_event::NormalizedUpdatePayload;

#[test]
fn deserialize_book_event() {
    let json = r#"{
        "event_type": "book",
        "asset_id": "123456",
        "market": "0xabcdef",
        "timestamp": "1700000000000",
        "bids": [
            {"price": "0.55", "size": "100"},
            {"price": "0.54", "size": "200"}
        ],
        "asks": [
            {"price": "0.56", "size": "150"},
            {"price": "0.57", "size": "250"}
        ],
        "hash": "abc123"
    }"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    match msg {
        WsMessage::Book(book) => {
            assert_eq!(book.asset_id, "123456");
            assert_eq!(book.market, "0xabcdef");
            assert_eq!(book.timestamp, "1700000000000");
            assert_eq!(book.bids.len(), 2);
            assert_eq!(book.asks.len(), 2);
            assert_eq!(book.bids[0].price, "0.55");
            assert_eq!(book.bids[0].size, "100");
            assert_eq!(book.asks[0].price, "0.56");
            assert_eq!(book.hash, Some("abc123".to_string()));
        }
        _ => panic!("Expected Book event"),
    }
}

#[test]
fn deserialize_book_event_no_hash() {
    let json = r#"{
        "event_type": "book",
        "asset_id": "123456",
        "market": "0xabcdef",
        "timestamp": "1700000000000",
        "bids": [],
        "asks": []
    }"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    match msg {
        WsMessage::Book(book) => {
            assert!(book.hash.is_none());
            assert!(book.bids.is_empty());
            assert!(book.asks.is_empty());
        }
        _ => panic!("Expected Book event"),
    }
}

#[test]
fn deserialize_price_change_event() {
    let json = r#"{
        "event_type": "price_change",
        "market": "0xabcdef",
        "timestamp": "1700000000000",
        "price_changes": [
            {
                "asset_id": "123456",
                "price": "0.55",
                "size": "100",
                "side": "BUY",
                "hash": "hash123",
                "best_bid": "0.55",
                "best_ask": "0.56"
            }
        ]
    }"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    match msg {
        WsMessage::PriceChange(pc) => {
            assert_eq!(pc.market, "0xabcdef");
            assert_eq!(pc.price_changes.len(), 1);
            let entry = &pc.price_changes[0];
            assert_eq!(entry.asset_id, "123456");
            assert_eq!(entry.price, "0.55");
            assert_eq!(entry.size, Some("100".to_string()));
            assert_eq!(entry.side, Side::Buy);
            assert_eq!(entry.hash, Some("hash123".to_string()));
            assert_eq!(entry.best_bid, Some("0.55".to_string()));
            assert_eq!(entry.best_ask, Some("0.56".to_string()));
        }
        _ => panic!("Expected PriceChange event"),
    }
}

#[test]
fn deserialize_price_change_size_zero_removal() {
    let json = r#"{
        "event_type": "price_change",
        "market": "0xabcdef",
        "timestamp": "1700000000000",
        "price_changes": [
            {
                "asset_id": "123456",
                "price": "0.55",
                "size": "0",
                "side": "SELL"
            }
        ]
    }"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    match msg {
        WsMessage::PriceChange(pc) => {
            let entry = &pc.price_changes[0];
            assert_eq!(entry.size, Some("0".to_string()));
            assert_eq!(entry.side, Side::Sell);
        }
        _ => panic!("Expected PriceChange event"),
    }
}

#[test]
fn deserialize_last_trade_price_event() {
    let json = r#"{
        "event_type": "last_trade_price",
        "asset_id": "123456",
        "market": "0xabcdef",
        "price": "0.55",
        "side": "BUY",
        "size": "50",
        "fee_rate_bps": "200",
        "timestamp": "1700000000000"
    }"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    match msg {
        WsMessage::LastTradePrice(ltp) => {
            assert_eq!(ltp.asset_id, "123456");
            assert_eq!(ltp.price, "0.55");
            assert_eq!(ltp.side, Some(Side::Buy));
            assert_eq!(ltp.size, Some("50".to_string()));
        }
        _ => panic!("Expected LastTradePrice event"),
    }
}

#[test]
fn deserialize_tick_size_change_event() {
    let json = r#"{
        "event_type": "tick_size_change",
        "asset_id": "123456",
        "market": "0xabcdef",
        "old_tick_size": "0.01",
        "new_tick_size": "0.001",
        "timestamp": "1700000000000"
    }"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    match msg {
        WsMessage::TickSizeChange(tsc) => {
            assert_eq!(tsc.asset_id, "123456");
            assert_eq!(tsc.old_tick_size, "0.01");
            assert_eq!(tsc.new_tick_size, "0.001");
        }
        _ => panic!("Expected TickSizeChange event"),
    }
}

#[test]
fn deserialize_best_bid_ask_event() {
    let json = r#"{
        "event_type": "best_bid_ask",
        "asset_id": "123456",
        "market": "0xabcdef",
        "best_bid": "0.55",
        "best_ask": "0.56",
        "spread": "0.01",
        "timestamp": "1700000000000"
    }"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    match msg {
        WsMessage::BestBidAsk(bba) => {
            assert_eq!(bba.asset_id, "123456");
            assert_eq!(bba.best_bid, "0.55");
            assert_eq!(bba.best_ask, "0.56");
            assert_eq!(bba.spread, "0.01");
        }
        _ => panic!("Expected BestBidAsk event"),
    }
}

#[test]
fn side_deserialize_aliases() {
    let buy: Side = serde_json::from_str(r#""BUY""#).unwrap();
    assert_eq!(buy, Side::Buy);
    let sell: Side = serde_json::from_str(r#""SELL""#).unwrap();
    assert_eq!(sell, Side::Sell);
    let buy2: Side = serde_json::from_str(r#""Buy""#).unwrap();
    assert_eq!(buy2, Side::Buy);
}

#[test]
fn order_book_snapshot_construction() {
    let snap = OrderBookSnapshot {
        asset_id: "123".to_string(),
        best_bid: Some(PriceLevel {
            price: "0.55".to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: "0.56".to_string(),
            size: "200".to_string(),
        }),
        timestamp_ms: 1700000000000,
        hash: "abc".to_string(),
    };
    assert_eq!(snap.best_bid.as_ref().unwrap().price, "0.55");
    assert_eq!(snap.best_ask.as_ref().unwrap().size, "200");
}

#[test]
fn book_events_normalize_into_shared_market_updates() {
    let json = r#"{
        "event_type": "book",
        "asset_id": "123456",
        "market": "0xabcdef",
        "timestamp": "1700000000000",
        "bids": [
            {"price": "0.55", "size": "100"}
        ],
        "asks": [
            {"price": "0.56", "size": "150"}
        ],
        "hash": "book-hash"
    }"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    let updates = msg.to_normalized_updates();

    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].notice.source_id, polymarket_public_source_id());
    assert_eq!(
        updates[0].notice.source_kind,
        rtt_core::SourceKind::PolymarketWs
    );
    assert_eq!(updates[0].notice.version, 1_700_000_000_000);
    assert_eq!(updates[0].notice.source_hash.as_deref(), Some("book-hash"));

    match &updates[0].payload {
        NormalizedUpdatePayload::BookSnapshot(snapshot) => {
            assert_eq!(snapshot.market_id.as_str(), "0xabcdef");
            assert_eq!(snapshot.asset_id.as_str(), "123456");
            assert_eq!(snapshot.bids[0].price.as_str(), "0.55");
            assert_eq!(snapshot.asks[0].size.as_str(), "150");
        }
        other => panic!("expected book snapshot, got {other:?}"),
    }
}

#[test]
fn price_change_and_bbo_events_normalize_with_exact_values() {
    let price_change_json = r#"{
        "event_type": "price_change",
        "market": "0xabcdef",
        "timestamp": "1700000000000",
        "price_changes": [
            {
                "asset_id": "123456",
                "price": "0.55",
                "size": "100",
                "side": "BUY",
                "hash": "delta-hash",
                "best_bid": "0.55",
                "best_ask": "0.56"
            }
        ]
    }"#;
    let best_bid_ask_json = r#"{
        "event_type": "best_bid_ask",
        "asset_id": "123456",
        "market": "0xabcdef",
        "best_bid": "0.55",
        "best_ask": "0.56",
        "spread": "0.01",
        "timestamp": "1700000000001"
    }"#;

    let price_change: WsMessage = serde_json::from_str(price_change_json).unwrap();
    let bbo: WsMessage = serde_json::from_str(best_bid_ask_json).unwrap();

    let delta_updates = price_change.to_normalized_updates();
    let bbo_updates = bbo.to_normalized_updates();

    match &delta_updates[0].payload {
        NormalizedUpdatePayload::BookDelta(delta) => {
            assert_eq!(delta.asset_id.as_str(), "123456");
            assert_eq!(delta.price.as_str(), "0.55");
            assert_eq!(delta.size.as_str(), "100");
            assert_eq!(delta.best_ask.as_ref().unwrap().as_str(), "0.56");
        }
        other => panic!("expected delta update, got {other:?}"),
    }

    match &bbo_updates[0].payload {
        NormalizedUpdatePayload::BestBidAsk(bbo) => {
            assert_eq!(bbo.best_bid.as_str(), "0.55");
            assert_eq!(bbo.best_ask.as_str(), "0.56");
            assert_eq!(bbo.spread.as_ref().unwrap().as_str(), "0.01");
        }
        other => panic!("expected best bid/ask update, got {other:?}"),
    }
}

#[test]
fn reconnect_events_normalize_into_source_scoped_notices() {
    let msg = WsMessage::Reconnected(ReconnectEvent {
        sequence: 7,
        timestamp_ms: 1_700_000_000_123,
    });

    let updates = msg.to_normalized_updates();

    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].notice.source_id, polymarket_public_source_id());
    assert_eq!(
        updates[0].notice.source_kind,
        rtt_core::SourceKind::PolymarketWs
    );
    assert_eq!(updates[0].notice.version, 7);
    assert_eq!(updates[0].notice.subject.instrument_id, "_source");

    match &updates[0].payload {
        NormalizedUpdatePayload::Reconnect(reconnect) => {
            assert_eq!(reconnect.sequence, 7);
            assert_eq!(reconnect.timestamp_ms, 1_700_000_000_123);
        }
        other => panic!("expected reconnect update, got {other:?}"),
    }
}
