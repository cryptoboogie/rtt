use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::safety::CircuitBreaker;

/// Run the HTTP health server.
pub async fn run_health_server(
    port: u16,
    circuit_breaker: CircuitBreaker,
    last_message_at: Arc<AtomicU64>,
    reconnect_count: Arc<AtomicU64>,
    start_time: Instant,
    mut shutdown: watch::Receiver<bool>,
) {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, port = port, "Failed to bind health server");
            return;
        }
    };
    tracing::info!(port = port, "Health server listening");

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let cb = circuit_breaker.clone();
                        let lma = last_message_at.clone();
                        let rc = reconnect_count.clone();
                        let st = start_time;
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);
                            let svc = service_fn(move |req| {
                                handle_request(req, cb.clone(), lma.clone(), rc.clone(), st)
                            });
                            let _ = http1::Builder::new().serve_connection(io, svc).await;
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Health server accept failed");
                    }
                }
            }
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    tracing::info!("Health server shutting down");
                    break;
                }
            }
        }
    }
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    cb: CircuitBreaker,
    last_message_at: Arc<AtomicU64>,
    reconnect_count: Arc<AtomicU64>,
    start_time: Instant,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    match req.uri().path() {
        "/health" => Ok(health_response(&cb, &last_message_at)),
        "/status" => Ok(status_response(&cb, &last_message_at, &reconnect_count, start_time)),
        _ => Ok(not_found_response()),
    }
}

fn health_response(
    cb: &CircuitBreaker,
    last_message_at: &Arc<AtomicU64>,
) -> Response<Full<Bytes>> {
    let tripped = cb.is_tripped();
    let lma = last_message_at.load(Ordering::Relaxed);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let stale = lma > 0 && (now_ms.saturating_sub(lma)) > 60_000;

    if tripped {
        json_response(StatusCode::SERVICE_UNAVAILABLE, r#"{"status":"unhealthy","reason":"circuit breaker tripped"}"#)
    } else if stale {
        json_response(StatusCode::SERVICE_UNAVAILABLE, r#"{"status":"unhealthy","reason":"websocket data stale"}"#)
    } else {
        json_response(StatusCode::OK, r#"{"status":"ok"}"#)
    }
}

fn status_response(
    cb: &CircuitBreaker,
    last_message_at: &Arc<AtomicU64>,
    reconnect_count: &Arc<AtomicU64>,
    start_time: Instant,
) -> Response<Full<Bytes>> {
    let (orders, usd) = cb.stats();
    let tripped = cb.is_tripped();
    let uptime = start_time.elapsed().as_secs();
    let lma = last_message_at.load(Ordering::Relaxed);
    let rc = reconnect_count.load(Ordering::Relaxed);

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let last_msg_ago = if lma > 0 {
        (now_ms.saturating_sub(lma)) as f64 / 1000.0
    } else {
        -1.0
    };

    let status = if tripped { "unhealthy" } else { "ok" };

    let body = serde_json::json!({
        "status": status,
        "uptime_seconds": uptime,
        "circuit_breaker": {
            "tripped": tripped,
            "orders_fired": orders,
            "max_orders": cb.max_orders(),
            "usd_committed": usd,
            "max_usd": cb.max_usd()
        },
        "websocket": {
            "last_message_seconds_ago": last_msg_ago,
            "reconnect_count": rc
        }
    });

    json_response(StatusCode::OK, &body.to_string())
}

fn json_response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .unwrap()
}

fn not_found_response() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Full::new(Bytes::from("Not Found")))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_returns_ok_when_healthy() {
        let cb = CircuitBreaker::new(10, 100.0);
        let lma = Arc::new(AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        ));
        let resp = health_response(&cb, &lma);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn health_returns_503_when_tripped() {
        let cb = CircuitBreaker::new(10, 100.0);
        cb.trip();
        let lma = Arc::new(AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        ));
        let resp = health_response(&cb, &lma);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn health_returns_503_when_stale() {
        let cb = CircuitBreaker::new(10, 100.0);
        // Set last_message_at to 2 minutes ago
        let two_min_ago = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 120_000;
        let lma = Arc::new(AtomicU64::new(two_min_ago));
        let resp = health_response(&cb, &lma);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn status_returns_json_with_all_fields() {
        let cb = CircuitBreaker::new(5, 10.0);
        cb.check_and_record("0.50", "10").unwrap();
        let lma = Arc::new(AtomicU64::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        ));
        let rc = Arc::new(AtomicU64::new(2));
        let start = Instant::now();

        let resp = status_response(&cb, &lma, &rc, start);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_server_shuts_down_on_signal() {
        let cb = CircuitBreaker::new(10, 100.0);
        let lma = Arc::new(AtomicU64::new(0));
        let rc = Arc::new(AtomicU64::new(0));
        let start = Instant::now();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(run_health_server(0, cb, lma, rc, start, shutdown_rx));

        // Give it a moment to bind
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let _ = shutdown_tx.send(true);
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "Health server should stop on shutdown");
    }
}
