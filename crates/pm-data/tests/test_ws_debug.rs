use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
const TEST_ASSET_ID: &str =
    "48825140812430902098404528620382945035793471220915259967486864813738884055220";

#[tokio::test]
#[ignore] // Requires network; may timeout on quiet markets
async fn raw_ws_connect_and_subscribe() {
    eprintln!("Connecting to {WS_URL}...");
    let (ws_stream, response) = connect_async(WS_URL).await.expect("Failed to connect");
    eprintln!("Connected! Status: {}", response.status());

    let (mut write, mut read) = ws_stream.split();

    let sub_msg = serde_json::json!({
        "assets_ids": [TEST_ASSET_ID],
        "type": "market",
        "custom_feature_enabled": true
    });
    let sub_str = serde_json::to_string(&sub_msg).unwrap();
    eprintln!("Sending subscription: {sub_str}");
    write
        .send(Message::Text(sub_str.into()))
        .await
        .expect("Failed to send subscribe");

    eprintln!("Waiting for messages...");
    for i in 0..5 {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), read.next())
            .await
            .expect("Timeout waiting for message");

        match msg {
            Some(Ok(Message::Text(text))) => {
                let text_str: &str = &text;
                // Check if it's an array or object
                let trimmed = text_str.trim();
                if trimmed.starts_with('[') {
                    eprintln!("Message {i}: ARRAY (len={})", trimmed.len());
                    // Parse as array
                    let arr: Vec<serde_json::Value> = serde_json::from_str(trimmed).unwrap();
                    for (j, item) in arr.iter().enumerate() {
                        let keys: Vec<&String> = item
                            .as_object()
                            .map(|o| o.keys().collect())
                            .unwrap_or_default();
                        eprintln!("  [{j}] keys: {:?}", keys);
                        if let Some(et) = item.get("event_type") {
                            eprintln!("  [{j}] event_type: {et}");
                        }
                    }
                } else if trimmed.starts_with('{') {
                    let obj: serde_json::Value = serde_json::from_str(trimmed).unwrap();
                    let keys: Vec<&String> = obj
                        .as_object()
                        .map(|o| o.keys().collect())
                        .unwrap_or_default();
                    eprintln!("Message {i}: OBJECT keys: {:?}", keys);
                    if let Some(et) = obj.get("event_type") {
                        eprintln!("  event_type: {et}");
                    }
                } else {
                    eprintln!("Message {i}: RAW: {}", &trimmed[..trimmed.len().min(100)]);
                }
            }
            Some(Ok(other)) => {
                eprintln!("Message {i}: non-text: {other:?}");
            }
            Some(Err(e)) => {
                eprintln!("Message {i}: error: {e}");
                break;
            }
            None => {
                eprintln!("Stream ended at message {i}");
                break;
            }
        }
    }
}
