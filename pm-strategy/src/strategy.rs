use crate::types::{OrderBookSnapshot, TradeEvent, TriggerMessage};

/// Trait that all strategies must implement.
pub trait Strategy: Send + Sync {
    /// Called when the order book updates. Return a trigger to fire, or None.
    fn on_book_update(&mut self, snapshot: &OrderBookSnapshot) -> Option<TriggerMessage>;

    /// Called when a trade occurs. Return a trigger to fire, or None.
    fn on_trade(&mut self, trade: &TradeEvent) -> Option<TriggerMessage>;

    /// Human-readable name of this strategy.
    fn name(&self) -> &str;
}
