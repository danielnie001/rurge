//! A small HTTP client for the daemon's API (M4 design §7): plain HTTP, or
//! HTTPS when `http-api-tls` is on, 5 s per request, `X-Key` header, JSON in
//! and out.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::ServerName;
use serde_json::Value;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;
use tokio_rustls::TlsConnector;

const TIMEOUT: Duration = Duration::from_secs(5);

/// How the client reaches the API: plain HTTP, or HTTPS with the profile's
/// MITM CA as the only root (`phase 4 M2 design 14.2`). The plain client is
/// boxed: it is much larger than the TLS one, and one of these exists for as
/// long as a command runs.
enum Transport {
    Plain(Box<Client<HttpConnector, Full<Bytes>>>),
    Tls {
        addr: SocketAddr,
        connector: TlsConnector,
        name: ServerName<'static>,
    },
}

pub struct ApiClient {
    base: String,
    key: String,
    transport: Transport,
}

#[derive(Debug)]
pub enum ClientError {
    Unreachable(String),
    Timeout,
    BadBody(String),
    /// The status line arrived but the body did not (the peer closed the
    /// connection, or the deadline hit while reading it). `rurge stop` accepts
    /// this behind a 2xx: the daemon may exit before its `{}` is fully flushed.
    BodyLost {
        status: u16,
    },
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Unreachable(e) => write!(f, "{e}"),
            ClientError::Timeout => write!(f, "timed out after {} s", TIMEOUT.as_secs()),
            ClientError::BadBody(e) => write!(f, "invalid response body: {e}"),
            ClientError::BodyLost { status } => {
                write!(
                    f,
                    "answered {status} but the connection closed before the body"
                )
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl ApiClient {
    pub fn new(addr: SocketAddr, key: String) -> ApiClient {
        ApiClient {
            base: format!("http://{addr}"),
            key,
            transport: Transport::Plain(Box::new(
                Client::builder(TokioExecutor::new()).build_http(),
            )),
        }
    }

    /// HTTPS to `addr`, trusting `ca_der` alone and checking the
    /// certificate against the IP of `addr`.
    pub fn with_tls(addr: SocketAddr, key: String, ca_der: &[u8]) -> anyhow::Result<ApiClient> {
        let config = rurge_http::mitm::api::client_config(ca_der)?;
        Ok(ApiClient {
            base: format!("https://{addr}"),
            key,
            transport: Transport::Tls {
                addr,
                connector: TlsConnector::from(config),
                // rustls sends no SNI for an IP address, and the API signs for
                // the local address of the connection then (phase 4 M2 design
                // 14.1), so the address is what the certificate is checked
                // against.
                name: ServerName::IpAddress(addr.ip().into()),
            },
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn get(&self, path: &str) -> Result<(u16, Value), ClientError> {
        self.call("GET", path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<(u16, Value), ClientError> {
        self.call("POST", path, Some(body)).await
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), ClientError> {
        // What each client is given, not what goes on the wire. The plain
        // client gets an absolute URI: it takes the address to dial and the
        // `Host` header from it, and then writes the URI as origin form.
        // `client::conn` has no such step — it writes the URI it is given — so
        // the TLS branch hands it the origin form and adds `Host` itself.
        let (uri, host) = match &self.transport {
            Transport::Plain(_) => (format!("{}{path}", self.base), None),
            Transport::Tls { addr, .. } => (path.to_string(), Some(addr.to_string())),
        };
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-key", &self.key);
        if let Some(host) = host {
            req = req.header("host", host);
        }
        let body = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                Full::new(Bytes::from(v.to_string()))
            }
            None => Full::new(Bytes::new()),
        };
        let req = req
            .body(body)
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
        // One deadline for the whole exchange, not one per phase. `seen_status`
        // survives the timeout so a deadline hit *after* the status line is
        // still reported as `BodyLost` rather than a bare `Timeout`.
        let mut seen_status: Option<u16> = None;
        let exchange = async {
            let resp = match &self.transport {
                Transport::Plain(client) => client
                    .request(req)
                    .await
                    .map_err(|e| ClientError::Unreachable(e.to_string()))?,
                // one connection per call: a command makes only a handful of
                // requests, which is not worth reusing a connection for
                // (`the phase 4 / M2c plan's P14`)
                Transport::Tls {
                    addr,
                    connector,
                    name,
                } => {
                    let tcp = tokio::net::TcpStream::connect(addr)
                        .await
                        .map_err(|e| ClientError::Unreachable(e.to_string()))?;
                    let tls = connector
                        .connect(name.clone(), tcp)
                        .await
                        .map_err(|e| ClientError::Unreachable(e.to_string()))?;
                    let (mut sender, connection) =
                        hyper::client::conn::http1::handshake(TokioIo::new(tls))
                            .await
                            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
                    tokio::spawn(connection);
                    sender
                        .send_request(req)
                        .await
                        .map_err(|e| ClientError::Unreachable(e.to_string()))?
                }
            };
            let status = resp.status().as_u16();
            seen_status = Some(status);
            let bytes = resp
                .into_body()
                .collect()
                .await
                .map_err(|_| ClientError::BodyLost { status })?
                .to_bytes();
            let value = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&bytes).map_err(|e| {
                    ClientError::BadBody(format!("{e}: {}", String::from_utf8_lossy(&bytes)))
                })?
            };
            Ok((status, value))
        };
        let result = tokio::time::timeout(TIMEOUT, exchange).await;
        match result {
            Ok(r) => r,
            Err(_) => match seen_status {
                Some(status) => Err(ClientError::BodyLost { status }),
                None => Err(ClientError::Timeout),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_name_the_failed_phase() {
        assert_eq!(
            ClientError::BodyLost { status: 200 }.to_string(),
            "answered 200 but the connection closed before the body"
        );
        assert_eq!(ClientError::Timeout.to_string(), "timed out after 5 s");
        assert_eq!(
            ClientError::BadBody("eof".into()).to_string(),
            "invalid response body: eof"
        );
    }
}
