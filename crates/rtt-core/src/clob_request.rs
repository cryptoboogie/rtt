//! Request construction for signed CLOB orders.
//!
//! The supported public path is to build a request from a fully signed payload.
//! Signed payload template mutation is intentionally not part of the public API.
//!
//! ```compile_fail
//! use rtt_core::clob_request::{
//!     build_order_template,
//!     build_request_from_template,
//!     find_salt_position,
//! };
//! ```

use bytes::Bytes;
use http::{Method, Request, Uri};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::clob_auth::{build_l2_headers, L2Credentials};
use crate::clob_order::SignedOrderPayload;

/// Build a POST /order request from a SignedOrderPayload and L2 credentials.
pub fn build_order_request(
    signed_order: &SignedOrderPayload,
    creds: &L2Credentials,
) -> Result<Request<Bytes>, Box<dyn std::error::Error>> {
    let body = serde_json::to_string(signed_order)?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .to_string();

    let headers = build_l2_headers(creds, &timestamp, "POST", "/order", &body)?;

    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(Uri::from_static("https://clob.polymarket.com/order"))
        .header("content-type", "application/json");

    for (name, value) in &headers {
        builder = builder.header(name.as_str(), value.as_str());
    }

    let req = builder.body(Bytes::from(body))?;
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clob_order::{Order, SignedOrderPayload};
    use crate::trigger::OrderType;
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

    fn test_signed_order() -> SignedOrderPayload {
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
        SignedOrderPayload::new(&order, "0xdeadbeef", OrderType::FOK, "owner-uuid")
    }

    #[test]
    fn test_build_order_request() {
        let creds = test_creds();
        let payload = test_signed_order();
        let req = build_order_request(&payload, &creds).unwrap();

        assert_eq!(req.method(), Method::POST);
        assert!(req.uri().to_string().contains("/order"));
        assert_eq!(
            req.headers().get("content-type").unwrap(),
            "application/json"
        );

        // All POLY_* headers present
        assert!(req.headers().get("POLY_ADDRESS").is_some());
        assert!(req.headers().get("POLY_API_KEY").is_some());
        assert!(req.headers().get("POLY_PASSPHRASE").is_some());
        assert!(req.headers().get("POLY_SIGNATURE").is_some());
        assert!(req.headers().get("POLY_TIMESTAMP").is_some());

        // Body is valid JSON
        let body = req.body();
        let v: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert!(v["order"].is_object());
        assert_eq!(v["orderType"].as_str().unwrap(), "FOK");
    }
}
