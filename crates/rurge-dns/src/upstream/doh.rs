//! DNS over HTTPS (design §7.2): RFC 8484 POST bodies of type
//! `application/dns-message` through the shared HTTP client (HTTP/2 when the
//! server negotiates it). The message ID is sent as 0 (RFC 8484 §4.1) and the
//! caller's ID is restored on the reply.

use super::{Upstream, UpstreamError};
use bytes::Bytes;
use http::Request;
use http::header::{ACCEPT, CONTENT_TYPE};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use rurge_net::BoxFuture;
use rurge_net::http::{HttpClient, HttpError};
use std::sync::Arc;
use tokio::time::Instant;
use url::Url;

pub const DNS_MESSAGE: &str = "application/dns-message";
pub const MAX_RESPONSE: usize = 65_535;

pub struct DohUpstream {
    name: String,
    url: Url,
    http: Arc<HttpClient>,
}

impl DohUpstream {
    pub fn new(url: Url, http: Arc<HttpClient>) -> DohUpstream {
        DohUpstream {
            name: url.as_str().to_string(),
            url,
            http,
        }
    }
}

impl Upstream for DohUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    fn query<'a>(
        &'a self,
        wire: &'a [u8],
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            if wire.len() < 12 {
                return Err(UpstreamError::BadResponse(
                    "query shorter than a DNS header".to_string(),
                ));
            }
            let id = [wire[0], wire[1]];
            let mut body = wire.to_vec();
            body[0] = 0;
            body[1] = 0;
            let req = Request::post(self.url.as_str())
                .header(CONTENT_TYPE, DNS_MESSAGE)
                .header(ACCEPT, DNS_MESSAGE)
                .body(Full::new(Bytes::from(body)))
                .map_err(|e| UpstreamError::Http(e.to_string()))?;
            let timeout = deadline.saturating_duration_since(Instant::now());
            let resp = self.http.send(req, timeout).await.map_err(|e| match e {
                HttpError::Timeout => UpstreamError::Timeout,
                other => UpstreamError::Http(other.to_string()),
            })?;
            let status = resp.status();
            if status != http::StatusCode::OK {
                return Err(UpstreamError::Http(format!("status {status}")));
            }
            let content_type = resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !content_type.starts_with(DNS_MESSAGE) {
                return Err(UpstreamError::BadResponse(format!(
                    "content-type `{content_type}`"
                )));
            }
            let collected = match tokio::time::timeout_at(
                deadline,
                Limited::new(resp.into_body(), MAX_RESPONSE).collect(),
            )
            .await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    return Err(if e.downcast_ref::<LengthLimitError>().is_some() {
                        UpstreamError::BadResponse("response larger than 65535 bytes".to_string())
                    } else {
                        UpstreamError::Http(e.to_string())
                    });
                }
                Err(_) => return Err(UpstreamError::Timeout),
            };
            let mut bytes = collected.to_bytes().to_vec();
            if bytes.len() < 12 {
                return Err(UpstreamError::BadResponse(
                    "response shorter than a DNS header".to_string(),
                ));
            }
            bytes[0] = id[0];
            bytes[1] = id[1];
            Ok(bytes)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::http::HttpClientConfig;
    use rurge_net::testing::TestServer;
    use std::time::Duration;

    fn client(skip_verify: bool) -> Arc<HttpClient> {
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        Arc::new(
            HttpClient::new(
                connector,
                HttpClientConfig {
                    skip_cert_verification: skip_verify,
                    ..HttpClientConfig::default()
                },
            )
            .unwrap(),
        )
    }

    fn header_with_id(id: u16) -> Vec<u8> {
        let mut v = vec![0u8; 12];
        v[0..2].copy_from_slice(&id.to_be_bytes());
        v.extend_from_slice(b"payload");
        v
    }

    fn deadline(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn posts_dns_message_and_restores_the_id() {
        let server = TestServer::spawn().await;
        let mut canned = vec![0u8; 12];
        canned[2] = 0x81; // QR + RD
        canned.extend_from_slice(b"answer");
        server.set("/dns-query", canned.clone());
        server.set_header("/dns-query", "content-type", "application/dns-message");
        let up = DohUpstream::new(server.url("/dns-query"), client(false));
        assert_eq!(up.name(), server.url("/dns-query").as_str());
        let resp = up
            .query(&header_with_id(0x1234), deadline(2000))
            .await
            .unwrap();
        assert_eq!(&resp[0..2], &[0x12, 0x34]);
        assert_eq!(&resp[2..], &canned[2..]);
        let req = &server.requests()[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.header("content-type"), Some("application/dns-message"));
        assert_eq!(req.header("accept"), Some("application/dns-message"));
    }

    #[tokio::test]
    async fn https_with_http2() {
        let server = TestServer::spawn_tls().await;
        server.set("/dns-query", vec![0u8; 12]);
        server.set_header("/dns-query", "content-type", "application/dns-message");
        let up = DohUpstream::new(server.url("/dns-query"), client(true));
        let resp = up.query(&header_with_id(1), deadline(3000)).await.unwrap();
        assert_eq!(&resp[0..2], &[0, 1]);
        assert_eq!(server.requests()[0].version, "HTTP/2.0");
    }

    #[tokio::test]
    async fn bad_status_and_content_type_are_errors() {
        let server = TestServer::spawn().await;
        server.set("/html", vec![0u8; 12]);
        server.set_header("/html", "content-type", "text/html");
        let up = DohUpstream::new(server.url("/html"), client(false));
        assert!(matches!(
            up.query(&header_with_id(2), deadline(2000)).await,
            Err(UpstreamError::BadResponse(_))
        ));
        server.set("/fail", vec![0u8; 12]);
        server.set_status("/fail", 500);
        let up = DohUpstream::new(server.url("/fail"), client(false));
        assert!(matches!(
            up.query(&header_with_id(3), deadline(2000)).await,
            Err(UpstreamError::Http(_))
        ));
        let up = DohUpstream::new(server.url("/missing"), client(false));
        assert!(matches!(
            up.query(&header_with_id(4), deadline(2000)).await,
            Err(UpstreamError::Http(_))
        ));
    }

    #[tokio::test]
    async fn slow_server_times_out() {
        let server = TestServer::spawn().await;
        server.set("/slow", vec![0u8; 12]);
        server.set_header("/slow", "content-type", "application/dns-message");
        server.set_delay("/slow", Duration::from_secs(3));
        let up = DohUpstream::new(server.url("/slow"), client(false));
        assert_eq!(
            up.query(&header_with_id(5), deadline(200)).await,
            Err(UpstreamError::Timeout)
        );
    }

    #[tokio::test]
    async fn oversized_body_is_bad_response() {
        let server = TestServer::spawn().await;
        server.set("/big", vec![0u8; 70_000]);
        server.set_header("/big", "content-type", "application/dns-message");
        let up = DohUpstream::new(server.url("/big"), client(false));
        assert!(matches!(
            up.query(&header_with_id(6), deadline(2000)).await,
            Err(UpstreamError::BadResponse(_))
        ));
    }
}
