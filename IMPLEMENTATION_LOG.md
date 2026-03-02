# Implementation Log

## 1.1 — CMake project structure with directory layout and empty main
- **Files changed**: `CMakeLists.txt`, `src/CMakeLists.txt`, `src/main.cpp`, `.gitignore`, `PLAN.md`
- **Tests run**: N/A (no test framework yet)
- **Commit**: `feat: add CMake project skeleton with empty main`
- **Deviation**: None

## 1.2 — Add GoogleTest and first trivial test
- **Files changed**: `CMakeLists.txt`, `tests/CMakeLists.txt`, `tests/test_sanity.cpp`
- **Tests run**: `Sanity.TrueIsTrue` — passed
- **Commit**: `feat: add GoogleTest framework with sanity test`
- **Deviation**: None

## 1.3 — Add OpenSSL and nghttp2 dependency resolution
- **Files changed**: `cmake/Dependencies.cmake`, `tests/test_deps.cpp`, `tests/CMakeLists.txt`, `CMakeLists.txt`
- **Tests run**: `Deps.OpenSSLVersion`, `Deps.Nghttp2Version` — both passed
- **Commit**: `feat: add OpenSSL and nghttp2 dependency resolution`
- **Deviation**: nghttp2 is packaged as `libnghttp2` on Homebrew (separate from the `nghttp2` tools package). Path adjusted to `/opt/homebrew/opt/libnghttp2`.

## 2.1 — Monotonic clock wrapper with nanosecond precision
- **Files changed**: `src/clock/monotonic_clock.h`, `src/clock/monotonic_clock.cpp`, `tests/test_monotonic_clock.cpp`, `src/CMakeLists.txt`, `tests/CMakeLists.txt`
- **Tests run**: `MonotonicClock.NowReturnsNonZero`, `MonotonicClock.IsMonotonic`, `MonotonicClock.SubMillisecondResolution`, `MonotonicClock.MeasuresRealTime` — all passed
- **Commit**: `feat: add portable monotonic clock with nanosecond precision`
- **Deviation**: None. Added `rtt_lib` static library target for source modules.

## 2.2 — TimestampRecord struct and derived metric computation
- **Files**: `src/metrics/timestamp_record.h`, `tests/test_timestamp_record.cpp`
- **Tests**: 9 TimestampRecord tests — all passed
- **Commit**: `feat: add TimestampRecord with 8 checkpoints and 7 derived metrics`

## 2.3 — Percentile statistics aggregator
- **Files**: `src/metrics/stats_aggregator.h`, `src/metrics/stats_aggregator.cpp`, `tests/test_stats_aggregator.cpp`
- **Tests**: 5 StatsAggregator tests — all passed
- **Commit**: `feat: add percentile statistics aggregator with reconnect filtering`

## 3.1 — Lock-free SPSC ring buffer
- **Files**: `src/queue/spsc_queue.h`, `tests/test_spsc_queue.cpp`
- **Tests**: 6 SPSCQueue tests including concurrent producer/consumer — all passed
- **Commit**: `feat: add lock-free SPSC ring buffer with concurrent tests`

## 3.2 — Binary trigger message format
- **Files**: `src/trigger/trigger_message.h`, `tests/test_trigger_message.cpp`
- **Tests**: 5 TriggerMessage tests — all passed
- **Commit**: `feat: add fixed-size binary trigger message format`

## 3.3 — Request template with zero-allocation offset patching
- **Files**: `src/request/request_template.h`, `src/request/request_template.cpp`, `tests/test_request_template.cpp`
- **Tests**: 6 RequestTemplate tests — all passed
- **Commit**: `feat: add zero-allocation request template with offset patching`

## 4.1 — Raw TCP connection to target host
- **Files**: `src/connection/tcp_connector.h`, `src/connection/tcp_connector.cpp`, `tests/test_tcp_connector.cpp`
- **Tests**: 6 TcpConnector tests (incl. integration) — all passed
- **Commit**: `feat: add TCP connector with DNS resolution and address family selection`

