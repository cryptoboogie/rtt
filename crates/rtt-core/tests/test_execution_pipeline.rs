//! # Execution Pipeline Tests
//!
//! These tests prove that rtt-core can:
//! 1. Dispatch a pre-signed order from the pool onto a warm H2 connection
//! 2. Record timestamps at 8 checkpoints through the hot path
//! 3. Compute derived latency metrics (trigger-to-wire, write duration, etc.)
//!
//! WHY THIS MATTERS:
//! This is the core of the system — the "hot path." When a trade signal
//! arrives, this code path determines how fast the order reaches Polymarket.
//! Every microsecond matters. The 8 timestamps let us see exactly where
//! time is spent:
//!
//!   t_trigger_rx    -> when the trigger was received
//!   t_dispatch_q    -> when it was dequeued for processing
//!   t_exec_start    -> when execution began
//!   t_buf_ready     -> when the request bytes were ready
//!   t_write_begin   -> when we started writing to the connection
//!   t_write_end     -> when the H2 frame was submitted to the kernel
//!   t_first_resp_byte -> when the first response byte arrived
//!   t_headers_done  -> when we finished processing the response
//!
//! Derived metrics:
//!   trigger_to_wire = t_write_begin - t_trigger_rx  (what WE control)
//!   warm_ttfb = t_first_resp_byte - t_write_begin   (network physics)

use rtt_core::clob_auth::L2Credentials;
use rtt_core::clob_order::{Order, SignedOrderPayload};
use rtt_core::connection::{AddressFamily, ConnectionPool};
use rtt_core::executor::{ExecutionThread, IngressThread};
use rtt_core::queue::TriggerQueue;
use rtt_core::trigger::{OrderType, Side, TriggerMessage};
use rtt_core::{
    process_one_clob, sign_and_dispatch, DispatchError, DispatchOutcome, PreSignedOrderPool,
};

use alloy::primitives::{address, Address, U256};
use alloy::signers::local::PrivateKeySigner;
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use std::sync::Arc;
use std::time::Duration;

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

fn test_signer() -> PrivateKeySigner {
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        .parse()
        .expect("valid test signer")
}

/// TEST: The hot path dispatches a pre-signed order in under 1ms.
///
/// This is the dispatch speed test. It measures how long the
/// PreSignedOrderPool::dispatch() call takes — the part where we:
/// 1. Grab the next pre-signed body (O(1) cursor advance)
/// 2. Recompute HMAC auth headers (with fresh timestamp)
/// 3. Build the HTTP request
///
/// WHY THIS MATTERS:
/// This is pure CPU work, no network. In release builds this should
/// be well under 100us. In debug builds, under 1ms. If this is slow,
/// something is wrong with the signing or serialization code.
#[test]
fn hot_path_dispatch_is_fast() {
    let payloads = test_payloads(20);
    let mut pool = PreSignedOrderPool::new(payloads).unwrap();
    let creds = test_creds();

    // Warmup dispatch (first call may be slower due to code paths being cold).
    let _ = pool.dispatch(&creds).unwrap();

    // Measure 10 dispatches.
    let start = std::time::Instant::now();
    for _ in 0..10 {
        let req = pool.dispatch(&creds).unwrap();
        assert!(req.is_some(), "pool should not be exhausted yet");

        // Verify each dispatch produces a valid POST request.
        let req = req.unwrap();
        assert_eq!(req.method(), http::Method::POST);
        assert!(req.headers().get("POLY_SIGNATURE").is_some());
    }
    let elapsed = start.elapsed();
    let per_dispatch = elapsed / 10;

    println!("\n=== Execution Pipeline: Dispatch Speed ===");
    println!("Total (10 dispatches): {:?}", elapsed);
    println!("Per dispatch:          {:?}", per_dispatch);

    // Each dispatch should be under 1ms in debug builds.
    assert!(
        per_dispatch.as_micros() < 1000,
        "dispatch took {:?} per call, expected <1ms",
        per_dispatch
    );

    println!("=== PASS ===\n");
}

