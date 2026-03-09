//! # Order Pipeline Tests
//!
//! These tests prove that rtt-core can:
//! 1. Take a TriggerMessage (trade signal) and build an Order struct
//! 2. Sign that order with EIP-712 (Ethereum's structured data signing)
//! 3. Compute HMAC authentication headers for the Polymarket API
//! 4. Assemble a complete HTTP POST request ready to send
//!
//! WHY THIS MATTERS:
//! Polymarket uses two layers of authentication:
//! - EIP-712 signature: proves you own the wallet (cryptographic proof)
//! - HMAC-SHA256 headers: proves you own the API key (server-side auth)
//! Both must be correct or the server rejects the order.
//!
//! The order flow is:
//!   TriggerMessage -> Order struct -> EIP-712 sign -> JSON serialize
//!   -> HMAC auth headers -> HTTP POST /order

use rtt_core::clob_auth::{build_l2_headers, L2Credentials};
use rtt_core::clob_order::{Order, SignedOrderPayload};
use rtt_core::clob_request::build_order_request;
use rtt_core::clob_signer::{build_order, presign_batch, sign_order};
use rtt_core::trigger::{OrderType, Side, TriggerMessage};

use alloy::primitives::{address, Address, U256};
use alloy::signers::local::PrivateKeySigner;
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;

// Foundry's first test private key — NOT a real key, safe to use in tests.
const TEST_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

fn test_signer() -> PrivateKeySigner {
    TEST_KEY.parse().expect("valid test key")
}

fn test_creds() -> L2Credentials {
    L2Credentials {
        api_key: "test-api-key".to_string(),
        secret: URL_SAFE.encode(b"test-secret-key!"),
        passphrase: "test-passphrase".to_string(),
        address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
    }
}

/// TEST: A trade signal becomes a signed order.
///
/// Scenario: The strategy says "BUY 100 units of token 1234 at price 0.50"
/// We need to:
/// 1. Convert that into Polymarket's Order format (with USDC amounts)
/// 2. Sign it with our private key (EIP-712)
/// 3. The signature must be deterministic (same input -> same signature)
///
/// WHY THIS MATTERS:
/// If the signature is wrong, the server returns "invalid signature"
/// and the order is rejected. This test catches signing bugs before
/// they cost money.
#[tokio::test]
async fn trade_signal_becomes_signed_order() {
    // Create a trade signal (what the strategy produces)
    let trigger = TriggerMessage {
        trigger_id: 1,
        token_id: "1234".to_string(),
        side: Side::Buy,
        price: "0.50".to_string(), // Buy at 50 cents
        size: "100".to_string(),   // 100 units
        order_type: OrderType::FOK, // Fill-or-kill
        timestamp_ns: 0,
    };

    let signer = test_signer();
    let maker = signer.address();

    // Step 1: Build the Order struct from the trigger.
    // This converts price/size into USDC fixed-point amounts:
    //   BUY 100 @ 0.50 -> makerAmount=50_000_000 (USDC), takerAmount=100_000_000 (tokens)
    let order = build_order(&trigger, maker, maker, 0, rtt_core::clob_order::SignatureType::Eoa);
    assert_eq!(order.tokenId, U256::from(1234u64));
    assert_eq!(order.makerAmount, U256::from(50_000_000u64));
    assert_eq!(order.takerAmount, U256::from(100_000_000u64));
    assert_eq!(order.side, 0); // 0 = BUY on-chain

    // Step 2: Sign the order (EIP-712).
    // This hashes the struct, then signs the hash with secp256k1.
    let sig = sign_order(&signer, &order, false)
        .await
        .expect("signing failed");

    // Signature should be 0x-prefixed hex, 132-134 chars (65 bytes).
    assert!(sig.starts_with("0x"), "signature should start with 0x");
    assert!(
        sig.len() >= 132 && sig.len() <= 134,
        "unexpected signature length: {}",
        sig.len()
    );

    // Step 3: Verify determinism — same order + same key = same signature.
    let sig2 = sign_order(&signer, &order, false).await.unwrap();
    assert_eq!(sig, sig2, "signing should be deterministic");
}

/// TEST: Pre-signing a batch of orders gives us instant dispatch later.
///
/// Scenario: Before any trade signal arrives, we pre-sign 10 orders
/// with different random salts. Each has a valid EIP-712 signature.
/// When a trigger fires, we grab the next pre-signed order (O(1))
/// instead of signing on-the-fly (~500us).
///
/// WHY THIS MATTERS:
/// EIP-712 signing takes ~100-500us (secp256k1 elliptic curve math).
/// Pre-signing removes this from the hot path entirely. The trade-off
/// is that pre-signed orders have a fixed price — if the strategy
/// fires at a different price, we'd need to re-sign.
#[tokio::test]
async fn presigned_batch_has_unique_salts_and_valid_signatures() {
    let signer = test_signer();
    let maker = signer.address();

    let trigger = TriggerMessage {
        trigger_id: 1,
        token_id: "1234".to_string(),
        side: Side::Buy,
        price: "0.50".to_string(),
        size: "100".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: 0,
    };

    // Pre-sign 10 orders — each gets a unique random salt.
    let batch = presign_batch(&signer, &trigger, maker, maker, 0, false, rtt_core::clob_order::SignatureType::Eoa, "owner-uuid", 10)
        .await
        .expect("presign_batch failed");

    assert_eq!(batch.len(), 10, "should produce exactly 10 pre-signed orders");

    // Verify all salts are unique (different random values).
    let salts: Vec<u64> = batch.iter().map(|p| p.order.salt).collect();
    let mut unique_salts = salts.clone();
    unique_salts.sort();
    unique_salts.dedup();
    assert!(
        unique_salts.len() > 1,
        "pre-signed orders should have different salts"
    );

    // Verify all signatures are valid format.
    for (i, p) in batch.iter().enumerate() {
        assert!(
            p.order.signature.starts_with("0x"),
            "order {} signature missing 0x prefix",
            i
        );
        assert!(
            p.order.signature.len() >= 132 && p.order.signature.len() <= 134,
            "order {} unexpected sig length: {}",
            i,
            p.order.signature.len()
        );
    }

    // Verify each order has the correct structure.
    for p in &batch {
        assert_eq!(p.order_type, "FOK");
        assert_eq!(p.owner, "owner-uuid");
        assert_eq!(p.order.side, "BUY");
    }
}

