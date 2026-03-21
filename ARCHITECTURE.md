<!-- AGENT CONTEXT — READ THIS FIRST -->
<!-- This file is your primary context artifact. Read it before doing anything. -->
<!-- Current phase: production-ready pipeline with safety rails; first live trade completed -->
<!-- Active branch: master -->
<!-- Do not modify files outside your assigned scope without checking with the user -->

# Architecture: Low-Latency Polymarket Execution Service

## Purpose

This system is a low-latency trading pipeline for Polymarket's CLOB (Central Limit Order Book) on Polygon. It connects to Polymarket's WebSocket feed, maintains a real-time local order book, runs configurable strategies that produce trade signals, and dispatches pre-signed EIP-712 orders over warm HTTP/2 connections. The entire pipeline is instrumented with nanosecond-precision timestamps at 8 checkpoints, enabling precise measurement of what the code controls (trigger-to-wire) vs. what physics controls (network RTT). The binary is `pm-executor`. It reads `config.toml`, connects to live markets, and either logs triggers (dry-run) or sends real orders.

## Architecture Overview

The system is a streaming pipeline with four stages, connected by typed channels:

```
┌─────────────────┐
│  Polymarket WS  │  wss://ws-subscriptions-clob.polymarket.com/ws/market
└────────┬────────┘
         │ JSON frames (book snapshots, price_change deltas)
         ▼
┌─────────────────┐
│    pm-data       │  WsClient → FeedManager → {OrderBookManager, ReferenceStore} → Pipeline
│   (async tokio)  │  Emits `NormalizedUpdate`/`UpdateNotice` and preserves the legacy snapshot path
└────────┬────────┘
         │ broadcast<OrderBookSnapshot>
         ▼
┌─────────────────┐
│  bridge (async)  │  broadcast_to_mpsc: forwards snapshots
└────────┬────────┘
         │ mpsc<OrderBookSnapshot>
         ▼
┌─────────────────┐
│  pm-strategy     │  StrategyRunner: calls strategy.on_book_update()
│   (async tokio)  │  Produces TriggerMessage when conditions met
└────────┬────────┘
         │ mpsc<TriggerMessage>
         ▼
┌─────────────────┐
│  bridge (async)  │  mpsc_to_crossbeam: stamps timestamp_ns via clock::now_ns()
└────────┬────────┘
         │ crossbeam<TriggerMessage>  (sync, bounded)
         ▼
┌─────────────────────────────────────────────────────┐
│  Execution Loop (dedicated OS thread, NOT tokio)     │
│                                                      │
│  Safety checks:                                      │
│    1. CircuitBreaker tripped? → break                │
│    2. RateLimiter exceeded? → drop trigger            │
│    3. OrderGuard conflict? → drop trigger             │
│    4. CircuitBreaker amount check → may trip          │
│                                                      │
│  dry_run=true:  log "[DRY RUN] Would fire order"     │
│  dry_run=false: process_one_clob() →                 │
│    PreSignedOrderPool.dispatch() → warm H2 → CLOB API│
└──────────────────────────────────────────────────────┘
```

## Workspace Crates

```
rtt/
├── Cargo.toml              # Workspace root
├── config.toml             # Runtime configuration
├── scripts/fire.sh         # One-shot order test via ignored e2e test
├── crates/
│   ├── rtt-core/           # Core engine: connections, signing, execution, metrics
│   ├── rtt-bench/          # CLI benchmark tool
│   ├── pm-data/            # WebSocket client + order book management
│   ├── pm-strategy/        # Strategy trait + implementations + backtest
│   └── pm-executor/        # Main binary: wires everything together
```

## Components

### rtt-core

The foundational library. Everything latency-sensitive lives here.

#### `clock.rs`
- `fn now_ns() -> u64` — Monotonic nanoseconds since process start
- Uses `OnceLock<Instant>` epoch, initialized on first call
- All timestamps in the system come from this clock

#### `trigger.rs` — Shared types
```rust
struct TriggerMessage { trigger_id, token_id, side, price, size, order_type, timestamp_ns }
struct OrderBookSnapshot { asset_id, best_bid, best_ask, timestamp_ms, hash }
struct PriceLevel { price: String, size: String }
struct TradeEvent { asset_id, price, size, side, timestamp_ms }
enum Side { BUY, SELL }
enum OrderType { GTC, GTD, FOK, FAK }
```
All types derive `Serialize`/`Deserialize`. Prices and sizes are strings (decimal precision preserved).
This module remains the legacy executor/runtime DTO seam during the `11a`–`12a` migration.

#### `market.rs` — Shared market identity, metadata, and exact values
- `MarketId`, `AssetId`, `OutcomeSide`, `OutcomeToken`, `MarketStatus`
- Exact-value wrappers: `Price`, `Size`, `Notional`, `TickSize`, `MinOrderSize`
- `MarketMeta { market_id, yes_asset, no_asset, condition_id, tick_size, min_order_size, status, reward }`
- `RewardParams` plus explicit `RewardFreshness::{Fresh, StaleButUsable, Unknown}`
- Helper methods keep YES/NO pairing, market-to-asset lookup, and tradability checks explicit

#### `feed_source.rs` — Shared source identity
- `SourceId` — stable source-instance identity
- `SourceKind` — `polymarket_ws`, `polymarket_rest`, `external_reference`, `external_trade`, `synthetic`
- `InstrumentRef` — source-scoped subject identifier with `Source`, `Market`, `Asset`, and `Symbol` kinds

#### `polymarket.rs` — Shared Polymarket endpoints and identities
- `CLOB_HOST` / `CLOB_PORT` plus `CLOB_BASE_URL`, `CLOB_ORDER_*`, and `CLOB_AUTH_API_KEYS_*` centralize the live CLOB endpoint literals
- `MARKET_WS_URL` centralizes the public market-feed WebSocket URL
- `public_source_id()` centralizes the shared Polymarket public-feed source identity used by normalized public updates

#### `public_event.rs` — Normalized source updates and notices
- `UpdateNotice { source_id, source_kind, subject, kind, version, source_hash }` — small source-agnostic handoff object with explicit source-family discrimination
- `NormalizedUpdate { notice, payload }` — shared public-event envelope
- Payload variants: book snapshot, book delta, best bid/ask, trade tick, reference price, tick-size change, reconnect, source status, with room for future source-specific/custom update kinds
- Normalized payloads use the exact-value wrappers from `market.rs`; they are the shared pre-hot-state representation for later specs

**Note:** Polymarket’s order/CLOB path is colocated in **eu-west-1** (Dublin). AWS eu-west-1 is Ireland/Dublin; eu-west-2 is London — so eu-west-1 is the Dublin region.

