# Low-Latency Execution Service

## Way of Working

All implementation follows this discipline:

1. **Break plans into big tasks** — each big task represents a meaningful capability milestone
2. **Break tasks into sub-tasks** — sub-tasks are atomic units of work
3. **TDD for every sub-task** — write a failing test first, then write the minimal code to pass the test(s)
4. **Once the test passes, move on** — do not gold-plate; proceed to the next sub-task immediately
5. **Do not stop until all sub-tasks are finished** — unless there is a fatal blocking issue
6. **Log every sub-task** — for each completed sub-task, append an entry to `IMPLEMENTATION_LOG.md` recording files changed, tests run, commit message, and any deviations from the plan

## Current Project Status

### Rust Implementation — Session 1 (in progress)

**Branch**: `rust-rtt-core` | **67 tests passing** | Workspace: `rtt-core`, `rtt-bench`

Modules implemented:
- `clock` — monotonic nanosecond timestamps
- `trigger` — TriggerMessage, Side, OrderType, OrderBookSnapshot, PriceLevel
- `queue` — SPSC trigger queue (crossbeam-channel)
- `request` — pre-built request template with zero-allocation patching
- `metrics` — TimestampRecord (8 checkpoints + connection_index), 7 derived metrics, StatsAggregator with percentile computation, reconnect filtering
- `connection` — H2 connection via hyper+rustls (TCP_NODELAY, ALPN h2), ConnectionPool with round-robin (returns connection index), DNS resolution with address family selection, cf-ray POP extraction
- `executor` — IngressThread, ExecutionThread (single-threaded, serial processing), MaintenanceThread, CPU pinning
- `benchmark` — three modes (single-shot, random-cadence, burst-race), CLI harness, percentile reporting, POP distribution
- `h3_stub` — alt-svc probe for HTTP/3 detection

### Known Issues
- **Timestamp instrumentation conflates write + response**: `pool.send()` blocks for the entire request+response cycle. `write_duration` / `warm_ttfb` measure full network RTT (~114ms), not actual wire time. `write_to_first_byte` is ~0ns. Needs split instrumentation to be comparable to C++ ~8us trigger-to-wire.
- **Executor is single-threaded**: burst mode triggers are processed serially, causing queue_delay to grow with burst depth. This is by design but means burst trigger_to_wire is dominated by queue wait.

# Session Plans: Polymarket Low-Latency Execution System (Rust)

## Overview

Build a single Rust binary that handles the full pipeline:
websocket market data → strategy gate → sign order → fire on warm H2 connection

Reference: The C++ executor at `~/Desktop/claude-plan-claude-impl/` proves the
architecture and has benchmark data. This Rust system replaces it.

Key external resources:
- Polymarket Rust SDK: https://github.com/Polymarket/rs-clob-client
- CLOB API docs: https://docs.polymarket.com
- Market WebSocket: wss://ws-subscriptions-clob.polymarket.com/ws/market
- User WebSocket: wss://ws-subscriptions-clob.polymarket.com/ws/user
- Trading endpoint: POST https://clob.polymarket.com/order

No staging endpoint exists. Test against prod with small orders or mocked responses.

---

## Dependency Graph

```
Session 1: Rust RTT Core ──────┐
                                ├──→ Session 4: CLOB Order Integration
Session 2: WS Data Pipeline ───┤
                                ├──→ Session 5: Integration
Session 3: Strategy Framework ──┘
```

Sessions 1, 2, 3 run in PARALLEL.
Session 4 needs Session 1 done.
Session 5 needs all done.

---

## Shared Interface Contracts

ALL sessions must use these shared types. Define them in a `common/` crate
before starting parallel work, or have Session 1 define them and Sessions 2+3
code against the same interface.

```rust
// === Trigger message (Session 1 defines, Session 2+3 produce) ===
pub struct TriggerMessage {
    pub trigger_id: u64,
    pub token_id: String,        // Polymarket asset/token ID
    pub side: Side,              // BUY or SELL
    pub price: String,           // Decimal string, e.g. "0.45"
    pub size: String,            // Decimal string
    pub order_type: OrderType,   // FOK, GTC, GTD, FAK
    pub timestamp_ns: u64,       // Monotonic nanoseconds when trigger created
}

pub enum Side { Buy, Sell }
pub enum OrderType { GTC, GTD, FOK, FAK }

// === Market data snapshot (Session 2 defines, Session 3 consumes) ===
pub struct OrderBookSnapshot {
    pub asset_id: String,
    pub best_bid: Option<PriceLevel>,
    pub best_ask: Option<PriceLevel>,
    pub timestamp_ms: u64,
    pub hash: String,
}

pub struct PriceLevel {
    pub price: String,
    pub size: String,
}

// === Timestamp record (Session 1 defines, all use for observability) ===
pub struct TimestampRecord {
    pub t_trigger_rx: u64,
    pub t_dispatch_q: u64,
    pub t_exec_start: u64,
    pub t_buf_ready: u64,
    pub t_write_begin: u64,
    pub t_write_end: u64,
    pub t_first_resp_byte: u64,
    pub t_headers_done: u64,
    pub is_reconnect: bool,
    pub cf_ray_pop: String,
    pub connection_index: usize,
}
```

