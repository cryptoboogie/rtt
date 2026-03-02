use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::ClientConfig;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;

/// Address family selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFamily {
    Auto,
    V4,
    V6,
}

/// Extract POP code from cf-ray header value.
/// cf-ray format: "abc123def-EWR" → "EWR"
pub fn extract_pop(cf_ray: &str) -> String {
    if let Some(pos) = cf_ray.rfind('-') {
        cf_ray[pos + 1..].to_string()
    } else {
        String::new()
    }
}

/// Resolve hostname:port with address family filtering.
pub fn resolve(host: &str, port: u16, af: AddressFamily) -> std::io::Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    let filtered: Vec<SocketAddr> = match af {
        AddressFamily::Auto => addrs,
        AddressFamily::V4 => addrs.into_iter().filter(|a| a.is_ipv4()).collect(),
        AddressFamily::V6 => addrs.into_iter().filter(|a| a.is_ipv6()).collect(),
    };
    if filtered.is_empty() {
        Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "no addresses found for requested family",
        ))
    } else {
        Ok(filtered)
    }
}

fn make_tls_config() -> Arc<ClientConfig> {
    let root_store = rustls::RootCertStore::from_iter(
        webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
    );
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    // ALPN h2 is set by rustls by default when using http2,
    // but we set it explicitly for clarity.
    let mut config = config;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(config)
}

type H2Sender = http2::SendRequest<Full<Bytes>>;

/// Single HTTP/2 connection.
struct H2Connection {
    sender: H2Sender,
    healthy: bool,
}

/// Establish a single HTTP/2 connection.
pub async fn connect_h2(
    host: &str,
    port: u16,
    af: AddressFamily,
) -> Result<H2Sender, Box<dyn std::error::Error + Send + Sync>> {
    let addrs = resolve(host, port, af)?;
    let addr = addrs[0];

    let tcp = TcpStream::connect(addr).await?;
    tcp.set_nodelay(true)?;

    let tls_config = make_tls_config();
    let connector = TlsConnector::from(tls_config);
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())?;
    let tls_stream = connector.connect(server_name, tcp).await?;

    let io = TokioIo::new(tls_stream);

    let (sender, conn) = http2::handshake(TokioExecutor::new(), io).await?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            eprintln!("h2 connection error: {}", e);
        }
    });

    Ok(sender)
}

/// Send a request on an H2 sender and return the response with collected body.
pub async fn send_request(
    sender: &mut H2Sender,
    req: Request<Bytes>,
) -> Result<Response<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
    let req = req.map(Full::new);
    let resp = sender.send_request(req).await?;
    let (parts, body) = resp.into_parts();
    let body_bytes = body.collect().await?.to_bytes();
    Ok(Response::from_parts(parts, body_bytes))
}