#### `metrics.rs` — Latency instrumentation
```rust
struct TimestampRecord {
    t_trigger_rx,       // Trigger received from crossbeam queue
    t_dispatch_q,       // Dequeued for dispatch
    t_exec_start,       // Execution processing began
    t_buf_ready,        // Request buffer prepared
    t_write_begin,      // H2 frame submission started
    t_write_end,        // H2 frame dispatched to kernel (microseconds)
    t_first_resp_byte,  // First response byte from server (milliseconds)
    t_headers_done,     // Response fully collected
    t_sign_start,       // EIP-712 signing started (dynamic pricing)
    t_sign_end,         // EIP-712 signing completed
    is_reconnect: bool, // true only for reconnect/cold-path samples
    cf_ray_pop: String, // Cloudflare POP code (e.g. "EWR", "DUB")
    connection_index: usize,
}
```

Derived metrics (all in nanoseconds):
- `queue_delay()` = exec_start - trigger_rx
- `prep_time()` = buf_ready - exec_start
- `trigger_to_wire()` = write_begin - trigger_rx (what WE control)
- `write_duration()` = write_end - write_begin (frame submission, NOT RTT)
- `warm_ttfb()` = first_resp_byte - write_begin (network physics)
- `trigger_to_first_byte()` = first_resp_byte - trigger_rx (total latency)
- `sign_duration()` = sign_end - sign_start (EIP-712 signing overhead)

`StatsAggregator` computes p50/p95/p99/p99.9/max over warm samples only (reconnects filtered).

#### `connection.rs` — HTTP/2 connection pool
- `ConnectionPool::new(host, port, pool_size, address_family)`
- `warmup()` — DNS → TCP (NODELAY) → TLS (rustls, ALPN h2) → H2 SETTINGS
- `send_start(req) -> SendHandle` — Submit H2 frame (returns in microseconds)
- `ConnectionPool::collect(handle) -> Response` — Await server response (milliseconds) and reconnect failed connections before reuse
- `ConnectionError` — typed pool/send/collect/reconnect failures
- `health_check_detailed()` — Per-connection success/failure status for warmed sessions
- Round-robin via `AtomicUsize`, reconnect on failure
- `extract_pop(cf_ray)` — Parse Cloudflare POP from cf-ray header

#### `request.rs` — Fixed-capacity request template
- Fixed `[u8; 4096]` body with named patch slots
- `register_patch(offset, length)` / `patch(slot, value)` for in-place updates in benchmark and executor scaffolding
- `build_request()` copies the active body into `Request<Bytes>`
- Production CLOB order dispatch does not mutate signed payloads through this type

#### `clob_order.rs` — Polymarket order types
- `Order` defined via alloy `sol!` macro (automatic EIP-712 struct hash)
- Exchange addresses: standard (`0x4bFb...`) and neg-risk (`0xC5d5...`)
- `compute_amounts(price, size, side)` — fixed-point base-unit math with explicit `AmountError`
- `generate_salt()` — Random u64 masked to 53 bits (JSON number safety)
- `SignedOrderPayload` — Order + signature + orderType + owner (API-ready JSON)
- `SignatureType` — EOA (0), Poly (1), GnosisSafe (2)

#### `clob_signer.rs` — EIP-712 signing
- `make_domain(is_neg_risk)` — EIP-712 domain for Polygon chain_id=137
- `sign_order(signer, order, is_neg_risk)` — Produce hex signature
- `build_order(trigger, maker, signer, fee_rate_bps, sig_type)` — fallible `TriggerMessage -> Order` conversion (`BuildOrderError` for invalid token IDs or amount math)
- `presign_batch(signer, trigger, ..., count)` — Sign N orders with unique salts at startup

#### `clob_auth.rs` — L2 API authentication
- HMAC-SHA256: message = timestamp + method + path + body
- Secret: base64url-decoded from API credential
- Headers: POLY_ADDRESS (lowercase), POLY_API_KEY, POLY_PASSPHRASE, POLY_SIGNATURE, POLY_TIMESTAMP
- `L2Credentials { api_key, secret, passphrase, address }`
- Address in headers = EOA signer (not proxy wallet)
- `validate_credentials(creds)` — async, hits GET /auth/api-keys (read-only, no orders placed)
- `build_validation_request(creds)` — builds HMAC headers for validation

#### `clob_request.rs` — Request building
- `encode_order_payload(payload)` — Shared JSON encoder for signed orders
- `build_order_request(payload, creds)` — Full POST /order with fresh HMAC
- `build_order_request_from_bytes(body, creds)` — Shared request assembly for cached immutable payload bytes
- `RequestBuildError` — typed serialization/auth/http assembly failures
- Signed-payload mutation helpers are intentionally not public API
- Live integration coverage for request/auth/transport lives under `crates/rtt-core/tests/`

#### `clob_executor.rs` — Order dispatch (pre-signed and dynamic)
```rust
struct PreSignedOrderPool {
    bodies: Vec<Bytes>,    // Immutable pre-serialized JSON payloads
    cursor: usize,         // Next to consume
}
```
- `dispatch(creds)` — Consume next body, recompute HMAC headers only (body frozen)
- `process_one_clob(pool, presigned, creds, msg, rt)` — Pre-signed hot-path function (legacy) returning `DispatchOutcome`
- `sign_and_dispatch(pool, signer, trigger, creds, maker, signer_addr, ...)` — **Dynamic pricing hot-path** (default):
  1. `build_order(trigger, ...)` → builds Order at **trigger's price** (not config price)
  2. `sign_order(signer, order, ...)` → EIP-712 sign (~100-500us)
  3. Build `SignedOrderPayload` → serialize to JSON → shared request encoder computes HMAC headers
  4. `pool.send_start(req)` → submit H2 frame (microseconds)
  5. `pool.collect(handle)` → await response (milliseconds)
  6. Returns `DispatchOutcome::{Sent, Rejected}` with `DispatchError` classification; `is_reconnect` is reserved for reconnect/cold-path samples only

#### `executor.rs` — Threading primitives
- `IngressThread` — Stamps `timestamp_ns` on triggers, sends to crossbeam queue
- `ExecutionThread` — Dedicated OS thread with internal tokio runtime, spin-loops on `try_recv()`
- `MaintenanceThread` — Periodic health checks via GET /
- `pin_to_core(core_id)` — CPU affinity via `core_affinity` crate

#### `benchmark.rs` — Latency profiling
- 3 modes: SingleShot (200ms delay), RandomCadence (50-500ms), BurstRace (N triggers + 500ms)
- Warms pool, injects synthetic triggers, collects TimestampRecords
- POP distribution histogram, warm/cold separation

#### `h3_stub.rs` — HTTP/3 placeholder
- `probe_alt_svc()` confirms Cloudflare advertises h3 (it does)
- Full QUIC client not implemented
- `cargo test -p rtt-core --lib` remains offline because live H2/H3 coverage is kept under `crates/rtt-core/tests/`

### pm-data

Real-time market data from Polymarket WebSocket.

#### `ws.rs` — WebSocket client
- Connects to `wss://ws-subscriptions-clob.polymarket.com/ws/market`
- Sends market subscription commands with `{"type": "market", "operation": "subscribe" | "unsubscribe", "assets_ids": [...]}`
- 10-second ping interval, auto-reconnect with exponential backoff (1s base, 2x factor, 60s cap, 500ms jitter)
- Replays the full desired subscription set after reconnect and can accept live diff commands from the feed manager without rebuilding the socket client
- Broadcasts `WsMessage` enum to subscribers (including `WsMessage::Reconnected(ReconnectEvent { sequence, timestamp_ms })` on reconnect)
- `reconnect_count: Arc<AtomicU64>` — incremented on each reconnect
- `last_message_at: Arc<AtomicU64>` — epoch millis of last received message

