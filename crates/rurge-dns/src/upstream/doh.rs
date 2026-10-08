//! DNS over HTTPS (design §7.2): RFC 8484 POST bodies of type
//! `application/dns-message` through the shared HTTP client (HTTP/2 when the
//! server negotiates it). The message ID is sent as 0 (RFC 8484 §4.1) and the
//! caller's ID is restored on the reply.

use super::{Upstream, UpstreamError};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use http::{HeaderMap, HeaderValue, Request, StatusCode};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use percent_encoding::percent_decode_str;
use rurge_net::BoxFuture;
use rurge_net::http::{HttpClient, HttpError};
use std::sync::Arc;
use tokio::time::Instant;
use url::Url;

pub const DNS_MESSAGE: &str = "application/dns-message";
pub const MAX_RESPONSE: usize = 65_535;

/// How much of what the server calls its content an error quotes: a field
/// value may be as long as a field section, and every query would log it.
const QUOTED_MAX: usize = 200;

/// The head of an answer to a DoH POST — over HTTP/2 or HTTP/3 — is one
/// (RFC 8484 4.2): status 200, and a body of `application/dns-message`,
/// whatever the case or the parameters of the type.
pub(super) fn check_head(status: StatusCode, headers: &HeaderMap) -> Result<(), UpstreamError> {
    if status != StatusCode::OK {
        return Err(UpstreamError::Http(format!("status {status}")));
    }
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.starts_with(DNS_MESSAGE) {
        let quoted: String = content_type.chars().take(QUOTED_MAX).collect();
        return Err(UpstreamError::BadResponse(format!(
            "content-type `{quoted}`"
        )));
    }
    Ok(())
}

/// A body of more than `MAX_RESPONSE` bytes.
pub(super) fn too_large() -> UpstreamError {
    UpstreamError::BadResponse(format!("response larger than {MAX_RESPONSE} bytes"))
}

/// The body of an answer is a DNS message: it has a header, and is at most
/// `MAX_RESPONSE` bytes.
pub(super) fn check_body(body: &[u8]) -> Result<(), UpstreamError> {
    if body.len() > MAX_RESPONSE {
        return Err(too_large());
    }
    if body.len() < 12 {
        return Err(UpstreamError::BadResponse(
            "response shorter than a DNS header".to_string(),
        ));
    }
    Ok(())
}

pub struct DohUpstream {
    name: String,
    /// Where the requests go: the URL without a user name and a password.
    url: Url,
    /// What the URL's user name and password are sent as, when it has them.
    authorization: Option<HeaderValue>,
    http: Arc<HttpClient>,
}

impl DohUpstream {
    pub fn new(mut url: Url, http: Arc<HttpClient>) -> DohUpstream {
        let name = super::doh_name(&url);
        let authorization = basic_authorization(&url);
        // an https URL has the authority these belong to: neither fails
        let _ = url.set_username("");
        let _ = url.set_password(None);
        DohUpstream {
            name,
            url,
            authorization,
            http,
        }
    }
}

