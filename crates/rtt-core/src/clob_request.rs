use bytes::Bytes;
use http::{Method, Request, Uri};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::clob_auth::{build_l2_headers, L2Credentials};
use crate::clob_order::SignedOrderPayload;
use crate::request::RequestTemplate;

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

/// Find the byte position of a sentinel salt value in serialized JSON.
/// Returns (offset, length) of the salt number in the JSON bytes.
pub fn find_salt_position(json: &[u8], sentinel_salt: u64) -> Option<(usize, usize)> {
    let salt_str = sentinel_salt.to_string();
    let needle = salt_str.as_bytes();

    // Search for the salt number after "salt":
    let json_str = std::str::from_utf8(json).ok()?;
    let salt_key = "\"salt\":";
    let key_pos = json_str.find(salt_key)?;
    let after_key = key_pos + salt_key.len();

    // Find the salt number starting position (skip whitespace)
    let rest = &json[after_key..];
    let num_start = rest.iter().position(|&b| b != b' ')?;
    let abs_start = after_key + num_start;

    // Verify the needle matches at this position
    if json[abs_start..].starts_with(needle) {
        Some((abs_start, needle.len()))
    } else {
        None
    }
}

/// Build an order request template with a pre-serialized body.
/// Returns (template, salt_patch_slot_index).
///
/// The template has the body pre-serialized with the given signed order.
/// At trigger time, patch the salt (zero-alloc), then recompute HMAC headers.
pub fn build_order_template(
    signed_order: &SignedOrderPayload,
) -> Result<(RequestTemplate, usize, u64), Box<dyn std::error::Error>> {
    let sentinel_salt = signed_order.order.salt;
    let body = serde_json::to_string(signed_order)?;
    let body_bytes = body.as_bytes();

    let (salt_offset, salt_len) = find_salt_position(body_bytes, sentinel_salt)
        .ok_or("could not find salt in serialized JSON")?;

    let mut template = RequestTemplate::new(
        Method::POST,
        "/order".parse::<Uri>()?,
    );
    template.add_header("content-type", "application/json");
    template.set_body(body_bytes);
    let slot = template.register_patch(salt_offset, salt_len);

    Ok((template, slot, sentinel_salt))
}

/// Build a request from a template with dynamic HMAC headers.
/// This is the hot-path: patch salt, compute HMAC, build request.
pub fn build_request_from_template(
    template: &RequestTemplate,
    creds: &L2Credentials,
) -> Result<Request<Bytes>, Box<dyn std::error::Error>> {
    let body_str = std::str::from_utf8(template.body_bytes())?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_secs()
        .to_string();

    let headers = build_l2_headers(creds, &timestamp, "POST", "/order", body_str)?;

    // Build a new request with the template body + dynamic headers
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(Uri::from_static("https://clob.polymarket.com/order"))
        .header("content-type", "application/json");

    for (name, value) in &headers {
        builder = builder.header(name.as_str(), value.as_str());
    }

    let req = builder.body(Bytes::copy_from_slice(template.body_bytes()))?;
    Ok(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clob_order::{Order, SignedOrderPayload};
    use crate::trigger::OrderType;
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

    #[test]
    fn test_find_salt_position() {
        let payload = test_signed_order();
        let json = serde_json::to_string(&payload).unwrap();
        let (offset, len) = find_salt_position(json.as_bytes(), 1234567890).unwrap();

        // Verify the bytes at that position match the salt
        assert_eq!(&json.as_bytes()[offset..offset + len], b"1234567890");
    }

    #[test]
    fn test_order_template_salt_patch() {
        let payload = test_signed_order();
        let (mut template, slot, _sentinel) = build_order_template(&payload).unwrap();

        // Patch with new salt of same length
        let new_salt = b"9876543210";
        template.patch(slot, new_salt);

        // Verify JSON is still valid
        let body = template.body_bytes();
        let v: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(v["order"]["salt"].as_u64().unwrap(), 9876543210);
        // Other fields unchanged
        assert_eq!(v["orderType"].as_str().unwrap(), "FOK");
    }

    #[test]
    fn test_build_request_from_template() {
        let payload = test_signed_order();
        let (template, _slot, _sentinel) = build_order_template(&payload).unwrap();
        let creds = test_creds();

        let req = build_request_from_template(&template, &creds).unwrap();
        assert_eq!(req.method(), Method::POST);
        assert!(req.headers().get("POLY_SIGNATURE").is_some());

        // Body is valid JSON
        let v: serde_json::Value = serde_json::from_slice(req.body()).unwrap();
        assert!(v["order"].is_object());
    }
}
