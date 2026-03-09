use crate::strategy::{
    IsolationPolicy, Strategy, StrategyDataRequirement, StrategyRequirements, StrategyRuntimeView,
    TriggerStrategy,
};
use crate::types::*;
use std::time::Instant;

/// Fires when the bid-ask spread narrows below a configured threshold.
pub struct SpreadStrategy {
    token_id: String,
    side: Side,
    max_spread: f64,
    size: String,
    order_type: OrderType,
    next_trigger_id: u64,
    epoch: Instant,
}

impl SpreadStrategy {
    pub fn new(
        token_id: String,
        side: Side,
        max_spread: f64,
        size: String,
        order_type: OrderType,
    ) -> Self {
        Self {
            token_id,
            side,
            max_spread,
            size,
            order_type,
            next_trigger_id: 1,
            epoch: Instant::now(),
        }
    }

    fn make_trigger(&mut self, price: &str) -> TriggerMessage {
        let id = self.next_trigger_id;
        self.next_trigger_id += 1;
        TriggerMessage {
            trigger_id: id,
            token_id: self.token_id.clone(),
            side: self.side,
            price: price.to_string(),
            size: self.size.clone(),
            order_type: self.order_type,
            timestamp_ns: self.epoch.elapsed().as_nanos() as u64,
        }
    }
}

impl Strategy for SpreadStrategy {
    fn on_book_update(&mut self, snapshot: &OrderBookSnapshot) -> Option<TriggerMessage> {
        if snapshot.asset_id != self.token_id {
            return None;
        }

        let bid = snapshot.best_bid.as_ref()?;
        let ask = snapshot.best_ask.as_ref()?;

        let bid_price: f64 = bid.price.parse().ok()?;
        let ask_price: f64 = ask.price.parse().ok()?;

        let spread = ask_price - bid_price;
        if spread < self.max_spread {
            let price = match self.side {
                Side::Buy => &ask.price,
                Side::Sell => &bid.price,
            };
            Some(self.make_trigger(price))
        } else {
            None
        }
    }

    fn on_trade(&mut self, _trade: &TradeEvent) -> Option<TriggerMessage> {
        None
    }

    fn name(&self) -> &str {
        "spread"
    }
}

impl TriggerStrategy for SpreadStrategy {
    fn requirements(&self) -> StrategyRequirements {
        StrategyRequirements::trigger(
            vec![StrategyDataRequirement::polymarket_bbo(
                self.token_id.clone(),
            )],
            IsolationPolicy::SharedFeedAcceptable,
        )
    }

    fn on_update(&mut self, view: &StrategyRuntimeView) -> Option<TriggerMessage> {
        let snapshot = view.snapshot(&self.token_id)?;
        self.on_book_update(&snapshot)
    }

    fn name(&self) -> &str {
        "spread"
    }
}
