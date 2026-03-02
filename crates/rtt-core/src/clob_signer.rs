use alloy::primitives::{Address, U256};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::sol_types::eip712_domain;
use alloy::sol_types::SolStruct;

use crate::clob_order::{
    ClobSide, Order, SignatureType, SignedOrderPayload,
    EXCHANGE_ADDRESS, NEG_RISK_EXCHANGE_ADDRESS,
    compute_amounts, generate_salt,
};
use crate::trigger::{OrderType, TriggerMessage};

const CHAIN_ID: u64 = 137; // Polygon mainnet

/// Build the EIP-712 domain for order signing.
pub fn make_domain(is_neg_risk: bool) -> alloy::sol_types::Eip712Domain {
    let exchange = if is_neg_risk {
        NEG_RISK_EXCHANGE_ADDRESS
    } else {
        EXCHANGE_ADDRESS
    };
    eip712_domain! {
        name: "Polymarket CTF Exchange",
        version: "1",
        chain_id: CHAIN_ID,
        verifying_contract: exchange,
    }
}

/// Sign an order with a local signer, returning the hex signature string.
pub async fn sign_order(
    signer: &PrivateKeySigner,
    order: &Order,
    is_neg_risk: bool,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let domain = make_domain(is_neg_risk);
    let hash = order.eip712_signing_hash(&domain);
    let signature = signer.sign_hash(&hash).await?;
    Ok(format!("0x{}", signature))
}

/// Build an Order from a TriggerMessage.
pub fn build_order(
    trigger: &TriggerMessage,
    maker: Address,
    signer_addr: Address,
    fee_rate_bps: u64,
) -> Order {
    let clob_side = ClobSide::from(trigger.side);
    let (maker_amount, taker_amount) = compute_amounts(&trigger.price, &trigger.size, clob_side);
    let token_id = U256::from_str_radix(trigger.token_id.as_str(), 10)
        .unwrap_or_else(|_| U256::from(0u64));

    Order {
        salt: U256::from(generate_salt()),
        maker,
        signer: signer_addr,
        taker: Address::ZERO,
        tokenId: token_id,
        makerAmount: maker_amount,
        takerAmount: taker_amount,
        expiration: U256::ZERO,
        nonce: U256::ZERO,
        feeRateBps: U256::from(fee_rate_bps),
        side: clob_side as u8,
        signatureType: SignatureType::Eoa as u8,
    }
}

/// Pre-sign a batch of orders for the same trigger parameters but different salts.
pub async fn presign_batch(
    signer: &PrivateKeySigner,
    trigger: &TriggerMessage,
    maker: Address,
    signer_addr: Address,
    fee_rate_bps: u64,
    is_neg_risk: bool,
    owner: &str,
    count: usize,
) -> Result<Vec<SignedOrderPayload>, Box<dyn std::error::Error + Send + Sync>> {
    let mut results = Vec::with_capacity(count);
    for _ in 0..count {
        let order = build_order(trigger, maker, signer_addr, fee_rate_bps);
        let sig = sign_order(signer, &order, is_neg_risk).await?;
        let payload = SignedOrderPayload::new(&order, &sig, trigger.order_type, owner);
        results.push(payload);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::Side;
    use alloy::primitives::address;
    use alloy::signers::local::PrivateKeySigner;

    // Foundry test key
    const TEST_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn test_signer() -> PrivateKeySigner {
        TEST_KEY.parse().expect("valid test key")
    }

    #[test]
    fn test_domain_standard_exchange() {
        let domain = make_domain(false);
        let vc = domain.verifying_contract.unwrap();
        assert_eq!(vc, EXCHANGE_ADDRESS);
    }

    #[test]
    fn test_domain_neg_risk_exchange() {
        let domain = make_domain(true);
        let vc = domain.verifying_contract.unwrap();
        assert_eq!(vc, NEG_RISK_EXCHANGE_ADDRESS);
    }

    #[tokio::test]
    async fn test_sign_order_produces_valid_signature() {
        let signer = test_signer();
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
            feeRateBps: U256::from(100u64),
            side: 0,
            signatureType: 0,
        };
        let sig = sign_order(&signer, &order, false).await.unwrap();
        assert!(sig.starts_with("0x"), "signature should start with 0x");
        // 0x + 130 hex chars (65 bytes = r[32] + s[32] + v[1])
        // 0x + 130 or 132 hex chars depending on v encoding
        assert!(sig.len() >= 132 && sig.len() <= 134, "unexpected sig length {}", sig.len());
    }

    #[tokio::test]
    async fn test_sign_order_deterministic() {
        let signer = test_signer();
        let order = Order {
            salt: U256::from(12345u64),
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
        let sig1 = sign_order(&signer, &order, false).await.unwrap();
        let sig2 = sign_order(&signer, &order, false).await.unwrap();
        assert_eq!(sig1, sig2, "same order should produce same signature");
    }

    #[test]
    fn test_build_order_from_trigger() {
        let trigger = TriggerMessage {
            trigger_id: 1,
            token_id: "1234".to_string(),
            side: Side::Buy,
            price: "0.50".to_string(),
            size: "100".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 0,
        };
        let maker = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let order = build_order(&trigger, maker, maker, 0);

        assert_eq!(order.maker, maker);
        assert_eq!(order.signer, maker);
        assert_eq!(order.taker, Address::ZERO);
        assert_eq!(order.tokenId, U256::from(1234u64));
        // Buy: maker=50_000_000 (USDC), taker=100_000_000 (tokens)
        assert_eq!(order.makerAmount, U256::from(50_000_000u64));
        assert_eq!(order.takerAmount, U256::from(100_000_000u64));
        assert_eq!(order.side, 0); // Buy
        assert_eq!(order.signatureType, 0); // EOA
    }

    #[tokio::test]
    async fn test_presign_batch() {
        let signer = test_signer();
        let maker = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        let trigger = TriggerMessage {
            trigger_id: 1,
            token_id: "1234".to_string(),
            side: Side::Buy,
            price: "0.50".to_string(),
            size: "100".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 0,
        };
        let batch = presign_batch(&signer, &trigger, maker, maker, 0, false, "owner-uuid", 10)
            .await
            .unwrap();
        assert_eq!(batch.len(), 10);

        // All should have different salts
        let salts: Vec<u64> = batch.iter().map(|p| p.order.salt).collect();
        let mut unique_salts = salts.clone();
        unique_salts.sort();
        unique_salts.dedup();
        assert!(unique_salts.len() > 1, "should have different salts");

        // All signatures should be valid (start with 0x, correct length)
        for p in &batch {
            assert!(p.order.signature.starts_with("0x"));
            assert!(p.order.signature.len() >= 132 && p.order.signature.len() <= 134);
        }
    }
}
