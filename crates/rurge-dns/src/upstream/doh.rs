//! DNS over HTTPS (design §7.2): RFC 8484 POST bodies of type
//! `application/dns-message` through the shared HTTP client (HTTP/2 when the
//! server negotiates it). The message ID is sent as 0 (RFC 8484 §4.1) and the
//! caller's ID is restored on the reply, whose records' TTLs are lowered by
//! the age it comes with (RFC 8484 §5.1).

use super::{Upstream, UpstreamError};
use crate::message::lower_ttls;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::header::{ACCEPT, AGE, AUTHORIZATION, CONTENT_TYPE};
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

/// What is handed on of an answer whose head passed `check_head` — over
/// HTTP/2 or HTTP/3 —: its `body`, which is a DNS message (`check_body`),
/// the TTLs of its records lowered by the age its `headers` say (`age`).
pub(super) fn answer(headers: &HeaderMap, mut body: Vec<u8>) -> Result<Vec<u8>, UpstreamError> {
    check_body(&body)?;
    lower_ttls(&mut body, age(headers));
    Ok(body)
}

/// How long ago the answer was made, in seconds, by its `Age`, which the
/// TTLs of its records do not count (RFC 8484 5.1): the first member of the
/// field read as the list it may come as (RFC 9111 5.1) — its lines in the
/// order they came, each split at its commas, the spaces and tabs around a
/// member and the empty members left out (RFC 9110 5.6.1) —, whatever
/// follows it. 0 — the answer is taken as it is — without the field, or
/// with a first member that is no number.
fn age(headers: &HeaderMap) -> u32 {
    let first = headers
        .get_all(AGE)
        .iter()
        .flat_map(|line| line.as_bytes().split(|&b| b == b','))
        .map(without_whitespace_around)
        .find(|member| !member.is_empty());
    match first {
        Some(digits) if digits.iter().all(u8::is_ascii_digit) => digits
            .iter()
            .map(|digit| u32::from(digit - b'0'))
            .try_fold(0u32, |seconds, digit| {
                seconds.checked_mul(10)?.checked_add(digit)
            })
            // more seconds than a TTL counts: older than any record lasts
            .unwrap_or(u32::MAX),
        _ => 0,
    }
}

/// `bytes` without the optional whitespace of a field value around them:
/// spaces and tabs (RFC 9110 5.6.3).
fn without_whitespace_around(mut bytes: &[u8]) -> &[u8] {
    while let [b' ' | b'\t', rest @ ..] = bytes {
        bytes = rest;
    }
    while let [rest @ .., b' ' | b'\t'] = bytes {
        bytes = rest;
    }
    bytes
}

/// A body of more than `MAX_RESPONSE` bytes.
pub(super) fn too_large() -> UpstreamError {
    UpstreamError::BadResponse(format!("response larger than {MAX_RESPONSE} bytes"))
}

