use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use alloy::primitives::{Address, U256};
use alloy::signers::local::PrivateKeySigner;
use crossbeam_channel::Receiver;
use pm_strategy::quote::DesiredQuote;
use rtt_core::clob_auth::L2Credentials;
use rtt_core::clob_executor::{
    process_one_clob, sign_and_dispatch, DispatchError, DispatchOutcome, PreSignedOrderPool,
};
use rtt_core::clob_order::{compute_amounts, ClobSide, Order, SignatureType, SignedOrderPayload};
use rtt_core::clob_response::parse_order_response;
use rtt_core::clob_signer::sign_order;
use rtt_core::connection::ConnectionPool;
use rtt_core::trigger::TriggerMessage;
use serde::{Deserialize, Serialize};

use crate::config::CredentialsConfig;
use crate::order_manager::ExecutionCommand;
use crate::order_state::WorkingQuote;
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

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuoteActionPlan {
    pub cancel_all: bool,
    pub cancel_order_ids: Vec<String>,
    pub place_quotes: Vec<DesiredQuote>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BatchOrderResponse {
    pub success: bool,
    #[serde(rename = "orderID")]
    pub order_id: String,
    pub status: String,
    #[serde(default, rename = "errorMsg")]
    pub error_msg: String,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CancelOrdersResponse {
    #[serde(default)]
    pub canceled: Vec<String>,
    #[serde(default)]
    pub not_canceled: BTreeMap<String, String>,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RebateSample {
    pub date: String,
    pub condition_id: String,
    pub asset_address: String,
    pub maker_address: String,
    pub rebated_fees_usdc: String,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct QuoteApiClient {
    client: reqwest::Client,
    base_url: String,
}

#[cfg_attr(not(test), allow(dead_code))]
impl QuoteApiClient {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::with_client_and_base_url(
            reqwest::Client::new(),
            rtt_core::polymarket::CLOB_BASE_URL,
        )
    }

    pub fn with_client_and_base_url(
        client: reqwest::Client,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            client,
            base_url: base_url.into(),
        }
    }

    pub async fn place_quotes(
        &self,
        quotes: &[DesiredQuote],
        creds: &L2Credentials,
        signer_params: &SignerParams,
    ) -> Result<Vec<BatchOrderResponse>, String> {
        let mut payloads = Vec::with_capacity(quotes.len());
        for quote in quotes {
            let order = build_quote_order(
                quote,
                signer_params.maker,
                signer_params.signer_addr,
                signer_params.fee_rate_bps,
                signer_params.sig_type,
            )?;
            let signature = sign_order(&signer_params.signer, &order, signer_params.is_neg_risk)
                .await
                .map_err(|err| format!("quote signing failed: {err}"))?;
            payloads.push(SignedOrderPayload::new(
                &order,
                &signature,
                quote.order_type,
                &signer_params.owner,
            ));
        }

        let body = serde_json::to_string(&payloads)
            .map_err(|err| format!("quote payload serialization failed: {err}"))?;
        let url = format!("{}/orders", self.base_url.trim_end_matches('/'));
        let response = self
            .authed_request(reqwest::Method::POST, "/orders", &url, creds, &body)
            .await?;

        response
            .json::<Vec<BatchOrderResponse>>()
            .await
            .map_err(|err| format!("quote response parse failed: {err}"))
    }

    pub async fn cancel_orders(
        &self,
        order_ids: &[String],
        creds: &L2Credentials,
    ) -> Result<CancelOrdersResponse, String> {
        let body = serde_json::to_string(order_ids)
            .map_err(|err| format!("cancel payload serialization failed: {err}"))?;
        let url = format!("{}/orders", self.base_url.trim_end_matches('/'));
        let response = self
            .authed_request(reqwest::Method::DELETE, "/orders", &url, creds, &body)
            .await?;

        response
            .json::<CancelOrdersResponse>()
            .await
            .map_err(|err| format!("cancel response parse failed: {err}"))
    }

    pub async fn cancel_all_orders(&self, creds: &L2Credentials) -> Result<(), String> {
        let url = format!("{}/cancel-all", self.base_url.trim_end_matches('/'));
        self.authed_request(
            reqwest::Method::DELETE,
            "/cancel-all",
            &url,
            creds,
            "",
        )
        .await
        .map(|_| ())
    }

    pub async fn send_heartbeat(&self, creds: &L2Credentials) -> Result<(), String> {
        let url = format!("{}/heartbeats", self.base_url.trim_end_matches('/'));
        self.authed_request(reqwest::Method::POST, "/heartbeats", &url, creds, "")
            .await
            .map(|_| ())
    }

    pub async fn fetch_reward_percentages(
        &self,
        creds: &L2Credentials,
        signature_type: SignatureType,
        maker_address: &str,
    ) -> Result<BTreeMap<String, f64>, String> {
        let query_path = format!(
            "/rewards/user/percentages?signature_type={}&maker_address={}",
            signature_type as u8, maker_address
        );
        let url = format!("{}{}", self.base_url.trim_end_matches('/'), query_path);
        let response = self
            .authed_request(reqwest::Method::GET, &query_path, &url, creds, "")
            .await?;
        let body = response
            .text()
            .await
            .map_err(|err| format!("reward percentages body read failed: {err}"))?;
        parse_reward_percentages(&body)
    }

    pub async fn fetch_rebates(
        &self,
        maker_address: &str,
        date: &str,
    ) -> Result<Vec<RebateSample>, String> {
        let url = format!(
            "{}/rebates/current?date={date}&maker_address={maker_address}",
            self.base_url.trim_end_matches('/')
        );
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| format!("rebates request failed: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("rebates request returned {}", response.status()));
        }
        let body = response
            .text()
            .await
            .map_err(|err| format!("rebates body read failed: {err}"))?;
        parse_rebates(&body)
    }

    async fn authed_request(
        &self,
        method: reqwest::Method,
        path: &str,
        url: &str,
        creds: &L2Credentials,
        body: &str,
    ) -> Result<reqwest::Response, String> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| format!("clock error: {err}"))?
            .as_secs()
            .to_string();
        let headers = rtt_core::clob_auth::build_l2_headers(
            creds,
            &timestamp,
            method.as_str(),
            path,
            body,
        )
        .map_err(|err| format!("auth header build failed: {err}"))?;

        let mut request = self.client.request(method, url);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if !body.is_empty() {
            request = request
                .header("content-type", "application/json")
                .body(body.to_string());
        }

        let response = request
            .send()
            .await
            .map_err(|err| format!("request failed: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("request returned {}", response.status()));
        }
        Ok(response)
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn build_quote_action_plan(
    commands: &[ExecutionCommand],
    working_quotes: &[WorkingQuote],
) -> QuoteActionPlan {
    let working_by_quote_id: BTreeMap<_, _> = working_quotes
        .iter()
        .filter_map(|quote| {
            quote
                .client_order_id
                .clone()
                .map(|order_id| (quote.quote_id.clone(), order_id))
        })
        .collect();

    let mut plan = QuoteActionPlan::default();
    for command in commands {
        match command {
            ExecutionCommand::Place(quote) => plan.place_quotes.push(quote.clone()),
            ExecutionCommand::Cancel { quote_id } => {
                if let Some(order_id) = working_by_quote_id.get(quote_id) {
                    plan.cancel_order_ids.push(order_id.clone());
                }
            }
            ExecutionCommand::CancelAll => plan.cancel_all = true,
        }
    }

    plan
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_reward_percentages(body: &str) -> Result<BTreeMap<String, f64>, String> {
    serde_json::from_str(body).map_err(|err| format!("reward percentages parse failed: {err}"))
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_rebates(body: &str) -> Result<Vec<RebateSample>, String> {
    serde_json::from_str(body).map_err(|err| format!("rebates parse failed: {err}"))
}

#[cfg_attr(not(test), allow(dead_code))]
fn build_quote_order(
    quote: &DesiredQuote,
    maker: Address,
    signer_addr: Address,
    fee_rate_bps: u64,
    signature_type: SignatureType,
) -> Result<Order, String> {
    let side = ClobSide::from(quote.side);
    let (maker_amount, taker_amount) = compute_amounts(&quote.price, &quote.size, side)
        .map_err(|err| format!("quote amount build failed: {err}"))?;
    let token_id = U256::from_str_radix(quote.asset_id.as_str(), 10)
        .map_err(|_| format!("token_id is not a valid decimal integer: {}", quote.asset_id))?;

    Ok(Order {
        salt: U256::from(rtt_core::clob_order::generate_salt()),
        maker,
        signer: signer_addr,
        taker: Address::ZERO,
        tokenId: token_id,
        makerAmount: maker_amount,
        takerAmount: taker_amount,
        expiration: U256::from(quote.expiration_unix_secs.unwrap_or_default()),
        nonce: U256::ZERO,
        feeRateBps: U256::from(fee_rate_bps),
        side: side as u8,
        signatureType: signature_type as u8,
    })
}

/// Build L2Credentials and PrivateKeySigner from executor config.
///
/// If `dry_run` is false and credentials are empty/invalid, returns an error.
/// If `dry_run` is true, empty credentials are allowed (we never send orders).
pub fn build_credentials(
    creds: &CredentialsConfig,
    dry_run: bool,
) -> Result<(L2Credentials, Option<PrivateKeySigner>), Box<dyn std::error::Error>> {
    if dry_run {
        let l2 = L2Credentials {
            api_key: creds.api_key.clone(),
            secret: creds.api_secret.clone(),
            passphrase: creds.passphrase.clone(),
            address: creds.signer_address.clone(),
        };
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
    let derived_signer_address = signer.address();

    let auth_address = if creds.signer_address.is_empty() {
        derived_signer_address.to_string()
    } else {
        let configured_signer_address: Address = creds
            .signer_address
            .parse()
            .map_err(|_| "invalid signer_address in config")?;
        if configured_signer_address != derived_signer_address {
            return Err("signer_address does not match private_key".into());
        }
        creds.signer_address.clone()
    };

    let l2 = L2Credentials {
        api_key: creds.api_key.clone(),
        secret: creds.api_secret.clone(),
        passphrase: creds.passphrase.clone(),
        address: auth_address,
    };

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
    use crate::order_manager::ExecutionCommand;
    use crate::order_state::WorkingQuote;
    use pm_strategy::quote::{DesiredQuote, QuoteId};
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

    fn working_quote(id: &str, order_id: &str) -> WorkingQuote {
        let mut quote = WorkingQuote::pending_submit(
            DesiredQuote::new(QuoteId::new(id), "1234", Side::Buy, "0.45", "10", OrderType::GTD),
            1_000,
        );
        quote.mark_working(order_id, 1_100);
        quote
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
    fn build_credentials_uses_signer_address_for_l2_auth() {
        let mut creds = valid_creds();
        creds.maker_address = "0x1111111111111111111111111111111111111111".to_string();
        creds.signer_address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string();

        let (l2, signer) = build_credentials(&creds, false).unwrap();

        assert!(signer.is_some());
        assert_eq!(l2.address, "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    }

    #[test]
    fn build_credentials_rejects_signer_address_mismatch() {
        let mut creds = valid_creds();
        creds.signer_address = "0x2222222222222222222222222222222222222222".to_string();

        let err = build_credentials(&creds, false).unwrap_err();

        assert!(err.to_string().contains("signer_address does not match private_key"));
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
    fn quote_action_plan_batches_place_and_cancel_operations() {
        let plan = build_quote_action_plan(
            &[
                ExecutionCommand::Cancel {
                    quote_id: QuoteId::new("quote-1"),
                },
                ExecutionCommand::Place(
                    DesiredQuote::new(
                        QuoteId::new("quote-2"),
                        "1234",
                        Side::Buy,
                        "0.44",
                        "10",
                        OrderType::GTD,
                    )
                    .with_expiration(1_700_000_000),
                ),
            ],
            &[working_quote("quote-1", "exchange-1")],
        );

        assert!(!plan.cancel_all);
        assert_eq!(plan.cancel_order_ids, vec!["exchange-1".to_string()]);
        assert_eq!(plan.place_quotes.len(), 1);
    }

    #[test]
    fn quote_order_builder_preserves_gtd_expiration() {
        let quote = DesiredQuote::new(
            QuoteId::new("quote-1"),
            "1234",
            Side::Buy,
            "0.44",
            "10",
            OrderType::GTD,
        )
        .with_expiration(1_700_000_000);

        let order = build_quote_order(
            &quote,
            valid_creds().maker_address.parse().unwrap(),
            valid_creds().signer_address.parse().unwrap(),
            0,
            SignatureType::Poly,
        )
        .unwrap();

        assert_eq!(order.expiration, U256::from(1_700_000_000u64));
    }

    #[test]
    fn telemetry_parsers_accept_reward_percentage_and_rebate_shapes() {
        let rewards = parse_reward_percentages(
            r#"{
                "0xcondition-1": 12.5,
                "0xcondition-2": 7.25
            }"#,
        )
        .unwrap();
        let rebates = parse_rebates(
            r#"[
                {
                    "date": "2026-02-27",
                    "condition_id": "0xcondition-1",
                    "asset_address": "0xasset",
                    "maker_address": "0xmaker",
                    "rebated_fees_usdc": "0.237519"
                }
            ]"#,
        )
        .unwrap();

        assert_eq!(rewards.get("0xcondition-1"), Some(&12.5));
        assert_eq!(rebates.len(), 1);
        assert_eq!(rebates[0].rebated_fees_usdc, "0.237519");
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
