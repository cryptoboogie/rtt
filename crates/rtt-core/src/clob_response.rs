use serde::Deserialize;

/// Response from POST /order.
#[derive(Debug, Clone, Deserialize)]
pub struct OrderResponse {
    pub success: bool,
    #[serde(rename = "orderID", default)]
    pub order_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(rename = "transactionsHashes", default)]
    pub transaction_hashes: Vec<String>,
    #[serde(rename = "tradeIDs", default)]
    pub trade_ids: Vec<String>,
    #[serde(rename = "errorMsg", default)]
    pub error_msg: Option<String>,
}

/// Parse order response from bytes.
pub fn parse_order_response(body: &[u8]) -> Result<OrderResponse, serde_json::Error> {
    serde_json::from_slice(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_success_response() {
        let json = r#"{
            "success": true,
            "orderID": "0xabc123",
            "status": "matched",
            "transactionsHashes": ["0xdef456"],
            "tradeIDs": ["trade-1"]
        }"#;
        let resp = parse_order_response(json.as_bytes()).unwrap();
        assert!(resp.success);
        assert_eq!(resp.order_id, "0xabc123");
        assert_eq!(resp.status, "matched");
        assert_eq!(resp.transaction_hashes, vec!["0xdef456"]);
        assert_eq!(resp.trade_ids, vec!["trade-1"]);
        assert!(resp.error_msg.is_none());
    }

    #[test]
    fn test_parse_error_response() {
        let json = r#"{
            "success": false,
            "errorMsg": "insufficient balance"
        }"#;
        let resp = parse_order_response(json.as_bytes()).unwrap();
        assert!(!resp.success);
        assert_eq!(resp.error_msg.as_deref(), Some("insufficient balance"));
        assert!(resp.order_id.is_empty());
    }

    #[test]
    fn test_parse_response_bytes() {
        let bytes = b"{\"success\":true,\"orderID\":\"abc\",\"status\":\"live\",\"transactionsHashes\":[],\"tradeIDs\":[]}";
        let resp = parse_order_response(bytes).unwrap();
        assert!(resp.success);
        assert_eq!(resp.order_id, "abc");
        assert_eq!(resp.status, "live");
    }
}
