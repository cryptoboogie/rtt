//! # HTTP/3 Probe Tests
//!
//! These tests stay in the explicit live integration lane because they depend
//! on reaching Cloudflare over the network.

use rtt_core::h3_stub::probe_alt_svc;
use rtt_core::polymarket::CLOB_HOST;

#[tokio::test]
async fn probe_detects_h3_alt_svc_when_advertised() {
    match probe_alt_svc(CLOB_HOST).await {
        Ok(Some(alt_svc)) => {
            assert!(
                alt_svc.contains("h3"),
                "alt-svc should contain h3: {}",
                alt_svc
            );
        }
        Ok(None) => {}
        Err(err) => panic!("probe failed: {}", err),
    }
}
