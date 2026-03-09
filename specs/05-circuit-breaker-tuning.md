# Spec 05: Circuit Breaker Tuning for Initial Production

## Priority: MUST HAVE (blocking production)

## Problem

Default circuit breaker limits are too generous for a first production run:
- `max_orders = 10` (default) — should be 5
- `max_usd_exposure = 50.0` (default) — should be 10.0

These defaults are set in `crates/pm-executor/src/config.rs` and `config.toml`. We want conservative defaults that fail closed, limiting blast radius for the first live run.

## Current Code

- `crates/pm-executor/src/config.rs` (lines 92-103): Default functions for safety config
- `config.toml` — Contains `[safety]` section with current values
- `crates/pm-executor/src/safety.rs` — CircuitBreaker implementation (no changes needed)

## Solution

### 1. Update defaults in `config.rs`

```rust
fn default_max_orders() -> u64 { 5 }
fn default_max_usd_exposure() -> f64 { 10.0 }
fn default_max_triggers_per_second() -> u64 { 2 }
```

### 2. Update `config.toml`

```toml
[safety]
max_orders = 5
max_usd_exposure = 10.0
max_triggers_per_second = 2
require_confirmation = true
```

### 3. Add startup log that highlights safety limits

In `main.rs`, the safety config is already logged (line 147-153). Enhance to use `warn!` level so it stands out:

```rust
tracing::warn!(
    max_orders = config.safety.max_orders,
    max_usd = config.safety.max_usd_exposure,
    "Safety limits active — circuit breaker will halt after these limits"
);
```

## Files to Modify

| File | Changes |
|------|---------|
| `crates/pm-executor/src/config.rs` | Update default values |
| `config.toml` | Update safety section |
| `crates/pm-executor/src/main.rs` | Change safety log to warn level |

## Tests

1. **Update existing test**: `safety_defaults_applied_without_section` — assert `max_orders == 5`, `max_usd_exposure == 10.0`, `max_triggers_per_second == 2`
2. **Existing test**: `circuit_breaker_fires_up_to_max_orders_then_trips` — still passes (uses explicit values, not defaults)

## Acceptance Criteria

- [ ] Default `max_orders` is 5
- [ ] Default `max_usd_exposure` is 10.0
- [ ] Default `max_triggers_per_second` is 2
- [ ] `config.toml` reflects new defaults
- [ ] Safety limits logged at `warn` level at startup
- [ ] All existing tests pass (update assertions for new defaults)

## Scope Boundaries

- Do NOT change CircuitBreaker implementation
- Do NOT add runtime config reload — restart is fine for changing limits
- This is a config-only change, minimal code
