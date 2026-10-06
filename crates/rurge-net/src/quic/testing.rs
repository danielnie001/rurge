//! Loopback QUIC servers for tests: a certificate and the configurations on
//! either side of it, an echo server, a server socket on several ports for
//! port hopping, and a port whose endpoints restart. Never used by
//! production code.

use super::socket::NeverBlocks;
use quinn::crypto::rustls::QuicServerConfig;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{
    AsyncUdpSocket, Connection, ConnectionId, ConnectionIdGenerator, Endpoint, EndpointConfig,
    Runtime, ServerConfig, TokioRuntime, UdpPoller,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::collections::HashMap;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::task::JoinHandle;

/// A self-signed certificate: for `localhost` and `127.0.0.1` unless it is
/// made for other names.
pub struct TestCert {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Default for TestCert {
    fn default() -> Self {
        TestCert::new()
    }
}

impl TestCert {
    pub fn new() -> TestCert {
        TestCert::for_names(&["localhost", "127.0.0.1"])
    }

    /// A self-signed certificate for `names`: a name that is an IP address
    /// gets an IP SAN, any other a DNS one.
    pub fn for_names(names: &[&str]) -> TestCert {
        let names = names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(names).expect("a self-signed certificate");
        TestCert {
            cert: cert.der().clone(),
            key: signing_key.into(),
        }
    }

    /// Roots that trust the certificate.
    pub fn roots(&self) -> Arc<rustls::RootCertStore> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.cert.clone()).expect("the certificate");
        Arc::new(roots)
    }

    /// A server presenting the certificate over TLS 1.3, choosing among
    /// `alpn`; datagrams on, the rest quinn's defaults.
    pub fn server_config(&self, alpn: &[&str]) -> ServerConfig {
        let mut tls = rustls::ServerConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_no_client_auth()
            .with_single_cert(vec![self.cert.clone()], self.key.clone_key())
            .expect("the certificate and its key");
        tls.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        let crypto = QuicServerConfig::try_from(Arc::new(tls)).expect("a QUIC server config");
        ServerConfig::with_crypto(Arc::new(crypto))
    }

    /// A client's TLS 1.3 that trusts the certificate, offering `alpn`.
    pub fn client_tls(&self, alpn: &[&str]) -> Arc<rustls::ClientConfig> {
        let mut tls = rustls::ClientConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .expect("TLS 1.3")
            .with_root_certificates(self.roots())
            .with_no_client_auth();
        tls.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
        Arc::new(tls)
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A server endpoint presenting `config` on `socket`.
pub fn endpoint(config: ServerConfig, socket: Arc<dyn AsyncUdpSocket>) -> Endpoint {
    Endpoint::new_with_abstract_socket(
        EndpointConfig::default(),
        Some(config),
        socket,
        Arc::new(TokioRuntime),
    )
    .expect("a server endpoint")
}

/// A UDP socket on `127.0.0.1`, any port, that quinn-udp drives.
pub fn loopback_socket() -> Arc<dyn AsyncUdpSocket> {
    socket_at(SocketAddr::from(([127, 0, 0, 1], 0)))
}

/// A UDP socket on `addr` that quinn-udp drives.
pub fn socket_at(addr: SocketAddr) -> Arc<dyn AsyncUdpSocket> {
    let socket = std::net::UdpSocket::bind(addr).expect("a loopback socket");
    TokioRuntime
        .wrap_udp_socket(socket)
        .expect("a quinn socket")
}

/// Answers on every connection: each bidirectional stream with what it
/// carries, each datagram with itself. Dropping it closes the endpoint.
pub struct EchoServer {
    endpoint: Endpoint,
    addr: SocketAddr,
    accept: JoinHandle<()>,
}

impl EchoServer {
    /// On `127.0.0.1`, any port.
    pub fn spawn(config: ServerConfig) -> EchoServer {
        EchoServer::spawn_on(config, loopback_socket())
    }

    /// On `socket` (`MultiPort` for port hopping).
    pub fn spawn_on(config: ServerConfig, socket: Arc<dyn AsyncUdpSocket>) -> EchoServer {
        let addr = socket.local_addr().expect("the server's address");
        let endpoint = endpoint(config, socket);
        let accept = tokio::spawn({
            let endpoint = endpoint.clone();
            async move {
                while let Some(incoming) = endpoint.accept().await {
                    tokio::spawn(async move {
                        if let Ok(connection) = incoming.await {
                            echo(connection).await;
                        }
                    });
                }
            }
        });
        EchoServer {
            endpoint,
            addr,
            accept,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for EchoServer {
    fn drop(&mut self) {
        self.accept.abort();
        self.endpoint.close(0u32.into(), b"");
    }
}

async fn echo(connection: Connection) {
    loop {
        tokio::select! {
            stream = connection.accept_bi() => {
                let Ok((mut send, mut recv)) = stream else { return };
                tokio::spawn(async move {
                    if tokio::io::copy(&mut recv, &mut send).await.is_ok() {
                        let _ = send.finish();
                    }
                });
            }
            datagram = connection.read_datagram() => {
                let Ok(datagram) = datagram else { return };
                let _ = connection.send_datagram(datagram);
            }
        }
    }
}

/// A server socket on several loopback ports at once, for port hopping
/// (phase 2 M7 design 3.3). It hears on every port and answers each client
/// address from the port it last heard that address on, as a server behind
/// a port-range DNAT does; it counts what each port heard.
#[derive(Debug)]
pub struct MultiPort {
    sockets: Vec<Arc<dyn AsyncUdpSocket>>,
    ports: Vec<u16>,
    heard: Vec<AtomicUsize>,
    /// The socket each client address was last heard on.
    routes: Mutex<HashMap<SocketAddr, usize>>,
}

impl MultiPort {
    /// On `count` ports of `127.0.0.1`.
    pub fn bind(count: usize) -> Arc<MultiPort> {
        let sockets: Vec<_> = (0..count).map(|_| loopback_socket()).collect();
        let ports = sockets
            .iter()
            .map(|s| s.local_addr().expect("a port").port())
            .collect();
        Arc::new(MultiPort {
            heard: sockets.iter().map(|_| AtomicUsize::new(0)).collect(),
            sockets,
            ports,
            routes: Mutex::default(),
        })
    }

    pub fn ports(&self) -> &[u16] {
        &self.ports
    }

    /// How many datagrams each port heard, in the order of `ports`.
    pub fn heard(&self) -> Vec<usize> {
        self.heard
            .iter()
            .map(|n| n.load(Ordering::Relaxed))
            .collect()
    }

    /// How many client addresses it heard from.
    pub fn clients(&self) -> usize {
        self.routes.lock().expect("the routes").len()
    }
}

impl AsyncUdpSocket for MultiPort {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(NeverBlocks)
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let at = self
            .routes
            .lock()
            .expect("the routes")
            .get(&transmit.destination)
            .copied()
            .unwrap_or(0);
        match self.sockets[at].try_send(transmit) {
            // dropped, as by a full socket: one port waiting stalls no other
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(()),
            sent => sent,
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        for (at, socket) in self.sockets.iter().enumerate() {
            if let Poll::Ready(received) = socket.poll_recv(cx, bufs, meta) {
                let n = received?;
                let mut routes = self.routes.lock().expect("the routes");
                for meta in &meta[..n] {
                    routes.insert(meta.addr, at);
                    self.heard[at].fetch_add(1, Ordering::Relaxed);
                }
                return Poll::Ready(Ok(n));
            }
        }
        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sockets[0].local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.sockets[0].max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.sockets[0].max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.sockets[0].may_fragment()
    }
}

/// A loopback UDP port for QUIC servers that restart (phase 2 M7 design
/// 3.5: the stale connection). Its endpoints share the key of their
/// stateless resets and the scheme of their connection IDs, as a server
/// restarted with the same configuration: a new endpoint knows none of the
/// connections of the one before and answers their packets with a
/// stateless reset (RFC 9000 10.3), which ends them at the client. The
/// endpoint before hears and sends nothing any more. A restart that lost
/// the key (`endpoint_with_new_key`) ends nothing: the connections of the
/// endpoint before go on, unanswered.
pub struct ServerPort {
    socket: Arc<dyn AsyncUdpSocket>,
    config: EndpointConfig,
    /// The way to the port of the endpoint that answers now.
    current: Mutex<Option<Arc<Gate>>>,
}

impl ServerPort {
    /// On `127.0.0.1`, any port.
    pub fn bind() -> ServerPort {
        ServerPort::bind_at(SocketAddr::from(([127, 0, 0, 1], 0)))
    }

    /// On `addr`.
    pub fn bind_at(addr: SocketAddr) -> ServerPort {
        let mut config = EndpointConfig::default();
        let ids = Arc::new(AtomicU64::new(1));
        config.cid_generator(move || Box::new(SharedIds(ids.clone())));
        ServerPort {
            socket: socket_at(addr),
            config,
            current: Mutex::new(None),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.socket.local_addr().expect("the port's address")
    }

    /// A server endpoint presenting `config` that answers on the port from
    /// now on; the one before it falls silent.
    pub fn endpoint(&self, config: ServerConfig) -> Endpoint {
        self.endpoint_over(config, |socket| socket)
    }

    /// As `endpoint`, its packets through what `layer` lays over the port
    /// (a fake's obfuscation): a layer of its own for each endpoint.
    pub fn endpoint_over(
        &self,
        config: ServerConfig,
        layer: impl FnOnce(Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket>,
    ) -> Endpoint {
        self.endpoint_with(self.config.clone(), config, layer)
    }

    /// As `endpoint`, of a server that does not have the key the stateless
    /// resets of the endpoints before it were made with — a restart that
    /// lost it. What it answers the packets of their connections with, no
    /// client can tell from noise: those connections are not told that they
    /// are gone, and go on talking to nobody until their idle timeout.
    pub fn endpoint_with_new_key(&self, config: ServerConfig) -> Endpoint {
        self.endpoint_with(EndpointConfig::default(), config, |socket| socket)
    }

    fn endpoint_with(
        &self,
        endpoint: EndpointConfig,
        config: ServerConfig,
        layer: impl FnOnce(Arc<dyn AsyncUdpSocket>) -> Arc<dyn AsyncUdpSocket>,
    ) -> Endpoint {
        let gate = Arc::new(Gate {
            socket: self.socket.clone(),
            open: AtomicBool::new(true),
        });
        let before = self.current.lock().expect("the gate").replace(gate.clone());
        if let Some(before) = before {
            before.open.store(false, Ordering::SeqCst);
        }
        Endpoint::new_with_abstract_socket(
            endpoint,
            Some(config),
            layer(gate),
            Arc::new(TokioRuntime),
        )
        .expect("a server endpoint")
    }
}

/// Connection IDs from a counter that all of a port's endpoints share:
/// never the same twice, and each taken for one of theirs (the trait's own
/// check), so that a packet for a connection an endpoint never had gets a
/// stateless reset instead of being dropped as not its own.
struct SharedIds(Arc<AtomicU64>);

impl ConnectionIdGenerator for SharedIds {
    fn generate_cid(&mut self) -> ConnectionId {
        ConnectionId::new(&self.0.fetch_add(1, Ordering::Relaxed).to_be_bytes())
    }

    fn cid_len(&self) -> usize {
        8
    }

    fn cid_lifetime(&self) -> Option<Duration> {
        None
    }
}

/// One endpoint's way to the port: open, everything passes; closed — for
/// good —, the endpoint hears nothing and what it sends is dropped.
#[derive(Debug)]
struct Gate {
    socket: Arc<dyn AsyncUdpSocket>,
    open: AtomicBool,
}

impl AsyncUdpSocket for Gate {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.socket.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        if self.open.load(Ordering::SeqCst) {
            self.socket.try_send(transmit)
        } else {
            Ok(())
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if !self.open.load(Ordering::SeqCst) {
            // never opened again: nothing to wake the endpoint for
            return Poll::Pending;
        }
        self.socket.poll_recv(cx, bufs, meta)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.socket.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.socket.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.socket.may_fragment()
    }
}
