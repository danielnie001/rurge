//! DNS over HTTP/3: DoH's POSTs (RFC 8484) on one QUIC connection per
//! server with ALPN `h3`, shared by its queries (`super::session`), through
//! the HTTP/3 client of `rurge_net`.
//!
//! - A query is a POST to the URL's path — `:authority` the URL's host,
//!   with its port unless 443 —, `content-type` and `accept`
//!   `application/dns-message`, the message with ID 0 as its content, then
//!   the FIN. Its answer is a 200 of that content type, at most 65535 bytes
//!   — as long as its `content-length` says, which HTTP/3 checks —, and
//!   gets the caller's ID back. No EDNS(0) padding, as DoH has none.
//! - A GOAWAY lets the queries under way on its connection run to their
//!   end; the next one dials. A query the server rejected for it
//!   (H3_REQUEST_REJECTED), on a connection that was up when it came, goes
//!   again on the new one.

use super::doh::{DNS_MESSAGE, MAX_RESPONSE};
use super::session::{self, Conn, Dialer, Failure, Protocol, Session, transport_error};
use super::{Upstream, UpstreamError};
use crate::bootstrap::Bootstrap;
use crate::message::wire_id;
use http::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE};
use http::{Request, StatusCode};
use quinn::{Connection, VarInt};
use rurge_net::BoxFuture;
use rurge_net::connector::Connector;
use rurge_net::quic::h3;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;
use url::{Host, Url};

/// HTTP/3's ALPN (RFC 9114 3.1).
pub const ALPN: &str = "h3";

/// H3_NO_ERROR (RFC 9114 8.1): a connection let go of.
const H3_NO_ERROR: VarInt = VarInt::from_u32(0x100);

/// A DNS server over HTTP/3.
pub struct Doh3Upstream {
    /// Where the POSTs go: the `h3://` URL in its `https` form
    /// (`UpstreamSpec::H3`).
    url: Url,
    session: Session<Doh3>,
}

impl Doh3Upstream {
    /// The server of `url` (`UpstreamSpec::H3`'s): its host — a name,
    /// resolved through `bootstrap`, or an address — and port, its
    /// connections' paths opened on `connector` with `config`
    /// (`client_config`).
    pub fn new(
        url: Url,
        connector: Arc<dyn Connector>,
        bootstrap: Arc<Bootstrap>,
        config: quinn::ClientConfig,
    ) -> Doh3Upstream {
        // an IPv6 address without its brackets, as TLS takes it
        let host = match url.host() {
            Some(Host::Domain(name)) => name.to_string(),
            Some(Host::Ipv4(address)) => address.to_string(),
            Some(Host::Ipv6(address)) => address.to_string(),
            None => String::new(),
        };
        let port = url.port_or_known_default().unwrap_or(443);
        let dialer = Dialer::new(
            super::h3_name(&url),
            &host,
            port,
            connector,
            bootstrap,
            config,
        );
        Doh3Upstream {
            url,
            session: Session::new(Doh3, dialer),
        }
    }

    /// The client configuration of DoH3 servers: ALPN `h3`, their
    /// certificates verified unless `skip_verify`.
    pub fn client_config(skip_verify: bool) -> io::Result<quinn::ClientConfig> {
        session::client_config(skip_verify, ALPN)
    }
}

/// HTTP/3 keeps its client for each connection.
struct Doh3;

impl Protocol for Doh3 {
    type State = h3::Client;
    const NO_ERROR: VarInt = H3_NO_ERROR;

    fn start(&self, connection: &Connection) -> h3::Client {
        h3::Client::new(connection.clone())
    }

    fn is_going_away(&self, client: &h3::Client) -> bool {
        client.is_going_away()
    }
}

impl Upstream for Doh3Upstream {
    fn name(&self) -> &str {
        self.session.name()
    }

    fn query<'a>(
        &'a self,
        wire: &'a [u8],
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            let id = wire_id(wire).ok_or_else(|| {
                UpstreamError::BadResponse("query shorter than a DNS header".to_string())
            })?;
            let mut body = wire.to_vec();
            body[..2].fill(0);
            let mut reply = self
                .session
                .query(deadline, |conn| Box::pin(exchange(conn, &self.url, &body)))
                .await?;
            reply[..2].copy_from_slice(&id.to_be_bytes());
            Ok(reply)
        })
    }
}

/// The POST of a query of `len` bytes to `url` (RFC 8484 4.1): the URL's
/// authority goes without the port 443, which its `https` form leaves out.
fn request(url: &Url, len: usize) -> Result<Request<()>, UpstreamError> {
    Request::post(url.as_str())
        .header(CONTENT_TYPE, DNS_MESSAGE)
        .header(ACCEPT, DNS_MESSAGE)
        .header(CONTENT_LENGTH, len)
        .body(())
        .map_err(|e| UpstreamError::Http(e.to_string()))
}