## 4.2 — TLS session over TCP with ALPN for HTTP/2
- **Files**: `src/connection/tls_session.h`, `src/connection/tls_session.cpp`, `tests/test_tls_session.cpp`
- **Tests**: 3 TlsSession tests (integration) — all passed. ALPN h2 confirmed.
- **Commit**: `feat: add TLS session with ALPN h2 negotiation`

## 4.3 — HTTP/2 session via nghttp2
- **Files**: `src/connection/h2_session.h`, `src/connection/h2_session.cpp`, `tests/test_h2_session.cpp`
- **Tests**: 4 H2Session tests (integration) — all passed. cf-ray captured. Session reuse confirmed.
- **Commit**: `feat: add HTTP/2 session via nghttp2 with request/response support`

## 4.4 — Connection pool with warmup, health check, and reconnect
- **Files**: `src/connection/connection_pool.h`, `src/connection/connection_pool.cpp`, `tests/test_connection_pool.cpp`
- **Tests**: 5 ConnectionPool tests (integration) — all passed. 2 warm connections, round-robin, reconnect.
- **Commit**: `feat: add connection pool with 2 warm H2 connections and auto-reconnect`

## 5.1 — Ingress thread (trigger receiver)
- **Files**: `src/executor/ingress_thread.h`, `src/executor/ingress_thread.cpp`, `tests/test_ingress_thread.cpp`
- **Tests**: 4 IngressThread tests — all passed. Timestamp set on inject, queue delivery confirmed.
- **Commit**: `feat: add ingress thread with trigger receive and queue dispatch`

## 5.2 — Execution thread (hot-path request dispatch)
- **Files**: `src/executor/execution_thread.h`, `src/executor/execution_thread.cpp`, `tests/test_execution_thread.cpp`
- **Tests**: 4 ExecutionThread tests (integration) — all passed. Full 8-checkpoint timestamps, cf-ray POP extraction, threaded processing.
- **Commit**: `feat: add execution thread with full hot-path timestamp capture`

## 5.3 — Maintenance thread (keepalive, reconnect, POP verification)
- **Files**: `src/executor/maintenance_thread.h`, `src/executor/maintenance_thread.cpp`, `tests/test_maintenance_thread.cpp`
- **Tests**: 3 MaintenanceThread tests (integration) — all passed. Health check, reconnect, periodic operation.
- **Commit**: `feat: add maintenance thread with health check and POP verification`

## 5.4 — Integrated pipeline smoke test with CPU pinning
- **Files**: `src/executor/cpu_pin.h`, `src/executor/cpu_pin.cpp`, `src/main.cpp`, `tests/test_integration_pipeline.cpp`, `src/CMakeLists.txt`, `tests/CMakeLists.txt`
- **Tests**: 3 tests — EndToEndTriggerToResponse (cf-ray POP extracted), TimestampRecordComplete (all timestamps populated, trigger_to_wire < 10ms), CpuPin.PinToCore (macOS returns false). 70 total tests pass.
- **Commit**: `feat: integrate full pipeline with CPU pinning and smoke test`
- **Deviation**: None

## 6.1 — Benchmark CLI entry point and trigger injection modes
- **Files**: `src/benchmark/benchmark_runner.h`, `src/benchmark/benchmark_runner.cpp`, `src/main.cpp`, `tests/test_benchmark_runner.cpp`, `src/CMakeLists.txt`, `tests/CMakeLists.txt`
- **Tests**: 5 BenchmarkRunner tests — all passed. SingleShot, RandomCadence, BurstRace modes. 75 total tests.
- **Commit**: `feat: add benchmark CLI with three trigger injection modes`
- **Deviation**: None

