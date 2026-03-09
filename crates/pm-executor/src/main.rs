mod alert;
mod bridge;
mod config;
mod execution;
mod health;
mod health_server;
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
        pool_size = config.connection.pool_size,
        dry_run = config.execution.dry_run,
        "Pipeline configuration loaded"
    );

    // Build credentials (validates for live mode)
    let (l2_creds, signer) =
        execution::build_credentials(&config.credentials, config.execution.dry_run).unwrap_or_else(
            |e| {
                tracing::error!("Credential error: {}", e);
                std::process::exit(1);
            },
        );

    // Build strategy
    let strategy = config.strategy.build_strategy().unwrap_or_else(|e| {
        tracing::error!("Failed to build strategy: {}", e);
        std::process::exit(1);
    });

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
    let mut pipeline = pm_data::Pipeline::new(
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