/// Extract cf-ray header from a response.
pub fn get_cf_ray(resp: &Response<Bytes>) -> Option<String> {
    resp.headers()
        .get("cf-ray")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// Connection pool with warm HTTP/2 connections.
pub struct ConnectionPool {
    host: String,
    port: u16,
    af: AddressFamily,
    connections: Vec<Arc<Mutex<H2Connection>>>,
    next_index: AtomicUsize,
}

impl ConnectionPool {
    pub fn new(host: &str, port: u16, pool_size: usize, af: AddressFamily) -> Self {
        Self {
            host: host.to_string(),
            port,
            af,
            connections: Vec::with_capacity(pool_size),
            next_index: AtomicUsize::new(0),
        }
    }

    /// Establish all connections (warmup).
    pub async fn warmup(&mut self) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let pool_size = self.connections.capacity();
        self.connections.clear();
        for _ in 0..pool_size {
            let sender = connect_h2(&self.host, self.port, self.af).await?;
            self.connections.push(Arc::new(Mutex::new(H2Connection {
                sender,
                healthy: true,
            })));
        }
        Ok(pool_size)
    }

    /// Acquire a connection (round-robin). Returns an Arc<Mutex> for internal use.
    fn acquire(&self) -> Arc<Mutex<H2Connection>> {
        let idx = self.next_index.fetch_add(1, Ordering::Relaxed) % self.connections.len();
        self.connections[idx].clone()
    }

    /// Send a request using round-robin connection selection.
    pub async fn send(
        &self,
        req: Request<Bytes>,
    ) -> Result<Response<Bytes>, Box<dyn std::error::Error + Send + Sync>> {
        let conn = self.acquire();
        let mut guard = conn.lock().await;
        send_request(&mut guard.sender, req).await
    }

    /// Reconnect a specific connection index.
    pub async fn reconnect(
        &self,
        index: usize,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sender = connect_h2(&self.host, self.port, self.af).await?;
        let mut guard = self.connections[index].lock().await;
        guard.sender = sender;
        guard.healthy = true;
        Ok(())
    }

    /// Health check all connections by sending GET /.
    pub async fn health_check(&self) -> usize {
        let mut healthy = 0;
        for (i, conn) in self.connections.iter().enumerate() {
            let mut guard = conn.lock().await;
            let req = Request::builder()
                .method("GET")
                .uri("/")
                .header("host", self.host.as_str())
                .body(Full::new(Bytes::new()))
                .unwrap();
            match guard.sender.send_request(req).await {
                Ok(resp) => {
                    let _ = resp.into_body().collect().await;
                    guard.healthy = true;
                    healthy += 1;
                }
                Err(_) => {
                    guard.healthy = false;
                    drop(guard);
                    let _ = self.reconnect(i).await;
                }
            }
        }
        healthy
    }

    pub fn pool_size(&self) -> usize {
        self.connections.len()
    }

    pub fn host(&self) -> &str {
        &self.host
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_pop_from_cf_ray() {
        assert_eq!(extract_pop("abc123-EWR"), "EWR");
        assert_eq!(extract_pop("xyz789-IAD"), "IAD");
        assert_eq!(extract_pop("single"), "");
    }

    #[test]
    fn extract_pop_empty() {
        assert_eq!(extract_pop(""), "");
    }

    #[test]
    fn resolve_auto() {
        let addrs = resolve("clob.polymarket.com", 443, AddressFamily::Auto).unwrap();
        assert!(!addrs.is_empty());
    }

    #[test]
    fn resolve_v4() {
        let addrs = resolve("clob.polymarket.com", 443, AddressFamily::V4).unwrap();
        assert!(addrs.iter().all(|a| a.is_ipv4()));
    }

    #[test]
    fn resolve_v6_may_fail() {
        let _ = resolve("clob.polymarket.com", 443, AddressFamily::V6);
    }

    #[tokio::test]
    async fn connect_and_send_request() {
        let mut sender = connect_h2("clob.polymarket.com", 443, AddressFamily::Auto)
            .await
            .expect("failed to connect");

        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header("host", "clob.polymarket.com")
            .body(Bytes::new())
            .unwrap();

        let resp = send_request(&mut sender, req).await.expect("request failed");
        assert!(resp.status().is_success() || resp.status().is_client_error());

        let cf_ray = get_cf_ray(&resp);
        assert!(cf_ray.is_some(), "cf-ray header missing");
        let pop = extract_pop(&cf_ray.unwrap());
        assert!(!pop.is_empty(), "POP code empty");
    }

    #[tokio::test]
    async fn h2_session_reuse() {
        let mut sender = connect_h2("clob.polymarket.com", 443, AddressFamily::Auto)
            .await
            .expect("failed to connect");

        for _ in 0..2 {
            let req = Request::builder()
                .method("GET")
                .uri("/")
                .header("host", "clob.polymarket.com")
                .body(Bytes::new())
                .unwrap();
            let resp = send_request(&mut sender, req).await.expect("request failed");
            assert!(resp.status().is_success() || resp.status().is_client_error());
        }
    }

    #[tokio::test]
    async fn connection_pool_warmup_and_send() {
        let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 2, AddressFamily::Auto);
        let warmed = pool.warmup().await.expect("warmup failed");
        assert_eq!(warmed, 2);
        assert_eq!(pool.pool_size(), 2);

        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header("host", "clob.polymarket.com")
            .body(Bytes::new())
            .unwrap();
        let resp = pool.send(req).await.expect("send failed");
        assert!(resp.status().is_success() || resp.status().is_client_error());
    }

    #[tokio::test]
    async fn connection_pool_health_check() {
        let mut pool = ConnectionPool::new("clob.polymarket.com", 443, 2, AddressFamily::Auto);
        pool.warmup().await.expect("warmup failed");
        let healthy = pool.health_check().await;
        assert_eq!(healthy, 2);
    }
}
