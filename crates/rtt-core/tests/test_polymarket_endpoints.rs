use rtt_core::polymarket::{
    CLOB_AUTH_API_KEYS_PATH, CLOB_AUTH_API_KEYS_URL, CLOB_BASE_URL, CLOB_HOST, CLOB_ORDER_PATH,
    CLOB_ORDER_URL, CLOB_PORT, CLOB_ROOT_URL, MARKET_WS_URL,
};

#[test]
fn clob_endpoints_share_one_base_url() {
    assert_eq!(CLOB_ROOT_URL, format!("{CLOB_BASE_URL}/"));
    assert_eq!(CLOB_ORDER_URL, format!("{CLOB_BASE_URL}{CLOB_ORDER_PATH}"));
    assert_eq!(
        CLOB_AUTH_API_KEYS_URL,
        format!("{CLOB_BASE_URL}{CLOB_AUTH_API_KEYS_PATH}")
    );
}

#[test]
fn clob_host_port_and_market_ws_constants_match_expected_shape() {
    assert_eq!(CLOB_HOST, "clob.polymarket.com");
    assert_eq!(CLOB_PORT, 443);
    assert_eq!(
        MARKET_WS_URL,
        "wss://ws-subscriptions-clob.polymarket.com/ws/market"
    );
}