/// TEST: Full execution records all 8 timestamps and they're monotonic.
///
/// This is the most important integration test for rtt-core.
/// It fires a trigger through the full pipeline:
///   trigger -> dequeue -> build request -> write to connection -> await response
/// and verifies every timestamp is populated and in order.
///
/// WHY THIS MATTERS:
/// If any timestamp is 0 or out of order, our latency measurements
/// are wrong. Wrong measurements mean we can't tell if optimizations
/// are actually helping.
#[tokio::test]
async fn full_execution_records_all_timestamps_in_order() {
    println!("\n=== Execution Pipeline: Full Timestamp Chain ===");

    // Warm a connection pool.
    let mut conn_pool = ConnectionPool::new("clob.polymarket.com", 443, 1, AddressFamily::Auto);
    conn_pool.warmup().await.expect("warmup failed");

    // Build a pre-signed order pool.
    let payloads = test_payloads(5);
    let mut presigned = PreSignedOrderPool::new(payloads).unwrap();
    let creds = test_creds();

    // Initialize the clock epoch so now_ns() returns non-zero values.
    let _ = rtt_core::clock::now_ns();

    // Create a trigger with a known timestamp.
    let trigger = TriggerMessage {
        trigger_id: 1,
        token_id: "9999".to_string(),
        side: Side::Buy,
        price: "0.50".to_string(),
        size: "100".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: rtt_core::clock::now_ns(),
    };

    // Run process_one_clob on a blocking thread (it uses block_on internally).
    let (rec, _body) = tokio::task::spawn_blocking(
        move || -> (rtt_core::metrics::TimestampRecord, Option<Vec<u8>>) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            match process_one_clob(&conn_pool, &mut presigned, &creds, &trigger, &rt) {
                DispatchOutcome::Sent { record, body } => (record, body),
                DispatchOutcome::Rejected { error, .. } => {
                    panic!("dispatch should succeed: {error}")
                }
            }
        },
    )
    .await
    .unwrap();

    // All 8 timestamps should be populated (non-zero).
    println!("t_trigger_rx:      {}", rec.t_trigger_rx);
    println!("t_dispatch_q:      {}", rec.t_dispatch_q);
    println!("t_exec_start:      {}", rec.t_exec_start);
    println!("t_buf_ready:       {}", rec.t_buf_ready);
    println!("t_write_begin:     {}", rec.t_write_begin);
    println!("t_write_end:       {}", rec.t_write_end);
    println!("t_first_resp_byte: {}", rec.t_first_resp_byte);
    println!("t_headers_done:    {}", rec.t_headers_done);

    assert!(rec.t_trigger_rx > 0, "t_trigger_rx should be set");
    assert!(rec.t_dispatch_q > 0, "t_dispatch_q should be set");
    assert!(rec.t_exec_start > 0, "t_exec_start should be set");
    assert!(rec.t_buf_ready > 0, "t_buf_ready should be set");
    assert!(rec.t_write_begin > 0, "t_write_begin should be set");
    assert!(rec.t_write_end > 0, "t_write_end should be set");
    assert!(rec.t_first_resp_byte > 0, "t_first_resp_byte should be set");
    assert!(rec.t_headers_done > 0, "t_headers_done should be set");

    // Timestamps should be monotonically increasing.
    assert!(
        rec.t_dispatch_q >= rec.t_trigger_rx,
        "dispatch_q >= trigger_rx"
    );
    assert!(
        rec.t_exec_start >= rec.t_dispatch_q,
        "exec_start >= dispatch_q"
    );
    assert!(
        rec.t_buf_ready >= rec.t_exec_start,
        "buf_ready >= exec_start"
    );
    assert!(
        rec.t_write_begin >= rec.t_buf_ready,
        "write_begin >= buf_ready"
    );
    assert!(
        rec.t_write_end >= rec.t_write_begin,
        "write_end >= write_begin"
    );
    assert!(
        rec.t_first_resp_byte >= rec.t_write_end,
        "first_resp_byte >= write_end"
    );
    assert!(
        rec.t_headers_done >= rec.t_first_resp_byte,
        "headers_done >= first_resp_byte"
    );

    // Derived metrics should be consistent.
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
        "warm_ttfb:         {:>8} ns ({:.1} ms)",
        rec.warm_ttfb(),
        rec.warm_ttfb() as f64 / 1_000_000.0
    );

    println!("=== PASS ===\n");
}