/// One query: the POST of `body` to `url` on a stream of its own, then the
/// answer's content.
async fn exchange(conn: Arc<Conn<h3::Client>>, url: &Url, body: &[u8]) -> Result<Vec<u8>, Failure> {
    let request = request(url, body.len()).map_err(Failure::Failed)?;
    let failed = |e| failure(&conn, e);
    let mut stream = conn.state.request(request).await.map_err(failed)?;
    stream.write_all(body).await.map_err(failed)?;
    // the FIN, and the rest of a DATA frame begun before it
    stream.shutdown().await.map_err(failed)?;
    let response = stream.response().await.map_err(failed)?;
    let status = response.status();
    if status != StatusCode::OK {
        return Err(Failure::Failed(UpstreamError::Http(format!(
            "status {status}"
        ))));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.starts_with(DNS_MESSAGE) {
        return Err(Failure::Failed(UpstreamError::BadResponse(format!(
            "content-type `{content_type}`"
        ))));
    }
    let mut reply = Vec::new();
    (&mut stream)
        .take(MAX_RESPONSE as u64 + 1)
        .read_to_end(&mut reply)
        .await
        .map_err(failed)?;
    if reply.len() > MAX_RESPONSE {
        return Err(Failure::Failed(UpstreamError::BadResponse(
            "response larger than 65535 bytes".to_string(),
        )));
    }
    if reply.len() < 12 {
        return Err(Failure::Failed(UpstreamError::BadResponse(
            "response shorter than a DNS header".to_string(),
        )));
    }
    Ok(reply)
}

/// How a query on `conn` failed: `Lost` when the connection ended or is
/// going away — a request the server rejected after its GOAWAY included
/// —, else final. The error says its layer (`h3: …`, `quic: …`).
fn failure(conn: &Conn<h3::Client>, e: io::Error) -> Failure {
    let error = transport_error(e);
    if conn.quic.close_reason().is_some() || conn.state.is_going_away() {
        Failure::Lost(error)
    } else {
        Failure::Failed(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::FanoutOpts;
    use crate::message::{Answer, Qtype, Question, build_query, parse_response};
    use crate::testing::{Doh3Script, FakeDoh3, MockDns};
    use crate::upstream::UpstreamSpec;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::time::Duration;

    const BOUND: Duration = Duration::from_secs(10);

    /// The `https` form of an `h3://` URL.
    fn h3(url: &str) -> Url {
        match UpstreamSpec::parse(url).unwrap() {
            UpstreamSpec::H3(url) => url,
            other => panic!("{other:?}"),
        }
    }

    /// The upstream of `fake` at `path` (none: the default one).
    fn upstream(fake: &FakeDoh3, path: &str) -> Doh3Upstream {
        Doh3Upstream::new(
            h3(&format!("h3://127.0.0.1:{}{path}", fake.addr().port())),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            Bootstrap::new(Vec::new(), false, FanoutOpts::default()),
            Doh3Upstream::client_config(true).unwrap(),
        )
    }

    fn query(id: u16, name: &str, qtype: Qtype) -> Vec<u8> {
        build_query(
            id,
            &Question {
                name: name.to_string(),
                qtype,
            },
        )
        .unwrap()
    }

    /// `name`'s A records, asked with `id` and a deadline far enough off.
    async fn ask(up: &Doh3Upstream, id: u16, name: &str) -> Result<Answer, UpstreamError> {
        let reply = up
            .query(&query(id, name, Qtype::A), Instant::now() + BOUND)
            .await?;
        Ok(parse_response(&reply).unwrap())
    }

    /// The connection and the stream of each request the fake read.
    fn streams(fake: &FakeDoh3) -> Vec<(usize, u64)> {
        fake.requests()
            .iter()
            .map(|r| (r.connection, r.stream))
            .collect()
    }

    /// A query is a POST of the message with ID 0 — not padded — to the
    /// default path, of `application/dns-message` and taking it, then the
    /// FIN, which the fake waits for; the caller's ID comes back. The
    /// queries share one connection.
    #[tokio::test]
    async fn a_query_is_a_post_of_the_message_with_id_0() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &["fd00::1"], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        let port = fake.addr().port();
        let up = upstream(&fake, "");
        assert_eq!(up.name(), format!("h3://127.0.0.1:{port}/dns-query"));
        let a = ask(&up, 0x1234, "a.test").await.unwrap();
        assert_eq!(a.id, 0x1234);
        assert_eq!(a.v4, [("10.0.0.1".parse().unwrap(), 60)]);
        let wire = query(0x4321, "a.test", Qtype::Aaaa);
        let reply = up.query(&wire, Instant::now() + BOUND).await.unwrap();
        let aaaa = parse_response(&reply).unwrap();
        assert_eq!((aaaa.id, aaaa.v6.len()), (0x4321, 1));
        assert_eq!(streams(&fake), [(0, 0), (0, 4)]);
        let request = &fake.requests()[1];
        let authority = format!("127.0.0.1:{port}");
        let length = wire.len().to_string();
        for (name, value) in [
            (":method", "POST"),
            (":scheme", "https"),
            (":authority", authority.as_str()),
            (":path", "/dns-query"),
            ("content-type", "application/dns-message"),
            ("accept", "application/dns-message"),
            ("content-length", length.as_str()),
        ] {
            assert_eq!(request.field(name), Some(value), "{name}");
        }
        let mut sent = wire;
        sent[..2].fill(0);
        assert_eq!(request.body, sent);
    }

    /// The URL's path goes as it is; the authority leaves the port out
    /// when it is 443.
    #[tokio::test]
    async fn the_request_names_the_urls_path_and_authority() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        ask(&upstream(&fake, "/q?x=1"), 1, "a.test").await.unwrap();
        assert_eq!(fake.requests()[0].field(":path"), Some("/q?x=1"));
        for (url, authority) in [
            ("h3://dns.example", "dns.example"),
            ("h3://dns.example:443/dns-query", "dns.example"),
            ("h3://dns.example:8443", "dns.example:8443"),
            ("h3://[::1]", "[::1]"),
        ] {
            let request = request(&h3(url), 33).unwrap();
            assert_eq!(
                request.uri().authority().map(|a| a.as_str()),
                Some(authority),
                "{url}"
            );
            assert_eq!(request.uri().path(), "/dns-query", "{url}");
        }
    }

    /// Only a 200 of `application/dns-message` is an answer.
    #[tokio::test]
    async fn another_status_or_content_type_is_refused() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        for (script, error) in [
            (
                Doh3Script {
                    status: Some(500),
                    ..Doh3Script::default()
                },
                UpstreamError::Http("status 500 Internal Server Error".to_string()),
            ),
            (
                Doh3Script {
                    content_type: Some("text/html".to_string()),
                    ..Doh3Script::default()
                },
                UpstreamError::BadResponse("content-type `text/html`".to_string()),
            ),
        ] {
            let fake = FakeDoh3::spawn(&dns, script);
            assert_eq!(ask(&upstream(&fake, ""), 1, "a.test").await, Err(error));
        }
    }

    /// An answer whose content its `content-length` does not match is
    /// malformed (RFC 9114 4.1.2): it fails, its connection carries on.
    #[tokio::test]
    async fn an_answer_off_its_content_length_is_refused() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        for (length, how) in [(1, "longer"), (1000, "shorter")] {
            let fake = FakeDoh3::spawn(
                &dns,
                Doh3Script {
                    content_length: Some(length),
                    ..Doh3Script::default()
                },
            );
            let up = upstream(&fake, "");
            let malformed = UpstreamError::Http(format!(
                "h3: the server sent a malformed response ({how} than its content-length)"
            ));
            assert_eq!(ask(&up, 1, "a.test").await, Err(malformed.clone()));
            assert_eq!(ask(&up, 2, "a.test").await, Err(malformed));
            assert_eq!(streams(&fake), [(0, 0), (0, 4)], "{how}");
        }
    }

    /// After a GOAWAY the next query goes on a new connection — directly,
    /// or after the one it went on rejected it.
    #[tokio::test]
    async fn a_goaway_sends_the_next_query_to_a_new_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(
            &dns,
            Doh3Script {
                goaway_after: Some(1),
                ..Doh3Script::default()
            },
        );
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        ask(&up, 2, "a.test").await.unwrap();
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// A server that restarted answers the query's packets with a stateless
    /// reset: the query goes again, once, on a new connection.
    #[tokio::test]
    async fn a_restarted_server_gets_the_query_again_on_a_new_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        fake.restart();
        assert_eq!(ask(&up, 2, "a.test").await.unwrap().id, 2);
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// A server that never answers costs the query its deadline.
    #[tokio::test]
    async fn a_silent_server_is_bounded_by_the_deadline() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        dns.set_drop_all(true);
        let deadline = Instant::now() + Duration::from_millis(300);
        assert_eq!(
            up.query(&query(2, "a.test", Qtype::A), deadline).await,
            Err(UpstreamError::Timeout)
        );
    }
}
