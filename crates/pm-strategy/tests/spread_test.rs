use pm_strategy::spread::SpreadStrategy;
use pm_strategy::*;

fn make_snapshot(bid: &str, ask: &str) -> OrderBookSnapshot {
    OrderBookSnapshot {
        asset_id: "token_abc".to_string(),
        best_bid: Some(PriceLevel {
            price: bid.to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: ask.to_string(),
            size: "100".to_string(),
        }),
        timestamp_ms: 1700000000000,
        hash: "h".to_string(),
    }
}

#[test]
fn spread_no_fire_when_spread_wide() {
    // Fire when spread narrows below 0.02
    let mut strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.02,
        "50".to_string(),
        OrderType::FOK,
    );

    // Spread = 0.46 - 0.44 = 0.02, not strictly below threshold
    let snap = make_snapshot("0.44", "0.46");
    assert!(strat.on_book_update(&snap).is_none());
}

#[test]
fn spread_fires_when_spread_narrows() {
    let mut strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.02,
        "50".to_string(),
        OrderType::FOK,
    );

    // Spread = 0.455 - 0.445 = 0.01, below threshold
    let snap = make_snapshot("0.445", "0.455");
    let trigger = strat.on_book_update(&snap);
    assert!(trigger.is_some());
    let t = trigger.unwrap();
    assert_eq!(t.side, Side::Buy);
    assert_eq!(t.token_id, "token_abc");
    assert_eq!(t.size, "50");
}

#[test]
fn spread_buy_uses_ask_price() {
    let mut strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.05,
        "10".to_string(),
        OrderType::FOK,
    );

    let snap = make_snapshot("0.48", "0.49");
    let t = strat.on_book_update(&snap).unwrap();
    // Buy side should use the ask price
    assert_eq!(t.price, "0.49");
}

#[test]
fn spread_sell_uses_bid_price() {
    let mut strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Sell,
        0.05,
        "10".to_string(),
        OrderType::GTC,
    );

    let snap = make_snapshot("0.48", "0.49");
    let t = strat.on_book_update(&snap).unwrap();
    // Sell side should use the bid price
    assert_eq!(t.price, "0.48");
}

#[test]
fn spread_no_fire_on_empty_book() {
    let mut strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.05,
        "50".to_string(),
        OrderType::FOK,
    );

    let snap = OrderBookSnapshot {
        asset_id: "token_abc".to_string(),
        best_bid: None,
        best_ask: None,
        timestamp_ms: 0,
        hash: "".to_string(),
    };
    assert!(strat.on_book_update(&snap).is_none());
}

#[test]
fn spread_no_fire_on_one_sided_book() {
    let mut strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.05,
        "50".to_string(),
        OrderType::FOK,
    );

    let snap = OrderBookSnapshot {
        asset_id: "token_abc".to_string(),
        best_bid: Some(PriceLevel {
            price: "0.44".to_string(),
            size: "100".to_string(),
        }),
        best_ask: None,
        timestamp_ms: 0,
        hash: "".to_string(),
    };
    assert!(strat.on_book_update(&snap).is_none());
}

#[test]
fn spread_ignores_wrong_asset() {
    let mut strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.05,
        "50".to_string(),
        OrderType::FOK,
    );

    let snap = OrderBookSnapshot {
        asset_id: "token_xyz".to_string(),
        best_bid: Some(PriceLevel {
            price: "0.49".to_string(),
            size: "100".to_string(),
        }),
        best_ask: Some(PriceLevel {
            price: "0.50".to_string(),
            size: "100".to_string(),
        }),
        timestamp_ms: 0,
        hash: "".to_string(),
    };
    assert!(strat.on_book_update(&snap).is_none());
}

#[test]
fn spread_name() {
    let strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.02,
        "50".to_string(),
        OrderType::FOK,
    );
    assert_eq!(strat.name(), "spread");
}

#[test]
fn spread_increments_trigger_id() {
    let mut strat = SpreadStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.05,
        "10".to_string(),
        OrderType::FOK,
    );

    let snap = make_snapshot("0.48", "0.49");
    let t1 = strat.on_book_update(&snap).unwrap();
    let t2 = strat.on_book_update(&snap).unwrap();
    assert_eq!(t1.trigger_id + 1, t2.trigger_id);
}
