use serde::{Deserialize, Serialize};

use crate::feed_source::{InstrumentRef, SourceId, SourceKind};
use crate::market::{AssetId, MarketId, Notional, Price, Size, TickSize};
use crate::trigger::Side;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKind {
    BookSnapshot,
    BookDelta,
    BestBidAsk,
    TradeTick,
    ReferencePrice,
    TickSizeChange,
    Reconnect,
    SourceStatus,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNotice {
    pub source_id: SourceId,
    pub source_kind: SourceKind,
    pub subject: InstrumentRef,
    pub kind: UpdateKind,
    pub version: u64,
    pub source_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookLevel {
    pub price: Price,
    pub size: Size,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookSnapshotUpdate {
    pub market_id: MarketId,
    pub asset_id: AssetId,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
    pub timestamp_ms: u64,
    pub source_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookDeltaUpdate {
    pub market_id: MarketId,
    pub asset_id: AssetId,
    pub price: Price,
    pub size: Size,
    pub side: Side,
    pub timestamp_ms: u64,
    pub best_bid: Option<Price>,
    pub best_ask: Option<Price>,
    pub source_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BestBidAskUpdate {
    pub market_id: MarketId,
    pub asset_id: AssetId,
    pub best_bid: Price,
    pub best_ask: Price,
    pub spread: Option<Price>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeTickUpdate {
    pub market_id: Option<MarketId>,
    pub asset_id: Option<AssetId>,
    pub price: Price,
    pub size: Option<Size>,
    pub side: Option<Side>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferencePriceUpdate {
    pub price: Price,
    pub notional: Option<Notional>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickSizeChangeUpdate {
    pub market_id: MarketId,
    pub asset_id: AssetId,
    pub old_tick_size: TickSize,
    pub new_tick_size: TickSize,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconnectUpdate {
    pub sequence: u64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStatusUpdate {
    pub healthy: bool,
    pub stale_after_ms: Option<u64>,
    pub observed_at_ms: u64,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NormalizedUpdatePayload {
    BookSnapshot(BookSnapshotUpdate),
    BookDelta(BookDeltaUpdate),
    BestBidAsk(BestBidAskUpdate),
    TradeTick(TradeTickUpdate),
    ReferencePrice(ReferencePriceUpdate),
    TickSizeChange(TickSizeChangeUpdate),
    Reconnect(ReconnectUpdate),
    SourceStatus(SourceStatusUpdate),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedUpdate {
    pub notice: UpdateNotice,
    pub payload: NormalizedUpdatePayload,
}

impl NormalizedUpdatePayload {
    pub fn kind(&self) -> UpdateKind {
        match self {
            NormalizedUpdatePayload::BookSnapshot(_) => UpdateKind::BookSnapshot,
            NormalizedUpdatePayload::BookDelta(_) => UpdateKind::BookDelta,
            NormalizedUpdatePayload::BestBidAsk(_) => UpdateKind::BestBidAsk,
            NormalizedUpdatePayload::TradeTick(_) => UpdateKind::TradeTick,
            NormalizedUpdatePayload::ReferencePrice(_) => UpdateKind::ReferencePrice,
            NormalizedUpdatePayload::TickSizeChange(_) => UpdateKind::TickSizeChange,
            NormalizedUpdatePayload::Reconnect(_) => UpdateKind::Reconnect,
            NormalizedUpdatePayload::SourceStatus(_) => UpdateKind::SourceStatus,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed_source::InstrumentRef;

    #[test]
    fn update_notice_is_small_but_resolvable() {
        let source_id = SourceId::new("polymarket-public");
        let notice = UpdateNotice {
            source_id: source_id.clone(),
            source_kind: SourceKind::PolymarketWs,
            subject: InstrumentRef::asset(source_id.clone(), "asset-1"),
            kind: UpdateKind::BookDelta,
            version: 42,
            source_hash: Some("hash-42".to_string()),
        };

        assert_eq!(notice.source_id, source_id);
        assert_eq!(notice.source_kind, SourceKind::PolymarketWs);
        assert_eq!(notice.subject.instrument_id, "asset-1");
        assert_eq!(notice.kind, UpdateKind::BookDelta);
        assert_eq!(notice.version, 42);
        assert_eq!(notice.source_hash.as_deref(), Some("hash-42"));
    }

    #[test]
    fn update_notice_can_discriminate_source_kind_and_custom_event_family() {
        let source_id = SourceId::new("custom-reference");
        let notice = UpdateNotice {
            source_id: source_id.clone(),
            source_kind: SourceKind::ExternalReference,
            subject: InstrumentRef::symbol(source_id, "BTC-USD"),
            kind: UpdateKind::Custom,
            version: 7,
            source_hash: None,
        };

        let json = serde_json::to_string(&notice).unwrap();
        let round_trip: UpdateNotice = serde_json::from_str(&json).unwrap();

        assert_eq!(round_trip.source_kind, SourceKind::ExternalReference);
        assert_eq!(round_trip.kind, UpdateKind::Custom);
    }

    #[test]
    fn normalized_updates_support_market_and_external_source_models() {
        let market_source = SourceId::new("polymarket-public");
        let market_event = NormalizedUpdate {
            notice: UpdateNotice {
                source_id: market_source.clone(),
                source_kind: SourceKind::PolymarketWs,
                subject: InstrumentRef::asset(market_source.clone(), "asset-1"),
                kind: UpdateKind::BestBidAsk,
                version: 101,
                source_hash: None,
            },
            payload: NormalizedUpdatePayload::BestBidAsk(BestBidAskUpdate {
                market_id: MarketId::new("market-1"),
                asset_id: AssetId::new("asset-1"),
                best_bid: Price::new("0.44"),
                best_ask: Price::new("0.45"),
                spread: Some(Price::new("0.01")),
                timestamp_ms: 1_700_000_000_000,
            }),
        };

        let external_source = SourceId::new("reference-mid");
        let external_event = NormalizedUpdate {
            notice: UpdateNotice {
                source_id: external_source.clone(),
                source_kind: SourceKind::ExternalReference,
                subject: InstrumentRef::symbol(external_source.clone(), "BTC-USD"),
                kind: UpdateKind::ReferencePrice,
                version: 202,
                source_hash: None,
            },
            payload: NormalizedUpdatePayload::ReferencePrice(ReferencePriceUpdate {
                price: Price::new("62345.12"),
                notional: Some(Notional::new("1000000")),
                timestamp_ms: 1_700_000_000_123,
            }),
        };

        assert_eq!(market_event.payload.kind(), UpdateKind::BestBidAsk);
        assert_eq!(external_event.payload.kind(), UpdateKind::ReferencePrice);

        let json = serde_json::to_string(&external_event).unwrap();
        let round_trip: NormalizedUpdate = serde_json::from_str(&json).unwrap();
        assert_eq!(round_trip, external_event);
    }
}
