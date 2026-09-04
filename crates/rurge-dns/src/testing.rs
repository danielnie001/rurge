//! In-process mock DNS servers for tests: one UDP socket and one TCP (or
//! TLS) listener on the same port, with per-name answers and programmable
//! misbehaviour (delay, drops, truncation, error codes).

use crate::message::{Qtype, Question, Rcode, build_response, parse_query};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

#[derive(Clone, Debug)]
enum Behaviour {
    Records {
        v4: Vec<IpAddr>,
        v6: Vec<IpAddr>,
        ttl: u32,
    },
    Empty,
    Rcode(Rcode),
}

#[derive(Default)]
struct State {
    names: HashMap<String, Behaviour>,
    delay: Duration,
    drop_all: bool,
    drop_first: usize,
    drop_qtype: HashSet<Qtype>,
    truncate_udp: bool,
    queries: Vec<(Question, String)>,
}

pub struct MockDns {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    udp_task: JoinHandle<()>,
    tcp_task: JoinHandle<()>,
}

impl Drop for MockDns {
    /// Stop both listening loops immediately so a dropped mock answers
    /// nothing: without this the UDP task (which has no shutdown signal to
    /// race) would keep serving after the test that owned it ended.
    fn drop(&mut self) {
        self.udp_task.abort();
        self.tcp_task.abort();
    }
}

fn norm(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Decide the reply for one query: `None` = drop.
fn respond(state: &Arc<Mutex<State>>, wire: &[u8], transport: &str) -> Option<(Duration, Vec<u8>)> {
    let (id, q) = parse_query(wire).ok()?;
    let mut st = state.lock().expect("mock state");
    st.queries.push((q.clone(), transport.to_string()));
    if st.drop_all || st.drop_qtype.contains(&q.qtype) {
        return None;
    }
    if st.drop_first > 0 {
        st.drop_first -= 1;
        return None;
    }
    let truncate = st.truncate_udp && transport == "udp";
    let (rcode, records): (Rcode, Vec<(IpAddr, u32)>) = match st.names.get(&q.name) {
        Some(Behaviour::Records { v4, v6, ttl }) => {
            let list = match q.qtype {
                Qtype::A => v4,
                Qtype::Aaaa => v6,
            };
            (Rcode::NoError, list.iter().map(|ip| (*ip, *ttl)).collect())
        }
        Some(Behaviour::Empty) => (Rcode::NoError, Vec::new()),
        Some(Behaviour::Rcode(rc)) => (*rc, Vec::new()),
        None => (Rcode::NxDomain, Vec::new()),
    };
    let delay = st.delay;
    drop(st);
    let bytes = build_response(id, &q, rcode, &records, truncate).ok()?;
    Some((delay, bytes))
}

impl MockDns {
    pub async fn spawn() -> MockDns {
        Self::start(false).await
    }

    pub async fn spawn_tls() -> MockDns {
        Self::start(true).await
    }

    async fn start(tls: bool) -> MockDns {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
        let addr = listener.local_addr().expect("addr");
        let udp = Arc::new(
            UdpSocket::bind(addr)
                .await
                .expect("bind udp on the same port"),
        );
        let state = Arc::new(Mutex::new(State::default()));
        let acceptor = if tls { Some(tls_acceptor()) } else { None };

        let udp_state = state.clone();
        let udp_socket = udp.clone();
        let udp_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                // A prior reply to a peer that's already gone can surface as
                // a receive error (e.g. WSAECONNRESET on Windows); that's
                // noise, not the end of the transport, so log and keep
                // serving. Cancellation happens from the outside via abort
                // (see `Drop for MockDns`), never by returning here.
                let (n, peer) = match udp_socket.recv_from(&mut buf).await {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::debug!("mock dns udp recv error: {e}");
                        continue;
                    }
                };
                if let Some((delay, reply)) = respond(&udp_state, &buf[..n], "udp") {
                    let sock = udp_socket.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = sock.send_to(&reply, peer).await;
                    });
                }
            }
        });

        let tcp_state = state.clone();
        let tcp_task = tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::debug!("mock dns tcp accept error: {e}");
                        continue;
                    }
                };
                let st = tcp_state.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    match acceptor {
                        Some(a) => {
                            if let Ok(tls) = a.accept(stream).await {
                                serve_framed(tls, st, "tcp").await;
                            }
                        }
                        None => serve_framed(stream, st, "tcp").await,
                    }
                });
            }
        });
        MockDns {
            addr,
            state,
            udp_task,
            tcp_task,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn set(&self, name: &str, v4: &[&str], v6: &[&str], ttl: u32) {
        let v4 = v4.iter().map(|s| s.parse().expect("ipv4")).collect();
        let v6 = v6.iter().map(|s| s.parse().expect("ipv6")).collect();
        self.state
            .lock()
            .expect("mock state")
            .names
            .insert(norm(name), Behaviour::Records { v4, v6, ttl });
    }

    pub fn set_empty(&self, name: &str) {
        self.state
            .lock()
            .expect("mock state")
            .names
            .insert(norm(name), Behaviour::Empty);
    }

    pub fn set_rcode(&self, name: &str, rcode: Rcode) {
        self.state
            .lock()
            .expect("mock state")
            .names
            .insert(norm(name), Behaviour::Rcode(rcode));
    }

    pub fn set_delay(&self, delay: Duration) {
        self.state.lock().expect("mock state").delay = delay;
    }

    pub fn set_drop_all(&self, drop: bool) {
        self.state.lock().expect("mock state").drop_all = drop;
    }

    pub fn set_drop_first(&self, n: usize) {
        self.state.lock().expect("mock state").drop_first = n;
    }

    pub fn set_drop_qtype(&self, qtype: Qtype, drop: bool) {
        let mut st = self.state.lock().expect("mock state");
        if drop {
            st.drop_qtype.insert(qtype);
        } else {
            st.drop_qtype.remove(&qtype);
        }
    }

    pub fn set_truncate_udp(&self, truncate: bool) {
        self.state.lock().expect("mock state").truncate_udp = truncate;
    }

    pub fn queries(&self) -> Vec<(Question, String)> {
        self.state.lock().expect("mock state").queries.clone()
    }

    pub fn query_count(&self, name: &str, qtype: Qtype) -> usize {
        let name = norm(name);
        self.state
            .lock()
            .expect("mock state")
            .queries
            .iter()
            .filter(|(q, _)| q.name == name && q.qtype == qtype)
            .count()
    }
}

