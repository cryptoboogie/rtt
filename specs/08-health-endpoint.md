# Spec 08: Health Endpoint / Heartbeat

## Priority: SHOULD HAVE (risky without)

## Problem

There's no way for external systems (monitoring, load balancer, cron watchdog) to check if the process is alive and receiving data. The health monitor logs internally but doesn't expose state externally. If the process hangs (deadlock, infinite loop, OOM), nothing notices.

## Current Code

- `crates/pm-executor/src/health.rs` — `run_health_monitor()`: Logs stats every 30s to tracing, no external interface
- No HTTP server in pm-executor

## Solution

### Approach: Minimal HTTP health endpoint

Add a tiny HTTP server on a configurable port that serves two endpoints:
- `GET /health` — Returns 200 if alive, 503 if circuit breaker tripped or data is stale
- `GET /status` — Returns JSON with detailed state

### 1. Add health server config

In `crates/pm-executor/src/config.rs`, add to a new or existing section:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,
    #[serde(default = "default_health_port")]
    pub port: u16,
}
```

Defaults: `enabled = true`, `port = 9090`.

Config:
```toml
[health]
enabled = true
port = 9090
```

### 2. Create health server

Create `crates/pm-executor/src/health_server.rs`:

Uses raw `hyper` (already a dependency).

```rust
pub async fn run_health_server(
    port: u16,
    circuit_breaker: CircuitBreaker,
    last_message_at: Arc<AtomicU64>,  // from WsClient (spec 01)
    shutdown: watch::Receiver<bool>,
)
```

#### `GET /health`
Returns:
- `200 OK` with `{"status": "ok"}` if:
  - Circuit breaker is NOT tripped
  - Last WS message was within 60 seconds (configurable staleness threshold)
- `503 Service Unavailable` with `{"status": "unhealthy", "reason": "..."}` if:
  - Circuit breaker is tripped
  - No WS message for > 60 seconds

#### `GET /status`
Returns `200 OK` with:
```json
{
  "status": "ok",
  "uptime_seconds": 3600,
  "circuit_breaker": {
    "tripped": false,
    "orders_fired": 2,
    "max_orders": 5,
    "usd_committed": 4.50,
    "max_usd": 10.00
  },
  "websocket": {
    "last_message_seconds_ago": 1.2,
    "reconnect_count": 0
  }
}
```

### 3. Wire into main.rs

Spawn the health server as a tokio task alongside the other components. Pass it the circuit breaker clone and WS client metrics.

**Dependency on Spec 01:** The `last_message_at` and `reconnect_count` fields come from the WsClient changes in Spec 01. If Spec 01 isn't done yet, the health endpoint can still work — just omit the websocket section and only report circuit breaker state.

### 4. Keep the existing log-based health monitor

The health server and the periodic log health monitor serve different purposes:
- Log monitor: visible in log aggregation, periodic summary
- HTTP endpoint: for external probes, load balancers, watchdogs

Both should coexist.

## Files to Modify

| File | Changes |
|------|---------|
| `crates/pm-executor/src/health_server.rs` | New file: HTTP health endpoint |
| `crates/pm-executor/src/config.rs` | Add `HealthConfig`, add `health` field to `ExecutorConfig` |
| `crates/pm-executor/src/main.rs` | Spawn health server, wire dependencies |

## Tests

1. **Unit: /health returns 200 when healthy** — Circuit breaker not tripped, recent data → 200
2. **Unit: /health returns 503 when tripped** — Trip circuit breaker → 503
3. **Unit: /status returns JSON with all fields** — Verify response structure
4. **Unit: health server shuts down on signal** — Send shutdown, verify server stops
5. **Integration: HTTP request to health endpoint** — Bind to random port, make actual HTTP request, verify response

## Acceptance Criteria

- [ ] `GET /health` returns 200 (healthy) or 503 (unhealthy)
- [ ] `GET /status` returns JSON with circuit breaker and uptime info
- [ ] Health port configurable via `[health] port = 9090`
- [ ] Health server can be disabled via `[health] enabled = false`
- [ ] Health server shuts down cleanly with the rest of the pipeline
- [ ] All existing tests pass

## Scope Boundaries

- Do NOT add Prometheus metrics export (future enhancement)
- Do NOT add authentication to the health endpoint (internal network only)
- Do NOT serve any other endpoints (no API, no dashboard)
- Do NOT add WebSocket status if Spec 01 isn't done yet — degrade gracefully
- Prefer raw `hyper`; do not add a framework layer for this endpoint
