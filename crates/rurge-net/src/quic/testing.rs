//! Loopback QUIC servers for tests: a certificate and the configurations on
//! either side of it, an echo server, a server socket on several ports for
//! port hopping, a port whose endpoints restart, and a client's first
//! datagram. Also Initial packets sealed frame by frame, the datagrams they
//! come in, and the ClientHello rustls writes, for the tests that read them
//! (phase 4 / M5a). Never used by production code.

use quinn::crypto::rustls::QuicServerConfig;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{
    AsyncUdpSocket, Connection, ConnectionId, ConnectionIdGenerator, Endpoint, EndpointConfig,
    Runtime, ServerConfig, TokioRuntime, UdpPoller,
};
use rustls::Side;
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

/// Never waits: for sockets that drop what they cannot send now, as a
/// full socket buffer would.
#[derive(Debug)]
pub(crate) struct NeverBlocks;

impl UdpPoller for NeverBlocks {
    fn poll_writable(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
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

/// A client's first datagram: one Initial packet of `version` (1,
/// 0x6b3343cf or a draft 0xff00001d–0xff000020) to the Destination
/// Connection ID `dcid` (any length, so that a test can make a wrong one),
/// sealed with the client Initial keys of that DCID and version, header
/// protection applied (RFC 9001 §5), the datagram 1200 bytes long (plan
/// P5). Panics on any other version.
pub fn client_initial(version: u32, dcid: &[u8]) -> Vec<u8> {
    client_initial_with_quic_bit(version, dcid, true)
}

/// The same, with the QUIC bit (RFC 9000 §17.2's fixed bit) set or clear in
/// the first byte before the packet is sealed: RFC 9287 §3.1 lets a client
/// clear it, and header protection does not cover it.
pub fn client_initial_with_quic_bit(version: u32, dcid: &[u8], quic_bit: bool) -> Vec<u8> {
    let quic = crate::quic::initial::quic_version(version)
        .unwrap_or_else(|| panic!("not a version whose Initial keys we have: {version:#x}"));
    let kind: u8 = match quic {
        rustls::quic::Version::V2 => 0b01,
        _ => 0b00,
    };
    let dcid_len = u8::try_from(dcid.len()).expect("a connection ID of at most 255 bytes");
    // a long header, the version's Initial type, and a 4-byte packet number;
    // the QUIC bit is the caller's
    let mut datagram = Vec::with_capacity(1200);
    datagram.push(0x80 | if quic_bit { 0x40 } else { 0 } | (kind << 4) | 0x03);
    datagram.extend_from_slice(&version.to_be_bytes());
    datagram.push(dcid_len);
    datagram.extend_from_slice(dcid);
    datagram.push(0); // source connection ID length
    datagram.push(0); // token length
    // the Length field in two bytes, so that the packet fills 1200 exactly
    let pn_offset = datagram.len() + 2;
    let length = u16::try_from(1200 - pn_offset).expect("room for the Length field");
    datagram.extend_from_slice(&(0x4000 | length).to_be_bytes());
    let pn_offset = datagram.len();
    datagram.extend_from_slice(&[0u8; 4]); // packet number 0
    // the plaintext: PADDING frames only — the check reads no frame — up to
    // where the 16-byte tag starts
    datagram.resize(1200 - 16, 0);

    let keys = crate::quic::initial::initial_suite().keys(dcid, Side::Client, quic);
    let (header, payload) = datagram.split_at_mut(pn_offset + 4);
    let tag = keys
        .local
        .packet
        .encrypt_in_place(0, header, payload)
        .expect("a payload the AEAD takes");
    datagram.extend_from_slice(tag.as_ref());

    // header protection (RFC 9001 §5.4): the sample is the ciphertext at
    // packet number offset plus four
    let sample: [u8; 16] = datagram[pn_offset + 4..pn_offset + 4 + 16]
        .try_into()
        .expect("the sample");
    let mut first = datagram[0];
    let mut number = [0u8; 4];
    number.copy_from_slice(&datagram[pn_offset..pn_offset + 4]);
    keys.local
        .header
        .encrypt_in_place(&sample, &mut first, &mut number)
        .expect("a 16-byte sample");
    datagram[0] = first;
    datagram[pn_offset..pn_offset + 4].copy_from_slice(&number);
    datagram
}

// The frames an Initial packet carries, and the packets themselves: what
// `initial.rs` reads is built here, byte by byte (phase 4 / M5a plan P3).

/// A frame of an Initial packet, as `encode` writes it: the frames these
/// tests need, not every frame QUIC has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// `count` zero bytes; one frame or several, a reader cannot tell them
    /// apart (RFC 9000 §19.1).
    Padding(usize),
    Ping,
    Ack {
        largest: u64,
        delay: u64,
        first_range: u64,
        /// Each further range as the gap before it and its length
        /// (RFC 9000 §19.3.1).
        ranges: Vec<(u64, u64)>,
        /// The three ECN counts, where the frame carries them (type 0x03).
        ecn: Option<[u64; 3]>,
    },
    Crypto {
        offset: u64,
        data: Vec<u8>,
    },
    ConnectionClose {
        error: u64,
        frame_type: u64,
        reason: Vec<u8>,
    },
    /// Bytes written as they are: a frame an Initial may not carry, or one
    /// cut short, for the reader to trip over.
    Raw(Vec<u8>),
}

impl Frame {
    /// Write the frame into `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Frame::Padding(count) => out.resize(out.len() + count, 0),
            Frame::Ping => out.push(0x01),
            Frame::Ack {
                largest,
                delay,
                first_range,
                ranges,
                ecn,
            } => {
                out.push(if ecn.is_some() { 0x03 } else { 0x02 });
                varint(out, *largest);
                varint(out, *delay);
                varint(out, u64::try_from(ranges.len()).expect("a range count"));
                varint(out, *first_range);
                for (gap, length) in ranges {
                    varint(out, *gap);
                    varint(out, *length);
                }
                if let Some(counts) = ecn {
                    for count in counts {
                        varint(out, *count);
                    }
                }
            }
            Frame::Crypto { offset, data } => {
                out.push(0x06);
                varint(out, *offset);
                varint(out, u64::try_from(data.len()).expect("a length"));
                out.extend_from_slice(data);
            }
            Frame::ConnectionClose {
                error,
                frame_type,
                reason,
            } => {
                out.push(0x1c);
                varint(out, *error);
                varint(out, *frame_type);
                varint(out, u64::try_from(reason.len()).expect("a length"));
                out.extend_from_slice(reason);
            }
            Frame::Raw(bytes) => out.extend_from_slice(bytes),
        }
    }
}

