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
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::clob_auth::{build_l2_headers, L2Credentials};
use crate::clob_order::SignedOrderPayload;

const ORDER_METHOD: &str = "POST";
const ORDER_PATH: &str = "/order";
const ORDER_URI: &str = "https://clob.polymarket.com/order";

#[derive(Debug)]
pub enum RequestBuildError {
    Serialize(serde_json::Error),
    Utf8(std::str::Utf8Error),
    Time(std::time::SystemTimeError),
    Auth(String),
    Http(http::Error),
}

impl fmt::Display for RequestBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(err) => write!(f, "failed to serialize signed order payload: {err}"),
            Self::Utf8(err) => write!(f, "request body is not valid UTF-8: {err}"),
            Self::Time(err) => write!(f, "failed to read current unix timestamp: {err}"),
            Self::Auth(err) => write!(f, "failed to build L2 auth headers: {err}"),
            Self::Http(err) => write!(f, "failed to assemble HTTP request: {err}"),
        }
    }
}

impl std::error::Error for RequestBuildError {}

fn unix_timestamp_secs() -> Result<String, RequestBuildError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(RequestBuildError::Time)?
        .as_secs()
        .to_string())
}

pub fn encode_order_payload(signed_order: &SignedOrderPayload) -> Result<Bytes, RequestBuildError> {
    serde_json::to_vec(signed_order)
        .map(Bytes::from)
        .map_err(RequestBuildError::Serialize)
}

pub fn build_order_request_from_bytes(
    body: Bytes,
    creds: &L2Credentials,
) -> Result<Request<Bytes>, RequestBuildError> {
    let timestamp = unix_timestamp_secs()?;
    build_order_request_from_bytes_with_timestamp(body, creds, &timestamp)
}

pub fn build_order_request_from_bytes_with_timestamp(
    body: Bytes,
    creds: &L2Credentials,
    timestamp: &str,
) -> Result<Request<Bytes>, RequestBuildError> {
    let body_str = std::str::from_utf8(body.as_ref()).map_err(RequestBuildError::Utf8)?;
    let headers = build_l2_headers(creds, timestamp, ORDER_METHOD, ORDER_PATH, body_str)
        .map_err(|err| RequestBuildError::Auth(err.to_string()))?;

    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(Uri::from_static(ORDER_URI))
        .header("content-type", "application/json");

    for (name, value) in &headers {
        builder = builder.header(name.as_str(), value.as_str());
    }

    builder.body(body).map_err(RequestBuildError::Http)
}

pub fn build_order_request(
    signed_order: &SignedOrderPayload,
    creds: &L2Credentials,
) -> Result<Request<Bytes>, RequestBuildError> {
    let body = encode_order_payload(signed_order)?;
    build_order_request_from_bytes(body, creds)
}

pub fn build_order_request_with_timestamp(
    signed_order: &SignedOrderPayload,
    creds: &L2Credentials,
    timestamp: &str,
) -> Result<Request<Bytes>, RequestBuildError> {
    let body = encode_order_payload(signed_order)?;
    build_order_request_from_bytes_with_timestamp(body, creds, timestamp)
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

        assert!(req.headers().get("POLY_ADDRESS").is_some());
        assert!(req.headers().get("POLY_API_KEY").is_some());
        assert!(req.headers().get("POLY_PASSPHRASE").is_some());
        assert!(req.headers().get("POLY_SIGNATURE").is_some());
        assert!(req.headers().get("POLY_TIMESTAMP").is_some());

        let body = req.body();
        let v: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert!(v["order"].is_object());
        assert_eq!(v["orderType"].as_str().unwrap(), "FOK");
    }

    #[test]
    fn test_build_order_request_from_cached_body_matches_payload_builder() {
        let creds = test_creds();
        let payload = test_signed_order();
        let timestamp = "1700000000";

        let from_payload = build_order_request_with_timestamp(&payload, &creds, timestamp).unwrap();
        let cached_body = encode_order_payload(&payload).unwrap();
        let from_cached =
            build_order_request_from_bytes_with_timestamp(cached_body, &creds, timestamp).unwrap();

        assert_eq!(from_payload.headers(), from_cached.headers());
        assert_eq!(from_payload.body(), from_cached.body());
    }
}
