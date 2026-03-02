use alloy::sol;
use alloy::primitives::{Address, U256};
use serde::Serialize;

use crate::trigger::{Side, OrderType};

sol! {
    #[derive(Debug, PartialEq, Eq)]
    struct Order {
        uint256 salt;
        address maker;
        address signer;
        address taker;
        uint256 tokenId;
        uint256 makerAmount;
        uint256 takerAmount;
        uint256 expiration;
        uint256 nonce;
        uint256 feeRateBps;
        uint8 side;
        uint8 signatureType;
    }
}

/// Exchange contract for standard markets on Polygon.
pub const EXCHANGE_ADDRESS: Address = address!("4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E");

/// Exchange contract for neg-risk markets on Polygon.
pub const NEG_RISK_EXCHANGE_ADDRESS: Address = address!("C5d563A36AE78145C45a50134d48A1215220f80a");

/// CLOB side: 0 = BUY, 1 = SELL (on-chain representation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClobSide {
    Buy = 0,
    Sell = 1,
}

impl From<Side> for ClobSide {
    fn from(s: Side) -> Self {
        match s {
            Side::Buy => ClobSide::Buy,
            Side::Sell => ClobSide::Sell,
        }
    }
}

/// Signature type for the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureType {
    Eoa = 0,
    Poly = 1,
    GnosisSafe = 2,
}

/// USDC has 6 decimal places.
const USDC_DECIMALS: u32 = 6;

/// Compute maker and taker amounts from price, size, and side.
///
/// For BUY: maker pays USDC (price * size), taker receives tokens (size).
/// For SELL: maker sells tokens (size), taker pays USDC (price * size).
///
/// All amounts are in 6-decimal fixed point (USDC units, 1e6 = $1).
pub fn compute_amounts(price: &str, size: &str, side: ClobSide) -> (U256, U256) {
    let price_f: f64 = price.parse().expect("invalid price");
    let size_f: f64 = size.parse().expect("invalid size");
    let scale = 10u64.pow(USDC_DECIMALS) as f64;

    let usdc_amount = (price_f * size_f * scale).trunc() as u64;
    let token_amount = (size_f * scale).trunc() as u64;

    match side {
        ClobSide::Buy => (U256::from(usdc_amount), U256::from(token_amount)),
        ClobSide::Sell => (U256::from(token_amount), U256::from(usdc_amount)),
    }
}

/// Generate a random salt masked to 53 bits for JSON number safety.
pub fn generate_salt() -> u64 {
    use rand::Rng;
    let raw: u64 = rand::thread_rng().gen();
    raw & ((1u64 << 53) - 1)
}

/// JSON-serializable wrapper for Order fields in the API-expected format.
#[derive(Debug, Serialize)]
pub struct OrderJson {
    pub salt: u64,
    pub maker: String,
    pub signer: String,
    pub taker: String,
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(rename = "makerAmount")]
    pub maker_amount: String,
    #[serde(rename = "takerAmount")]
    pub taker_amount: String,
    pub expiration: String,
    pub nonce: String,
    #[serde(rename = "feeRateBps")]
    pub fee_rate_bps: String,
    pub side: String,
    #[serde(rename = "signatureType")]
    pub signature_type: u8,
    pub signature: String,
}

impl OrderJson {
    pub fn from_order(order: &Order, signature: &str) -> Self {
        let side_str = if order.side == 0 { "BUY" } else { "SELL" };
        Self {
            salt: order.salt.to::<u64>(),
            maker: format!("{:?}", order.maker),
            signer: format!("{:?}", order.signer),
            taker: format!("{:?}", order.taker),
            token_id: order.tokenId.to_string(),
            maker_amount: order.makerAmount.to_string(),
            taker_amount: order.takerAmount.to_string(),
            expiration: order.expiration.to_string(),
            nonce: order.nonce.to_string(),
            fee_rate_bps: order.feeRateBps.to_string(),
            side: side_str.to_string(),
            signature_type: order.signatureType,
            signature: signature.to_string(),
        }
    }
}

/// Top-level signed order payload for POST /order.
#[derive(Debug, Serialize)]
pub struct SignedOrderPayload {
    pub order: OrderJson,
    #[serde(rename = "orderType")]
    pub order_type: String,
    pub owner: String,
}

/// Convenience: create the full payload.
impl SignedOrderPayload {
    pub fn new(order: &Order, signature: &str, order_type: OrderType, owner: &str) -> Self {
        let ot = match order_type {
            OrderType::GTC => "GTC",
            OrderType::GTD => "GTD",
            OrderType::FOK => "FOK",
            OrderType::FAK => "FAK",
        };
        Self {
            order: OrderJson::from_order(order, signature),
            order_type: ot.to_string(),
            owner: owner.to_string(),
        }
    }
}