/// A variable-length integer in the shortest form that holds `value`
/// (RFC 9000 §16).
fn varint(out: &mut Vec<u8>, value: u64) {
    match value {
        0..=0x3f => out.push(value as u8),
        0x40..=0x3fff => out.extend_from_slice(&(0x4000u16 | value as u16).to_be_bytes()),
        0x4000..=0x3fff_ffff => {
            out.extend_from_slice(&(0x8000_0000u32 | value as u32).to_be_bytes())
        }
        _ => out.extend_from_slice(&(0xc000_0000_0000_0000u64 | value).to_be_bytes()),
    }
}

/// An Initial packet to seal, and what to seal it with.
#[derive(Clone, Debug)]
pub struct Initial {
    /// The version on the wire: 1, 0x6b3343cf, or a draft 0xff00001d to
    /// 0xff000020. `seal` panics on any other.
    pub version: u32,
    /// The Destination Connection ID the packet names.
    pub dcid: Vec<u8>,
    /// The bytes its keys come from: the Destination Connection ID, unless
    /// a test says otherwise.
    pub keys_from: Vec<u8>,
    pub number: u64,
    /// One to four: how many bytes of `number` the packet carries. `new`
    /// picks the shortest that holds `number`; a test may set another.
    pub number_len: usize,
    pub frames: Vec<Frame>,
}

impl Initial {
    /// An Initial of `version` to `dcid`, packet number `number`, sealed
    /// with the keys of `dcid` and carrying the shortest packet number that
    /// holds `number`.
    pub fn new(version: u32, dcid: &[u8], number: u64, frames: Vec<Frame>) -> Initial {
        let number_len = match number {
            0..=0xff => 1,
            0x100..=0xffff => 2,
            0x1_0000..=0xff_ffff => 3,
            _ => 4,
        };
        Initial {
            version,
            dcid: dcid.to_vec(),
            keys_from: dcid.to_vec(),
            number,
            number_len,
            frames,
        }
    }

    /// The packet, sealed and protected: a long header with this version's
    /// Initial type and an empty token, the frames as the plaintext, and
    /// `size` bytes in all — the rest PADDING — or, when `size` is `None`,
    /// just the frames padded to the least that header protection needs.
    /// Panics where the frames do not fit.
    pub fn seal(&self, size: Option<usize>) -> Vec<u8> {
        assert!(
            (1..=4).contains(&self.number_len),
            "one to four bytes of packet number, not {}",
            self.number_len
        );
        let quic = crate::quic::initial::quic_version(self.version).unwrap_or_else(|| {
            panic!(
                "not a version whose Initial keys we have: {:#x}",
                self.version
            )
        });
        let kind: u8 = match quic {
            rustls::quic::Version::V2 => 0b01,
            _ => 0b00,
        };
        let mut plaintext = Vec::new();
        for frame in &self.frames {
            frame.encode(&mut plaintext);
        }
        // header protection samples the ciphertext from four bytes past the
        // packet number's start (RFC 9001 §5.4.2), so the plaintext is at
        // least what is left of those four bytes after the packet number
        let least = 4usize.saturating_sub(self.number_len);
        plaintext.resize(plaintext.len().max(least), 0);

        // the header up to the packet number: the first byte, the version,
        // the Destination Connection ID's length and itself, the source
        // connection ID's length, the token's length, the two-byte Length
        let header_len = 1 + 4 + 1 + self.dcid.len() + 1 + 1 + 2;
        if let Some(size) = size {
            let room = size
                .checked_sub(header_len + self.number_len + 16)
                .unwrap_or_else(|| {
                    panic!(
                        "a {size}-byte packet has no room for a {header_len}-byte header, \
                         {} bytes of packet number and the tag",
                        self.number_len
                    )
                });
            if plaintext.len() > room {
                panic!(
                    "the frames take {} bytes, {} more than a {size}-byte packet has room for",
                    plaintext.len(),
                    plaintext.len() - room
                );
            }
            plaintext.resize(room, 0);
        }

        let length = self.number_len + plaintext.len() + 16;
        if length > 0x3fff {
            panic!("a Length of {length} bytes does not fit two bytes");
        }
        let length = length as u16;

        let mut datagram = Vec::with_capacity(header_len + self.number_len + plaintext.len() + 16);
        datagram.push(
            0x80 | 0x40
                | (kind << 4)
                | u8::try_from(self.number_len - 1).expect("one to four bytes of packet number"),
        );
        datagram.extend_from_slice(&self.version.to_be_bytes());
        datagram.push(u8::try_from(self.dcid.len()).expect("a connection ID of at most 255 bytes"));
        datagram.extend_from_slice(&self.dcid);
        datagram.push(0); // source connection ID length
        datagram.push(0); // token length
        datagram.extend_from_slice(&(0x4000u16 | length).to_be_bytes());
        let pn_offset = datagram.len();
        datagram.extend_from_slice(&self.number.to_be_bytes()[8 - self.number_len..]);
        datagram.extend_from_slice(&plaintext);

        let keys = crate::quic::initial::initial_suite().keys(&self.keys_from, Side::Client, quic);
        let (header, payload) = datagram.split_at_mut(pn_offset + self.number_len);
        let tag = keys
            .local
            .packet
            .encrypt_in_place(self.number, header, payload)
            .expect("a payload the AEAD takes");
        datagram.extend_from_slice(tag.as_ref());

        // header protection: the sample is the ciphertext at the packet
        // number offset plus four, and it masks the first byte's low bits and
        // the packet number's bytes (RFC 9001 §5.4)
        let sample: [u8; 16] = datagram[pn_offset + 4..pn_offset + 4 + 16]
            .try_into()
            .expect("the sample");
        let mut first = datagram[0];
        let mut number = [0u8; 4];
        number.copy_from_slice(&datagram[pn_offset..pn_offset + 4]);
        keys.local
            .header
            .encrypt_in_place(&sample, &mut first, &mut number)
            .expect("a 16-byte sample");
        datagram[0] = first;
        // only the packet number's own bytes are protected: what follows is
        // ciphertext that the masking must not touch
        datagram[pn_offset..pn_offset + self.number_len]
            .copy_from_slice(&number[..self.number_len]);
        datagram
    }
}

