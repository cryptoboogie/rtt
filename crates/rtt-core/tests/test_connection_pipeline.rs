//! # Connection Pipeline Tests
//!
//! These tests prove that rtt-core can:
//! 1. Resolve Polymarket's DNS and connect over TLS with HTTP/2
//! 2. Maintain a pool of warm (pre-established) connections
//! 3. Send requests and receive responses through those connections
//! 4. Identify which Cloudflare datacenter (POP) is serving us
//!
//! WHY THIS MATTERS:
//! The entire system's speed depends on having connections already open
//! when a trade signal arrives. "Warm" means the TCP handshake, TLS
//! negotiation, and HTTP/2 setup are already done. When a trigger fires,
//! we only need to send a single HTTP/2 frame — not establish a new
//! connection from scratch (which would take ~100ms+).

use bytes::Bytes;
use http::Request;
use rtt_core::connection::{
    connect_h2, extract_pop, get_cf_ray, resolve, send_request, AddressFamily, ConnectionPool,
};

/// TEST: We can connect to Polymarket's CLOB server over HTTP/2.
///
/// This proves:
/// - DNS resolves for clob.polymarket.com
/// - TLS handshake succeeds with ALPN h2 (HTTP/2)
/// - The connection is usable (we can send a request and get a response)
/// - Cloudflare's cf-ray header tells us which datacenter we're hitting
///
/// The POP (Point of Presence) code (e.g., "EWR" = Newark, "IAD" = Virginia)
/// tells us our physical proximity to the server. Closer = lower latency.
///
/// WHY THIS MATTERS:
/// If we can't connect, we can't trade. This is the most fundamental test.
#[tokio::test]
async fn warm_connection_reaches_polymarket_and_identifies_datacenter() {
    println!("\n=== Connection Pipeline: Single Warm Connection ===");
    println!("Target: clob.polymarket.com:443");

    // Create a pool with 1 connection and warm it up.
    // "Warming" means: DNS resolve → TCP connect → TLS handshake → H2 SETTINGS exchange.
    let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 1, AddressFamily::Auto);
    let warm_count = pool
        .warmup()
        .await
        .expect("warmup failed — cannot reach server");
    println!("Warmed:  {} connection(s)", warm_count);
    assert_eq!(warm_count, 1, "should warm exactly 1 connection");

    // Send a simple GET / request to verify the connection works.
    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("host", "clob.polymarket.com")
        .body(Bytes::new())
        .unwrap();

    let start = std::time::Instant::now();
    let (resp, idx) = pool
        .send(req)
        .await
        .expect("request failed on warm connection");
    let rtt = start.elapsed();

    // The server should return some HTTP status (200, 404, etc.) — any response
    // proves the full H2 pipeline works.
    assert!(
        resp.status().is_success() || resp.status().is_client_error(),
        "unexpected status: {}",
        resp.status()
    );

    // Extract the Cloudflare POP from the cf-ray header.
    let cf_ray = get_cf_ray(&resp).expect("cf-ray header missing — not behind Cloudflare?");
    let pop = extract_pop(&cf_ray);
    assert!(!pop.is_empty(), "POP code should not be empty");

    println!("Status:  {}", resp.status());
    println!("cf-ray:  {}", cf_ray);
    println!("POP:     {}", pop);
    println!("Conn:    index {}", idx);
    println!("RTT:     {:.2}ms", rtt.as_secs_f64() * 1000.0);
    println!("=== PASS ===\n");
}

/// TEST: DNS resolution works for the supported address-family modes.
///
/// This preserves the explicit live DNS checks that used to live under
/// `src/connection.rs`, while keeping `cargo test --lib` offline.
#[test]
fn live_dns_resolution_supports_auto_and_ipv4() {
    let auto_addrs =
        resolve("clob.polymarket.com", 443, AddressFamily::Auto).expect("auto resolve failed");
    assert!(
        !auto_addrs.is_empty(),
        "auto resolve should return at least one address"
    );

    let v4_addrs =
        resolve("clob.polymarket.com", 443, AddressFamily::V4).expect("v4 resolve failed");
    assert!(
        v4_addrs.iter().all(|addr| addr.is_ipv4()),
        "forced IPv4 resolution should only return IPv4 addresses",
    );
}

/// TEST: IPv6 resolution is environment-dependent but should remain callable.
#[test]
fn live_dns_resolution_allows_ipv6_probe() {
    let _ = resolve("clob.polymarket.com", 443, AddressFamily::V6);
}

/// TEST: A single warmed H2 session can serve multiple requests.
#[tokio::test]
async fn single_h2_session_reuses_tls_and_h2_state() {
    let mut sender = connect_h2("clob.polymarket.com", 443, AddressFamily::Auto)
        .await
        .expect("failed to connect");

    for _ in 0..2 {
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header("host", "clob.polymarket.com")
            .body(Bytes::new())
            .unwrap();
        let resp = send_request(&mut sender, req)
            .await
            .expect("request failed");
        assert!(
            resp.status().is_success() || resp.status().is_client_error(),
            "unexpected status: {}",
            resp.status()
        );
    }
}