async fn serve_framed<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    mut stream: S,
    state: Arc<Mutex<State>>,
    transport: &str,
) {
    loop {
        let mut len = [0u8; 2];
        if stream.read_exact(&mut len).await.is_err() {
            return;
        }
        let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
        if stream.read_exact(&mut msg).await.is_err() {
            return;
        }
        let Some((delay, reply)) = respond(&state, &msg, transport) else {
            continue;
        };
        tokio::time::sleep(delay).await;
        let mut out = (reply.len() as u16).to_be_bytes().to_vec();
        out.extend_from_slice(&reply);
        if stream.write_all(&out).await.is_err() {
            return;
        }
    }
}

fn tls_acceptor() -> TlsAcceptor {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("self-signed cert");
    let cert_der = cert.der().clone();
    let key_der: rustls::pki_types::PrivateKeyDer<'static> = signing_key.into();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("server config");
    TlsAcceptor::from(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{build_query, parse_response};
    use tokio::net::TcpStream;

    fn q(name: &str, qtype: Qtype) -> Question {
        Question {
            name: name.to_string(),
            qtype,
        }
    }

    async fn udp_ask(addr: SocketAddr, wire: &[u8]) -> Option<Vec<u8>> {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        s.send_to(wire, addr).await.unwrap();
        let mut buf = vec![0u8; 4096];
        match tokio::time::timeout(Duration::from_millis(500), s.recv_from(&mut buf)).await {
            Ok(Ok((n, _))) => Some(buf[..n].to_vec()),
            _ => None,
        }
    }

    async fn tcp_ask<S: AsyncReadExt + AsyncWriteExt + Unpin>(
        stream: &mut S,
        wire: &[u8],
    ) -> Vec<u8> {
        let mut out = (wire.len() as u16).to_be_bytes().to_vec();
        out.extend_from_slice(wire);
        stream.write_all(&out).await.unwrap();
        let mut len = [0u8; 2];
        stream.read_exact(&mut len).await.unwrap();
        let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
        stream.read_exact(&mut msg).await.unwrap();
        msg
    }

    #[tokio::test]
    async fn answers_over_udp_and_tcp_and_records_queries() {
        let m = MockDns::spawn().await;
        m.set("A.test.", &["10.0.0.1"], &["fd00::1"], 42);
        let wire = build_query(5, &q("a.test", Qtype::A)).unwrap();
        let a = parse_response(&udp_ask(m.addr(), &wire).await.unwrap()).unwrap();
        assert_eq!(a.id, 5);
        assert_eq!(a.v4, vec![("10.0.0.1".parse().unwrap(), 42)]);
        let mut tcp = TcpStream::connect(m.addr()).await.unwrap();
        let aaaa = parse_response(
            &tcp_ask(
                &mut tcp,
                &build_query(6, &q("a.test", Qtype::Aaaa)).unwrap(),
            )
            .await,
        )
        .unwrap();
        assert_eq!(aaaa.v6, vec![("fd00::1".parse().unwrap(), 42)]);
        let nx = parse_response(
            &tcp_ask(
                &mut tcp,
                &build_query(7, &q("nope.test", Qtype::A)).unwrap(),
            )
            .await,
        )
        .unwrap();
        assert_eq!(nx.rcode, Rcode::NxDomain);
        assert_eq!(m.query_count("a.test", Qtype::A), 1);
        assert_eq!(m.query_count("a.test", Qtype::Aaaa), 1);
        assert_eq!(m.queries().len(), 3);
        assert_eq!(m.queries()[1].1, "tcp");
    }

    #[tokio::test]
    async fn misbehaviours() {
        let m = MockDns::spawn().await;
        m.set("a.test", &["10.0.0.1"], &[], 1);
        m.set_empty("empty.test");
        m.set_rcode("fail.test", Rcode::ServFail);
        let wire = build_query(1, &q("a.test", Qtype::A)).unwrap();
        m.set_drop_first(1);
        assert!(udp_ask(m.addr(), &wire).await.is_none());
        assert!(udp_ask(m.addr(), &wire).await.is_some());
        m.set_drop_qtype(Qtype::A, true);
        assert!(udp_ask(m.addr(), &wire).await.is_none());
        m.set_drop_qtype(Qtype::A, false);
        m.set_truncate_udp(true);
        let tc = parse_response(&udp_ask(m.addr(), &wire).await.unwrap()).unwrap();
        assert!(tc.truncated && tc.v4.is_empty());
        let mut tcp = TcpStream::connect(m.addr()).await.unwrap();
        let full = parse_response(&tcp_ask(&mut tcp, &wire).await).unwrap();
        assert!(!full.truncated && full.v4.len() == 1);
        m.set_truncate_udp(false);
        let e = parse_response(
            &udp_ask(
                m.addr(),
                &build_query(2, &q("empty.test", Qtype::A)).unwrap(),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(e.rcode == Rcode::NoError && e.v4.is_empty());
        let f = parse_response(
            &udp_ask(
                m.addr(),
                &build_query(3, &q("fail.test", Qtype::A)).unwrap(),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(f.rcode, Rcode::ServFail);
        m.set_delay(Duration::from_millis(200));
        let started = std::time::Instant::now();
        assert!(udp_ask(m.addr(), &wire).await.is_some());
        assert!(started.elapsed() >= Duration::from_millis(200));
        m.set_drop_all(true);
        assert!(udp_ask(m.addr(), &wire).await.is_none());
    }

    #[tokio::test]
    async fn tls_variant_serves_dot() {
        let m = MockDns::spawn_tls().await;
        m.set("dot.test", &["10.0.0.9"], &[], 9);
        let config = rurge_net::http::tls_client_config(true).unwrap();
        let tcp = TcpStream::connect(m.addr()).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1".to_string()).unwrap();
        let mut tls = tokio_rustls::TlsConnector::from(config)
            .connect(name, tcp)
            .await
            .unwrap();
        let a = parse_response(
            &tcp_ask(&mut tls, &build_query(8, &q("dot.test", Qtype::A)).unwrap()).await,
        )
        .unwrap();
        assert_eq!(a.v4, vec![("10.0.0.9".parse().unwrap(), 9)]);
        // UDP still works on the same port
        let u = parse_response(
            &udp_ask(m.addr(), &build_query(9, &q("dot.test", Qtype::A)).unwrap())
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(u.id, 9);
    }

    #[tokio::test]
    async fn dropping_the_mock_stops_both_transports() {
        let m = MockDns::spawn().await;
        m.set("a.test", &["10.0.0.1"], &[], 1);
        let addr = m.addr();
        let wire = build_query(1, &q("a.test", Qtype::A)).unwrap();
        // Sanity: the mock answers before it's dropped.
        assert!(udp_ask(addr, &wire).await.is_some());
        drop(m);

        // UDP: no task is left to answer. Depending on the OS, "nobody is
        // listening" surfaces either as a timeout or (e.g. Windows turning
        // an ICMP port-unreachable into WSAECONNRESET on the next recv) as
        // a receive error; either way no reply payload should arrive.
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        s.send_to(&wire, addr).await.unwrap();
        let mut buf = vec![0u8; 4096];
        let udp_reply =
            tokio::time::timeout(Duration::from_millis(200), s.recv_from(&mut buf)).await;
        assert!(
            !matches!(udp_reply, Ok(Ok(_))),
            "expected no UDP reply after drop, got {udp_reply:?}"
        );

        // TCP: the listener is gone, so connecting fails outright, or (if a
        // connect attempt races the socket's close) nothing is left to serve
        // the framed request and the read never completes.
        let tcp_outcome = tokio::time::timeout(Duration::from_millis(200), async {
            let mut stream = TcpStream::connect(addr).await?;
            let mut out = (wire.len() as u16).to_be_bytes().to_vec();
            out.extend_from_slice(&wire);
            stream.write_all(&out).await?;
            let mut len = [0u8; 2];
            stream.read_exact(&mut len).await?;
            Ok::<(), std::io::Error>(())
        })
        .await;
        assert!(
            !matches!(tcp_outcome, Ok(Ok(()))),
            "expected tcp connect/request to fail or time out after drop, got {tcp_outcome:?}"
        );
    }
}