/// A long-header packet that is not sealed: `kind` in bits 4 and 5 of the
/// first byte (the caller gives the version's numbering), no token, a
/// two-byte Length of `payload`, then `payload` bytes of 0x5a. A reader
/// skips it by its Length.
pub fn long_packet(version: u32, kind: u8, dcid: &[u8], payload: usize) -> Vec<u8> {
    let length = u16::try_from(payload).expect("a payload of at most 65535 bytes");
    let mut packet = Vec::with_capacity(payload + 16);
    packet.push(0x80 | 0x40 | ((kind & 0b11) << 4));
    packet.extend_from_slice(&version.to_be_bytes());
    packet.push(u8::try_from(dcid.len()).expect("a connection ID of at most 255 bytes"));
    packet.extend_from_slice(dcid);
    packet.push(0); // source connection ID length
    packet.extend_from_slice(&(0x4000u16 | length).to_be_bytes());
    packet.resize(packet.len() + payload, 0x5a);
    packet
}

/// The packets joined into one datagram, padded with zeros where they are
/// shorter than `pad_to` bytes (RFC 9000 §12.2).
pub fn datagram(packets: &[Vec<u8>], pad_to: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(pad_to);
    for packet in packets {
        out.extend_from_slice(packet);
    }
    out.resize(out.len().max(pad_to), 0);
    out
}

/// The ClientHello a rustls client writes for one QUIC connection: one
/// handshake message, with no handshake made. `server_name` becomes the
/// Server Name, or, where it is `None`, an address, which rustls leaves out
/// of the Server Name extension. The transport parameters carry one
/// `initial_max_data` of one mebibyte (RFC 9000 §18.2); rustls writes them
/// without checking them.
pub fn client_hello(server_name: Option<&str>, alpn: &[&[u8]]) -> Vec<u8> {
    let config = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("TLS 1.3")
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let name = match server_name {
        Some(name) => {
            rustls::pki_types::ServerName::try_from(name.to_string()).expect("a server name")
        }
        None => rustls::pki_types::ServerName::IpAddress(
            std::net::IpAddr::from(std::net::Ipv4Addr::LOCALHOST).into(),
        ),
    };
    let alpn: Vec<Vec<u8>> = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    let mut connection = rustls::quic::ClientConnection::new_with_alpn(
        Arc::new(config),
        rustls::quic::Version::V1,
        name,
        vec![0x04, 0x04, 0x80, 0x10, 0x00, 0x00],
        alpn,
    )
    .expect("a QUIC client");
    let mut hello = Vec::new();
    connection.write_hs(&mut hello);
    hello
}

/// A ClientHello too big for one 1200-byte datagram and small enough for
/// the two `hello_initials` splits it into (phase 4 / M5a plan P3 6, and the
/// coordinator's ruling in that plan's execution record row 9): the ALPN
/// list is long enough to take it there.
pub fn big_client_hello(server_name: Option<&str>) -> Vec<u8> {
    /// The number of ALPN names that puts a hello with a Server Name and one
    /// without one both between 1550 and 1700 bytes: each name adds nineteen
    /// bytes, so 74 names are 1601 bytes with a Server Name and 1583
    /// without, and `hello_initials`' second packet (1001 bytes from offset
    /// 600) still fits its 1200.
    const NAMES: usize = 74;
    let names: Vec<String> = (0..NAMES)
        .map(|at| format!("rurge-test-alpn-{at:02}"))
        .collect();
    let alpn: Vec<&[u8]> = names.iter().map(|name| name.as_bytes()).collect();
    client_hello(server_name, &alpn)
}

/// One datagram carrying the whole ClientHello from offset zero, sealed to
/// 1200 bytes.
pub fn hello_initial(dcid: &[u8], hello: &[u8]) -> Vec<u8> {
    let frames = vec![Frame::Crypto {
        offset: 0,
        data: hello.to_vec(),
    }];
    Initial::new(1, dcid, 0, frames).seal(Some(1200))
}

/// Two datagrams: the first carries the ClientHello's bytes from zero up to
/// `at` in packet 0, the second the rest from `at` in packet 1, each sealed
/// to 1200 bytes.
pub fn hello_initials(dcid: &[u8], hello: &[u8], at: usize) -> [Vec<u8>; 2] {
    let offset = u64::try_from(at).expect("an offset of at most 64 KiB");
    let first = vec![Frame::Crypto {
        offset: 0,
        data: hello[..at].to_vec(),
    }];
    let second = vec![Frame::Crypto {
        offset,
        data: hello[at..].to_vec(),
    }];
    [
        Initial::new(1, dcid, 0, first).seal(Some(1200)),
        Initial::new(1, dcid, 1, second).seal(Some(1200)),
    ]
}

