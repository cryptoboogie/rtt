use crate::strategy::Strategy;
use crate::types::*;
use std::time::Instant;

/// Fires when best_ask drops to/below threshold (Buy side)
/// or best_bid rises to/above threshold (Sell side).
pub struct ThresholdStrategy {
    token_id: String,
    side: Side,
    threshold: f64,
    size: String,
    order_type: OrderType,
    next_trigger_id: u64,
    epoch: Instant,
}

impl ThresholdStrategy {
    pub fn new(
        token_id: String,
        side: Side,
        threshold: f64,
        size: String,
        order_type: OrderType,
    ) -> Self {
        Self {
            token_id,
            side,
            threshold,
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

impl Strategy for ThresholdStrategy {
    fn on_book_update(&mut self, snapshot: &OrderBookSnapshot) -> Option<TriggerMessage> {
        if snapshot.asset_id != self.token_id {
            return None;
        }

        match self.side {
            Side::Buy => {
                // Fire when best_ask drops to or below threshold
                let ask = snapshot.best_ask.as_ref()?;
                let ask_price: f64 = ask.price.parse().ok()?;
                if ask_price <= self.threshold {
                    Some(self.make_trigger(&ask.price))
                } else {
                    None
                }
            }
            Side::Sell => {
                // Fire when best_bid rises to or above threshold
                let bid = snapshot.best_bid.as_ref()?;
                let bid_price: f64 = bid.price.parse().ok()?;
                if bid_price >= self.threshold {
                    Some(self.make_trigger(&bid.price))
                } else {
                    None
                }
            }
        }
    }

    fn on_trade(&mut self, _trade: &TradeEvent) -> Option<TriggerMessage> {
        None
    }

    fn name(&self) -> &str {
        "threshold"
    }
}
