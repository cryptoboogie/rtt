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
const RECONNECT_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize)]
struct SubscribeRequest {
    assets_ids: Vec<String>,
    r#type: String,
    custom_feature_enabled: bool,
}

pub struct WsClient {
    asset_ids: Vec<String>,
    tx: broadcast::Sender<WsMessage>,
    shutdown_tx: Option<tokio::sync::watch::Sender<bool>>,
}

impl WsClient {
    pub fn new(asset_ids: Vec<String>, channel_capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(channel_capacity);
        Self {
            asset_ids,
            tx,
            shutdown_tx: None,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WsMessage> {
        self.tx.subscribe()
    }

    pub fn sender(&self) -> broadcast::Sender<WsMessage> {
        self.tx.clone()
    }

    pub async fn run(&mut self) {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        loop {
            if *shutdown_rx.borrow() {
                info!("WsClient shutdown requested");
                break;
            }

            match connect_and_run(
                &self.asset_ids,
                self.tx.clone(),
                shutdown_rx.clone(),
            )
            .await
            {
                Ok(()) => {
                    info!("WebSocket connection closed cleanly");
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

            warn!("Reconnecting in {:?}...", RECONNECT_DELAY);
            tokio::time::sleep(RECONNECT_DELAY).await;
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
}