/// A SplitMix64 generator (Steele, Lea and Flood, 2014): a seed in, the same
/// numbers out, which is all these tests need. Nothing a secret depends on
/// uses it.
#[derive(Clone, Debug)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> SplitMix64 {
        SplitMix64 { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number below `n`, which must not be zero. The bias of taking a
    /// remainder is nothing these tests mind.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % u64::try_from(n).expect("a count of at least one")) as usize
    }
}

/// The pieces of a ClientHello broken up as a browser breaks them: each cut
/// at random places into two to ten more CRYPTO frames than there were
/// pieces, with several PINGs and a little PADDING among them, all in a
/// random order (QUICHE's `quic_chaos_protector.cc`, taken as fact: phase 4
/// M5 design 4.2). Which bytes a frame carries stays the same.
pub fn chaos_frames(pieces: &[(u64, &[u8])], seed: u64) -> Vec<Frame> {
    let mut rng = SplitMix64::new(seed);
    // each piece as the frames it is cut into: an offset and a length each
    let mut segments: Vec<Vec<(u64, usize)>> = pieces
        .iter()
        .map(|(offset, data)| vec![(*offset, data.len())])
        .collect();

    let mut extra = 2 + rng.below(9);
    while extra > 0 {
        let cuttable: Vec<(usize, usize)> = segments
            .iter()
            .enumerate()
            .flat_map(|(piece, frames)| {
                frames
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, len))| *len > 1)
                    .map(move |(frame, _)| (piece, frame))
            })
            .collect();
        // a piece of one byte per frame cannot be cut again: fewer frames
        // than asked for, as a browser would send them
        let Some(&(piece, frame)) = cuttable.get(rng.below(cuttable.len().max(1))) else {
            break;
        };
        let (offset, len) = segments[piece][frame];
        let cut = offset + 1 + rng.below(len - 1) as u64;
        segments[piece][frame] = (offset, usize::try_from(cut - offset).expect("a cut"));
        segments[piece].insert(
            frame + 1,
            (
                cut,
                usize::try_from(offset + len as u64 - cut).expect("a cut"),
            ),
        );
        extra -= 1;
    }

    let mut frames = Vec::new();
    for (piece, (offset, data)) in pieces.iter().enumerate() {
        for (at, len) in &segments[piece] {
            let from = usize::try_from(at - offset).expect("an offset inside its piece");
            frames.push(Frame::Crypto {
                offset: *at,
                data: data[from..from + len].to_vec(),
            });
        }
    }
    for _ in 0..2 + rng.below(9) {
        frames.push(Frame::Ping);
    }
    for _ in 0..1 + rng.below(4) {
        frames.push(Frame::Padding(1 + rng.below(8)));
    }
    // Fisher and Yates: every arrangement as likely as another
    for at in (1..frames.len()).rev() {
        frames.swap(at, rng.below(at + 1));
    }
    frames
}

/// What Chrome sends for one ClientHello in two packets (QUICHE's
/// `MultiPacketChaosProtect`, taken as fact: phase 4 M5 design 4.2): the
/// first carries the start and the end of the message, the second the
/// middle.
pub fn chrome_frames(hello: &[u8], seed: u64) -> [Vec<Frame>; 2] {
    let mut rng = SplitMix64::new(seed);
    let len = hello.len();
    // the start the first packet carries: 55 to 86 bytes
    let k = 55 + rng.below(32);
    // and the end: what leaves the middle room in one 1200-byte datagram,
    // and up to 64 bytes more
    let e = (len - k).saturating_sub(1000) + rng.below(65);
    let first = chaos_frames(
        &[(0, &hello[..k]), (len as u64 - e as u64, &hello[len - e..])],
        seed,
    );
    let second = chaos_frames(&[(k as u64, &hello[k..len - e])], seed ^ 1);
    [first, second]
}

// The two RFCs' sample Initial packets, moved here from `initial.rs`'s
// tests so that other tests can seal and read packets of their own
// (phase 4 / M5a plan P3 9).

/// The Destination Connection ID both RFCs' examples use.
pub const RFC_DCID: &[u8] = &[0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];

fn hex_digit(digit: u8) -> u8 {
    match digit {
        b'0'..=b'9' => digit - b'0',
        b'a'..=b'f' => digit - b'a' + 10,
        b'A'..=b'F' => digit - b'A' + 10,
        _ => panic!("not a hexadecimal digit: {}", char::from(digit)),
    }
}

/// The bytes of a hexadecimal string, whitespace skipped.
pub fn unhex(hex: &str) -> Vec<u8> {
    let digits: Vec<u8> = hex
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();
    assert_eq!(digits.len() % 2, 0, "an even number of digits");
    digits
        .chunks(2)
        .map(|pair| (hex_digit(pair[0]) << 4) | hex_digit(pair[1]))
        .collect()
}

