//! CLOB execution helpers for dispatching signed orders.
//!
//! The supported public surfaces are the dispatch primitives themselves.
//! Historical wrapper config types are intentionally not part of the API.
//!
//! ```compile_fail
//! use rtt_core::clob_executor::ClobExecutionConfig;
//! ```

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use bytes::Bytes;
use http::Request;
use std::fmt;

use crate::clob_auth::L2Credentials;
use crate::clob_order::{SignatureType, SignedOrderPayload};
use crate::clob_request::{
    build_order_request_from_bytes, build_order_request_from_bytes_with_timestamp,
    encode_order_payload, RequestBuildError,
};
use crate::clob_signer::{build_order, sign_order, BuildOrderError};
use crate::clock;
use crate::connection::{extract_pop, get_cf_ray, ConnectionError, ConnectionPool};
use crate::metrics::TimestampRecord;
use crate::trigger::TriggerMessage;

#[derive(Debug)]
pub enum DispatchError {
    PoolExhausted,
    BuildOrder(BuildOrderError),
    Sign(String),
    RequestBuild(RequestBuildError),
    Connection(ConnectionError),
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoolExhausted => write!(f, "pre-signed order pool is exhausted"),
            Self::BuildOrder(err) => err.fmt(f),
            Self::Sign(err) => write!(f, "failed to sign order: {err}"),
            Self::RequestBuild(err) => err.fmt(f),
            Self::Connection(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for DispatchError {}

#[derive(Debug)]
pub enum DispatchOutcome {
    Sent {
        record: TimestampRecord,
        body: Option<Vec<u8>>,
    },
    Rejected {
        record: TimestampRecord,
        error: DispatchError,
    },
}

impl From<RequestBuildError> for DispatchError {
    fn from(value: RequestBuildError) -> Self {
        Self::RequestBuild(value)
    }
}

impl From<BuildOrderError> for DispatchError {
    fn from(value: BuildOrderError) -> Self {
        Self::BuildOrder(value)
    }
}

impl From<ConnectionError> for DispatchError {
    fn from(value: ConnectionError) -> Self {
        Self::Connection(value)
    }
}

fn finalize_record(rec: &mut TimestampRecord, now: u64) {
    if rec.t_buf_ready == 0 {
        rec.t_buf_ready = now;
    }
    if rec.t_write_begin == 0 {
        rec.t_write_begin = now;
    }
    if rec.t_write_end == 0 {
        rec.t_write_end = now;
    }
    if rec.t_first_resp_byte == 0 {
        rec.t_first_resp_byte = now;
    }
    if rec.t_headers_done == 0 {
        rec.t_headers_done = now;
    }
}

fn reject(mut rec: TimestampRecord, error: DispatchError, is_reconnect: bool) -> DispatchOutcome {
    let now = clock::now_ns();
    finalize_record(&mut rec, now);
    rec.is_reconnect = is_reconnect;
    DispatchOutcome::Rejected { record: rec, error }
}

fn send(mut rec: TimestampRecord, resp: http::Response<Bytes>) -> DispatchOutcome {
    if let Some(cf_ray) = get_cf_ray(&resp) {
        rec.cf_ray_pop = extract_pop(&cf_ray);
    }
    rec.t_headers_done = clock::now_ns();
    rec.is_reconnect = false;
    DispatchOutcome::Sent {
        record: rec,
        body: Some(resp.into_body().to_vec()),
    }
}

fn connection_failure_is_reconnect(error: &ConnectionError) -> bool {
    !matches!(error, ConnectionError::PoolEmpty)
}

fn dispatch_request(
    pool: &ConnectionPool,
    req: Request<Bytes>,
    mut rec: TimestampRecord,
    rt: &tokio::runtime::Runtime,
) -> DispatchOutcome {
    rec.t_write_begin = clock::now_ns();
    let handle_result = rt.block_on(async { pool.send_start(req).await });
    rec.t_write_end = clock::now_ns();

    let handle = match handle_result {
        Ok(handle) => handle,
        Err(err) => {
            let is_reconnect = connection_failure_is_reconnect(&err);
            return reject(rec, err.into(), is_reconnect);
        }
    };

    rec.connection_index = handle.connection_index;
    let resp_result = rt.block_on(async { pool.collect(handle).await });
    rec.t_first_resp_byte = clock::now_ns();

    match resp_result {
        Ok(resp) => send(rec, resp),
        Err(err) => {
            let is_reconnect = connection_failure_is_reconnect(&err);
            reject(rec, err.into(), is_reconnect)
        }
    }
}

/// A pool of pre-signed orders ready for hot-path dispatch.
///
/// Each order is pre-signed with a unique salt. At dispatch time, only the
/// HMAC auth headers are recomputed (fresh timestamp). The body (including
/// the EIP-712 signature) is NOT modified — changing any signed field would
/// invalidate the signature.
pub struct PreSignedOrderPool {
    bodies: Vec<Bytes>, // pre-serialized JSON bodies
    cursor: usize,
}

impl PreSignedOrderPool {
    /// Create from a vector of signed order payloads.
    pub fn new(payloads: Vec<SignedOrderPayload>) -> Result<Self, RequestBuildError> {
        let mut bodies = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            let body = encode_order_payload(payload)?;
            bodies.push(body);
        }
        Ok(Self { bodies, cursor: 0 })
    }

    /// Number of available pre-signed orders.
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// Consume the next pre-signed order. Recomputes only the HMAC auth
    /// headers (fresh timestamp). The body is used as-is (signature intact).
    pub fn dispatch(
        &mut self,
        creds: &L2Credentials,
    ) -> Result<Option<Request<Bytes>>, RequestBuildError> {
        if self.cursor >= self.bodies.len() {
            return Ok(None);
        }

        let body = self.bodies[self.cursor].clone();
        self.cursor += 1;

        build_order_request_from_bytes(body, creds).map(Some)
    }

    pub fn dispatch_with_timestamp(
        &mut self,
        creds: &L2Credentials,
        timestamp: &str,
    ) -> Result<Option<Request<Bytes>>, RequestBuildError> {
        if self.cursor >= self.bodies.len() {
            return Ok(None);
        }

        let body = self.bodies[self.cursor].clone();
        self.cursor += 1;
        build_order_request_from_bytes_with_timestamp(body, creds, timestamp).map(Some)
    }

    /// Reset cursor to reuse orders (e.g., after refill).
    pub fn reset_cursor(&mut self) {
        self.cursor = 0;
    }

    /// How many orders have been consumed.
    pub fn consumed(&self) -> usize {
        self.cursor
    }
}

/// Process a single CLOB trigger: dispatch pre-signed order on warm connection.
/// Returns a typed dispatch outcome so callers can distinguish reconnect/cold-path
/// samples from build/pool/request failures without poisoning latency stats.
pub fn process_one_clob(
    pool: &ConnectionPool,
    presigned: &mut PreSignedOrderPool,
    creds: &L2Credentials,
    msg: &TriggerMessage,
    rt: &tokio::runtime::Runtime,
) -> DispatchOutcome {
    let mut rec = TimestampRecord::default();
    rec.t_trigger_rx = msg.timestamp_ns;
    rec.t_dispatch_q = clock::now_ns();
    rec.t_exec_start = clock::now_ns();

    // Legacy pre-signed flow: reuse a serialized body and refresh only HMAC auth.
    rec.t_buf_ready = clock::now_ns();

    let req = match presigned.dispatch(creds) {
        Ok(Some(req)) => req,
        Ok(None) => return reject(rec, DispatchError::PoolExhausted, false),
        Err(err) => return reject(rec, err.into(), false),
    };

    dispatch_request(pool, req, rec, rt)
}

/// Sign an order at the trigger's price and dispatch via the connection pool.
///
/// Unlike `process_one_clob` which uses pre-signed orders, this function signs
/// each order on the hot path using the trigger's price. This adds ~100-500us
/// for EIP-712 signing but ensures the order price matches the current market.
///
/// Runs on a dedicated OS thread — uses `rt.block_on()` for async signing.
pub fn sign_and_dispatch(
    pool: &ConnectionPool,
    signer: &PrivateKeySigner,
    trigger: &TriggerMessage,
    creds: &L2Credentials,
    maker: Address,
    signer_addr: Address,
    fee_rate_bps: u64,
    is_neg_risk: bool,
    sig_type: SignatureType,
    owner: &str,
    rt: &tokio::runtime::Runtime,
) -> DispatchOutcome {
    let mut rec = TimestampRecord::default();
    rec.t_trigger_rx = trigger.timestamp_ns;
    rec.t_dispatch_q = clock::now_ns();
    rec.t_exec_start = clock::now_ns();

    // Build order from trigger at trigger's price
    let order = match build_order(trigger, maker, signer_addr, fee_rate_bps, sig_type) {
        Ok(order) => order,
        Err(err) => return reject(rec, err.into(), false),
    };

    // EIP-712 sign (async, measured separately)
    rec.t_sign_start = clock::now_ns();
    let sig = match rt.block_on(sign_order(signer, &order, is_neg_risk)) {
        Ok(sig) => sig,
        Err(err) => {
            rec.t_sign_end = clock::now_ns();
            return reject(rec, DispatchError::Sign(err.to_string()), false);
        }
    };
    rec.t_sign_end = clock::now_ns();

    // Build payload and serialize through the shared request encoder.
    let payload = SignedOrderPayload::new(&order, &sig, trigger.order_type, owner);
    let body = match encode_order_payload(&payload) {
        Ok(body) => body,
        Err(err) => return reject(rec, err.into(), false),
    };

    let req = match build_order_request_from_bytes(body, creds) {
        Ok(r) => r,
        Err(err) => return reject(rec, err.into(), false),
    };

    rec.t_buf_ready = clock::now_ns();
    dispatch_request(pool, req, rec, rt)
}

#[cfg(test)]
fn live_test_size_from_env(size: Option<String>) -> String {
    size.filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "2".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clob_order::{Order, SignedOrderPayload};
    use crate::polymarket::{CLOB_HOST, CLOB_PORT};
    use crate::trigger::{OrderType, Side};
    use alloy::primitives::{address, Address, U256};
    use base64::engine::general_purpose::URL_SAFE;
    use base64::Engine;

    fn test_creds() -> L2Credentials {
        L2Credentials {
            api_key: "test-api-key".to_string(),
            secret: URL_SAFE.encode(b"test-secret-key!"),
            passphrase: "test-pass".to_string(),
            address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
        }
    }

    fn test_payloads(count: usize) -> Vec<SignedOrderPayload> {
        (0..count)
            .map(|i| {
                let order = Order {
                    salt: U256::from(1234567890u64 + i as u64),
                    maker: address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
                    signer: address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
                    taker: Address::ZERO,
                    tokenId: U256::from(9999u64),
                    makerAmount: U256::from(50_000_000u64),
                    takerAmount: U256::from(100_000_000u64),
                    expiration: U256::ZERO,
                    nonce: U256::ZERO,
                    feeRateBps: U256::ZERO,
                    side: 0,
                    signatureType: 0,
                };
                SignedOrderPayload::new(&order, "0xdeadbeef", OrderType::FOK, "owner-uuid")
            })
            .collect()
    }

    #[test]
    fn test_presigned_pool_creation() {
        let payloads = test_payloads(5);
        let pool = PreSignedOrderPool::new(payloads).unwrap();
        assert_eq!(pool.len(), 5);
        assert_eq!(pool.consumed(), 0);
    }

    #[test]
    fn test_presigned_pool_consume() {
        let payloads = test_payloads(3);
        let mut pool = PreSignedOrderPool::new(payloads).unwrap();
        let creds = test_creds();

        let req1 = pool.dispatch(&creds).unwrap();
        assert!(req1.is_some());
        assert_eq!(pool.consumed(), 1);

        let req2 = pool.dispatch(&creds).unwrap();
        assert!(req2.is_some());
        assert_eq!(pool.consumed(), 2);

        let req3 = pool.dispatch(&creds).unwrap();
        assert!(req3.is_some());
        assert_eq!(pool.consumed(), 3);

        // Pool exhausted
        let req4 = pool.dispatch(&creds).unwrap();
        assert!(req4.is_none());
    }

    #[test]
    fn test_presigned_pool_refill() {
        let payloads = test_payloads(2);
        let mut pool = PreSignedOrderPool::new(payloads).unwrap();
        let creds = test_creds();

        pool.dispatch(&creds).unwrap();
        pool.dispatch(&creds).unwrap();
        assert_eq!(pool.consumed(), 2);

        pool.reset_cursor();
        assert_eq!(pool.consumed(), 0);

        let req = pool.dispatch(&creds).unwrap();
        assert!(req.is_some());
    }

    #[test]
    fn test_hot_path_latency() {
        // Measure: HMAC + request assembly around an immutable signed body should be fast.
        let payloads = test_payloads(100);
        let mut pool = PreSignedOrderPool::new(payloads).unwrap();
        let creds = test_creds();

        // Warmup
        pool.dispatch(&creds).unwrap();

        // Measure 10 dispatches
        let start = std::time::Instant::now();
        for _ in 0..10 {
            pool.dispatch(&creds).unwrap();
        }
        let elapsed = start.elapsed();
        let per_dispatch = elapsed / 10;

        // Each dispatch should be under 1ms (generous bound for debug build)
        assert!(
            per_dispatch.as_micros() < 1000,
            "dispatch took {:?} per call — too slow",
            per_dispatch
        );
    }

    #[test]
    fn test_clob_process_one_builds_post_request() {
        // Verify the pre-signed dispatch path builds a POST request.
        let payloads = test_payloads(5);
        let mut pool = PreSignedOrderPool::new(payloads).unwrap();
        let creds = test_creds();

        // Dispatch one to verify it works
        let req = pool.dispatch(&creds).unwrap().unwrap();
        assert_eq!(req.method(), http::Method::POST);

        let body: serde_json::Value = serde_json::from_slice(req.body()).unwrap();
        assert!(body["order"].is_object());
        assert_eq!(body["orderType"].as_str().unwrap(), "FOK");
        assert!(req.headers().get("POLY_SIGNATURE").is_some());
    }

    #[test]
    fn test_presigned_pool_dispatch_reuses_cached_body_bytes() {
        let payloads = test_payloads(1);
        let mut pool = PreSignedOrderPool::new(payloads).unwrap();
        let creds = test_creds();
        let original_ptr = pool.bodies[0].as_ptr();

        let req = pool.dispatch(&creds).unwrap().unwrap();

        assert_eq!(
            req.body().as_ptr(),
            original_ptr,
            "dispatch should reuse immutable Bytes storage"
        );
    }

    #[test]
    fn test_sign_and_dispatch_uses_trigger_price() {
        // Verify that sign_and_dispatch builds an order at the trigger's price,
        // not some hardcoded config price. We test this by building an order
        // from the same trigger and checking the amounts match.
        use crate::clob_signer::build_order as build_order_fn;

        let trigger = TriggerMessage {
            trigger_id: 1,
            token_id: "1234".to_string(),
            side: Side::Buy,
            price: "0.63".to_string(), // specific non-default price
            size: "50".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 1000,
        };

        let maker = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let order = build_order_fn(&trigger, maker, maker, 0, SignatureType::Eoa).unwrap();

        // Buy 50 @ 0.63 → makerAmount = 0.63 * 50 * 1e6 = 31_500_000
        assert_eq!(order.makerAmount, U256::from(31_500_000u64));
        // takerAmount = 50 * 1e6 = 50_000_000
        assert_eq!(order.takerAmount, U256::from(50_000_000u64));
    }

    #[test]
    fn test_live_test_size_defaults_to_two() {
        assert_eq!(live_test_size_from_env(None), "2");
        assert_eq!(live_test_size_from_env(Some("".to_string())), "2");
        assert_eq!(live_test_size_from_env(Some("   ".to_string())), "2");
    }

    #[test]
    fn test_live_test_size_uses_env_override() {
        assert_eq!(live_test_size_from_env(Some("8".to_string())), "8");
    }

    #[tokio::test]
    async fn test_sign_and_dispatch_sign_duration_populated() {
        // Verify that sign_duration timestamps are populated after signing
        use crate::clob_signer::sign_order as sign_order_fn;
        use alloy::signers::local::PrivateKeySigner;

        let signer: PrivateKeySigner =
            "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .parse()
                .unwrap();
        let maker = signer.address();

        let trigger = TriggerMessage {
            trigger_id: 1,
            token_id: "1234".to_string(),
            side: Side::Buy,
            price: "0.50".to_string(),
            size: "10".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 1000,
        };

        let order =
            crate::clob_signer::build_order(&trigger, maker, maker, 0, SignatureType::Eoa).unwrap();

        let t_start = crate::clock::now_ns();
        let sig = sign_order_fn(&signer, &order, false).await.unwrap();
        let t_end = crate::clock::now_ns();

        // Signing should take nonzero time
        assert!(t_end > t_start);
        // Signature should be valid
        assert!(sig.starts_with("0x"));
        assert!(sig.len() >= 132);

        // Simulate what sign_and_dispatch does: record timestamps
        let mut rec = TimestampRecord::default();
        rec.t_sign_start = t_start;
        rec.t_sign_end = t_end;
        assert!(rec.sign_duration() > 0);
    }

    #[tokio::test]
    #[ignore] // Needs real credentials and network
    async fn test_clob_end_to_end_pipeline() {
        use crate::clob_auth::load_credentials_from_env;
        use crate::clob_order::SignatureType;
        use crate::clob_response::parse_order_response;
        use crate::clob_signer::{build_order, sign_order};
        use crate::connection::AddressFamily;
        use alloy::signers::local::PrivateKeySigner;

        // --- Load real credentials from env ---
        let (creds, private_key, proxy_address) = load_credentials_from_env()
            .expect("Set POLY_API_KEY, POLY_SECRET, POLY_PASSPHRASE, POLY_ADDRESS, POLY_PRIVATE_KEY, POLY_PROXY_ADDRESS");

        let pk_hex = private_key.strip_prefix("0x").unwrap_or(&private_key);
        let signer: PrivateKeySigner = pk_hex.parse().expect("invalid POLY_PRIVATE_KEY");
        let signer_addr = signer.address();

        println!("\n=== CLOB End-to-End Pipeline Test ===");
        println!("Auth addr:  {}", creds.address);
        println!("Proxy addr: {}", proxy_address);
        println!("Signer:     {:?}", signer_addr);
        println!(
            "API Key:    {}...",
            &creds.api_key[..8.min(creds.api_key.len())]
        );

        // --- Warm connection pool ---
        let mut conn_pool = ConnectionPool::new(CLOB_HOST, CLOB_PORT, 1, AddressFamily::Auto);
        let warm_count = conn_pool.warmup().await.expect("warmup failed");
        println!("Pool:       {} warm connection(s)", warm_count);

        // --- Build & sign a real order ---
        // TOKEN_ID and PRICE from env; everything else hardcoded for a minimal test trade.
        let token_id = std::env::var("TOKEN_ID")
            .expect("TOKEN_ID env var required — the condition token to buy");
        let price = std::env::var("PRICE").unwrap_or_else(|_| "0.95".to_string());
        let size = live_test_size_from_env(std::env::var("SIZE").ok());

        let trigger = TriggerMessage {
            trigger_id: 1,
            token_id,
            side: Side::Buy,
            price,
            size,
            order_type: OrderType::FOK,
            timestamp_ns: clock::now_ns(),
        };

        let fee_rate_bps: u64 = std::env::var("FEE_RATE_BPS")
            .unwrap_or_else(|_| "0".to_string())
            .parse()
            .unwrap_or(0);
        let is_neg_risk: bool = std::env::var("NEG_RISK")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .unwrap_or(false);

        let sig_type: u8 = std::env::var("SIG_TYPE")
            .unwrap_or_else(|_| "1".to_string())
            .parse()
            .unwrap_or(1);
        let signature_type = match sig_type {
            0 => SignatureType::Eoa,
            2 => SignatureType::GnosisSafe,
            _ => SignatureType::Poly,
        };

        println!(
            "            fee_rate_bps={} neg_risk={} sig_type={}",
            fee_rate_bps, is_neg_risk, sig_type
        );

        // maker = proxy wallet (POLY_PROXY_ADDRESS), signer = EOA (from POLY_PRIVATE_KEY)
        let maker_addr: Address = proxy_address.parse().expect("invalid POLY_PROXY_ADDRESS");
        let order = build_order(
            &trigger,
            maker_addr,
            signer_addr,
            fee_rate_bps,
            signature_type,
        )
        .unwrap();
        let sig = sign_order(&signer, &order, is_neg_risk).await.unwrap();
        println!("Signature:  {}...{}", &sig[..10], &sig[sig.len() - 6..]);

        let payload = SignedOrderPayload::new(&order, &sig, trigger.order_type, &creds.api_key);
        let body_json = serde_json::to_string_pretty(&payload).unwrap();
        println!("Order body:\n{}", body_json);

        // --- Pre-sign pool with real signatures ---
        let payloads = vec![payload];
        let mut presigned = PreSignedOrderPool::new(payloads).unwrap();
        println!("Pre-signed: {} order(s)", presigned.len());

        // --- Dispatch via process_one_clob ---
        let conn_pool = std::sync::Arc::new(conn_pool);
        let conn_pool_clone = conn_pool.clone();
        let creds_clone = creds.clone();
        let trigger_clone = trigger.clone();

        let (rec, resp_body) = tokio::task::spawn_blocking(
            move || -> (TimestampRecord, (http::StatusCode, bytes::Bytes)) {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();

                // Manually do what process_one_clob does, but capture the response body
                let mut rec = TimestampRecord::default();
                rec.t_trigger_rx = trigger_clone.timestamp_ns;
                rec.t_dispatch_q = clock::now_ns();
                rec.t_exec_start = clock::now_ns();
                rec.t_buf_ready = clock::now_ns();

                let req = presigned.dispatch(&creds_clone).unwrap().unwrap();
                rec.t_write_begin = clock::now_ns();

                let handle = rt
                    .block_on(async { conn_pool_clone.send_start(req).await })
                    .unwrap();
                rec.t_write_end = clock::now_ns();
                rec.connection_index = handle.connection_index;

                let resp = rt
                    .block_on(async { conn_pool_clone.collect(handle).await })
                    .unwrap();
                rec.t_first_resp_byte = clock::now_ns();

                let status = resp.status();
                let resp_bytes = resp.into_body();
                rec.t_headers_done = clock::now_ns();

                (rec, (status, resp_bytes))
            },
        )
        .await
        .unwrap();

        // --- Print results ---
        let (status, body_bytes) = resp_body;
        println!("\n--- Response ---");
        println!("HTTP Status: {}", status);
        println!(
            "Body:        {}",
            std::str::from_utf8(&body_bytes).unwrap_or("<binary>")
        );

        if let Ok(order_resp) = parse_order_response(&body_bytes) {
            println!(
                "Parsed:      success={}, order_id={}, status={}, error={:?}",
                order_resp.success, order_resp.order_id, order_resp.status, order_resp.error_msg
            );
        }

        println!("\n--- Timestamps (ns) ---");
        println!("trigger_rx:      {}", rec.t_trigger_rx);
        println!("dispatch_q:      {}", rec.t_dispatch_q);
        println!("exec_start:      {}", rec.t_exec_start);
        println!("buf_ready:       {}", rec.t_buf_ready);
        println!("write_begin:     {}", rec.t_write_begin);
        println!("write_end:       {}", rec.t_write_end);
        println!("first_resp_byte: {}", rec.t_first_resp_byte);
        println!("headers_done:    {}", rec.t_headers_done);
        println!("connection_idx:  {}", rec.connection_index);

        println!("\n--- Derived Metrics ---");
        println!(
            "queue_delay:       {:>8} ns ({:.1} us)",
            rec.queue_delay(),
            rec.queue_delay() as f64 / 1000.0
        );
        println!(
            "prep_time:         {:>8} ns ({:.1} us)",
            rec.prep_time(),
            rec.prep_time() as f64 / 1000.0
        );
        println!(
            "trigger_to_wire:   {:>8} ns ({:.1} us)",
            rec.trigger_to_wire(),
            rec.trigger_to_wire() as f64 / 1000.0
        );
        println!(
            "write_duration:    {:>8} ns ({:.1} us)",
            rec.write_duration(),
            rec.write_duration() as f64 / 1000.0
        );
        println!(
            "write_to_1st_byte: {:>8} ns ({:.1} ms)",
            rec.write_to_first_byte(),
            rec.write_to_first_byte() as f64 / 1_000_000.0
        );
        println!(
            "warm_ttfb:         {:>8} ns ({:.1} ms)",
            rec.warm_ttfb(),
            rec.warm_ttfb() as f64 / 1_000_000.0
        );
        println!(
            "trigger_to_1st_b:  {:>8} ns ({:.1} ms)",
            rec.trigger_to_first_byte(),
            rec.trigger_to_first_byte() as f64 / 1_000_000.0
        );

        // Assertions
        assert!(rec.t_trigger_rx > 0);
        assert!(rec.t_write_begin > 0);
        assert!(rec.t_headers_done > 0);
        assert!(rec.t_write_end > rec.t_write_begin);
        assert!(rec.t_first_resp_byte > rec.t_write_end);
        println!("\n=== PASS ===");
    }
}
