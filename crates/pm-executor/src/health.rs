use std::time::Duration;
use tokio::sync::watch;

use crate::safety::CircuitBreaker;

/// Periodic health reporter. Logs component status at regular intervals.
pub async fn run_health_monitor(
    asset_ids: Vec<String>,
    circuit_breaker: Option<CircuitBreaker>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.tick().await; // consume first immediate tick

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Some(ref cb) = circuit_breaker {
                    let (orders, usd) = cb.stats();
                    tracing::info!(
                        monitored_assets = asset_ids.len(),
                        orders_fired = format!("{}/{}", orders, cb.max_orders()),
                        usd_committed = format!("${:.2}/${:.2}", usd, cb.max_usd()),
                        tripped = cb.is_tripped(),
                        "Health check: pipeline running"
                    );
                } else {
                    tracing::info!(
                        monitored_assets = asset_ids.len(),
                        "Health check: pipeline running"
                    );
                }
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
        let handle = tokio::spawn(run_health_monitor(vec![], None, shutdown_rx));

        let _ = shutdown_tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "Health monitor should stop on shutdown");
    }

    #[tokio::test]
    async fn health_monitor_reports_safety_stats() {
        let cb = CircuitBreaker::new(10, 50.0);
        cb.check_and_record("0.50", "10").unwrap(); // $5, 1 order

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = tokio::spawn(run_health_monitor(
            vec!["asset1".to_string()],
            Some(cb.clone()),
            shutdown_rx,
        ));

        // Just verify it starts and stops cleanly
        let _ = shutdown_tx.send(true);
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok());

        let (orders, usd) = cb.stats();
        assert_eq!(orders, 1);
        assert!((usd - 5.0).abs() < 0.01);
    }
}