#[test]
fn pool_exhaustion_is_classified_without_poisoning_reconnect_metrics() {
    let pool = ConnectionPool::new("localhost", 443, 0, AddressFamily::Auto);
    let mut presigned = PreSignedOrderPool::new(vec![]).unwrap();
    let creds = test_creds();
    let trigger = TriggerMessage {
        trigger_id: 1,
        token_id: "1234".to_string(),
        side: Side::Buy,
        price: "0.50".to_string(),
        size: "1".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: rtt_core::clock::now_ns(),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let outcome = process_one_clob(&pool, &mut presigned, &creds, &trigger, &rt);
    match outcome {
        DispatchOutcome::Rejected { record, error } => {
            assert!(matches!(error, DispatchError::PoolExhausted));
            assert!(
                !record.is_reconnect,
                "pool exhaustion should not look like a reconnect sample"
            );
        }
        DispatchOutcome::Sent { .. } => panic!("pool exhaustion should not send"),
    }
}

#[test]
fn invalid_token_dispatch_is_classified_as_order_build_failure() {
    let pool = ConnectionPool::new("localhost", 443, 0, AddressFamily::Auto);
    let signer = test_signer();
    let creds = test_creds();
    let trigger = TriggerMessage {
        trigger_id: 1,
        token_id: "not-a-token".to_string(),
        side: Side::Buy,
        price: "0.50".to_string(),
        size: "1".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: rtt_core::clock::now_ns(),
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let outcome = sign_and_dispatch(
        &pool,
        &signer,
        &trigger,
        &creds,
        signer.address(),
        signer.address(),
        0,
        false,
        rtt_core::clob_order::SignatureType::Eoa,
        "owner-uuid",
        &rt,
    );

    match outcome {
        DispatchOutcome::Rejected { record, error } => {
            assert!(matches!(error, DispatchError::BuildOrder(_)));
            assert!(
                !record.is_reconnect,
                "build failures should not be counted as reconnect samples"
            );
        }
        DispatchOutcome::Sent { .. } => panic!("invalid token should not dispatch"),
    }
}

/// TEST: `ExecutionThread::process_one()` records the round-robin connection index.
#[tokio::test]
async fn process_one_records_connection_index_after_round_robin_advance() {
    let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 2, AddressFamily::Auto);
    pool.warmup().await.expect("warmup failed");

    let dummy_req = http::Request::builder()
        .method("GET")
        .uri("/")
        .header("host", "clob.polymarket.com")
        .body(bytes::Bytes::new())
        .unwrap();
    let _ = pool.send(dummy_req).await;

    let pool = Arc::new(pool);
    let mut template =
        rtt_core::request::RequestTemplate::new(http::Method::GET, "/".parse().unwrap());
    template.add_header("host", "clob.polymarket.com");

    let mut msg = rtt_core::trigger::TriggerMessage {
        trigger_id: 1,
        token_id: "tok".to_string(),
        side: rtt_core::trigger::Side::Buy,
        price: "0.50".to_string(),
        size: "10".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: 0,
    };
    msg.timestamp_ns = rtt_core::clock::now_ns();

    let pool_clone = pool.clone();
    let rec = tokio::task::spawn_blocking(move || -> rtt_core::metrics::TimestampRecord {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        ExecutionThread::process_one(&pool_clone, &mut template, &msg, &rt)
    })
    .await
    .unwrap();

    assert_eq!(
        rec.connection_index, 1,
        "round-robin should advance to connection 1"
    );
}

/// TEST: Frame submission time stays distinct from network RTT in the execution lane.
#[tokio::test]
async fn write_duration_stays_well_below_network_ttfb() {
    let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 1, AddressFamily::Auto);
    pool.warmup().await.expect("warmup failed");
    let pool = Arc::new(pool);

    let mut template =
        rtt_core::request::RequestTemplate::new(http::Method::GET, "/".parse().unwrap());
    template.add_header("host", "clob.polymarket.com");

    let mut msg = rtt_core::trigger::TriggerMessage {
        trigger_id: 1,
        token_id: "tok".to_string(),
        side: rtt_core::trigger::Side::Buy,
        price: "0.50".to_string(),
        size: "10".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: 0,
    };
    msg.timestamp_ns = rtt_core::clock::now_ns();

    let pool_clone = pool.clone();
    let rec = tokio::task::spawn_blocking(move || -> rtt_core::metrics::TimestampRecord {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        ExecutionThread::process_one(&pool_clone, &mut template, &msg, &rt)
    })
    .await
    .unwrap();

    let write_duration = rec.write_duration();
    let warm_ttfb = rec.warm_ttfb();

    assert!(
        write_duration < 1_000_000,
        "write_duration {}ns should stay below 1ms",
        write_duration
    );
    assert!(
        warm_ttfb > 1_000_000,
        "warm_ttfb {}ns should reflect real network latency",
        warm_ttfb
    );
    assert!(
        write_duration < warm_ttfb / 10,
        "write_duration ({}) should stay far below warm_ttfb ({})",
        write_duration,
        warm_ttfb
    );
}