---

## SESSION 1: Rust RTT Core Executor

**Branch**: `rust-rtt-core`
**Goal**: Port the C++ hot executor to Rust with equivalent or better performance.
**Status**: Core complete (67 tests). Instrumentation needs split write/response timestamps.

### Context for the agent
The C++ implementation at `~/Desktop/claude-plan-claude-impl/` is the architecture reference.
Key files: `src/connection/*`, `src/executor/*`, `src/benchmark/*`, `src/main.cpp`.
The C++ version achieves ~8us trigger-to-wire on warm connections.

### What's done
1. Cargo workspace: `rtt-core`, `rtt-bench`
2. Warm HTTP/2 connection pool (hyper + rustls, TCP_NODELAY, ALPN h2, round-robin with index tracking)
3. SPSC channel (crossbeam-channel)
4. Request template with zero-allocation patching
5. Monotonic ns timestamps, TimestampRecord with 8 checkpoints + connection_index
6. StatsAggregator with percentile computation (p50/p95/p99/p99.9), reconnect filtering
7. Benchmark CLI: single-shot, random-cadence, burst-race modes
8. IPv4/IPv6 forced path selection, cf-ray POP extraction
9. CPU pinning (core_affinity), H3 alt-svc probe stub
10. Burst contention test with connection distribution + latency assertions

### What's remaining
- **Split write/response instrumentation** — `send_request()` needs to expose the point where the H2 frame is submitted to the kernel vs when the response arrives. Current `write_duration` = full RTT (~114ms), not wire time.
- Verify trigger-to-wire <= 10us p50 after instrumentation fix

### Key Rust crates (actual)
- hyper 1.x (HTTP/2 client)
- rustls + webpki-roots (TLS)
- tokio (async runtime)
- crossbeam-channel (SPSC)
- core_affinity (CPU pinning)
- rand (random cadence mode)

### Test approach
- Unit tests for each module (timestamp, stats, queue, template)
- Integration tests hitting clob.polymarket.com (warm connection, cf-ray)
- Benchmark binary with CLI flags matching C++ version

---

## SESSION 2: WebSocket Data Pipeline

**Branch**: `ws-data-pipeline`
**Goal**: Maintain a real-time local order book from Polymarket WebSocket feeds.

### Requirements
1. Cargo crate: `pm-data`
2. Connect to `wss://ws-subscriptions-clob.polymarket.com/ws/market`
3. Subscribe to configurable list of asset_ids
4. Handle all market channel events:
   - `book` (full snapshot)
   - `price_change` (delta updates)
   - `last_trade_price`
   - `tick_size_change`
   - `best_bid_ask`
5. Maintain in-memory order book per subscribed asset
6. Validate order book hash to detect missed updates
7. Auto-reconnect on disconnect with re-subscribe
8. PING every 10 seconds for keepalive
9. Expose order book state via a thread-safe read interface
10. Optional: User WebSocket channel for trade lifecycle tracking

### Shared interface
- Produce `OrderBookSnapshot` and `PriceLevel` types (see contracts above)
- Expose a channel or callback that the strategy layer can subscribe to

### Success criteria
- `cargo test` passes
- Integration test: connect, subscribe to a real market, receive book snapshot
- Order book updates correctly on price_change events
- Reconnects cleanly after simulated disconnect
- Keepalive PING works (no timeout disconnects over 60+ seconds)

### Key Rust crates
- tokio-tungstenite (WebSocket client)
- serde / serde_json (message parsing)
- tokio (async runtime)
- dashmap or RwLock<HashMap> (concurrent order book)

### Data flow
```
WebSocket → parse JSON → update local order book → notify strategy via channel
```

---

## SESSION 3: Strategy Framework

**Branch**: `strategy-framework`
**Goal**: Build a pluggable strategy engine that consumes market data and emits triggers.

### Requirements
1. Cargo crate: `pm-strategy`
2. Define a Strategy trait:
   ```rust
   pub trait Strategy: Send + Sync {
       fn on_book_update(&mut self, snapshot: &OrderBookSnapshot) -> Option<TriggerMessage>;
       fn on_trade(&mut self, trade: &TradeEvent) -> Option<TriggerMessage>;
       fn name(&self) -> &str;
   }
   ```
3. Implement at least two example strategies:
   - `ThresholdStrategy`: fires when best_bid or best_ask crosses a configured price
   - `SpreadStrategy`: fires when spread narrows below a threshold
4. Strategy runner that:
   - Receives OrderBookSnapshot from a channel (produced by Session 2)
   - Calls the active strategy
   - If trigger returned, sends TriggerMessage to executor channel (consumed by Session 1)