// Re-export the alloy address! macro usage
use alloy::primitives::address;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_struct_fields() {
        let order = Order {
            salt: U256::from(12345u64),
            maker: Address::ZERO,
            signer: Address::ZERO,
            taker: Address::ZERO,
            tokenId: U256::from(9999u64),
            makerAmount: U256::from(100_000_000u64),
            takerAmount: U256::from(50_000_000u64),
            expiration: U256::ZERO,
            nonce: U256::ZERO,
            feeRateBps: U256::ZERO,
            side: 0,
            signatureType: 0,
        };
        assert_eq!(order.salt, U256::from(12345u64));
        assert_eq!(order.makerAmount, U256::from(100_000_000u64));
        assert_eq!(order.side, 0);
    }

    #[test]
    fn test_buy_amounts() {
        // 100 shares @ $0.45 → maker=45_000_000 (USDC), taker=100_000_000 (tokens)
        let (maker, taker) = compute_amounts("0.45", "100", ClobSide::Buy);
        assert_eq!(maker, U256::from(45_000_000u64));
        assert_eq!(taker, U256::from(100_000_000u64));
    }

    #[test]
    fn test_sell_amounts() {
        // 100 shares @ $0.45 → maker=100_000_000 (tokens), taker=45_000_000 (USDC)
        let (maker, taker) = compute_amounts("0.45", "100", ClobSide::Sell);
        assert_eq!(maker, U256::from(100_000_000u64));
        assert_eq!(taker, U256::from(45_000_000u64));
    }

    #[test]
    fn test_salt_masked_to_53_bits() {
        for _ in 0..100 {
            let salt = generate_salt();
            assert!(salt < (1u64 << 53), "salt {} exceeds 53-bit limit", salt);
        }
    }

    #[test]
    fn test_generate_salt_nonzero() {
        // At least one of 100 salts should be nonzero
        let nonzero = (0..100).any(|_| generate_salt() != 0);
        assert!(nonzero);
    }

    #[test]
    fn test_order_json_format() {
        let order = Order {
            salt: U256::from(479249096354u64),
            maker: address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
            signer: address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
            taker: Address::ZERO,
            tokenId: U256::from(1234u64),
            makerAmount: U256::from(100_000_000u64),
            takerAmount: U256::from(50_000_000u64),
            expiration: U256::ZERO,
            nonce: U256::ZERO,
            feeRateBps: U256::ZERO,
            side: 0,
            signatureType: 0,
        };
        let json = OrderJson::from_order(&order, "0xdeadbeef");
        let s = serde_json::to_string(&json).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();

        // salt is a JSON number
        assert!(v["salt"].is_number());
        assert_eq!(v["salt"].as_u64().unwrap(), 479249096354);

        // tokenId, amounts are strings
        assert_eq!(v["tokenId"].as_str().unwrap(), "1234");
        assert_eq!(v["makerAmount"].as_str().unwrap(), "100000000");
        assert_eq!(v["takerAmount"].as_str().unwrap(), "50000000");

        // addresses are 0x-hex
        assert!(v["maker"].as_str().unwrap().starts_with("0x"));

        // side is string
        assert_eq!(v["side"].as_str().unwrap(), "BUY");

        // signatureType is number
        assert_eq!(v["signatureType"].as_u64().unwrap(), 0);
    }

    #[test]
    fn test_signed_order_json_structure() {
        let order = Order {
            salt: U256::from(12345u64),
            maker: Address::ZERO,
            signer: Address::ZERO,
            taker: Address::ZERO,
            tokenId: U256::from(1234u64),
            makerAmount: U256::from(100_000_000u64),
            takerAmount: U256::from(50_000_000u64),
            expiration: U256::ZERO,
            nonce: U256::ZERO,
            feeRateBps: U256::ZERO,
            side: 0,
            signatureType: 0,
        };
        let payload = SignedOrderPayload::new(&order, "0xsig", OrderType::FOK, "owner-uuid");
        let s = serde_json::to_string(&payload).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();

        // Top-level keys
        assert!(v["order"].is_object());
        assert_eq!(v["orderType"].as_str().unwrap(), "FOK");
        assert_eq!(v["owner"].as_str().unwrap(), "owner-uuid");

        // order contains signature
        assert_eq!(v["order"]["signature"].as_str().unwrap(), "0xsig");
        assert_eq!(v["order"]["side"].as_str().unwrap(), "BUY");
    }

    #[test]
    fn test_exchange_addresses() {
        assert_ne!(EXCHANGE_ADDRESS, Address::ZERO);
        assert_ne!(NEG_RISK_EXCHANGE_ADDRESS, Address::ZERO);
        assert_ne!(EXCHANGE_ADDRESS, NEG_RISK_EXCHANGE_ADDRESS);
    }

    #[test]
    fn test_clob_side_from_side() {
        assert_eq!(ClobSide::from(Side::Buy), ClobSide::Buy);
        assert_eq!(ClobSide::from(Side::Sell), ClobSide::Sell);
    }
}