// RFC 9001 A.2, SHA-256 of the 1200 bytes:
// 73fa0210cb4a5a17dc10b9dc98e5cc359ba1c20fe7c0e93a9e1dcb473c37e097
pub const RFC9001_CLIENT_INITIAL: &str = concat!(
    "c000000001088394c8f03e5157080000449e7b9aec34d1b1c98dd7689fb8ec11",
    "d242b123dc9bd8bab936b47d92ec356c0bab7df5976d27cd449f63300099f399",
    "1c260ec4c60d17b31f8429157bb35a1282a643a8d2262cad67500cadb8e7378c",
    "8eb7539ec4d4905fed1bee1fc8aafba17c750e2c7ace01e6005f80fcb7df6212",
    "30c83711b39343fa028cea7f7fb5ff89eac2308249a02252155e2347b63d58c5",
    "457afd84d05dfffdb20392844ae812154682e9cf012f9021a6f0be17ddd0c208",
    "4dce25ff9b06cde535d0f920a2db1bf362c23e596d11a4f5a6cf3948838a3aec",
    "4e15daf8500a6ef69ec4e3feb6b1d98e610ac8b7ec3faf6ad760b7bad1db4ba3",
    "485e8a94dc250ae3fdb41ed15fb6a8e5eba0fc3dd60bc8e30c5c4287e53805db",
    "059ae0648db2f64264ed5e39be2e20d82df566da8dd5998ccabdae053060ae6c",
    "7b4378e846d29f37ed7b4ea9ec5d82e7961b7f25a9323851f681d582363aa5f8",
    "9937f5a67258bf63ad6f1a0b1d96dbd4faddfcefc5266ba6611722395c906556",
    "be52afe3f565636ad1b17d508b73d8743eeb524be22b3dcbc2c7468d54119c74",
    "68449a13d8e3b95811a198f3491de3e7fe942b330407abf82a4ed7c1b311663a",
    "c69890f4157015853d91e923037c227a33cdd5ec281ca3f79c44546b9d90ca00",
    "f064c99e3dd97911d39fe9c5d0b23a229a234cb36186c4819e8b9c5927726632",
    "291d6a418211cc2962e20fe47feb3edf330f2c603a9d48c0fcb5699dbfe58964",
    "25c5bac4aee82e57a85aaf4e2513e4f05796b07ba2ee47d80506f8d2c25e50fd",
    "14de71e6c418559302f939b0e1abd576f279c4b2e0feb85c1f28ff18f58891ff",
    "ef132eef2fa09346aee33c28eb130ff28f5b766953334113211996d20011a198",
    "e3fc433f9f2541010ae17c1bf202580f6047472fb36857fe843b19f5984009dd",
    "c324044e847a4f4a0ab34f719595de37252d6235365e9b84392b061085349d73",
    "203a4a13e96f5432ec0fd4a1ee65accdd5e3904df54c1da510b0ff20dcc0c77f",
    "cb2c0e0eb605cb0504db87632cf3d8b4dae6e705769d1de354270123cb11450e",
    "fc60ac47683d7b8d0f811365565fd98c4c8eb936bcab8d069fc33bd801b03ade",
    "a2e1fbc5aa463d08ca19896d2bf59a071b851e6c239052172f296bfb5e724047",
    "90a2181014f3b94a4e97d117b438130368cc39dbb2d198065ae3986547926cd2",
    "162f40a29f0c3c8745c0f50fba3852e566d44575c29d39a03f0cda721984b6f4",
    "40591f355e12d439ff150aab7613499dbd49adabc8676eef023b15b65bfc5ca0",
    "6948109f23f350db82123535eb8a7433bdabcb909271a6ecbcb58b936a88cd4e",
    "8f2e6ff5800175f113253d8fa9ca8885c2f552e657dc603f252e1a8e308f76f0",
    "be79e2fb8f5d5fbbe2e30ecadd220723c8c0aea8078cdfcb3868263ff8f09400",
    "54da48781893a7e49ad5aff4af300cd804a6b6279ab3ff3afb64491c85194aab",
    "760d58a606654f9f4400e8b38591356fbf6425aca26dc85244259ff2b19c41b9",
    "f96f3ca9ec1dde434da7d2d392b905ddf3d1f9af93d1af5950bd493f5aa731b4",
    "056df31bd267b6b90a079831aaf579be0a39013137aac6d404f518cfd4684064",
    "7e78bfe706ca4cf5e9c5453e9f7cfd2b8b4c8d169a44e55c88d4a9a7f9474241",
    "e221af44860018ab0856972e194cd934",
);

