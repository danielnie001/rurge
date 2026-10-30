//! The listener of the API when `http-api-tls` is on (phase 4 M2 design
//! 14.1): every connection of it is a TLS connection, whose certificate the
//! MITM CA signs for the name the client asked for, or for the address the
//! client connected to when it asked for no name.
//!
//! The handshake is not done in the accept loop. A connection that is accepted
//! is given a place of its own among [`TLS_HANDSHAKES`] and goes to a task,
//! which is given [`TLS_HANDSHAKE_TIME`]; a connection for which there is no
//! place left is closed at once, and one whose handshake does not finish in
//! time is closed when its time is up. What a task has handshaken is handed to
//! `axum` over a channel, which is what `accept` answers with, so that a client
//! that says nothing holds up no other connection. Stopping the server ends the
//! handshakes as well.
//!
//! Closing is the same as for a plain connection: a TLS stream that is dropped
//! does not write a `close_notify` (phase 4 M2 design 14.1), and the TCP
//! connection underneath is let go of by `Closing`, in the stages RFC 9112
//! section 9.6 describes (`the phase 4 / M2c plan's P10`).

use crate::linger::{Closing, LINGER_MAX, Lingering, Peer};
use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use rurge_config::HostName;
use rurge_engine::ApiTls;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio_rustls::LazyConfigAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// How the listener of the API gets the TLS of the API: asked once for each
/// connection, so that the connections after a reload that brought another CA
/// are signed with that one (the phase 4 / M2c plan's P9).
pub(crate) type TlsSource = Arc<dyn Fn() -> Option<ApiTls> + Send + Sync>;

/// How many handshakes are in flight at once: the same count as the
/// connections that are read away from (`LINGER_MAX`), so that a client that
/// connects over and over holds this many descriptors and no more.
pub(crate) const TLS_HANDSHAKES: usize = LINGER_MAX;

/// The longest a handshake is given (`M2-D21`): a client that says nothing, or
/// that speaks too slowly, is closed.
pub(crate) const TLS_HANDSHAKE_TIME: Duration = Duration::from_secs(30);

/// The listener of the API over TLS: the connections of [`Lingering`], with a
/// handshake in a task of its own before `axum` sees one.
pub(crate) struct TlsListener {
    inner: Lingering,
    source: TlsSource,
    /// The places among the handshakes in flight.
    room: Arc<Semaphore>,
    /// How long one handshake is given.
    time: Duration,
    /// Cancelled when the server stops: the handshakes are let go of.
    stop: CancellationToken,
    /// The connections of `inner`, from the accept to the end of the
    /// lingering: the handshake tasks are counted here too, so that a server
    /// that has stopped and waited for its connections has none of them
    /// running.
    tracker: TaskTracker,
    ready_tx: mpsc::Sender<(TlsStream<Closing>, SocketAddr)>,
    ready_rx: mpsc::Receiver<(TlsStream<Closing>, SocketAddr)>,
}

impl TlsListener {
    pub(crate) fn new(inner: Lingering, source: TlsSource, stop: CancellationToken) -> TlsListener {
        TlsListener::with_limits(inner, source, stop, TLS_HANDSHAKES, TLS_HANDSHAKE_TIME)
    }

    pub(crate) fn with_limits(
        inner: Lingering,
        source: TlsSource,
        stop: CancellationToken,
        max: usize,
        time: Duration,
    ) -> TlsListener {
        let tracker = inner.connections();
        let (ready_tx, ready_rx) = mpsc::channel(TLS_HANDSHAKES);
        TlsListener {
            inner,
            source,
            room: Arc::new(Semaphore::new(max)),
            time,
            stop,
            tracker,
            ready_tx,
            ready_rx,
        }
    }

