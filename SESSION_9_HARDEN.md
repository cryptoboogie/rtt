# Session 9: Harden for Unattended Running

**Branch**: `add-hardening`
**Depends on**: Session 8 (first live trade confirmed working)
**Goal**: Make the system resilient enough to run unattended for hours/days without human intervention.

---

## Context

After Session 8, we've confirmed a single live trade works. Now we need the system to be robust enough to:
- Survive network blips (WS disconnects, H2 connection drops)
- Log everything for post-mortem analysis
- Alert on critical failures
- Restart cleanly after crashes

## Deployment Notes (from Session 8 testing)

**CLOB matching engine**: AWS eu-west-2 (London). Cloudflare fronts all traffic.

**Recommended hosting**: AWS eu-west-1 (Ireland) — 1.2ms ping to CLOB, DUB Cloudflare POP, ~$12-15/mo for t3/t4g.small. This is where serious PM bots run.
- Try Hetzner, couple of other options - search net, there are other providers

**Alternative regions** (not blocked as of March 2026):
- **Switzerland** — low latency to London, crypto-friendly jurisdiction
- **Austria** — similar latency profile to Switzerland
- **Germany** — Frankfurt is well-connected to London (~5-8ms)
- **Ireland** remains the best for raw latency (same AWS backbone to eu-west-2)

**Blocked regions**: US, UK, Netherlands. Polymarket geoblocks CLOB order placement from these. Market data (WebSocket) may still work from blocked regions.

See `DEPLOY_IRELAND.md` for full EC2 setup instructions.

---

## Sub-Tasks

### 9.1 — Persistent trade logging

**File**: `crates/pm-executor/src/trade_log.rs` (new file)

Every order fired must be persisted to disk. Use append-only JSONL (one JSON object per line):

```rust
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use serde::Serialize;

#[derive(Serialize)]
pub struct TradeLogEntry {
    pub timestamp_utc: String,      // ISO 8601
    pub trigger_id: u64,
    pub token_id: String,
    pub side: String,
    pub price: String,
    pub size: String,
    pub order_type: String,
    // Execution metrics
    pub trigger_to_wire_ns: u64,
    pub write_duration_ns: u64,
    pub warm_ttfb_ns: u64,
    pub connection_index: usize,
    pub cf_ray_pop: String,
    // Response
    pub success: Option<bool>,
    pub order_id: Option<String>,
    pub error_msg: Option<String>,
    pub http_status: Option<u16>,
    // Safety state
    pub orders_fired_total: u64,
    pub usd_committed_total: f64,
}

pub struct TradeLogger {
    file: std::fs::File,
}

impl TradeLogger {
    pub fn new(path: &Path) -> std::io::Result<Self> { /* open append */ }
    pub fn log(&mut self, entry: &TradeLogEntry) -> std::io::Result<()> { /* write + flush */ }
}
```

**IMPORTANT**: Always `flush()` after each write. If the process crashes, we need the last trade logged.

**Config addition**:
```toml
[logging]
level = "info"
trade_log_path = "trades.jsonl"  # NEW
```

**Tests**: Write entries, read back, verify JSON parses correctly.

### 9.2 — TimestampRecord persistent logging

**File**: `crates/pm-executor/src/trade_log.rs` (extend)

Also log raw TimestampRecords to a separate file for latency analysis:

```toml
[logging]
metrics_log_path = "metrics.jsonl"  # NEW
```

Each line: `{"t_trigger_rx":12345,...,"trigger_to_wire":890,...}`

This gives you a dataset for latency analysis (histogram, p50/p95/p99 over time, POP distribution, etc.)

**Test**: Write and read back metrics entries.

### 9.3 — Connection pool watchdog

**File**: `crates/pm-executor/src/execution.rs` or new `crates/pm-executor/src/watchdog.rs`

The `rtt_core::executor::MaintenanceThread` already does health checks and reconnects for the benchmark pool. Wire this into the pm-executor:

```rust
use rtt_core::executor::MaintenanceThread;

// In main.rs, after creating the connection pool:
let mut maintenance = MaintenanceThread::new();
maintenance.start(conn_pool.clone(), Duration::from_secs(10));

// In shutdown:
maintenance.stop();
```

Also add: if the execution loop encounters a connection error (send_start fails), log it, skip the trigger, and let the maintenance thread handle reconnection. Don't crash.