/// The body of an answer is a DNS message: it has a header, and is at most
/// `MAX_RESPONSE` bytes.
fn check_body(body: &[u8]) -> Result<(), UpstreamError> {
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
    /// The server of `url` (`UpstreamSpec::Https`'s), where its requests
    /// go; `pathless`: the URL was written without its path, which the
    /// upstream's name leaves out.
    pub fn new(mut url: Url, pathless: bool, http: Arc<HttpClient>) -> DohUpstream {
        let name = super::doh_name(&url, pathless);
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
            let mut resp = self.http.send(req, timeout).await.map_err(|e| match e {
                HttpError::Timeout => UpstreamError::Timeout,
                other => UpstreamError::Http(other.to_string()),
            })?;
            check_head(resp.status(), resp.headers())?;
            let collected = match tokio::time::timeout_at(
                deadline,
                Limited::new(resp.body_mut(), MAX_RESPONSE).collect(),
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
            let mut bytes = answer(resp.headers(), collected.to_bytes().to_vec())?;
            bytes[0] = id[0];
            bytes[1] = id[1];
            Ok(bytes)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Qtype, Question, Rcode, build_query, build_response, parse_response};
    use crate::upstream::UpstreamSpec;
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

    /// What the canned answers answer.
    fn question() -> Question {
        Question {
            name: "a.test".to_string(),
            qtype: Qtype::A,
        }
    }

    /// An answer to `question` with an A record of each of `ttls`, and at
    /// its end an OPT record (RFC 6891 6.1.2) whose TTL field — no TTL: it
    /// holds the DO flag — an ageing that took it for one would change.
    fn canned(id: u16, ttls: [u32; 2]) -> Vec<u8> {
        let records = [
            ("10.0.0.1".parse().unwrap(), ttls[0]),
            ("10.0.0.2".parse().unwrap(), ttls[1]),
        ];
        let mut wire = build_response(id, &question(), Rcode::NoError, &records, false).unwrap();
        // the root name, OPT, a payload size of 1232, the DO flag, no data
        wire.extend([0, 0, 41, 0x04, 0xd0, 0, 0, 0x80, 0, 0, 0]);
        // ARCOUNT
        wire[11] += 1;
        wire
    }

    #[tokio::test]
    async fn posts_dns_message_and_restores_the_id() {
        let server = TestServer::spawn().await;
        let mut canned = vec![0u8; 12];
        canned[2] = 0x81; // QR + RD
        canned.extend_from_slice(b"answer");
        server.set("/dns-query", canned.clone());
        server.set_header("/dns-query", "content-type", "application/dns-message");
        let up = DohUpstream::new(server.url("/dns-query"), false, client(false));
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
        let up = DohUpstream::new(server.url("/dns-query"), false, client(true));
        let resp = up.query(&header_with_id(1), deadline(3000)).await.unwrap();
        assert_eq!(&resp[0..2], &[0, 1]);
        assert_eq!(server.requests()[0].version, "HTTP/2.0");
    }

    #[tokio::test]
    async fn bad_status_and_content_type_are_errors() {
        let server = TestServer::spawn().await;
        server.set("/html", vec![0u8; 12]);
        server.set_header("/html", "content-type", "text/html");
        let up = DohUpstream::new(server.url("/html"), false, client(false));
        assert!(matches!(
            up.query(&header_with_id(2), deadline(2000)).await,
            Err(UpstreamError::BadResponse(_))
        ));
        server.set("/fail", vec![0u8; 12]);
        server.set_status("/fail", 500);
        let up = DohUpstream::new(server.url("/fail"), false, client(false));
        assert!(matches!(
            up.query(&header_with_id(3), deadline(2000)).await,
            Err(UpstreamError::Http(_))
        ));
        let up = DohUpstream::new(server.url("/missing"), false, client(false));
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
        let up = DohUpstream::new(server.url("/slow"), false, client(false));
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
            let up = DohUpstream::new(url, false, client(tls));
            up.query(&header_with_id(7), deadline(3000)).await.unwrap();
            let plain = DohUpstream::new(server.url("/dns-query"), false, client(tls));
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
        let up = DohUpstream::new(url, false, client(false));
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
        let up = DohUpstream::new(url, false, client(false));
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

    /// What is handed on of an answer is its body as old as its head says
    /// — or the error of a body that is no DNS message —, in the one step
    /// both transports take with it.
    #[test]
    fn an_answer_is_handed_on_checked_and_as_old_as_its_head_says() {
        let mut aged = HeaderMap::new();
        aged.insert(AGE, HeaderValue::from_static("30"));
        let fresh = HeaderMap::new();
        assert_eq!(answer(&aged, canned(7, [100, 20])), Ok(canned(7, [70, 0])));
        assert_eq!(
            answer(&fresh, canned(7, [100, 20])),
            Ok(canned(7, [100, 20]))
        );
        for headers in [&aged, &fresh] {
            assert_eq!(
                answer(headers, vec![0; 11]),
                Err(UpstreamError::BadResponse(
                    "response shorter than a DNS header".to_string()
                ))
            );
        }
    }

    #[tokio::test]
    async fn oversized_body_is_bad_response() {
        let server = TestServer::spawn().await;
        server.set("/big", vec![0u8; 70_000]);
        server.set_header("/big", "content-type", "application/dns-message");
        let up = DohUpstream::new(server.url("/big"), false, client(false));
        assert!(matches!(
            up.query(&header_with_id(6), deadline(2000)).await,
            Err(UpstreamError::BadResponse(_))
        ));
    }

    /// A server written without a path is asked at `/dns-query`, under the
    /// name it was written with; written with the path `/`, it is asked
    /// there. Written without a path and with a user name and a password,
    /// it is asked at `/dns-query` with them as its `Authorization`, under
    /// the name that leaves them out.
    #[tokio::test]
    async fn a_url_written_without_a_path_posts_to_dns_query() {
        let server = TestServer::spawn_tls().await;
        for (path, address) in [("/dns-query", "10.0.0.1"), ("/", "10.0.0.2")] {
            let records = [(address.parse().unwrap(), 60)];
            let message = build_response(0, &question(), Rcode::NoError, &records, false);
            server.set(path, message.unwrap());
            server.set_header(path, "content-type", "application/dns-message");
        }
        let authority = format!("127.0.0.1:{}", server.url("/").port().unwrap());
        let bare = format!("https://{authority}");
        for (written, name, address) in [
            (bare.clone(), bare.clone(), "10.0.0.1"),
            (format!("{bare}/"), format!("{bare}/"), "10.0.0.2"),
            (
                format!("https://al1ce:s3cret@{authority}"),
                bare.clone(),
                "10.0.0.1",
            ),
        ] {
            let up = match UpstreamSpec::parse(&written).unwrap() {
                UpstreamSpec::Https { url, pathless } => {
                    DohUpstream::new(url, pathless, client(true))
                }
                other => panic!("{other:?}"),
            };
            assert_eq!(up.name(), name, "{written}");
            let wire = build_query(0x1234, &question()).unwrap();
            let reply = up.query(&wire, deadline(3000)).await.unwrap();
            let parsed = parse_response(&reply).unwrap();
            assert_eq!(parsed.id, 0x1234);
            assert_eq!(parsed.v4, [(address.parse().unwrap(), 60)], "{written}");
        }
        let requests = server.requests();
        let asked: Vec<_> = requests
            .iter()
            .map(|r| {
                (
                    r.method.as_str(),
                    r.path.as_str(),
                    r.header("authorization"),
                )
            })
            .collect();
        assert_eq!(
            asked,
            [
                ("POST", "/dns-query", None),
                ("POST", "/", None),
                // base64("al1ce:s3cret")
                ("POST", "/dns-query", Some("Basic YWwxY2U6czNjcmV0")),
            ]
        );
    }

    /// The age an answer comes with is off the TTL of each of its records
    /// (RFC 8484 5.1), no further than 0, and off nothing else: the OPT
    /// record's TTL field, which is none, and the rest of the message come
    /// as the server sent them. Of two `Age` fields, or of a list in one,
    /// the first counts (RFC 9111 5.1); without an `Age`, or with one that
    /// is no number, the answer comes as it is. Over HTTP/1.1 and over
    /// HTTP/2.
    #[tokio::test]
    async fn the_age_of_an_answer_comes_off_its_ttls() {
        for tls in [false, true] {
            let server = if tls {
                TestServer::spawn_tls().await
            } else {
                TestServer::spawn().await
            };
            let http = client(tls);
            for (path, ages, ttls) in [
                ("/aged", &["30"][..], [70, 0]),
                ("/fresh", &["0"][..], [100, 20]),
                ("/no-age", &[][..], [100, 20]),
                ("/no-number", &["abc"][..], [100, 20]),
                ("/two-ages", &["30", "90"][..], [70, 0]),
                ("/listed", &["30, 90"][..], [70, 0]),
            ] {
                server.set(path, canned(0, [100, 20]));
                server.set_header(path, "content-type", "application/dns-message");
                for age in ages {
                    server.set_header(path, "age", age);
                }
                let up = DohUpstream::new(server.url(path), false, http.clone());
                let wire = build_query(0x1234, &question()).unwrap();
                let reply = up.query(&wire, deadline(3000)).await.unwrap();
                let handed_on: Vec<u32> = parse_response(&reply)
                    .unwrap()
                    .v4
                    .iter()
                    .map(|&(_, ttl)| ttl)
                    .collect();
                assert_eq!(handed_on, ttls, "tls: {tls}, {path}");
                assert_eq!(reply, canned(0x1234, ttls), "tls: {tls}, {path}");
            }
        }
    }

    /// An answer's age is the number the first member of its `Age` says, in
    /// seconds, whatever follows it: the field is a list — its lines in the
    /// order they came, each split at its commas, spaces and tabs around a
    /// member and empty members aside (RFC 9111 5.1, RFC 9110 5.6.1). More
    /// than a TTL counts is as many as it counts. Without the field, or
    /// with a first member that is no number, the answer has no age.
    #[test]
    fn the_age_of_an_answer_is_what_the_first_member_of_its_age_says() {
        for (lines, seconds) in [
            (&["30"][..], 30),
            (&["0"][..], 0),
            (&["007"][..], 7),
            (&["4294967295"][..], u32::MAX),
            (&["4294967296"][..], u32::MAX),
            (&["99999999999999999999999999"][..], u32::MAX),
            // the first of a list, on one line or on several
            (&["30, 40"][..], 30),
            (&["30 , 40"][..], 30),
            (&["30,abc"][..], 30),
            (&["30", "30"][..], 30),
            (&["30", "abc"][..], 30),
            // spaces and tabs around it, empty members before it
            (&[" \t30\t "][..], 30),
            (&[", 30"][..], 30),
            (&[" , ,30"][..], 30),
            (&["", "30"][..], 30),
            // no field, no member, or a first member that is no number
            (&[][..], 0),
            (&[""][..], 0),
            (&[" , "][..], 0),
            (&["abc"][..], 0),
            (&["abc", "30"][..], 0),
            (&["abc, 30"][..], 0),
            (&["-30"][..], 0),
            (&["+30"][..], 0),
            (&["30.5"][..], 0),
            (&["30s"][..], 0),
            (&["3 0"][..], 0),
        ] {
            let mut headers = HeaderMap::new();
            for line in lines {
                headers.append(AGE, line.parse().unwrap());
            }
            assert_eq!(age(&headers), seconds, "{lines:?}");
        }
        // what follows the first member is not read, were it no text
        let mut headers = HeaderMap::new();
        headers.append(AGE, HeaderValue::from_bytes(b"30, \xff").unwrap());
        assert_eq!(age(&headers), 30);
    }
}
