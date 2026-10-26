//! DNS over dedicated QUIC (RFC 9250): one QUIC connection per server with
//! ALPN `doq`, shared by its queries (`super::session`), and each query on
//! a bidirectional stream of its own.
//!
//! - The query goes out as a 2-byte length and the message — message ID 0,
//!   EDNS(0) padding to a multiple of 128 bytes (4.2, 4.2.1, 5.4: a MUST of
//!   the RFC, which none of the clients rurge looked at follows) —, then
//!   the FIN. The reply is what the stream carries to its end: one length
//!   and one message, whose ID must be 0; the caller's ID goes back on it.
//! - A reply that breaks those rules is the server's protocol error: the
//!   query fails, and the connection is closed with DOQ_PROTOCOL_ERROR
//!   (4.3.3). A stream the server resets fails its query alone (4.3.2).
//! - A query given up on — its future dropped: the fan-out's deadline —
//!   stops its stream with DOQ_REQUEST_CANCELLED, and resets the sending
//!   side unless the FIN went out (4.3.1).

use super::session::{self, Conn, Dialer, Failure, Progress, Protocol, Session};
use super::tcp::MAX_MESSAGE;
use super::{Upstream, UpstreamError};
use crate::bootstrap::Bootstrap;
use crate::message::{doq_query, wire_id};
use quinn::{Connection, ReadError, ReadToEndError, RecvStream, SendStream, VarInt, WriteError};
use rurge_net::BoxFuture;
use rurge_net::connector::Connector;
use std::borrow::Cow;
use std::io;
use std::sync::Arc;
use tokio::time::Instant;

/// DoQ's ALPN (RFC 9250 4.1), alone: the drafts' (`doq-i0x`) frame their
/// messages otherwise.
pub const ALPN: &str = "doq";

// error codes (RFC 9250 4.3)
const DOQ_NO_ERROR: VarInt = VarInt::from_u32(0x0);
const DOQ_PROTOCOL_ERROR: VarInt = VarInt::from_u32(0x2);
const DOQ_REQUEST_CANCELLED: VarInt = VarInt::from_u32(0x3);

// the server's protocol errors with the reply's framing (RFC 9250 4.3.3)
const CUT_SHORT: &str = "the stream ended within the server's reply";
const MORE_THAN_A_REPLY: &str = "the server sent more than its reply on the stream";

/// A DNS server over QUIC.
pub struct DoqUpstream {
    session: Session<Doq>,
}

impl DoqUpstream {
    /// The server at `host` — a name, resolved through `bootstrap`, or an
    /// address — and `port`, its connections' paths opened on `connector`
    /// with `config` (`client_config`).
    pub fn new(
        host: &str,
        port: u16,
        connector: Arc<dyn Connector>,
        bootstrap: Arc<Bootstrap>,
        config: quinn::ClientConfig,
    ) -> DoqUpstream {
        let name = format!("quic://{}", super::host_port_text(host, port));
        DoqUpstream {
            session: Session::new(
                Doq,
                Dialer::new(name, host, port, connector, bootstrap, config),
            ),
        }
    }

    /// The client configuration of DoQ servers: ALPN `doq`, their
    /// certificates verified unless `skip_verify`.
    pub fn client_config(skip_verify: bool) -> io::Result<quinn::ClientConfig> {
        session::client_config(skip_verify, ALPN)
    }
}

/// DoQ keeps nothing for a connection.
struct Doq;

impl Protocol for Doq {
    type State = ();
    const NO_ERROR: VarInt = DOQ_NO_ERROR;

    fn start(&self, _connection: &Connection) {}
}

impl Upstream for DoqUpstream {
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
            let message =
                doq_query(wire).map_err(|e| UpstreamError::BadResponse(format!("query: {e}")))?;
            let len = u16::try_from(message.len()).map_err(|_| {
                UpstreamError::BadResponse("query larger than 65535 bytes".to_string())
            })?;
            let mut framed = Vec::with_capacity(2 + message.len());
            framed.extend_from_slice(&len.to_be_bytes());
            framed.extend_from_slice(&message);
            let mut reply = self
                .session
                .query(deadline, |conn, progress| {
                    Box::pin(exchange(conn, progress, &framed))
                })
                .await?;
            reply[..2].copy_from_slice(&id.to_be_bytes());
            Ok(reply)
        })
    }
}

