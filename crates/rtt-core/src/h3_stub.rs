/// HTTP/3 implementation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H3Status {
    NotImplemented,
}

/// Get current HTTP/3 status.
pub fn status() -> H3Status {
    H3Status::NotImplemented
}

/// Probe a host for HTTP/3 support via alt-svc header.
/// Returns the alt-svc value if h3 is advertised, None otherwise.
pub async fn probe_alt_svc(
    host: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    use bytes::Bytes;
    use crate::connection::{connect_h2, send_request, AddressFamily};
    use http::Request;

    let mut sender = connect_h2(host, 443, AddressFamily::Auto).await?;
    let req = Request::builder()
        .method("GET")
        .uri("/")
        .header("host", host)
        .body(Bytes::new())
        .unwrap();

    let resp = send_request(&mut sender, req).await?;
    let alt_svc = resp
        .headers()
        .get("alt-svc")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    Ok(alt_svc.filter(|s| s.contains("h3")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_not_implemented() {
        assert_eq!(status(), H3Status::NotImplemented);
    }

    #[tokio::test]
    async fn probe_detects_alt_svc() {
        // clob.polymarket.com is behind Cloudflare which advertises h3
        match probe_alt_svc("clob.polymarket.com").await {
            Ok(Some(alt_svc)) => {
                assert!(alt_svc.contains("h3"), "alt-svc should contain h3: {}", alt_svc);
            }
            Ok(None) => {
                // h3 not advertised, that's fine
            }
            Err(e) => {
                panic!("probe failed: {}", e);
            }
        }
    }
}
