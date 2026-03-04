mod bridge;
mod config;
mod execution;
mod health;
mod logging;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use config::ExecutorConfig;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let config_path = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .or_else(|| args.get(1).filter(|a| !a.starts_with('-')).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("config.toml"));

    let config = ExecutorConfig::load(&config_path).unwrap_or_else(|e| {
        eprintln!("Failed to load config from {}: {}", config_path.display(), e);
        std::process::exit(1);
    });

    logging::init(&config.logging);

    tracing::info!("Starting pm-executor");

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    rt.block_on(run(config));
}

async fn run(config: ExecutorConfig) {
    tracing::info!(
        markets = ?config.websocket.asset_ids,
        strategy = %config.strategy.strategy,
        pool_size = config.connection.pool_size,
        dry_run = config.execution.dry_run,
        "Pipeline configuration loaded"
    );

    // Build credentials (validates for live mode)
    let (l2_creds, signer) =
        execution::build_credentials(&config.credentials, config.execution.dry_run)
            .unwrap_or_else(|e| {
                tracing::error!("Credential error: {}", e);
                std::process::exit(1);
            });

    // Build strategy
    let strategy = config.strategy.build_strategy().unwrap_or_else(|e| {
        tracing::error!("Failed to build strategy: {}", e);
        std::process::exit(1);
    });

    // Parse address family
    let af = match config.connection.address_family.as_str() {
        "ipv4" => rtt_core::connection::AddressFamily::V4,
        "ipv6" => rtt_core::connection::AddressFamily::V6,
        _ => rtt_core::connection::AddressFamily::Auto,
    };

    // Warm connection pool (only in live mode)
    let mut conn_pool = rtt_core::connection::ConnectionPool::new(
        "clob.polymarket.com",
        443,
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

    // Pre-sign orders (only in live mode)
    let payloads = if !config.execution.dry_run {
        let signer = signer.expect("signer required for live mode");
        let signer_addr = signer.address();

        // Build a trigger template for pre-signing
        let presign_trigger = rtt_core::trigger::TriggerMessage {
            trigger_id: 0,
            token_id: config.strategy.token_id.clone(),
            side: config.strategy.side,
            price: format!(
                "{:.2}",
                config
                    .strategy
                    .params
                    .threshold
                    .unwrap_or(0.50)
            ),
            size: config.strategy.size.clone(),
            order_type: config.strategy.order_type,
            timestamp_ns: 0,
        };

        rtt_core::clob_signer::presign_batch(
            &signer,
            &presign_trigger,
            signer_addr,
            signer_addr,
            config.execution.fee_rate_bps,
            config.execution.is_neg_risk,
            &config.credentials.api_key,
            config.execution.presign_count,
        )
        .await
        .expect("Failed to pre-sign orders")
    } else {
        vec![]
    };

    let presigned_pool = rtt_core::clob_executor::PreSignedOrderPool::new(payloads)
        .expect("Failed to create pre-signed order pool");

    tracing::info!(
        presigned_count = presigned_pool.len(),
        dry_run = config.execution.dry_run,
        "Execution setup complete"
    );

    // Create channels
    let (snapshot_mpsc_tx, snapshot_mpsc_rx) = tokio::sync::mpsc::channel(256);
    let (trigger_mpsc_tx, trigger_mpsc_rx) = tokio::sync::mpsc::channel(256);
    let (trigger_crossbeam_tx, trigger_crossbeam_rx) = crossbeam_channel::bounded(256);

    // Create shutdown signal
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

    // Spawn execution thread (dedicated OS thread, not tokio)
    let exec_shutdown = Arc::new(AtomicBool::new(false));
    let exec_shutdown_clone = exec_shutdown.clone();
    let conn_pool = Arc::new(conn_pool);
    let exec_dry_run = config.execution.dry_run;
    let exec_creds = l2_creds;
    let exec_handle = std::thread::spawn(move || {
        execution::run_execution_loop(
            trigger_crossbeam_rx,
            conn_pool,
            presigned_pool,
            exec_creds,
            exec_dry_run,
            exec_shutdown_clone,
        );
    });

    // Start WebSocket pipeline
    let mut pipeline = pm_data::Pipeline::new(
        config.websocket.asset_ids.clone(),
        config.websocket.ws_channel_capacity,
        config.websocket.snapshot_channel_capacity,
    );
    let snapshot_broadcast_rx = pipeline.subscribe_snapshots();

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
    let mut runner = pm_strategy::runner::StrategyRunner::new(
        strategy,
        snapshot_mpsc_rx,
        trigger_mpsc_tx,
    );
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

    // Start health monitoring
    let shutdown_rx_health = shutdown_tx.subscribe();
    let health_handle = tokio::spawn(health::run_health_monitor(
        config.websocket.asset_ids.clone(),
        shutdown_rx_health,
    ));

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

    tracing::info!("pm-executor shut down cleanly");
}
