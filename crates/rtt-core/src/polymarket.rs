use crate::feed_source::SourceId;

pub const CLOB_HOST: &str = "clob.polymarket.com";
pub const CLOB_PORT: u16 = 443;

pub const CLOB_BASE_URL: &str = "https://clob.polymarket.com";
pub const CLOB_ROOT_URL: &str = "https://clob.polymarket.com/";
pub const CLOB_ORDER_PATH: &str = "/order";
pub const CLOB_ORDER_URL: &str = "https://clob.polymarket.com/order";
pub const CLOB_AUTH_API_KEYS_PATH: &str = "/auth/api-keys";
pub const CLOB_AUTH_API_KEYS_URL: &str = "https://clob.polymarket.com/auth/api-keys";

pub const MARKET_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

pub const PUBLIC_SOURCE_ID: &str = "polymarket-public";

pub fn public_source_id() -> SourceId {
    SourceId::new(PUBLIC_SOURCE_ID)
}
