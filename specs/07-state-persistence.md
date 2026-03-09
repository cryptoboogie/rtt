# Spec 07: Graceful Shutdown State Persistence

## Priority: SHOULD HAVE (risky without)

## Problem

When the process restarts (crash, deploy, manual restart), the circuit breaker resets to zero. The system has no memory of how many orders were fired or how much USD was committed in previous runs. This means:

- A crash + restart effectively gives a fresh budget (dangerous if the crash was caused by rapid firing)
- No audit trail of cumulative exposure across restarts
- Circuit breaker limits are per-run, not per-day or per-session

## Current Code

- `crates/pm-executor/src/safety.rs` — `CircuitBreaker` uses in-memory `AtomicU64` counters
- `crates/pm-executor/src/main.rs` (line 226-248) — Shutdown sequence: sends shutdown signals, waits 5s, joins threads
- `crates/pm-executor/src/execution.rs` (lines 206-212) — Logs final stats on loop exit but doesn't persist them

## Solution

### Approach: Simple JSON state file

Write a state file on shutdown, read it on startup. No database, no external service.

### 1. Define state file format

```json
{
  "orders_fired": 3,
  "usd_committed_cents": 1500,
  "last_shutdown": "2026-03-07T12:34:56Z",
  "tripped": false
}
```

File location: `state.json` in the working directory (next to `config.toml`), or configurable via `[execution] state_file = "state.json"`.

### 2. Add state persistence module

Create `crates/pm-executor/src/state.rs`:

```rust
#[derive(Serialize, Deserialize, Default)]
pub struct ExecutorState {
    pub orders_fired: u64,
    pub usd_committed_cents: u64,
    pub last_shutdown: String,
    pub tripped: bool,
}

impl ExecutorState {
    pub fn load(path: &Path) -> Self { /* read file, return Default if missing */ }
    pub fn save(&self, path: &Path) -> Result<(), io::Error> { /* write JSON */ }
}
```

### 3. Load state on startup

In `main.rs`, after building the circuit breaker:
- Load `ExecutorState` from file
- If previous run was tripped, log a WARNING and require explicit `--force` flag to continue (or just log and continue with fresh limits — operator chose to restart)
- Initialize circuit breaker counters with previous values using a new method:

```rust
impl CircuitBreaker {
    pub fn with_initial_counts(max_orders: u64, max_usd: f64, initial_orders: u64, initial_usd_cents: u64) -> Self
}
```

### 4. Save state on shutdown

In the shutdown sequence in `main.rs`, after the execution loop has stopped:
- Read final stats from circuit breaker
- Write `ExecutorState` to file
- Log that state was persisted

### 5. Save state periodically

In `health.rs`, every health check (30s), also write state to file. This protects against ungraceful shutdowns (kill -9, crash).

## Files to Modify

| File | Changes |
|------|---------|
| `crates/pm-executor/src/state.rs` | New file: ExecutorState load/save |
| `crates/pm-executor/src/safety.rs` | Add `with_initial_counts()` constructor to CircuitBreaker |
| `crates/pm-executor/src/config.rs` | Add optional `state_file` to ExecutionConfig |
| `crates/pm-executor/src/main.rs` | Load state on startup, save on shutdown, wire state file path |
| `crates/pm-executor/src/health.rs` | Periodic state save |

## Tests

1. **Unit: save and load roundtrip** — Write state, read it back, verify all fields match
2. **Unit: load missing file returns default** — No file exists, get zeroed state
3. **Unit: load corrupt file returns default** — Garbage in file, get zeroed state (don't crash)
4. **Unit: CircuitBreaker with_initial_counts starts at correct values** — Create with 3 orders and $15, verify stats() returns those values
5. **Unit: CircuitBreaker with_initial_counts respects limits** — Create with initial 4/5 orders, verify next check_and_record trips at 5

## Acceptance Criteria

- [ ] State file written on graceful shutdown
- [ ] State file written periodically (every health check)
- [ ] State file read on startup, counters restored
- [ ] Missing or corrupt state file doesn't prevent startup
- [ ] Previous tripped state logged as warning on startup
- [ ] All existing tests pass

## Scope Boundaries

- Do NOT use a database — plain JSON file is sufficient
- Do NOT implement state file locking (single process assumption)
- Do NOT add state migration/versioning — simple enough to be forward-compatible
- Do NOT persist pre-signed pool state (re-sign on startup is fine)
