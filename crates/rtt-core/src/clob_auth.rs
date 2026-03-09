use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute HMAC-SHA256: base64url-decode secret, HMAC message, base64url-encode result.
pub fn hmac_signature(secret: &str, message: &str) -> Result<String, Box<dyn std::error::Error>> {
    let decoded_secret = URL_SAFE.decode(secret)?;
    let mut mac = HmacSha256::new_from_slice(&decoded_secret)?;
    mac.update(message.as_bytes());
    let result = mac.finalize().into_bytes();
    Ok(URL_SAFE.encode(result))
}

/// L2 API credentials for Polymarket CLOB.
#[derive(Debug, Clone)]
pub struct L2Credentials {
    pub api_key: String,
    pub secret: String,
    pub passphrase: String,
    pub address: String,
}

/// Build L2 authentication headers for a CLOB API request.
///
/// Returns Vec of (header_name, header_value) pairs.
pub fn build_l2_headers(
    creds: &L2Credentials,
    timestamp: &str,
    method: &str,
    path: &str,
    body: &str,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let message = format!("{}{}{}{}", timestamp, method, path, body);
    let signature = hmac_signature(&creds.secret, &message)?;

    Ok(vec![
        ("POLY_ADDRESS".to_string(), creds.address.to_lowercase()),
        ("POLY_API_KEY".to_string(), creds.api_key.clone()),
        ("POLY_PASSPHRASE".to_string(), creds.passphrase.clone()),
        ("POLY_SIGNATURE".to_string(), signature),
        ("POLY_TIMESTAMP".to_string(), timestamp.to_string()),
    ])
}

/// Load L2 credentials from environment variables.
/// Returns (creds, private_key, proxy_address).
/// - POLY_ADDRESS = EOA signer address (used in auth headers)
/// - POLY_PROXY_ADDRESS = proxy/funder address (used as maker in orders)
/// - POLY_PRIVATE_KEY = EOA private key
pub fn load_credentials_from_env(
) -> Result<(L2Credentials, String, String), Box<dyn std::error::Error>> {
    let api_key = std::env::var("POLY_API_KEY")?;
    let secret = std::env::var("POLY_SECRET")?;
    let passphrase = std::env::var("POLY_PASSPHRASE")?;
    let address = std::env::var("POLY_ADDRESS")?;
    let private_key = std::env::var("POLY_PRIVATE_KEY")?;
    let proxy_address = std::env::var("POLY_PROXY_ADDRESS")?;

    Ok((
        L2Credentials {
            api_key,
            secret,
            passphrase,
            address, // EOA address — used in auth headers
        },
        private_key,
        proxy_address, // proxy wallet — used as maker/funder
    ))
}

/// Build a validation request for GET /auth/api-keys.
/// Returns (method, path, headers) for testing/use.
pub fn build_validation_request(
    creds: &L2Credentials,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let timestamp = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
    );
    build_l2_headers(creds, &timestamp, "GET", "/auth/api-keys", "")
}

