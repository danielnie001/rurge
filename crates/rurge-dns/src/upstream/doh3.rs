//! DNS over HTTP/3: DoH's POSTs (RFC 8484) on one QUIC connection per
//! server with ALPN `h3`, shared by its queries (`super::session`), through
//! the HTTP/3 client of `rurge_net`.
//!
//! - A query is a POST to the URL's path — `:authority` the URL's host,
//!   with its port unless 443 —, `content-type` and `accept`
//!   `application/dns-message`, the message with ID 0 as its content, then
//!   the FIN. Its answer is a 200 of that content type, at most 65535 bytes
//!   — as long as its `content-length` says, which HTTP/3 checks —, and
//!   gets the caller's ID back, its records' TTLs lowered by the age it
//!   comes with (RFC 8484 5.1). No EDNS(0) padding, as DoH has none.
//! - A GOAWAY lets the queries under way on its connection run to their
//!   end; the next one dials. A request the server rejected
//!   (H3_REQUEST_REJECTED: nothing of it was processed, RFC 9114 4.1.1),
//!   after a GOAWAY or without, on a connection that was up when its query
//!   came, goes again on a new one.

use super::doh::{DNS_MESSAGE, MAX_RESPONSE, age, check_body, check_head};
use super::session::{self, Conn, Dialer, Failure, Progress, Protocol, Session, transport_error};
use super::{Upstream, UpstreamError};
use crate::bootstrap::Bootstrap;
use crate::message::{lower_ttls, wire_id};
use http::Request;
use http::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE};
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
                .query(deadline, |conn, progress| {
                    Box::pin(exchange(conn, progress, &self.url, &body))
                })
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
async fn exchange(
    conn: Arc<Conn<h3::Client>>,
    progress: Progress,
    url: &Url,
    body: &[u8],
) -> Result<Vec<u8>, Failure> {
    let request = request(url, body.len()).map_err(Failure::Failed)?;
    let failed = |e| failure(&conn, e);
    let mut stream = conn.state.request(request).await.map_err(failed)?;
    stream.write_all(body).await.map_err(failed)?;
    // the FIN, and the rest of a DATA frame begun before it
    stream.shutdown().await.map_err(failed)?;
    progress.written(&conn.quic);
    let response = stream.response().await.map_err(failed)?;
    check_head(response.status(), response.headers()).map_err(Failure::Failed)?;
    let mut reply = Vec::new();
    (&mut stream)
        .take(MAX_RESPONSE as u64 + 1)
        .read_to_end(&mut reply)
        .await
        .map_err(failed)?;
    check_body(&reply).map_err(Failure::Failed)?;
    lower_ttls(&mut reply, age(response.headers()));
    Ok(reply)
}