/// One query on a stream of its own: `framed` out and the FIN, then the
/// reply — its message.
async fn exchange(
    conn: Arc<Conn<()>>,
    progress: Progress,
    framed: &[u8],
) -> Result<Vec<u8>, Failure> {
    let (send, recv) = conn.quic.open_bi().await.map_err(|e| Failure::lost(&e))?;
    let mut stream = QueryStream {
        send,
        recv,
        finished: false,
        read: false,
    };
    stream.send.write_all(framed).await.map_err(write_failure)?;
    progress.written(&conn.quic);
    // fails only for a stream finished or reset before, which this is not
    let _ = stream.send.finish();
    stream.finished = true;
    let bytes = stream.recv.read_to_end(2 + MAX_MESSAGE).await;
    stream.read = true;
    match bytes {
        Ok(bytes) => reply(&conn, &bytes).map(<[u8]>::to_vec),
        Err(ReadToEndError::TooLong) => Err(violation(&conn, MORE_THAN_A_REPLY)),
        Err(ReadToEndError::Read(e)) => Err(read_failure(e)),
    }
}

/// The message of `bytes`, a stream read to its end: one length and one
/// message, whose ID is 0 (RFC 9250 4.2, 4.2.1, 4.3.3).
fn reply<'a>(conn: &Conn<()>, bytes: &'a [u8]) -> Result<&'a [u8], Failure> {
    let Some((len, message)) = bytes.split_first_chunk::<2>() else {
        return Err(violation(conn, CUT_SHORT));
    };
    let len = usize::from(u16::from_be_bytes(*len));
    if message.len() < len {
        return Err(violation(conn, CUT_SHORT));
    }
    if message.len() > len {
        return Err(violation(conn, MORE_THAN_A_REPLY));
    }
    let Some(id) = wire_id(message) else {
        return Err(Failure::Failed(UpstreamError::BadResponse(
            "response shorter than a DNS header".to_string(),
        )));
    };
    if id != 0 {
        return Err(violation(
            conn,
            &format!("the server's reply has message ID {id}, not 0"),
        ));
    }
    Ok(message)
}

/// The server broke DoQ: the query fails, and the connection is closed with
/// DOQ_PROTOCOL_ERROR (RFC 9250 4.3.3) — its other queries go again on a
/// new one.
fn violation(conn: &Conn<()>, what: &str) -> Failure {
    conn.close(DOQ_PROTOCOL_ERROR, what);
    Failure::Failed(UpstreamError::BadResponse(what.to_string()))
}

fn write_failure(e: WriteError) -> Failure {
    match e {
        WriteError::ConnectionLost(e) => Failure::lost(&e),
        WriteError::Stopped(code) => Failure::Failed(UpstreamError::Io(format!(
            "the server stopped the query's stream ({})",
            code_name(code)
        ))),
        e => Failure::Failed(UpstreamError::Io(e.to_string())),
    }
}

fn read_failure(e: ReadError) -> Failure {
    match e {
        ReadError::ConnectionLost(e) => Failure::lost(&e),
        ReadError::Reset(code) => Failure::Failed(UpstreamError::Io(format!(
            "the server reset the query's stream ({})",
            code_name(code)
        ))),
        e => Failure::Failed(UpstreamError::Io(e.to_string())),
    }
}

/// The name of a DoQ error code (RFC 9250 4.3), as error texts say it; an
/// unknown one in hex, which means DOQ_UNSPECIFIED_ERROR (4.3.4).
fn code_name(code: VarInt) -> Cow<'static, str> {
    Cow::Borrowed(match code.into_inner() {
        0x0 => "DOQ_NO_ERROR",
        0x1 => "DOQ_INTERNAL_ERROR",
        0x2 => "DOQ_PROTOCOL_ERROR",
        0x3 => "DOQ_REQUEST_CANCELLED",
        0x4 => "DOQ_EXCESSIVE_LOAD",
        0x5 => "DOQ_UNSPECIFIED_ERROR",
        other => return Cow::Owned(format!("{other:#x}")),
    })
}

/// A query's stream. Dropped before its reply was read — the query given
/// up on —, it is stopped with DOQ_REQUEST_CANCELLED, and its sending side
/// reset with it unless the FIN went out (RFC 9250 4.3.1).
struct QueryStream {
    send: SendStream,
    recv: RecvStream,
    /// The FIN went out.
    finished: bool,
    /// The reply was read, or reading it failed.
    read: bool,
}