#### `subscription_plan.rs` — Diff planner and shard assignment
- Encodes the verified market-channel semantics (`subscribe`, `unsubscribe`, no ack expected, reconnect must replay desired subscriptions) behind a small stable adapter
- Computes deterministic adds/removes/unchanged sets from current vs desired subscriptions
- Plans bounded subscription commands with per-command pacing and explicit shard ownership so one feed instance can stay single-connection by default or own a stable shard when scaling is enabled

#### `types.rs` — WebSocket message types and normalization
```rust
enum WsMessage {
    Book(BookUpdate),           // Full order book snapshot
    PriceChange(PriceChangeEvent),  // Incremental delta
    LastTradePrice(...),        // Trade/tick update
    TickSizeChange(...),        // Tick-size metadata update
    BestBidAsk(...),            // Best bid/ask update
    Reconnected(...),           // Emitted after WS reconnect
}
```
Tagged by `event_type` field in JSON.
- `polymarket_public_source_id()` identifies the shared public-feed source instance
- `to_normalized_updates()` converts raw Polymarket wire messages into `rtt_core::NormalizedUpdate` values; the feed-manager layer now scopes those updates to the owning source instance before applying stores and broadcasting notices

#### `registry_provider.rs` — Discovery provider boundary
- `RegistryProvider` is the async control-plane trait for paged market discovery
- `GammaRegistryProvider` crawls Gamma `events` pages, normalizes valid records into shared `MarketMeta`, and quarantines malformed upstream markets instead of poisoning the whole refresh
- `RegistryPageRequest` / `RegistryPage` make offset-based traversal explicit and keep retry/backoff policy out of the hot path

#### `snapshot.rs` — Registry snapshots and universe selection
- `RegistrySnapshot` stores provider identity, refresh sequence/timestamp, the full normalized market set, and quarantined record metadata
- `SelectedUniverse` applies deterministic include/exclude decisions over the snapshot with explicit bypass support for direct source bindings
- Snapshot JSON import/export exists for deterministic offline registry replay

#### `market_registry.rs` — Refresh orchestration
- `MarketRegistry` owns paged discovery refreshes, retry/backoff, last-known-good fallback, and selection projection
- `RegistryRefreshPolicy` keeps page size, cadence, and retry policy explicit without coupling discovery-backed workloads to the live WebSocket path
- Refresh failure returns the last known good snapshot/universe in degraded mode instead of erasing the control-plane state

#### `orderbook.rs` — Local order book state
- `OrderBookManager` — `Arc<RwLock<HashMap<asset_id, BookState>>>`
- `BookState` uses `BTreeMap<String, String>` for price ladders (sorted by price string)
- `apply_book_update()` — Full snapshot replacement
- `apply_price_change()` — Incremental delta (upsert/remove on "0" size)
- `clear_all()` — Empties all order books (called on WS reconnect)
- Best bid = last BTreeMap entry (highest), best ask = first (lowest)

#### `reference_store.rs` — Non-depth source state
- `ReferenceStore` keeps the latest non-book state keyed by source-scoped subject
- Tracks last seen reference-price, trade-tick, BBO, tick-size-change, reconnect, and source-status updates plus the last emitted `UpdateNotice`
- Provides the small-notice resolution seam for feeds that are not backed by full depth and for informational Polymarket events that should survive past parsing

#### `feed.rs` — Feed-manager and source-adapter boundary
- `ScopedPolymarketAdapter` rewrites normalized updates onto the owning `SourceId`, so shared and dedicated source instances can reuse the frozen `11a` event model without mutating it
- `PolymarketFeedManager` is the explicit owner for one live Polymarket source instance: it owns the `WsClient`, authoritative stores, notice/update fan-out, reconnect/reset behavior, and diff-driven asset-set reconfiguration
- `FeedStores` keeps full state in-process while `FeedOutputs` broadcasts small `UpdateNotice`s plus richer `NormalizedUpdate`s for consumers that need them
- Reconfiguration now clears only removed asset state, stages unsubscribe/subscribe batches through the `WsClient`, and can be constructed with explicit shard ownership while keeping the default single-connection path intact
- The manager preserves the legacy `OrderBookSnapshot` broadcast only for `BookSnapshot` / `BookDelta` payloads so the old runtime path continues to work during the `11c` → `12a` transition

#### `pipeline.rs` — Compatibility wrapper
- `Pipeline` is now a thin wrapper over the shared Polymarket `FeedManager`
- Still exposes `subscribe_snapshots()` / `order_books()` / WS health counters for the legacy trigger/runtime path
- Also exposes `subscribe_updates()`, `subscribe_notices()`, `reference_store()`, and `reconfigure_assets()` for the new notice-driven path and later runtime work

### pm-strategy

Trading strategy framework.

#### `strategy.rs` — Split strategy contracts and requirements
```rust
trait Strategy: Send + Sync {
    fn on_book_update(&mut self, snapshot: &OrderBookSnapshot) -> Option<TriggerMessage>;
    fn on_trade(&mut self, trade: &TradeEvent) -> Option<TriggerMessage>;
    fn name(&self) -> &str;
}
```
- The legacy `Strategy` trait remains the compatibility surface for the snapshot runner and the `12a` notice bridge
- `TriggerStrategy` and `QuoteStrategy` add explicit behavior-specific contracts over a shared `StrategyRuntimeView`
- `StrategyRequirements` declares `ExecutionMode`, `IsolationPolicy`, and the data requirements a strategy needs, such as `polymarket_bbo` or `external_reference_price`
- `StrategyRuntimeView` exposes resolved hot book/reference state plus snapshot-projection helpers so trigger strategies can be upgraded without learning feed topology details

#### `quote.rs` — Desired quote outputs
- `QuoteId` gives each quote lane a stable local identity for downstream reconciliation
- `DesiredQuote` and `DesiredQuotes` describe quote intent only
- Quote reconciliation, order lifecycles, and exchange sync remain outside `12b`

#### `threshold.rs` — ThresholdStrategy
- Fires when best_ask <= threshold (buy) or best_bid >= threshold (sell)
- Auto-incrementing trigger_id, timestamp from `Instant::elapsed()`
- Also implements `TriggerStrategy` by declaring a shared-acceptable Polymarket BBO requirement and adapting the resolved runtime view back into the legacy snapshot logic

#### `spread.rs` — SpreadStrategy
- Fires when bid-ask spread < max_spread
- Buy side uses ask price, sell side uses bid price
- Also implements `TriggerStrategy` with the same explicit requirement declaration model as `ThresholdStrategy`

