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
