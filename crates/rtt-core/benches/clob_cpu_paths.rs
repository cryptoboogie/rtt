use alloy::primitives::{address, Address, U256};
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use rtt_core::clob_auth::L2Credentials;
use rtt_core::clob_executor::PreSignedOrderPool;
use rtt_core::clob_order::{compute_amounts, ClobSide, Order, SignedOrderPayload};
use rtt_core::clob_request::{build_order_request_from_bytes_with_timestamp, encode_order_payload};
use rtt_core::trigger::OrderType;

fn test_creds() -> L2Credentials {
    L2Credentials {
        api_key: "bench-api-key".to_string(),
        secret: URL_SAFE.encode(b"bench-secret-key!"),
        passphrase: "bench-pass".to_string(),
        address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
    }
}

fn test_payload(index: u64) -> SignedOrderPayload {
    let order = Order {
        salt: U256::from(1234567890u64 + index),
        maker: address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
        signer: address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"),
        taker: Address::ZERO,
        tokenId: U256::from(9999u64),
        makerAmount: U256::from(31_500_000u64),
        takerAmount: U256::from(50_000_000u64),
        expiration: U256::ZERO,
        nonce: U256::ZERO,
        feeRateBps: U256::ZERO,
        side: 0,
        signatureType: 0,
    };
    SignedOrderPayload::new(&order, "0xdeadbeef", OrderType::FOK, "owner-uuid")
}

fn test_payloads(count: usize) -> Vec<SignedOrderPayload> {
    (0..count).map(|index| test_payload(index as u64)).collect()
}

fn bench_compute_amounts(c: &mut Criterion) {
    let mut group = c.benchmark_group("clob_amounts");
    group.bench_function("buy", |b| {
        b.iter(|| {
            let amounts = compute_amounts(black_box("0.63"), black_box("50"), ClobSide::Buy)
                .expect("bench amount conversion should succeed");
            black_box(amounts);
        })
    });
    group.bench_function("sell", |b| {
        b.iter(|| {
            let amounts = compute_amounts(black_box("0.41"), black_box("25"), ClobSide::Sell)
                .expect("bench amount conversion should succeed");
            black_box(amounts);
        })
    });
    group.finish();
}

fn bench_request_build(c: &mut Criterion) {
    let creds = test_creds();
    let body = encode_order_payload(&test_payload(0)).expect("bench payload encoding should work");

    c.bench_function("clob_request_build_from_cached_body", |b| {
        b.iter(|| {
            let req = build_order_request_from_bytes_with_timestamp(
                body.clone(),
                &creds,
                black_box("1700000000"),
            )
            .expect("bench request assembly should succeed");
            black_box(req);
        })
    });
}

fn bench_presigned_dispatch(c: &mut Criterion) {
    let creds = test_creds();

    c.bench_function("presigned_pool_dispatch_with_cached_body", |b| {
        b.iter_batched(
            || PreSignedOrderPool::new(test_payloads(64)).expect("bench pool build should work"),
            |mut pool| {
                for _ in 0..64 {
                    let req = pool
                        .dispatch_with_timestamp(&creds, black_box("1700000000"))
                        .expect("bench dispatch should build a request");
                    black_box(req);
                }
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(
    clob_cpu_paths,
    bench_compute_amounts,
    bench_request_build,
    bench_presigned_dispatch
);
criterion_main!(clob_cpu_paths);
