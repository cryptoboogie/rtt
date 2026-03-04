use std::time::Duration;
use tokio::sync::watch;

/// Periodic health reporter. Logs component status at regular intervals.
pub async fn run_health_monitor(
    asset_ids: Vec<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.tick().await; // consume first immediate tick

    loop {
        tokio::select! {
            _ = interval.tick() => {
                tracing::info!(
                    monitored_assets = asset_ids.len(),
                    "Health check: pipeline running"
                );
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("Health monitor shutting down");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn health_monitor_stops_on_shutdown() {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_health_monitor(vec![], shutdown_rx));

        let _ = shutdown_tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "Health monitor should stop on shutdown");
    }
}
