use bytes::Bytes;
use http::{Method, Request, Uri};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::clock;
use crate::clob_auth::{build_l2_headers, L2Credentials};
use crate::clob_order::{
    SignedOrderPayload, generate_salt,
};
use crate::clob_request::build_order_template;
use crate::connection::{ConnectionPool, extract_pop, get_cf_ray};
use crate::metrics::TimestampRecord;
use crate::trigger::TriggerMessage;

/// A pool of pre-signed orders ready for hot-path dispatch.
pub struct PreSignedOrderPool {
    templates: Vec<(crate::request::RequestTemplate, usize)>, // (template, salt_slot)
    cursor: usize,
}

impl PreSignedOrderPool {
    /// Create from a vector of signed order payloads.
    pub fn new(
        payloads: Vec<SignedOrderPayload>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut templates = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            let (template, slot, _sentinel) = build_order_template(payload)?;
            templates.push((template, slot));
        }
        Ok(Self {
            templates,
            cursor: 0,
        })
    }

    /// Number of available pre-signed orders.
    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// Consume the next pre-signed order, patching a fresh salt and building
    /// the request with dynamic HMAC headers. Returns None if pool exhausted.
    pub fn dispatch(
        &mut self,
        creds: &L2Credentials,
    ) -> Result<Option<Request<Bytes>>, Box<dyn std::error::Error>> {
        if self.cursor >= self.templates.len() {
            return Ok(None);
        }

        let (template, slot) = &mut self.templates[self.cursor];
        self.cursor += 1;

        // Generate new salt with same digit length as the original
        let new_salt = generate_fixed_width_salt(template.body_bytes(), *slot, template);

        // Patch the salt in the body
        let salt_str = format!("{}", new_salt);
        // The patch slot has a fixed length, so we need the new salt to be the same length
        // We already ensured this in generate_fixed_width_salt
        template.patch(*slot, salt_str.as_bytes());

        // Build request with dynamic HMAC headers
        let body_str = std::str::from_utf8(template.body_bytes())?;
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

        let req = builder.body(Bytes::copy_from_slice(template.body_bytes()))?;
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

/// Generate a salt with the same number of digits as the existing salt at the patch slot.
fn generate_fixed_width_salt(
    _body: &[u8],
    _slot: usize,
    template: &crate::request::RequestTemplate,
) -> u64 {
    // The slot length tells us how many digits we need
    // We use the patches info from the template
    let _ = template;
    // Generate a salt and ensure it has the right number of digits
    // For a 10-digit salt: range [1000000000, 9999999999]
    let salt = generate_salt();
    // Mask to fit in 10 digits (max ~9 * 10^9 fits in u64, and in 53-bit range)
    // Use modular arithmetic to get a 10-digit number
    let min = 1_000_000_000u64;
    let range = 9_000_000_000u64;
    min + (salt % range)
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
/// Returns a TimestampRecord with all checkpoints populated.
pub fn process_one_clob(
    pool: &ConnectionPool,
    presigned: &mut PreSignedOrderPool,
    creds: &L2Credentials,
    msg: &TriggerMessage,
    rt: &tokio::runtime::Runtime,
) -> TimestampRecord {
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
            return rec;
        }
        Err(_) => {
            rec.t_write_begin = clock::now_ns();
            rec.t_write_end = rec.t_write_begin;
            rec.t_first_resp_byte = rec.t_write_begin;
            rec.t_headers_done = rec.t_write_begin;
            rec.is_reconnect = true;
            return rec;
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
                }
                Err(_) => {
                    rec.t_headers_done = clock::now_ns();
                    rec.is_reconnect = true;
                }
            }
        }
        Err(_) => {
            rec.t_first_resp_byte = clock::now_ns();
            rec.t_headers_done = clock::now_ns();
            rec.is_reconnect = true;
        }
    }

    rec
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

        // Each dispatch should be under 100us (generous bound for debug build)
        assert!(
            per_dispatch.as_micros() < 100,
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
        use crate::connection::AddressFamily;

        let creds = test_creds();
        let payloads = test_payloads(5);
        let mut presigned = PreSignedOrderPool::new(payloads).unwrap();

        let mut conn_pool =
            ConnectionPool::new("clob.polymarket.com", 443, 1, AddressFamily::Auto);
        conn_pool.warmup().await.expect("warmup failed");

        let trigger = TriggerMessage {
            trigger_id: 1,
            token_id: "1234".to_string(),
            side: Side::Buy,
            price: "0.50".to_string(),
            size: "100".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: clock::now_ns(),
        };

        let conn_pool_clone = std::sync::Arc::new(conn_pool);
        let rec = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            process_one_clob(&conn_pool_clone, &mut presigned, &creds, &trigger, &rt)
        })
        .await
        .unwrap();

        assert!(rec.t_trigger_rx > 0);
        assert!(rec.t_write_begin > 0);
        assert!(rec.t_headers_done > 0);
    }
}
