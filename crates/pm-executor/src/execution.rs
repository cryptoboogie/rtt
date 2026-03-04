use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use alloy::signers::local::PrivateKeySigner;
use crossbeam_channel::Receiver;
use rtt_core::clob_auth::L2Credentials;
use rtt_core::clob_executor::{PreSignedOrderPool, process_one_clob};
use rtt_core::connection::ConnectionPool;
use rtt_core::trigger::TriggerMessage;

use crate::config::CredentialsConfig;

/// Build L2Credentials and PrivateKeySigner from executor config.
///
/// If `dry_run` is false and credentials are empty/invalid, returns an error.
/// If `dry_run` is true, empty credentials are allowed (we never send orders).
pub fn build_credentials(
    creds: &CredentialsConfig,
    dry_run: bool,
) -> Result<(L2Credentials, Option<PrivateKeySigner>), Box<dyn std::error::Error>> {
    let l2 = L2Credentials {
        api_key: creds.api_key.clone(),
        secret: creds.api_secret.clone(),
        passphrase: creds.passphrase.clone(),
        address: creds.maker_address.clone(),
    };

    if dry_run {
        return Ok((l2, None));
    }

    // Validate credentials for live mode
    if creds.private_key.is_empty() {
        return Err("private_key is required when dry_run = false".into());
    }
    if creds.api_key.is_empty() {
        return Err("api_key is required when dry_run = false".into());
    }
    if creds.api_secret.is_empty() {
        return Err("api_secret is required when dry_run = false".into());
    }
    if creds.passphrase.is_empty() {
        return Err("passphrase is required when dry_run = false".into());
    }
    if creds.maker_address.is_empty() {
        return Err("maker_address is required when dry_run = false".into());
    }

    let pk_hex = creds
        .private_key
        .strip_prefix("0x")
        .unwrap_or(&creds.private_key);
    let signer: PrivateKeySigner = pk_hex.parse()?;

    Ok((l2, Some(signer)))
}

/// The execution loop — runs on a dedicated OS thread (not tokio).
///
/// Reads triggers from the crossbeam channel and either:
/// - Dry-run: logs what it would do
/// - Live: dispatches a pre-signed order via `process_one_clob()`
pub fn run_execution_loop(
    rx: Receiver<TriggerMessage>,
    pool: Arc<ConnectionPool>,
    mut presigned: PreSignedOrderPool,
    creds: L2Credentials,
    dry_run: bool,
    shutdown: Arc<AtomicBool>,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to build executor tokio runtime");

    tracing::info!(
        dry_run = dry_run,
        presigned_count = presigned.len(),
        "Execution loop started"
    );

    while !shutdown.load(Ordering::Relaxed) {
        match rx.try_recv() {
            Ok(trigger) => {
                if dry_run {
                    tracing::info!(
                        trigger_id = trigger.trigger_id,
                        token_id = %trigger.token_id,
                        side = ?trigger.side,
                        price = %trigger.price,
                        size = %trigger.size,
                        "[DRY RUN] Would fire order"
                    );
                    continue;
                }

                let rec = process_one_clob(&pool, &mut presigned, &creds, &trigger, &rt);

                tracing::info!(
                    trigger_id = trigger.trigger_id,
                    trigger_to_wire_us = rec.trigger_to_wire() as f64 / 1000.0,
                    write_duration_us = rec.write_duration() as f64 / 1000.0,
                    warm_ttfb_ms = rec.warm_ttfb() as f64 / 1_000_000.0,
                    connection = rec.connection_index,
                    pop = %rec.cf_ray_pop,
                    reconnect = rec.is_reconnect,
                    "Order dispatched"
                );

                if presigned.consumed() >= presigned.len() {
                    tracing::warn!("Pre-signed order pool exhausted! Need refill.");
                    break;
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                std::thread::yield_now();
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
        }
    }

    tracing::info!("Execution loop stopped");
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtt_core::trigger::{OrderType, Side};

    fn empty_creds() -> CredentialsConfig {
        CredentialsConfig {
            api_key: String::new(),
            api_secret: String::new(),
            passphrase: String::new(),
            private_key: String::new(),
            maker_address: String::new(),
            signer_address: String::new(),
        }
    }

    fn valid_creds() -> CredentialsConfig {
        CredentialsConfig {
            api_key: "test-key".to_string(),
            api_secret: "dGVzdC1zZWNyZXQ=".to_string(),
            passphrase: "test-pass".to_string(),
            // Foundry test private key
            private_key: "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
                .to_string(),
            maker_address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
            signer_address: "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266".to_string(),
        }
    }

    #[test]
    fn build_credentials_dry_run_allows_empty() {
        let (l2, signer) = build_credentials(&empty_creds(), true).unwrap();
        assert!(signer.is_none());
        assert!(l2.api_key.is_empty());
    }

    #[test]
    fn build_credentials_live_rejects_empty_private_key() {
        let err = build_credentials(&empty_creds(), false);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("private_key"), "error: {}", msg);
    }

    #[test]
    fn build_credentials_live_rejects_empty_api_key() {
        let mut creds = empty_creds();
        creds.private_key = "0xdeadbeef".to_string();
        let err = build_credentials(&creds, false);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("api_key"), "error: {}", msg);
    }

    #[test]
    fn build_credentials_live_valid() {
        let (_l2, signer) = build_credentials(&valid_creds(), true).unwrap();
        // dry_run=true skips signer
        assert!(signer.is_none());

        let (l2, signer) = build_credentials(&valid_creds(), false).unwrap();
        assert!(signer.is_some());
        assert_eq!(l2.api_key, "test-key");
        assert_eq!(l2.secret, "dGVzdC1zZWNyZXQ=");
        assert_eq!(
            l2.address,
            "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
        );
    }

    #[test]
    fn dry_run_execution_loop_logs_and_exits() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let shutdown = Arc::new(AtomicBool::new(false));

        // Send a trigger, then drop tx to disconnect
        tx.send(TriggerMessage {
            trigger_id: 42,
            token_id: "test-token".to_string(),
            side: Side::Buy,
            price: "0.45".to_string(),
            size: "10".to_string(),
            order_type: OrderType::FOK,
            timestamp_ns: 1000,
        })
        .unwrap();
        drop(tx);

        // Create a minimal pool (0 connections — dry run won't use it)
        let pool = Arc::new(ConnectionPool::new("localhost", 443, 0, rtt_core::connection::AddressFamily::Auto));
        let presigned = PreSignedOrderPool::new(vec![]).unwrap();
        let creds = L2Credentials {
            api_key: String::new(),
            secret: String::new(),
            passphrase: String::new(),
            address: String::new(),
        };

        // Run the loop — should process the trigger in dry-run mode and exit on disconnect
        run_execution_loop(rx, pool, presigned, creds, true, shutdown);
        // If we get here without panic, the dry-run path works
    }
}
