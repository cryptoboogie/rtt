# Session 8: First Live Trade

**Branch**: `first-live-trade`
**Depends on**: Session 6 (wiring) + Session 7 (safety rails)
**Goal**: Execute a single real trade on Polymarket with minimum possible risk, verify the complete round-trip.

---

## Context

After Sessions 6-7:
- Pipeline is wired end-to-end
- Dry-run mode works (verified by watching logs)
- Safety rails are in place (max 10 orders, $50 limit, 1/sec rate limit)

This session is about **validation**, not new features. We're verifying every link in the chain with real money.

---

## Pre-Requisites (Manual — Human Must Do These)

Before the agent starts any work, the human operator must:

1. **Fund a Polymarket account** with a small amount (~$20 USDC on Polygon)
2. **Generate API credentials** at https://polymarket.com (API key, secret, passphrase)
3. **Export the private key** of the funded wallet
4. **Set environment variables**:
   ```bash
   export POLY_API_KEY="your-api-key"
   export POLY_API_SECRET="your-base64-api-secret"
   export POLY_PASSPHRASE="your-passphrase"
   export POLY_PRIVATE_KEY="0xyour-private-key"
   export POLY_MAKER_ADDRESS="0xyour-wallet-address"
   export POLY_SIGNER_ADDRESS="0xyour-wallet-address"  # same as maker for EOA
   ```
5. **Choose a target market** — Pick an active, liquid market. Ideal properties:
   - Currently trading near 0.50 (maximum liquidity on both sides)
   - Tight spread (best_bid and best_ask within 0.02 of each other)
   - Not about to resolve (gives time to test)
   - Note the token_id (the long number) for the outcome you want to trade

---

## Sub-Tasks

### 8.1 — Verify credentials end-to-end (no order)

**File**: `crates/rtt-core/src/clob_executor.rs` — use the existing `test_clob_end_to_end_pipeline` test

Run the existing ignored e2e test with real credentials:
```bash
cargo test -p rtt-core test_clob_end_to_end_pipeline -- --ignored --nocapture
```

This test:
1. Loads credentials from env
2. Warms a connection pool
3. Signs and sends a real order (to a test token_id)
4. Prints the response

**Expected**: The server responds (might reject for business rules like "market not found" or "min order size"). The important thing is:
- HTTP 200 or 4xx (not 401/403 — that means auth is broken)
- The POLY_SIGNATURE header is accepted (no "invalid signature" error)

If auth fails, debug before proceeding. Common issues:
- `POLY_API_SECRET` must be the base64url-encoded secret (not raw)
- `POLY_MAKER_ADDRESS` must be checksummed (mixed case)
- Timestamp must be within 60s of server time

### 8.2 — Configure for a real market

**File**: `config.toml`

Update with real values:
```toml
[credentials]
# Leave empty — env vars will override

[connection]
pool_size = 1              # Single connection for first test
address_family = "auto"

[websocket]
asset_ids = ["<REAL_TOKEN_ID>"]   # The token_id from the chosen market
ws_channel_capacity = 1024
snapshot_channel_capacity = 256

[strategy]
strategy = "threshold"
token_id = "<REAL_TOKEN_ID>"      # Same as above
side = "Buy"
size = "5"                         # Minimum viable size ($5)
order_type = "FOK"                 # Fill-or-kill: either fills immediately or cancels

[strategy.params]
threshold = <CHOOSE_CAREFULLY>     # Set this to current best_ask or slightly below
                                   # e.g., if best_ask is 0.52, set threshold = 0.52
                                   # This means: "buy when ask drops to 0.52 or below"

[execution]
presign_count = 5                  # Only 5 pre-signed orders for first test
is_neg_risk = false                # true if the market is in the "neg risk" category
fee_rate_bps = 0
dry_run = false                    # THE SWITCH

[safety]
max_orders = 1                     # Only allow ONE order
max_usd_exposure = 10.0            # Max $10
max_triggers_per_second = 1
require_confirmation = true

[logging]
level = "info"
```

**CRITICAL SAFETY**: `max_orders = 1` and `max_usd_exposure = 10.0`. The circuit breaker will trip after the first order.

### 8.3 — Dry-run validation

Before going live, run with `dry_run = true` and real WebSocket data:

```bash
cargo run --release -p pm-executor -- config.toml
```

