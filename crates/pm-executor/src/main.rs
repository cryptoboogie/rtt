mod bridge;
mod config;
mod health;
mod logging;

use std::path::PathBuf;

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
        "Pipeline configuration loaded"
    );

    // Build strategy
    let strategy = config.strategy.build_strategy().unwrap_or_else(|e| {
        tracing::error!("Failed to build strategy: {}", e);
        std::process::exit(1);
    });

    // Create channels
    let (snapshot_mpsc_tx, snapshot_mpsc_rx) = tokio::sync::mpsc::channel(256);
    let (trigger_mpsc_tx, trigger_mpsc_rx) = tokio::sync::mpsc::channel(256);
    let (trigger_crossbeam_tx, _trigger_crossbeam_rx) = crossbeam_channel::bounded(256);

    // Create shutdown signal
    let (shutdown_tx, _) = tokio::sync::watch::channel(false);

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

    // Wait for components to finish
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let _ = ws_handle.await;
        let _ = bridge_handle.await;
        let _ = strategy_handle.await;
        let _ = trigger_bridge_handle.await;
        let _ = health_handle.await;
    })
    .await;

    tracing::info!("pm-executor shut down cleanly");
}