#### `config.rs` — TOML-driven factory
```rust
struct StrategyConfig { strategy, token_id, side, size, order_type, params }
struct StrategyParams { threshold: Option<f64>, max_spread: Option<f64> }
```
- `build_strategy()` — Factory method: "threshold" or "spread" → `Box<dyn Strategy>`
- `build_trigger_strategy()` — Compatibility factory for the new explicit trigger contract without changing the config file shape

#### `runner.rs` — Async execution loop
- Receives `OrderBookSnapshot` via mpsc, calls `strategy.on_book_update()`
- Forwards `TriggerMessage` to mpsc sender
- Exits when input channel closes

#### `runtime.rs` — Shared runtime scaffolding
- `NoticeDrivenRuntime` consumes `rtt_core::UpdateNotice` values, resolves the current `OrderBookSnapshot` view from `HotStateStore`, and invokes the existing `Strategy` trait without widening strategy-facing contracts
- This is the `12a` migration seam from feed-manager notices to strategy logic while the legacy snapshot runner remains available
- `RuntimeTopologyPlan` and `ProvisionedInput` translate strategy requirements into shared vs dedicated source-instance ownership without exposing transport details to the strategy
- `SharedRuntimeScaffold` resolves the current notice version exactly and only exposes companion source state after that source's notice stream has also been observed, preventing cross-feed strategies from seeing future state out of order
- `TriggerRuntime` and `QuoteRuntime` both evaluate a uniform `StrategyRuntimeView`, so single-feed and cross-feed strategies share the same filtering, topology, and hot-state plumbing
- `QuoteRuntime` now also carries a small in-process inventory store fed by executor exposure deltas, so quote strategies can consume `Inventory` / `LiveOrderState` requirements without depending on a hedge or P&L subsystem

#### `backtest.rs` — Offline replay
- `BacktestRunner::run(strategy, snapshots)` — Replay historical data through any strategy
- `BacktestRunner::run_notice_replay(strategy, markets, updates)` — Replay normalized updates through `HotStateStore` and compare behavior against the legacy snapshot path
- `BacktestRunner::run_trigger_notice_replay(strategy, markets, updates)` — Replay normalized updates through the explicit trigger contract and shared runtime scaffold
- `BacktestRunner::run_quote_notice_replay(strategy, markets, updates)` — Replay normalized updates through the quote contract using the same topology-aware state resolution
- `load_snapshots(path)` — Load from JSON file

### pm-executor

Main binary. Wires all components together.

#### `main.rs` — Entry point
1. Loads `config.toml` (or `--config <path>`)
2. `--validate-creds` flag: validates credentials against live API and exits
3. Builds credentials (validates only in live mode)
4. Branches by strategy mode:
   - trigger strategies keep the legacy snapshot → trigger → execution-thread path
   - `liquidity_rewards` runs a quote-mode controller that owns startup market selection, `QuoteRuntime`, reconciliation, quote execution, telemetry sampling, and fail-closed kill switches
5. Live mode validates credentials against API; the legacy trigger branch also warms the HTTP/2 pool for hot-path order dispatch
6. Loads persisted state from `state.json` (restores circuit breaker counters)
7. Starts WebSocket pipeline, health monitor, and HTTP health server
8. Waits for Ctrl+C, graceful shutdown with 5-second timeout
9. Persists state (orders fired, USD committed, tripped status) on shutdown

#### `config.rs` — Configuration
```rust
struct ExecutorConfig {
    credentials: CredentialsConfig,
    connection: ConnectionConfig,   // pool_size=2, address_family="auto"
    websocket: WebSocketConfig,     // asset_ids, channel capacities
    strategy: StrategyConfig,       // reuses pm-strategy config
    execution: ExecutionConfig,     // presign_count=100, dry_run=true, state_file="state.json", is_neg_risk/fee_rate_bps/signature_type live signing controls
    quote_mode: QuoteModeConfig,    // analysis_db_path, quote API base URL, user WS URL, heartbeat/telemetry intervals
    safety: SafetyConfig,           // max_orders=5, max_usd_exposure=10.0, alert_webhook_url
    health: HealthConfig,           // enabled=true, port=9090
    logging: LoggingConfig,         // level="info"
}
```
All credential fields support `POLY_*` env var overrides. `alert_webhook_url` supports `POLY_ALERT_WEBHOOK_URL`.
For backwards compatibility with older deploys, `pm-executor` also accepts the legacy Polymarket names `POLY_SECRET`, `POLY_ADDRESS`, and `POLY_PROXY_ADDRESS` as fallbacks for `POLY_API_SECRET`, `POLY_SIGNER_ADDRESS`, and `POLY_MAKER_ADDRESS`.
Runtime switching for deployments also supports `RTT_*` env var overrides, including:
- `RTT_STRATEGY=liquidity_rewards`
- `RTT_DRY_RUN=false`
- signing controls such as `RTT_NEG_RISK`, `RTT_FEE_RATE_BPS`, and `RTT_SIG_TYPE`
- bankroll/quote controls such as `RTT_MAX_TOTAL_DEPLOYED_USD`, `RTT_BASE_QUOTE_SIZE`, `RTT_EDGE_BUFFER`
- quote runtime settings such as `RTT_ANALYSIS_DB_PATH`, `RTT_CLOB_BASE_URL`, and `RTT_USER_WS_URL`

To stay compatible with the known-good `scripts/fire.sh` live-order lane, `pm-executor` also honors the older execution env names `NEG_RISK`, `FEE_RATE_BPS`, and `SIG_TYPE` when the `RTT_*` forms are absent. Live runtime signing therefore follows the same startup contract as `fire.sh`: explicit env-provided signing parameters win, and only missing `SIG_TYPE` falls back to address-based derivation.

#### `capital.rs` — Deployment-budget accounting
- Computes active deployed capital as working quote notional plus unresolved inventory notional
- Projects post-command capital so quote-mode execution can reject any place/cancel set that would breach the configured bankroll cap
- Keeps the `$100` low-risk deployment limit enforced independently from strategy-side quote generation

#### `analysis_store.rs` — Append-only SQLite operation journal
- Opens a dedicated SQLite database at startup and fails fast if it cannot be created
- Appends one row per material operation with timestamps, quote/order identifiers, requested price/size, status, error text, and capital before/after
- Quote mode now records per-order `quote_submit_result` rows so operators can distinguish request errors, rejected orders, non-resting statuses (`matched` / `delayed` / `unmatched`), and true resting `live` placements
- Used only for offline research and operator diagnostics; it does not replace the JSON restart-state file

#### `order_state.rs` — Local quote lifecycle state
- `WorkingQuoteState` is explicit: `PendingSubmit`, `Working`, `PendingCancel`, `Canceled`, `Rejected`, `UnknownOrStale`
- `WorkingQuote` is the local trusted-state record keyed by `QuoteId`, carrying the last desired order parameters plus local timestamps and optional `client_order_id`
- `UnknownOrStale` is first-class from v1, so the local planner can halt instead of guessing when trust is lost
- `ExchangeObservedQuote` and `ExchangeObservedQuoteState` are the exchange-facing overlay that merges authoritative working/canceled/rejected observations onto local quote state
- Timeouts, authoritative absences, and reconnect-resync gaps now transition active quotes into `UnknownOrStale` through explicit helpers instead of ad hoc executor guesses

