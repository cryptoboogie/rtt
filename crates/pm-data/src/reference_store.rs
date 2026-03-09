use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rtt_core::{
    feed_source::InstrumentKind,
    BestBidAskUpdate, InstrumentRef, NormalizedUpdate, NormalizedUpdatePayload, ReconnectUpdate,
    ReferencePriceUpdate, SourceId, SourceStatusUpdate, TickSizeChangeUpdate, TradeTickUpdate,
    UpdateNotice,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReferenceStoreKey {
    pub source_id: SourceId,
    pub instrument_kind: String,
    pub instrument_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferenceState {
    pub last_notice: Option<UpdateNotice>,
    pub last_reference_price: Option<ReferencePriceUpdate>,
    pub last_trade_tick: Option<TradeTickUpdate>,
    pub last_best_bid_ask: Option<BestBidAskUpdate>,
    pub last_tick_size_change: Option<TickSizeChangeUpdate>,
    pub last_source_status: Option<SourceStatusUpdate>,
    pub last_reconnect: Option<ReconnectUpdate>,
}

#[derive(Clone, Default)]
pub struct ReferenceStore {
    state: Arc<RwLock<HashMap<ReferenceStoreKey, ReferenceState>>>,
}

impl ReferenceStoreKey {
    pub fn for_subject(subject: &InstrumentRef) -> Self {
        Self {
            source_id: subject.source_id.clone(),
            instrument_kind: format!("{:?}", subject.kind),
            instrument_id: subject.instrument_id.clone(),
        }
    }
}

impl ReferenceStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_update(&self, update: &NormalizedUpdate) {
        let mut state = self.state.write().unwrap();
        let entry = state
            .entry(ReferenceStoreKey::for_subject(&update.notice.subject))
            .or_default();
        entry.last_notice = Some(update.notice.clone());

        match &update.payload {
            NormalizedUpdatePayload::ReferencePrice(reference_price) => {
                entry.last_reference_price = Some(reference_price.clone());
            }
            NormalizedUpdatePayload::TradeTick(trade_tick) => {
                entry.last_trade_tick = Some(trade_tick.clone());
            }
            NormalizedUpdatePayload::BestBidAsk(best_bid_ask) => {
                entry.last_best_bid_ask = Some(best_bid_ask.clone());
            }
            NormalizedUpdatePayload::TickSizeChange(tick_size_change) => {
                entry.last_tick_size_change = Some(tick_size_change.clone());
            }
            NormalizedUpdatePayload::SourceStatus(source_status) => {
                entry.last_source_status = Some(source_status.clone());
            }
            NormalizedUpdatePayload::Reconnect(reconnect) => {
                entry.last_reconnect = Some(reconnect.clone());
            }
            NormalizedUpdatePayload::BookSnapshot(_) | NormalizedUpdatePayload::BookDelta(_) => {}
        }
    }

    pub fn resolve_subject(&self, subject: &InstrumentRef) -> Option<ReferenceState> {
        let state = self.state.read().unwrap();
        state.get(&ReferenceStoreKey::for_subject(subject)).cloned()
    }

    pub fn clear_source(&self, source_id: &SourceId) {
        let mut state = self.state.write().unwrap();
        state.retain(|key, _| &key.source_id != source_id);
    }

    pub fn clear_instrument(
        &self,
        source_id: &SourceId,
        instrument_kind: InstrumentKind,
        instrument_id: &str,
    ) {
        let instrument_kind = format!("{instrument_kind:?}");
        let mut state = self.state.write().unwrap();
        state.retain(|key, _| {
            !(&key.source_id == source_id
                && key.instrument_kind == instrument_kind
                && key.instrument_id == instrument_id)
        });
    }

    pub fn len(&self) -> usize {
        let state = self.state.read().unwrap();
        state.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtt_core::{
        feed_source::{InstrumentKind, InstrumentRef, SourceId, SourceKind},
        market::Price,
        public_event::{NormalizedUpdate, NormalizedUpdatePayload, ReferencePriceUpdate},
        UpdateKind, UpdateNotice,
    };

    #[test]
    fn reference_store_tracks_latest_reference_update_by_subject() {
        let store = ReferenceStore::new();
        let source_id = SourceId::new("reference-mid");
        let subject = InstrumentRef {
            source_id: source_id.clone(),
            kind: InstrumentKind::Symbol,
            instrument_id: "BTC-USD".to_string(),
        };
        let update = NormalizedUpdate {
            notice: UpdateNotice {
                source_id: source_id.clone(),
                source_kind: SourceKind::ExternalReference,
                subject: subject.clone(),
                kind: UpdateKind::ReferencePrice,
                version: 11,
                source_hash: None,
            },
            payload: NormalizedUpdatePayload::ReferencePrice(ReferencePriceUpdate {
                price: Price::new("62000.00"),
                notional: None,
                timestamp_ms: 1_700_000_000_000,
            }),
        };

        store.apply_update(&update);

        let state = store.resolve_subject(&subject).expect("state");
        assert_eq!(
            state
                .last_reference_price
                .expect("reference price")
                .price
                .as_str(),
            "62000.00"
        );
        assert_eq!(state.last_notice.expect("notice").version, 11);
    }

    #[test]
    fn clear_instrument_removes_only_matching_subject_for_source() {
        let store = ReferenceStore::new();
        let source_id = SourceId::new("reference-mid");
        let other_source_id = SourceId::new("reference-alt");

        let updates = [
            (
                source_id.clone(),
                "BTC-USD",
                "62000.00",
                11_u64,
            ),
            (
                source_id.clone(),
                "ETH-USD",
                "3200.00",
                12_u64,
            ),
            (
                other_source_id.clone(),
                "BTC-USD",
                "62100.00",
                13_u64,
            ),
        ];

        for (source_id, instrument_id, price, version) in updates {
            let subject = InstrumentRef {
                source_id: source_id.clone(),
                kind: InstrumentKind::Symbol,
                instrument_id: instrument_id.to_string(),
            };
            store.apply_update(&NormalizedUpdate {
                notice: UpdateNotice {
                    source_id: source_id.clone(),
                    source_kind: SourceKind::ExternalReference,
                    subject: subject.clone(),
                    kind: UpdateKind::ReferencePrice,
                    version,
                    source_hash: None,
                },
                payload: NormalizedUpdatePayload::ReferencePrice(ReferencePriceUpdate {
                    price: Price::new(price),
                    notional: None,
                    timestamp_ms: 1_700_000_000_000 + version,
                }),
            });
        }

        store.clear_instrument(&source_id, InstrumentKind::Symbol, "BTC-USD");

        let btc_subject = InstrumentRef {
            source_id: source_id.clone(),
            kind: InstrumentKind::Symbol,
            instrument_id: "BTC-USD".to_string(),
        };
        let eth_subject = InstrumentRef {
            source_id: source_id.clone(),
            kind: InstrumentKind::Symbol,
            instrument_id: "ETH-USD".to_string(),
        };
        let other_btc_subject = InstrumentRef {
            source_id: other_source_id,
            kind: InstrumentKind::Symbol,
            instrument_id: "BTC-USD".to_string(),
        };

        assert!(store.resolve_subject(&btc_subject).is_none());
        assert!(store.resolve_subject(&eth_subject).is_some());
        assert!(store.resolve_subject(&other_btc_subject).is_some());
    }
}
