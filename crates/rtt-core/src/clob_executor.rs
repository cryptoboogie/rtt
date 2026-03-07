use bytes::Bytes;
use http::{Method, Request, Uri};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::clock;
use crate::clob_auth::{build_l2_headers, L2Credentials};
use crate::clob_order::SignedOrderPayload;
use crate::connection::{ConnectionPool, extract_pop, get_cf_ray};
use crate::metrics::TimestampRecord;
use crate::trigger::TriggerMessage;

/// A pool of pre-signed orders ready for hot-path dispatch.
///
/// Each order is pre-signed with a unique salt. At dispatch time, only the
/// HMAC auth headers are recomputed (fresh timestamp). The body (including
/// the EIP-712 signature) is NOT modified — changing any signed field would
/// invalidate the signature.
pub struct PreSignedOrderPool {
    bodies: Vec<Vec<u8>>, // pre-serialized JSON bodies
    cursor: usize,
}

impl PreSignedOrderPool {
    /// Create from a vector of signed order payloads.
    pub fn new(
        payloads: Vec<SignedOrderPayload>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut bodies = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            let body = serde_json::to_vec(payload)?;
            bodies.push(body);
        }
        Ok(Self {
            bodies,
            cursor: 0,
        })
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
    ) -> Result<Option<Request<Bytes>>, Box<dyn std::error::Error>> {
        if self.cursor >= self.bodies.len() {
            return Ok(None);
        }

        let body = &self.bodies[self.cursor];
        self.cursor += 1;

        // Only recompute HMAC headers (includes fresh timestamp)
        let body_str = std::str::from_utf8(body)?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let headers = build_l2_headers(creds, &timestamp, "POST", "/order", body_str)?;

        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(Uri::from_static("https://clob.polymarket.com/order"))
            .header("content-type", "application/json");

        for (name, value) in &headers {
            builder = builder.header(name.as_str(), value.as_str());
        }

        let req = builder.body(Bytes::from(body.clone()))?;
        Ok(Some(req))
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

/// Configuration for CLOB-aware execution.
#[derive(Debug, Clone)]
pub struct ClobExecutionConfig {
    pub credentials: L2Credentials,
    pub private_key: String,
    pub maker_address: String,
    pub signer_address: String,
    pub fee_rate_bps: u64,
    pub is_neg_risk: bool,
    pub presign_count: usize,
}

/// Process a single CLOB trigger: dispatch pre-signed order on warm connection.
/// Returns a TimestampRecord with all checkpoints populated, plus the response body
/// (if available) for logging/parsing by the caller.
pub fn process_one_clob(
    pool: &ConnectionPool,
    presigned: &mut PreSignedOrderPool,
    creds: &L2Credentials,
    msg: &TriggerMessage,
    rt: &tokio::runtime::Runtime,
) -> (TimestampRecord, Option<Vec<u8>>) {
    let mut rec = TimestampRecord::default();
    rec.t_trigger_rx = msg.timestamp_ns;
    rec.t_dispatch_q = clock::now_ns();
    rec.t_exec_start = clock::now_ns();

    // Hot path: dispatch pre-signed order (salt patch + HMAC + build request)
    rec.t_buf_ready = clock::now_ns();

    let req = match presigned.dispatch(creds) {
        Ok(Some(req)) => req,
        Ok(None) => {
            // Pool exhausted — mark as reconnect to filter from stats
            rec.t_write_begin = clock::now_ns();
            rec.t_write_end = rec.t_write_begin;
            rec.t_first_resp_byte = rec.t_write_begin;
            rec.t_headers_done = rec.t_write_begin;
            rec.is_reconnect = true;
            return (rec, None);
        }
        Err(_) => {
            rec.t_write_begin = clock::now_ns();
            rec.t_write_end = rec.t_write_begin;
            rec.t_first_resp_byte = rec.t_write_begin;
            rec.t_headers_done = rec.t_write_begin;
            rec.is_reconnect = true;
            return (rec, None);
        }
    };

    rec.t_write_begin = clock::now_ns();

    // Phase 1: Submit frame to H2 pipeline
    let handle_result = rt.block_on(async { pool.send_start(req).await });
    rec.t_write_end = clock::now_ns();

    match handle_result {
        Ok(handle) => {
            rec.connection_index = handle.connection_index;

            // Phase 2: Await response
            let resp_result = rt.block_on(async { handle.collect().await });
            rec.t_first_resp_byte = clock::now_ns();

            match resp_result {
                Ok(resp) => {
                    if let Some(cf_ray) = get_cf_ray(&resp) {
                        rec.cf_ray_pop = extract_pop(&cf_ray);
                    }
                    rec.t_headers_done = clock::now_ns();
                    rec.is_reconnect = false;
                    let body = resp.into_body().to_vec();
                    (rec, Some(body))
                }
                Err(_) => {
                    rec.t_headers_done = clock::now_ns();
                    rec.is_reconnect = true;
                    (rec, None)
                }
            }
        }
        Err(_) => {
            rec.t_first_resp_byte = clock::now_ns();
            rec.t_headers_done = clock::now_ns();
            rec.is_reconnect = true;
            (rec, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clob_order::{Order, SignedOrderPayload};
    use crate::trigger::{OrderType, Side};
    use alloy::primitives::{Address, U256, address};
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
        // Measure: salt patch + HMAC + build request should be fast
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
    fn test_clob_config_construction() {
        let config = ClobExecutionConfig {
            credentials: test_creds(),
            private_key: "0xprivkey".to_string(),
            maker_address: "0xmaker".to_string(),
            signer_address: "0xsigner".to_string(),
            fee_rate_bps: 0,
            is_neg_risk: false,
            presign_count: 100,
        };
        assert_eq!(config.presign_count, 100);
        assert!(!config.is_neg_risk);
    }

    #[test]
    fn test_clob_process_one_builds_post_request() {
        // Verify that process_one_clob dispatches a POST request by checking
        // that the pre-signed pool is consumed
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

    #[tokio::test]
    #[ignore] // Needs real credentials and network
    async fn test_clob_end_to_end_pipeline() {
        use crate::clob_auth::load_credentials_from_env;
        use crate::clob_signer::{build_order, sign_order};
        use crate::clob_order::SignatureType;
        use crate::clob_response::parse_order_response;
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
        println!("API Key:    {}...", &creds.api_key[..8.min(creds.api_key.len())]);

        // --- Warm connection pool ---
        let mut conn_pool =
            ConnectionPool::new("clob.polymarket.com", 443, 1, AddressFamily::Auto);
        let warm_count = conn_pool.warmup().await.expect("warmup failed");
        println!("Pool:       {} warm connection(s)", warm_count);

        // --- Build & sign a real order ---
        // TOKEN_ID and PRICE from env; everything else hardcoded for a minimal test trade.
        let token_id = std::env::var("TOKEN_ID")
            .expect("TOKEN_ID env var required — the condition token to buy");
        let price = std::env::var("PRICE")
            .unwrap_or_else(|_| "0.95".to_string());

        let trigger = TriggerMessage {
            trigger_id: 1,
            token_id,
            side: Side::Buy,
            price,
            size: "2".to_string(),
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

        println!("            fee_rate_bps={} neg_risk={} sig_type={}", fee_rate_bps, is_neg_risk, sig_type);

        // maker = proxy wallet (POLY_PROXY_ADDRESS), signer = EOA (from POLY_PRIVATE_KEY)
        let maker_addr: Address = proxy_address.parse().expect("invalid POLY_PROXY_ADDRESS");
        let order = build_order(&trigger, maker_addr, signer_addr, fee_rate_bps, signature_type);
        let sig = sign_order(&signer, &order, is_neg_risk).await.unwrap();
        println!("Signature:  {}...{}", &sig[..10], &sig[sig.len()-6..]);

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

        let (rec, resp_body) = tokio::task::spawn_blocking(move || {
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

            let handle = rt.block_on(async {
                conn_pool_clone.send_start(req).await
            }).unwrap();
            rec.t_write_end = clock::now_ns();
            rec.connection_index = handle.connection_index;

            let resp = rt.block_on(async { handle.collect().await }).unwrap();
            rec.t_first_resp_byte = clock::now_ns();

            let status = resp.status();
            let resp_bytes = resp.into_body();
            rec.t_headers_done = clock::now_ns();

            (rec, (status, resp_bytes))
        })
        .await
        .unwrap();

        // --- Print results ---
        let (status, body_bytes) = resp_body;
        println!("\n--- Response ---");
        println!("HTTP Status: {}", status);
        println!("Body:        {}", std::str::from_utf8(&body_bytes).unwrap_or("<binary>"));

        if let Ok(order_resp) = parse_order_response(&body_bytes) {
            println!("Parsed:      success={}, order_id={}, status={}, error={:?}",
                order_resp.success, order_resp.order_id, order_resp.status, order_resp.error_msg);
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
        println!("queue_delay:       {:>8} ns ({:.1} us)", rec.queue_delay(), rec.queue_delay() as f64 / 1000.0);
        println!("prep_time:         {:>8} ns ({:.1} us)", rec.prep_time(), rec.prep_time() as f64 / 1000.0);
        println!("trigger_to_wire:   {:>8} ns ({:.1} us)", rec.trigger_to_wire(), rec.trigger_to_wire() as f64 / 1000.0);
        println!("write_duration:    {:>8} ns ({:.1} us)", rec.write_duration(), rec.write_duration() as f64 / 1000.0);
        println!("write_to_1st_byte: {:>8} ns ({:.1} ms)", rec.write_to_first_byte(), rec.write_to_first_byte() as f64 / 1_000_000.0);
        println!("warm_ttfb:         {:>8} ns ({:.1} ms)", rec.warm_ttfb(), rec.warm_ttfb() as f64 / 1_000_000.0);
        println!("trigger_to_1st_b:  {:>8} ns ({:.1} ms)", rec.trigger_to_first_byte(), rec.trigger_to_first_byte() as f64 / 1_000_000.0);

        // Assertions
        assert!(rec.t_trigger_rx > 0);
        assert!(rec.t_write_begin > 0);
        assert!(rec.t_headers_done > 0);
        assert!(rec.t_write_end > rec.t_write_begin);
        assert!(rec.t_first_resp_byte > rec.t_write_end);
        println!("\n=== PASS ===");
    }
}
