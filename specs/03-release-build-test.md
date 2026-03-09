# Spec 03: Release Build Test

## Priority: MUST HAVE (blocking production)

## Problem

The system has never been compiled or tested with `--release`. Debug builds have no inlining, bounds checks everywhere, and unoptimized codegen. Current trigger-to-wire is ~80us in debug; expected ~10-20us in release. We need to:

1. Verify all tests pass in release mode
2. Measure release-mode latency
3. Ensure `fire.sh` works in release mode
4. Document the results

## Current Code

- `scripts/fire.sh` — Runs `cargo test` (debug mode) for the e2e order test
- `crates/rtt-core/src/clob_executor.rs` — `test_hot_path_latency` test asserts dispatch < 100us (debug threshold)
- No `--release` flag used anywhere in the project

## Solution

### 1. Run full test suite in release mode

```bash
cargo test --workspace --release
```

Fix any tests that fail due to timing assumptions (e.g., `test_hot_path_latency` may need a tighter threshold in release, or tests that rely on debug-mode slowness for ordering).

### 2. Update `fire.sh` to support release mode

Add `--release` flag to the `cargo test` command in `fire.sh`. Either always use release, or accept a `--debug` flag to opt into debug mode.

### 3. Benchmark release latency

Run `rtt-bench` in release mode:
```bash
cargo run --release -p rtt-bench -- --benchmark --samples 100 --mode single-shot
```

Record: trigger-to-wire p50/p95/p99, write_duration, warm_ttfb. Compare against debug numbers.

### 4. Add Cargo.toml release profile (if not present)

```toml
[profile.release]
opt-level = 3
lto = "thin"       # Link-time optimization for cross-crate inlining
codegen-units = 1  # Better optimization at cost of compile time
```

### 5. Document results

Add a section to `IMPLEMENTATION_LOG.md` recording:
- All test results (pass/fail counts)
- Latency numbers (debug vs release)
- Any tests that needed adjustment
- Any compilation issues

## Files to Modify

| File | Changes |
|------|---------|
| `Cargo.toml` (workspace root) | Add `[profile.release]` section if not present |
| `scripts/fire.sh` | Add `--release` to cargo test command |
| `crates/rtt-core/src/clob_executor.rs` | Adjust `test_hot_path_latency` threshold if needed |
| `IMPLEMENTATION_LOG.md` | Document release build results |

## Steps (Manual + Automated)

This spec is partially manual (running commands, observing output) and partially automated (fixing code).

1. `cargo test --workspace --release` — Run and record results
2. Fix any failing tests
3. `cargo run --release -p rtt-bench -- --benchmark --samples 100` — Benchmark
4. Update `fire.sh`
5. Add release profile to Cargo.toml
6. Log results in IMPLEMENTATION_LOG.md

## Acceptance Criteria

- [ ] `cargo test --workspace --release` — all tests pass (except ignored ones)
- [ ] `fire.sh` uses `--release` by default
- [ ] Release latency numbers documented in IMPLEMENTATION_LOG.md
- [ ] Release profile configured in workspace Cargo.toml

## Scope Boundaries

- Do NOT enable `aws-lc-rs` backend for rustls (separate optimization)
- Do NOT change any algorithm or architecture — this is purely a build/test task
- Do NOT run the e2e order test (`test_clob_end_to_end_pipeline`) — that costs money