#### `order_manager.rs` — Deterministic local and exchange-aware reconciliation
- `ExecutionCommand::{Place, Cancel, CancelAll}` is the local command plan emitted by the order manager
- `LocalOrderManager` still computes a deterministic minimal command set, but `12d` extends it to reconcile across desired state, local working state, and an `ExchangeSyncSnapshot`
- `ExchangeSyncSnapshot` is the exchange-observed seam for authoritative order presence, reconnect-resync gaps, and fill snapshots, whether that data comes from polling, a documented resync endpoint, or a later private feed adapter
- `ReconciliationPolicy` now also defines submit/cancel confirmation timeouts so stale pending orders transition into explicit uncertainty instead of lingering forever
- `ReconciliationOutcome` returns synchronized working state, `resync_required`, and minimal `ExposureDelta` values alongside the command plan so later runtime layers can consume recovery and inventory signals directly
- If any local quote is `UnknownOrStale`, reconciliation returns a blocked outcome with no speculative commands

#### `bridge.rs` — Channel adapters
- `broadcast_to_mpsc()` — Forwards OrderBookSnapshot, handles `Lagged` by logging warning
- `mpsc_to_crossbeam()` — Forwards TriggerMessage, **stamps `timestamp_ns`** via `clock::now_ns()`

#### `execution.rs` — Execution loop
- `build_credentials()` — Dry-run allows empty creds; live validates all fields
- `SignerParams` — Holds signer, maker, fee_rate_bps, neg-risk flag, signature type, and owner/api-key context for dynamic signing
- `run_execution_loop()` — Spin loop on `crossbeam::try_recv()` with `yield_now()`
  - Creates dedicated tokio runtime inside OS thread
  - 4-layer safety: CircuitBreaker → RateLimiter → OrderGuard → CircuitBreaker amount check
  - With `signer_params`: uses `sign_and_dispatch()` (signs at trigger's price)
  - Without `signer_params`: falls back to `process_one_clob()` (pre-signed orders)
  - Handles `DispatchOutcome` explicitly so build/request/pool failures do not masquerade as reconnect samples
  - Sends webhook alert on circuit breaker trip
- `QuoteCommandPolicy`, `QuoteCommandThrottle`, and `retry_decision()` define the bounded retry/backoff/throttling seam for quote-maintenance commands without redesigning the current trigger hot path
- `QuoteApiClient` signs `DesiredQuote` values directly (including GTD expirations), submits quote batches as documented post-only maker orders, batches `POST /orders` and `DELETE /orders`, issues `DELETE /cancel-all`, sends chained heartbeats to `/v1/heartbeats`, and samples `/rewards/user/percentages` plus `/rebates/current`
- Quote-mode submit handling treats only `status = live` as a resting working quote; successful-but-non-resting statuses such as `matched`, `delayed`, and `unmatched` are journaled and fed back into reconciliation instead of being assumed open on exchange
- Quote-mode execution is correctness-first rather than latency-first: it uses authenticated REST requests and deterministic batching instead of the trigger branch's dedicated HTTP/2 thread

For liquidity-rewards quote generation, passive maker behavior is enforced twice:
- the strategy clamps depth-aware bid prices to remain at least one tick below the current best ask, so a size-cutoff-adjusted midpoint cannot accidentally turn a quote marketable
- the executor submits quote orders with `postOnly = true`, so any remaining cross-book race is rejected by the exchange instead of executing as taker flow

Live signature type selection now follows a two-step rule:
- if `ExecutionConfig.signature_type` is set via config/env (`RTT_SIG_TYPE` or legacy `SIG_TYPE`), that explicit value is used
- otherwise the executor derives `EOA (0)` when maker and signer addresses match, or `GnosisSafe (2)` when a proxy maker differs from the signing EOA

#### `safety.rs` — Lock-free safety rails
- **CircuitBreaker**: Atomic counters for orders fired and USD committed (cents). Once tripped, stays tripped (restart required). Limits: `max_orders=5`, `max_usd_exposure=10.0` (conservative defaults).
  - `with_initial_counts()` — Restores counters from persisted state on startup
- **RateLimiter**: 1-second sliding window. Drops excess triggers (not queued). Limit: `max_triggers_per_second=2`.
- **OrderGuard**: `AtomicBool` CAS. Ensures single in-flight order at a time (when `require_confirmation=true`).

#### `alert.rs` — Webhook alerting
- `send_alert(url, message)` — Fire-and-forget POST to Slack-compatible webhook
- Sends `{"text": "..."}` JSON body with 5-second timeout
- Called on circuit breaker trip (with order/USD stats in message)

#### `state.rs` — State persistence
- `ExecutorState { orders_fired, usd_committed_cents, last_shutdown, tripped }`
- `load(path)` — Returns `Default` on missing/corrupt file (safe startup)
- `save(path)` — Writes pretty JSON
- `from_stats()` — Builds state from circuit breaker stats with ISO 8601 timestamp
- Persisted every 30s by health monitor and on graceful shutdown

#### `health.rs` — Periodic health reporting
- 30-second interval, logs: asset count, orders fired, USD committed, circuit breaker status
- Persists state to `state_file` every 30 seconds

#### `health_server.rs` — HTTP health endpoint
- Raw hyper HTTP/1 server on configurable port (default 9090)
- `GET /health` — Returns 200 if healthy, 503 if circuit breaker tripped or WS stale (>60s)
- `GET /status` — Returns JSON with orders_fired, usd_committed, tripped, uptime_secs, reconnects, ws_stale
- Graceful shutdown via watch channel

#### `logging.rs` — Structured logging
- `tracing_subscriber` with env-filter
- Suppresses verbose rtt-core instrumentation modules and upstream library chatter
- Respects `RUST_LOG` env var

#### `user_feed.rs` — Authenticated Polymarket user-channel adapter
- Parses documented user-channel order/trade events into executor-facing order observations and fill deltas
- Maintains a non-authoritative exchange snapshot for quote reconciliation: observed working/canceled/rejected quotes plus deduplicated fills keyed by trade/order pair
- Runs a lightweight WebSocket client against `wss://ws-subscriptions-clob.polymarket.com/ws/user`, authenticates with API key/secret/passphrase, subscribes to the selected condition set, sends `{}` pings every 10 seconds, and marks the controller degraded on disconnect or parse failure
- Quote mode treats user-feed degradation as fail-closed: stop quoting, mark local quotes stale, and issue `CancelAll`

### rtt-bench

CLI benchmark tool. Wraps rtt-core's benchmark module.

```
rtt-bench --benchmark --samples 100 --connections 2 --mode single-shot --af v6
rtt-bench --trigger-test  # single trigger
```

## Data Flow (Detailed)

### Trigger-to-wire path (hot path)

1. **WebSocket frame arrives** → `WsClient` parses JSON → broadcasts `WsMessage`
2. **FeedManager processes** → scopes `NormalizedUpdate`s to its source instance, applies `OrderBookManager` / `ReferenceStore`, emits `UpdateNotice`, and still broadcasts `OrderBookSnapshot` for book-changing events
3. **Bridge** → `broadcast_to_mpsc()` forwards the legacy snapshot stream
4. **StrategyRunner** → calls `strategy.on_book_update(snapshot)` → if condition met, returns `TriggerMessage { trigger_id, token_id, side, price, size, order_type, timestamp_ns: 0 }`
5. **Bridge** → `mpsc_to_crossbeam()` **stamps `timestamp_ns = clock::now_ns()`**, sends to crossbeam channel
6. **Execution loop** (OS thread) → `try_recv()` gets `TriggerMessage`
7. **Safety checks** → CircuitBreaker → RateLimiter → OrderGuard → CircuitBreaker amount
8. **`sign_and_dispatch()`** (dynamic pricing, default):
   - `build_order(trigger, ...)` → Order at trigger's price
   - `sign_order(signer, order, ...)` → EIP-712 sign (~100-500us, measured in `sign_duration`)
   - Build `SignedOrderPayload` → shared encoder serializes → computes HMAC → `Request<Bytes>`
   - `pool.send_start(req)` → submits H2 frame to kernel buffer → returns `SendHandle` (microseconds)
   - `pool.collect(handle)` → awaits server response (milliseconds: network RTT + server processing)
   - Returns typed `DispatchOutcome`; only reconnect/cold-path failures set `is_reconnect`
   - Records all 10 timestamps (including sign_start/sign_end), extracts cf-ray POP
9. **Response parsed** → `OrderResponse { success, order_id, status, error_msg, ... }`
10. **OrderGuard released**

### Dynamic signing flow (default, Spec 02)

1. Trigger arrives with market price from strategy
2. `build_order(trigger, maker, signer, ...)` → Order at trigger's exact price
3. `sign_order(signer, order, is_neg_risk)` → EIP-712 sign on Polygon (chain_id=137)
4. Build `SignedOrderPayload`, serialize to JSON, compute HMAC headers
5. Dispatch via warm H2 connection pool

### Pre-signing flow (legacy, not used in default path)

1. Strategy threshold determines the order price
2. `presign_batch()` creates N orders with unique random salts (53-bit masked)
3. Each order is EIP-712 signed on Polygon (chain_id=137)
4. Orders are serialized to JSON and stored as immutable `Bytes` in `PreSignedOrderPool`
5. At trigger time, body is used as-is (signature remains valid), only HMAC headers recomputed

## Key Design Decisions

### DECISION: Dynamic pricing — sign orders at trigger time (Spec 02)
- **PREVIOUS**: Pre-sign orders at startup with fixed price from config threshold
- **CURRENT**: Sign each order at trigger arrival using the trigger's market price via `sign_and_dispatch()`
- **REASON**: Pre-signing at a fixed price made orders fill at wrong prices or get rejected when the market moved. Dynamic pricing adds ~100-500us for EIP-712 signing to the hot path but ensures correct pricing.
- **TRADEOFFS**: Signing on hot path adds latency (measured via `sign_duration()` metric). Pre-signed pool kept as fallback (`process_one_clob`) but not used in default path. `PreSignedOrderPool` retained for potential future use (price ladders, pre-computed levels).

### DECISION: Dedicated OS thread for execution, not tokio task
- **ALTERNATIVES CONSIDERED**: tokio::spawn, tokio::spawn_blocking, async execution loop
- **REASON**: `try_recv()` spin loop with `yield_now()` avoids tokio task scheduling overhead. The execution thread creates its own single-threaded tokio runtime internally for the async H2 operations (`send_start` + `collect`). This keeps the trigger dequeue path synchronous and deterministic.
- **TRADEOFFS**: Wastes a CPU core spinning. Not suitable for multi-strategy parallelism without multiple threads.

### DECISION: Split `send_start` / `collect` instrumentation
- **ALTERNATIVES CONSIDERED**: Single `send()` call with before/after timestamps
- **REASON**: `send_start` submits the H2 frame to the kernel (microseconds). `collect` awaits the server response (milliseconds). Separating them lets us measure what we control (trigger-to-wire, write_duration) independently from what physics controls (warm_ttfb). This was the key insight from Session 9 — before the split, `write_duration` was ~162ms (included RTT), after the split it dropped to <1ms.
- **TRADEOFFS**: Two `block_on()` calls per request instead of one. Negligible overhead.

### DECISION: Crossbeam channel between async and sync worlds
- **ALTERNATIVES CONSIDERED**: tokio::sync::mpsc with blocking recv, std::sync::mpsc, flume
- **REASON**: Crossbeam's bounded channel has excellent performance for SPSC patterns and works natively in sync context. The async-to-sync bridge stamps `timestamp_ns` at the handoff point, giving an accurate trigger receive time.
- **TRADEOFFS**: crossbeam is MPMC (more sync overhead than pure SPSC). A dedicated SPSC ring buffer (e.g. `rtrb`) would be faster but adds complexity.

### DECISION: Rust over C++
- **ALTERNATIVES CONSIDERED**: Continue with C++ prototype (completed through milestone 7.3, 92 tests)
- **REASON**: The C++ version achieved ~8µs trigger-to-wire but required manual memory management, nghttp2 callback ceremony, and platform-specific build hacks (Homebrew libnghttp2 paths). Rust's hyper + rustls stack provides safety guarantees, cross-platform builds, and adequate performance (~80µs debug, estimated ~10-20µs release). The entire C++ prototype was ported and then deleted (commit `6e6016f`).
- **TRADEOFFS**: Rust's async bridge adds overhead vs. C++'s direct nghttp2 calls. Release build optimizations not yet fully characterized.

### DECISION: rustls over native-tls (OpenSSL)
- **ALTERNATIVES CONSIDERED**: native-tls (wraps OpenSSL), openssl crate directly
- **REASON**: ALPN h2 negotiation failed silently on macOS with native-tls. rustls handles ALPN correctly and provides consistent behavior across platforms. Uses webpki-roots for CA certificates (no system cert store dependency).
- **TRADEOFFS**: rustls is pure Rust without hardware-accelerated AES-NI by default. Enabling `aws-lc-rs` backend would add assembly-optimized crypto but is not yet configured.

### DECISION: BTreeMap for order book price ladders
- **ALTERNATIVES CONSIDERED**: HashMap + manual sorting, Vec with binary search, custom skip list
- **REASON**: BTreeMap provides O(log n) insert/remove with automatic sort order. Best bid = last entry, best ask = first entry. Price keys are strings (exact decimal matching, no floating point).
- **TRADEOFFS**: String comparison for ordering means "0.9" < "0.95" works correctly but "0.10" < "0.9" also sorts correctly by coincidence (Polymarket uses consistent decimal formats). Would break with inconsistent decimal places.

### DECISION: Lock-free atomics for safety rails
- **ALTERNATIVES CONSIDERED**: Mutex-protected counters, channel-based accounting
- **REASON**: CircuitBreaker, RateLimiter, and OrderGuard all use `AtomicU64`/`AtomicBool` for zero-contention updates from the hot path. USD tracked in cents (integer) to avoid floating-point atomics.
- **TRADEOFFS**: Once the circuit breaker trips, it stays tripped (requires process restart). This is intentional — the system fails closed.

### DECISION: Prices as strings, not floats
- **ALTERNATIVES CONSIDERED**: f64 everywhere, rust_decimal crate
- **REASON**: Polymarket's API uses string prices ("0.45", "0.95"). EIP-712 signing requires exact USDC amounts computed from these strings. Using f64 for computation and truncating to integer is sufficient for 6-decimal USDC precision without adding a decimal library dependency.
- **TRADEOFFS**: f64 arithmetic has rounding edge cases at extreme precision. Accepted because USDC has only 6 decimals.

### DECISION: Signature type defaults to GnosisSafe (2)
- **ALTERNATIVES CONSIDERED**: EOA (0), Poly Proxy (1)
- **REASON**: Polymarket's Magic Link wallets are Gnosis Safe proxy contracts. The EOA signs, but the proxy wallet is the maker/funder. signatureType=2 tells the exchange to verify the EOA signature against the proxy wallet's authorized signers.
- **TRADEOFFS**: Requires knowing both the EOA address (for auth) and proxy address (for order maker). These are separate env vars: `POLY_ADDRESS` (EOA, used in HMAC headers) and `POLY_PROXY_ADDRESS` / `POLY_MAKER_ADDRESS` (proxy, used in order struct).
- The executor validates this separation in live mode: HMAC/L2 auth uses the signer EOA, order structs use the maker/proxy wallet, and a configured signer address must match the supplied private key.
- Runtime signer selection now derives the signature type from the live maker/signer pair: same address => `EOA (0)`, proxy maker with distinct signer => `GnosisSafe (2)`. This prevents live quote mode from hard-coding the wrong signature type for proxy-wallet accounts.

## External Dependencies

| Dependency | Purpose | What breaks if unavailable |
|---|---|---|
| Polymarket WS (`wss://ws-subscriptions-clob.polymarket.com/ws/market`) | Real-time order book data | No market data, no triggers, pipeline idle |
| Polymarket CLOB API (`https://clob.polymarket.com`) | Order submission | Orders fail, but pipeline continues (circuit breaker trips) |
| Cloudflare CDN | Fronts CLOB API, provides cf-ray POP | TLS handshake fails, connections down |
| Polygon RPC (via alloy) | Not used at runtime; only in `approve.js` setup script | No runtime impact |

The Polymarket public WS parser is intentionally tolerant of newly-added informational `event_type` values. Unknown market events are ignored as no-ops unless they carry one of the supported book/trade/reference payloads used by the runtime.
The authenticated user-feed path also follows the documented plain-text heartbeat contract for market/user sockets: send `PING`, accept `PONG`, and ignore non-payload heartbeat frames so quote mode does not fail closed on keepalive traffic.

**Rust crate dependencies (key ones):**
- `hyper` 1.x + `hyper-util` — HTTP/2 client
- `tokio-rustls` 0.26 + `rustls` 0.23 — TLS
- `alloy` 1.x — EIP-712 signing, Ethereum types
- `crossbeam-channel` 0.5 — Sync bounded channels
- `tokio-tungstenite` 0.26 — WebSocket client
- `clap` 4.x — CLI parsing (rtt-bench only)
- `tracing` + `tracing-subscriber` — Structured logging

## Configuration

### `config.toml` (all sections)

```toml
[credentials]
api_key = ""          # POLY_API_KEY env override
api_secret = ""       # POLY_API_SECRET env override
passphrase = ""       # POLY_PASSPHRASE env override
private_key = ""      # POLY_PRIVATE_KEY env override
maker_address = ""    # POLY_MAKER_ADDRESS env override (proxy wallet)
signer_address = ""   # POLY_SIGNER_ADDRESS env override (EOA)

[connection]
pool_size = 2         # Number of warm H2 connections
address_family = "auto"  # "auto", "ipv4", "ipv6"

[websocket]
asset_ids = ["48825..."]  # Legacy explicit Polymarket asset subscriptions; still supported
ws_channel_capacity = 1024
snapshot_channel_capacity = 256

# Optional discovery-backed control-plane shape. The registry exists in pm-data,
# but executor wiring still remains on the legacy explicit-subscription path here.
# [websocket.market_universe]
# mode = "discovery"
# source_id = "gamma-primary"
# market_ids = ["0xmarket-1", "0xmarket-2"]

# Optional explicit per-source binding shape.
# [[websocket.source_bindings]]
# source_id = "polymarket-public"
# source_kind = "polymarket_ws"
# asset_ids = ["48825...", "12345..."]
#
# [[websocket.source_bindings]]
# source_id = "reference-mid"
# source_kind = "external_reference"
# instrument_ids = ["BTC-USD"]

[strategy]
strategy = "threshold"    # "threshold" or "spread"
token_id = "48825..."     # Execution asset; executor also merges explicit source bindings when resolving subscriptions
side = "Buy"
size = "5"                # USDC amount
order_type = "FOK"        # FOK, FAK, GTC, GTD

[strategy.params]
threshold = 0.45          # ThresholdStrategy: fire when ask <= 0.45 (buy)
# max_spread = 0.02       # SpreadStrategy: fire when spread < 0.02

[execution]
presign_count = 100       # Orders pre-signed at startup
is_neg_risk = false       # Use neg-risk exchange contract
fee_rate_bps = 0          # Taker fee (some markets require >0, e.g. 1000)
dry_run = true            # SAFETY: false = real orders

[safety]
max_orders = 10           # Circuit breaker: total orders before halt
max_usd_exposure = 50.0   # Circuit breaker: max USD committed
max_triggers_per_second = 2  # Rate limiter
require_confirmation = true  # Wait for response before next order

[logging]
level = "info"            # Also controlled by RUST_LOG env var
```

### Environment variables

| Variable | Purpose | Required when |
|---|---|---|
| `POLY_API_KEY` | L2 API key | Live mode |
| `POLY_API_SECRET` | L2 API secret (base64url-encoded) | Live mode |
| `POLY_PASSPHRASE` | L2 API passphrase | Live mode |
| `POLY_PRIVATE_KEY` | EOA private key (hex, with or without 0x) | Live mode |
| `POLY_MAKER_ADDRESS` | Proxy wallet address (order maker/funder) | Live mode |
| `POLY_SIGNER_ADDRESS` | EOA address (HMAC auth, lowercased in headers) | Live mode |
| `RUST_LOG` | Override tracing filter | Optional |

Dry-run mode allows all credentials to be empty.

## Running

```bash
# All tests (251 pass, 2 ignored)
cargo test --workspace

# Unit tests only (no network)
cargo test --workspace --lib

# rtt-core offline unit tests
cargo test -p rtt-core --lib

# rtt-core live integration tests (no orders placed)
cargo test -p rtt-core --test '*'

# Single crate
cargo test -p rtt-core
cargo test -p pm-data
cargo test -p pm-strategy
cargo test -p pm-executor

# Validate Polymarket credentials without placing an order
cargo run -p pm-executor -- --validate-creds

# Run pipeline in dry-run mode
cargo run -p pm-executor

# Run pipeline with custom config
cargo run -p pm-executor -- --config my_config.toml

# Fire a single real order (needs .env with POLY_* vars)
./scripts/fire.sh <token_id> [price] [fee_rate_bps] [neg_risk]

# Benchmark connection latency
cargo run -p rtt-bench -- --benchmark --samples 100

# Benchmark trigger path smoke test
cargo run -p rtt-bench --release -- --trigger-test --af auto

# Benchmark comparison command for Spec 09 (keep address family fixed)
cargo run -p rtt-bench --release -- --benchmark --mode single-shot --samples 100 --connections 2 --af auto

# End-to-end test with real credentials (costs money)
cargo test -p rtt-core -- --ignored test_clob_end_to_end_pipeline
```

For latency-sensitive changes, benchmark baselines, and manual order-path sign-off rules, use [specs/09-rtt-core-refactor.md](/Users/sam/Desktop/Projects/rtt/specs/09-rtt-core-refactor.md) as the source of truth.

## Current Limitations & Known Issues

1. **Pre-signed orders have fixed price** — Signed at startup using strategy threshold. Strategy must fire at the same price. No runtime price adaptation.

2. **Pre-signed pool uses cursor reset for refill** — When pool drops below 20%, cursor resets to 0 (reuses same bodies). Filled orders will be rejected by exchange on duplicate salt. Circuit breaker catches the failures.

3. **Executor is single-threaded** — Burst triggers processed serially. `queue_delay` grows with burst depth. No parallel order dispatch.

4. **BTreeMap price ordering relies on consistent decimal format** — String comparison works for Polymarket's format but would break with inconsistent decimal places (e.g. "0.1" vs "0.10").

5. **IPv6 intermittently fails on macOS** — Rust's socket layer sometimes can't connect via IPv6 even when system resolver works. `AddressFamily::Auto` is the safe default. IPv6 previously showed better latency (p99 ~178ms v6 vs ~410ms v4 from NYC).

6. **No TLS session resumption** — Each `warmup()` does a full TLS handshake. Session tickets/PSK not implemented.

7. **Release build trigger-to-wire is ~56µs** — Down from ~136µs in debug (2.4x improvement). Dispatch (HMAC + request build) is 2.7µs in release vs 176µs in debug (65x). The remaining ~56µs trigger-to-wire is dominated by queue_delay (~34µs) from the async-to-sync crossbeam bridge, not CPU work. C++ baseline was ~8µs — the gap is the channel bridge overhead. Profile: `opt-level=3, lto="thin", codegen-units=1`.

8. **No QUIC/HTTP3** — Cloudflare advertises h3 via alt-svc header. Stub exists but no implementation.

9. **`require_confirmation = true` is serial** — OrderGuard blocks next order until previous response received. Throughput limited to 1/(network RTT) orders/second.

## What's Next

Based on IMPLEMENTATION_LOG.md and known limitations:

- **Dynamic re-signing** — Sign orders at runtime prices, not just startup threshold. Requires moving signing to a background thread or using a pre-computed price ladder.
- **Reduce trigger-to-wire below 56µs** — The ~34µs queue_delay from the crossbeam bridge dominates. A dedicated SPSC ring buffer (e.g. `rtrb`) or direct invocation could cut this further. C++ baseline was ~8µs.
- **Pool auto-replenishment** — Actually re-sign new orders instead of cursor reset. Requires background signing thread.
- **Multi-strategy support** — Run multiple strategies on different assets simultaneously.
- **TLS session resumption** — Reduce reconnect latency with session tickets.
- **HTTP/3 experiment** — Alt-svc probe confirms support. Implement with quinn or h3 crate.
- **aws-lc-rs crypto backend** — Enable hardware-accelerated AES for rustls.

## Glossary

### Trading & Polymarket

| Term | Meaning |
|---|---|
| **CLOB** | Central Limit Order Book — Polymarket's order matching system |
| **FOK** | Fill or Kill — order must fill completely or be cancelled |
| **FAK** | Fill and Kill — fill what's available, cancel the rest |
| **GTC** | Good Till Cancelled |
| **GTD** | Good Till Date |
| **neg-risk** | Polymarket market type that uses a different exchange contract address |

### Ethereum & Signing

| Term | Meaning |
|---|---|
| **EOA** | Externally Owned Account — the private key that signs transactions and orders |
| **proxy wallet** | Gnosis Safe contract wallet — the maker/funder address on orders. Controlled by an EOA |
| **EIP-712** | Ethereum typed structured data signing standard. Used to sign orders off-chain |
| **salt** | Random nonce baked into each order — prevents replay, must be unique per filled order |
| **pre-signed pool** | Orders signed at startup with unique salts; at dispatch time only HMAC headers are recomputed |

### API & Auth

| Term | Meaning |
|---|---|
| **L2** | Polymarket's API auth layer — HMAC-SHA256 over (timestamp + method + path + body) |
| **HMAC** | Hash-based Message Authentication Code — proves the request came from the API key holder |

### Networking & Latency

| Term | Meaning |
|---|---|
| **H2** | HTTP/2 — the transport protocol used for order submission over warm connections |
| **ALPN** | Application-Layer Protocol Negotiation — TLS extension that selects H2 during handshake |
| **cf-ray** | Cloudflare request ID header, includes POP code (e.g. "abc123-EWR") |
| **POP** | Point of Presence — Cloudflare edge datacenter code (EWR = Newark, DUB = Dublin) |
| **trigger-to-wire** | Time from trigger dequeue to H2 frame submission — what our code controls |
| **warm_ttfb** | Time from frame submission to first response byte — network physics + server processing |

### Channels & Concurrency

The pipeline uses three channel types to move data between stages:

| Term | Meaning |
|---|---|
| **broadcast** | Tokio channel: one sender, many receivers. Used for WS messages → Pipeline and snapshots → subscribers |
| **mpsc** | Multi-Producer Single-Consumer — async tokio channel for passing data between async tasks (snapshots → strategy, triggers → bridge) |
| **crossbeam** | Sync bounded channel from the `crossbeam-channel` crate. Used at the async→OS-thread boundary so the execution loop can spin on `try_recv()` without a tokio runtime |

Data flows: `broadcast` → `mpsc` → `mpsc` → `crossbeam` → execution loop. Each arrow is a bridge that converts between channel types.

## Project History

The project started as a C++ prototype (milestones 1.1–7.3, 92 tests) using CMake, GoogleTest, OpenSSL, and nghttp2. The entire prototype was ported to Rust across Sessions 1–4 (R1.1–S4-7.3), then integrated in Session 5, wired with dry-run in Session 6, safety rails added in Session 7, and first live trade executed in Session 8. The C++ code was deleted after port completion (commit `6e6016f`). See `IMPLEMENTATION_LOG.md` for full session-by-session history.
