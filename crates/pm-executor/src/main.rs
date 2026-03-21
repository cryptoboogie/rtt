mod alert;
mod analysis_store;
mod bridge;
mod capital;
mod config;
mod execution;
mod health;
mod health_server;
mod logging;
mod order_manager;
mod order_state;
mod safety;
mod state;
mod user_feed;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use analysis_store::{AnalysisOperation, AnalysisStore};
use config::ExecutorConfig;
use order_manager::{ExecutionCommand, LocalOrderManager, ReconciliationPolicy};
use order_state::WorkingQuote;
use pm_strategy::liquidity_rewards::LiquidityRewardsMarket;
use pm_strategy::quote::DesiredQuotes;
use pm_strategy::runtime::QuoteRuntime;
use pm_strategy::strategy::InventoryDelta;
use rtt_core::{HotStateStore, NormalizedUpdate, UpdateNotice};
use user_feed::{UserFeedRuntimeEvent, UserFeedState};

#[derive(Debug, Clone)]
struct SelectedQuotePortfolio {
    selected_markets: Vec<pm_data::market_registry::SelectedRewardMarket>,
    quote_markets: Vec<LiquidityRewardsMarket>,
    market_meta: Vec<rtt_core::MarketMeta>,
    asset_ids: Vec<String>,
    condition_ids: Vec<String>,
}

