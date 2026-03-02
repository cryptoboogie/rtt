# Implementation Plan: Low-Latency C++ Hot Executor

## 1. Assumptions

- **Source of truth**: AGENTS.md exclusively. No invented requirements.
- **Language**: C++ (C++20) for the hot executor.
- **Target endpoint**: `https://clob.polymarket.com` (Cloudflare-fronted, EWR POP).
- **Production OS**: Linux. **Development OS**: macOS (darwin).
- **Dependencies**: OpenSSL, nghttp2, GoogleTest.
- **HTTP/2 first**. HTTP/3 post-baseline. Rust post-baseline.
- **Auth**: placeholder patch slot (mechanism unspecified).
- **Trigger source**: synthetic in-process for benchmarks.

## 2. Milestones

M1: Project Skeleton & Build System
M2: Latency Instrumentation Core
M3: Hot-Path Data Structures
M4: Connection Manager
M5: Execution Pipeline Integration
M6: Benchmark Harness & First Measurements
M7: Transport Optimization & Protocol Experiments

## 3. Execution Order

1.1 -> 1.2 -> 1.3 -> 2.1 -> 2.2 -> 2.3 -> 3.1 -> 3.2 -> 3.3 -> 4.1 -> 4.2 -> 4.3 -> 4.4 -> 5.1 -> 5.2 -> 5.3 -> 5.4 -> 6.1 -> 6.2 -> 6.3 -> 7.1 -> 7.2 -> 7.3

## 4. Earliest Meaningful Benchmarks

- 5.4: First end-to-end smoke test with timestamps
- 6.2: First real benchmark with percentile reporting
- 6.3: First benchmark with POP verification

See full plan details in conversation history.
