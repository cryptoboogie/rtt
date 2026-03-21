use std::collections::BTreeMap;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message;

use crate::order_manager::{ExchangeFill, ExchangeSyncSnapshot};
use crate::order_state::{ExchangeObservedQuote, ExchangeObservedQuoteState, WorkingQuote};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserFeedEvent {
    Pong,
    Order(UserOrderEvent),
    Trade(UserTradeEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserFeedRuntimeEvent {
    Connected,
    Event(UserFeedEvent),
    Degraded(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserOrderEvent {
    pub order_id: String,
    pub asset_id: String,
    pub status: String,
    pub side: String,
    pub price: String,
    pub original_size: String,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTradeEvent {
    pub trade_id: String,
    pub asset_id: String,
    pub price: String,
    pub timestamp_ms: u64,
    pub maker_orders: Vec<UserTradeMakerOrder>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserTradeMakerOrder {
    pub order_id: String,
    pub asset_id: String,
    pub matched_amount: String,
    pub side: String,
}

#[derive(Debug, Default, Clone)]
pub struct UserFeedState {
    orders_by_id: BTreeMap<String, UserOrderEvent>,
    trades_by_fill_id: BTreeMap<String, ExchangeFill>,
    degraded_reason: Option<String>,
    connected: bool,
}

impl UserFeedState {
    pub fn mark_connected(&mut self) {
        self.connected = true;
        self.degraded_reason = None;
    }

    pub fn mark_degraded(&mut self, reason: impl Into<String>) {
        self.degraded_reason = Some(reason.into());
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded_reason.is_some()
    }

    pub fn apply_event(&mut self, event: UserFeedEvent, working_quotes: &[WorkingQuote]) {
        match event {
            UserFeedEvent::Pong => {}
            UserFeedEvent::Order(order) => {
                self.orders_by_id.insert(order.order_id.clone(), order);
            }
            UserFeedEvent::Trade(trade) => {
                for maker_order in trade.maker_orders {
                    if !working_quotes.iter().any(|quote| {
                        quote.client_order_id.as_deref() == Some(maker_order.order_id.as_str())
                    }) {
                        continue;
                    }

                    let Some(side) = parse_side(&maker_order.side) else {
                        continue;
                    };

                    self.trades_by_fill_id.insert(
                        format!("{}:{}", trade.trade_id, maker_order.order_id),
                        ExchangeFill {
                            fill_id: format!("{}:{}", trade.trade_id, maker_order.order_id),
                            asset_id: maker_order.asset_id.clone(),
                            side,
                            filled_size: maker_order.matched_amount,
                            price: trade.price.clone(),
                            observed_at_ms: trade.timestamp_ms,
                        },
                    );
                }
            }
        }
    }

    pub fn exchange_snapshot(
        &self,
        working_quotes: &[WorkingQuote],
        observed_at_ms: u64,
    ) -> ExchangeSyncSnapshot {
        let mut quotes = Vec::new();
        for working in working_quotes {
            let Some(order_id) = working.client_order_id.as_ref() else {
                continue;
            };
            let Some(order) = self.orders_by_id.get(order_id) else {
                continue;
            };

            let Some(state) = map_order_state(&order.status) else {
                continue;
            };

            quotes.push(ExchangeObservedQuote {
                quote_id: working.quote_id.clone(),
                client_order_id: Some(order.order_id.clone()),
                state,
                observed_at_ms: order.timestamp_ms,
            });
        }

        ExchangeSyncSnapshot {
            authoritative: false,
            resync_pending: self.is_degraded(),
            quotes,
            fills: self.trades_by_fill_id.values().cloned().collect(),
            observed_at_ms,
        }
    }
}

pub fn parse_user_feed_event(message: &str) -> Result<Option<UserFeedEvent>, String> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.eq_ignore_ascii_case("PONG") || trimmed == "{}" {
        return Ok(Some(UserFeedEvent::Pong));
    }
    if trimmed.eq_ignore_ascii_case("PING") {
        return Ok(None);
    }

    let envelope: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|err| format!("invalid user feed json: {err}"))?;
    let Some(event_type) = envelope.get("event_type").and_then(|value| value.as_str()) else {
        return Ok(None);
    };

    match event_type {
        "order" => {
            let raw: RawOrderEvent = serde_json::from_value(envelope)
                .map_err(|err| format!("invalid user order event: {err}"))?;
            Ok(Some(UserFeedEvent::Order(UserOrderEvent {
                order_id: raw.id,
                asset_id: raw.asset_id,
                status: raw.status,
                side: raw.side,
                price: raw.price,
                original_size: raw.original_size,
                timestamp_ms: parse_secs_to_ms(&raw.timestamp),
            })))
        }
        "trade" => {
            let raw: RawTradeEvent = serde_json::from_value(envelope)
                .map_err(|err| format!("invalid user trade event: {err}"))?;
            Ok(Some(UserFeedEvent::Trade(UserTradeEvent {
                trade_id: raw.id,
                asset_id: raw.asset_id,
                price: raw.price,
                timestamp_ms: parse_secs_to_ms(&raw.timestamp),
                maker_orders: raw
                    .maker_orders
                    .into_iter()
                    .map(|maker_order| UserTradeMakerOrder {
                        order_id: maker_order.order_id,
                        asset_id: maker_order.asset_id,
                        matched_amount: maker_order.matched_amount,
                        side: maker_order.side,
                    })
                    .collect(),
            })))
        }
        _ => Ok(None),
    }
}

pub async fn run_user_feed(
    ws_url: String,
    creds: rtt_core::clob_auth::L2Credentials,
    markets: Vec<String>,
    event_tx: mpsc::Sender<UserFeedRuntimeEvent>,
    mut shutdown: watch::Receiver<bool>,
) {
    let connect = tokio_tungstenite::connect_async(ws_url).await;
    let Ok((mut stream, _)) = connect else {
        let err = connect
            .err()
            .map(|err| err.to_string())
            .unwrap_or_else(|| "unknown websocket connect error".to_string());
        let _ = event_tx.send(UserFeedRuntimeEvent::Degraded(err)).await;
        return;
    };

    let auth = serde_json::json!({
        "auth": {
            "apiKey": creds.api_key,
            "secret": creds.secret,
            "passphrase": creds.passphrase,
        },
        "type": "user"
    });
    if stream
        .send(Message::Text(auth.to_string()))
        .await
        .is_err()
    {
        let _ = event_tx
            .send(UserFeedRuntimeEvent::Degraded("user_feed_auth_send_failed".to_string()))
            .await;
        return;
    }

    if !markets.is_empty() {
        let subscribe = serde_json::json!({
            "operation": "subscribe",
            "markets": markets,
        });
        if stream
            .send(Message::Text(subscribe.to_string()))
            .await
            .is_err()
        {
            let _ = event_tx
                .send(UserFeedRuntimeEvent::Degraded(
                    "user_feed_subscribe_send_failed".to_string(),
                ))
                .await;
            return;
        }
    }

    if event_tx.send(UserFeedRuntimeEvent::Connected).await.is_err() {
        return;
    }

    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(10));
    loop {
        tokio::select! {
            next_message = stream.next() => {
                match next_message {
                    Some(Ok(Message::Text(text))) => {
                        match parse_user_feed_event(&text) {
                            Ok(Some(event)) => {
                                if event_tx.send(UserFeedRuntimeEvent::Event(event)).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {}
                            Err(err) => {
                                let _ = event_tx.send(UserFeedRuntimeEvent::Degraded(err)).await;
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Ping(payload))) => {
                        if stream.send(Message::Pong(payload)).await.is_err() {
                            let _ = event_tx.send(UserFeedRuntimeEvent::Degraded("user_feed_pong_failed".to_string())).await;
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        let _ = event_tx.send(UserFeedRuntimeEvent::Degraded("user_feed_closed".to_string())).await;
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        let _ = event_tx.send(UserFeedRuntimeEvent::Degraded(err.to_string())).await;
                        break;
                    }
                    None => {
                        let _ = event_tx.send(UserFeedRuntimeEvent::Degraded("user_feed_stream_ended".to_string())).await;
                        break;
                    }
                }
            }
            _ = ping_interval.tick() => {
                if stream.send(Message::Text("PING".to_string())).await.is_err() {
                    let _ = event_tx.send(UserFeedRuntimeEvent::Degraded("user_feed_ping_failed".to_string())).await;
                    break;
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    let _ = stream.close(None).await;
                    break;
                }
            }
        }
    }
}

fn parse_side(value: &str) -> Option<rtt_core::trigger::Side> {
    match value {
        "BUY" => Some(rtt_core::trigger::Side::Buy),
        "SELL" => Some(rtt_core::trigger::Side::Sell),
        _ => None,
    }
}

fn map_order_state(status: &str) -> Option<ExchangeObservedQuoteState> {
    match status {
        "LIVE" | "MATCHED" => Some(ExchangeObservedQuoteState::Working),
        "DELAYED" => Some(ExchangeObservedQuoteState::PendingCancel),
        "CANCELED" => Some(ExchangeObservedQuoteState::Canceled),
        "REJECTED" => Some(ExchangeObservedQuoteState::Rejected {
            reason: "rejected".to_string(),
        }),
        _ => None,
    }
}

fn parse_secs_to_ms(value: &str) -> u64 {
    value.parse::<u64>().unwrap_or_default().saturating_mul(1_000)
}

#[derive(Debug, Deserialize)]
struct RawOrderEvent {
    id: String,
    asset_id: String,
    side: String,
    original_size: String,
    price: String,
    status: String,
    timestamp: String,
}

#[derive(Debug, Deserialize)]
struct RawTradeEvent {
    id: String,
    asset_id: String,
    price: String,
    timestamp: String,
    #[serde(default)]
    maker_orders: Vec<RawTradeMakerOrder>,
}

#[derive(Debug, Deserialize)]
struct RawTradeMakerOrder {
    order_id: String,
    asset_id: String,
    matched_amount: String,
    side: String,
}

#[cfg(test)]
mod tests {
    use pm_strategy::quote::{DesiredQuote, QuoteId};
    use pm_strategy::types::{OrderType, Side};

    use super::*;

    fn working_quote(id: &str, order_id: &str) -> WorkingQuote {
        let mut quote = WorkingQuote::pending_submit(
            DesiredQuote::new(
                QuoteId::new(id),
                "asset-yes",
                Side::Buy,
                "0.45",
                "50",
                OrderType::GTD,
            ),
            1_000,
        );
        quote.mark_working(order_id, 2_000);
        quote
    }

    #[test]
    fn parses_user_feed_order_and_trade_messages() {
        let order = parse_user_feed_event(
            r#"{
                "event_type":"order",
                "id":"order-1",
                "asset_id":"asset-yes",
                "side":"BUY",
                "original_size":"50",
                "price":"0.45",
                "status":"LIVE",
                "timestamp":"1672290687"
            }"#,
        )
        .unwrap()
        .expect("order event");
        let trade = parse_user_feed_event(
            r#"{
                "event_type":"trade",
                "id":"trade-1",
                "asset_id":"asset-yes",
                "price":"0.45",
                "timestamp":"1672290701",
                "maker_orders":[
                    {
                        "order_id":"order-1",
                        "asset_id":"asset-yes",
                        "matched_amount":"10",
                        "side":"BUY"
                    }
                ]
            }"#,
        )
        .unwrap()
        .expect("trade event");

        assert!(matches!(order, UserFeedEvent::Order(_)));
        assert!(matches!(trade, UserFeedEvent::Trade(_)));
    }

    #[test]
    fn parses_plaintext_user_feed_heartbeats() {
        let pong = parse_user_feed_event("PONG")
            .unwrap()
            .expect("pong event");
        let ping = parse_user_feed_event("PING").unwrap();

        assert!(matches!(pong, UserFeedEvent::Pong));
        assert!(ping.is_none());
    }

    #[test]
    fn user_feed_state_maps_events_into_exchange_snapshot_and_fills() {
        let working = vec![working_quote("condition-1:yes:entry", "order-1")];
        let mut state = UserFeedState::default();
        state.mark_connected();
        state.apply_event(
            parse_user_feed_event(
                r#"{
                    "event_type":"order",
                    "id":"order-1",
                    "asset_id":"asset-yes",
                    "side":"BUY",
                    "original_size":"50",
                    "price":"0.45",
                    "status":"LIVE",
                    "timestamp":"1672290687"
                }"#,
            )
            .unwrap()
            .unwrap(),
            &working,
        );
        state.apply_event(
            parse_user_feed_event(
                r#"{
                    "event_type":"trade",
                    "id":"trade-1",
                    "asset_id":"asset-yes",
                    "price":"0.45",
                    "timestamp":"1672290701",
                    "maker_orders":[
                        {
                            "order_id":"order-1",
                            "asset_id":"asset-yes",
                            "matched_amount":"10",
                            "side":"BUY"
                        }
                    ]
                }"#,
            )
            .unwrap()
            .unwrap(),
            &working,
        );

        let snapshot = state.exchange_snapshot(&working, 1_700_000_000_000);
        assert!(!snapshot.authoritative);
        assert!(!snapshot.resync_pending);
        assert_eq!(snapshot.quotes.len(), 1);
        assert_eq!(snapshot.quotes[0].quote_id, QuoteId::new("condition-1:yes:entry"));
        assert_eq!(snapshot.fills.len(), 1);
        assert_eq!(snapshot.fills[0].fill_id, "trade-1:order-1");
        assert_eq!(snapshot.fills[0].filled_size, "10");
    }

    #[test]
    fn degraded_user_feed_fails_closed_via_resync_pending_snapshot() {
        let working = vec![working_quote("condition-1:yes:entry", "order-1")];
        let mut state = UserFeedState::default();
        state.mark_connected();
        state.mark_degraded("socket_closed");

        let snapshot = state.exchange_snapshot(&working, 1_700_000_000_000);
        assert!(snapshot.resync_pending);
    }
}