## 6.2 — Full pipeline timestamp capture and percentile reporting
- **Files**: `tests/test_benchmark_stats.cpp`, `tests/CMakeLists.txt`
- **Tests**: 4 BenchmarkStats tests — all passed. All 8 timestamps populated, derived metrics monotonic, percentiles computed correctly across all modes. 79 total tests.
- **Commit**: `feat: verify full pipeline timestamp capture and percentile reporting`
- **Deviation**: None

## 6.3 — cf-ray POP extraction and warm/cold sample separation
- **Files**: `tests/test_benchmark_pop.cpp`, `tests/CMakeLists.txt`
- **Tests**: 4 BenchmarkPOP tests — all passed. POP extracted in every warm record, distribution tracked, warm/cold correctly separated. 83 total tests.
- **Commit**: `feat: verify cf-ray POP extraction and warm/cold sample separation`
- **Deviation**: None

## 7.1 — IPv4 vs IPv6 forced path selection
- **Files**: `src/benchmark/benchmark_runner.h`, `src/benchmark/benchmark_runner.cpp`, `src/main.cpp`, `tests/test_address_family.cpp`, `tests/CMakeLists.txt`
- **Tests**: 4 AddressFamily tests — all passed. Both IPv4 and IPv6 resolve, connect, and complete full benchmark pipeline. 87 total tests.
- **Commit**: `feat: add IPv4 vs IPv6 forced path selection for benchmarks`
- **Deviation**: None

## 7.2 — Dual-connection benchmark comparison
- **Files**: `tests/test_dual_connection.cpp`, `tests/CMakeLists.txt`
- **Tests**: 3 DualConnection tests — all passed. Single vs dual connection benchmarks with burst mode contention test. 90 total tests.
- **Commit**: `feat: add dual-connection benchmark comparison tests`
- **Deviation**: None

## 7.3 — HTTP/3 experiment path
- **Files**: `src/connection/h3_stub.h`, `src/connection/h3_stub.cpp`, `src/CMakeLists.txt`, `tests/test_h3_stub.cpp`, `tests/CMakeLists.txt`
- **Tests**: 2 H3Stub tests — all passed. Status correctly reports NotImplemented. Probe confirms endpoint advertises h3 via alt-svc. 92 total tests.
- **Commit**: `feat: add HTTP/3 experiment stub with alt-svc probe`
- **Deviation**: H3 is a stub/probe only; full QUIC client deferred pending library integration.

---

# Rust Port Implementation Log (Session 1)

## R1.1 — Cargo workspace with rtt-core and rtt-bench crates
- **Files changed**: `Cargo.toml`, `crates/rtt-core/Cargo.toml`, `crates/rtt-core/src/lib.rs`, `crates/rtt-bench/Cargo.toml`, `crates/rtt-bench/src/main.rs`
- **Tests run**: `sanity` — 1 passed
- **Commit**: `feat: add Cargo workspace with rtt-core and rtt-bench crates`
- **Deviation**: None. Dependencies: hyper 1.x, tokio-rustls, crossbeam-channel, clap, core_affinity.

## R2.1 — Monotonic nanosecond clock wrapper
- **Files changed**: `crates/rtt-core/src/clock.rs`
- **Tests run**: 4 clock tests — all passed (now_returns_value, is_monotonic, sub_millisecond_resolution, measures_real_time)
- **Commit**: `feat: add Rust monotonic clock with nanosecond precision`
- **Deviation**: Uses `std::time::Instant` with OnceLock epoch instead of platform-specific APIs.

## R2.2 — TimestampRecord with 8 checkpoints and 7 derived metrics
- **Files changed**: `crates/rtt-core/src/metrics.rs`
- **Tests run**: 9 TimestampRecord tests — all passed
- **Commit**: `feat: add TimestampRecord with 8 checkpoints and 7 derived metrics`
- **Deviation**: None.