    /// Takes a connection that was accepted into a task of its own, with a
    /// place among the handshakes, or closes it when there is no place left.
    fn start(&self, closing: Closing, peer: SocketAddr) {
        let Ok(room) = Arc::clone(&self.room).try_acquire_owned() else {
            // the connection is closed, and lingered on, by being dropped
            tracing::debug!(
                %peer,
                "http-api: too many TLS handshakes at once; a connection is closed"
            );
            return;
        };
        self.tracker.spawn(handshake(
            closing,
            peer,
            Arc::clone(&self.source),
            self.time,
            self.stop.clone(),
            self.ready_tx.clone(),
            room,
        ));
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<Closing>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (TlsStream<Closing>, SocketAddr) {
        loop {
            let (closing, peer) = tokio::select! {
                // a connection whose handshake is done first: it has waited
                // at least as long as anything still to be accepted
                biased;
                Some(ready) = self.ready_rx.recv() => return ready,
                accepted = self.inner.accept() => accepted,
            };
            self.start(closing, peer);
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

/// One handshake, with the place among the handshakes in flight held until the
/// stream is handed over or the connection is closed.
async fn handshake(
    closing: Closing,
    peer: SocketAddr,
    source: TlsSource,
    time: Duration,
    stop: CancellationToken,
    ready: mpsc::Sender<(TlsStream<Closing>, SocketAddr)>,
    _room: OwnedSemaphorePermit,
) {
    let local = match closing.local_addr() {
        Ok(local) => local,
        Err(e) => {
            tracing::debug!(%peer, error = %e, "http-api: a TLS handshake failed");
            return;
        }
    };
    let shake = async move {
        let start = LazyConfigAcceptor::new(rustls::server::Acceptor::default(), closing).await?;
        let name = name_of(start.client_hello().server_name(), local);
        let tls = source().ok_or_else(|| io::Error::other("no MITM CA to serve TLS with"))?;
        let config = tls
            .config(&name)
            .await
            .map_err(|e| io::Error::other(e.to_string()))?;
        start.into_stream(config).await
    };
    let outcome = tokio::select! {
        _ = stop.cancelled() => return,
        outcome = tokio::time::timeout(time, shake) => outcome,
    };
    match outcome {
        Ok(Ok(stream)) => {
            tokio::select! {
                _ = stop.cancelled() => {}
                _ = ready.send((stream, peer)) => {}
            }
        }
        Ok(Err(e)) => tracing::debug!(%peer, error = %e, "http-api: a TLS handshake failed"),
        Err(_) => tracing::debug!(%peer, "http-api: a TLS handshake did not finish in time"),
    }
}

/// The name the certificate of one connection is signed for: the one the
/// client asked for, or the address it connected to when it asked for none. A
/// listener on the wildcard address is reached at a mapped address by a client
/// of the other family, and the certificate names the address a client can
/// write.
fn name_of(sni: Option<&str>, local: SocketAddr) -> HostName {
    sni.map(HostName::parse)
        .unwrap_or_else(|| HostName::Ip(local.ip().to_canonical()))
}

/// The address of the peer of a connection over TLS, as `axum` asks for it.
impl Connected<IncomingStream<'_, TlsListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Peer {
        Peer(*stream.remote_addr())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::serve::Listener;
    use rurge_config::MitmConfig;
    use rurge_http::mitm::Authority;
    use rurge_http::mitm::api::client_config;
    use rurge_proto::testing::TlsFixture;
    use rustls::pki_types::ServerName;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Instant;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;
    use tokio_rustls::TlsConnector;
    use x509_parser::extensions::GeneralName;
    use x509_parser::parse_x509_certificate;

    /// The most any step of these cases may take. A step that does not finish
    /// within it is a failure: a handshake that hangs is what is being guarded
    /// against.
    const BOUND: Duration = Duration::from_secs(5);

    /// A CA in memory, and the TLS of an API that signs with it.
    struct Fixture {
        ca: Arc<TlsFixture>,
        api: ApiTls,
    }

    fn fixture() -> Fixture {
        let ca = TlsFixture::new(&[]);
        let cfg = MitmConfig {
            ca_p12: Some(ca.ca_p12_base64("pw")),
            ca_passphrase: Some("pw".to_string()),
            ..MitmConfig::default()
        };
        let authority = Arc::new(
            Authority::load(&cfg, &[])
                .expect("a usable CA")
                .expect("a CA was named"),
        );
        Fixture {
            ca,
            api: ApiTls::new(authority).expect("the TLS of the API"),
        }
    }

    /// A source that hands out `api` for every connection.
    fn signing(api: ApiTls) -> TlsSource {
        Arc::new(move || Some(api.clone()))
    }

    /// The TLS of the API over a listener on a free port of the loopback,
    /// driven in a task of its own: the connections whose handshake is done
    /// come back on `ready`.
    struct Rig {
        addr: SocketAddr,
        ready: mpsc::Receiver<(TlsStream<Closing>, SocketAddr)>,
        stop: CancellationToken,
        connections: TaskTracker,
    }

    impl Rig {
        async fn new(source: TlsSource, max: usize, time: Duration) -> Rig {
            let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = tcp.local_addr().unwrap();
            let lingering = Lingering::new(tcp);
            let connections = lingering.connections();
            let stop = CancellationToken::new();
            let mut listener = TlsListener::with_limits(lingering, source, stop.clone(), max, time);
            let (tx, ready) = mpsc::channel(8);
            tokio::spawn(async move {
                loop {
                    let (stream, peer) = listener.accept().await;
                    if tx.send((stream, peer)).await.is_err() {
                        return;
                    }
                }
            });
            Rig {
                addr,
                ready,
                stop,
                connections,
            }
        }
    }

    /// Waits, up to the bound, for `f` to say so: the cases below need a
    /// connection to have been accepted before the next one is made, and the
    /// order the kernel accepts them in is not promised. The tracker counts a
    /// connection from the moment it is accepted, and the handshake task that
    /// carries it on top of that, so a case asks for at least as many as it
    /// has made.
    async fn until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + BOUND;
        while !f() {
            assert!(
                Instant::now() < deadline,
                "{what} did not happen within the bound"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// A plain connection to `addr`, with nothing said on it yet.
    async fn dial(addr: SocketAddr) -> TcpStream {
        tokio::time::timeout(BOUND, TcpStream::connect(addr))
            .await
            .expect("a loopback connection within the bound")
            .expect("a loopback connection")
    }

    /// The TLS handshake of a client that trusts `ca` alone, over `stream`.
    async fn tls_over(
        stream: TcpStream,
        ca: &[u8],
        name: ServerName<'static>,
    ) -> io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        let config = client_config(ca).expect("a client configuration");
        tokio::time::timeout(BOUND, TlsConnector::from(config).connect(name, stream))
            .await
            .expect("the handshake finishes within the bound")
    }

    /// A client of `addr` that trusts `ca` alone, with the handshake done.
    async fn handshake_with(
        addr: SocketAddr,
        ca: &[u8],
        name: ServerName<'static>,
    ) -> io::Result<tokio_rustls::client::TlsStream<TcpStream>> {
        tls_over(dial(addr).await, ca, name).await
    }

    /// The address the client of a `handshake_with` stream connected from.
    fn client_addr(stream: &tokio_rustls::client::TlsStream<TcpStream>) -> SocketAddr {
        stream.get_ref().0.local_addr().expect("an address")
    }

    /// The subject alternative names of the certificate `stream` was served,
    /// each written the way `IpAddr` writes an address.
    fn san_of(stream: &tokio_rustls::client::TlsStream<TcpStream>) -> Vec<String> {
        let leaf = stream
            .get_ref()
            .1
            .peer_certificates()
            .expect("a certificate")[0]
            .clone();
        let (_, cert) = parse_x509_certificate(leaf.as_ref()).expect("a certificate");
        let san = cert
            .subject_alternative_name()
            .expect("the extensions parse")
            .expect("a subject alternative name");
        san.value
            .general_names
            .iter()
            .map(|name| match name {
                GeneralName::DNSName(name) => name.to_string(),
                GeneralName::IPAddress(raw) => {
                    let bytes = <[u8; 4]>::try_from(*raw).expect("an IPv4 address");
                    IpAddr::V4(bytes.into()).to_string()
                }
                other => panic!("a name of another kind: {other:?}"),
            })
            .collect()
    }

    /// Asserts that the server closed `stream`: its peer reads the end of it,
    /// or a reset. A connection the server lets go of is shut down and what it
    /// still sends is read away, and a system that had the bytes of a reset in
    /// flight first reports that one instead; on Windows it does.
    async fn assert_closed(stream: &mut TcpStream) {
        let mut buf = [0u8; 64];
        let read = tokio::time::timeout(BOUND, stream.read(&mut buf))
            .await
            .expect("the server closes the connection within the bound");
        match read {
            Ok(0) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                ) => {}
            Ok(n) => panic!("the server sent {n} bytes instead of closing the connection"),
            Err(e) => panic!("the connection failed instead of being closed: {e}"),
        }
    }

    /// A connection whose handshake is done is handed over with what came
    /// after the handshake, and with the client's own address.
    #[tokio::test]
    async fn a_connection_is_handed_over_once_its_handshake_is_done() {
        let Fixture { ca, api } = fixture();
        let mut rig = Rig::new(signing(api), TLS_HANDSHAKES, TLS_HANDSHAKE_TIME).await;
        let name = ServerName::IpAddress(IpAddr::from(Ipv4Addr::LOCALHOST).into());
        let mut client = handshake_with(rig.addr, ca.ca_der(), name)
            .await
            .expect("the client trusts the CA");
        let peer = client_addr(&client);
        tokio::time::timeout(BOUND, client.write_all(b"ping"))
            .await
            .expect("the write finishes within the bound")
            .expect("a write");
        let (mut served, addr) = tokio::time::timeout(BOUND, rig.ready.recv())
            .await
            .expect("the connection is handed over within the bound")
            .expect("the listener is still running");
        assert_eq!(addr, peer, "the address the client connected from");
        let mut got = [0u8; 4];
        tokio::time::timeout(BOUND, served.read_exact(&mut got))
            .await
            .expect("the answer finishes within the bound")
            .expect("a read");
        assert_eq!(&got, b"ping", "what the client sent after the handshake");
    }

    /// A client that says nothing holds up no other connection: the one whose
    /// handshake is done is handed over while the silent one is still
    /// connected.
    #[tokio::test]
    async fn a_silent_client_does_not_hold_up_the_next() {
        let Fixture { ca, api } = fixture();
        let mut rig = Rig::new(signing(api), TLS_HANDSHAKES, TLS_HANDSHAKE_TIME).await;
        let mut silent = dial(rig.addr).await;
        let talking = handshake_with(
            rig.addr,
            ca.ca_der(),
            ServerName::IpAddress(IpAddr::from(Ipv4Addr::LOCALHOST).into()),
        )
        .await
        .expect("the client trusts the CA");
        let (served, addr) = tokio::time::timeout(BOUND, rig.ready.recv())
            .await
            .expect("the connection is handed over within the bound")
            .expect("the listener is still running");
        assert_eq!(
            addr,
            client_addr(&talking),
            "the connection that shook hands, not the one that said nothing"
        );
        drop(served);
        // Nothing happens here, and that is what is asserted: the silent
        // client is still connected, and only a connection the server closed
        // or wrote to would end this read. The whole of it is 200 ms against
        // the 5 s a case may take, and a loopback answers in milliseconds.
        let mut buf = [0u8; 1];
        assert!(
            tokio::time::timeout(Duration::from_millis(200), silent.read(&mut buf))
                .await
                .is_err(),
            "the silent connection was closed or answered instead of being left open"
        );
    }

    /// At most `max` handshakes are in flight: a connection beyond that is
    /// closed at once.
    #[tokio::test]
    async fn handshakes_beyond_the_limit_are_closed() {
        let Fixture { api, .. } = fixture();
        let rig = Rig::new(signing(api), 2, Duration::from_secs(600)).await;
        let _first = dial(rig.addr).await;
        until("the first connection to be accepted", || {
            !rig.connections.is_empty()
        })
        .await;
        let _second = dial(rig.addr).await;
        until("the second connection to be accepted", || {
            rig.connections.len() >= 2
        })
        .await;
        let mut third = dial(rig.addr).await;
        until("the third connection to be accepted", || {
            rig.connections.len() >= 3
        })
        .await;
        assert_closed(&mut third).await;
    }

    /// A handshake that does not finish in time is closed when its time is up,
    /// and not before.
    #[tokio::test]
    async fn a_handshake_that_does_not_finish_in_time_is_closed() {
        let Fixture { api, .. } = fixture();
        let rig = Rig::new(signing(api), TLS_HANDSHAKES, Duration::from_millis(300)).await;
        let started = Instant::now();
        let mut silent = dial(rig.addr).await;
        assert_closed(&mut silent).await;
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(250),
            "closed after {waited:?}, before its time was up"
        );
    }

    /// Stopping the server lets go of the handshakes in flight.
    #[tokio::test]
    async fn a_stop_lets_go_of_the_handshakes() {
        let Fixture { api, .. } = fixture();
        let rig = Rig::new(signing(api), TLS_HANDSHAKES, Duration::from_secs(600)).await;
        let mut silent = dial(rig.addr).await;
        until("the connection to be accepted", || {
            !rig.connections.is_empty()
        })
        .await;
        rig.stop.cancel();
        assert_closed(&mut silent).await;
    }

    /// The certificate names the name the client asked for, or the address it
    /// connected to when it asked for none: rustls sends no SNI for an
    /// address.
    #[tokio::test]
    async fn the_certificate_names_the_sni_or_the_local_address() {
        let Fixture { ca, api } = fixture();
        let rig = Rig::new(signing(api), TLS_HANDSHAKES, TLS_HANDSHAKE_TIME).await;
        let by_address = handshake_with(
            rig.addr,
            ca.ca_der(),
            ServerName::IpAddress(IpAddr::from(Ipv4Addr::LOCALHOST).into()),
        )
        .await
        .expect("the client trusts the CA, and the leaf names the address");
        assert_eq!(
            san_of(&by_address),
            ["127.0.0.1"],
            "the leaf names the address the client connected to"
        );
        let by_name = handshake_with(
            rig.addr,
            ca.ca_der(),
            ServerName::try_from("api.test".to_string()).expect("a name"),
        )
        .await
        .expect("the client trusts the CA, and the leaf names the name");
        assert_eq!(
            san_of(&by_name),
            ["api.test"],
            "the leaf names the name the client asked for"
        );
    }

    /// A listener on the wildcard address is reached at a mapped address by a
    /// client of the other family, and the certificate names the address a
    /// client can write.
    #[test]
    fn a_mapped_local_address_is_named_as_ipv4() {
        assert_eq!(
            name_of(None, "[::ffff:127.0.0.1]:6171".parse().unwrap()),
            HostName::Ip(IpAddr::from(Ipv4Addr::LOCALHOST))
        );
        assert_eq!(
            name_of(Some("API.Test"), "127.0.0.1:6171".parse().unwrap()),
            HostName::Domain("api.test".to_string())
        );
    }

    /// With no CA to sign with, there is no certificate: the handshake fails
    /// and the connection is closed.
    #[tokio::test]
    async fn without_a_source_the_connection_is_closed() {
        let ca = TlsFixture::new(&[]);
        let rig = Rig::new(Arc::new(|| None), TLS_HANDSHAKES, TLS_HANDSHAKE_TIME).await;
        let stream = dial(rig.addr).await;
        until("the connection to be accepted", || {
            !rig.connections.is_empty()
        })
        .await;
        let handshake = tls_over(
            stream,
            ca.ca_der(),
            ServerName::IpAddress(IpAddr::from(Ipv4Addr::LOCALHOST).into()),
        )
        .await;
        assert!(
            handshake.is_err(),
            "the client was given a certificate with no CA behind it"
        );
    }
}
