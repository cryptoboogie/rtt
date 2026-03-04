use pm_data::types::WsMessage;

#[test]
fn parse_array_book_snapshot() {
    // Real format: initial dump comes as array
    let json = r#"[{"event_type":"book","market":"0xabc","asset_id":"123","timestamp":"1700000000000","hash":"abc123","bids":[{"price":"0.55","size":"100"}],"asks":[{"price":"0.56","size":"150"}],"last_trade_price":"0.55","tick_size":"0.01"}]"#;

    let msgs: Vec<WsMessage> = serde_json::from_str(json).unwrap();
    assert_eq!(msgs.len(), 1);
    match &msgs[0] {
        WsMessage::Book(book) => {
            assert_eq!(book.asset_id, "123");
            assert_eq!(book.bids.len(), 1);
            assert_eq!(book.asks.len(), 1);
            assert_eq!(book.hash, Some("abc123".to_string()));
        }
        _ => panic!("Expected Book"),
    }
}

#[test]
fn parse_single_price_change() {
    let json = r#"{"event_type":"price_change","market":"0xabc","timestamp":"1700000000000","price_changes":[{"asset_id":"123","price":"0.55","size":"100","side":"BUY","hash":"hash1","best_bid":"0.55","best_ask":"0.56"}]}"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    match msg {
        WsMessage::PriceChange(pc) => {
            assert_eq!(pc.price_changes.len(), 1);
            assert_eq!(pc.price_changes[0].asset_id, "123");
        }
        _ => panic!("Expected PriceChange"),
    }
}

#[test]
fn parse_book_with_extra_fields_ignored() {
    // Real API sends extra fields like last_trade_price, tick_size
    let json = r#"{"event_type":"book","market":"0xabc","asset_id":"123","timestamp":"1700000000000","bids":[],"asks":[],"hash":"h","last_trade_price":"0.55","tick_size":"0.01","some_future_field":"whatever"}"#;

    let msg: WsMessage = serde_json::from_str(json).unwrap();
    assert!(matches!(msg, WsMessage::Book(_)));
}