## R2.3 — Percentile stats aggregator with reconnect filtering
- **Files changed**: `crates/rtt-core/src/metrics.rs` (same file as R2.2)
- **Tests run**: 5 StatsAggregator tests — all passed. 14 total metrics tests.
- **Commit**: `feat: add percentile stats aggregator with reconnect filtering`
- **Deviation**: Combined with R2.2 in single file for cohesion.

## R3.1 — Shared types (TriggerMessage, Side, OrderType, OrderBookSnapshot, PriceLevel)
- **Files changed**: `crates/rtt-core/src/trigger.rs`
- **Tests run**: 4 trigger tests — all passed (construction, serialization, snapshot, enum variants)
- **Commit**: `feat: add shared types matching interface contracts`
- **Deviation**: None. All types derive Serialize/Deserialize for cross-session compatibility.

## R3.2 — SPSC channel wrapper (crossbeam-channel)
- **Files changed**: `crates/rtt-core/src/queue.rs`
- **Tests run**: 5 queue tests — all passed (push/pop, empty, FIFO, capacity, concurrent)
- **Commit**: `feat: add SPSC trigger queue via crossbeam-channel`
- **Deviation**: Uses crossbeam bounded(1024) instead of custom ring buffer. Same semantics.

## R3.3 — Request template with zero-allocation body patching
- **Files changed**: `crates/rtt-core/src/request.rs`
- **Tests run**: 6 request tests — all passed (create, set_body, register/patch, multiple patches, headers, build)
- **Commit**: `feat: add request template with zero-allocation body patching`
- **Deviation**: Uses fixed [u8; 4096] body array with patch slots matching C++ design.

## R4.1 — HTTP/2 connection with rustls + hyper
- **Files changed**: `crates/rtt-core/src/connection.rs`, `crates/rtt-core/Cargo.toml`
- **Tests run**: 9 connection tests — all passed (cf-ray extraction, DNS resolution, H2 connect, session reuse, pool warmup, health check)
- **Commit**: `feat: add HTTP/2 connection stack with rustls ALPN h2`
- **Deviation**: Switched from native-tls to rustls for proper ALPN h2 support on macOS. Combines TCP, TLS, H2 session, connection pool, cf-ray extraction, and address family selection in single module.

## R5.1–5.5 — Executor pipeline (ingress, execution, maintenance, CPU pin, integration)
- **Files changed**: `crates/rtt-core/src/executor.rs`
- **Tests run**: 6 executor tests — all passed (ingress timestamp, queue delivery, process_one timestamps, CPU pin, maintenance thread, end-to-end pipeline)
- **Commit**: `feat: add full executor pipeline with ingress/execution/maintenance threads`
- **Deviation**: Combined all executor components in single module. Execution thread uses tokio current_thread runtime for async H2 I/O from sync thread.

## R6.1–6.3 — Benchmark harness with three modes and percentile reporting
- **Files changed**: `crates/rtt-core/src/benchmark.rs`, `crates/rtt-bench/src/main.rs`, `crates/rtt-core/src/lib.rs`
- **Tests run**: 8 benchmark tests — all passed (format_ns, config default, single-shot, random-cadence, burst-race, timestamps populated, POP extracted, warm/cold separation)
- **Commit**: `feat: add benchmark harness with CLI and three injection modes`
- **Deviation**: None. CLI matches C++ version flags.

## R7.1–7.2 — Protocol experiments (IPv4/IPv6, dual-connection)
- **Files changed**: `crates/rtt-core/src/benchmark.rs` (added test cases)
- **Tests run**: 4 protocol experiment tests — all passed (IPv4 forced path, IPv6 forced path, dual-connection comparison, burst contention)
- **Commit**: `feat: add IPv4/IPv6 and dual-connection benchmark experiments`
- **Deviation**: None. IPv6 test allows graceful failure if not available.

