use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::connect_async;
use tracing::{error, info, warn};

use crate::types::WsMessage;

const WS_MARKET_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
const PING_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize)]
struct SubscribeRequest {
    assets_ids: Vec<String>,
    r#type: String,
    custom_feature_enabled: bool,
}

/// Exponential backoff state for reconnection delays.
pub struct BackoffState {
    current_ms: u64,
    base_ms: u64,
    cap_ms: u64,
    jitter_ms: u64,
}

impl BackoffState {
    pub fn new() -> Self {
        Self {
            current_ms: 1000,
            base_ms: 1000,
            cap_ms: 60_000,
            jitter_ms: 500,
        }
    }

    /// Returns the next backoff delay with jitter.
    pub fn next_delay(&mut self) -> Duration {
        let delay_ms = self.current_ms;
        // Double for next time, capped
        self.current_ms = (self.current_ms * 2).min(self.cap_ms);
        // Add jitter: 0 to jitter_ms
        let jitter = if self.jitter_ms > 0 {
            rand_jitter(self.jitter_ms)
        } else {
            0
        };
        Duration::from_millis(delay_ms + jitter)
    }

    /// Reset backoff to initial value (after successful connection).
    pub fn reset(&mut self) {
        self.current_ms = self.base_ms;
    }

    /// Current base delay in ms (without jitter), for testing.
    pub fn current_ms(&self) -> u64 {
        self.current_ms
    }
}

impl Default for BackoffState {
    fn default() -> Self {
        Self::new()
    }
}

/// Simple jitter using timestamp-based pseudo-randomness (no extra deps).
fn rand_jitter(max_ms: u64) -> u64 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    seed % (max_ms + 1)
}

pub struct WsClient {
    asset_ids: Vec<String>,
    tx: broadcast::Sender<WsMessage>,
    shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
    reconnect_count: Arc<AtomicU64>,
    last_message_at: Arc<AtomicU64>,
}

impl WsClient {
    pub fn new(asset_ids: Vec<String>, channel_capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(channel_capacity);
        Self {
            asset_ids,
            tx,
            shutdown_tx: None,
            reconnect_count: Arc::new(AtomicU64::new(0)),
            last_message_at: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WsMessage> {
        self.tx.subscribe()
    }

    pub fn sender(&self) -> broadcast::Sender<WsMessage> {
        self.tx.clone()
    }

    /// Total number of reconnections since startup.
    pub fn reconnect_count(&self) -> u64 {
        self.reconnect_count.load(Ordering::Relaxed)
    }

    /// Arc to the reconnect counter (for sharing with health monitor).
    pub fn reconnect_count_arc(&self) -> Arc<AtomicU64> {
        self.reconnect_count.clone()
    }

    /// Epoch millis of the last received WS message.
    pub fn last_message_at(&self) -> u64 {
        self.last_message_at.load(Ordering::Relaxed)
    }

    /// Arc to the last_message_at counter (for sharing with health monitor).
    pub fn last_message_at_arc(&self) -> Arc<AtomicU64> {
        self.last_message_at.clone()
    }

    pub async fn run(&mut self) {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);
        let mut backoff = BackoffState::new();
        let mut first_connect = true;

        loop {
            if *shutdown_rx.borrow() {
                info!("WsClient shutdown requested");
                break;
            }

            match connect_and_run(
                &self.asset_ids,
                self.tx.clone(),
                shutdown_rx.clone(),
                self.last_message_at.clone(),
            )
            .await
            {
                Ok(()) => {
                    info!("WebSocket connection closed cleanly");
                    backoff.reset();
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
                Err(e) => {
                    error!("WebSocket error: {e}");
                }
            }

            if *shutdown_rx.borrow() {
                break;
            }

            // Track reconnection
            if !first_connect {
                let count = self.reconnect_count.fetch_add(1, Ordering::Relaxed) + 1;
                // Signal reconnect to pipeline so it clears stale state
                let _ = self.tx.send(WsMessage::Reconnected);
                let delay = backoff.next_delay();
                warn!(reconnect_count = count, delay_ms = delay.as_millis() as u64, "Reconnecting...");
                tokio::time::sleep(delay).await;
            } else {
                first_connect = false;
            }
        }
    }

    pub fn shutdown(&self) {
        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(true);
        }
    }
}

async fn connect_and_run(
    asset_ids: &[String],
    tx: broadcast::Sender<WsMessage>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    last_message_at: Arc<AtomicU64>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (ws_stream, _) = connect_async(WS_MARKET_URL).await?;
    info!("Connected to {WS_MARKET_URL}");

    let (mut write, mut read) = ws_stream.split();

    // Send subscription message
    let sub = SubscribeRequest {
        assets_ids: asset_ids.to_vec(),
        r#type: "market".to_string(),
        custom_feature_enabled: true,
    };
    let sub_json = serde_json::to_string(&sub)?;
    write.send(Message::Text(sub_json.into())).await?;
    info!("Subscribed to {} assets", asset_ids.len());

    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    ping_interval.tick().await; // consume first immediate tick

    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                if *shutdown_rx.borrow() {
                    break;
                }
                write.send(Message::Text("PING".into())).await?;
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Update last_message_at on every received message
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        last_message_at.store(now_ms, Ordering::Relaxed);

                        let text_str: &str = &text;
                        if text_str == "PONG" {
                            continue;
                        }
                        parse_and_send(text_str, &tx);
                    }
                    Some(Ok(Message::Ping(data))) => {
                        write.send(Message::Pong(data)).await?;
                    }
                    Some(Ok(Message::Close(_))) => {
                        info!("Server sent close frame");
                        break;
                    }
                    Some(Err(e)) => {
                        return Err(Box::new(e));
                    }
                    None => {
                        info!("WebSocket stream ended");
                        break;
                    }
                    _ => {}
                }
            }
            _ = async {
                let mut rx = shutdown_rx.clone();
                let _ = rx.changed().await;
            }, if !*shutdown_rx.borrow() => {
                if *shutdown_rx.borrow() {
                    info!("Shutdown during WS loop");
                    let _ = write.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    }

    Ok(())
}

