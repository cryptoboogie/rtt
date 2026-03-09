# Spec 01: WebSocket Reconnect + Resubscribe

## Priority: MUST HAVE (blocking production)

## Problem

When the WebSocket connection drops and reconnects, the client does NOT re-send the subscription message. The reconnect loop in `WsClient::run()` calls `connect_and_run()` again, which does send the subscribe message — so **this actually works correctly today**.

However, there are reliability gaps:

1. **No exponential backoff** — Fixed 2-second delay between reconnects. If Polymarket is down for minutes, we hammer them with connection attempts every 2 seconds.
2. **No reconnect metrics** — We don't know how many reconnects happened, when, or how long data was stale.
3. **No stale data detection** — If the WS silently stops sending data (server-side issue, network partition), the pipeline doesn't know. It continues running with an increasingly stale order book.
4. **Order book not cleared on reconnect** — After a reconnect, the initial book snapshot will replace the old state, but there's a window where the `OrderBookManager` has stale data from the previous connection and strategies could fire on it.

## Current Code

- `crates/pm-data/src/ws.rs` — `WsClient::run()` (line 47-82): reconnect loop with fixed `RECONNECT_DELAY = 2s`
- `crates/pm-data/src/ws.rs` — `connect_and_run()` (line 91-162): connects, subscribes, processes messages
- `crates/pm-data/src/orderbook.rs` — `OrderBookManager`: holds `Arc<RwLock<HashMap<asset_id, BookState>>>`
- `crates/pm-data/src/pipeline.rs` — `Pipeline`: orchestrates WS → OrderBookManager → broadcast

## Solution

### 1. Exponential backoff with jitter

In `WsClient::run()`, replace the fixed `RECONNECT_DELAY` with exponential backoff:
- Start at 1 second
- Double each attempt, cap at 60 seconds
- Add random jitter (0-500ms) to prevent thundering herd
- Reset backoff to 1 second on successful connection + first message received

### 2. Reconnect counter and logging

Add fields to `WsClient`:
```rust
reconnect_count: Arc<AtomicU64>,
last_message_at: Arc<AtomicU64>,  // epoch millis
```

Log reconnect count on each reconnect. Expose `reconnect_count()` and `last_message_at()` for health monitoring.

### 3. Stale data detection

In the message processing loop inside `connect_and_run()`, update `last_message_at` on every received message. Add a check: if no message received for 30 seconds (configurable), log a warning. The health monitor (spec 08) will use `last_message_at()` to detect staleness externally.

### 4. Clear order book on reconnect

At the top of `connect_and_run()`, before subscribing, signal to the pipeline that a reconnect occurred. Options:
- Add a `WsMessage::Reconnected` variant that the pipeline handles by calling `OrderBookManager::clear()`
- Or: broadcast the reconnect event so the pipeline clears state

The simplest approach: add `WsMessage::Reconnected` variant. Pipeline handles it by calling a new `OrderBookManager::clear_all()` method that empties the HashMap. The next book snapshot from the server will repopulate it.

## Files to Modify

| File | Changes |
|------|---------|
| `crates/pm-data/src/ws.rs` | Exponential backoff, reconnect counter, last_message_at tracking |
| `crates/pm-data/src/types.rs` | Add `WsMessage::Reconnected` variant |
| `crates/pm-data/src/orderbook.rs` | Add `clear_all()` method |
| `crates/pm-data/src/pipeline.rs` | Handle `WsMessage::Reconnected` by clearing order book |

## Tests

1. **Unit: exponential backoff computes correct delays** — Verify 1s, 2s, 4s, 8s, ..., capped at 60s
2. **Unit: backoff resets after successful connection** — Simulate connect → fail → backoff grows → connect succeeds → backoff resets
3. **Unit: reconnect counter increments** — Create WsClient, simulate two reconnects, verify count is 2
4. **Unit: clear_all empties OrderBookManager** — Add book state, call clear_all, verify empty
5. **Unit: pipeline clears book on Reconnected message** — Send `WsMessage::Reconnected` through pipeline, verify OrderBookManager is empty
6. **Unit: last_message_at updates on message** — Verify timestamp updates when messages are processed

## Acceptance Criteria

- [ ] Reconnect delay grows exponentially from 1s to 60s max
- [ ] Reconnect delay resets to 1s after successful reconnection
- [ ] `reconnect_count()` returns total reconnects since startup
- [ ] `last_message_at()` returns epoch millis of last WS message
- [ ] Order book is cleared on reconnect (before new subscription)
- [ ] All existing tests pass
- [ ] New tests pass

## Scope Boundaries

- Do NOT change the subscription message format
- Do NOT add WebSocket authentication (Polymarket's WS is unauthenticated)
- Do NOT implement multi-WS-connection failover
