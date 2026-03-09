use serde::{Deserialize, Serialize};

use crate::quote::DesiredQuotes;
use crate::types::{OrderBookSnapshot, PriceLevel, Side, TradeEvent, TriggerMessage};
use rtt_core::{HotBookLevel, HotBookState, HotReferenceState, UpdateNotice};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Trigger,
    Quote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationPolicy {
    SharedFeedAcceptable,
    DedicatedPreferred,
    DedicatedRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyDataRequirementKind {
    PolymarketBbo,
    PolymarketDepthTopN { levels: usize },
    ExternalReferencePrice,
    RecentTrades,
    RewardMetadata,
    Inventory,
    LiveOrderState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "selector", content = "value", rename_all = "snake_case")]
pub enum RequirementSelector {
    Asset(String),
    Symbol(String),
    Market(String),
    Source(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyDataRequirement {
    pub kind: StrategyDataRequirementKind,
    pub selector: RequirementSelector,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StrategyRequirements {
    pub execution_mode: ExecutionMode,
    pub isolation_policy: IsolationPolicy,
    pub data: Vec<StrategyDataRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryPosition {
    pub asset_id: String,
    pub side: Side,
    pub filled_size: String,
    pub net_notional: String,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryDelta {
    pub asset_id: String,
    pub side: Side,
    pub filled_size_delta: String,
    pub notional_delta: String,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyRuntimeView {
    notice: UpdateNotice,
    books: Vec<HotBookState>,
    references: Vec<HotReferenceState>,
    inventory: Vec<InventoryPosition>,
}

pub trait TriggerStrategy: Send + Sync {
    fn requirements(&self) -> StrategyRequirements;
    fn on_update(&mut self, view: &StrategyRuntimeView) -> Option<TriggerMessage>;
    fn name(&self) -> &str;
}

pub trait QuoteStrategy: Send + Sync {
    fn requirements(&self) -> StrategyRequirements;
    fn on_update(&mut self, view: &StrategyRuntimeView) -> Option<DesiredQuotes>;
    fn name(&self) -> &str;
}

/// Trait that all strategies must implement.
pub trait Strategy: Send + Sync {
    /// Called when the order book updates. Return a trigger to fire, or None.
    fn on_book_update(&mut self, snapshot: &OrderBookSnapshot) -> Option<TriggerMessage>;

    /// Called when a trade occurs. Return a trigger to fire, or None.
    fn on_trade(&mut self, trade: &TradeEvent) -> Option<TriggerMessage>;

    /// Human-readable name of this strategy.
    fn name(&self) -> &str;
}

impl StrategyDataRequirement {
    pub fn polymarket_bbo(asset_id: impl Into<String>) -> Self {
        Self {
            kind: StrategyDataRequirementKind::PolymarketBbo,
            selector: RequirementSelector::Asset(asset_id.into()),
        }
    }

    pub fn external_reference_price(symbol: impl Into<String>) -> Self {
        Self {
            kind: StrategyDataRequirementKind::ExternalReferencePrice,
            selector: RequirementSelector::Symbol(symbol.into()),
        }
    }
}

impl StrategyRequirements {
    pub fn trigger(data: Vec<StrategyDataRequirement>, isolation_policy: IsolationPolicy) -> Self {
        Self {
            execution_mode: ExecutionMode::Trigger,
            isolation_policy,
            data,
        }
    }

    pub fn quote(data: Vec<StrategyDataRequirement>, isolation_policy: IsolationPolicy) -> Self {
        Self {
            execution_mode: ExecutionMode::Quote,
            isolation_policy,
            data,
        }
    }
}

impl StrategyRuntimeView {
    pub fn new(
        notice: UpdateNotice,
        books: Vec<HotBookState>,
        references: Vec<HotReferenceState>,
        inventory: Vec<InventoryPosition>,
    ) -> Self {
        Self {
            notice,
            books,
            references,
            inventory,
        }
    }

    pub fn notice(&self) -> &UpdateNotice {
        &self.notice
    }

    pub fn books(&self) -> &[HotBookState] {
        &self.books
    }

    pub fn references(&self) -> &[HotReferenceState] {
        &self.references
    }

    pub fn inventory_positions(&self) -> &[InventoryPosition] {
        &self.inventory
    }

    pub fn book(&self, asset_id: &str) -> Option<&HotBookState> {
        self.books
            .iter()
            .find(|book| book.asset_id.as_str() == asset_id)
    }

    pub fn reference(&self, instrument_id: &str) -> Option<&HotReferenceState> {
        self.references
            .iter()
            .find(|reference| reference.notice.subject.instrument_id == instrument_id)
    }

    pub fn inventory(&self, asset_id: &str, side: Side) -> Option<&InventoryPosition> {
        self.inventory
            .iter()
            .find(|position| position.asset_id == asset_id && position.side == side)
    }

    pub fn snapshot(&self, asset_id: &str) -> Option<OrderBookSnapshot> {
        let book = self.book(asset_id)?;
        Some(OrderBookSnapshot {
            asset_id: book.asset_id.as_str().to_string(),
            best_bid: book.best_bid.as_ref().map(book_level_to_snapshot_level),
            best_ask: book.best_ask.as_ref().map(book_level_to_snapshot_level),
            timestamp_ms: book.timestamp_ms,
            hash: book
                .source_hash
                .clone()
                .or_else(|| book.notice.source_hash.clone())
                .unwrap_or_default(),
        })
    }

    pub fn primary_snapshot(&self) -> Option<OrderBookSnapshot> {
        self.books
            .first()
            .and_then(|book| self.snapshot(book.asset_id.as_str()))
    }
}

impl InventoryDelta {
    pub fn new(
        asset_id: impl Into<String>,
        side: Side,
        filled_size_delta: impl Into<String>,
        notional_delta: impl Into<String>,
        observed_at_ms: u64,
    ) -> Self {
        Self {
            asset_id: asset_id.into(),
            side,
            filled_size_delta: filled_size_delta.into(),
            notional_delta: notional_delta.into(),
            observed_at_ms,
        }
    }
}

fn book_level_to_snapshot_level(level: &HotBookLevel) -> PriceLevel {
    PriceLevel {
        price: level.price.exact.clone(),
        size: level.size.exact.clone(),
    }
}