/// Parse a WS text message that may be a single JSON object or a JSON array
/// (initial book dump comes as an array of book events).
fn parse_and_send(text: &str, tx: &broadcast::Sender<WsMessage>) {
    let trimmed = text.trim();
    if trimmed.starts_with('[') {
        // Array of messages (initial dump)
        match serde_json::from_str::<Vec<WsMessage>>(trimmed) {
            Ok(msgs) => {
                for msg in msgs {
                    let _ = tx.send(msg);
                }
            }
            Err(e) => {
                warn!(
                    "Failed to parse WS array message: {e}, raw: {}",
                    &trimmed[..trimmed.len().min(200)]
                );
            }
        }
    } else if trimmed.starts_with('{') {
        // Single message
        match serde_json::from_str::<WsMessage>(trimmed) {
            Ok(ws_msg) => {
                let _ = tx.send(ws_msg);
            }
            Err(e) => {
                warn!(
                    "Failed to parse WS message: {e}, raw: {}",
                    &trimmed[..trimmed.len().min(200)]
                );
            }
        }
    }
    // Ignore other formats (e.g., empty strings, "[]")
}

/// Build a subscription JSON string for testing/external use.
pub fn build_subscribe_message(asset_ids: &[String], custom_features: bool) -> String {
    let sub = SubscribeRequest {
        assets_ids: asset_ids.to_vec(),
        r#type: "market".to_string(),
        custom_feature_enabled: custom_features,
    };
    serde_json::to_string(&sub).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subscribe_message_format() {
        let ids = vec!["asset1".to_string(), "asset2".to_string()];
        let json = build_subscribe_message(&ids, true);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "market");
        assert_eq!(parsed["custom_feature_enabled"], true);
        let assets = parsed["assets_ids"].as_array().unwrap();
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0], "asset1");
        assert_eq!(assets[1], "asset2");
    }

    #[test]
    fn test_subscribe_message_no_custom_features() {
        let ids = vec!["asset1".to_string()];
        let json = build_subscribe_message(&ids, false);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["custom_feature_enabled"], false);
    }

    #[test]
    fn test_ws_client_subscribe_returns_receiver() {
        let client = WsClient::new(vec!["test".to_string()], 100);
        let _rx = client.subscribe();
        // Should be able to create multiple receivers
        let _rx2 = client.subscribe();
    }

    #[test]
    fn test_backoff_exponential_delays() {
        let mut backoff = BackoffState {
            current_ms: 1000,
            base_ms: 1000,
            cap_ms: 60_000,
            jitter_ms: 0, // disable jitter for deterministic test
        };
        assert_eq!(backoff.current_ms(), 1000);
        let d1 = backoff.next_delay();
        assert_eq!(d1, Duration::from_millis(1000));
        assert_eq!(backoff.current_ms(), 2000);
        let d2 = backoff.next_delay();
        assert_eq!(d2, Duration::from_millis(2000));
        assert_eq!(backoff.current_ms(), 4000);
        let d3 = backoff.next_delay();
        assert_eq!(d3, Duration::from_millis(4000));
        assert_eq!(backoff.current_ms(), 8000);
    }

    #[test]
    fn test_backoff_caps_at_60s() {
        let mut backoff = BackoffState {
            current_ms: 32_000,
            base_ms: 1000,
            cap_ms: 60_000,
            jitter_ms: 0,
        };
        let _ = backoff.next_delay(); // 32s, next = 64s capped to 60s
        assert_eq!(backoff.current_ms(), 60_000);
        let _ = backoff.next_delay(); // 60s, next stays 60s
        assert_eq!(backoff.current_ms(), 60_000);
    }

    #[test]
    fn test_backoff_reset() {
        let mut backoff = BackoffState {
            current_ms: 1000,
            base_ms: 1000,
            cap_ms: 60_000,
            jitter_ms: 0,
        };
        let _ = backoff.next_delay();
        let _ = backoff.next_delay();
        assert_eq!(backoff.current_ms(), 4000);
        backoff.reset();
        assert_eq!(backoff.current_ms(), 1000);
    }

    #[test]
    fn test_backoff_jitter_bounded() {
        let mut backoff = BackoffState {
            current_ms: 1000,
            base_ms: 1000,
            cap_ms: 60_000,
            jitter_ms: 500,
        };
        for _ in 0..20 {
            let d = backoff.next_delay();
            // delay should be base + [0, 500ms] jitter
            assert!(d.as_millis() >= 1000);
            assert!(d.as_millis() <= 60_500);
            backoff.reset();
        }
    }

    #[test]
    fn test_reconnect_counter_increments() {
        let client = WsClient::new(vec!["test".to_string()], 100);
        assert_eq!(client.reconnect_count(), 0);
        client.reconnect_count.fetch_add(1, Ordering::Relaxed);
        assert_eq!(client.reconnect_count(), 1);
        client.reconnect_count.fetch_add(1, Ordering::Relaxed);
        assert_eq!(client.reconnect_count(), 2);
    }

    #[test]
    fn test_last_message_at_updates() {
        let client = WsClient::new(vec!["test".to_string()], 100);
        assert_eq!(client.last_message_at(), 0);
        client.last_message_at.store(1700000000000, Ordering::Relaxed);
        assert_eq!(client.last_message_at(), 1700000000000);
    }
}
