//! A small HTTP client for the daemon's API (M4 design §7): plain HTTP, 5 s
//! per request, `X-Key` header, JSON in and out.

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(5);

pub struct ApiClient {
    base: String,
    key: String,
    client: Client<HttpConnector, Full<Bytes>>,
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
            client: Client::builder(TokioExecutor::new()).build_http(),
        }
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
        let mut req = Request::builder()
            .method(method)
            .uri(format!("{}{path}", self.base))
            .header("x-key", &self.key);
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
            let resp = self
                .client
                .request(req)
                .await
                .map_err(|e| ClientError::Unreachable(e.to_string()))?;
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
