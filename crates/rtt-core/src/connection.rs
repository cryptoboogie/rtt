use bytes::Bytes;
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::client::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::ClientConfig;
use std::fmt;
use std::future::Future;
use std::net::{SocketAddr, ToSocketAddrs};
use std::pin::Pin;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionHealth {
    pub index: usize,
    pub healthy: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionError {
    PoolEmpty,
    Resolve(String),
    Connect {
        address: Option<SocketAddr>,
        error: String,
    },
    Collect {
        connection_index: usize,
        error: String,
    },
    Reconnect {
        connection_index: usize,
        error: String,
    },
}

impl ConnectionError {
    pub fn connection_index(&self) -> Option<usize> {
        match self {
            Self::Collect {
                connection_index, ..
            }
            | Self::Reconnect {
                connection_index, ..
            } => Some(*connection_index),
            Self::PoolEmpty | Self::Resolve(_) | Self::Connect { .. } => None,
        }
    }
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoolEmpty => write!(f, "connection pool is empty"),
            Self::Resolve(err) => write!(f, "failed to resolve host: {err}"),
            Self::Connect { address, error } => match address {
                Some(address) => {
                    write!(f, "failed to establish H2 connection to {address}: {error}")
                }
                None => write!(f, "failed to establish H2 connection: {error}"),
            },
            Self::Collect {
                connection_index,
                error,
            } => write!(
                f,
                "failed while collecting response on connection {connection_index}: {error}"
            ),
            Self::Reconnect {
                connection_index,
                error,
            } => write!(
                f,
                "failed to reconnect connection {connection_index}: {error}"
            ),
        }
    }
}

impl std::error::Error for ConnectionError {}

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
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
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

async fn connect_sender_for_addr(
    host: &str,
    addr: SocketAddr,
) -> Result<H2Sender, ConnectionError> {
    let tcp = TcpStream::connect(addr)
        .await
        .map_err(|err| ConnectionError::Connect {
            address: Some(addr),
            error: err.to_string(),
        })?;
    tcp.set_nodelay(true)
        .map_err(|err| ConnectionError::Connect {
            address: Some(addr),
            error: err.to_string(),
        })?;

    let tls_config = make_tls_config();
    let connector = TlsConnector::from(tls_config);
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|err| {
        ConnectionError::Connect {
            address: Some(addr),
            error: err.to_string(),
        }
    })?;
    let tls_stream =
        connector
            .connect(server_name, tcp)
            .await
            .map_err(|err| ConnectionError::Connect {
                address: Some(addr),
                error: err.to_string(),
            })?;

    let io = TokioIo::new(tls_stream);
    let (sender, conn) = http2::handshake(TokioExecutor::new(), io)
        .await
        .map_err(|err| ConnectionError::Connect {
            address: Some(addr),
            error: err.to_string(),
        })?;
    tokio::spawn(async move {
        if let Err(err) = conn.await {
            tracing::warn!(error = %err, "background h2 connection task failed");
        }
    });

    Ok(sender)
}

