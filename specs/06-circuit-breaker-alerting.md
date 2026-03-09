# Spec 06: Circuit Breaker Alerting

## Priority: SHOULD HAVE (risky without)

## Problem

When the circuit breaker trips, the execution loop logs an error and stops processing orders. But if the operator isn't watching logs, they won't know the system has stopped trading. There's no external notification.

## Current Code

- `crates/pm-executor/src/execution.rs` (line 92-94): Logs `"Circuit breaker tripped!"` and breaks
- `crates/pm-executor/src/safety.rs` — `CircuitBreaker::trip()` and `is_tripped()`
- `crates/pm-executor/src/health.rs` — Logs tripped status every 30s but no external notification

## Solution

### Approach: Webhook-based alerting

Add a configurable webhook URL. When the circuit breaker trips, POST a JSON payload to the webhook. This works with:
- Slack incoming webhooks
- Discord webhooks
- Telegram bot API (via webhook adapter)
- PagerDuty, Opsgenie, custom endpoints
- ntfy.sh (simple push notifications)

### 1. Add alert config

In `crates/pm-executor/src/config.rs`, add to `SafetyConfig`:
```rust
#[serde(default)]
pub alert_webhook_url: Option<String>,
```

Config example:
```toml
[safety]
alert_webhook_url = "https://hooks.slack.com/services/T.../B.../xxx"
```

Also support env var override: `POLY_ALERT_WEBHOOK_URL`.

### 2. Add alert module

Create `crates/pm-executor/src/alert.rs`:
```rust
/// Fire-and-forget webhook alert. Does not block or retry.
pub async fn send_alert(webhook_url: &str, message: &str) { ... }
```

Use `reqwest` (already transitively available, or add as dependency) or a raw `hyper` POST. The function:
- POSTs `{"text": "<message>"}` (Slack-compatible format)
- Timeout: 5 seconds
- Does NOT retry — alerting should not block trading logic
- Logs success/failure but does not propagate errors

### 3. Wire alerting into the execution loop

The challenge: the execution loop runs on a dedicated OS thread, not tokio. Options:

**Option A (recommended):** Pass the webhook URL to the execution loop. When the circuit breaker trips, use the execution thread's internal tokio runtime (`rt`) to fire the webhook:
```rust
if circuit_breaker.is_tripped() {
    if let Some(url) = &alert_webhook_url {
        let msg = format!("Circuit breaker tripped: {} orders, ${:.2} committed", orders, usd);
        rt.block_on(alert::send_alert(url, &msg));
    }
    break;
}
```

**Option B:** Use a sync HTTP client (`ureq`) to avoid async. Simpler but adds a dependency.

Go with Option A — we already have `rt` in the execution loop.

### 4. Also alert from health monitor

In `health.rs`, if the circuit breaker is tripped during a health check, send an alert. This catches cases where the trip happened between health check intervals.

## Files to Modify

| File | Changes |
|------|---------|
| `crates/pm-executor/src/alert.rs` | New file: `send_alert()` function |
| `crates/pm-executor/src/config.rs` | Add `alert_webhook_url` to SafetyConfig, env var override |
| `crates/pm-executor/src/execution.rs` | Accept webhook URL, send alert on trip |
| `crates/pm-executor/src/health.rs` | Send alert if circuit breaker tripped during health check |
| `crates/pm-executor/src/main.rs` | Wire webhook URL through, add mod alert |
| `crates/pm-executor/Cargo.toml` | Add `reqwest` dependency (if not already available) |

## Tests

1. **Unit: send_alert constructs correct POST** — Mock server (or just verify the request is built correctly)
2. **Unit: alert fires on circuit breaker trip** — Trip the breaker in execution loop, verify alert function is called (use a test webhook or mock)
3. **Unit: missing webhook URL skips alerting** — No panic, no error when `alert_webhook_url` is None
4. **Unit: alert timeout doesn't block execution** — Verify the execution loop continues even if webhook is unreachable

## Acceptance Criteria

- [ ] `alert_webhook_url` configurable in `config.toml` and via `POLY_ALERT_WEBHOOK_URL` env var
- [ ] Circuit breaker trip sends JSON POST to webhook URL
- [ ] Alert includes: reason, orders fired, USD committed
- [ ] Alert does not block or crash the execution loop
- [ ] Alert is also sent from health monitor if tripped state detected
- [ ] No alert sent if webhook URL is not configured
- [ ] All existing tests pass

## Scope Boundaries

- Do NOT implement email, SMS, or Telegram directly — webhook covers all via adapters
- Do NOT retry failed alerts — fire and forget
- Do NOT add alert throttling (circuit breaker trips once, so at most one alert per run)
- Do NOT make alerting a separate service/process
