use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::feed_source::{InstrumentRef, SourceId};
use crate::market::{AssetId, MarketId, MarketMeta, MinOrderSize, TickSize};
use crate::public_event::{
    BestBidAskUpdate, BookLevel, BookSnapshotUpdate, NormalizedUpdate, NormalizedUpdatePayload,
    ReconnectUpdate, ReferencePriceUpdate, SourceStatusUpdate, TickSizeChangeUpdate,
    TradeTickUpdate, UpdateKind, UpdateNotice,
};
use crate::trigger::{OrderBookSnapshot, PriceLevel};

pub const HOT_VALUE_SCALE: u64 = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotStateValue {
    pub exact: String,
    pub units: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotBookLevel {
    pub price: HotStateValue,
    pub size: HotStateValue,
    pub price_ticks: Option<u64>,
    pub size_lots: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotBookState {
    pub notice: UpdateNotice,
    pub market_id: Option<MarketId>,
    pub asset_id: AssetId,
    pub best_bid: Option<HotBookLevel>,
    pub best_ask: Option<HotBookLevel>,
    pub midpoint: Option<HotStateValue>,
    pub tick_size: Option<TickSize>,
    pub tick_size_units: Option<u64>,
    pub lot_size: Option<MinOrderSize>,
    pub lot_size_units: Option<u64>,
    pub version: u64,
    pub timestamp_ms: u64,
    pub source_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotReferenceState {
    pub notice: UpdateNotice,
    pub reference_price: Option<HotStateValue>,
    pub best_bid: Option<HotStateValue>,
    pub best_ask: Option<HotStateValue>,
    pub last_trade_price: Option<HotStateValue>,
    pub last_trade_size: Option<HotStateValue>,
    pub healthy: Option<bool>,
    pub stale_after_ms: Option<u64>,
    pub observed_at_ms: Option<u64>,
    pub reconnect_sequence: Option<u64>,
    pub version: u64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotStateView {
    Book(HotBookState),
    Reference(HotReferenceState),
}

#[derive(Debug, Clone)]
pub enum NoticeResolution {
    OrderBook(OrderBookSnapshot),
    BestBidAsk(BestBidAskUpdate),
    TradeTick(TradeTickUpdate),
    ReferencePrice(ReferencePriceUpdate),
    TickSizeChange(TickSizeChangeUpdate),
    SourceStatus(SourceStatusUpdate),
    Reconnect(ReconnectUpdate),
}

#[derive(Clone, Default)]
pub struct HotStateStore {
    inner: Arc<RwLock<HotStateInner>>,
}

#[derive(Default)]
struct HotStateInner {
    markets_by_asset: HashMap<String, RegisteredMarket>,
    books: HashMap<HotStateKey, HotBookState>,
    references: HashMap<HotStateKey, HotReferenceState>,
}

#[derive(Debug, Clone)]
struct RegisteredMarket {
    market_id: MarketId,
    tick_size: TickSize,
    tick_size_units: u64,
    lot_size: Option<MinOrderSize>,
    lot_size_units: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct HotStateKey {
    source_id: String,
    instrument_kind: String,
    instrument_id: String,
}

impl HotStateStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_market(&self, market: &MarketMeta) {
        let tick_size_units = parse_scaled(market.tick_size.as_str()).unwrap_or(0);
        let lot_size_units = market
            .min_order_size
            .as_ref()
            .and_then(|value| parse_scaled(value.as_str()));

        let registered = RegisteredMarket {
            market_id: market.market_id.clone(),
            tick_size: market.tick_size.clone(),
            tick_size_units,
            lot_size: market.min_order_size.clone(),
            lot_size_units,
        };

        let mut inner = self.inner.write().unwrap();
        for asset in [&market.yes_asset.asset_id, &market.no_asset.asset_id] {
            inner
                .markets_by_asset
                .insert(asset.as_str().to_string(), registered.clone());
            for book in inner
                .books
                .values_mut()
                .filter(|book| book.asset_id.as_str() == asset.as_str())
            {
                apply_registered_market(book, &registered);
            }
        }
    }

    pub fn apply_update(&self, update: &NormalizedUpdate) {
        let mut inner = self.inner.write().unwrap();
        match &update.payload {
            NormalizedUpdatePayload::BookSnapshot(snapshot) => {
                apply_book_snapshot_update(&mut inner, &update.notice, snapshot);
            }
            NormalizedUpdatePayload::BookDelta(delta) => {
                apply_book_delta_update(&mut inner, &update.notice, delta);
            }
            NormalizedUpdatePayload::BestBidAsk(best_bid_ask) => {
                apply_best_bid_ask_update(&mut inner, &update.notice, best_bid_ask);
            }
            NormalizedUpdatePayload::TradeTick(trade_tick) => {
                apply_trade_tick_update(&mut inner, &update.notice, trade_tick);
            }
            NormalizedUpdatePayload::ReferencePrice(reference_price) => {
                apply_reference_price_update(&mut inner, &update.notice, reference_price);
            }
            NormalizedUpdatePayload::TickSizeChange(tick_size_change) => {
                apply_tick_size_change_update(&mut inner, &update.notice, tick_size_change);
            }
            NormalizedUpdatePayload::Reconnect(reconnect) => {
                apply_reconnect_update(&mut inner, &update.notice, reconnect);
            }
            NormalizedUpdatePayload::SourceStatus(source_status) => {
                apply_source_status_update(&mut inner, &update.notice, source_status);
            }
        }
    }

    pub fn apply_resolution(&self, notice: &UpdateNotice, resolution: &NoticeResolution) {
        let mut inner = self.inner.write().unwrap();
        match resolution {
            NoticeResolution::OrderBook(snapshot) => {
                apply_orderbook_snapshot_resolution(&mut inner, notice, snapshot);
            }
            NoticeResolution::BestBidAsk(best_bid_ask) => {
                apply_best_bid_ask_update(&mut inner, notice, best_bid_ask);
            }
            NoticeResolution::TradeTick(trade_tick) => {
                apply_trade_tick_update(&mut inner, notice, trade_tick);
            }
            NoticeResolution::ReferencePrice(reference_price) => {
                apply_reference_price_update(&mut inner, notice, reference_price);
            }
            NoticeResolution::TickSizeChange(tick_size_change) => {
                apply_tick_size_change_update(&mut inner, notice, tick_size_change);
            }
            NoticeResolution::SourceStatus(source_status) => {
                apply_source_status_update(&mut inner, notice, source_status);
            }
            NoticeResolution::Reconnect(reconnect) => {
                apply_reconnect_update(&mut inner, notice, reconnect);
            }
        }
    }

    pub fn book_state(&self, source_id: &SourceId, asset_id: &str) -> Option<HotBookState> {
        let inner = self.inner.read().unwrap();
        inner.books.get(&HotStateKey::asset(source_id, asset_id)).cloned()
    }

    pub fn reference_state(&self, subject: &InstrumentRef) -> Option<HotReferenceState> {
        let inner = self.inner.read().unwrap();
        inner.references.get(&HotStateKey::from_subject(subject)).cloned()
    }

    pub fn resolve_notice(&self, notice: &UpdateNotice) -> Option<HotStateView> {
        match notice.kind {
            UpdateKind::BookSnapshot
            | UpdateKind::BookDelta
            | UpdateKind::BestBidAsk
            | UpdateKind::TickSizeChange => self.resolve_book_notice(notice).map(HotStateView::Book),
            UpdateKind::TradeTick
            | UpdateKind::ReferencePrice
            | UpdateKind::Reconnect
            | UpdateKind::SourceStatus
            | UpdateKind::Custom => self
                .reference_state(&notice.subject)
                .map(HotStateView::Reference),
        }
    }

    pub fn project_snapshot(&self, notice: &UpdateNotice) -> Option<OrderBookSnapshot> {
        self.resolve_book_notice(notice).map(|state| OrderBookSnapshot {
            asset_id: state.asset_id.as_str().to_string(),
            best_bid: state.best_bid.as_ref().map(book_level_to_snapshot_level),
            best_ask: state.best_ask.as_ref().map(book_level_to_snapshot_level),
            timestamp_ms: state.timestamp_ms,
            hash: state
                .source_hash
                .clone()
                .or_else(|| notice.source_hash.clone())
                .unwrap_or_default(),
        })
    }

    fn resolve_book_notice(&self, notice: &UpdateNotice) -> Option<HotBookState> {
        let inner = self.inner.read().unwrap();
        inner
            .books
            .get(&HotStateKey::asset(
                &notice.source_id,
                &notice.subject.instrument_id,
            ))
            .cloned()
    }
}

impl HotStateKey {
    fn from_subject(subject: &InstrumentRef) -> Self {
        Self {
            source_id: subject.source_id.as_str().to_string(),
            instrument_kind: format!("{:?}", subject.kind),
            instrument_id: subject.instrument_id.clone(),
        }
    }

    fn asset(source_id: &SourceId, asset_id: &str) -> Self {
        Self::from_subject(&InstrumentRef::asset(source_id.clone(), asset_id))
    }
}

fn apply_book_snapshot_update(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    snapshot: &BookSnapshotUpdate,
) {
    let registered = inner.markets_by_asset.get(snapshot.asset_id.as_str()).cloned();
    let best_bid = best_level(&snapshot.bids, true, registered.as_ref());
    let best_ask = best_level(&snapshot.asks, false, registered.as_ref());

    let mut state = HotBookState {
        notice: notice.clone(),
        market_id: Some(snapshot.market_id.clone()),
        asset_id: snapshot.asset_id.clone(),
        best_bid,
        best_ask,
        midpoint: None,
        tick_size: registered.as_ref().map(|market| market.tick_size.clone()),
        tick_size_units: registered.as_ref().map(|market| market.tick_size_units),
        lot_size: registered.as_ref().and_then(|market| market.lot_size.clone()),
        lot_size_units: registered.as_ref().and_then(|market| market.lot_size_units),
        version: notice.version,
        timestamp_ms: snapshot.timestamp_ms,
        source_hash: snapshot
            .source_hash
            .clone()
            .or_else(|| notice.source_hash.clone()),
    };
    if let Some(registered) = registered.as_ref() {
        apply_registered_market(&mut state, registered);
    }
    state.midpoint = midpoint_for_levels(state.best_bid.as_ref(), state.best_ask.as_ref());
    inner
        .books
        .insert(HotStateKey::asset(&notice.source_id, snapshot.asset_id.as_str()), state);
}

fn apply_book_delta_update(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    delta: &crate::public_event::BookDeltaUpdate,
) {
    let key = HotStateKey::asset(&notice.source_id, delta.asset_id.as_str());
    let registered = inner.markets_by_asset.get(delta.asset_id.as_str()).cloned();
    let mut state = inner.books.get(&key).cloned().unwrap_or_else(|| HotBookState {
        notice: notice.clone(),
        market_id: Some(delta.market_id.clone()),
        asset_id: delta.asset_id.clone(),
        best_bid: None,
        best_ask: None,
        midpoint: None,
        tick_size: registered.as_ref().map(|market| market.tick_size.clone()),
        tick_size_units: registered.as_ref().map(|market| market.tick_size_units),
        lot_size: registered.as_ref().and_then(|market| market.lot_size.clone()),
        lot_size_units: registered.as_ref().and_then(|market| market.lot_size_units),
        version: notice.version,
        timestamp_ms: delta.timestamp_ms,
        source_hash: delta.source_hash.clone(),
    });

    state.notice = notice.clone();
    state.market_id = Some(delta.market_id.clone());
    state.version = notice.version;
    state.timestamp_ms = delta.timestamp_ms;
    state.source_hash = delta.source_hash.clone();

    let candidate = make_book_level(delta.price.as_str(), delta.size.as_str(), registered.as_ref());
    match delta.side {
        crate::trigger::Side::Buy => {
            if let Some(best_bid) = delta.best_bid.as_ref() {
                let size = if best_bid.as_str() == delta.price.as_str() {
                    delta.size.as_str()
                } else {
                    state
                        .best_bid
                        .as_ref()
                        .map(|level| level.size.exact.as_str())
                        .unwrap_or("0")
                };
                state.best_bid = make_book_level(best_bid.as_str(), size, registered.as_ref());
            } else if updates_best_side(state.best_bid.as_ref(), candidate.as_ref(), true) {
                state.best_bid = candidate;
            }
        }
        crate::trigger::Side::Sell => {
            if let Some(best_ask) = delta.best_ask.as_ref() {
                let size = if best_ask.as_str() == delta.price.as_str() {
                    delta.size.as_str()
                } else {
                    state
                        .best_ask
                        .as_ref()
                        .map(|level| level.size.exact.as_str())
                        .unwrap_or("0")
                };
                state.best_ask = make_book_level(best_ask.as_str(), size, registered.as_ref());
            } else if updates_best_side(state.best_ask.as_ref(), candidate.as_ref(), false) {
                state.best_ask = candidate;
            }
        }
    }

    state.midpoint = midpoint_for_levels(state.best_bid.as_ref(), state.best_ask.as_ref());
    inner.books.insert(key, state);
}

fn apply_orderbook_snapshot_resolution(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    snapshot: &OrderBookSnapshot,
) {
    let registered = inner.markets_by_asset.get(snapshot.asset_id.as_str()).cloned();
    let key = HotStateKey::asset(&notice.source_id, snapshot.asset_id.as_str());
    let existing_market_id = inner.books.get(&key).and_then(|state| state.market_id.clone());
    let best_bid = snapshot
        .best_bid
        .as_ref()
        .and_then(|level| make_book_level(&level.price, &level.size, registered.as_ref()));
    let best_ask = snapshot
        .best_ask
        .as_ref()
        .and_then(|level| make_book_level(&level.price, &level.size, registered.as_ref()));

    let mut state = HotBookState {
        notice: notice.clone(),
        market_id: registered
            .as_ref()
            .map(|market| market.market_id.clone())
            .or(existing_market_id),
        asset_id: AssetId::new(snapshot.asset_id.clone()),
        best_bid,
        best_ask,
        midpoint: None,
        tick_size: registered.as_ref().map(|market| market.tick_size.clone()),
        tick_size_units: registered.as_ref().map(|market| market.tick_size_units),
        lot_size: registered.as_ref().and_then(|market| market.lot_size.clone()),
        lot_size_units: registered.as_ref().and_then(|market| market.lot_size_units),
        version: notice.version,
        timestamp_ms: snapshot.timestamp_ms,
        source_hash: if snapshot.hash.is_empty() {
            notice.source_hash.clone()
        } else {
            Some(snapshot.hash.clone())
        },
    };
    if let Some(registered) = registered.as_ref() {
        apply_registered_market(&mut state, registered);
    }
    state.midpoint = midpoint_for_levels(state.best_bid.as_ref(), state.best_ask.as_ref());
    inner.books.insert(key, state);
}

fn apply_best_bid_ask_update(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    update: &BestBidAskUpdate,
) {
    let key = HotStateKey::asset(&notice.source_id, update.asset_id.as_str());
    let registered = inner.markets_by_asset.get(update.asset_id.as_str()).cloned();
    let mut state = inner.books.get(&key).cloned().unwrap_or_else(|| HotBookState {
        notice: notice.clone(),
        market_id: Some(update.market_id.clone()),
        asset_id: update.asset_id.clone(),
        best_bid: None,
        best_ask: None,
        midpoint: None,
        tick_size: registered.as_ref().map(|market| market.tick_size.clone()),
        tick_size_units: registered.as_ref().map(|market| market.tick_size_units),
        lot_size: registered.as_ref().and_then(|market| market.lot_size.clone()),
        lot_size_units: registered.as_ref().and_then(|market| market.lot_size_units),
        version: notice.version,
        timestamp_ms: update.timestamp_ms,
        source_hash: notice.source_hash.clone(),
    });

    state.notice = notice.clone();
    state.market_id = Some(update.market_id.clone());
    state.version = notice.version;
    state.timestamp_ms = update.timestamp_ms;
    state.best_bid = make_book_level(update.best_bid.as_str(), "0", registered.as_ref());
    state.best_ask = make_book_level(update.best_ask.as_str(), "0", registered.as_ref());
    if let Some(registered) = registered.as_ref() {
        apply_registered_market(&mut state, registered);
    }
    state.midpoint = midpoint_for_levels(state.best_bid.as_ref(), state.best_ask.as_ref());
    inner.books.insert(key, state);
}

fn apply_trade_tick_update(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    update: &TradeTickUpdate,
) {
    let key = HotStateKey::from_subject(&notice.subject);
    let mut state = inner.references.get(&key).cloned().unwrap_or_else(|| empty_reference_state(notice));
    state.notice = notice.clone();
    state.version = notice.version;
    state.timestamp_ms = update.timestamp_ms;
    state.last_trade_price = hot_value(update.price.as_str());
    state.last_trade_size = update.size.as_ref().and_then(|value| hot_value(value.as_str()));
    inner.references.insert(key, state);
}

fn apply_reference_price_update(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    update: &ReferencePriceUpdate,
) {
    let key = HotStateKey::from_subject(&notice.subject);
    let mut state = inner.references.get(&key).cloned().unwrap_or_else(|| empty_reference_state(notice));
    state.notice = notice.clone();
    state.version = notice.version;
    state.timestamp_ms = update.timestamp_ms;
    state.reference_price = hot_value(update.price.as_str());
    inner.references.insert(key, state);
}

fn apply_tick_size_change_update(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    update: &TickSizeChangeUpdate,
) {
    let key = HotStateKey::asset(&notice.source_id, update.asset_id.as_str());
    let registered = inner.markets_by_asset.get(update.asset_id.as_str()).cloned();
    let mut state = inner.books.get(&key).cloned().unwrap_or_else(|| HotBookState {
        notice: notice.clone(),
        market_id: Some(update.market_id.clone()),
        asset_id: update.asset_id.clone(),
        best_bid: None,
        best_ask: None,
        midpoint: None,
        tick_size: registered.as_ref().map(|market| market.tick_size.clone()),
        tick_size_units: registered.as_ref().map(|market| market.tick_size_units),
        lot_size: registered.as_ref().and_then(|market| market.lot_size.clone()),
        lot_size_units: registered.as_ref().and_then(|market| market.lot_size_units),
        version: notice.version,
        timestamp_ms: update.timestamp_ms,
        source_hash: notice.source_hash.clone(),
    });

    state.notice = notice.clone();
    state.market_id = Some(update.market_id.clone());
    state.version = notice.version;
    state.timestamp_ms = update.timestamp_ms;
    state.tick_size = Some(update.new_tick_size.clone());
    state.tick_size_units = parse_scaled(update.new_tick_size.as_str());
    refresh_book_level_units(&mut state.best_bid, state.tick_size_units, state.lot_size_units);
    refresh_book_level_units(&mut state.best_ask, state.tick_size_units, state.lot_size_units);
    state.midpoint = midpoint_for_levels(state.best_bid.as_ref(), state.best_ask.as_ref());
    inner.books.insert(key, state);
}

fn apply_source_status_update(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    update: &SourceStatusUpdate,
) {
    let key = HotStateKey::from_subject(&notice.subject);
    let mut state = inner.references.get(&key).cloned().unwrap_or_else(|| empty_reference_state(notice));
    state.notice = notice.clone();
    state.version = notice.version;
    state.timestamp_ms = update.observed_at_ms;
    state.healthy = Some(update.healthy);
    state.stale_after_ms = update.stale_after_ms;
    state.observed_at_ms = Some(update.observed_at_ms);
    inner.references.insert(key, state);
}

fn apply_reconnect_update(
    inner: &mut HotStateInner,
    notice: &UpdateNotice,
    update: &ReconnectUpdate,
) {
    let source_id = notice.source_id.as_str().to_string();
    inner
        .books
        .retain(|key, _| key.source_id != source_id);
    inner
        .references
        .retain(|key, _| key.source_id != source_id);

    let key = HotStateKey::from_subject(&notice.subject);
    let mut state = empty_reference_state(notice);
    state.reconnect_sequence = Some(update.sequence);
    state.timestamp_ms = update.timestamp_ms;
    state.version = notice.version;
    inner.references.insert(key, state);
}

fn apply_registered_market(state: &mut HotBookState, registered: &RegisteredMarket) {
    state.market_id = Some(registered.market_id.clone());
    state.tick_size = Some(registered.tick_size.clone());
    state.tick_size_units = Some(registered.tick_size_units);
    state.lot_size = registered.lot_size.clone();
    state.lot_size_units = registered.lot_size_units;
    refresh_book_level_units(
        &mut state.best_bid,
        state.tick_size_units,
        state.lot_size_units,
    );
    refresh_book_level_units(
        &mut state.best_ask,
        state.tick_size_units,
        state.lot_size_units,
    );
}

fn refresh_book_level_units(
    level: &mut Option<HotBookLevel>,
    tick_size_units: Option<u64>,
    lot_size_units: Option<u64>,
) {
    if let Some(level) = level.as_mut() {
        level.price_ticks = ticks_for(level.price.units, tick_size_units);
        level.size_lots = ticks_for(level.size.units, lot_size_units);
    }
}

fn empty_reference_state(notice: &UpdateNotice) -> HotReferenceState {
    HotReferenceState {
        notice: notice.clone(),
        reference_price: None,
        best_bid: None,
        best_ask: None,
        last_trade_price: None,
        last_trade_size: None,
        healthy: None,
        stale_after_ms: None,
        observed_at_ms: None,
        reconnect_sequence: None,
        version: notice.version,
        timestamp_ms: 0,
    }
}

fn best_level(
    levels: &[BookLevel],
    take_max: bool,
    registered: Option<&RegisteredMarket>,
) -> Option<HotBookLevel> {
    levels
        .iter()
        .filter_map(|level| make_book_level(level.price.as_str(), level.size.as_str(), registered))
        .reduce(|current, candidate| {
            let replace = if take_max {
                candidate.price.units > current.price.units
            } else {
                candidate.price.units < current.price.units
            };
            if replace { candidate } else { current }
        })
}

fn make_book_level(
    price: &str,
    size: &str,
    registered: Option<&RegisteredMarket>,
) -> Option<HotBookLevel> {
    Some(HotBookLevel {
        price: hot_value(price)?,
        size: hot_value(size)?,
        price_ticks: registered.and_then(|market| ticks_for(parse_scaled(price)?, Some(market.tick_size_units))),
        size_lots: registered.and_then(|market| ticks_for(parse_scaled(size)?, market.lot_size_units)),
    })
}

fn midpoint_for_levels(
    best_bid: Option<&HotBookLevel>,
    best_ask: Option<&HotBookLevel>,
) -> Option<HotStateValue> {
    let bid = best_bid?;
    let ask = best_ask?;
    let midpoint_units = (bid.price.units + ask.price.units) / 2;
    Some(HotStateValue {
        exact: format_scaled(midpoint_units),
        units: midpoint_units,
    })
}

fn book_level_to_snapshot_level(level: &HotBookLevel) -> PriceLevel {
    PriceLevel {
        price: level.price.exact.clone(),
        size: level.size.exact.clone(),
    }
}

fn hot_value(exact: &str) -> Option<HotStateValue> {
    Some(HotStateValue {
        exact: exact.to_string(),
        units: parse_scaled(exact)?,
    })
}

fn parse_scaled(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }

    let mut parts = trimmed.split('.');
    let whole = parts.next()?.parse::<u64>().ok()?;
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return None;
    }

    let mut padded = fraction.chars().take(6).collect::<String>();
    while padded.len() < 6 {
        padded.push('0');
    }
    let fraction_value = if padded.is_empty() {
        0
    } else {
        padded.parse::<u64>().ok()?
    };

    whole
        .checked_mul(HOT_VALUE_SCALE)?
        .checked_add(fraction_value)
}

fn format_scaled(units: u64) -> String {
    let whole = units / HOT_VALUE_SCALE;
    let fraction = units % HOT_VALUE_SCALE;
    if fraction == 0 {
        return whole.to_string();
    }

    let mut rendered = format!("{whole}.{fraction:06}");
    while rendered.ends_with('0') {
        rendered.pop();
    }
    rendered
}

fn ticks_for(units: u64, step_units: Option<u64>) -> Option<u64> {
    let step_units = step_units?;
    if step_units == 0 || units % step_units != 0 {
        return None;
    }
    Some(units / step_units)
}

fn updates_best_side(
    current: Option<&HotBookLevel>,
    candidate: Option<&HotBookLevel>,
    take_max: bool,
) -> bool {
    let candidate = match candidate {
        Some(candidate) => candidate,
        None => return false,
    };

    match current {
        Some(current) => {
            if take_max {
                candidate.price.units >= current.price.units
            } else {
                candidate.price.units <= current.price.units
            }
        }
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed_source::{InstrumentKind, SourceKind};
    use crate::market::{
        MarketStatus, MinOrderSize, Notional, OutcomeSide, OutcomeToken, Price, RewardFreshness,
        RewardParams, Size,
    };

    fn market_notice(kind: UpdateKind, version: u64) -> UpdateNotice {
        let source_id = SourceId::new("polymarket-public");
        UpdateNotice {
            source_id: source_id.clone(),
            source_kind: SourceKind::PolymarketWs,
            subject: InstrumentRef::asset(source_id, "asset-yes"),
            kind,
            version,
            source_hash: Some(format!("hash-{version}")),
        }
    }

    fn sample_market_meta() -> MarketMeta {
        MarketMeta {
            market_id: MarketId::new("market-1"),
            yes_asset: OutcomeToken::new(AssetId::new("asset-yes"), OutcomeSide::Yes),
            no_asset: OutcomeToken::new(AssetId::new("asset-no"), OutcomeSide::No),
            condition_id: Some("condition-1".to_string()),
            tick_size: TickSize::new("0.01"),
            min_order_size: Some(MinOrderSize::new("5")),
            status: MarketStatus::Active,
            reward: Some(RewardParams {
                rate_bps: Some(25),
                max_spread: Some(Price::new("0.02")),
                min_size: Some(Size::new("10")),
                min_notional: Some(Notional::new("100")),
                updated_at_ms: Some(1_700_000_000_000),
                freshness: RewardFreshness::Fresh,
            }),
        }
    }

    fn book_snapshot_update() -> NormalizedUpdate {
        NormalizedUpdate {
            notice: market_notice(UpdateKind::BookSnapshot, 11),
            payload: NormalizedUpdatePayload::BookSnapshot(BookSnapshotUpdate {
                market_id: MarketId::new("market-1"),
                asset_id: AssetId::new("asset-yes"),
                bids: vec![
                    BookLevel {
                        price: Price::new("0.54"),
                        size: Size::new("95"),
                    },
                    BookLevel {
                        price: Price::new("0.55"),
                        size: Size::new("100"),
                    },
                ],
                asks: vec![
                    BookLevel {
                        price: Price::new("0.56"),
                        size: Size::new("150"),
                    },
                    BookLevel {
                        price: Price::new("0.57"),
                        size: Size::new("175"),
                    },
                ],
                timestamp_ms: 1_700_000_000_111,
                source_hash: Some("book-hash".to_string()),
            }),
        }
    }

    #[test]
    fn normalized_book_updates_convert_into_hot_market_units() {
        let store = HotStateStore::new();
        store.register_market(&sample_market_meta());
        store.apply_update(&book_snapshot_update());

        let state = store
            .book_state(&SourceId::new("polymarket-public"), "asset-yes")
            .expect("book state");

        assert_eq!(state.market_id.as_ref().unwrap().as_str(), "market-1");
        assert_eq!(state.tick_size_units, Some(10_000));
        assert_eq!(state.lot_size_units, Some(5_000_000));
        assert_eq!(state.best_bid.as_ref().unwrap().price.units, 550_000);
        assert_eq!(state.best_bid.as_ref().unwrap().price_ticks, Some(55));
        assert_eq!(state.best_ask.as_ref().unwrap().size.units, 150_000_000);
        assert_eq!(state.best_ask.as_ref().unwrap().size_lots, Some(30));
        assert_eq!(state.midpoint.as_ref().unwrap().units, 555_000);
    }

    #[test]
    fn hot_state_can_hold_market_and_reference_sources_together() {
        let store = HotStateStore::new();
        store.register_market(&sample_market_meta());
        store.apply_update(&book_snapshot_update());

        let source_id = SourceId::new("reference-mid");
        let subject = InstrumentRef {
            source_id: source_id.clone(),
            kind: InstrumentKind::Symbol,
            instrument_id: "BTC-USD".to_string(),
        };
        let notice = UpdateNotice {
            source_id: source_id.clone(),
            source_kind: SourceKind::ExternalReference,
            subject: subject.clone(),
            kind: UpdateKind::ReferencePrice,
            version: 22,
            source_hash: None,
        };
        store.apply_update(&NormalizedUpdate {
            notice: notice.clone(),
            payload: NormalizedUpdatePayload::ReferencePrice(ReferencePriceUpdate {
                price: Price::new("62000.25"),
                notional: Some(Notional::new("1000000")),
                timestamp_ms: 1_700_000_000_222,
            }),
        });

        assert!(store
            .book_state(&SourceId::new("polymarket-public"), "asset-yes")
            .is_some());
        let reference_state = store.reference_state(&subject).expect("reference state");
        assert_eq!(
            reference_state.reference_price.as_ref().unwrap().units,
            62_000_250_000
        );
        assert_eq!(reference_state.version, 22);
        assert!(matches!(
            store.resolve_notice(&notice),
            Some(HotStateView::Reference(_))
        ));
    }

    #[test]
    fn notice_resolutions_project_legacy_snapshots_without_snapshot_channels() {
        let store = HotStateStore::new();
        let notice = market_notice(UpdateKind::BestBidAsk, 33);
        let resolution = NoticeResolution::BestBidAsk(BestBidAskUpdate {
            market_id: MarketId::new("market-1"),
            asset_id: AssetId::new("asset-yes"),
            best_bid: Price::new("0.48"),
            best_ask: Price::new("0.49"),
            spread: Some(Price::new("0.01")),
            timestamp_ms: 1_700_000_000_333,
        });

        store.apply_resolution(&notice, &resolution);

        let snapshot = store.project_snapshot(&notice).expect("snapshot");
        assert_eq!(snapshot.asset_id, "asset-yes");
        assert_eq!(snapshot.best_bid.as_ref().unwrap().price, "0.48");
        assert_eq!(snapshot.best_ask.as_ref().unwrap().price, "0.49");
        assert!(matches!(
            store.resolve_notice(&notice),
            Some(HotStateView::Book(_))
        ));
    }
}