/// Validate L2 credentials by hitting GET /auth/api-keys.
/// Returns Ok(()) if the server accepts our HMAC auth.
/// Returns Err with the HTTP status and body if rejected.
pub async fn validate_credentials(creds: &L2Credentials) -> Result<(), String> {
    let timestamp = format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs()
    );
    let headers = build_l2_headers(creds, &timestamp, "GET", "/auth/api-keys", "")
        .map_err(|e| format!("Failed to build auth headers: {}", e))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let mut req = client.get("https://clob.polymarket.com/auth/api-keys");
    for (name, value) in &headers {
        req = req.header(name, value);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else {
        let body = resp.text().await.unwrap_or_default();
        Err(format!(
            "Credential validation failed: HTTP {} — {}",
            status, body
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hmac_computation() {
        // Known test: use a base64url-encoded secret
        let secret = URL_SAFE.encode(b"test-secret-key!");
        let message = "1234567890POST/order{\"test\":true}";
        let result = hmac_signature(&secret, message).unwrap();

        // Verify it's valid base64url
        let decoded = URL_SAFE.decode(&result).unwrap();
        assert_eq!(decoded.len(), 32, "HMAC-SHA256 output should be 32 bytes");

        // Verify deterministic
        let result2 = hmac_signature(&secret, message).unwrap();
        assert_eq!(result, result2);
    }

    #[test]
    fn test_l2_headers_all_present() {
        let creds = L2Credentials {
            api_key: "test-api-key".to_string(),
            secret: URL_SAFE.encode(b"test-secret"),
            passphrase: "test-passphrase".to_string(),
            address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
        };

        let headers = build_l2_headers(&creds, "1234567890", "POST", "/order", "{}").unwrap();

        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"POLY_ADDRESS"));
        assert!(names.contains(&"POLY_API_KEY"));
        assert!(names.contains(&"POLY_PASSPHRASE"));
        assert!(names.contains(&"POLY_SIGNATURE"));
        assert!(names.contains(&"POLY_TIMESTAMP"));
        assert_eq!(headers.len(), 5);

        // Verify values
        let find = |name: &str| headers.iter().find(|(n, _)| n == name).unwrap().1.clone();
        assert_eq!(find("POLY_ADDRESS"), creds.address.to_lowercase());
        assert_eq!(find("POLY_API_KEY"), creds.api_key);
        assert_eq!(find("POLY_PASSPHRASE"), creds.passphrase);
        assert_eq!(find("POLY_TIMESTAMP"), "1234567890");
        // Signature should be non-empty base64url
        let sig = find("POLY_SIGNATURE");
        assert!(!sig.is_empty());
        assert!(URL_SAFE.decode(&sig).is_ok());
    }

    #[test]
    fn test_build_validation_request_has_correct_headers() {
        let creds = L2Credentials {
            api_key: "val-key".to_string(),
            secret: URL_SAFE.encode(b"val-secret"),
            passphrase: "val-pass".to_string(),
            address: "0xVALIDATOR".to_string(),
        };
        let headers = build_validation_request(&creds).unwrap();
        let names: Vec<&str> = headers.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"POLY_ADDRESS"));
        assert!(names.contains(&"POLY_API_KEY"));
        assert!(names.contains(&"POLY_PASSPHRASE"));
        assert!(names.contains(&"POLY_SIGNATURE"));
        assert!(names.contains(&"POLY_TIMESTAMP"));
        assert_eq!(headers.len(), 5);

        let find = |name: &str| headers.iter().find(|(n, _)| n == name).unwrap().1.clone();
        assert_eq!(find("POLY_ADDRESS"), "0xvalidator"); // lowercased
        assert_eq!(find("POLY_API_KEY"), "val-key");
    }

    #[test]
    fn test_credentials_from_env() {
        // Set env vars for test
        std::env::set_var("POLY_API_KEY", "test-key");
        std::env::set_var("POLY_SECRET", "dGVzdC1zZWNyZXQ=");
        std::env::set_var("POLY_PASSPHRASE", "test-pass");
        std::env::set_var("POLY_ADDRESS", "0xdeadbeef");
        std::env::set_var("POLY_PRIVATE_KEY", "0xprivkey");
        std::env::set_var("POLY_PROXY_ADDRESS", "0xproxyaddr");

        let (creds, pk, proxy) = load_credentials_from_env().unwrap();
        assert_eq!(creds.api_key, "test-key");
        assert_eq!(creds.secret, "dGVzdC1zZWNyZXQ=");
        assert_eq!(creds.passphrase, "test-pass");
        assert_eq!(creds.address, "0xdeadbeef");
        assert_eq!(pk, "0xprivkey");
        assert_eq!(proxy, "0xproxyaddr");

        // Clean up
        std::env::remove_var("POLY_API_KEY");
        std::env::remove_var("POLY_SECRET");
        std::env::remove_var("POLY_PASSPHRASE");
        std::env::remove_var("POLY_ADDRESS");
        std::env::remove_var("POLY_PRIVATE_KEY");
        std::env::remove_var("POLY_PROXY_ADDRESS");
    }

    #[tokio::test]
    #[ignore] // Requires live POLY_* env vars — does not place orders
    async fn test_validate_credentials_live() {
        let (creds, _, _) = load_credentials_from_env()
            .expect("Set POLY_API_KEY, POLY_SECRET, POLY_PASSPHRASE, POLY_ADDRESS, POLY_PRIVATE_KEY, POLY_PROXY_ADDRESS");
        validate_credentials(&creds)
            .await
            .expect("Credential validation should succeed with valid env vars");
    }
}