enum QuoteModeEvent {
    MarketUpdate(NormalizedUpdate),
    UserFeed(UserFeedRuntimeEvent),
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let validate_creds_only = args.iter().any(|a| a == "--validate-creds");
    let config_path = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .or_else(|| {
            args.iter()
                .find(|a| !a.starts_with('-') && *a != &args[0])
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("config.toml"));

    let config = ExecutorConfig::load(&config_path).unwrap_or_else(|e| {
        eprintln!(
            "Failed to load config from {}: {}",
            config_path.display(),
            e
        );
        std::process::exit(1);
    });

    logging::init(&config.logging);

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    if validate_creds_only {
        tracing::info!("Validating credentials...");
        let (l2_creds, _) = execution::build_credentials(&config.credentials, false)
            .unwrap_or_else(|e| {
                tracing::error!("Credential error: {}", e);
                std::process::exit(1);
            });
        match rt.block_on(rtt_core::clob_auth::validate_credentials(&l2_creds)) {
            Ok(()) => {
                tracing::info!("Credentials validated successfully");
                std::process::exit(0);
            }
            Err(e) => {
                tracing::error!("Credential validation failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    tracing::info!("Starting pm-executor");
    rt.block_on(run(config));
}

async fn run(config: ExecutorConfig) {
    let monitored_assets = config.resolved_subscription_asset_ids();
    tracing::info!(
        markets = ?monitored_assets,
        strategy = %config.strategy.strategy,
        dry_run = config.execution.dry_run,
        "Pipeline configuration loaded"
    );

    let (l2_creds, signer) =
        execution::build_credentials(&config.credentials, config.execution.dry_run).unwrap_or_else(
            |e| {
                tracing::error!("Credential error: {}", e);
                std::process::exit(1);
            },
        );

    if !config.execution.dry_run {
        tracing::info!("Validating credentials against live API...");
        match rtt_core::clob_auth::validate_credentials(&l2_creds).await {
            Ok(()) => tracing::info!("Credentials validated successfully"),
            Err(e) => {
                tracing::error!("Credential validation failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    let signer_params = build_signer_params(&config, signer);

    if config.strategy.strategy == "liquidity_rewards" {
        run_quote_mode(config, l2_creds, signer_params).await;
    } else {
        run_trigger_mode(config, l2_creds, signer_params, monitored_assets).await;
    }
}

fn build_signer_params(
    config: &ExecutorConfig,
    signer: Option<alloy::signers::local::PrivateKeySigner>,
) -> Option<execution::SignerParams> {
    if config.execution.dry_run {
        return None;
    }

    let signer = signer.expect("signer required for live mode");
    let signer_addr = signer.address();
    let maker_addr: alloy::primitives::Address = config
        .credentials
        .maker_address
        .parse()
        .expect("invalid maker_address in config");
    let sig_type = resolve_signature_type(config.execution.signature_type, maker_addr, signer_addr);

    Some(execution::SignerParams {
        signer,
        maker: maker_addr,
        signer_addr,
        fee_rate_bps: config.execution.fee_rate_bps,
        is_neg_risk: config.execution.is_neg_risk,
        sig_type,
        owner: config.credentials.api_key.clone(),
    })
}

fn resolve_signature_type(
    configured_sig_type: Option<u8>,
    maker_addr: alloy::primitives::Address,
    signer_addr: alloy::primitives::Address,
) -> rtt_core::clob_order::SignatureType {
    match configured_sig_type {
        Some(0) => rtt_core::clob_order::SignatureType::Eoa,
        Some(1) => rtt_core::clob_order::SignatureType::Poly,
        Some(2) => rtt_core::clob_order::SignatureType::GnosisSafe,
        Some(other) => {
            let derived = derive_signature_type(maker_addr, signer_addr);
            tracing::warn!(
                configured_sig_type = other,
                derived_sig_type = derived as u8,
                "Unsupported signature type override; falling back to derived signature type"
            );
            derived
        }
        None => derive_signature_type(maker_addr, signer_addr),
    }
}

fn derive_signature_type(
    maker_addr: alloy::primitives::Address,
    signer_addr: alloy::primitives::Address,
) -> rtt_core::clob_order::SignatureType {
    if maker_addr == signer_addr {
        rtt_core::clob_order::SignatureType::Eoa
    } else {
        rtt_core::clob_order::SignatureType::GnosisSafe
    }
}

async fn run_trigger_mode(
    config: ExecutorConfig,
    l2_creds: rtt_core::clob_auth::L2Credentials,
    signer_params: Option<execution::SignerParams>,
    monitored_assets: Vec<String>,
) {
    let strategy = config.strategy.build_strategy().unwrap_or_else(|e| {
        tracing::error!("Failed to build strategy: {}", e);
        std::process::exit(1);
    });

    let af = match config.connection.address_family.as_str() {
        "ipv4" => rtt_core::connection::AddressFamily::V4,
        "ipv6" => rtt_core::connection::AddressFamily::V6,
        _ => rtt_core::connection::AddressFamily::Auto,
    };

    let mut conn_pool = rtt_core::connection::ConnectionPool::new(
        rtt_core::polymarket::CLOB_HOST,
        rtt_core::polymarket::CLOB_PORT,
        config.connection.pool_size,
        af,
    );
    if !config.execution.dry_run {
        let warm = conn_pool
            .warmup()
            .await
            .expect("Failed to warm connection pool");
        tracing::info!(warm_connections = warm, "Connection pool ready");
    }

    let presigned_pool = rtt_core::clob_executor::PreSignedOrderPool::new(vec![])
        .expect("Failed to create pre-signed order pool");
    tracing::info!(
        dynamic_signing = signer_params.is_some(),
        dry_run = config.execution.dry_run,
        "Execution setup complete"
    );

    let (snapshot_mpsc_tx, snapshot_mpsc_rx) = tokio::sync::mpsc::channel(256);
    let (trigger_mpsc_tx, trigger_mpsc_rx) = tokio::sync::mpsc::channel(256);
    let (trigger_crossbeam_tx, trigger_crossbeam_rx) = crossbeam_channel::bounded(256);
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

    let state_path = std::path::Path::new(&config.execution.state_file);
    let prev_state = state::ExecutorState::load(state_path);
    if prev_state.orders_fired > 0 || prev_state.tripped {
        tracing::warn!(
            prev_orders = prev_state.orders_fired,
            prev_usd_cents = prev_state.usd_committed_cents,
            prev_tripped = prev_state.tripped,
            last_shutdown = %prev_state.last_shutdown,
            "Restoring state from previous run"
        );
    }

    let circuit_breaker = safety::CircuitBreaker::with_initial_counts(
        config.safety.max_orders,
        config.safety.max_usd_exposure,
        prev_state.orders_fired,
        prev_state.usd_committed_cents,
    );
    let rate_limiter = safety::RateLimiter::new(config.safety.max_triggers_per_second);
    let order_guard = safety::OrderGuard::new();

    tracing::warn!(
        max_orders = config.safety.max_orders,
        max_usd = config.safety.max_usd_exposure,
        max_triggers_per_sec = config.safety.max_triggers_per_second,
        require_confirmation = config.safety.require_confirmation,
        "Safety limits active — circuit breaker will halt after these limits"
    );

    let exec_shutdown = Arc::new(AtomicBool::new(false));
    let exec_shutdown_clone = exec_shutdown.clone();
    let conn_pool = Arc::new(conn_pool);
    let exec_dry_run = config.execution.dry_run;
    let exec_creds = l2_creds;
    let exec_cb = circuit_breaker.clone();
    let exec_webhook = config.safety.alert_webhook_url.clone();
    let exec_handle = std::thread::spawn(move || {
        execution::run_execution_loop(
            trigger_crossbeam_rx,
            conn_pool,
            presigned_pool,
            exec_creds,
            exec_dry_run,
            signer_params,
            exec_cb,
            &rate_limiter,
            order_guard,
            exec_shutdown_clone,
            exec_webhook,
        );
    });

    let mut pipeline = pm_data::Pipeline::new(
        monitored_assets.clone(),
        config.websocket.ws_channel_capacity,
        config.websocket.snapshot_channel_capacity,
    );
    let snapshot_broadcast_rx = pipeline.subscribe_snapshots();
    let ws_last_message_at = pipeline.ws_client_last_message_at();
    let ws_reconnect_count = pipeline.ws_client_reconnect_count();

    let ws_handle = tokio::spawn(async move {
        pipeline.run().await;
    });

    let shutdown_rx_bridge = shutdown_tx.subscribe();
    let bridge_handle = tokio::spawn(bridge::broadcast_to_mpsc(
        snapshot_broadcast_rx,
        snapshot_mpsc_tx,
        shutdown_rx_bridge,
    ));

    let mut runner =
        pm_strategy::runner::StrategyRunner::new(strategy, snapshot_mpsc_rx, trigger_mpsc_tx);
    let strategy_handle = tokio::spawn(async move {
        runner.run().await;
    });

    let shutdown_rx_trigger = shutdown_tx.subscribe();
    let trigger_bridge_handle = tokio::spawn(bridge::mpsc_to_crossbeam(
        trigger_mpsc_rx,
        trigger_crossbeam_tx,
        shutdown_rx_trigger,
    ));

    let shutdown_rx_health = shutdown_tx.subscribe();
    let health_cb = circuit_breaker.clone();
    let health_state_path = config.execution.state_file.clone();
    let health_handle = tokio::spawn(health::run_health_monitor(
        monitored_assets.clone(),
        Some(health_cb),
        shutdown_rx_health,
        Some(health_state_path),
    ));

    let _health_server_handle = if config.health.enabled {
        let shutdown_rx_hs = shutdown_tx.subscribe();
        let hs_cb = circuit_breaker.clone();
        let hs_lma = ws_last_message_at.clone();
        let hs_rc = ws_reconnect_count.clone();
        let start_time = Instant::now();
        Some(tokio::spawn(health_server::run_health_server(
            config.health.port,
            hs_cb,
            hs_lma,
            hs_rc,
            start_time,
            shutdown_rx_hs,
        )))
    } else {
        None
    };

    tracing::info!("All components started. Waiting for shutdown signal (Ctrl+C)...");
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl_c");

    tracing::info!("Shutdown signal received. Stopping components...");
    let _ = shutdown_tx.send(true);
    exec_shutdown.store(true, Ordering::Relaxed);

    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let _ = ws_handle.await;
        let _ = bridge_handle.await;
        let _ = strategy_handle.await;
        let _ = trigger_bridge_handle.await;
        let _ = health_handle.await;
    })
    .await;
    let _ = exec_handle.join();
    persist_final_state(&config.execution.state_file, &circuit_breaker);

    tracing::info!("pm-executor shut down cleanly");
}

struct QuoteModeController {
    runtime: QuoteRuntime,
    hot_store: HotStateStore,
    order_manager: LocalOrderManager,
    user_feed_state: UserFeedState,
    analysis_store: AnalysisStore,
    quote_client: execution::QuoteApiClient,
    creds: rtt_core::clob_auth::L2Credentials,
    signer_params: Option<execution::SignerParams>,
    dry_run: bool,
    budget_limit_usd: f64,
    maker_address: String,
    working: Vec<WorkingQuote>,
    last_desired: DesiredQuotes,
    last_notice: Option<UpdateNotice>,
    last_heartbeat_id: Option<String>,
}

impl QuoteModeController {
    async fn handle_market_update(&mut self, update: NormalizedUpdate) {
        self.hot_store.apply_update(&update);
        self.last_notice = Some(update.notice.clone());
        let desired = self
            .runtime
            .handle_notice(&update.notice)
            .unwrap_or_default();
        self.last_desired = desired.clone();
        let capital = self.capital_snapshot();
        let _ = self.analysis_store.append(&AnalysisOperation {
            timestamp_ms: now_ms(),
            operation_type: "quote_set_emitted".to_string(),
            condition_id: None,
            asset_id: None,
            quote_id: None,
            client_order_id: None,
            exchange_order_id: None,
            side: None,
            requested_price: None,
            requested_size: None,
            result_status: format!("quotes={}", desired.quotes.len()),
            error_text: None,
            capital_before_usd: Some(capital.active_deployed_usd),
            capital_after_usd: Some(capital.active_deployed_usd),
            reward_share: None,
            payload_json: serde_json::to_string(&desired).ok(),
        });
        self.reconcile_and_execute().await;
    }

    async fn handle_user_feed_event(&mut self, event: UserFeedRuntimeEvent) {
        match event {
            UserFeedRuntimeEvent::Connected => {
                self.user_feed_state.mark_connected();
                self.reconcile_and_execute().await;
            }
            UserFeedRuntimeEvent::Event(event) => {
                self.user_feed_state.apply_event(event, &self.working);
                self.reconcile_and_execute().await;
                if let Some(notice) = self.last_notice.clone() {
                    self.last_desired = self.runtime.handle_notice(&notice).unwrap_or_default();
                    self.reconcile_and_execute().await;
                }
            }
            UserFeedRuntimeEvent::Degraded(reason) => {
                self.fail_closed(&reason).await;
            }
        }
    }

    async fn heartbeat(&mut self) {
        if self.dry_run || !self.has_live_quotes() {
            self.last_heartbeat_id = None;
            return;
        }

        match self
            .quote_client
            .send_heartbeat(&self.creds, self.last_heartbeat_id.as_deref())
            .await
        {
            Ok(heartbeat_id) => self.last_heartbeat_id = Some(heartbeat_id),
            Err(err) => self.fail_closed(&format!("heartbeat_failed:{err}")).await,
        }
    }

    async fn sample_reward_percentages(&mut self) {
        if self.dry_run {
            return;
        }

        let Some(signer_params) = self.signer_params.as_ref() else {
            return;
        };

        let Ok(samples) = self
            .quote_client
            .fetch_reward_percentages(&self.creds, signer_params.sig_type, &self.maker_address)
            .await
        else {
            return;
        };

        let capital = self.capital_snapshot();
        for (condition_id, reward_share) in samples {
            let _ = self.analysis_store.append(&AnalysisOperation {
                timestamp_ms: now_ms(),
                operation_type: "reward_percentage_sample".to_string(),
                condition_id: Some(condition_id),
                asset_id: None,
                quote_id: None,
                client_order_id: None,
                exchange_order_id: None,
                side: None,
                requested_price: None,
                requested_size: None,
                result_status: "ok".to_string(),
                error_text: None,
                capital_before_usd: Some(capital.active_deployed_usd),
                capital_after_usd: Some(capital.active_deployed_usd),
                reward_share: Some(reward_share),
                payload_json: None,
            });
        }
    }

    async fn sample_rebates(&mut self) {
        if self.dry_run {
            return;
        }

        let today = chrono::Utc::now().format("%F").to_string();
        let Ok(samples) = self
            .quote_client
            .fetch_rebates(&self.maker_address, &today)
            .await
        else {
            return;
        };
        let capital = self.capital_snapshot();
        for sample in samples {
            let _ = self.analysis_store.append(&AnalysisOperation {
                timestamp_ms: now_ms(),
                operation_type: "rebate_sample".to_string(),
                condition_id: Some(sample.condition_id.clone()),
                asset_id: Some(sample.asset_address.clone()),
                quote_id: None,
                client_order_id: None,
                exchange_order_id: None,
                side: None,
                requested_price: None,
                requested_size: None,
                result_status: sample.date.clone(),
                error_text: None,
                capital_before_usd: Some(capital.active_deployed_usd),
                capital_after_usd: Some(capital.active_deployed_usd),
                reward_share: None,
                payload_json: serde_json::to_string(&sample).ok(),
            });
        }
    }

    async fn reconcile_and_execute(&mut self) {
        let now_ms = now_ms();
        let snapshot = self
            .user_feed_state
            .exchange_snapshot(&self.working, now_ms);
        let outcome = if self.dry_run {
            self.order_manager
                .reconcile(&self.last_desired, &self.working, now_ms)
        } else {
            self.order_manager.reconcile_with_exchange(
                &self.last_desired,
                &self.working,
                &snapshot,
                now_ms,
            )
        };

        self.working = outcome.working.clone();
        for delta in outcome.exposure_deltas {
            self.runtime.apply_inventory_delta(InventoryDelta::new(
                delta.asset_id,
                delta.side,
                delta.filled_size_delta,
                delta.notional_delta,
                delta.observed_at_ms,
            ));
        }

        if outcome.blocked || outcome.commands.is_empty() {
            return;
        }

        let filtered_commands = self.filter_budgeted_commands(&outcome.commands);
        if filtered_commands.is_empty() {
            return;
        }

        self.execute_commands(filtered_commands).await;
    }

    fn filter_budgeted_commands(&self, commands: &[ExecutionCommand]) -> Vec<ExecutionCommand> {
        let inventory = self.runtime.inventory_positions();
        let mut filtered = Vec::new();
        for command in commands {
            match command {
                ExecutionCommand::Cancel { .. } | ExecutionCommand::CancelAll => {
                    filtered.push(command.clone());
                }
                ExecutionCommand::Place(_) => {
                    let mut candidate = filtered.clone();
                    candidate.push(command.clone());
                    if capital::command_plan_within_budget(
                        self.budget_limit_usd,
                        &self.working,
                        &inventory,
                        &candidate,
                    ) {
                        filtered = candidate;
                    }
                }
            }
        }
        filtered
    }

    async fn execute_commands(&mut self, commands: Vec<ExecutionCommand>) {
        let capital_before = self.capital_snapshot();
        let plan = execution::build_quote_action_plan(&commands, &self.working);
        let mut batch_errors = Vec::new();

        if plan.cancel_all {
            if self.dry_run {
                for quote in &mut self.working {
                    if quote.is_cancelable() {
                        quote.mark_canceled(now_ms());
                    }
                }
            } else {
                match self.quote_client.cancel_all_orders(&self.creds).await {
                    Ok(()) => {
                        for quote in &mut self.working {
                            if quote.is_cancelable() {
                                quote.mark_pending_cancel(now_ms());
                            }
                        }
                    }
                    Err(err) => batch_errors.push(format!("cancel_all:{err}")),
                }
            }
        }

        if !plan.cancel_order_ids.is_empty() {
            if self.dry_run {
                for order_id in &plan.cancel_order_ids {
                    if let Some(quote) = self
                        .working
                        .iter_mut()
                        .find(|quote| quote.client_order_id.as_deref() == Some(order_id.as_str()))
                    {
                        quote.mark_canceled(now_ms());
                    }
                }
            } else {
                match self
                    .quote_client
                    .cancel_orders(&plan.cancel_order_ids, &self.creds)
                    .await
                {
                    Ok(_) => {
                        for order_id in &plan.cancel_order_ids {
                            if let Some(quote) = self.working.iter_mut().find(|quote| {
                                quote.client_order_id.as_deref() == Some(order_id.as_str())
                            }) {
                                quote.mark_pending_cancel(now_ms());
                            }
                        }
                    }
                    Err(err) => batch_errors.push(format!("cancel_orders:{err}")),
                }
            }
        }

        if !plan.place_quotes.is_empty() {
            if self.dry_run {
                for desired in &plan.place_quotes {
                    upsert_working_quote(&mut self.working, desired.clone(), now_ms());
                    if let Some(quote) = self
                        .working
                        .iter_mut()
                        .find(|quote| quote.quote_id == desired.quote_id)
                    {
                        quote.mark_working(format!("dry-run:{}", desired.quote_id), now_ms());
                    }
                }
            } else if let Some(signer_params) = self.signer_params.as_ref() {
                match self
                    .quote_client
                    .place_quotes(&plan.place_quotes, &self.creds, signer_params)
                    .await
                {
                    Ok(responses) => {
                        if responses.len() != plan.place_quotes.len() {
                            batch_errors.push(format!(
                                "place_quotes:response_count_mismatch:{}!={}",
                                responses.len(),
                                plan.place_quotes.len()
                            ));
                        }

                        for (desired, response) in plan.place_quotes.iter().zip(responses.iter()) {
                            upsert_working_quote(&mut self.working, desired.clone(), now_ms());
                            if let Some(quote) = self
                                .working
                                .iter_mut()
                                .find(|quote| quote.quote_id == desired.quote_id)
                            {
                                match execution::classify_quote_response(response) {
                                    execution::QuotePlacementDisposition::Resting {
                                        client_order_id,
                                        ..
                                    } => quote.mark_working(client_order_id, now_ms()),
                                    execution::QuotePlacementDisposition::NonResting { status } => {
                                        quote.mark_rejected(
                                            format!("non_resting_status:{status}"),
                                            now_ms(),
                                        )
                                    }
                                    execution::QuotePlacementDisposition::Rejected { reason } => {
                                        quote.mark_rejected(reason, now_ms())
                                    }
                                }
                            }

                            let capital = self.capital_snapshot();
                            let status = execution::normalized_order_status(&response.status);
                            let error_text = if response.error_msg.trim().is_empty() {
                                None
                            } else {
                                Some(response.error_msg.clone())
                            };
                            let _ = self.analysis_store.append(&AnalysisOperation {
                                timestamp_ms: now_ms(),
                                operation_type: "quote_submit_result".to_string(),
                                condition_id: Some(
                                    desired
                                        .quote_id
                                        .as_str()
                                        .split(':')
                                        .next()
                                        .unwrap_or_default()
                                        .to_string(),
                                ),
                                asset_id: Some(desired.asset_id.clone()),
                                quote_id: Some(desired.quote_id.to_string()),
                                client_order_id: if response.order_id.is_empty() {
                                    None
                                } else {
                                    Some(response.order_id.clone())
                                },
                                exchange_order_id: None,
                                side: Some(format!("{:?}", desired.side)),
                                requested_price: Some(desired.price.clone()),
                                requested_size: Some(desired.size.clone()),
                                result_status: if response.success {
                                    status
                                } else {
                                    "rejected".to_string()
                                },
                                error_text,
                                capital_before_usd: Some(capital.active_deployed_usd),
                                capital_after_usd: Some(capital.active_deployed_usd),
                                reward_share: None,
                                payload_json: serde_json::to_string(response).ok(),
                            });
                        }

                        for desired in plan.place_quotes.iter().skip(responses.len()) {
                            let _ = self.analysis_store.append(&AnalysisOperation {
                                timestamp_ms: now_ms(),
                                operation_type: "quote_submit_result".to_string(),
                                condition_id: Some(
                                    desired
                                        .quote_id
                                        .as_str()
                                        .split(':')
                                        .next()
                                        .unwrap_or_default()
                                        .to_string(),
                                ),
                                asset_id: Some(desired.asset_id.clone()),
                                quote_id: Some(desired.quote_id.to_string()),
                                client_order_id: None,
                                exchange_order_id: None,
                                side: Some(format!("{:?}", desired.side)),
                                requested_price: Some(desired.price.clone()),
                                requested_size: Some(desired.size.clone()),
                                result_status: "missing_response".to_string(),
                                error_text: Some("place_quotes missing batch response".to_string()),
                                capital_before_usd: Some(capital_before.active_deployed_usd),
                                capital_after_usd: Some(capital_before.active_deployed_usd),
                                reward_share: None,
                                payload_json: None,
                            });
                        }
                    }
                    Err(err) => {
                        batch_errors.push(format!("place_quotes:{err}"));
                        for desired in &plan.place_quotes {
                            let _ = self.analysis_store.append(&AnalysisOperation {
                                timestamp_ms: now_ms(),
                                operation_type: "quote_submit_result".to_string(),
                                condition_id: Some(
                                    desired
                                        .quote_id
                                        .as_str()
                                        .split(':')
                                        .next()
                                        .unwrap_or_default()
                                        .to_string(),
                                ),
                                asset_id: Some(desired.asset_id.clone()),
                                quote_id: Some(desired.quote_id.to_string()),
                                client_order_id: None,
                                exchange_order_id: None,
                                side: Some(format!("{:?}", desired.side)),
                                requested_price: Some(desired.price.clone()),
                                requested_size: Some(desired.size.clone()),
                                result_status: "request_error".to_string(),
                                error_text: Some(err.clone()),
                                capital_before_usd: Some(capital_before.active_deployed_usd),
                                capital_after_usd: Some(capital_before.active_deployed_usd),
                                reward_share: None,
                                payload_json: None,
                            });
                        }
                    }
                }
            }
        }

        let capital_after = self.capital_snapshot();
        let _ = self.analysis_store.append(&AnalysisOperation {
            timestamp_ms: now_ms(),
            operation_type: "quote_command_batch".to_string(),
            condition_id: None,
            asset_id: None,
            quote_id: None,
            client_order_id: None,
            exchange_order_id: None,
            side: None,
            requested_price: None,
            requested_size: None,
            result_status: format!("commands={}", commands.len()),
            error_text: (!batch_errors.is_empty()).then(|| batch_errors.join("; ")),
            capital_before_usd: Some(capital_before.active_deployed_usd),
            capital_after_usd: Some(capital_after.active_deployed_usd),
            reward_share: None,
            payload_json: serde_json::to_string(&plan.place_quotes).ok(),
        });
    }

    async fn fail_closed(&mut self, reason: &str) {
        self.user_feed_state.mark_degraded(reason.to_string());
        let capital = self.capital_snapshot();
        for quote in &mut self.working {
            if quote.is_cancelable() {
                quote.mark_unknown_or_stale(reason.to_string(), now_ms());
            }
        }
        if !self.dry_run {
            let _ = self.quote_client.cancel_all_orders(&self.creds).await;
        }
        self.last_heartbeat_id = None;
        self.last_desired = DesiredQuotes::default();
        let _ = self.analysis_store.append(&AnalysisOperation {
            timestamp_ms: now_ms(),
            operation_type: "kill_switch".to_string(),
            condition_id: None,
            asset_id: None,
            quote_id: None,
            client_order_id: None,
            exchange_order_id: None,
            side: None,
            requested_price: None,
            requested_size: None,
            result_status: "cancel_all".to_string(),
            error_text: Some(reason.to_string()),
            capital_before_usd: Some(capital.active_deployed_usd),
            capital_after_usd: Some(capital.active_deployed_usd),
            reward_share: None,
            payload_json: None,
        });
    }

    fn has_live_quotes(&self) -> bool {
        self.working.iter().any(WorkingQuote::is_cancelable)
    }

    fn capital_snapshot(&self) -> capital::DeploymentSnapshot {
        capital::deployment_snapshot(
            self.budget_limit_usd,
            &self.working,
            &self.runtime.inventory_positions(),
        )
    }
}

async fn run_quote_mode(
    config: ExecutorConfig,
    l2_creds: rtt_core::clob_auth::L2Credentials,
    signer_params: Option<execution::SignerParams>,
) {
    let portfolio = discover_quote_portfolio(&config)
        .await
        .unwrap_or_else(|err| {
            tracing::error!("Failed to discover reward portfolio: {}", err);
            std::process::exit(1);
        });

    let analysis_store =
        AnalysisStore::open(&config.quote_mode.analysis_db_path).unwrap_or_else(|e| {
            tracing::error!("Failed to open analysis store: {}", e);
            std::process::exit(1);
        });
    for decision in &portfolio.selected_markets {
        let _ = analysis_store.append(&AnalysisOperation {
            timestamp_ms: now_ms(),
            operation_type: "startup_market_selected".to_string(),
            condition_id: decision.market.condition_id.clone(),
            asset_id: None,
            quote_id: None,
            client_order_id: None,
            exchange_order_id: None,
            side: None,
            requested_price: None,
            requested_size: None,
            result_status: format!("reserved_usd={:.4}", decision.reserved_capital_usd),
            error_text: None,
            capital_before_usd: Some(0.0),
            capital_after_usd: Some(decision.reserved_capital_usd),
            reward_share: None,
            payload_json: serde_json::to_string(&decision.market.market_id).ok(),
        });
    }

    let hot_store = HotStateStore::new();
    for market in &portfolio.market_meta {
        hot_store.register_market(market);
    }

    let quote_strategy = config
        .strategy
        .build_quote_strategy(portfolio.quote_markets.clone())
        .unwrap_or_else(|e| {
            tracing::error!("Failed to build quote strategy: {}", e);
            std::process::exit(1);
        });
    let (_notice_tx, notice_rx) = tokio::sync::mpsc::channel::<UpdateNotice>(1);
    let (quote_tx, _quote_rx) = tokio::sync::mpsc::channel::<DesiredQuotes>(1);
    let runtime = QuoteRuntime::new(quote_strategy, hot_store.clone(), notice_rx, quote_tx);

    let params = config.strategy.liquidity_rewards_params();
    let mut controller = QuoteModeController {
        runtime,
        hot_store: hot_store.clone(),
        order_manager: LocalOrderManager::new(ReconciliationPolicy::default()),
        user_feed_state: UserFeedState::default(),
        analysis_store,
        quote_client: execution::QuoteApiClient::with_client_and_base_url(
            reqwest::Client::new(),
            config.quote_mode.clob_base_url.clone(),
        ),
        creds: l2_creds.clone(),
        signer_params,
        dry_run: config.execution.dry_run,
        budget_limit_usd: params.max_total_deployed_usd,
        maker_address: config.credentials.maker_address.clone(),
        working: Vec::new(),
        last_desired: DesiredQuotes::default(),
        last_notice: None,
        last_heartbeat_id: None,
    };

    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(512);
    let mut pipeline = pm_data::Pipeline::new(
        portfolio.asset_ids.clone(),
        config.websocket.ws_channel_capacity,
        config.websocket.snapshot_channel_capacity,
    );
    let mut updates_rx = pipeline.subscribe_updates();
    let ws_last_message_at = pipeline.ws_client_last_message_at();
    let ws_reconnect_count = pipeline.ws_client_reconnect_count();

    let ws_handle = tokio::spawn(async move {
        pipeline.run().await;
    });

    let shutdown_rx_updates = shutdown_tx.subscribe();
    let update_tx = event_tx.clone();
    let update_handle = tokio::spawn(async move {
        let mut shutdown = shutdown_rx_updates;
        loop {
            tokio::select! {
                result = updates_rx.recv() => {
                    match result {
                        Ok(update) => {
                            if update_tx.send(QuoteModeEvent::MarketUpdate(update)).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Quote update bridge lagged, missed {n} updates");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    });

    let user_feed_handle = if !config.execution.dry_run {
        let shutdown_rx_user = shutdown_tx.subscribe();
        let user_tx = event_tx.clone();
        Some(tokio::spawn(async move {
            let (user_event_tx, mut user_event_rx) = tokio::sync::mpsc::channel(128);
            let relay = tokio::spawn(async move {
                while let Some(event) = user_event_rx.recv().await {
                    if user_tx.send(QuoteModeEvent::UserFeed(event)).await.is_err() {
                        break;
                    }
                }
            });
            user_feed::run_user_feed(
                config.quote_mode.user_ws_url.clone(),
                l2_creds,
                portfolio.condition_ids.clone(),
                user_event_tx,
                shutdown_rx_user,
            )
            .await;
            let _ = relay.await;
        }))
    } else {
        None
    };

    let state_path = std::path::Path::new(&config.execution.state_file);
    let prev_state = state::ExecutorState::load(state_path);
    let circuit_breaker = safety::CircuitBreaker::with_initial_counts(
        config.safety.max_orders,
        config.safety.max_usd_exposure,
        prev_state.orders_fired,
        prev_state.usd_committed_cents,
    );
    let shutdown_rx_health = shutdown_tx.subscribe();
    let health_handle = tokio::spawn(health::run_health_monitor(
        portfolio.asset_ids.clone(),
        Some(circuit_breaker.clone()),
        shutdown_rx_health,
        Some(config.execution.state_file.clone()),
    ));

    let _health_server_handle = if config.health.enabled {
        let shutdown_rx_hs = shutdown_tx.subscribe();
        let start_time = Instant::now();
        Some(tokio::spawn(health_server::run_health_server(
            config.health.port,
            circuit_breaker.clone(),
            ws_last_message_at,
            ws_reconnect_count,
            start_time,
            shutdown_rx_hs,
        )))
    } else {
        None
    };

    let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(
        config.quote_mode.heartbeat_interval_secs,
    ));
    let mut reward_interval = tokio::time::interval(std::time::Duration::from_secs(
        config.quote_mode.reward_poll_interval_secs,
    ));
    let mut rebate_interval = tokio::time::interval(std::time::Duration::from_secs(
        config.quote_mode.rebate_poll_interval_secs,
    ));

    tracing::info!(
        selected_markets = portfolio.selected_markets.len(),
        assets = ?portfolio.asset_ids,
        "Liquidity rewards quote mode started"
    );

    loop {
        tokio::select! {
            Some(event) = event_rx.recv() => {
                match event {
                    QuoteModeEvent::MarketUpdate(update) => controller.handle_market_update(update).await,
                    QuoteModeEvent::UserFeed(event) => controller.handle_user_feed_event(event).await,
                }
            }
            _ = heartbeat_interval.tick() => {
                controller.heartbeat().await;
            }
            _ = reward_interval.tick() => {
                controller.sample_reward_percentages().await;
            }
            _ = rebate_interval.tick() => {
                controller.sample_rebates().await;
            }
            signal = tokio::signal::ctrl_c() => {
                if signal.is_ok() {
                    break;
                }
            }
        }
    }

    let _ = shutdown_tx.send(true);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let _ = ws_handle.await;
        let _ = update_handle.await;
        if let Some(handle) = user_feed_handle {
            let _ = handle.await;
        }
        let _ = health_handle.await;
    })
    .await;
    persist_final_state(&config.execution.state_file, &circuit_breaker);
}

async fn discover_quote_portfolio(
    config: &ExecutorConfig,
) -> Result<SelectedQuotePortfolio, String> {
    let registry = pm_data::market_registry::MarketRegistry::new(
        pm_data::registry_provider::GammaRegistryProvider::new("gamma-primary"),
        pm_data::market_registry::RegistryRefreshPolicy {
            page_size: 500,
            refresh_interval: std::time::Duration::from_secs(300),
            retry_policy: pm_data::market_registry::RetryPolicy {
                max_retries: 3,
                initial_backoff: std::time::Duration::from_millis(100),
                max_backoff: std::time::Duration::from_secs(2),
            },
        },
        pm_data::snapshot::UniverseSelectionPolicy {
            active_only: true,
            ..Default::default()
        },
    );
    let refresh = registry.refresh_once().await.map_err(|err| err.message)?;

    let reward_provider =
        pm_data::registry_provider::PolymarketRewardProvider::with_client_and_base_url(
            reqwest::Client::new(),
            config.quote_mode.clob_base_url.clone(),
        );
    let current_configs = reward_provider
        .fetch_current_reward_configs()
        .await
        .map_err(|err| err.to_string())?;

    let discovery_markets: Vec<_> = refresh
        .snapshot
        .markets
        .values()
        .cloned()
        .map(|market| pm_data::market_registry::RewardDiscoveryMarket {
            accepting_orders: market.is_tradable(),
            end_time_ms: None,
            market,
        })
        .collect();

    let mut raw_rewards = Vec::new();
    for config_row in &current_configs {
        let mut rows = reward_provider
            .fetch_raw_market_rewards(&config_row.condition_id)
            .await
            .map_err(|err| err.to_string())?;
        raw_rewards.append(&mut rows);
    }

    let enriched = pm_data::market_registry::enrich_reward_markets(
        &discovery_markets,
        &current_configs,
        &raw_rewards,
    );
    let params = config.strategy.liquidity_rewards_params();
    let selection = pm_data::market_registry::select_reward_markets(
        &enriched,
        &pm_data::market_registry::RewardSelectionPolicy {
            max_markets: params.max_markets,
            max_total_deployed_usd: params.max_total_deployed_usd,
            base_quote_size: params.base_quote_size,
            edge_buffer: params.edge_buffer,
            min_total_daily_rate: params.min_total_daily_rate,
            max_market_competitiveness: params.max_market_competitiveness,
            min_time_to_expiry_secs: params.min_time_to_expiry_secs,
            max_reward_age_ms: 300_000,
        },
        now_ms(),
    );

    if selection.selected.is_empty() {
        return Err("no eligible liquidity reward markets selected".to_string());
    }

    let mut quote_markets = Vec::new();
    let mut market_meta = Vec::new();
    let mut asset_ids = Vec::new();
    let mut condition_ids = Vec::new();
    for selected in &selection.selected {
        let market = &selected.market;
        let reward = market.reward.as_ref().ok_or_else(|| {
            format!(
                "selected market {} missing reward metadata",
                market.market_id
            )
        })?;
        let condition_id = market
            .condition_id
            .clone()
            .ok_or_else(|| format!("selected market {} missing condition id", market.market_id))?;
        quote_markets.push(LiquidityRewardsMarket {
            condition_id: condition_id.clone(),
            yes_asset_id: market.yes_asset.asset_id.to_string(),
            no_asset_id: market.no_asset.asset_id.to_string(),
            tick_size: market.tick_size.to_string(),
            min_order_size: market.min_order_size.as_ref().map(ToString::to_string),
            reward_max_spread: reward
                .max_spread
                .as_ref()
                .ok_or_else(|| {
                    format!(
                        "selected market {} missing reward max spread",
                        market.market_id
                    )
                })?
                .to_string(),
            reward_min_size: reward
                .min_size
                .as_ref()
                .ok_or_else(|| {
                    format!(
                        "selected market {} missing reward min size",
                        market.market_id
                    )
                })?
                .to_string(),
            end_time_ms: selected.end_time_ms,
        });
        market_meta.push(market.clone());
        condition_ids.push(condition_id);
        asset_ids.push(market.yes_asset.asset_id.to_string());
        asset_ids.push(market.no_asset.asset_id.to_string());
    }

    Ok(SelectedQuotePortfolio {
        selected_markets: selection.selected,
        quote_markets,
        market_meta,
        asset_ids,
        condition_ids,
    })
}

fn upsert_working_quote(
    working: &mut Vec<WorkingQuote>,
    desired: pm_strategy::quote::DesiredQuote,
    now_ms: u64,
) {
    working.retain(|quote| quote.quote_id != desired.quote_id);
    working.push(WorkingQuote::pending_submit(desired, now_ms));
}

#[cfg(test)]
mod tests {
    use alloy::primitives::address;

    use super::{derive_signature_type, resolve_signature_type};

    #[test]
    fn derive_signature_type_uses_eoa_when_maker_and_signer_match() {
        let addr = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

        let sig_type = derive_signature_type(addr, addr);

        assert_eq!(sig_type, rtt_core::clob_order::SignatureType::Eoa);
    }

    #[test]
    fn derive_signature_type_uses_gnosis_safe_for_proxy_wallets() {
        let maker = address!("1111111111111111111111111111111111111111");
        let signer = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

        let sig_type = derive_signature_type(maker, signer);

        assert_eq!(sig_type, rtt_core::clob_order::SignatureType::GnosisSafe);
    }

    #[test]
    fn resolve_signature_type_prefers_explicit_fire_sh_override() {
        let maker = address!("1111111111111111111111111111111111111111");
        let signer = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

        let sig_type = resolve_signature_type(Some(2), maker, signer);

        assert_eq!(sig_type, rtt_core::clob_order::SignatureType::GnosisSafe);
    }

    #[test]
    fn resolve_signature_type_falls_back_to_derived_when_override_is_invalid() {
        let maker = address!("1111111111111111111111111111111111111111");
        let signer = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

        let sig_type = resolve_signature_type(Some(7), maker, signer);

        assert_eq!(sig_type, rtt_core::clob_order::SignatureType::GnosisSafe);
    }
}

fn persist_final_state(state_path: &str, circuit_breaker: &safety::CircuitBreaker) {
    let path = std::path::Path::new(state_path);
    let (final_orders, final_usd) = circuit_breaker.stats();
    let final_usd_cents = (final_usd * 100.0) as u64;
    let final_state = state::ExecutorState::from_stats(
        final_orders,
        final_usd_cents,
        circuit_breaker.is_tripped(),
    );
    if let Err(e) = final_state.save(path) {
        tracing::error!(error = %e, "Failed to save state on shutdown");
    } else {
        tracing::info!(
            orders = final_orders,
            usd = format!("{:.2}", final_usd),
            path = %path.display(),
            "State persisted"
        );
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