**Test**: Existing MaintenanceThread tests cover this. Add a test that verifies the execution loop continues after a simulated connection error.

### 9.4 — WebSocket reconnection resilience

**File**: `crates/pm-data/src/ws.rs`

The WsClient already has reconnection logic. Verify it handles:
- Clean disconnect (server closes connection)
- Dirty disconnect (TCP reset, timeout)
- Re-subscription after reconnect (must re-send subscribe messages)

Add a counter for reconnections and expose it for health monitoring:
```rust
pub fn reconnect_count(&self) -> u64 { ... }
```

**Test**: This should already be tested in pm-data. If not, add a test that simulates disconnect and verifies re-subscription.

### 9.5 — Graceful degradation on pre-signed pool exhaustion

**File**: `crates/pm-executor/src/execution.rs`

When the pre-signed pool is exhausted:
1. Log a warning
2. **Re-sign a new batch** (requires passing the signer into the execution context)
3. Continue operating

This is critical for unattended running. The current code just stops when the pool runs out.

```rust
if presigned.consumed() >= presigned.len() {
    tracing::warn!("Pre-signed pool exhausted. Re-signing {} orders...", config.presign_count);
    let new_payloads = rt.block_on(async {
        presign_batch(
            &signer, &trigger_template, maker, signer_addr,
            fee_rate_bps, is_neg_risk, owner, config.presign_count
        ).await
    });
    match new_payloads {
        Ok(payloads) => {
            presigned = PreSignedOrderPool::new(payloads).expect("Failed to rebuild pool");
            tracing::info!("Pre-signed pool refilled with {} orders", presigned.len());
        }
        Err(e) => {
            tracing::error!("Failed to re-sign orders: {}. Stopping execution.", e);
            circuit_breaker.trip();
            break;
        }
    }
}
```

**Note**: Re-signing takes ~50-500ms (100 secp256k1 signatures). This blocks the execution thread, which is fine — we just fired our last pre-signed order and are waiting for the response anyway.

**Test**: Pool of 3, fire 3, verify refill happens and 4th trigger succeeds.

### 9.6 — Alerting on critical events

**File**: `crates/pm-executor/src/alerts.rs` (new file)

Simple webhook alerting. Send a POST to a configurable URL on:
- Circuit breaker tripped
- Process starting / stopping
- Connection pool fully down (all connections unhealthy)
- Pre-signed pool exhaustion (before refill)
- Any order rejected with an unexpected error

```rust
pub struct AlertSender {
    webhook_url: Option<String>,
    client: reqwest::Client, // or just use the existing H2 pool
}

impl AlertSender {
    pub async fn send(&self, level: &str, message: &str) {
        if let Some(url) = &self.webhook_url {
            // POST JSON: {"level": "critical", "message": "Circuit breaker tripped", "timestamp": "..."}
            // Fire-and-forget, don't block on response
            let _ = self.client.post(url)
                .json(&serde_json::json!({
                    "level": level,
                    "message": message,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                    "source": "pm-executor"
                }))
                .send()
                .await;
        }
    }
}
```

**Config**:
```toml
[alerting]
webhook_url = ""  # Set to Discord/Slack/Telegram webhook URL
```

If `webhook_url` is empty, alerts just go to the tracing log (no external notification).

**Test**: Unit test with a mock HTTP server (or just verify the JSON format is correct).

### 9.7 — Startup validation checks

**File**: `crates/pm-executor/src/main.rs`

Add pre-flight checks at startup before entering the main loop:

```rust
async fn preflight_checks(config: &ExecutorConfig) -> Result<(), Box<dyn std::error::Error>> {
    // 1. Credentials present (if not dry_run)
    if !config.execution.dry_run {
        if config.credentials.api_key.is_empty() {
            return Err("POLY_API_KEY not set".into());
        }
        if config.credentials.private_key.is_empty() {
            return Err("POLY_PRIVATE_KEY not set".into());
        }
        // ... check all required credentials
    }

    // 2. Test connection to clob.polymarket.com
    // Quick HTTP GET / to verify connectivity
    tracing::info!("Preflight: testing connection to clob.polymarket.com...");
    let mut test_pool = ConnectionPool::new("clob.polymarket.com", 443, 1, AddressFamily::Auto);
    let warm = test_pool.warmup().await?;
    tracing::info!("Preflight: connection OK ({} warm)", warm);

    // 3. Verify wallet has balance (optional — would need a Polygon RPC call)
    // Skipping for now — the order will simply fail if no balance

    // 4. Verify the target market exists (optional — GET /markets/{id})

    Ok(())
}
```

