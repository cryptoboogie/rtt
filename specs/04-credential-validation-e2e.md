# Spec 04: Credential Validation End-to-End

## Priority: MUST HAVE (blocking production)

## Problem

The system validates credentials structurally (non-empty strings) but has no automated way to verify they actually work against the live API without sending a real order. Session 8 proved credentials work by sending a real order, but there's no lightweight validation path.

## Current Code

- `crates/pm-executor/src/execution.rs` — `build_credentials()` (lines 19-58): Checks for empty strings only
- `crates/rtt-core/src/clob_auth.rs` — `build_l2_headers()`: Computes HMAC but doesn't verify against server
- `scripts/fire.sh` — Sends a real order (costs money)

## Solution

### 1. Add a credential validation function that hits a read-only API endpoint

Polymarket's CLOB API has authenticated GET endpoints that don't create orders. Use one to verify credentials work without spending money.

Add to `crates/rtt-core/src/clob_auth.rs`:
```rust
/// Validate L2 credentials by hitting GET /auth/api-keys.
/// Returns Ok(()) if the server accepts our HMAC auth.
/// Returns Err with the HTTP status and body if rejected.
pub async fn validate_credentials(creds: &L2Credentials) -> Result<(), String>
```

This function:
- Builds HMAC headers for `GET /auth/api-keys` with empty body
- Sends the request via a one-shot HTTPS client (reqwest or hyper)
- Returns Ok if status is 2xx, Err with status + body otherwise

### 2. Call validation at startup in live mode

In `pm-executor/src/main.rs`, after `build_credentials()` succeeds and before warming the connection pool, call `validate_credentials()`. If it fails, exit with a clear error message.

### 3. Add `--validate-creds` flag to pm-executor

Allow running just the credential check without starting the full pipeline:
```bash
cargo run -p pm-executor -- --validate-creds
```

This loads config, builds credentials, validates against the API, and exits.

## Files to Modify

| File | Changes |
|------|---------|
| `crates/rtt-core/src/clob_auth.rs` | Add `validate_credentials()` async function |
| `crates/pm-executor/src/main.rs` | Call validation at startup in live mode, add `--validate-creds` CLI flag |

## Tests

1. **Unit: validate_credentials builds correct GET request** — Verify method, path, headers (mock test, don't hit network)
2. **Integration (ignored): validate_credentials against live API** — `#[ignore]` test that runs with real `POLY_*` env vars

## Acceptance Criteria

- [ ] `validate_credentials()` hits a read-only endpoint (no orders placed)
- [ ] Live mode startup fails fast with clear error if credentials are invalid
- [ ] `--validate-creds` flag works standalone
- [ ] No money spent during validation

## Scope Boundaries

- Do NOT change `fire.sh` — it already works for order-level testing
- Do NOT add retry logic to validation — fail fast is correct here
- Do NOT cache validation results — check every startup
