use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rtt_core::polymarket::MARKET_WS_URL;
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

use crate::subscription_plan::{
    assigned_asset_ids_for_config, plan_subscription_commands, SubscriptionCommand,
    SubscriptionOperation, SubscriptionPlannerConfig,
};
use crate::types::{ReconnectEvent, WsMessage};

const PING_INTERVAL: Duration = Duration::from_secs(10);

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
    desired_asset_ids: Arc<RwLock<BTreeSet<String>>>,
    planner_config: SubscriptionPlannerConfig,
    tx: broadcast::Sender<WsMessage>,
    shutdown_tx: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
    control_tx: Mutex<Option<mpsc::UnboundedSender<SubscriptionCommand>>>,
    reconnect_count: Arc<AtomicU64>,
    last_message_at: Arc<AtomicU64>,
    issued_commands: Arc<Mutex<Vec<SubscriptionCommand>>>,
}

impl WsClient {
    pub fn new(asset_ids: Vec<String>, channel_capacity: usize) -> Self {
        Self::with_subscription_planner(
            asset_ids,
            channel_capacity,
            SubscriptionPlannerConfig::default(),
        )
    }

    pub fn with_subscription_planner(
        asset_ids: Vec<String>,
        channel_capacity: usize,
        planner_config: SubscriptionPlannerConfig,
    ) -> Self {
        let (tx, _) = broadcast::channel(channel_capacity);
        Self {
            desired_asset_ids: Arc::new(RwLock::new(asset_ids.into_iter().collect())),
            planner_config,
            tx,
            shutdown_tx: Mutex::new(None),
            control_tx: Mutex::new(None),
            reconnect_count: Arc::new(AtomicU64::new(0)),
            last_message_at: Arc::new(AtomicU64::new(0)),
            issued_commands: Arc::new(Mutex::new(Vec::new())),
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

    pub fn assigned_asset_ids(&self) -> Vec<String> {
        let desired_asset_ids = self.desired_asset_ids.read().unwrap();
        assigned_asset_ids_for_config(&desired_asset_ids, &self.planner_config)
    }

    pub fn reconfigure_assets(&self, asset_ids: Vec<String>) -> Vec<SubscriptionCommand> {
        let current_asset_ids = self
            .assigned_asset_ids()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let desired_asset_ids = asset_ids.into_iter().collect::<BTreeSet<_>>();
        let commands =
            plan_subscription_commands(&current_asset_ids, &desired_asset_ids, &self.planner_config);

        {
            let mut desired = self.desired_asset_ids.write().unwrap();
            *desired = desired_asset_ids;
        }

        if !commands.is_empty() {
            self.issued_commands
                .lock()
                .unwrap()
                .extend(commands.clone());
            let control_tx = self.control_tx.lock().unwrap().clone();
            if let Some(control_tx) = control_tx {
                for command in &commands {
                    let _ = control_tx.send(command.clone());
                }
            }
        }

        commands
    }

    pub async fn run(&mut self) {
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        *self.shutdown_tx.lock().unwrap() = Some(shutdown_tx);
        let (control_tx, mut control_rx) = mpsc::unbounded_channel();
        *self.control_tx.lock().unwrap() = Some(control_tx);
        let mut backoff = BackoffState::new();
        let mut first_connect = true;

        loop {
            if *shutdown_rx.borrow() {
                info!("WsClient shutdown requested");
                break;
            }

            match connect_and_run(
                &self.desired_asset_ids,
                &self.planner_config,
                self.tx.clone(),
                &mut control_rx,
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
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                // Signal reconnect to pipeline so it clears stale state
                let _ = self.tx.send(WsMessage::Reconnected(ReconnectEvent {
                    sequence: count,
                    timestamp_ms: now_ms,
                }));
                let delay = backoff.next_delay();
                warn!(
                    reconnect_count = count,
                    delay_ms = delay.as_millis() as u64,
                    "Reconnecting..."
                );
                tokio::time::sleep(delay).await;
            } else {
                first_connect = false;
            }
        }

        self.control_tx.lock().unwrap().take();
        self.shutdown_tx.lock().unwrap().take();
    }

    pub fn shutdown(&self) {
        if let Some(tx) = self.shutdown_tx.lock().unwrap().as_ref() {
            let _ = tx.send(true);
        }
    }

    #[cfg(test)]
    pub(crate) fn issued_commands_for_test(&self) -> Vec<SubscriptionCommand> {
        self.issued_commands.lock().unwrap().clone()
    }
}

async fn connect_and_run(
    desired_asset_ids: &Arc<RwLock<BTreeSet<String>>>,
    planner_config: &SubscriptionPlannerConfig,
    tx: broadcast::Sender<WsMessage>,
    control_rx: &mut mpsc::UnboundedReceiver<SubscriptionCommand>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
    last_message_at: Arc<AtomicU64>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (ws_stream, _) = connect_async(MARKET_WS_URL).await?;
    info!("Connected to {MARKET_WS_URL}");

    let (mut write, mut read) = ws_stream.split();

    let current_subscriptions = BTreeSet::new();
    let desired_asset_ids = desired_asset_ids.read().unwrap().clone();
    let initial_commands =
        plan_subscription_commands(&current_subscriptions, &desired_asset_ids, planner_config);
    for command in &initial_commands {
        send_subscription_command(&mut write, command, true).await?;
    }
    info!(
        "Subscribed to {} assets",
        assigned_asset_ids_for_config(&desired_asset_ids, planner_config).len()
    );

    let mut ping_interval = tokio::time::interval(PING_INTERVAL);
    ping_interval.tick().await; // consume first immediate tick

    loop {
        tokio::select! {
            command = control_rx.recv() => {
                match command {
                    Some(command) => {
                        send_subscription_command(&mut write, &command, true).await?;
                    }
                    None => break,
                }
            }
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

async fn send_subscription_command<S>(
    write: &mut S,
    command: &SubscriptionCommand,
    custom_features: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    if command.pacing_ms > 0 {
        tokio::time::sleep(Duration::from_millis(command.pacing_ms)).await;
    }

    let message = build_subscription_message(command, custom_features);
    write.send(Message::Text(message.into())).await?;
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
    build_subscription_message(
        &SubscriptionCommand {
            operation: SubscriptionOperation::Subscribe,
            asset_ids: asset_ids.to_vec(),
            shard_index: 0,
            pacing_ms: 0,
        },
        custom_features,
    )
}

pub fn build_subscription_message(command: &SubscriptionCommand, custom_features: bool) -> String {
    let mut value = serde_json::json!({
        "assets_ids": command.asset_ids,
        "type": "market",
        "operation": match command.operation {
            SubscriptionOperation::Subscribe => "subscribe",
            SubscriptionOperation::Unsubscribe => "unsubscribe",
        },
    });

    value["custom_feature_enabled"] = serde_json::Value::Bool(custom_features);
    serde_json::to_string(&value).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription_plan::{market_subscription_semantics, SubscriptionPlannerConfig};

    #[test]
    fn test_subscribe_message_format() {
        let ids = vec!["asset1".to_string(), "asset2".to_string()];
        let json = build_subscribe_message(&ids, true);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "market");
        assert_eq!(parsed["operation"], "subscribe");
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
    fn test_unsubscribe_message_format() {
        let json = build_subscription_message(
            &SubscriptionCommand {
                operation: SubscriptionOperation::Unsubscribe,
                asset_ids: vec!["asset1".to_string()],
                shard_index: 0,
                pacing_ms: 0,
            },
            true,
        );
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["operation"], "unsubscribe");
        assert_eq!(parsed["assets_ids"][0], "asset1");
    }

    #[test]
    fn test_market_subscription_semantics_match_documented_contract() {
        let semantics = market_subscription_semantics();
        assert!(semantics.supports_unsubscribe);
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
        client
            .last_message_at
            .store(1700000000000, Ordering::Relaxed);
        assert_eq!(client.last_message_at(), 1700000000000);
    }

    #[test]
    fn test_reconfigure_assets_updates_desired_state_and_stages_diff_commands() {
        let client = WsClient::new(vec!["asset1".to_string(), "asset2".to_string()], 100);

        let commands = client.reconfigure_assets(vec!["asset2".to_string(), "asset3".to_string()]);

        assert_eq!(
            commands,
            vec![
                SubscriptionCommand {
                    operation: SubscriptionOperation::Unsubscribe,
                    asset_ids: vec!["asset1".to_string()],
                    shard_index: 0,
                    pacing_ms: 0,
                },
                SubscriptionCommand {
                    operation: SubscriptionOperation::Subscribe,
                    asset_ids: vec!["asset3".to_string()],
                    shard_index: 0,
                    pacing_ms: 0,
                },
            ]
        );
        assert_eq!(
            client.assigned_asset_ids(),
            vec!["asset2".to_string(), "asset3".to_string()]
        );
        assert_eq!(client.issued_commands_for_test(), commands);
    }

    #[test]
    fn test_reconfigure_assets_respects_explicit_shard_assignment() {
        let client = WsClient::with_subscription_planner(
            vec![
                "asset1".to_string(),
                "asset2".to_string(),
                "asset3".to_string(),
                "asset4".to_string(),
            ],
            100,
            SubscriptionPlannerConfig {
                max_batch_size: 64,
                pacing_ms: 0,
                shard_count: 2,
                shard_index: 1,
            },
        );

        assert_eq!(
            client.assigned_asset_ids(),
            vec!["asset2".to_string(), "asset4".to_string()]
        );

        let commands = client.reconfigure_assets(vec![
            "asset1".to_string(),
            "asset3".to_string(),
            "asset5".to_string(),
            "asset6".to_string(),
        ]);

        assert_eq!(
            commands,
            vec![
                SubscriptionCommand {
                    operation: SubscriptionOperation::Unsubscribe,
                    asset_ids: vec!["asset2".to_string(), "asset4".to_string()],
                    shard_index: 1,
                    pacing_ms: 0,
                },
                SubscriptionCommand {
                    operation: SubscriptionOperation::Subscribe,
                    asset_ids: vec!["asset3".to_string(), "asset6".to_string()],
                    shard_index: 1,
                    pacing_ms: 0,
                },
            ]
        );
        assert_eq!(
            client.assigned_asset_ids(),
            vec!["asset3".to_string(), "asset6".to_string()]
        );
    }
}
