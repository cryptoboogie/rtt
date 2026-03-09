# Spec 09: rtt-core Latency Optimization

## Priority: MUST HAVE (hot-path latency and live no-regression proof)

## Recommended Order

Run this spec after [specs/10-rtt-core-cleanup.md](/Users/sam/Desktop/Projects/rtt/specs/10-rtt-core-cleanup.md).

Reason:

- Spec 10 establishes cleaner test boundaries and removes misleading/dead surfaces
- Spec 09 should then optimize the remaining real hot path with a cleaner benchmark and verification loop

If any cleanup work in Spec 10 would itself change hot-path behavior, benchmark it before and after so Spec 09 still starts from a known baseline.

## Problem

`rtt-core` has a few concrete performance problems in the outbound Polymarket path, but the current spec still mixes true latency work with broader cleanup. That makes it too easy for an implementation thread to spend time on structure without moving the actual speed metrics.

The latency-relevant issues are:

- The request-building path is duplicated across modules and still allocates/clones on the hot path.
- Connection management is optimistic: it uses the first resolved address only and has weak recovery behavior after failures.
- The order path does internal `f64` conversion before emitting Polymarket’s string-encoded base-unit fields, which risks avoidable rejects and inconsistent timing.
- Several code paths collapse unrelated failures into the same outcome, which makes latency measurements noisy and can trigger overly blunt recovery behavior.

Important scope note:

- Polymarket intentionally uses strings for many order fields on the wire, including `tokenId`, `makerAmount`, `takerAmount`, `expiration`, and `feeRateBps`.
- This spec does not propose changing that wire format.
- Any numeric refactor is only about how `rtt-core` computes those string fields internally before submission.

This spec keeps the existing execution model intact:

- Dedicated execution thread stays
- `send_start()` / `collect()` split stays
- Dynamic signing remains the default live path

The goal is to reduce CPU work, allocations, avoidable retries, and exchange-side rejects without changing the current low-latency architecture.

## Current Code

- `crates/rtt-core/src/clob_order.rs`
  - `compute_amounts()` parses `price` and `size` as `f64` and truncates.
  - `OrderJson::from_order()` uses debug formatting for addresses instead of a dedicated canonical serializer.
- `crates/rtt-core/src/clob_signer.rs`
  - `build_order()` silently maps an invalid `token_id` to `0`.
  - Public API returns `Order` directly even when input validation should be fallible.
- `crates/rtt-core/src/clob_request.rs`
  - `find_salt_position()` / `build_order_template()` expose a salt-patching path that is incompatible with signed EIP-712 payloads.
  - The module is effectively test-only today; production dispatch bypasses it.
- `crates/rtt-core/src/clob_executor.rs`
  - Pre-signed and dynamic dispatch rebuild nearly identical HTTP requests in separate code paths.
  - `PreSignedOrderPool::dispatch()` clones body bytes for each request.
  - `process_one_clob()` and `sign_and_dispatch()` collapse build, send, pool-exhaustion, and response failures into `is_reconnect = true`.
- `crates/rtt-core/src/connection.rs`
  - `connect_h2()` tries only the first resolved address.
  - Send-path failures do not immediately mark/recover the affected connection.

## Solution

### Big Task 1: Tighten the order path only where it affects latency or reject rate

#### Sub-task 1.1: Replace `f64` conversion with a small fixed-point path if it is neutral-to-faster

`compute_amounts()` currently parses `price` and `size` as `f64`, then truncates into base units.

That is not a wire-format problem. It is an internal conversion problem:

- Polymarket expects string-encoded base-unit integers for `makerAmount` and `takerAmount`
- tick sizes are bounded to 1-4 decimal places
- market-order semantics differ by side (`BUY` spend dollars, `SELL` sell shares)

Refactor only if the replacement is neutral or better in benchmarks.

Preferred implementation:

- a tiny fixed-point parser specialized to Polymarket constraints
- integer math only after parsing
- no general-purpose decimal crate unless profiling proves it is not slower

#### Sub-task 1.2: Remove silent coercions that create avoidable rejects

`build_order()` must stop turning an invalid `token_id` into `0`.

This is not a readability concern. Invalid coercion creates bad orders, exchange rejects, and misleading latency samples.

Keep the failure path cheap, but make it explicit.

### Big Task 2: Unify request encoding and dispatch assembly

#### Sub-task 2.1: Create a single request encoder for CLOB orders

Add a shared builder/encoder that owns:

- JSON body serialization
- Auth timestamp generation
- POLY_* header construction
- `Request<Bytes>` assembly