/// TEST: Pre-signed pool exhaustion is handled, not a crash.
///
/// Scenario: Pool has 3 orders. We fire 3 triggers. The 4th trigger
/// should get a "pool exhausted" signal, not a panic.
///
/// WHY THIS MATTERS:
/// In production, running out of pre-signed orders must degrade
/// gracefully — log a warning and stop, not crash the process.
#[test]
fn pool_exhaustion_returns_none_not_panic() {
    let payloads = test_payloads(3);
    let mut pool = PreSignedOrderPool::new(payloads).unwrap();
    let creds = test_creds();

    // Consume all 3 orders.
    for i in 0..3 {
        let req = pool.dispatch(&creds).expect("dispatch should not error");
        assert!(
            req.is_some(),
            "order {} should be available (pool has 3)",
            i
        );
    }

    // 4th dispatch: pool exhausted. Should return Ok(None), not panic.
    let req = pool
        .dispatch(&creds)
        .expect("dispatch should not error even when exhausted");
    assert!(
        req.is_none(),
        "exhausted pool should return None, not panic"
    );

    // Verify the cursor matches.
    assert_eq!(pool.consumed(), 3, "should have consumed exactly 3 orders");

    // After reset, orders should be available again.
    pool.reset_cursor();
    assert_eq!(pool.consumed(), 0, "reset should clear consumed count");
    let req = pool.dispatch(&creds).unwrap();
    assert!(
        req.is_some(),
        "after reset, orders should be available again"
    );
}

/// TEST: The threaded ingress/queue/execution pipeline still produces one warm record.
#[tokio::test]
async fn threaded_end_to_end_pipeline_produces_single_record() {
    let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 2, AddressFamily::Auto);
    pool.warmup().await.expect("warmup failed");
    let pool = Arc::new(pool);

    let q = TriggerQueue::new();
    let ingress = IngressThread::new(q.sender());

    let mut template =
        rtt_core::request::RequestTemplate::new(http::Method::GET, "/".parse().unwrap());
    template.add_header("host", "clob.polymarket.com");

    let mut exec = ExecutionThread::new(q.receiver());
    exec.start(pool.clone(), template);

    let msg = rtt_core::trigger::TriggerMessage {
        trigger_id: 1,
        token_id: "tok".to_string(),
        side: rtt_core::trigger::Side::Buy,
        price: "0.50".to_string(),
        size: "10".to_string(),
        order_type: OrderType::FOK,
        timestamp_ns: 0,
    };
    ingress.inject(msg).unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    exec.stop();
    let records = exec.get_records();
    assert_eq!(records.len(), 1, "should have exactly one execution record");

    let rec = &records[0];
    assert!(rec.t_trigger_rx > 0);
    assert!(rec.t_exec_start >= rec.t_trigger_rx);
    assert!(rec.t_write_begin >= rec.t_exec_start);
    assert!(rec.t_first_resp_byte >= rec.t_write_begin);
    assert!(!rec.cf_ray_pop.is_empty(), "POP should be extracted");
    assert!(
        rec.trigger_to_wire() < 10_000_000,
        "trigger_to_wire {}ns is unexpectedly high",
        rec.trigger_to_wire()
    );
}