**Test**: Preflight fails on empty credentials, succeeds with valid config.

### 9.8 — Process wrapper script

**File**: `scripts/run.sh` (new file)

```bash
#!/usr/bin/env bash
set -euo pipefail

# Load environment variables from .env file if it exists
if [ -f .env ]; then
    set -a
    source .env
    set +a
fi

BINARY="./target/release/pm-executor"
CONFIG="${1:-config.toml}"
LOG_DIR="logs"
mkdir -p "$LOG_DIR"

echo "Starting pm-executor with config: $CONFIG"
echo "Logs: $LOG_DIR/"

# Build release if needed
if [ ! -f "$BINARY" ] || [ "$BINARY" -ot "crates/pm-executor/src/main.rs" ]; then
    echo "Building release..."
    cargo build --release -p pm-executor
fi

# Run with restart on crash
while true; do
    TIMESTAMP=$(date +%Y%m%d_%H%M%S)
    echo "[$TIMESTAMP] Starting pm-executor..."
    
    "$BINARY" "$CONFIG" 2>&1 | tee "$LOG_DIR/executor_$TIMESTAMP.log"
    EXIT_CODE=$?
    
    if [ $EXIT_CODE -eq 0 ]; then
        echo "pm-executor exited cleanly."
        break
    fi
    
    echo "pm-executor crashed with exit code $EXIT_CODE. Restarting in 5 seconds..."
    sleep 5
done
```

Also create a `.env.example`:
```
POLY_API_KEY=
POLY_API_SECRET=
POLY_PASSPHRASE=
POLY_PRIVATE_KEY=0x
POLY_MAKER_ADDRESS=0x
POLY_SIGNER_ADDRESS=0x
```

### 9.9 — Release build configuration

**File**: `Cargo.toml` (workspace root)

Add release profile optimizations:

```toml
[profile.release]
opt-level = 3
lto = "thin"       # Link-time optimization
codegen-units = 1  # Better optimization at cost of compile time
strip = true       # Strip debug symbols from binary
```

This should give a significant performance improvement over debug builds (5-10x on hot path).

**Test**: `cargo build --release -p pm-executor` succeeds. Run benchmark tests in release mode:
```bash
cargo test --release -p rtt-core benchmark -- --nocapture
```

---

## Config File (Final State After All Sessions)

After all sessions, `config.toml` should look like:

```toml
[credentials]
# Set via environment variables (see .env.example)

[connection]
pool_size = 2
address_family = "auto"

[websocket]
asset_ids = ["<token_id>"]
ws_channel_capacity = 1024
snapshot_channel_capacity = 256

[strategy]
strategy = "threshold"
token_id = "<token_id>"
side = "Buy"
size = "5"
order_type = "FOK"

[strategy.params]
threshold = 0.45

[execution]
presign_count = 100
is_neg_risk = false
fee_rate_bps = 0
dry_run = true

[safety]
max_orders = 100
max_usd_exposure = 500.0
max_triggers_per_second = 2
require_confirmation = true

[logging]
level = "info"
trade_log_path = "trades.jsonl"
metrics_log_path = "metrics.jsonl"

[alerting]
webhook_url = ""
```

## Success Criteria

- [ ] `cargo test --workspace` — all tests pass
- [ ] Trade log written to disk, survives process restart, entries parse as valid JSON
- [ ] Metrics log captures all TimestampRecords
- [ ] Connection pool auto-recovers from connection drops (MaintenanceThread)
- [ ] WebSocket reconnects after disconnect, re-subscribes to markets
- [ ] Pre-signed pool auto-refills when exhausted
- [ ] Alerts fire on critical events (if webhook_url configured)
- [ ] Startup preflight checks catch misconfigurations
- [ ] `scripts/run.sh` restarts on crash, logs to files
- [ ] Release build produces optimized binary
- [ ] System runs for 10+ minutes unattended without crashing (dry_run=true)
- [ ] System runs for 10+ minutes unattended without crashing (dry_run=false, with safety limits)
