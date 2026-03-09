# Spec 02: Dynamic Pricing (Sign Orders at Market Price)

## Priority: MUST HAVE (blocking production)

## Problem

Orders are pre-signed at startup using a fixed price (the strategy's `threshold` value from config). When a strategy fires, the pre-signed order has whatever price was set at startup — not the current market price. If the market has moved, the order either:
- Fills at a worse price than necessary
- Gets rejected because the price is too far from the current market

This makes the system unusable for any strategy that needs to trade at the current best bid/ask.

## Current Code

- `crates/pm-executor/src/main.rs` (lines 88-120): Pre-signs orders at startup using `config.strategy.params.threshold` as the price
- `crates/rtt-core/src/clob_signer.rs` — `presign_batch()`: Signs N orders with unique salts at a fixed price
- `crates/rtt-core/src/clob_executor.rs` — `PreSignedOrderPool`: Stores pre-serialized JSON bodies, dispatches with fresh HMAC only
- `crates/pm-executor/src/execution.rs` — `run_execution_loop()` (line 134): Calls `process_one_clob()` which uses pre-signed body as-is

### Why pre-signing exists
EIP-712 signing is async (alloy's signer) and takes ~100-500us. Pre-signing moves this off the hot path. The tradeoff was: fixed price, but instant dispatch.

## Solution

### Approach: Sign on trigger arrival

Instead of pre-signing at startup, sign each order when the trigger arrives. The trigger already contains the exact price the strategy wants. This adds ~100-500us to the hot path but gives correct pricing.

### Implementation

1. **Remove `PreSignedOrderPool` from the hot path.** Keep the struct for future optimization but don't use it for dispatch.

2. **Add `sign_and_dispatch()` to `clob_executor.rs`:**
```rust
pub fn sign_and_dispatch(
    pool: &ConnectionPool,
    signer: &PrivateKeySigner,
    trigger: &TriggerMessage,
    creds: &L2Credentials,
    maker: Address,
    signer_addr: Address,
    fee_rate_bps: u64,
    is_neg_risk: bool,
    sig_type: SignatureType,
    owner: &str,
    rt: &tokio::runtime::Runtime,
) -> (TimestampRecord, Option<Vec<u8>>)
```

This function:
- Calls `build_order(trigger, maker, signer_addr, fee_rate_bps, sig_type)` — builds Order from trigger at trigger's price
- Calls `sign_order(signer, &order, is_neg_risk)` — EIP-712 sign
- Builds `SignedOrderPayload` with the signature
- Calls `build_order_request(&payload, creds)` — full HTTP request with HMAC
- Calls `pool.send_start(req)` + `handle.collect()` — dispatch

3. **Update `run_execution_loop()`** to accept a `PrivateKeySigner` and call `sign_and_dispatch()` instead of `process_one_clob()`.

4. **Update `main.rs`** to pass the signer through to the execution loop. Remove pre-signing at startup (or make it optional behind a config flag for future use).

5. **Add signing timestamp to `TimestampRecord`:**
```rust
t_sign_start: u64,  // Before EIP-712 signing
t_sign_end: u64,    // After EIP-712 signing
```
This lets us measure signing overhead separately.

## Files to Modify

| File | Changes |
|------|---------|
| `crates/rtt-core/src/clob_executor.rs` | Add `sign_and_dispatch()` function |
| `crates/rtt-core/src/metrics.rs` | Add `t_sign_start`, `t_sign_end` to `TimestampRecord`, add `sign_duration()` derived metric |
| `crates/pm-executor/src/execution.rs` | Accept `PrivateKeySigner`, use `sign_and_dispatch()` in live mode |
| `crates/pm-executor/src/main.rs` | Pass signer to execution loop, remove mandatory pre-signing |
| `crates/pm-executor/src/config.rs` | No changes needed — signer_address already exists |

## Tests

1. **Unit: `sign_and_dispatch` produces valid request** — Build order from trigger, sign, verify POST /order structure with correct price from trigger
2. **Unit: signing uses trigger price, not config price** — Create trigger with price "0.63", verify the order's makerAmount/takerAmount match 0.63
3. **Unit: `sign_duration()` metric is populated** — Verify t_sign_start < t_sign_end
4. **Integration: execution loop signs at trigger price** — Send trigger with price "0.42" through dry-run loop, verify log shows price "0.42" (not config threshold)

## Acceptance Criteria

- [ ] Orders are signed with the price from `TriggerMessage`, not from config
- [ ] `sign_duration()` metric measures EIP-712 signing time
- [ ] Existing `process_one_clob` tests still pass (function not removed, just not used on hot path)
- [ ] `fire.sh` still works (it bypasses the executor pipeline)
- [ ] All existing tests pass

## Scope Boundaries

- Do NOT remove `PreSignedOrderPool` — keep it for potential future use (price ladders, pre-computed levels)
- Do NOT implement background re-signing thread (that's a future optimization)
- Do NOT change the strategy interface — strategies already return price in `TriggerMessage`
- Keep pre-signing as an optional startup step (controlled by config) for backwards compatibility