/// Establish a single HTTP/2 connection.
pub async fn connect_h2(
    host: &str,
    port: u16,
    af: AddressFamily,
) -> Result<H2Sender, ConnectionError> {
    let addrs = resolve(host, port, af).map_err(|err| ConnectionError::Resolve(err.to_string()))?;
    let mut last_error = None;

    for addr in addrs {
        match connect_sender_for_addr(host, addr).await {
            Ok(sender) => return Ok(sender),
            Err(err) => last_error = Some(err),
        }
    }

    Err(last_error.unwrap_or(ConnectionError::Connect {
        address: None,
        error: "all resolved addresses failed".to_string(),
    }))
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

/// Handle returned by `send_start`. The H2 frame has been dispatched;
/// call `collect()` to await the response.
pub struct SendHandle {
    resp_future:
        Pin<Box<dyn Future<Output = hyper::Result<hyper::Response<hyper::body::Incoming>>> + Send>>,
    pub connection_index: usize,
}

impl SendHandle {
    /// Await the response and collect the body.
    pub async fn collect(self) -> Result<Response<Bytes>, ConnectionError> {
        let connection_index = self.connection_index;
        let resp = self
            .resp_future
            .await
            .map_err(|err| ConnectionError::Collect {
                connection_index,
                error: err.to_string(),
            })?;
        let (parts, body) = resp.into_parts();
        let body_bytes = body
            .collect()
            .await
            .map_err(|err| ConnectionError::Collect {
                connection_index,
                error: err.to_string(),
            })?;
        Ok(Response::from_parts(parts, body_bytes.to_bytes()))
    }
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
    pub async fn warmup(&mut self) -> Result<usize, ConnectionError> {
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

    /// Acquire a connection (round-robin). Returns (connection, index).
    fn acquire(&self) -> Result<(Arc<Mutex<H2Connection>>, usize), ConnectionError> {
        if self.connections.is_empty() {
            return Err(ConnectionError::PoolEmpty);
        }

        let idx = self.next_index.fetch_add(1, Ordering::Relaxed) % self.connections.len();
        Ok((self.connections[idx].clone(), idx))
    }

    async fn ensure_healthy(
        &self,
        index: usize,
        conn: &Arc<Mutex<H2Connection>>,
    ) -> Result<(), ConnectionError> {
        let healthy = conn.lock().await.healthy;
        if healthy {
            Ok(())
        } else {
            self.reconnect(index).await
        }
    }

    pub async fn report_failure(&self, index: usize) -> Result<(), ConnectionError> {
        let Some(conn) = self.connections.get(index) else {
            return Err(ConnectionError::Reconnect {
                connection_index: index,
                error: "connection index out of range".to_string(),
            });
        };

        let mut guard = conn.lock().await;
        guard.healthy = false;
        drop(guard);

        self.reconnect(index).await
    }

    /// Submit a request to the H2 pipeline. Returns a `SendHandle` immediately
    /// after the frame is dispatched (no network wait). Prefer
    /// `ConnectionPool::collect(handle)` so failed collections trigger reconnects
    /// before the connection is reused.
    pub async fn send_start(&self, req: Request<Bytes>) -> Result<SendHandle, ConnectionError> {
        let (conn, idx) = self.acquire()?;
        self.ensure_healthy(idx, &conn).await?;

        let mut guard = conn.lock().await;
        let req = req.map(Full::new);
        let resp_future = guard.sender.send_request(req);
        drop(guard);

        Ok(SendHandle {
            resp_future: Box::pin(resp_future),
            connection_index: idx,
        })
    }

    /// Await a response from a `SendHandle` and reconnect the underlying
    /// connection before reuse if collection fails.
    pub async fn collect(&self, handle: SendHandle) -> Result<Response<Bytes>, ConnectionError> {
        let idx = handle.connection_index;
        match handle.collect().await {
            Ok(resp) => Ok(resp),
            Err(err) => {
                let _ = self.report_failure(idx).await;
                Err(err)
            }
        }
    }

    /// Send a request and await the full response. Convenience wrapper
    /// around `send_start` + `collect`. Returns (response, connection_index).
    pub async fn send(
        &self,
        req: Request<Bytes>,
    ) -> Result<(Response<Bytes>, usize), ConnectionError> {
        let handle = self.send_start(req).await?;
        let idx = handle.connection_index;
        let resp = self.collect(handle).await?;
        Ok((resp, idx))
    }

    /// Reconnect a specific connection index.
    pub async fn reconnect(&self, index: usize) -> Result<(), ConnectionError> {
        let sender = connect_h2(&self.host, self.port, self.af)
            .await
            .map_err(|err| ConnectionError::Reconnect {
                connection_index: index,
                error: err.to_string(),
            })?;

        let Some(conn) = self.connections.get(index) else {
            return Err(ConnectionError::Reconnect {
                connection_index: index,
                error: "connection index out of range".to_string(),
            });
        };

        let mut guard = conn.lock().await;
        guard.sender = sender;
        guard.healthy = true;
        Ok(())
    }

    pub async fn health_check_detailed(&self) -> Vec<ConnectionHealth> {
        let mut statuses = Vec::with_capacity(self.connections.len());

        for (index, conn) in self.connections.iter().enumerate() {
            let req = Request::builder()
                .method("GET")
                .uri("/")
                .header("host", self.host.as_str())
                .body(Full::new(Bytes::new()))
                .unwrap();

            let (healthy, last_error) = {
                let mut guard = conn.lock().await;
                match guard.sender.send_request(req).await {
                    Ok(resp) => match resp.into_body().collect().await {
                        Ok(_) => {
                            guard.healthy = true;
                            (true, None)
                        }
                        Err(err) => {
                            guard.healthy = false;
                            (false, Some(err.to_string()))
                        }
                    },
                    Err(err) => {
                        guard.healthy = false;
                        (false, Some(err.to_string()))
                    }
                }
            };

            if !healthy {
                let _ = self.reconnect(index).await;
            }

            statuses.push(ConnectionHealth {
                index,
                healthy,
                last_error,
            });
        }

        statuses
    }

    /// Health check all connections by sending GET /.
    pub async fn health_check(&self) -> usize {
        self.health_check_detailed()
            .await
            .into_iter()
            .filter(|status| status.healthy)
            .count()
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
    use tokio::net::TcpListener;

    async fn connect_tcp_first_available(
        addrs: &[SocketAddr],
    ) -> Result<TcpStream, ConnectionError> {
        let mut last_error = None;
        for addr in addrs {
            match TcpStream::connect(addr).await {
                Ok(stream) => return Ok(stream),
                Err(err) => {
                    last_error = Some(ConnectionError::Connect {
                        address: Some(*addr),
                        error: err.to_string(),
                    });
                }
            }
        }

        Err(last_error.unwrap_or(ConnectionError::Connect {
            address: None,
            error: "no addresses available".to_string(),
        }))
    }

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
    fn resolve_ip_literal_v4() {
        let addrs = resolve("127.0.0.1", 443, AddressFamily::Auto).unwrap();
        assert_eq!(addrs.len(), 1);
        assert!(addrs[0].is_ipv4());
    }

    #[test]
    fn resolve_filters_requested_family() {
        let addrs = resolve("127.0.0.1", 443, AddressFamily::V4).unwrap();
        assert!(addrs.iter().all(|a| a.is_ipv4()));
        assert!(resolve("127.0.0.1", 443, AddressFamily::V6).is_err());

        let addrs = resolve("::1", 443, AddressFamily::V6).unwrap();
        assert!(addrs.iter().all(|a| a.is_ipv6()));
        assert!(resolve("::1", 443, AddressFamily::V4).is_err());
    }

    #[tokio::test]
    async fn connect_tcp_helper_tries_next_address() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let good_addr = listener.local_addr().unwrap();

        let temp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bad_addr = temp.local_addr().unwrap();
        drop(temp);

        let accept_task = tokio::spawn(async move {
            let _ = listener.accept().await.unwrap();
        });

        let stream = connect_tcp_first_available(&[bad_addr, good_addr])
            .await
            .expect("should connect to the second address");
        assert_eq!(stream.peer_addr().unwrap(), good_addr);

        accept_task.await.unwrap();
    }
}