impl Drop for QueryStream {
    fn drop(&mut self) {
        if self.read {
            return;
        }
        if !self.finished {
            let _ = self.send.reset(DOQ_REQUEST_CANCELLED);
        }
        let _ = self.recv.stop(DOQ_REQUEST_CANCELLED);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fanout::FanoutOpts;
    use crate::message::{Answer, Qtype, Question, build_query, parse_query, parse_response};
    use crate::testing::{DoqScript, FakeDoq, MockDns, RecordedReset, RecordedStop};
    use crate::upstream::udp::UdpUpstream;
    use quinn::ConnectionError;
    use rurge_config::HostName;
    use rurge_net::connector::{
        BoxedDatagram, BoxedStream, ConnectOpts, DirectConnector, SystemResolve, Target,
    };
    use std::net::IpAddr;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::time::timeout;

    const BOUND: Duration = Duration::from_secs(10);

    fn direct() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    /// A bootstrap without a server: an address needs none.
    fn no_bootstrap() -> Arc<Bootstrap> {
        Bootstrap::new(Vec::new(), false, FanoutOpts::default())
    }

    /// The upstream of `fake`, by its address.
    fn upstream(fake: &FakeDoq, skip_verify: bool) -> DoqUpstream {
        DoqUpstream::new(
            "127.0.0.1",
            fake.addr().port(),
            direct(),
            no_bootstrap(),
            DoqUpstream::client_config(skip_verify).unwrap(),
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
    async fn ask(up: &DoqUpstream, id: u16, name: &str) -> Result<Answer, UpstreamError> {
        let reply = up
            .query(&query(id, name, Qtype::A), Instant::now() + BOUND)
            .await?;
        Ok(parse_response(&reply).unwrap())
    }

    /// The connection and the stream of each query the fake read.
    fn streams(fake: &FakeDoq) -> Vec<(usize, u64)> {
        fake.queries()
            .iter()
            .map(|q| (q.connection, q.stream))
            .collect()
    }

    /// Waits until `check` holds, looking again and again until the bound.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !check() {
            assert!(Instant::now() < deadline, "not within the bound");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Each query on a stream of its own of the one connection: a length,
    /// then the message — ID 0, padded to a multiple of 128 bytes —, then
    /// the FIN, which the fake waits for; the caller's ID comes back.
    #[tokio::test]
    async fn a_query_is_its_padded_message_with_id_0_on_a_stream_of_its_own() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &["fd00::1"], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        assert_eq!(
            up.name(),
            format!("quic://127.0.0.1:{}", fake.addr().port())
        );
        let a = ask(&up, 0x1234, "a.test").await.unwrap();
        assert_eq!(a.id, 0x1234);
        assert_eq!(a.v4, [("10.0.0.1".parse().unwrap(), 60)]);
        let wire = query(0x4321, "a.test", Qtype::Aaaa);
        let reply = up.query(&wire, Instant::now() + BOUND).await.unwrap();
        let aaaa = parse_response(&reply).unwrap();
        assert_eq!(aaaa.id, 0x4321);
        assert_eq!(aaaa.v6, [("fd00::1".parse().unwrap(), 60)]);
        assert_eq!(streams(&fake), [(0, 0), (0, 4)]);
        for (q, qtype) in fake.queries().iter().zip([Qtype::A, Qtype::Aaaa]) {
            let len = usize::from(u16::from_be_bytes([q.bytes[0], q.bytes[1]]));
            assert_eq!(q.bytes.len(), 2 + len, "one message, then the FIN");
            assert_eq!(len % 128, 0, "padded");
            let (id, question) = parse_query(q.message()).unwrap();
            assert_eq!(
                (id, question.name.as_str(), question.qtype),
                (0, "a.test", qtype)
            );
        }
        assert_eq!(fake.connections(), 1);
    }

    /// Queries that come together share one dial.
    #[tokio::test]
    async fn queries_that_come_together_share_one_dial() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        dns.set("b.test", &["10.0.0.2"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        let (a, b) = tokio::join!(ask(&up, 1, "a.test"), ask(&up, 2, "b.test"));
        assert_eq!(a.unwrap().v4[0].0.to_string(), "10.0.0.1");
        assert_eq!(b.unwrap().v4[0].0.to_string(), "10.0.0.2");
        assert_eq!(fake.connections(), 1);
        let mut used = streams(&fake);
        used.sort();
        assert_eq!(used, [(0, 0), (0, 4)]);
    }

    /// A reply with another ID than 0 breaks DoQ: the query fails and the
    /// connection is closed with DOQ_PROTOCOL_ERROR; the next query dials
    /// again.
    #[tokio::test]
    async fn a_reply_with_a_message_id_closes_the_connection() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(
            &dns,
            DoqScript {
                reply_id: Some(7),
                ..DoqScript::default()
            },
        );
        let up = upstream(&fake, true);
        let reason = "the server's reply has message ID 7, not 0";
        assert_eq!(
            ask(&up, 1, "a.test").await.unwrap_err(),
            UpstreamError::BadResponse(reason.to_string())
        );
        match timeout(BOUND, fake.connection(0).closed()).await.unwrap() {
            ConnectionError::ApplicationClosed(close) => {
                assert_eq!(close.error_code, DOQ_PROTOCOL_ERROR);
                assert_eq!(&close.reason[..], reason.as_bytes());
            }
            other => panic!("closed otherwise: {other:?}"),
        }
        assert!(ask(&up, 2, "a.test").await.is_err());
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// A stream that does not carry one length and one message breaks DoQ
    /// as well (RFC 9250 4.3.3); a message too short for a DNS header is a
    /// bad response alone.
    #[tokio::test]
    async fn a_reply_framed_otherwise_closes_the_connection() {
        let dns = MockDns::spawn().await;
        let message = [0u8; 12];
        for (bytes, error, breaks) in [
            (Vec::new(), CUT_SHORT, true),
            (vec![0], CUT_SHORT, true),
            ([&[0, 13][..], &message].concat(), CUT_SHORT, true),
            ([&[0, 11][..], &message].concat(), MORE_THAN_A_REPLY, true),
            (
                vec![0, 2, 0, 0],
                "response shorter than a DNS header",
                false,
            ),
        ] {
            let fake = FakeDoq::spawn(
                &dns,
                DoqScript {
                    reply_bytes: Some(bytes.clone()),
                    ..DoqScript::default()
                },
            );
            let up = upstream(&fake, true);
            assert_eq!(
                ask(&up, 1, "a.test").await.unwrap_err(),
                UpstreamError::BadResponse(error.to_string()),
                "{bytes:?}"
            );
            if breaks {
                match timeout(BOUND, fake.connection(0).closed()).await.unwrap() {
                    ConnectionError::ApplicationClosed(close) => {
                        assert_eq!(close.error_code, DOQ_PROTOCOL_ERROR, "{bytes:?}");
                    }
                    other => panic!("closed otherwise: {other:?}"),
                }
            } else {
                assert!(fake.connection(0).close_reason().is_none());
            }
        }
    }

    /// A stream the server resets fails its query alone: the connection
    /// carries the next one.
    #[tokio::test]
    async fn a_stream_reset_by_the_server_fails_its_query_alone() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(
            &dns,
            DoqScript {
                reset: true,
                ..DoqScript::default()
            },
        );
        let up = upstream(&fake, true);
        for id in [1, 2] {
            assert_eq!(
                ask(&up, id, "a.test").await.unwrap_err(),
                UpstreamError::Io(
                    "the server reset the query's stream (DOQ_INTERNAL_ERROR)".to_string()
                )
            );
        }
        assert_eq!(streams(&fake), [(0, 0), (0, 4)]);
        assert!(fake.connection(0).close_reason().is_none());
    }

    /// A query given up on — its deadline passed — stops its stream with
    /// DOQ_REQUEST_CANCELLED; the FIN went out before. Its connection, which
    /// answered before and acknowledged the query, carries the next query.
    #[tokio::test]
    async fn a_query_given_up_on_stops_its_stream() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        ask(&up, 1, "a.test").await.unwrap();
        dns.set_drop_all(true);
        let deadline = Instant::now() + Duration::from_millis(300);
        assert_eq!(
            up.query(&query(2, "a.test", Qtype::A), deadline).await,
            Err(UpstreamError::Timeout)
        );
        eventually(|| {
            fake.stops()
                == [RecordedStop {
                    connection: 0,
                    stream: 4,
                    code: 0x3,
                }]
        })
        .await;
        dns.set_drop_all(false);
        ask(&up, 3, "a.test").await.unwrap();
        assert_eq!(fake.connections(), 1);
        assert_eq!(streams(&fake), [(0, 0), (0, 4), (0, 8)]);
    }

    /// A query given up on before its FIN went out — a window too small
    /// for it keeps the client writing — resets the sending side of its
    /// stream with DOQ_REQUEST_CANCELLED as well (RFC 9250 4.3.1).
    #[tokio::test]
    async fn a_query_given_up_on_before_its_fin_resets_its_stream() {
        let dns = MockDns::spawn().await;
        let fake = FakeDoq::spawn(
            &dns,
            DoqScript {
                stall: Some(16),
                ..DoqScript::default()
            },
        );
        let up = upstream(&fake, true);
        let deadline = Instant::now() + Duration::from_millis(300);
        assert_eq!(
            up.query(&query(1, "a.test", Qtype::A), deadline).await,
            Err(UpstreamError::Timeout)
        );
        eventually(|| {
            fake.resets()
                == [RecordedReset {
                    connection: 0,
                    stream: 0,
                    code: 0x3,
                }]
        })
        .await;
    }

    /// A server that restarted and lost the key of its stateless resets
    /// leaves the connection it had silent: the query that finds it so
    /// times out, and the next one dials a new connection, instead of
    /// waiting out the old one's idle timeout.
    #[tokio::test]
    async fn a_connection_gone_silent_is_replaced_by_the_next_query() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        ask(&up, 1, "a.test").await.unwrap();
        // what the server had to say after the answer is out: nothing of it
        // is on its way to the client when the server falls silent
        timeout(BOUND, fake.idle())
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
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
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

    /// A server that never answers the handshake costs the query its
    /// deadline, not QUIC's idle timeout.
    #[tokio::test]
    async fn a_server_that_never_answers_is_bounded_by_the_deadline() {
        let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let up = DoqUpstream::new(
            "127.0.0.1",
            silent.local_addr().unwrap().port(),
            direct(),
            no_bootstrap(),
            DoqUpstream::client_config(true).unwrap(),
        );
        let started = Instant::now();
        let deadline = started + Duration::from_millis(300);
        assert_eq!(
            up.query(&query(1, "a.test", Qtype::A), deadline).await,
            Err(UpstreamError::Timeout)
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// A connection the server let go of once it idled is replaced: the
    /// next query dials anew.
    #[tokio::test]
    async fn an_idle_connection_is_replaced() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(
            &dns,
            DoqScript {
                idle_timeout: Some(Duration::from_millis(300)),
                ..DoqScript::default()
            },
        );
        let up = upstream(&fake, true);
        ask(&up, 1, "a.test").await.unwrap();
        timeout(BOUND, fake.connection(0).closed())
            .await
            .expect("idled out within the bound");
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
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let up = upstream(&fake, true);
        ask(&up, 1, "a.test").await.unwrap();
        fake.restart();
        assert_eq!(ask(&up, 2, "a.test").await.unwrap().id, 2);
        assert_eq!(fake.connections(), 2);
        assert_eq!(streams(&fake), [(0, 0), (1, 0)]);
    }

    /// The server's certificate is verified unless the profile says not to.
    #[tokio::test]
    async fn the_certificate_is_verified_unless_skipped() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let err = ask(&upstream(&fake, false), 1, "a.test").await.unwrap_err();
        assert!(
            matches!(&err, UpstreamError::Tls(text) if text.starts_with("invalid peer certificate")),
            "{err}"
        );
        assert_eq!(fake.connections(), 0);
        ask(&upstream(&fake, true), 2, "a.test").await.unwrap();
    }

    /// Carries UDP as a direct connector does, but refuses the flows to
    /// `refused`; a QUIC path goes over its flows.
    struct Refusing {
        inner: DirectConnector,
        refused: IpAddr,
        asked: Mutex<Vec<Target>>,
    }

    impl Connector for Refusing {
        fn connect<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedStream>> {
            self.inner.connect(target, opts)
        }

        fn connect_udp<'a>(
            &'a self,
            target: &'a Target,
            opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, io::Result<BoxedDatagram>> {
            self.asked.lock().unwrap().push(target.clone());
            if target.host == HostName::Ip(self.refused) {
                return Box::pin(std::future::ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "refused",
                ))));
            }
            self.inner.connect_udp(target, opts)
        }
    }

    /// A server named by host goes through the bootstrap — the traditional
    /// server is asked for it, once —, and its addresses are tried in turn
    /// until one connects.
    #[tokio::test]
    async fn a_name_goes_through_the_bootstrap_and_its_addresses_in_turn() {
        let traditional = MockDns::spawn().await;
        traditional.set("doq.test", &["192.0.2.1", "127.0.0.1"], &[], 60);
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        let fake = FakeDoq::spawn(&dns, DoqScript::default());
        let port = fake.addr().port();
        let connector = Arc::new(Refusing {
            inner: DirectConnector::new(Arc::new(SystemResolve)),
            refused: "192.0.2.1".parse().unwrap(),
            asked: Mutex::new(Vec::new()),
        });
        let bootstrap = Bootstrap::new(
            vec![Arc::new(UdpUpstream::new(traditional.addr(), direct()))],
            false,
            FanoutOpts::default(),
        );
        let up = DoqUpstream::new(
            "doq.test",
            port,
            connector.clone(),
            bootstrap,
            DoqUpstream::client_config(true).unwrap(),
        );
        assert_eq!(up.name(), format!("quic://doq.test:{port}"));
        ask(&up, 1, "a.test").await.unwrap();
        ask(&up, 2, "a.test").await.unwrap();
        let at = |ip: &str| Target::new(HostName::parse(ip), port);
        assert_eq!(
            *connector.asked.lock().unwrap(),
            [at("192.0.2.1"), at("127.0.0.1")]
        );
        assert_eq!(traditional.query_count("doq.test", Qtype::A), 1);
        assert_eq!(dns.query_count("a.test", Qtype::A), 2);
    }

    /// A server written as an IP address is verified by an IP name of its
    /// certificate, not by a DNS name that spells it.
    #[tokio::test]
    async fn the_certificate_of_an_ip_literal_needs_an_ip_name() {
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        for (names, accepted) in [(None, true), (Some(vec!["localhost".to_string()]), false)] {
            let fake = FakeDoq::spawn(
                &dns,
                DoqScript {
                    cert_names: names,
                    ..DoqScript::default()
                },
            );
            let up = DoqUpstream::new(
                "127.0.0.1",
                fake.addr().port(),
                direct(),
                no_bootstrap(),
                fake.client_config(),
            );
            let answer = ask(&up, 1, "a.test").await;
            if accepted {
                assert_eq!(answer.unwrap().id, 1);
            } else {
                assert!(
                    matches!(&answer, Err(UpstreamError::Tls(text)) if text.starts_with("invalid peer certificate")),
                    "{answer:?}"
                );
            }
        }
    }

    /// A server's addresses get the time left by turns, each its share: one
    /// that never answers — QUIC has no refusal to fail it quickly — does
    /// not take it all from the one after it. IPv6 where this machine has a
    /// loopback of it.
    #[tokio::test]
    async fn an_address_that_never_answers_costs_its_share_of_the_time() {
        if std::net::UdpSocket::bind("[::1]:0").is_err() {
            return;
        }
        let dns = MockDns::spawn().await;
        dns.set("a.test", &["10.0.0.1"], &[], 60);
        // the server on `::1`, and on the same port of `127.0.0.1` a socket
        // that nobody reads
        let (fake, _silent) = (0..16)
            .find_map(|_| {
                let fake =
                    FakeDoq::spawn_at(&dns, DoqScript::default(), "[::1]:0".parse().unwrap());
                let silent = std::net::UdpSocket::bind(("127.0.0.1", fake.addr().port())).ok()?;
                Some((fake, silent))
            })
            .expect("a port free on both loopbacks");
        let traditional = MockDns::spawn().await;
        traditional.set("doq.test", &["127.0.0.1"], &["::1"], 60);
        let bootstrap = Bootstrap::new(
            vec![Arc::new(UdpUpstream::new(traditional.addr(), direct()))],
            true,
            FanoutOpts::default(),
        );
        let up = DoqUpstream::new(
            "doq.test",
            fake.addr().port(),
            direct(),
            bootstrap,
            DoqUpstream::client_config(true).unwrap(),
        );
        let started = Instant::now();
        let deadline = started + Duration::from_secs(3);
        let reply = up.query(&query(1, "a.test", Qtype::A), deadline).await;
        assert_eq!(parse_response(&reply.unwrap()).unwrap().id, 1);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(fake.connections(), 1);
    }

    #[test]
    fn doq_error_codes_have_their_names() {
        assert_eq!(code_name(VarInt::from_u32(0x1)), "DOQ_INTERNAL_ERROR");
        assert_eq!(code_name(VarInt::from_u32(0x5)), "DOQ_UNSPECIFIED_ERROR");
        assert_eq!(code_name(VarInt::from_u32(0x42)), "0x42");
    }
}
