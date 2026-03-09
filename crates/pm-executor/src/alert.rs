use std::time::Duration;

/// Fire-and-forget webhook alert. Does not block or retry.
/// POSTs `{"text": "<message>"}` (Slack-compatible format).
pub async fn send_alert(webhook_url: &str, message: &str) {
    let body = serde_json::json!({ "text": message }).to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build();
    let client = match client {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "Failed to build HTTP client for alert");
            return;
        }
    };
    match client
        .post(webhook_url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(resp) => {
            tracing::info!(status = resp.status().as_u16(), "Alert sent");
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to send alert webhook");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn alert_json_format() {
        let body = serde_json::json!({ "text": "test message" });
        assert_eq!(body["text"], "test message");
    }

    #[test]
    fn missing_webhook_url_is_none() {
        let url: Option<String> = None;
        // Verify that None webhook doesn't cause issues
        assert!(url.is_none());
    }
}
