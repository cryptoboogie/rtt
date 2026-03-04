# Low-Latency Polymarket Execution Service

## Architecture Overview

A low-latency trading pipeline for Polymarket's CLOB (Central Limit Order Book). The system monitors real-time order book data via WebSocket, runs configurable strategies that produce trade signals, and dispatches pre-signed orders over warm HTTP/2 connections with microsecond-level instrumentation.

### End-to-End Flow

```
[Polymarket WS] → pm-data::Pipeline → broadcast<OrderBookSnapshot>
    → bridge → mpsc<OrderBookSnapshot>
    → pm-strategy::StrategyRunner → mpsc<TriggerMessage>
    → bridge (stamps timestamp_ns) → crossbeam<TriggerMessage>
    → execution::run_execution_loop (dedicated OS thread)
        ├─ dry_run=true:  logs "[DRY RUN] Would fire order"
        └─ dry_run=false: process_one_clob() → warm H2 → Polymarket CLOB API
```

### Crates

**`rtt-core`** — Core execution engine and shared types.
- Shared types: `TriggerMessage`, `OrderBookSnapshot`, `Side`, `OrderType`, `TimestampRecord`
- Warm HTTP/2 connection pool with split instrumentation (`send_start` dispatches frame in µs, `collect` awaits response)
- CLOB order pipeline: EIP-712 signing (`clob_signer`), HMAC L2 auth (`clob_auth`), pre-signed order pool (`clob_executor`)
- `PreSignedOrderPool`: orders signed at startup with unique salts; at dispatch time only HMAC headers are recomputed (body untouched)
- `process_one_clob()`: hot-path function — dispatch pre-signed order on warm connection, returns `TimestampRecord` with 8 timestamp checkpoints
- Monotonic nanosecond clock, crossbeam trigger queue, request templating, benchmark harness

**`pm-data`** — Real-time market data from Polymarket WebSocket.
- `WsClient`: connects to `wss://ws-subscriptions-clob.polymarket.com/ws/market`, subscribes to asset channels
- `OrderBookManager`: maintains local order book state (snapshots + deltas), thread-safe concurrent reads
- `Pipeline`: orchestrates WS → parse → order book update → broadcast snapshots to subscribers

**`pm-strategy`** — Trading strategy framework.
- `Strategy` trait: `on_book_update(&OrderBookSnapshot) → Option<TriggerMessage>`
- `ThresholdStrategy`: fires when ask ≤ threshold (buy) or bid ≥ threshold (sell)
- `SpreadStrategy`: fires when bid-ask spread narrows below max_spread
- `StrategyRunner`: async loop consuming snapshots, forwarding triggers
- `BacktestRunner`: replay historical snapshots through any strategy
- Config-driven: TOML → `StrategyConfig` → `Box<dyn Strategy>`

**`pm-executor`** — Pipeline orchestrator binary (the main entrypoint).
- Loads `config.toml`, builds all components, wires channels, manages lifecycle
- Channel bridges: `broadcast→mpsc` (snapshot), `mpsc→crossbeam` (trigger with timestamp stamp)
- Execution loop runs on dedicated OS thread (not tokio) — matches the latency-sensitive pattern from rtt-core
- `dry_run` mode (default=true): logs triggers without sending orders
- Live mode: warms connection pool, pre-signs orders at startup, dispatches via `process_one_clob()`
- Graceful Ctrl+C shutdown of all components

**`rtt-bench`** — CLI benchmark tool for connection latency profiling.

### Key Config (`config.toml`)

- `[credentials]` — Polymarket API creds (or use `POLY_*` env vars)
- `[execution]` — `dry_run = true` (SAFETY: set false only for real orders), `presign_count`, `is_neg_risk`, `fee_rate_bps`
- `[strategy]` — strategy name, token_id, side, size, order_type, params (threshold, max_spread)
- `[connection]` — pool_size, address_family
- `[websocket]` — asset_ids to monitor

## Way of Working
1. **Break plans into big tasks** — each big task represents a meaningful capability milestone
2. **Break tasks into sub-tasks** — sub-tasks are atomic units of work
3. **TDD for every sub-task** — write a failing test first, then write the minimal code to pass the test(s)
4. **Once the test passes, move on** — do not gold-plate; proceed to the next sub-task immediately
5. **Do not stop until all sub-tasks are finished** — unless there is a fatal blocking issue
6. **Log every sub-task** — for each completed sub-task, append an entry to `IMPLEMENTATION_LOG.md` recording files changed, tests run, commit message, any deviations from the plan, and any notes that can be relevant for later.
7. **When finished, run and verify ALL project test suites pass (unit and integration)**

NOTE: The `IMPLEMENTATION_LOG.md` is a great source of history for the work that's been done in this repo. Use it as a reference point.

## Running Tests

```bash
cargo test --workspace              # All tests (196 pass, 1 ignored)
cargo test --workspace --lib        # Unit tests only (no network)
cargo test --workspace --test '*'   # Integration tests only (some need network)
cargo test -p <crate>               # Single crate: rtt-core, pm-data, pm-strategy, pm-executor
cargo test --workspace -- --ignored # Real orders — costs money, needs POLY_* env vars
```

Only `test_clob_end_to_end_pipeline` in rtt-core is ignored. It sends a real order and requires `POLY_API_KEY`, `POLY_SECRET`, `POLY_PASSPHRASE`, `POLY_ADDRESS`, and `POLY_PRIVATE_KEY` environment variables.

## Known Limitations

- **Pre-signed orders have a fixed price** — signed at startup using the strategy threshold. The strategy must fire at the same price. Re-signing at different prices is planned for a future session.
- **Pre-signed pool is finite** — no auto-refill yet (planned for Session 7). Pool exhaustion stops the execution loop.
- **Executor is single-threaded** — burst triggers processed serially; queue_delay grows with burst depth.

## References

- Polymarket API docs: https://docs.polymarket.com/market-data/overview
- Rust CLOB client: https://github.com/Polymarket/rs-clob-client