## R7.3 — HTTP/3 stub with alt-svc probe
- **Files changed**: `crates/rtt-core/src/h3_stub.rs`, `crates/rtt-core/src/lib.rs`
- **Tests run**: 2 H3 stub tests — all passed (status NotImplemented, alt-svc probe detects h3)
- **Commit**: `feat: add HTTP/3 stub with alt-svc probe`
- **Deviation**: Stub only, full QUIC deferred. Cloudflare advertises h3 via alt-svc.

---

**Total Rust tests: 63 passed, 0 failed**

**Modules completed:**
| Module | Tests | Description |
|--------|-------|-------------|
| clock | 4 | Monotonic nanosecond timestamps |
| metrics | 14 | TimestampRecord + StatsAggregator |
| trigger | 4 | Shared types (TriggerMessage, OrderBookSnapshot) |
| queue | 5 | SPSC channel (crossbeam) |
| request | 6 | Zero-allocation request template |
| connection | 9 | HTTP/2 + TLS + pool + cf-ray + address family |
| executor | 6 | Ingress + execution + maintenance + CPU pin |
| benchmark | 12 | 3 modes + percentiles + protocol experiments |
| h3_stub | 2 | HTTP/3 status + alt-svc probe |
| sanity | 1 | Workspace verification |
| **TOTAL** | **63** | |

---

## Optimization Notes (Rust vs C++ latency gap)

Current Rust trigger-to-wire: ~80us p50 (debug build). C++ baseline: ~8us p50.
The gap comes from architectural differences, not algorithmic ones. Ordered by expected impact:

### High impact
1. **Release build** — debug build has no inlining, bounds checks everywhere, unoptimized codegen. Expect 5–10x improvement from `--release` alone.
2. **Async bridge overhead** — The execution thread creates a `tokio::runtime::Runtime` per thread and uses `block_on()` to drive hyper's async H2 client from a sync context. This adds task scheduling, waker allocation, and event loop overhead on every request. Fix: use the `h2` crate directly in synchronous mode, or keep a persistent runtime and use `tokio::runtime::Handle::block_on()` instead of rebuilding.
3. **hyper framing cost** — hyper builds HTTP/2 frames through multiple abstraction layers (http-body-util, h2 crate, internal buffers). The C++ version writes directly to nghttp2's buffer with zero intermediate copies. Fix: use the `h2` crate directly, bypassing hyper's client abstraction, to get closer to raw frame submission.

### Medium impact
4. **SPSC queue** — `crossbeam-channel::bounded` is an MPMC channel with more synchronization overhead than needed. The C++ version uses a custom SPSC ring buffer with cache-line-padded atomics and acquire/release ordering only. Fix: replace with a true SPSC ring buffer (e.g. `rtrb` crate or custom implementation with `std::sync::atomic`).
5. **Request template build** — `template.build_request()` allocates a new `Bytes` and `http::Request` on every call. The C++ version patches an existing nghttp2 header array in-place. Fix: pre-allocate the `http::Request` and only mutate the body bytes, or use the `h2` crate's `SendRequest::send_request` directly with pre-built header maps.
6. **TLS implementation** — rustls is pure Rust (no asm-optimized crypto). The C++ version uses OpenSSL with hardware-accelerated AES-NI. Fix: enable `aws-lc-rs` backend for rustls (already a transitive dep) which uses assembly-optimized crypto.

### Low impact
7. **Clock** — `std::time::Instant` goes through `clock_gettime(CLOCK_MONOTONIC)` on Linux. The C++ version uses `CLOCK_MONOTONIC_RAW` which avoids NTP adjustments. Difference is negligible for relative measurements.
8. **String allocations in TriggerMessage** — `token_id`, `price`, `size` are heap-allocated `String`. The C++ version uses a fixed 64-byte `char[]` payload. Fix: use `ArrayString` or `[u8; N]` for hot-path fields.

### Not worth optimizing
- Connection pool mutex — only touched once per request, dwarfed by network I/O.
- DNS resolution — happens once at warmup, not on hot path.
- cf-ray parsing — happens after response, not on critical path.