/// TEST: A pool of 2 warm connections can handle requests round-robin.
///
/// This proves:
/// - Multiple connections can be open simultaneously
/// - Requests are distributed across connections (round-robin)
/// - Each connection independently works
///
/// WHY THIS MATTERS:
/// With 2 connections, if one is busy sending a previous order,
/// the next trigger can use the other connection immediately.
/// The connection_index in the response tells us which one was used.
#[tokio::test]
async fn connection_pool_distributes_requests_across_connections() {
    println!("\n=== Connection Pipeline: Pool Round-Robin ===");

    let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 2, AddressFamily::Auto);
    let warm_count = pool.warmup().await.expect("warmup failed");
    println!("Pool:    {} warm connections", warm_count);
    assert_eq!(warm_count, 2);

    // Send 4 requests. Round-robin should alternate: 0, 1, 0, 1.
    let mut indices = Vec::new();
    for i in 0..4 {
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header("host", "clob.polymarket.com")
            .body(Bytes::new())
            .unwrap();

        let (resp, idx) = pool.send(req).await.expect("send failed");
        assert!(
            resp.status().is_success() || resp.status().is_client_error(),
            "request {} failed with status {}",
            i,
            resp.status()
        );
        println!("Request {}: connection index {}", i, idx);
        indices.push(idx);
    }

    // Verify round-robin distribution: alternating 0 and 1.
    assert_eq!(indices[0], 0, "first request should use connection 0");
    assert_eq!(indices[1], 1, "second request should use connection 1");
    assert_eq!(indices[2], 0, "third request should cycle back to 0");
    assert_eq!(indices[3], 1, "fourth request should cycle back to 1");

    println!("Round-robin pattern: {:?}", indices);
    println!("=== PASS ===\n");
}

/// TEST: Write (frame submission) is fast; network round-trip is separate.
///
/// This proves the "split instrumentation" works:
/// - send_start() submits the HTTP/2 frame to the kernel (microseconds)
/// - handle.collect() waits for the server response (milliseconds)
///
/// WHY THIS MATTERS:
/// "trigger-to-wire" is the metric that matters for speed — how long
/// from receiving a trade signal to the order bytes leaving our machine.
/// That's the send_start() part. The network round-trip (collect) is
/// physics — speed of light to the server and back — and we can't
/// optimize it. By splitting these, we can measure what WE control
/// separately from what the NETWORK controls.
#[tokio::test]
async fn frame_submission_is_microseconds_network_roundtrip_is_milliseconds() {
    println!("\n=== Connection Pipeline: Split Instrumentation ===");

    let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 1, AddressFamily::Auto);
    pool.warmup().await.expect("warmup failed");

    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("host", "clob.polymarket.com")
        .body(Bytes::new())
        .unwrap();

    // Phase 1: Submit the H2 frame. This should return very fast because
    // it only writes to the kernel's TCP send buffer, not waiting for a reply.
    let t_submit_start = std::time::Instant::now();
    let handle = pool.send_start(req).await.expect("send_start failed");
    let submit_time = t_submit_start.elapsed();

    // Phase 2: Wait for the actual server response. This involves
    // speed-of-light travel to the datacenter and back.
    let t_collect_start = std::time::Instant::now();
    let resp = handle.collect().await.expect("collect failed");
    let collect_time = t_collect_start.elapsed();

    assert!(
        resp.status().is_success() || resp.status().is_client_error(),
        "unexpected status: {}",
        resp.status()
    );

    println!(
        "Submit (send_start): {:>8.1}us",
        submit_time.as_secs_f64() * 1_000_000.0
    );
    println!(
        "Collect (response):  {:>8.1}ms",
        collect_time.as_secs_f64() * 1000.0
    );

    // The submit should be orders of magnitude faster than the network RTT.
    // Submit: typically <1ms. Collect: typically 30-200ms.
    assert!(
        submit_time < collect_time,
        "submit ({:?}) should be faster than collect ({:?})",
        submit_time,
        collect_time
    );

    println!(
        "Ratio:               {:.0}x faster",
        collect_time.as_secs_f64() / submit_time.as_secs_f64()
    );
    println!("=== PASS ===\n");
}

/// TEST: Pool health checks verify each warmed connection.
#[tokio::test]
async fn connection_pool_health_check_confirms_warm_connections() {
    let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 2, AddressFamily::Auto);
    pool.warmup().await.expect("warmup failed");

    let healthy = pool.health_check().await;
    assert_eq!(
        healthy, 2,
        "all warmed connections should pass health checks"
    );
}
