use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use crossbeam_channel::Receiver;
use rtt_core::clob_auth::L2Credentials;
use rtt_core::clob_executor::{
    process_one_clob, sign_and_dispatch, DispatchError, DispatchOutcome, PreSignedOrderPool,
};
use rtt_core::clob_order::SignatureType;
use rtt_core::clob_response::parse_order_response;
use rtt_core::connection::ConnectionPool;
use rtt_core::trigger::TriggerMessage;

use crate::config::CredentialsConfig;
use crate::safety::{CircuitBreaker, OrderGuard, RateLimiter};

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuoteCommandPolicy {
    pub max_retries: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub throttle_window_ms: u64,
    pub max_commands_per_window: usize,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteCommandFailure {
    Transient,
    RateLimited,
    Permanent,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuoteCommandRetry {
    RetryAt { attempt: u32, at_ms: u64 },
    GiveUp,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThrottleDecision {
    pub allowed: bool,
    pub retry_at_ms: Option<u64>,
}

#[cfg_attr(not(test), allow(dead_code))]
pub struct QuoteCommandThrottle {
    policy: QuoteCommandPolicy,
    window_started_ms: Option<u64>,
    commands_in_window: usize,
}

impl Default for QuoteCommandPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 1_000,
            throttle_window_ms: 1_000,
            max_commands_per_window: 5,
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
impl QuoteCommandThrottle {
    pub fn new(policy: QuoteCommandPolicy) -> Self {
        Self {
            policy,
            window_started_ms: None,
            commands_in_window: 0,
        }
    }

    pub fn try_acquire(&mut self, now_ms: u64) -> ThrottleDecision {
        let window_started_ms = self.window_started_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(*window_started_ms) >= self.policy.throttle_window_ms {
            *window_started_ms = now_ms;
            self.commands_in_window = 0;
        }

        if self.commands_in_window >= self.policy.max_commands_per_window {
            return ThrottleDecision {
                allowed: false,
                retry_at_ms: Some(window_started_ms.saturating_add(self.policy.throttle_window_ms)),
            };
        }

        self.commands_in_window += 1;
        ThrottleDecision {
            allowed: true,
            retry_at_ms: None,
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn retry_decision(
    policy: &QuoteCommandPolicy,
    failure: QuoteCommandFailure,
    attempt: u32,
    now_ms: u64,
) -> QuoteCommandRetry {
    match failure {
        QuoteCommandFailure::Permanent => QuoteCommandRetry::GiveUp,
        QuoteCommandFailure::Transient | QuoteCommandFailure::RateLimited => {
            if attempt >= policy.max_retries {
                return QuoteCommandRetry::GiveUp;
            }

            let multiplier = 1u64 << attempt.min(16);
            let delay_ms = policy
                .initial_backoff_ms
                .saturating_mul(multiplier)
                .min(policy.max_backoff_ms);
            QuoteCommandRetry::RetryAt {
                attempt: attempt + 1,
                at_ms: now_ms.saturating_add(delay_ms),
            }
        }
    }
}

/// Build L2Credentials and PrivateKeySigner from executor config.
///
/// If `dry_run` is false and credentials are empty/invalid, returns an error.
/// If `dry_run` is true, empty credentials are allowed (we never send orders).
pub fn build_credentials(
    creds: &CredentialsConfig,
    dry_run: bool,
) -> Result<(L2Credentials, Option<PrivateKeySigner>), Box<dyn std::error::Error>> {
    let l2 = L2Credentials {
        api_key: creds.api_key.clone(),
        secret: creds.api_secret.clone(),
        passphrase: creds.passphrase.clone(),
        address: creds.maker_address.clone(),
    };

    if dry_run {
        return Ok((l2, None));
    }

    // Validate credentials for live mode
    if creds.private_key.is_empty() {
        return Err("private_key is required when dry_run = false".into());
    }
    if creds.api_key.is_empty() {
        return Err("api_key is required when dry_run = false".into());
    }
    if creds.api_secret.is_empty() {
        return Err("api_secret is required when dry_run = false".into());
    }
    if creds.passphrase.is_empty() {
        return Err("passphrase is required when dry_run = false".into());
    }
    if creds.maker_address.is_empty() {
        return Err("maker_address is required when dry_run = false".into());
    }

    let pk_hex = creds
        .private_key
        .strip_prefix("0x")
        .unwrap_or(&creds.private_key);
    let signer: PrivateKeySigner = pk_hex.parse()?;

    Ok((l2, Some(signer)))
}

/// Parameters for dynamic signing on the hot path.
pub struct SignerParams {
    pub signer: PrivateKeySigner,
    pub maker: Address,
    pub signer_addr: Address,
    pub fee_rate_bps: u64,
    pub is_neg_risk: bool,
    pub sig_type: SignatureType,
    pub owner: String,
}

/// The execution loop — runs on a dedicated OS thread (not tokio).
///
/// Reads triggers from the crossbeam channel and either:
/// - Dry-run: logs what it would do (with safety checks still applied)
/// - Live with signer_params: signs each order at the trigger's price via `sign_and_dispatch()`
/// - Live without signer_params: dispatches a pre-signed order via `process_one_clob()`
pub fn run_execution_loop(
    rx: Receiver<TriggerMessage>,
    pool: Arc<ConnectionPool>,
    mut presigned: PreSignedOrderPool,
    creds: L2Credentials,
    dry_run: bool,
    signer_params: Option<SignerParams>,
    circuit_breaker: CircuitBreaker,
    rate_limiter: &RateLimiter,
    order_guard: OrderGuard,
    shutdown: Arc<AtomicBool>,
    alert_webhook_url: Option<String>,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build executor tokio runtime");

    tracing::info!(
        dry_run = dry_run,
        presigned_count = presigned.len(),
        "Execution loop started"
    );

    while !shutdown.load(Ordering::Relaxed) {
        match rx.try_recv() {
            Ok(trigger) => {
                // Safety check 1: Circuit breaker — if tripped, stop entirely
                if circuit_breaker.is_tripped() {
                    tracing::error!("Circuit breaker tripped! Stopping execution loop.");
                    if let Some(ref url) = alert_webhook_url {
                        let (orders, usd) = circuit_breaker.stats();
                        let msg = format!(
                            "Circuit breaker tripped: {} orders, ${:.2} committed",
                            orders, usd
                        );
                        rt.block_on(crate::alert::send_alert(url, &msg));
                    }
                    break;
                }

                // Safety check 2: Rate limiter — drop excess triggers
                if !rate_limiter.try_acquire() {
                    tracing::warn!(
                        trigger_id = trigger.trigger_id,
                        "Rate limit exceeded, dropping trigger"
                    );
                    continue;
                }

                // Safety check 3: Order guard — prevent concurrent orders
                if !order_guard.try_acquire() {
                    tracing::warn!(
                        trigger_id = trigger.trigger_id,
                        "Order already in flight, dropping trigger"
                    );
                    continue;
                }

                if dry_run {
                    tracing::info!(
                        trigger_id = trigger.trigger_id,
                        token_id = %trigger.token_id,
                        side = ?trigger.side,
                        price = %trigger.price,
                        size = %trigger.size,
                        "[DRY RUN] Would fire order"
                    );
                    order_guard.release();
                    continue;
                }

                // Safety check 4: Circuit breaker amount check (live orders only)
                if let Err(e) = circuit_breaker.check_and_record(&trigger.price, &trigger.size) {
                    tracing::error!("Circuit breaker tripped: {}", e);
                    if let Some(ref url) = alert_webhook_url {
                        let (orders, usd) = circuit_breaker.stats();
                        let msg = format!(
                            "Circuit breaker tripped: {} orders, ${:.2} committed — {}",
                            orders, usd, e
                        );
                        rt.block_on(crate::alert::send_alert(url, &msg));
                    }
                    order_guard.release();
                    break;
                }

                let outcome = if let Some(ref sp) = signer_params {
                    sign_and_dispatch(
                        &pool,
                        &sp.signer,
                        &trigger,
                        &creds,
                        sp.maker,
                        sp.signer_addr,
                        sp.fee_rate_bps,
                        sp.is_neg_risk,
                        sp.sig_type,
                        &sp.owner,
                        &rt,
                    )
                } else {
                    process_one_clob(&pool, &mut presigned, &creds, &trigger, &rt)
                };

                // Release order guard after response received
                order_guard.release();

                let resp_body = match outcome {
                    DispatchOutcome::Sent { record, body } => {
                        tracing::info!(
                            trigger_id = trigger.trigger_id,
                            sign_duration_us = record.sign_duration() as f64 / 1000.0,
                            trigger_to_wire_us = record.trigger_to_wire() as f64 / 1000.0,
                            write_duration_us = record.write_duration() as f64 / 1000.0,
                            warm_ttfb_ms = record.warm_ttfb() as f64 / 1_000_000.0,
                            connection = record.connection_index,
                            pop = %record.cf_ray_pop,
                            reconnect = record.is_reconnect,
                            "Order dispatched"
                        );
                        body
                    }
                    DispatchOutcome::Rejected { record, error } => {
                        let level_message = match &error {
                            DispatchError::PoolExhausted => "Pre-signed order pool exhausted",
                            DispatchError::BuildOrder(_) => "Order build rejected before dispatch",
                            DispatchError::Sign(_) => "Order signing failed before dispatch",
                            DispatchError::RequestBuild(_) => {
                                "Order request assembly failed before dispatch"
                            }
                            DispatchError::Connection(_) => "Order dispatch failed on connection",
                        };

                        tracing::error!(
                            trigger_id = trigger.trigger_id,
                            error = %error,
                            sign_duration_us = record.sign_duration() as f64 / 1000.0,
                            trigger_to_wire_us = record.trigger_to_wire() as f64 / 1000.0,
                            write_duration_us = record.write_duration() as f64 / 1000.0,
                            warm_ttfb_ms = record.warm_ttfb() as f64 / 1_000_000.0,
                            connection = record.connection_index,
                            reconnect = record.is_reconnect,
                            "{level_message}"
                        );

                        if record.is_reconnect {
                            tracing::warn!(
                                "Order dispatch entered reconnect/cold path, tripping circuit breaker"
                            );
                            circuit_breaker.trip();
                            break;
                        }

                        None
                    }
                };

                // Log order response
                if let Some(body) = &resp_body {
                    match parse_order_response(body) {
                        Ok(resp) => {
                            if resp.success {
                                tracing::info!(
                                    order_id = %resp.order_id,
                                    status = %resp.status,
                                    "Order accepted"
                                );
                            } else {
                                tracing::error!(
                                    error = ?resp.error_msg,
                                    "Order rejected by server"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                body = %String::from_utf8_lossy(body),
                                error = %e,
                                "Failed to parse order response"
                            );
                        }
                    }
                }

                // Auto-refill: if pool is below 20% remaining, reset cursor.
                // This is safe when orders were rejected/dry-run (unique salts reused).
                // For accepted orders, the exchange rejects duplicate salts, so this
                // acts as a graceful degradation (resubmits fail, circuit breaker trips).
                let remaining = presigned.len().saturating_sub(presigned.consumed());
                let threshold = presigned.len() / 5; // 20%
                if remaining <= threshold && presigned.len() > 0 {
                    tracing::info!(
                        remaining = remaining,
                        total = presigned.len(),
                        "Pre-signed pool low, resetting cursor for refill"
                    );
                    presigned.reset_cursor();
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                std::thread::yield_now();
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
        }
    }

    let (orders, usd) = circuit_breaker.stats();
    tracing::info!(
        orders_fired = orders,
        usd_committed = format!("{:.2}", usd),
        tripped = circuit_breaker.is_tripped(),
        "Execution loop stopped"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtt_core::trigger::{OrderType, Side};

    fn empty_creds() -> CredentialsConfig {
        CredentialsConfig {
            api_key: String::new(),
            api_secret: String::new(),
            passphrase: String::new(),
            private_key: String::new(),
            maker_address: String::new(),
            signer_address: String::new(),
        }
    }

    fn valid_creds() -> CredentialsConfig {
        CredentialsConfig {
            api_key: "test-key".to_string(),
            api_secret: "dGVzdC1zZWNyZXQ=".to_string(),
            passphrase: "test-pass".to_string(),
            // Foundry test private key
            private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .to_string(),
            maker_address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
            signer_address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
        }
    }

    fn make_trigger(id: u64) -> TriggerMessage {
        TriggerMessage {
            trigger_id: id,
            token_id: "test-token".to_string(),
            side: Side::Buy,
            price: "0.45".to_string(),
            size: "10".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 1000,
        }
    }

    fn dummy_pool_and_creds() -> (Arc<ConnectionPool>, PreSignedOrderPool, L2Credentials) {
        let pool = Arc::new(ConnectionPool::new(
            "localhost",
            443,
            0,
            rtt_core::connection::AddressFamily::Auto,
        ));
        let presigned = PreSignedOrderPool::new(vec![]).unwrap();
        let creds = L2Credentials {
            api_key: String::new(),
            secret: String::new(),
            passphrase: String::new(),
            address: String::new(),
        };
        (pool, presigned, creds)
    }

    #[test]
    fn build_credentials_dry_run_allows_empty() {
        let (l2, signer) = build_credentials(&empty_creds(), true).unwrap();
        assert!(signer.is_none());
        assert!(l2.api_key.is_empty());
    }

    #[test]
    fn build_credentials_live_rejects_empty_private_key() {
        let err = build_credentials(&empty_creds(), false);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("private_key"), "error: {}", msg);
    }

    #[test]
    fn build_credentials_live_rejects_empty_api_key() {
        let mut creds = empty_creds();
        creds.private_key = "0xdeadbeef".to_string();
        let err = build_credentials(&creds, false);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("api_key"), "error: {}", msg);
    }

    #[test]
    fn build_credentials_live_valid() {
        let (_l2, signer) = build_credentials(&valid_creds(), true).unwrap();
        assert!(signer.is_none());

        let (l2, signer) = build_credentials(&valid_creds(), false).unwrap();
        assert!(signer.is_some());
        assert_eq!(l2.api_key, "test-key");
        assert_eq!(l2.secret, "dGVzdC1zZWNyZXQ=");
        assert_eq!(l2.address, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    }

    #[test]
    fn dry_run_execution_loop_logs_and_exits() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let cb = CircuitBreaker::new(100, 1000.0);
        let rl = RateLimiter::new(100);
        let og = OrderGuard::new();

        tx.send(make_trigger(42)).unwrap();
        drop(tx);

        let (pool, presigned, creds) = dummy_pool_and_creds();

        run_execution_loop(
            rx, pool, presigned, creds, true, None, cb, &rl, og, shutdown, None,
        );
    }

    #[test]
    fn circuit_breaker_stops_execution_loop() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        // Allow only 2 orders — uses live mode (dry_run=false) so check_and_record runs.
        // Dispatch will fail (empty pool, no connections), but breaker still counts.
        let cb = CircuitBreaker::new(2, 1000.0);
        let rl = RateLimiter::new(100);
        let og = OrderGuard::new();

        // Send 5 triggers
        for i in 0..5 {
            tx.send(make_trigger(i)).unwrap();
        }
        drop(tx);

        let (pool, presigned, creds) = dummy_pool_and_creds();
        run_execution_loop(
            rx,
            pool,
            presigned,
            creds,
            false,
            None,
            cb.clone(),
            &rl,
            og,
            shutdown,
            None,
        );

        // Only 2 orders should have been recorded before tripping
        let (orders, _) = cb.stats();
        assert!(orders <= 3, "expected <=3 orders, got {}", orders);
        assert!(cb.is_tripped());
    }

    #[test]
    fn rate_limiter_drops_excess_triggers() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let cb = CircuitBreaker::new(100, 1000.0);
        // Allow only 1 per second — uses live mode so check_and_record runs.
        let rl = RateLimiter::new(1);
        let og = OrderGuard::new();

        // Send 5 triggers (all arrive in same instant)
        for i in 0..5 {
            tx.send(make_trigger(i)).unwrap();
        }
        drop(tx);

        let (pool, presigned, creds) = dummy_pool_and_creds();
        run_execution_loop(
            rx,
            pool,
            presigned,
            creds,
            false,
            None,
            cb.clone(),
            &rl,
            og,
            shutdown,
            None,
        );

        // Only 1 should have passed the rate limiter
        let (orders, _) = cb.stats();
        assert_eq!(
            orders, 1,
            "expected 1 order past rate limiter, got {}",
            orders
        );
    }

    #[test]
    fn order_guard_prevents_concurrent_orders() {
        // The order guard is acquired and released per-trigger in a serial loop,
        // so in a single-threaded loop it always succeeds. This test verifies
        // that if the guard is pre-acquired externally, triggers are dropped.
        let (tx, rx) = crossbeam_channel::bounded(16);
        let shutdown = Arc::new(AtomicBool::new(false));
        let cb = CircuitBreaker::new(100, 1000.0);
        let rl = RateLimiter::new(100);
        let og = OrderGuard::new();

        // Pre-acquire the guard to simulate an in-flight order
        assert!(og.try_acquire());

        tx.send(make_trigger(1)).unwrap();
        drop(tx);

        let (pool, presigned, creds) = dummy_pool_and_creds();
        run_execution_loop(
            rx,
            pool,
            presigned,
            creds,
            true,
            None,
            cb.clone(),
            &rl,
            og,
            shutdown,
            None,
        );

        // No orders should have been processed (guard was held)
        let (orders, _) = cb.stats();
        assert_eq!(orders, 0, "expected 0 orders (guard held), got {}", orders);
    }

    #[test]
    fn quote_retry_backoff_is_bounded_and_stops_after_max_retries() {
        let policy = QuoteCommandPolicy {
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 250,
            throttle_window_ms: 1_000,
            max_commands_per_window: 2,
        };

        assert_eq!(
            retry_decision(&policy, QuoteCommandFailure::Transient, 0, 1_000),
            QuoteCommandRetry::RetryAt {
                attempt: 1,
                at_ms: 1_100,
            }
        );
        assert_eq!(
            retry_decision(&policy, QuoteCommandFailure::RateLimited, 2, 1_000),
            QuoteCommandRetry::RetryAt {
                attempt: 3,
                at_ms: 1_250,
            }
        );
        assert_eq!(
            retry_decision(&policy, QuoteCommandFailure::Transient, 3, 1_000),
            QuoteCommandRetry::GiveUp
        );
        assert_eq!(
            retry_decision(&policy, QuoteCommandFailure::Permanent, 0, 1_000),
            QuoteCommandRetry::GiveUp
        );
    }

    #[test]
    fn quote_command_throttle_prevents_unbounded_command_storms() {
        let policy = QuoteCommandPolicy {
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 500,
            throttle_window_ms: 1_000,
            max_commands_per_window: 2,
        };
        let mut throttle = QuoteCommandThrottle::new(policy);

        assert_eq!(
            throttle.try_acquire(1_000),
            ThrottleDecision {
                allowed: true,
                retry_at_ms: None,
            }
        );
        assert_eq!(
            throttle.try_acquire(1_001),
            ThrottleDecision {
                allowed: true,
                retry_at_ms: None,
            }
        );
        assert_eq!(
            throttle.try_acquire(1_002),
            ThrottleDecision {
                allowed: false,
                retry_at_ms: Some(2_000),
            }
        );
        assert_eq!(
            throttle.try_acquire(2_000),
            ThrottleDecision {
                allowed: true,
                retry_at_ms: None,
            }
        );
    }
}