Both of these paths must use it:

- Pre-signed dispatch
- Dynamic sign-and-dispatch

#### Sub-task 2.2: Remove avoidable body clones from the pre-signed path

`PreSignedOrderPool` should store immutable body bytes in a sharable representation (`Bytes`, `Arc<[u8]>`, or equivalent) so dispatch does not clone the payload each time.

The pool should remain single-consumer for the current executor design, but the body representation should be immutable and reusable.

#### Sub-task 2.3: Delete duplicate plumbing in the live order path

Evaluate and remove or wire up:

- duplicated request construction in `clob_request.rs` vs `clob_executor.rs`
- duplicated logic between pre-signed and dynamic dispatch where the same bytes/headers are assembled twice

After the refactor there should be one obvious way to turn a signed order payload plus credentials into an HTTP request.

### Big Task 3: Tighten failure handling only where it changes latency behavior

#### Sub-task 3.1: Stop overloading `is_reconnect`

`TimestampRecord.is_reconnect` should only mean “this sample involved a reconnect/cold-path condition”.

It must not be used as a generic failure bit for:

- pool exhaustion
- serialization failure
- request build failure
- auth failure
- response parse failure

Callers need a real dispatch result classification, for example:

```rust
enum DispatchOutcome {
    Sent { record: TimestampRecord, body: Option<Vec<u8>> },
    Rejected(DispatchError),
}
```

The exact type is up to the implementation, but the point is to avoid poisoning latency metrics and reconnect behavior.

#### Sub-task 3.2: Harden `ConnectionPool`

Requirements:

- `connect_h2()` tries all resolved addresses before failing
- failed send/collect marks the connection unhealthy and triggers reconnect before reuse
- health checks report per-connection success/failure cleanly

Stretch goal if cheap:

- run health checks concurrently instead of serially

#### Sub-task 3.3: Route internal connection errors through tracing, not stderr

The background H2 task currently logs directly with `eprintln!`.

Replace that with structured logging so failures are visible in the same telemetry path as the rest of the executor.

## Files to Modify

| File | Changes |
|------|---------|
| `crates/rtt-core/src/clob_order.rs` | Replace floating-point math, add validated amount conversion |
| `crates/rtt-core/src/clob_signer.rs` | Make `build_order()` fallible, propagate typed errors |
| `crates/rtt-core/src/clob_auth.rs` | Reduce duplicate auth/header assembly in the live order path |
| `crates/rtt-core/src/clob_request.rs` | Converge on shared request encoder for the live order path |
| `crates/rtt-core/src/clob_executor.rs` | Unify dispatch assembly, stop cloning bodies, return typed dispatch outcomes |
| `crates/rtt-core/src/connection.rs` | Harden warmup/send/reconnect behavior and error handling |
| `crates/rtt-core/src/executor.rs` | Update callers to consume non-poisoning dispatch outcomes |
| `crates/rtt-core/tests/test_order_pipeline.rs` | Add exact amount math and typed-failure tests |
| `crates/rtt-core/tests/test_connection_pipeline.rs` | Keep or expand live connection checks used for latency regression proof |
| `crates/rtt-core/tests/test_execution_pipeline.rs` | Keep or expand live execution-path checks used for latency regression proof |
| `crates/rtt-core/README.md` | Correct test command expectations and live-test guidance |

## Tests

1. `compute_amounts()` handles Polymarket-style decimal cases with deterministic base-unit output
2. `compute_amounts()` rejects malformed values, unsupported precision, and overflow if the fixed-point path is adopted
3. `build_order()` returns an error for invalid token IDs instead of converting to zero
4. Shared request encoder produces the same request shape for pre-signed and dynamic paths
5. `PreSignedOrderPool` reuses immutable body bytes without cloning payload contents on dispatch
6. Dispatch failures are classified distinctly enough that reconnect samples are not polluted
7. Live connection tests still prove warm H2 connectivity and round-robin behavior
8. Live execution tests still prove timestamp ordering and split `send_start` / `collect` behavior
9. Microbench or timing harness compares old vs new amount-conversion/request-build path if those internals change

### Integration tests (live network)

1. Warm H2 connection reaches `clob.polymarket.com` and extracts `cf-ray`
2. Connection pool round-robins across warm connections
3. `send_start()` remains much faster than `collect()`
4. Live benchmark smoke test still works when explicitly requested

## Win Condition

This refactor is only “done” if all of the following are true.

### 1. Offline regression lane is green

