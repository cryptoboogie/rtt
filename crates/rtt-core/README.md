# rtt-core

Low-latency execution engine: warm HTTP/2 connection pool, CLOB order signing, request assembly, and nanosecond timestamp instrumentation.

Hot-path or benchmark-sensitive changes should follow [Spec 09](/Users/sam/Desktop/Projects/rtt/specs/09-rtt-core-refactor.md). The commands below keep offline, live-no-order, and live-costs-money verification lanes explicit.

## Verification Commands

### Offline unit tests

```bash
cargo test -p rtt-core --lib
```

Proves the library lane stays fully offline.

### Live integration tests (no orders placed)

```bash
cargo test -p rtt-core --test '*'
```

Proves the DNS/TLS/H2/execution/benchmark integration lane still works against `clob.polymarket.com`.

### Credential validation (live, no orders placed)

```bash
cargo run -p pm-executor -- --validate-creds
```

Proves the auth path works without placing an order.

### Benchmark smoke test (live, no orders placed)

```bash
cargo run -p rtt-bench --release -- --trigger-test --af auto
```

Exercises the warmed trigger path without changing order semantics.

### Benchmark comparison command (Spec 09)

```bash
cargo run -p rtt-bench --release -- --benchmark --mode single-shot --samples 100 --connections 2 --af auto
```

Use the same address family before and after a refactor. Swap `--af auto` for `--af v4` or `--af v6` only if that is the stable path in the current environment.

### Reject-path live submit

```bash
./scripts/fire.sh 15618813684181907001395592606810435123428289302309615516360336906716628815319 0.10
```

Proves live signing, authentication, request shape, and transport against a token that should reject.

### Acceptance-path live submit (costs money)

```bash
./scripts/fire.sh <known_good_token_id> <price>
```

Manual acceptance proof for changes that touch order encoding, signing, auth, or dispatch semantics.

### Ignored real-order test (costs money)

```bash
cargo test -p rtt-core -- --ignored test_clob_end_to_end_pipeline
```

Runs the ignored end-to-end live-order test. It requires `POLY_API_KEY`, `POLY_SECRET`, `POLY_PASSPHRASE`, `POLY_ADDRESS`, and `POLY_PRIVATE_KEY`, plus explicit operator approval.