5. Configuration via TOML or JSON:
   - Which strategy to use
   - Strategy-specific parameters
   - Target asset_id / token_id
   - Order parameters (side, size, order_type)
6. Backtesting mode: replay saved order book snapshots through strategy

### Shared interface
- Consumes `OrderBookSnapshot` (from Session 2)
- Produces `TriggerMessage` (consumed by Session 1)

### Success criteria
- `cargo test` passes
- ThresholdStrategy correctly fires trigger at configured price
- SpreadStrategy correctly fires trigger at configured spread
- Strategy runner processes a sequence of mock snapshots
- Config loads from TOML file

### Key Rust crates
- tokio (async channels)
- serde / toml (config)
- chrono (timestamps for backtesting)

---

## SESSION 4: CLOB Order Integration

**Branch**: `clob-order-integration`
**Depends on**: Session 1 (rust-rtt-core)

**Goal**: Replace the generic GET / request with actual Polymarket order placement.

### Context
The Polymarket order flow requires:
1. EIP-712 signing of order struct (secp256k1 ECDSA)
2. HMAC-SHA256 L2 authentication headers
3. POST to /order with signed JSON payload

### Requirements
1. Extend rtt-core with CLOB-specific order building
2. EIP-712 order signing:
   - Domain: { name: "Polymarket CTF Exchange", version: "1", chainId: 137,
     verifyingContract: <exchange_address> }
   - Order struct: salt, maker, signer, taker, tokenId, makerAmount, takerAmount,
     expiration, nonce, feeRateBps, side, signatureType
   - Sign with ethers-rs or alloy (secp256k1)
3. HMAC-SHA256 L2 auth header computation:
   - Message = timestamp + method + path + body
   - Key = base64-decoded API secret
   - Result = URL-safe base64 HMAC-SHA256
4. Build complete POST /order request:
   - Headers: POLY_API_KEY, POLY_ADDRESS, POLY_SIGNATURE, POLY_PASSPHRASE, POLY_TIMESTAMP
   - Body: { order: {..., signature: "0x..."}, owner: "<uuid>", orderType: "FOK" }
5. Pre-sign optimization: sign orders for known parameters before trigger,
   only patch salt/timestamp at trigger time
6. Parse order response: { success, orderID, status, transactionsHashes, tradeIDs }

### Existing Rust SDK reference
Check https://github.com/Polymarket/rs-clob-client for reference implementation.
You may use it directly or extract the signing logic.

### Success criteria
- Can construct a valid signed order payload
- HMAC auth headers pass server validation
- POST /order returns a valid response (test with a tiny real order or verify
  the request format matches what the TypeScript SDK produces)
- Pre-signed orders work correctly
- Trigger-to-wire latency stays under 50us with signing included

### Key Rust crates
- alloy or ethers (EIP-712 signing, secp256k1)
- hmac + sha2 (HMAC-SHA256)
- base64 (secret decoding)
- serde_json (payload serialization)

### Critical detail for hot path
The EIP-712 signature is the most expensive operation (~100-500us for secp256k1).
Options to minimize impact:
1. Pre-sign batch of orders with different salts before trigger
2. Sign in parallel on a dedicated thread, hand signed payload to executor
3. Accept the ~500us cost (still very fast)

---

## SESSION 5: Integration

**Branch**: `integration`
**Depends on**: All other sessions complete

**Goal**: Merge all crates into a single workspace binary.

### Requirements
1. Cargo workspace: `pm-executor` (binary) depending on all crates
2. Single binary startup flow:
   a. Load config (credentials, target markets, strategy, connection params)
   b. Establish warm HTTP/2 connections to clob.polymarket.com
   c. Connect WebSocket to market data feed
   d. Start strategy runner
   e. Strategy emits trigger → executor fires signed order on warm connection
3. Graceful shutdown (Ctrl+C)
4. Logging (tracing crate) with levels: hot path = off, everything else = info
5. Config file (TOML): credentials, markets, strategy, pool size, address family
6. Health monitoring: connection pool health, WebSocket state, POP verification

### Success criteria
- Single `cargo run` starts the full pipeline
- WebSocket receives market data
- Strategy processes updates
- When strategy fires, order is signed and sent on warm connection
- End-to-end latency from WebSocket event to order-on-wire is measurable
- Clean shutdown on SIGINT

---

## Running Sessions

For each parallel session (1, 2, 3), open a terminal:

```bash
cd ~/Desktop
mkdir pm-executor && cd pm-executor  # or wherever
git init && git checkout -b <branch-name>

claude --dangerously-skip-permissions
```

Then paste the relevant session plan above as the first message.

Sessions 4 and 5 run after their dependencies are complete.

 ### Note:
 For polymarket api related code you can go to:
 
 https://docs.polymarket.com/market-data/overview

 https://github.com/Polymarket/rs-clob-client

 The rust client is only for clob, which is the trade-execution orderbook. There are other categories of polymarket API's - for the sessions described above we may not need any other external libraries for data, websockets etc, but if you do need it, look for the official polymarket rust client for that category of API.