/// `Basic base64(user:password)` (RFC 7617) of the user name and the
/// password `url` carries, percent-decoded; `None` without either. HTTP has
/// no other place for them: a request's target takes none (RFC 9110 4.2.4),
/// and HTTP/2 forbids them in `:authority` (RFC 9113 8.3.1).
fn basic_authorization(url: &Url) -> Option<HeaderValue> {
    if url.username().is_empty() && url.password().is_none() {
        return None;
    }
    let mut pair = percent_decode_str(url.username()).collect::<Vec<u8>>();
    pair.push(b':');
    pair.extend(percent_decode_str(url.password().unwrap_or_default()));
    let mut value = HeaderValue::from_str(&format!("Basic {}", STANDARD.encode(pair))).ok()?;
    value.set_sensitive(true);
    Some(value)
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
            let mut req = Request::post(self.url.as_str())
                .header(CONTENT_TYPE, DNS_MESSAGE)
                .header(ACCEPT, DNS_MESSAGE);
            if let Some(authorization) = &self.authorization {
                req = req.header(AUTHORIZATION, authorization);
            }
            let req = req
                .body(Full::new(Bytes::from(body)))
                .map_err(|e| UpstreamError::Http(e.to_string()))?;
            let timeout = deadline.saturating_duration_since(Instant::now());
            let resp = self.http.send(req, timeout).await.map_err(|e| match e {
                HttpError::Timeout => UpstreamError::Timeout,
                other => UpstreamError::Http(other.to_string()),
            })?;
            check_head(resp.status(), resp.headers())?;
            let collected = match tokio::time::timeout_at(
                deadline,
                Limited::new(resp.into_body(), MAX_RESPONSE).collect(),
            )
            .await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    return Err(if e.downcast_ref::<LengthLimitError>().is_some() {
                        too_large()
                    } else {
                        UpstreamError::Http(e.to_string())
                    });
                }
                Err(_) => return Err(UpstreamError::Timeout),
            };
            let mut bytes = collected.to_bytes().to_vec();
            check_body(&bytes)?;
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

    #[test]
    fn the_head_of_an_answer_is_a_200_of_a_dns_message() {
        let headers = |content_type: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, content_type.parse().unwrap());
            headers
        };
        for content_type in [
            "application/dns-message",
            "application/dns-message; charset=utf-8",
            "Application/DNS-Message",
        ] {
            assert_eq!(
                check_head(StatusCode::OK, &headers(content_type)),
                Ok(()),
                "{content_type}"
            );
        }
        assert_eq!(
            check_head(StatusCode::NOT_FOUND, &headers("application/dns-message")),
            Err(UpstreamError::Http("status 404 Not Found".to_string()))
        );
        assert_eq!(
            check_head(StatusCode::OK, &headers("Text/HTML")),
            Err(UpstreamError::BadResponse(
                "content-type `text/html`".to_string()
            ))
        );
        assert_eq!(
            check_head(StatusCode::OK, &HeaderMap::new()),
            Err(UpstreamError::BadResponse("content-type ``".to_string()))
        );
    }

    /// What the server calls its content goes into the error cut short:
    /// a field value may be as long as a field section.
    #[test]
    fn a_long_content_type_is_cut_short() {
        let long = format!("text/{}", "x".repeat(60_000));
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, long.parse().unwrap());
        assert_eq!(
            check_head(StatusCode::OK, &headers),
            Err(UpstreamError::BadResponse(format!(
                "content-type `text/{}`",
                "x".repeat(195)
            )))
        );
    }

    /// A user name and a password in the URL go as `Authorization: Basic`
    /// (RFC 7617), percent-decoded, and leave the request's target: HTTP
    /// has no other place for them, and HTTP/2 forbids them in `:authority`
    /// (RFC 9113 8.3.1). A URL without them sends no such header.
    #[tokio::test]
    async fn a_urls_user_name_and_password_go_as_basic_authorization() {
        for tls in [false, true] {
            let server = if tls {
                TestServer::spawn_tls().await
            } else {
                TestServer::spawn().await
            };
            server.set("/dns-query", vec![0u8; 12]);
            server.set_header("/dns-query", "content-type", "application/dns-message");
            let mut url = server.url("/dns-query");
            url.set_username("al ice").unwrap();
            url.set_password(Some("s3cret:doh")).unwrap();
            assert_eq!(url.username(), "al%20ice", "the URL holds them encoded");
            let up = DohUpstream::new(url, client(tls));
            up.query(&header_with_id(7), deadline(3000)).await.unwrap();
            let plain = DohUpstream::new(server.url("/dns-query"), client(tls));
            plain
                .query(&header_with_id(8), deadline(3000))
                .await
                .unwrap();
            let requests = server.requests();
            assert_eq!(
                requests[0].header("authorization"),
                // base64("al ice:s3cret:doh")
                Some("Basic YWwgaWNlOnMzY3JldDpkb2g="),
                "tls: {tls}"
            );
            assert_eq!(requests[1].header("authorization"), None, "tls: {tls}");
            for request in &requests {
                for (name, value) in &request.headers {
                    assert!(
                        name == "authorization" || !value.contains("s3cret"),
                        "tls: {tls}: {name}: {value}"
                    );
                    assert!(!value.contains('@'), "tls: {tls}: {name}: {value}");
                }
            }
        }
    }

    /// A redirect is an answer that is none, not somewhere to go next: the
    /// credentials of the URL are never carried to where a server points.
    #[tokio::test]
    async fn a_redirect_is_not_followed() {
        let elsewhere = TestServer::spawn().await;
        elsewhere.set("/dns-query", vec![0u8; 12]);
        elsewhere.set_header("/dns-query", "content-type", "application/dns-message");
        let server = TestServer::spawn().await;
        server.set("/dns-query", vec![0u8; 12]);
        server.set_status("/dns-query", 307);
        server.set_header(
            "/dns-query",
            "location",
            elsewhere.url("/dns-query").as_str(),
        );
        let mut url = server.url("/dns-query");
        url.set_username("alice").unwrap();
        url.set_password(Some("s3cret")).unwrap();
        let up = DohUpstream::new(url, client(false));
        assert!(matches!(
            up.query(&header_with_id(9), deadline(2000)).await,
            Err(UpstreamError::Http(_))
        ));
        assert_eq!(server.requests().len(), 1);
        assert!(elsewhere.requests().is_empty());
    }

    /// A user name and a password in the URL stay out of the upstream's
    /// name.
    #[tokio::test]
    async fn the_name_leaves_the_urls_credentials_out() {
        let url: Url = "https://alice:s3cret-doh@dns.example/dns-query"
            .parse()
            .unwrap();
        let up = DohUpstream::new(url, client(false));
        assert_eq!(up.name(), "https://dns.example/dns-query");
    }

    #[test]
    fn the_body_of_an_answer_is_a_dns_message_of_at_most_65535_bytes() {
        assert_eq!(check_body(&[0; 12]), Ok(()));
        assert_eq!(check_body(&vec![0; MAX_RESPONSE]), Ok(()));
        assert_eq!(
            check_body(&[0; 11]),
            Err(UpstreamError::BadResponse(
                "response shorter than a DNS header".to_string()
            ))
        );
        let too_large = Err(UpstreamError::BadResponse(
            "response larger than 65535 bytes".to_string(),
        ));
        assert_eq!(check_body(&vec![0; MAX_RESPONSE + 1]), too_large);
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
