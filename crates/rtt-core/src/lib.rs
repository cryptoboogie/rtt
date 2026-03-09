pub mod benchmark;
pub mod clob_auth;
pub mod clob_executor;
pub mod clob_order;
pub mod clob_request;
pub mod clob_response;
pub mod clob_signer;
pub mod clock;
pub mod connection;
pub mod executor;
pub mod feed_source;
pub mod h3_stub;
pub mod market;
pub mod metrics;
pub mod polymarket;
pub mod public_event;
pub mod queue;
pub mod request;
pub mod trigger;

pub use clob_executor::{
    process_one_clob, sign_and_dispatch, DispatchError, DispatchOutcome, PreSignedOrderPool,
};
pub use feed_source::{InstrumentKind, InstrumentRef, SourceId, SourceKind};
pub use market::{
    AssetId, MarketId, MarketMeta, MarketStatus, MinOrderSize, Notional, OutcomeSide, OutcomeToken,
    Price, RewardFreshness, RewardParams, Size, TickSize,
};
pub use polymarket::{
    public_source_id as polymarket_public_source_id, CLOB_AUTH_API_KEYS_PATH,
    CLOB_AUTH_API_KEYS_URL, CLOB_BASE_URL, CLOB_HOST, CLOB_ORDER_PATH, CLOB_ORDER_URL, CLOB_PORT,
    CLOB_ROOT_URL, MARKET_WS_URL,
};
pub use public_event::{
    BestBidAskUpdate, BookDeltaUpdate, BookLevel, BookSnapshotUpdate, NormalizedUpdate,
    NormalizedUpdatePayload, ReconnectUpdate, ReferencePriceUpdate, SourceStatusUpdate,
    TickSizeChangeUpdate, TradeTickUpdate, UpdateKind, UpdateNotice,
};

#[cfg(test)]
mod tests {
    #[test]
    fn sanity() {
        assert!(true);
    }
}
