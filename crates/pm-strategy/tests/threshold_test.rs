use pm_strategy::*;
use pm_strategy::threshold::ThresholdStrategy;

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

fn make_empty_snapshot() -> OrderBookSnapshot {
    OrderBookSnapshot {
        asset_id: "token_abc".to_string(),
        best_bid: None,
        best_ask: None,
        timestamp_ms: 1700000000000,
        hash: "h".to_string(),
    }
}

#[test]
fn threshold_no_fire_below_bid_threshold() {
    // Strategy: fire buy when best_ask drops to 0.40 or below
    let mut strat = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.40,
        "50".to_string(),
        OrderType::FOK,
    );

    // Ask is 0.45, above threshold — should not fire
    let snap = make_snapshot("0.44", "0.45");
    assert!(strat.on_book_update(&snap).is_none());
}

#[test]
fn threshold_fires_when_ask_crosses_down() {
    let mut strat = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.40,
        "50".to_string(),
        OrderType::FOK,
    );

    // Ask drops to 0.39 — below threshold, should fire buy
    let snap = make_snapshot("0.38", "0.39");
    let trigger = strat.on_book_update(&snap);
    assert!(trigger.is_some());
    let t = trigger.unwrap();
    assert_eq!(t.side, Side::Buy);
    assert_eq!(t.price, "0.39");
    assert_eq!(t.size, "50");
    assert_eq!(t.token_id, "token_abc");
}

#[test]
fn threshold_fires_when_bid_crosses_up() {
    // Strategy: fire sell when best_bid rises to 0.60 or above
    let mut strat = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Sell,
        0.60,
        "25".to_string(),
        OrderType::GTC,
    );

    // Bid at 0.55 — below threshold, no fire
    let snap = make_snapshot("0.55", "0.58");
    assert!(strat.on_book_update(&snap).is_none());

    // Bid rises to 0.61 — above threshold, should fire sell
    let snap2 = make_snapshot("0.61", "0.63");
    let trigger = strat.on_book_update(&snap2);
    assert!(trigger.is_some());
    let t = trigger.unwrap();
    assert_eq!(t.side, Side::Sell);
    assert_eq!(t.price, "0.61");
    assert_eq!(t.size, "25");
    assert_eq!(t.order_type, OrderType::GTC);
}

#[test]
fn threshold_does_not_fire_on_empty_book() {
    let mut strat = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.40,
        "50".to_string(),
        OrderType::FOK,
    );

    let snap = make_empty_snapshot();
    assert!(strat.on_book_update(&snap).is_none());
}

#[test]
fn threshold_fires_on_exact_price() {
    let mut strat = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.40,
        "50".to_string(),
        OrderType::FOK,
    );

    // Ask exactly at threshold
    let snap = make_snapshot("0.39", "0.40");
    let trigger = strat.on_book_update(&snap);
    assert!(trigger.is_some());
}

#[test]
fn threshold_ignores_wrong_asset() {
    let mut strat = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.40,
        "50".to_string(),
        OrderType::FOK,
    );

    let snap = OrderBookSnapshot {
        asset_id: "token_xyz".to_string(),
        best_bid: Some(PriceLevel { price: "0.30".to_string(), size: "100".to_string() }),
        best_ask: Some(PriceLevel { price: "0.35".to_string(), size: "100".to_string() }),
        timestamp_ms: 1700000000000,
        hash: "h".to_string(),
    };
    assert!(strat.on_book_update(&snap).is_none());
}

#[test]
fn threshold_name() {
    let strat = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.40,
        "50".to_string(),
        OrderType::FOK,
    );
    assert_eq!(strat.name(), "threshold");
}

#[test]
fn threshold_increments_trigger_id() {
    let mut strat = ThresholdStrategy::new(
        "token_abc".to_string(),
        Side::Buy,
        0.50,
        "10".to_string(),
        OrderType::FOK,
    );

    let snap = make_snapshot("0.44", "0.45");
    let t1 = strat.on_book_update(&snap).unwrap();
    let t2 = strat.on_book_update(&snap).unwrap();
    assert_eq!(t1.trigger_id + 1, t2.trigger_id);
}