These commands must be documented and runnable as written:

```bash
cargo test --workspace --lib
cargo test -p rtt-core --lib
```

What this proves:

- normal unit coverage still passes
- moved tests are still discoverable
- fast local iteration exists after the test split

### 2. Live no-order regression lane is green

These commands must be documented and runnable as written:

```bash
cargo test -p rtt-core --test '*'
cargo run -p pm-executor -- --validate-creds
cargo run -p rtt-bench --release -- --trigger-test --af v6
```

What this proves:

- the live H2/TLS/CLOB path still works
- auth still works without placing orders
- the benchmark harness still exercises the warm connection path

If `v6` is not stable in the current environment, the exact same commands should also be callable with `--af auto` or `--af v4`. The chosen comparison mode must be written down in the verification notes so baseline and after-change numbers are comparable.

### 3. Speed has a baseline and a no-regression threshold

Before and after the refactor, run the same benchmark command on the same machine and network:

```bash
cargo run -p rtt-bench --release -- --benchmark --mode single-shot --samples 100 --connections 2 --af v6
```

If `v6` is not the stable path in practice, use the same command with `--af auto` or `--af v4`, but do not mix address families between baseline and comparison runs.

Primary latency win condition:

- warm `trigger_to_wire` p50 does not regress
- warm `trigger_to_wire` p95 does not regress by more than 5%
- `write_duration` p50/p95 do not regress by more than 5%

Secondary CPU-path win condition:

- for any change that touches amount conversion, request building, or pre-signed dispatch, there is either
  - a benchmark showing the changed path is faster, or
  - a benchmark showing it is flat while eliminating a known reject/retry source

### 4. Real order submission path is manually proven

For changes that touch any of these modules:

- `clob_order.rs`
- `clob_signer.rs`
- `clob_auth.rs`
- `clob_request.rs`
- `clob_executor.rs`

the verification section must include both of these manual commands:

#### Reject-path live submit

```bash
./scripts/fire.sh 15618813684181907001395592606810435123428289302309615516360336906716628815319 0.10
```

Expected outcome:

- the order is signed
- the request is authenticated
- the request is sent over the warm CLOB path
- the exchange returns a real response body
- rejection is acceptable here because the token is expired or invalid

This proves transport/auth/signing/request-shape correctness better than a dry run, but it does **not** prove a valid order would be accepted.

#### Acceptance-path live submit

Run the same command shape with a known-good small valid token and price:

```bash
./scripts/fire.sh <known_good_token_id> <price>
```

Expected outcome:

- the exchange accepts the order path as it has in prior manual testing

This step is required for final sign-off whenever the refactor changes order encoding, signing, auth, or dispatch semantics. Because it can cost real money, it remains a manual gated step under explicit user approval.

### 5. Verification commands are part of the deliverable

The final implementation PR or handoff notes must contain a dedicated section named `Verification Commands` with:

- every command above
- one-line explanation of what each command proves
- which commands are offline, live-no-order, live-reject-path, and live-costs-money
- the benchmark baseline numbers and after-change numbers
- the exact address family used for the latency comparison

### Non-goals for this spec

Do not add:

- HTTP/3 implementation
- new execution strategies
- persistence changes
- changes to `pm-data` or `pm-strategy` behavior
- a wholesale rewrite of the dedicated execution-thread model
- broad cleanup work that does not move latency or live-path confidence

## Acceptance Criteria

- [ ] The Polymarket wire format remains unchanged: string fields stay strings on the wire
- [ ] Any amount-conversion refactor is benchmark-neutral or faster than the current path
- [ ] Invalid `token_id` no longer becomes `0`
- [ ] Pre-signed and dynamic dispatch share a single request-encoding path
- [ ] `PreSignedOrderPool` no longer clones full payload bodies on each dispatch
- [ ] Connection establishment retries all resolved addresses before failing
- [ ] Send/collect failures are distinguishable from reconnect/cold-path samples
- [ ] Live-network checks remain available as an explicit regression lane
- [ ] `cargo test --workspace` passes (with the real-order ignored test still left ignored)

## Scope Boundaries

- Do NOT change the 8/10 timestamp model or remove `send_start()` / `collect()` split instrumentation
- Do NOT replace the dedicated execution thread with a fully async executor model
- Do NOT turn the dynamic signing path back into fixed-price pre-signing
- Do NOT introduce generic abstractions unless they remove measurable hot-path cost or enable better retry/reconnect behavior
- Prefer small changes with measurable latency impact over broad API cleanup