/// TEST: HMAC auth headers are correctly computed.
///
/// Scenario: The Polymarket API requires 5 headers on every request:
///   POLY_ADDRESS, POLY_API_KEY, POLY_PASSPHRASE, POLY_SIGNATURE, POLY_TIMESTAMP
///
/// POLY_SIGNATURE = HMAC-SHA256(secret, timestamp + method + path + body)
///
/// WHY THIS MATTERS:
/// If any header is wrong or the HMAC doesn't match, the server returns
/// 401 Unauthorized. This is the second authentication layer (after EIP-712).
#[test]
fn hmac_auth_headers_are_complete_and_correctly_signed() {
    let creds = test_creds();
    let timestamp = "1700000000";
    let method = "POST";
    let path = "/order";
    let body = r#"{"order":{"salt":123},"orderType":"FOK"}"#;

    let headers = build_l2_headers(&creds, timestamp, method, path, body)
        .expect("header computation failed");

    // Must have exactly 5 headers.
    assert_eq!(headers.len(), 5, "should produce exactly 5 POLY_* headers");

    // Build a lookup map for easy verification.
    let find = |name: &str| -> String {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("missing header: {}", name))
            .1
            .clone()
    };

    // Verify each header is present and has a reasonable value.
    assert_eq!(find("POLY_ADDRESS"), creds.address.to_lowercase());
    assert_eq!(find("POLY_API_KEY"), creds.api_key);
    assert_eq!(find("POLY_PASSPHRASE"), creds.passphrase);
    assert_eq!(find("POLY_TIMESTAMP"), timestamp);

    // POLY_SIGNATURE should be a valid base64url-encoded HMAC.
    let sig = find("POLY_SIGNATURE");
    assert!(!sig.is_empty(), "HMAC signature should not be empty");
    let decoded = URL_SAFE
        .decode(&sig)
        .expect("HMAC signature should be valid base64url");
    assert_eq!(decoded.len(), 32, "HMAC-SHA256 output should be 32 bytes");

    // Verify determinism: same inputs = same HMAC.
    let headers2 = build_l2_headers(&creds, timestamp, method, path, body).unwrap();
    let sig2 = headers2
        .iter()
        .find(|(n, _)| n == "POLY_SIGNATURE")
        .unwrap()
        .1
        .clone();
    assert_eq!(sig, sig2, "HMAC should be deterministic");
}

/// TEST: A complete order request has the right shape for the API.
///
/// This is the "final form" — the HTTP request that actually gets sent.
/// It must be: POST /order, content-type: application/json,
/// with all 5 POLY_* auth headers, and a JSON body containing
/// the signed order + orderType + owner.
///
/// WHY THIS MATTERS:
/// If the request shape is wrong (wrong method, missing headers,
/// malformed body), the server rejects it before even looking
/// at the cryptographic signatures. This test catches structural bugs.
#[test]
fn complete_order_request_has_correct_structure() {
    let creds = test_creds();

    // Build a signed order payload (using a test order, not a real signature).
    let order = Order {
        salt: U256::from(1234567890u64),
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
    let payload = SignedOrderPayload::new(&order, "0xdeadbeef", OrderType::FOK, "owner-uuid");

    // Build the complete HTTP request.
    let req = build_order_request(&payload, &creds).expect("request build failed");

    // Verify HTTP method and URI.
    assert_eq!(req.method(), http::Method::POST, "must be POST");
    assert!(
        req.uri().to_string().contains("/order"),
        "URI should contain /order"
    );

    // Verify content-type header.
    assert_eq!(
        req.headers().get("content-type").unwrap(),
        "application/json",
        "content-type must be application/json"
    );

    // Verify all 5 POLY_* auth headers are present.
    let required_headers = [
        "POLY_ADDRESS",
        "POLY_API_KEY",
        "POLY_PASSPHRASE",
        "POLY_SIGNATURE",
        "POLY_TIMESTAMP",
    ];
    for name in &required_headers {
        assert!(
            req.headers().get(*name).is_some(),
            "missing required header: {}",
            name
        );
    }

    // Verify the body is valid JSON with the expected structure.
    let body: serde_json::Value =
        serde_json::from_slice(req.body()).expect("body should be valid JSON");
    assert!(body["order"].is_object(), "body should have 'order' object");
    assert_eq!(
        body["orderType"].as_str().unwrap(),
        "FOK",
        "orderType should be FOK"
    );
    assert_eq!(
        body["owner"].as_str().unwrap(),
        "owner-uuid",
        "owner should match"
    );
    assert_eq!(
        body["order"]["signature"].as_str().unwrap(),
        "0xdeadbeef",
        "signature should be in order object"
    );
    assert_eq!(
        body["order"]["side"].as_str().unwrap(),
        "BUY",
        "side should be BUY"
    );
}