Watch for:
- [ ] WebSocket connects: `"WS connected"` or similar
- [ ] Book snapshots arrive: strategy processes them
- [ ] If threshold is set correctly, strategy fires: `"[DRY RUN] Would fire order"`
- [ ] Trigger details look correct (token_id, side, price, size)
- [ ] Ctrl+C shuts down cleanly

If the strategy doesn't fire within a few minutes, adjust the threshold to match the current market price.

### 8.4 — Live execution

1. Set `dry_run = false` in config.toml
2. Run:
   ```bash
   RUST_LOG=info cargo run --release -p pm-executor -- config.toml
   ```
3. Watch the logs for:
   - [ ] Connection pool warms up
   - [ ] Pre-signed orders created (5 orders)
   - [ ] WebSocket connects and receives data
   - [ ] Strategy fires a trigger
   - [ ] `"Order dispatched"` log with TimestampRecord metrics
   - [ ] Circuit breaker trips after 1 order: `"Circuit breaker tripped!"`
   - [ ] System stops cleanly

4. Verify on Polymarket:
   - [ ] Check your positions — did the order fill?
   - [ ] If FOK and not enough liquidity at that price, order may have been rejected (this is fine — it means the system works, the market just didn't have matching liquidity)

### 8.5 — Parse and log order response

**File**: `crates/pm-executor/src/execution.rs`

After `process_one_clob` returns, we need to see the server response. Currently `process_one_clob` only returns a `TimestampRecord` — it doesn't expose the response body.

**Option A** (recommended): Add response logging inside `process_one_clob` or return the response alongside the TimestampRecord.

Modify `process_one_clob` in `crates/rtt-core/src/clob_executor.rs` to also return the response body:

```rust
pub fn process_one_clob(
    // ... existing params ...
) -> (TimestampRecord, Option<Vec<u8>>) {
    // ... existing logic ...
    // After receiving response, capture body bytes
    // Return (rec, Some(body_bytes)) or (rec, None) on error
}
```

Then in the execution loop, parse and log:
```rust
let (rec, maybe_body) = process_one_clob(&pool, &mut presigned, &creds, &trigger, &rt);
if let Some(body) = maybe_body {
    match parse_order_response(&body) {
        Ok(resp) => {
            tracing::info!(
                success = resp.success,
                order_id = %resp.order_id,
                status = %resp.status,
                "Order response"
            );
            if !resp.success {
                tracing::error!(error = ?resp.error_msg, "Order rejected by server");
            }
        }
        Err(e) => {
            tracing::error!(
                body = %String::from_utf8_lossy(&body),
                error = %e,
                "Failed to parse order response"
            );
        }
    }
}
```

**Test**: Mock response body parsing in unit test.

### 8.6 — Document the first trade

After the first successful (or attempted) trade, log the results:

Update `IMPLEMENTATION_LOG.md` with:
- Timestamp of first trade attempt
- Market and token_id used
- Order parameters (side, price, size, order_type)
- Server response (success/failure, order_id, error message if any)
- TimestampRecord metrics (trigger_to_wire, warm_ttfb, etc.)
- POP (Cloudflare location)
- Any issues encountered

---

## Troubleshooting Guide

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| `"invalid signature"` | EIP-712 signing mismatch | Check chain_id=137, exchange address, order field order |
| `"unauthorized"` or 401 | HMAC auth header wrong | Check POLY_API_SECRET is base64url-encoded, timestamp within 60s |
| `"min order size"` | Order too small | Polymarket min is typically $1-5, check current min |
| `"market not found"` | Wrong token_id | Verify token_id matches an active market |
| `"insufficient balance"` | Wallet not funded | Fund with USDC on Polygon |
| Strategy never fires | Threshold too aggressive | Set threshold = current best_ask (for Buy) |
| WS never connects | Network issue | Check firewall, try `wscat -c wss://ws-subscriptions-clob.polymarket.com/ws/market` |
| Connection pool warmup fails | TLS/DNS issue | Check network, try `curl https://clob.polymarket.com/` |

## Success Criteria

- [ ] `test_clob_end_to_end_pipeline` passes with real credentials
- [ ] Dry-run shows correct triggers for the target market
- [ ] Live run sends exactly 1 order (circuit breaker trips)
- [ ] Server response is logged and parsed
- [ ] Order either fills (position visible on Polymarket) or is cleanly rejected with a business-logic reason
- [ ] TimestampRecord metrics are plausible (trigger_to_wire < 1ms in release build)
- [ ] No panics, no crashes
- [ ] Results documented in IMPLEMENTATION_LOG.md