// RFC 9369 A.2, SHA-256 of the 1200 bytes:
// b0b766fe7e762577072d3365cc4f9635809210cba301a67f8457f5e6fdac5c68
pub const RFC9369_CLIENT_INITIAL: &str = concat!(
    "d76b3343cf088394c8f03e5157080000449ea0c95e82ffe67b6abcdb4298b485",
    "dd04de806071bf03dceebfa162e75d6c96058bdbfb127cdfcbf903388e99ad04",
    "9f9a3dd4425ae4d0992cfff18ecf0fdb5a842d09747052f17ac2053d21f57c5d",
    "250f2c4f0e0202b70785b7946e992e58a59ac52dea6774d4f03b55545243cf1a",
    "12834e3f249a78d395e0d18f4d766004f1a2674802a747eaa901c3f10cda5500",
    "cb9122faa9f1df66c392079a1b40f0de1c6054196a11cbea40afb6ef5253cd68",
    "18f6625efce3b6def6ba7e4b37a40f7732e093daa7d52190935b8da58976ff33",
    "12ae50b187c1433c0f028edcc4c2838b6a9bfc226ca4b4530e7a4ccee1bfa2a3",
    "d396ae5a3fb512384b2fdd851f784a65e03f2c4fbe11a53c7777c023462239dd",
    "6f7521a3f6c7d5dd3ec9b3f233773d4b46d23cc375eb198c63301c21801f6520",
    "bcfb7966fc49b393f0061d974a2706df8c4a9449f11d7f3d2dcbb90c6b877045",
    "636e7c0c0fe4eb0f697545460c806910d2c355f1d253bc9d2452aaa549e27a1f",
    "ac7cf4ed77f322e8fa894b6a83810a34b361901751a6f5eb65a0326e07de7c12",
    "16ccce2d0193f958bb3850a833f7ae432b65bc5a53975c155aa4bcb4f7b2c4e5",
    "4df16efaf6ddea94e2c50b4cd1dfe06017e0e9d02900cffe1935e0491d77ffb4",
    "fdf85290fdd893d577b1131a610ef6a5c32b2ee0293617a37cbb08b847741c3b",
    "8017c25ca9052ca1079d8b78aebd47876d330a30f6a8c6d61dd1ab5589329de7",
    "14d19d61370f8149748c72f132f0fc99f34d766c6938597040d8f9e2bb522ff9",
    "9c63a344d6a2ae8aa8e51b7b90a4a806105fcbca31506c446151adfeceb51b91",
    "abfe43960977c87471cf9ad4074d30e10d6a7f03c63bd5d4317f68ff325ba3bd",
    "80bf4dc8b52a0ba031758022eb025cdd770b44d6d6cf0670f4e990b22347a7db",
    "848265e3e5eb72dfe8299ad7481a408322cac55786e52f633b2fb6b614eaed18",
    "d703dd84045a274ae8bfa73379661388d6991fe39b0d93debb41700b41f90a15",
    "c4d526250235ddcd6776fc77bc97e7a417ebcb31600d01e57f32162a8560cacc",
    "7e27a096d37a1a86952ec71bd89a3e9a30a2a26162984d7740f81193e8238e61",
    "f6b5b984d4d3dfa033c1bb7e4f0037febf406d91c0dccf32acf423cfa1e70710",
    "10d3f270121b493ce85054ef58bada42310138fe081adb04e2bd901f2f13458b",
    "3d6758158197107c14ebb193230cd1157380aa79cae1374a7c1e5bbcb80ee23e",
    "06ebfde206bfb0fcbc0edc4ebec309661bdd908d532eb0c6adc38b7ca7331dce",
    "8dfce39ab71e7c32d318d136b6100671a1ae6a6600e3899f31f0eed19e3417d1",
    "34b90c9058f8632c798d4490da4987307cba922d61c39805d072b589bd52fdf1",
    "e86215c2d54e6670e07383a27bbffb5addf47d66aa85a0c6f9f32e59d85a44dd",
    "5d3b22dc2be80919b490437ae4f36a0ae55edf1d0b5cb4e9a3ecabee93dfc6e3",
    "8d209d0fa6536d27a5d6fbb17641cde27525d61093f1b28072d111b2b4ae5f89",
    "d5974ee12e5cf7d5da4d6a31123041f33e61407e76cffcdcfd7e19ba58cf4b53",
    "6f4c4938ae79324dc402894b44faf8afbab35282ab659d13c93f70412e85cb19",
    "9a37ddec600545473cfb5a05e08d0b209973b2172b4d21fb69745a262ccde96b",
    "a18b2faa745b6fe189cf772a9f84cbfc",
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quic::initial::{initial_suite, quic_version};
    use rustls::Side;

    /// The plaintext room a 1200-byte Initial leaves when its Destination
    /// Connection ID is `RFC_DCID`'s eight bytes and its packet number one
    /// byte: the header up to the packet number, the packet number and the
    /// tag (phase 4 / M5a plan P3 2).
    const ROOM: usize = 1200 - 18 - 1 - 16;

    /// A variable-length integer (RFC 9000 §16) read by hand here, so that
    /// these tests do not lean on the reader they check.
    fn varint_at(buf: &[u8], at: usize) -> Option<(u64, usize)> {
        let first = *buf.get(at)?;
        let len = 1usize << (first >> 6);
        let bytes = buf.get(at..at + len)?;
        let value = bytes[1..]
            .iter()
            .fold(u64::from(first & 0x3f), |value, byte| {
                (value << 8) | u64::from(*byte)
            });
        Some((value, len))
    }

    /// The packet number and the plaintext of a sealed Initial packet: the
    /// header read, header protection taken off and the payload decrypted
    /// with `keys_from`'s keys, all by hand. This is an independent check on
    /// `Initial::seal`, which is why it uses neither `header` nor `opens`:
    /// `None` where the packet does not open.
    fn unseal(packet: &[u8], keys_from: &[u8]) -> Option<(u64, Vec<u8>)> {
        let version = quic_version(u32::from_be_bytes(packet.get(1..5)?.try_into().ok()?))?;
        let dcid_len = usize::from(*packet.get(5)?);
        let mut at = 6 + dcid_len;
        let scid_len = usize::from(*packet.get(at)?);
        at += 1 + scid_len;
        let (token_len, used) = varint_at(packet, at)?;
        at += used + usize::try_from(token_len).ok()?;
        let (length, used) = varint_at(packet, at)?;
        let pn_offset = at + used;
        let body = packet.get(pn_offset..pn_offset + usize::try_from(length).ok()?)?;

        let keys = initial_suite().keys(keys_from, Side::Server, version);
        // the sample is the ciphertext at the packet number offset plus four
        // (RFC 9001 §5.4.2)
        let sample: [u8; 16] = body.get(4..20)?.try_into().ok()?;
        let mut first = *packet.first()?;
        let mut number = [0u8; 4];
        number.copy_from_slice(body.get(..4)?);
        keys.remote
            .header
            .decrypt_in_place(&sample, &mut first, &mut number)
            .ok()?;
        let number_len = usize::from(first & 0x03) + 1;
        let mut aad = packet[..pn_offset].to_vec();
        aad[0] = first;
        aad.extend_from_slice(&number[..number_len]);
        let mut payload = body[number_len..].to_vec();
        let number = number[..number_len]
            .iter()
            .fold(0u64, |number, byte| (number << 8) | u64::from(*byte));
        let plain = keys
            .remote
            .packet
            .decrypt_in_place(number, &aad, &mut payload)
            .ok()?
            .to_vec();
        Some((number, plain))
    }

    /// The frames of these tests' own packets: PADDING, PING and CRYPTO,
    /// which are the three they write. Any other type is a bug in a test, so
    /// it panics.
    fn read_frames(plaintext: &[u8]) -> Vec<Frame> {
        let mut frames = Vec::new();
        let mut at = 0;
        while at < plaintext.len() {
            match plaintext[at] {
                // a run of zeros is one PADDING frame or several: one is
                // enough to read them back
                0x00 => {
                    let start = at;
                    while plaintext.get(at) == Some(&0x00) {
                        at += 1;
                    }
                    frames.push(Frame::Padding(at - start));
                }
                0x01 => {
                    frames.push(Frame::Ping);
                    at += 1;
                }
                0x06 => {
                    at += 1;
                    let (offset, used) = varint_at(plaintext, at).expect("a CRYPTO offset");
                    at += used;
                    let (len, used) = varint_at(plaintext, at).expect("a CRYPTO length");
                    at += used;
                    let len = usize::try_from(len).expect("a CRYPTO length this test writes");
                    let data = plaintext[at..at + len].to_vec();
                    at += len;
                    frames.push(Frame::Crypto { offset, data });
                }
                other => panic!("a frame these tests do not write: {other:#x}"),
            }
        }
        frames
    }

    /// Only the CRYPTO frames, in the order they were written.
    fn crypto(frames: &[Frame]) -> Vec<(u64, Vec<u8>)> {
        frames
            .iter()
            .filter_map(|frame| match frame {
                Frame::Crypto { offset, data } => Some((*offset, data.clone())),
                _ => None,
            })
            .collect()
    }

    /// The Ping frames' count.
    fn pings(frames: &[Frame]) -> usize {
        frames
            .iter()
            .filter(|frame| matches!(frame, Frame::Ping))
            .count()
    }

    /// Where each byte of a message of `len` bytes came from: a CRYPTO
    /// frame's, or nothing where no frame carried it. Panics where two
    /// frames carry the same byte.
    fn assemble(len: usize, crypto: &[(u64, Vec<u8>)]) -> Vec<Option<u8>> {
        let mut out = vec![None; len];
        for (offset, data) in crypto {
            let offset = usize::try_from(*offset).expect("an offset these tests write");
            for (i, byte) in data.iter().enumerate() {
                assert!(out[offset + i].is_none(), "byte {} came twice", offset + i);
                out[offset + i] = Some(*byte);
            }
        }
        out
    }

    /// A 1200-byte datagram holding one Initial of packet number `number`
    /// and `frames`. The frames have to fit the room such a packet leaves,
    /// and `what` (the seed, where a test has one) rides along in the
    /// message.
    fn sealed(frames: &[Frame], number: u64, what: &str) -> Vec<u8> {
        let mut plain = Vec::new();
        for frame in frames {
            frame.encode(&mut plain);
        }
        assert!(
            plain.len() <= ROOM,
            "{what}: {} bytes of frames, room for {ROOM}",
            plain.len()
        );
        Initial::new(1, RFC_DCID, number, frames.to_vec()).seal(Some(1200))
    }

    /// Every frame `encode` writes reads back as written, in three versions
    /// and with one to four bytes of packet number.
    #[test]
    fn every_frame_reads_back_as_written() {
        let frames = vec![
            Frame::Ack {
                largest: 5,
                delay: 3,
                first_range: 2,
                ranges: vec![(1, 2)],
                ecn: Some([1, 2, 3]),
            },
            Frame::Ping,
            Frame::Padding(3),
            Frame::Crypto {
                offset: 5,
                data: b"hello".to_vec(),
            },
            Frame::ConnectionClose {
                error: 0x0a,
                frame_type: 0x06,
                reason: b"bye".to_vec(),
            },
        ];
        let mut written = Vec::new();
        for frame in &frames {
            frame.encode(&mut written);
        }

        for version in [1u32, 0x6b33_43cf, 0xff00_001d] {
            for (number_len, number) in [
                (1usize, 0x12u64),
                (2, 0x1234),
                (3, 0x12_3456),
                (4, 0x1234_5678),
            ] {
                let mut initial = Initial::new(version, RFC_DCID, number, frames.clone());
                initial.number_len = number_len;
                let what = format!("{version:#x}, {number_len} bytes of packet number");

                // no room to spare: the plaintext is the frames themselves
                let packet = initial.seal(None);
                let (read, plaintext) = unseal(&packet, RFC_DCID).expect("a packet of our own");
                assert_eq!(read, number, "{what}");
                assert!(
                    plaintext.starts_with(&written),
                    "{what}: the frames as written"
                );

                // sealed into 1200 bytes: the rest is PADDING
                let packet = initial.seal(Some(1200));
                assert_eq!(packet.len(), 1200, "{what}");
                let (read, plaintext) = unseal(&packet, RFC_DCID).expect("a packet of our own");
                assert_eq!(read, number, "{what}");
                assert!(
                    plaintext.starts_with(&written),
                    "{what}: the frames as written"
                );
                assert!(
                    plaintext[written.len()..].iter().all(|byte| *byte == 0),
                    "{what}: the rest is PADDING"
                );
            }
        }
    }

    /// A packet's keys come from `keys_from`, not from its Destination
    /// Connection ID: only the keys it was sealed with open it.
    #[test]
    fn a_packet_sealed_with_other_keys_opens_only_with_them() {
        let keys_from = [0xaa; 8];
        let mut initial = Initial::new(1, RFC_DCID, 0, vec![Frame::Ping]);
        initial.keys_from = keys_from.to_vec();
        let packet = initial.seal(Some(1200));
        assert!(
            unseal(&packet, RFC_DCID).is_none(),
            "the connection ID's keys"
        );
        assert!(
            unseal(&packet, &keys_from).is_some(),
            "the keys it was sealed with"
        );
    }

    /// What rustls writes is one handshake message of type 1, with the
    /// Server Name in it when there is one.
    #[test]
    fn the_hello_rustls_writes_is_one_client_hello() {
        for name in [Some("held.test"), None] {
            let hello = client_hello(name, &[b"h3"]);
            assert_eq!(hello[0], 1, "{name:?}: the ClientHello's type");
            let len = usize::try_from(u32::from_be_bytes([0, hello[1], hello[2], hello[3]]))
                .expect("a length of a message this test builds");
            assert_eq!(len + 4, hello.len(), "{name:?}: one handshake message");
            assert_eq!(
                hello.windows(9).any(|window| window == b"held.test"),
                name.is_some(),
                "{name:?}: the Server Name"
            );
        }
    }

    /// The big ClientHello does not fit one 1200-byte datagram, and does fit
    /// the two `hello_initials` splits it into.
    #[test]
    fn a_big_hello_needs_two_packets() {
        let hello = big_client_hello(Some("held.test"));
        assert!(
            (1500..=1900).contains(&hello.len()),
            "{} bytes, and a 1200-byte datagram holds about 1160",
            hello.len()
        );
        assert!(hello.windows(9).any(|window| window == b"held.test"));
    }

    /// Two packets carry the two halves of a ClientHello, numbered 0 and 1,
    /// each datagram 1200 bytes long.
    #[test]
    fn hello_initials_carry_the_two_halves() {
        let hello = big_client_hello(Some("held.test"));
        let [first, second] = hello_initials(RFC_DCID, &hello, 600);
        assert_eq!(first.len(), 1200);
        assert_eq!(second.len(), 1200);

        let (number, plaintext) = unseal(&first, RFC_DCID).expect("the first packet");
        assert_eq!(number, 0);
        assert_eq!(
            crypto(&read_frames(&plaintext)),
            [(0, hello[..600].to_vec())]
        );

        let (number, plaintext) = unseal(&second, RFC_DCID).expect("the second packet");
        assert_eq!(number, 1);
        assert_eq!(
            crypto(&read_frames(&plaintext)),
            [(600, hello[600..].to_vec())]
        );
    }

    /// What a browser does: the first packet holds the start and the end of
    /// the ClientHello, the second the middle, each with several CRYPTO
    /// frames, PINGs and PADDING; both fit a 1200-byte datagram; together
    /// they are the ClientHello byte for byte.
    #[test]
    fn chrome_frames_put_the_start_and_the_end_first() {
        let hello = big_client_hello(Some("held.test"));
        let len = hello.len();
        for seed in 0..32u64 {
            let [first, second] = chrome_frames(&hello, seed);
            assert!(
                pings(&first) >= 1,
                "seed {seed}: the first packet has a PING"
            );
            assert!(
                pings(&second) >= 1,
                "seed {seed}: the second packet has a PING"
            );
            let one = sealed(&first, 0, &format!("seed {seed}, the first packet"));
            let two = sealed(&second, 1, &format!("seed {seed}, the second packet"));

            let (number, plaintext) = unseal(&one, RFC_DCID).expect("the first packet");
            assert_eq!(number, 0, "seed {seed}");
            let start = assemble(len, &crypto(&read_frames(&plaintext)));
            let (number, plaintext) = unseal(&two, RFC_DCID).expect("the second packet");
            assert_eq!(number, 1, "seed {seed}");
            let middle = assemble(len, &crypto(&read_frames(&plaintext)));

            // the first packet's bytes are a prefix and a suffix
            let k = start.iter().position(Option::is_none).unwrap_or(len);
            let e = start
                .iter()
                .rposition(Option::is_none)
                .map_or(0, |at| len - at - 1);
            assert!((55..=86).contains(&k), "seed {seed}: k = {k}");
            assert!(
                start[k..len - e].iter().all(Option::is_none),
                "seed {seed}: the middle is the second packet's"
            );
            assert!(
                start[..k]
                    .iter()
                    .chain(&start[len - e..])
                    .all(Option::is_some),
                "seed {seed}: the start and the end are the first packet's"
            );
            assert!(
                middle[k..len - e].iter().all(Option::is_some),
                "seed {seed}: the middle is the second packet's"
            );
            assert!(
                middle[..k]
                    .iter()
                    .chain(&middle[len - e..])
                    .all(Option::is_none),
                "seed {seed}: the start and the end are the first packet's"
            );
            for (at, byte) in hello.iter().enumerate() {
                assert_eq!(
                    start[at].or(middle[at]),
                    Some(*byte),
                    "seed {seed}, byte {at}"
                );
            }
        }
    }

    /// `chaos_frames` covers its pieces byte for byte, in more CRYPTO frames
    /// than there were pieces, with several PINGs. The asserion messages
    /// carry the seed.
    #[test]
    fn chaos_frames_cover_their_pieces() {
        let hello = big_client_hello(Some("held.test"));
        let pieces: [(u64, &[u8]); 2] = [(0, &hello[..500]), (500, &hello[500..])];
        for seed in 0..64u64 {
            let frames = chaos_frames(&pieces, seed);
            let cryptos = crypto(&frames).len();
            assert!(
                (pieces.len() + 2..=pieces.len() + 10).contains(&cryptos),
                "seed {seed}: {cryptos} CRYPTO frames for {} pieces",
                pieces.len()
            );
            let pings = pings(&frames);
            assert!((2..=10).contains(&pings), "seed {seed}: {pings} PINGs");

            let covered = assemble(hello.len(), &crypto(&frames));
            for (at, byte) in hello.iter().enumerate() {
                assert_eq!(covered[at], Some(*byte), "seed {seed}, byte {at}");
            }
        }
    }

    /// `datagram` joins packets and pads what is short of its length with
    /// zeros.
    #[test]
    fn datagram_pads_with_zeros() {
        let packet = Initial::new(1, RFC_DCID, 0, vec![Frame::Ping]).seal(Some(1200));
        let padded = datagram(std::slice::from_ref(&packet), 1300);
        assert_eq!(padded.len(), 1300);
        assert!(padded.starts_with(&packet));
        assert!(padded[packet.len()..].iter().all(|byte| *byte == 0));
    }

    /// An unsealed long-header packet ends where its Length says: the reader
    /// skips what it does not read by that field alone.
    #[test]
    fn a_long_packet_ends_where_its_length_says() {
        let packet = long_packet(1, 0b01, RFC_DCID, 100);
        // no token: the Length field follows the source connection ID's
        // length (RFC 9000 §17.2.2)
        let at = 7 + RFC_DCID.len();
        assert_eq!(packet[at], 0x40);
        assert_eq!(usize::from(packet[at + 1]), 100);
        assert_eq!(packet.len(), at + 2 + 100);
    }
}