/// How a query on `conn` failed: `Lost` when the connection ended or is
/// going away, or the server rejected the request — nothing of it was
/// processed (RFC 9114 4.1.1), after a GOAWAY or without —, else final. The
/// error says its layer (`h3: …`, `quic: …`).
fn failure(conn: &Conn<h3::Client>, e: io::Error) -> Failure {
    let lost = conn.quic.close_reason().is_some() || conn.state.is_going_away() || h3::rejected(&e);
    let error = transport_error(e);
    if lost {
        Failure::Lost(error)
    } else {
        Failure::Failed(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::FanoutOpts;
    use crate::message::{
        Answer, Qtype, Question, Rcode, build_query, build_response, parse_response,
    };
    use crate::testing::{Doh3Script, FakeDoh3, MockDns};
    use crate::upstream::UpstreamSpec;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::Ipv4Addr;
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

    /// A server that never answers costs the query its deadline. Its
    /// connection, which acknowledged the query, carries the next one.
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
        dns.set_drop_all(false);
        ask(&up, 3, "a.test").await.unwrap();
        assert_eq!(fake.connections(), 1);
        assert_eq!(streams(&fake), [(0, 0), (0, 4), (0, 8)]);
    }

    /// A server that rejects a request — nothing of it was processed (RFC
    /// 9114 4.1.1) — without a GOAWAY: the query goes again, once, on a new
    /// connection, and gets its answer.
    #[tokio::test]
    async fn a_rejected_query_goes_again_on_a_new_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(
            &dns,
            Doh3Script {
                reject_after: Some(1),
                ..Doh3Script::default()
            },
        );
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        assert_eq!(ask(&up, 2, "a.test").await.unwrap().id, 2);
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// On a connection dialed for it, a rejected query fails with what the
    /// server said: a server that rejects every request is not asked
    /// twice for one.
    #[tokio::test]
    async fn a_query_rejected_on_the_connection_it_dialed_fails() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(
            &dns,
            Doh3Script {
                reject_after: Some(0),
                ..Doh3Script::default()
            },
        );
        let up = upstream(&fake, "");
        let rejected = UpstreamError::Http(
            "h3: the server reset the stream (H3_REQUEST_REJECTED)".to_string(),
        );
        for id in [1, 2] {
            assert_eq!(ask(&up, id, "a.test").await, Err(rejected.clone()));
        }
        // the connection that rejected one takes no other
        assert_eq!(fake.connections(), 2);
    }

    /// An answer is a DNS message: a header at least, and at most 65535
    /// bytes. The size is the content's, whatever its `content-length`
    /// says: it says it right.
    #[tokio::test]
    async fn an_answer_too_short_or_too_long_is_refused() {
        let dns = MockDns::spawn().await;
        for (bytes, error) in [
            (vec![0; 11], "response shorter than a DNS header"),
            (vec![0; 65_536], "response larger than 65535 bytes"),
        ] {
            let fake = FakeDoh3::spawn(
                &dns,
                Doh3Script {
                    reply_bytes: Some(bytes),
                    ..Doh3Script::default()
                },
            );
            let up = upstream(&fake, "");
            for id in [1, 2] {
                assert_eq!(
                    ask(&up, id, "a.test").await,
                    Err(UpstreamError::BadResponse(error.to_string()))
                );
            }
            assert_eq!(fake.connections(), 1, "{error}");
        }
        // and the largest there is is an answer
        let largest = [vec![0; 12], vec![0; 65_523]].concat();
        let fake = FakeDoh3::spawn(
            &dns,
            Doh3Script {
                reply_bytes: Some(largest.clone()),
                ..Doh3Script::default()
            },
        );
        let reply = upstream(&fake, "")
            .query(&query(5, "a.test", Qtype::A), Instant::now() + BOUND)
            .await
            .unwrap();
        assert_eq!(reply.len(), largest.len());
        assert_eq!(&reply[..2], [0, 5]);
    }

    /// The content type may carry parameters, and is read as its media
    /// type is: whatever its case.
    #[tokio::test]
    async fn a_content_type_with_parameters_is_accepted() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        for content_type in [
            "application/dns-message; charset=utf-8",
            "Application/DNS-Message",
        ] {
            let fake = FakeDoh3::spawn(
                &dns,
                Doh3Script {
                    content_type: Some(content_type.to_string()),
                    ..Doh3Script::default()
                },
            );
            let up = upstream(&fake, "");
            assert_eq!(ask(&up, 1, "a.test").await.unwrap().id, 1, "{content_type}");
        }
    }

    /// The age an answer comes with is off the TTL of each of its records,
    /// no further than 0, as over HTTP/2 (RFC 8484 5.1); an answer without
    /// an age comes as it is.
    #[tokio::test]
    async fn the_age_of_an_answer_comes_off_its_ttls() {
        let dns = MockDns::spawn().await;
        let question = Question {
            name: "a.test".to_string(),
            qtype: Qtype::A,
        };
        let addresses = [Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 2)];
        let records = [(addresses[0].into(), 100), (addresses[1].into(), 20)];
        let reply = build_response(0, &question, Rcode::NoError, &records, false).unwrap();
        for (age, ttls) in [(Some(30), [70, 0]), (Some(0), [100, 20]), (None, [100, 20])] {
            let fake = FakeDoh3::spawn(
                &dns,
                Doh3Script {
                    reply_bytes: Some(reply.clone()),
                    age,
                    ..Doh3Script::default()
                },
            );
            let answer = ask(&upstream(&fake, ""), 1, "a.test").await.unwrap();
            assert_eq!(
                answer.v4,
                [(addresses[0], ttls[0]), (addresses[1], ttls[1])],
                "{age:?}"
            );
        }
    }

    /// A server that restarted and lost the key of its stateless resets
    /// leaves the connection it had silent: the query that finds it so
    /// times out, and the next one dials a new connection, instead of
    /// waiting out the old one's idle timeout.
    #[tokio::test]
    async fn a_connection_gone_silent_is_replaced_by_the_next_query() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        // what the server had to say after the answer is out: nothing of it
        // is on its way to the client when the server falls silent
        tokio::time::timeout(BOUND, fake.idle())
            .await
            .expect("idle within the bound");
        fake.restart_without_resets();
        let deadline = Instant::now() + Duration::from_millis(500);
        assert_eq!(
            up.query(&query(2, "a.test", Qtype::A), deadline).await,
            Err(UpstreamError::Timeout)
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let reply = up.query(&query(3, "a.test", Qtype::A), deadline).await;
        assert_eq!(parse_response(&reply.unwrap()).unwrap().id, 3);
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// A query given up on while the server answers others — the fan-out
    /// drops the upstreams that lost — says nothing of its connection: the
    /// next query goes on it.
    #[tokio::test]
    async fn a_query_given_up_on_while_the_server_answers_others_keeps_the_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &["fd00::1"], 60);
        let fake = FakeDoh3::spawn(&dns, Doh3Script::default());
        let up = upstream(&fake, "");
        ask(&up, 1, "a.test").await.unwrap();
        // its AAAA goes unanswered, the A of the next is answered meanwhile
        dns.set_drop_qtype(Qtype::Aaaa, true);
        let wire = query(2, "a.test", Qtype::Aaaa);
        let mut slow = Box::pin(up.query(&wire, Instant::now() + BOUND));
        tokio::select! {
            slow = &mut slow => panic!("answered: {slow:?}"),
            answer = ask(&up, 3, "a.test") => assert_eq!(answer.unwrap().id, 3),
        }
        // long enough for a connection that took nothing to be told apart
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(slow);
        ask(&up, 4, "a.test").await.unwrap();
        assert_eq!(fake.connections(), 1);
    }
}
