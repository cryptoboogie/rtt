use pm_strategy::*;

/// A mock strategy that always fires on the first book update, then never again.
struct OneShotStrategy {
    fired: bool,
}

impl OneShotStrategy {
    fn new() -> Self {
        Self { fired: false }
    }
}

impl Strategy for OneShotStrategy {
    fn on_book_update(&mut self, snapshot: &OrderBookSnapshot) -> Option<TriggerMessage> {
        if self.fired {
            return None;
        }
        self.fired = true;
        Some(TriggerMessage {
            trigger_id: 1,
            token_id: snapshot.asset_id.clone(),
            side: Side::Buy,
            price: "0.50".to_string(),
            size: "10".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 0,
        })
    }

    fn on_trade(&mut self, _trade: &TradeEvent) -> Option<TriggerMessage> {
        None
    }

    fn name(&self) -> &str {
        "one-shot"
    }
}

fn make_snapshot(bid: &str, ask: &str) -> OrderBookSnapshot {
    OrderBookSnapshot {
        asset_id: "asset_1".to_string(),
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
fn strategy_trait_fires_once() {
    let mut strat = OneShotStrategy::new();
    assert_eq!(strat.name(), "one-shot");

    let snap = make_snapshot("0.44", "0.46");
    let trigger = strat.on_book_update(&snap);
    assert!(trigger.is_some());
    assert_eq!(trigger.unwrap().trigger_id, 1);

    // Second call should not fire
    let trigger2 = strat.on_book_update(&snap);
    assert!(trigger2.is_none());
}

#[test]
fn strategy_trait_on_trade_returns_none() {
    let mut strat = OneShotStrategy::new();
    let trade = TradeEvent {
        asset_id: "asset_1".to_string(),
        price: "0.50".to_string(),
        size: "5".to_string(),
        side: Side::Sell,
        timestamp_ms: 1700000000000,
    };
    assert!(strat.on_trade(&trade).is_none());
}

#[test]
fn strategy_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OneShotStrategy>();
}
