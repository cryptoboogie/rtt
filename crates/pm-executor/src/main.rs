mod alert;
mod btc5m;
mod bridge;
mod config;
mod execution;
mod health;
mod health_server;
mod journal;
mod logging;
mod order_manager;
mod order_state;
mod safety;
mod state;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use config::ExecutorConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliOptions {
    validate_creds_only: bool,
    config_path: PathBuf,
    dry_run_override: Option<bool>,
}

fn parse_cli_options(args: &[String]) -> Result<CliOptions, String> {
    let mut validate_creds_only = false;
    let mut config_path = None;
    let mut dry_run_override = None;
    let mut index = 1;

    while index < args.len() {
        let arg = &args[index];

        if arg == "--validate-creds" {
            validate_creds_only = true;
            index += 1;
            continue;
        }

        if arg == "--config" {
            let path = args
                .get(index + 1)
                .ok_or_else(|| "--config requires a following path".to_string())?;
            config_path = Some(PathBuf::from(path));
            index += 2;
            continue;
        }

        if let Some(path) = arg.strip_prefix("--config=") {
            config_path = Some(PathBuf::from(path));
            index += 1;
            continue;
        }

        if arg == "--live" {
            set_dry_run_override(&mut dry_run_override, false, "--live")?;
            index += 1;
            continue;
        }

        if arg == "--dry-run" {
            if let Some(value) = args.get(index + 1).and_then(|value| parse_bool_token(value)) {
                set_dry_run_override(&mut dry_run_override, value, "--dry-run")?;
                index += 2;
            } else {
                set_dry_run_override(&mut dry_run_override, true, "--dry-run")?;
                index += 1;
            }
            continue;
        }

        if let Some(value) = parse_dry_run_assignment(arg) {
            set_dry_run_override(&mut dry_run_override, value, arg)?;
            index += 1;
            continue;
        }

        if arg.starts_with('-') {
            return Err(format!("unrecognized argument: {arg}"));
        }

        if config_path.is_none() {
            config_path = Some(PathBuf::from(arg));
            index += 1;
            continue;
        }

        return Err(format!("unexpected positional argument: {arg}"));
    }

    Ok(CliOptions {
        validate_creds_only,
        config_path: config_path.unwrap_or_else(|| PathBuf::from("config.toml")),
        dry_run_override,
    })
}

fn parse_dry_run_assignment(arg: &str) -> Option<bool> {
    let value = arg
        .strip_prefix("--dry-run=")
        .or_else(|| arg.strip_prefix("dry_run="))
        .or_else(|| arg.strip_prefix("execution.dry_run="))?;
    parse_bool_token(value)
}

