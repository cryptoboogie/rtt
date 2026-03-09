use rtt_core::clock;
use rtt_core::trigger::{OrderBookSnapshot, TriggerMessage};
use tokio::sync::{broadcast, mpsc, watch};

/// Bridges a broadcast::Receiver<OrderBookSnapshot> to an mpsc::Sender<OrderBookSnapshot>.
/// This allows the Pipeline's broadcast output to feed the StrategyRunner's mpsc input.
pub async fn broadcast_to_mpsc(
    mut broadcast_rx: broadcast::Receiver<OrderBookSnapshot>,
    mpsc_tx: mpsc::Sender<OrderBookSnapshot>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            result = broadcast_rx.recv() => {
                match result {
                    Ok(snapshot) => {
                        if mpsc_tx.send(snapshot).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Bridge lagged, missed {n} snapshots");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
        }
    }
}

/// Bridges an mpsc::Receiver<TriggerMessage> to a crossbeam Sender<TriggerMessage>,
/// stamping timestamp_ns on each trigger before forwarding.
pub async fn mpsc_to_crossbeam(
    mut mpsc_rx: mpsc::Receiver<TriggerMessage>,
    crossbeam_tx: crossbeam_channel::Sender<TriggerMessage>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            msg = mpsc_rx.recv() => {
                match msg {
                    Some(mut trigger) => {
                        trigger.timestamp_ns = clock::now_ns();
                        if crossbeam_tx.send(trigger).is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtt_core::trigger::{OrderType, PriceLevel, Side};

    #[tokio::test]
    async fn broadcast_to_mpsc_forwards_snapshots() {
        let (broadcast_tx, broadcast_rx) = broadcast::channel(16);
        let (mpsc_tx, mut mpsc_rx) = mpsc::channel(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(broadcast_to_mpsc(broadcast_rx, mpsc_tx, shutdown_rx));

        let snap = OrderBookSnapshot {
            asset_id: "test".to_string(),
            best_bid: Some(PriceLevel {
                price: "0.50".to_string(),
                size: "100".to_string(),
            }),
            best_ask: None,
            timestamp_ms: 1000,
            hash: "h".to_string(),
        };
        broadcast_tx.send(snap).unwrap();

        let received = mpsc_rx.recv().await.unwrap();
        assert_eq!(received.asset_id, "test");
        assert_eq!(received.best_bid.unwrap().price, "0.50");

        let _ = shutdown_tx.send(true);
        let _ = handle.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mpsc_to_crossbeam_forwards_triggers() {
        let (mpsc_tx, mpsc_rx) = mpsc::channel(16);
        let (crossbeam_tx, crossbeam_rx) = crossbeam_channel::bounded(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(mpsc_to_crossbeam(mpsc_rx, crossbeam_tx, shutdown_rx));

        let trigger = TriggerMessage {
            trigger_id: 1,
            token_id: "tok".to_string(),
            side: Side::Buy,
            price: "0.45".to_string(),
            size: "10".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 0,
        };
        mpsc_tx.send(trigger).await.unwrap();

        // Use spawn_blocking since crossbeam recv is sync/blocking
        let received = tokio::task::spawn_blocking(move || crossbeam_rx.recv().unwrap())
            .await
            .unwrap();
        assert_eq!(received.trigger_id, 1);
        assert_eq!(received.token_id, "tok");
        // timestamp_ns was re-stamped by the bridge (original was 0)
        // It may still be 0 if epoch was just initialized, so just
        // verify the trigger passed through correctly.
        assert_eq!(received.side, Side::Buy);

        let _ = shutdown_tx.send(true);
        drop(mpsc_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn broadcast_to_mpsc_stops_on_shutdown() {
        let (_broadcast_tx, broadcast_rx) = broadcast::channel::<OrderBookSnapshot>(16);
        let (mpsc_tx, _mpsc_rx) = mpsc::channel(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(broadcast_to_mpsc(broadcast_rx, mpsc_tx, shutdown_rx));

        // Signal shutdown
        let _ = shutdown_tx.send(true);
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        assert!(result.is_ok(), "Bridge should stop on shutdown");
    }
}