fn parse_bool_token(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn set_dry_run_override(
    slot: &mut Option<bool>,
    value: bool,
    source: &str,
) -> Result<(), String> {
    match slot {
        Some(existing) if *existing != value => Err(format!(
            "conflicting dry-run overrides: already set to {}, cannot also apply {}",
            existing, source
        )),
        Some(_) => Ok(()),
        None => {
            *slot = Some(value);
            Ok(())
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cli = parse_cli_options(&args).unwrap_or_else(|e| {
        eprintln!("Failed to parse CLI arguments: {e}");
        std::process::exit(2);
    });

    let mut config = ExecutorConfig::load(&cli.config_path).unwrap_or_else(|e| {
        eprintln!(
            "Failed to load config from {}: {}",
            cli.config_path.display(),
            e
        );
        std::process::exit(1);
    });

    if let Some(dry_run_override) = cli.dry_run_override {
        config.execution.dry_run = dry_run_override;
    }

    logging::init(&config.logging);

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    if cli.validate_creds_only {
        tracing::info!("Validating credentials...");
        let l2_creds = execution::build_validation_credentials(&config.credentials)
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

    tracing::info!(
        dry_run = config.execution.dry_run,
        cli_override = cli.dry_run_override.is_some(),
        "Starting pm-executor"
    );
    rt.block_on(run(config));
}

async fn run(config: ExecutorConfig) {
    let uses_specialized_runtime = config.strategy.uses_specialized_runtime();

    // Build credentials (validates for live mode)
    let (l2_creds, signer) =
        execution::build_credentials(&config.credentials, config.execution.dry_run).unwrap_or_else(
            |e| {
                tracing::error!("Credential error: {}", e);
                std::process::exit(1);
            },
        );

    // Validate credentials against live API (only in live mode)
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

    // Parse address family
    let af = match config.connection.address_family.as_str() {
        "ipv4" => rtt_core::connection::AddressFamily::V4,
        "ipv6" => rtt_core::connection::AddressFamily::V6,
        _ => rtt_core::connection::AddressFamily::Auto,
    };

    // Warm connection pool (only in live mode)
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

    // Build signer params for dynamic pricing (sign at trigger's price)
    let signer_params = if !config.execution.dry_run {
        let signer = signer.expect("signer required for live mode");
        let signer_addr = signer.address();
        let maker_addr: alloy::primitives::Address = config
            .credentials
            .maker_address
            .parse()
            .expect("invalid maker_address in config");

        Some(execution::SignerParams {
            signer,
            maker: maker_addr,
            signer_addr,
            fee_rate_bps: config.execution.fee_rate_bps,
            is_neg_risk: config.execution.is_neg_risk,
            sig_type: rtt_core::clob_order::SignatureType::Poly,
            owner: config.credentials.api_key.clone(),
        })
    } else {
        None
    };

    if uses_specialized_runtime {
        run_btc5m(config, l2_creds, Arc::new(conn_pool), signer_params).await;
        return;
    }

    let monitored_assets = config.resolved_subscription_asset_ids();

    tracing::info!(
        markets = ?monitored_assets,
        strategy = %config.strategy.strategy,
        pool_size = config.connection.pool_size,
        dry_run = config.execution.dry_run,
        "Pipeline configuration loaded"
    );

    // Build strategy
    let strategy = config.strategy.build_strategy().unwrap_or_else(|e| {
        tracing::error!("Failed to build strategy: {}", e);
        std::process::exit(1);
    });

    // Empty pre-signed pool (kept for backwards compatibility / future use)
    let presigned_pool = rtt_core::clob_executor::PreSignedOrderPool::new(vec![])
        .expect("Failed to create pre-signed order pool");

    tracing::info!(
        dynamic_signing = signer_params.is_some(),
        dry_run = config.execution.dry_run,
        "Execution setup complete"
    );

    // Create channels
    let (snapshot_mpsc_tx, snapshot_mpsc_rx) = tokio::sync::mpsc::channel(256);
    let (trigger_mpsc_tx, trigger_mpsc_rx) = tokio::sync::mpsc::channel(256);
    let (trigger_crossbeam_tx, trigger_crossbeam_rx) = crossbeam_channel::bounded(256);

    // Create shutdown signal
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

    // Load persisted state
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

    // Build safety rails (with restored counters)
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

    // Spawn execution thread (dedicated OS thread, not tokio)
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

    // Start WebSocket pipeline
    let pipeline = pm_data::Pipeline::new(
        monitored_assets.clone(),
        config.websocket.ws_channel_capacity,
        config.websocket.snapshot_channel_capacity,
    );
    let snapshot_broadcast_rx = pipeline.subscribe_snapshots();
    // Grab WS metric arcs before moving pipeline into the spawn
    let ws_last_message_at = pipeline.ws_client_last_message_at();
    let ws_reconnect_count = pipeline.ws_client_reconnect_count();

    let ws_handle = tokio::spawn(async move {
        pipeline.run().await;
    });

    // Start broadcast → mpsc bridge
    let shutdown_rx_bridge = shutdown_tx.subscribe();
    let bridge_handle = tokio::spawn(bridge::broadcast_to_mpsc(
        snapshot_broadcast_rx,
        snapshot_mpsc_tx,
        shutdown_rx_bridge,
    ));

    // Start strategy runner
    let mut runner =
        pm_strategy::runner::StrategyRunner::new(strategy, snapshot_mpsc_rx, trigger_mpsc_tx);
    let strategy_handle = tokio::spawn(async move {
        runner.run().await;
    });

    // Start mpsc → crossbeam bridge
    let shutdown_rx_trigger = shutdown_tx.subscribe();
    let trigger_bridge_handle = tokio::spawn(bridge::mpsc_to_crossbeam(
        trigger_mpsc_rx,
        trigger_crossbeam_tx,
        shutdown_rx_trigger,
    ));

    // Start health monitoring (log-based)
    let shutdown_rx_health = shutdown_tx.subscribe();
    let health_cb = circuit_breaker.clone();
    let health_state_path = config.execution.state_file.clone();
    let health_handle = tokio::spawn(health::run_health_monitor(
        monitored_assets.clone(),
        Some(health_cb),
        shutdown_rx_health,
        Some(health_state_path),
    ));

    // Start HTTP health endpoint
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

    // Wait for shutdown
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl_c");

    tracing::info!("Shutdown signal received. Stopping components...");
    let _ = shutdown_tx.send(true);

    // Signal execution thread to stop
    exec_shutdown.store(true, Ordering::Relaxed);

    // Wait for async components to finish
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let _ = ws_handle.await;
        let _ = bridge_handle.await;
        let _ = strategy_handle.await;
        let _ = trigger_bridge_handle.await;
        let _ = health_handle.await;
    })
    .await;

    // Wait for execution thread
    let _ = exec_handle.join();

    // Save state on shutdown
    let (final_orders, final_usd) = circuit_breaker.stats();
    let final_usd_cents = (final_usd * 100.0) as u64;
    let final_state = state::ExecutorState::from_stats(
        final_orders,
        final_usd_cents,
        circuit_breaker.is_tripped(),
    );
    if let Err(e) = final_state.save(state_path) {
        tracing::error!(error = %e, "Failed to save state on shutdown");
    } else {
        tracing::info!(
            orders = final_orders,
            usd = format!("{:.2}", final_usd),
            path = %state_path.display(),
            "State persisted"
        );
    }

    tracing::info!("pm-executor shut down cleanly");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parse_cli_options_defaults_to_config_toml_without_override() {
        let cli = parse_cli_options(&args(&["pm-executor"])).unwrap();
        assert_eq!(cli.config_path, PathBuf::from("config.toml"));
        assert!(!cli.validate_creds_only);
        assert_eq!(cli.dry_run_override, None);
    }

    #[test]
    fn parse_cli_options_accepts_live_flag() {
        let cli = parse_cli_options(&args(&["pm-executor", "--live"])).unwrap();
        assert_eq!(cli.dry_run_override, Some(false));
    }

    #[test]
    fn parse_cli_options_accepts_dry_run_assignment_alias() {
        let cli = parse_cli_options(&args(&[
            "pm-executor",
            "--config",
            "prod.toml",
            "dry_run=false",
        ]))
        .unwrap();
        assert_eq!(cli.config_path, PathBuf::from("prod.toml"));
        assert_eq!(cli.dry_run_override, Some(false));
    }

    #[test]
    fn parse_cli_options_accepts_explicit_dry_run_value() {
        let cli = parse_cli_options(&args(&["pm-executor", "--dry-run", "false"])).unwrap();
        assert_eq!(cli.dry_run_override, Some(false));
    }

    #[test]
    fn parse_cli_options_rejects_conflicting_live_and_dry_run_flags() {
        let error = parse_cli_options(&args(&["pm-executor", "--live", "--dry-run=true"]))
            .unwrap_err();
        assert!(error.contains("conflicting"));
    }
}

async fn run_btc5m(
    config: ExecutorConfig,
    l2_creds: rtt_core::clob_auth::L2Credentials,
    conn_pool: Arc<rtt_core::connection::ConnectionPool>,
    signer_params: Option<execution::SignerParams>,
) {
    let params = config.strategy.btc_5m_params().unwrap_or_else(|e| {
        tracing::error!("Failed to parse btc_5m strategy params: {}", e);
        std::process::exit(1);
    });

    let state_path = std::path::Path::new(&config.execution.state_file);
    let prev_state = state::ExecutorState::load(state_path);
    if prev_state.orders_fired > 0 || prev_state.tripped {
        tracing::warn!(
            prev_orders = prev_state.orders_fired,
            prev_usd_cents = prev_state.usd_committed_cents,
            prev_tripped = prev_state.tripped,
            last_shutdown = %prev_state.last_shutdown,
            "Restoring state from previous BTC 5m run"
        );
    }

    let circuit_breaker = safety::CircuitBreaker::with_initial_counts(
        config.safety.max_orders,
        config.safety.max_usd_exposure,
        prev_state.orders_fired,
        prev_state.usd_committed_cents,
    );

    tracing::warn!(
        max_orders = config.safety.max_orders,
        max_usd = config.safety.max_usd_exposure,
        max_triggers_per_sec = config.safety.max_triggers_per_second,
        risk_mode = ?params.risk_mode,
        probe_budget_usd = params.probe_budget_usd,
        initial_burst_budget_usd = params.initial_burst_budget_usd,
        max_pair_budget_usd = params.max_pair_budget_usd,
        max_single_side_budget_usd = params.max_single_side_budget_usd,
        max_gross_deployed_per_market = params.max_gross_deployed_per_market,
        max_unpaired_exposure_usd = params.max_unpaired_exposure_usd,
        max_cleanup_loss_usd = params.max_cleanup_loss_usd,
        carry_pair_sum_max = params.carry_pair_sum_max,
        entry_window_start_seconds = params.entry_window_start_seconds,
        entry_window_end_seconds = params.entry_window_end_seconds,
        allow_one_sided_continuation = params.allow_one_sided_continuation,
        "BTC 5m safety and strategy limits active"
    );

    let journal_path = std::path::Path::new(&config.execution.journal_db_path);
    let (journal, journal_worker) = journal::Btc5mJournal::start(journal_path).unwrap_or_else(|e| {
        tracing::error!(
            error = %e,
            path = %journal_path.display(),
            "Failed to initialize BTC 5m SQLite journal"
        );
        std::process::exit(1);
    });

    let execution_ctx = btc5m::Btc5mExecutionContext {
        dry_run: config.execution.dry_run,
        order_type: config.strategy.order_type,
        pool: conn_pool,
        creds: l2_creds,
        signer_params,
        circuit_breaker: circuit_breaker.clone(),
        rate_limiter: safety::RateLimiter::new(config.safety.max_triggers_per_second),
        order_guard: safety::OrderGuard::new(),
        journal: journal.clone(),
    };

    let mut runner = match btc5m::Btc5mRunner::new(
        params,
        config.websocket.ws_channel_capacity,
        config.websocket.snapshot_channel_capacity,
        execution_ctx,
    )
    .await
    {
        Ok(runner) => runner,
        Err(error) => {
            drop(journal);
            if let Err(join_error) = journal_worker.join() {
                tracing::error!(error = %join_error, "Failed to close BTC 5m SQLite journal after init error");
            }
            tracing::error!("Failed to initialize BTC 5m runner: {}", error);
            std::process::exit(1);
        }
    };

    let monitored_assets = runner.monitored_assets();
    tracing::info!(
        markets = ?monitored_assets,
        strategy = %config.strategy.strategy,
        pool_size = config.connection.pool_size,
        dry_run = config.execution.dry_run,
        "BTC 5m configuration loaded"
    );

    let ws_last_message_at = runner.ws_last_message_at();
    let ws_reconnect_count = runner.ws_reconnect_count();
    let start_time = Instant::now();
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);
    let runner_shutdown_rx = shutdown_tx.subscribe();
    let health_state_path = config.execution.state_file.clone();

    let runner_handle = tokio::spawn(async move { runner.run(runner_shutdown_rx).await });
    let health_shutdown_rx = shutdown_tx.subscribe();
    let health_handle = tokio::spawn(health::run_health_monitor(
        monitored_assets.clone(),
        Some(circuit_breaker.clone()),
        health_shutdown_rx,
        Some(health_state_path),
    ));

    let health_server_handle = if config.health.enabled {
        let server_shutdown_rx = shutdown_tx.subscribe();
        Some(tokio::spawn(health_server::run_health_server(
            config.health.port,
            circuit_breaker.clone(),
            ws_last_message_at,
            ws_reconnect_count,
            start_time,
            server_shutdown_rx,
        )))
    } else {
        None
    };

    tracing::info!("BTC 5m runner started. Waiting for shutdown signal (Ctrl+C)...");

    let mut runner_handle = runner_handle;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Shutdown signal received");
        }
        result = &mut runner_handle => {
            match result {
                Ok(Ok(())) => tracing::warn!("BTC 5m runner exited cleanly before shutdown signal"),
                Ok(Err(error)) => tracing::error!(error = %error, "BTC 5m runner exited with error"),
                Err(join_error) => tracing::error!(error = %join_error, "BTC 5m runner task failed"),
            }
        }
    }

    let _ = shutdown_tx.send(true);

    let shutdown_result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let _ = runner_handle.await;
        let _ = health_handle.await;
        if let Some(handle) = health_server_handle {
            let _ = handle.await;
        }
    })
    .await;

    drop(journal);
    if shutdown_result.is_ok() {
        if let Err(error) = journal_worker.join() {
            tracing::error!(
                error = %error,
                path = %journal_path.display(),
                "BTC 5m SQLite journal did not shut down cleanly"
            );
        } else {
            tracing::info!(path = %journal_path.display(), "BTC 5m SQLite journal flushed");
        }
    } else {
        tracing::warn!(
            path = %journal_path.display(),
            "Skipping BTC 5m journal join because shutdown timed out"
        );
    }

    let (orders, usd) = circuit_breaker.stats();
    let final_state =
        state::ExecutorState::from_stats(orders, (usd * 100.0) as u64, circuit_breaker.is_tripped());
    if let Err(e) = final_state.save(state_path) {
        tracing::error!(error = %e, "Failed to save final BTC 5m state");
    } else {
        tracing::info!(
            orders = orders,
            usd = format!("{:.2}", usd),
            path = %state_path.display(),
            "BTC 5m state persisted"
        );
    }

    tracing::info!("BTC 5m runner shut down cleanly");
}
