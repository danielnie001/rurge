//! A scriptable TUIC server (phase 2 M7 design 6.4) — v5, or v4 as tuic
//! 0.8.5's server speaks it when its script names a token: QUIC with ALPN
//! `h3` on a loopback port of its own (`ServerPort`: it can restart) or on
//! a socket it is given (`MultiPort` for port hopping). It checks the
//! authentication — v5's against what its own side of the TLS session
//! exports, v4's against the BLAKE3 hash of its token —, holds the commands
//! that come before it (as both protocols ask; sing-box leaves early
//! datagrams in QUIC's queue so), relays each Connect to its target —
//! answering a v4 one with whether it reached it —, and gives each UDP
//! association a loopback socket of its own (full cone), which answers the
//! way the association's first packet came: in datagrams — a v5 answer that
//! one cannot hold split into fragments of its own, a v4 one dropped — or on
//! streams. A unidirectional stream it cannot read as one whole command —
//! one cut short — closes the connection, as sing-box closes it. It can
//! hold the client's streams unread for a while, each taking no more than a
//! window of the script's meanwhile. A v4 fake does what tuic 0.8.5's
//! server does that a client must live with: it names an IPv4 source in
//! IPv6 form (its dual-stack socket), and closes the connection with the
//! protocol's codes on a stream whose command's header it cannot read
//! (0xfffffff0; a packet cut short past its header is only dropped), a
//! token it does not know (0xfffffff1), a datagram that is no packet and a
//! packet that does not come the way the connection's first came
//! (0xfffffff3). What the client sends is read by code that is not the
//! client's: the commands and their addresses by the code here, which also
//! writes the answers. It counts heartbeats and records Dissociates. It
//! never resolves a name.

use super::ServerPort;
use quinn::{
    AsyncUdpSocket, Connection, Endpoint, ReadError, ReadToEndError, RecvStream, SendDatagramError,
    SendStream, ServerConfig, TransportConfig, VarInt,
};
use rurge_net::quic::testing::{TestCert, endpoint};
use rustls::RootCertStore;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{AsyncReadExt, copy_bidirectional_with_sizes};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::task::AbortOnDropHandle;

// the wire as the fake knows it: the versions, the commands' types
const V4: u8 = 0x04;
const V5: u8 = 0x05;
const AUTHENTICATE: u8 = 0x00;
const CONNECT: u8 = 0x01;
const PACKET: u8 = 0x02;
const DISSOCIATE: u8 = 0x03;
const HEARTBEAT: u8 = 0x04;
/// v4's answer to a Connect, `04 ff ‖ REP`, and its two replies.
const RESPONSE: u8 = 0xff;
const SUCCEEDED: u8 = 0x00;
const FAILED: u8 = 0xff;
/// How a v5 fake closes a connection, and how every fake stops a stream:
/// with 0, as sing-box does.
const CLOSE: VarInt = VarInt::from_u32(0);
/// How a v4 fake closes a connection, with tuic 0.8.5's codes and reasons:
/// for a stream it cannot read, the text of the I/O error that a command
/// cut short gives.
const PROTOCOL_ERROR: (u32, &[u8]) = (0xffff_fff0, b"early eof");
const AUTHENTICATION_FAILED: (u32, &[u8]) = (0xffff_fff1, b"authentication failed");
const BAD_COMMAND: (u32, &[u8]) = (0xffff_fff3, b"bad command");
/// How much of a TCP tunnel the fake relays at a time, each way.
const RELAY: usize = 64 << 10;
/// The longest unidirectional stream it reads: past any one command.
const MAX_STREAM: usize = 128 << 10;

#[derive(Clone, Debug, Default)]
pub struct TuicScript {
    /// v5: the one user it lets in, by its UUID's 16 bytes, and its
    /// password.
    pub uuid: [u8; 16],
    pub password: String,
    /// Speak TUIC v4, as tuic 0.8.5's server, letting this token in.
    pub v4_token: Option<String>,
    /// v5: the code it closes a connection with when the authentication
    /// failed, never with a reason: 0 — as tuic-server 1.0.0 and sing-box
    /// close it — unless set. A v4 fake closes it as tuic 0.8.5 does.
    pub auth_failure_code: u32,
    /// Relay here whatever the client asked for, TCP and UDP alike (needed
    /// for a name).
    pub connect_to: Option<SocketAddr>,
    /// Never carry out a Connect, nor answer a v4 one: what the client sends
    /// on it is dropped until it gives up.
    pub silent: bool,
    /// v4: end the stream of a Connect it cannot carry out with no response,
    /// rather than with the one that says so: the client finds the stream's
    /// end where it reads the response.
    pub no_response: bool,
    /// Take no QUIC datagrams at all: the transport parameter that allows
    /// them is not sent, and the client relays UDP on streams (`quic` mode).
    pub no_datagrams: bool,
    /// The window QUIC gives each of the client's streams, in bytes: what
    /// the client sends on one goes no further ahead of what the fake read
    /// of it. quinn's (1.25 MB) unless set.
    pub stream_window: Option<u32>,
    /// v5: the first UDP answer it splits into fragments goes without this
    /// one.
    pub drop_fragment: Option<u8>,
}

impl TuicScript {
    /// A v4 server that lets `token` in.
    pub fn v4(token: &str) -> TuicScript {
        TuicScript {
            v4_token: Some(token.to_string()),
            ..TuicScript::default()
        }
    }
}

/// An Authenticate command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTuicAuth {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// v5's user; v4 names none.
    pub uuid: Option<[u8; 16]>,
    pub token: [u8; 32],
    /// The token it lets in: v5's as the fake's own side of the TLS session
    /// exports it for its user, v4's the hash of its token.
    pub expected: [u8; 32],
    pub accepted: bool,
}

/// A Connect command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedConnect {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// The target as the command named it: `name:port`, or an address and
    /// its port (IPv6 in brackets).
    pub address: String,
}

/// A Packet command from the client: a whole UDP packet, or one fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTuicPacket {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    /// v5's are 16 bits.
    pub association: u32,
    /// v4's packets have neither ids nor fragments: 0, 1 and 0.
    pub packet: u16,
    pub fragments: u8,
    pub fragment: u8,
    /// The address as the command named it, as a Connect's is recorded;
    /// `None` for none.
    pub address: Option<String>,
    /// How long its payload was: a fragment's own part, for a fragment.
    pub len: usize,
    /// It came on a unidirectional stream of its own (`quic` mode), not in
    /// a datagram.
    pub on_stream: bool,
}

/// A Dissociate command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedDissociate {
    /// The connection it came on, 0 for the first accepted.
    pub connection: usize,
    pub association: u32,
}

/// A Packet command as the fake reads and writes it, for tests that send
/// their own: v5's `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE
/// ‖ ADDR ‖ payload`, or v4's `04 02 ‖ ASSOC_ID ‖ LEN ‖ ADDR ‖ payload`, which
/// has neither a packet id nor fragments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuicPacket {
    /// v5's are 16 bits.
    pub association: u32,
    pub packet: u16,
    pub fragments: u8,
    pub fragment: u8,
    /// `host:port`, an address (IPv6 in brackets) or a name; `None` for
    /// none.
    pub address: Option<String>,
    pub payload: Vec<u8>,
}

impl TuicPacket {
    /// A UDP packet that goes whole: fragment 0 of 1.
    pub fn whole(association: u32, packet: u16, address: &str, payload: &[u8]) -> TuicPacket {
        TuicPacket {
            association,
            packet,
            fragments: 1,
            fragment: 0,
            address: Some(address.to_string()),
            payload: payload.to_vec(),
        }
    }

    /// As a v5 server writes it.
    pub fn encode(&self) -> Vec<u8> {
        let association = u16::try_from(self.association).expect("a v5 association id of 16 bits");
        let mut out = vec![V5, PACKET];
        out.extend_from_slice(&association.to_be_bytes());
        out.extend_from_slice(&self.packet.to_be_bytes());
        out.extend_from_slice(&[self.fragments, self.fragment]);
        self.put_rest(&mut out);
        out
    }

    /// As a v4 server writes it: the packet's id and fragments are not.
    pub fn encode_v4(&self) -> Vec<u8> {
        let mut out = vec![V4, PACKET];
        out.extend_from_slice(&self.association.to_be_bytes());
        self.put_rest(&mut out);
        out
    }

    /// `SIZE ‖ ADDR ‖ payload`, v4's LEN alike.
    fn put_rest(&self, out: &mut Vec<u8>) {
        let size = u16::try_from(self.payload.len()).expect("a payload of at most 65535 bytes");
        out.extend_from_slice(&size.to_be_bytes());
        match self.address.as_deref() {
            None => out.push(0xff),
            Some(address) => put_address(out, address),
        }
        out.extend_from_slice(&self.payload);
    }

    /// `None` for anything but a v5 Packet whose SIZE is just what follows
    /// its address, as sing-box reads one.
    fn decode(data: &[u8]) -> Option<TuicPacket> {
        let (header, rest) = data.split_at_checked(10)?;
        if header[..2] != [V5, PACKET] {
            return None;
        }
        let association = u16::from_be_bytes([header[2], header[3]]);
        let mut packet = TuicPacket::rest(header[8..].try_into().ok()?, rest)?;
        packet.association = u32::from(association);
        packet.packet = u16::from_be_bytes([header[4], header[5]]);
        (packet.fragments, packet.fragment) = (header[6], header[7]);
        Some(packet)
    }

    /// `None` for anything but a v4 Packet whose LEN is just what follows
    /// its address — whole, as v4 has no fragments.
    fn decode_v4(data: &[u8]) -> Option<TuicPacket> {
        let (header, rest) = data.split_at_checked(8)?;
        if header[..2] != [V4, PACKET] {
            return None;
        }
        let mut packet = TuicPacket::rest(header[6..].try_into().ok()?, rest)?;
        packet.association = u32::from_be_bytes(header[2..6].try_into().ok()?);
        Some(packet)
    }

    /// The address and the payload that follow a header ending with
    /// `size`, as a whole packet of association 0.
    fn rest(size: [u8; 2], rest: &[u8]) -> Option<TuicPacket> {
        let (address, rest) = match rest.split_first()? {
            (&0xff, rest) => (None, rest),
            _ => {
                let (address, rest) = take_address(rest)?;
                (Some(address.to_string()), rest)
            }
        };
        if rest.len() != usize::from(u16::from_be_bytes(size)) {
            return None;
        }
        Some(TuicPacket {
            association: 0,
            packet: 0,
            fragments: 1,
            fragment: 0,
            address,
            payload: rest.to_vec(),
        })
    }
}

/// `host:port` as ADDR: an address as its type and bytes, a name with its
/// length first, then the port.
fn put_address(out: &mut Vec<u8>, address: &str) {
    let port = match address.parse::<SocketAddr>() {
        Ok(addr) => {
            match addr.ip() {
                IpAddr::V4(v4) => {
                    out.push(0x01);
                    out.extend_from_slice(&v4.octets());
                }
                IpAddr::V6(v6) => {
                    out.push(0x02);
                    out.extend_from_slice(&v6.octets());
                }
            }
            addr.port()
        }
        Err(_) => {
            let (name, port) = address.rsplit_once(':').expect("host:port");
            out.push(0x00);
            out.push(u8::try_from(name.len()).expect("a name of at most 255 bytes"));
            out.extend_from_slice(name.as_bytes());
            port.parse().expect("a port")
        }
    };
    out.extend_from_slice(&port.to_be_bytes());
}

/// ADDR from its type on (SPEC "Address"), and what follows it: `None` for
/// a type it does not know, or an address cut short.
fn take_address(data: &[u8]) -> Option<(Address, &[u8])> {
    let (kind, rest) = data.split_first()?;
    let (ip, rest) = match kind {
        0x00 => {
            let (&len, rest) = rest.split_first()?;
            let (name, rest) = rest.split_at_checked(usize::from(len))?;
            let (port, rest) = rest.split_at_checked(2)?;
            let name = String::from_utf8(name.to_vec()).ok()?;
            return Some((
                Address::Name(name, u16::from_be_bytes([port[0], port[1]])),
                rest,
            ));
        }
        0x01 => {
            let (octets, rest) = rest.split_first_chunk::<4>()?;
            (IpAddr::from(*octets), rest)
        }
        0x02 => {
            let (octets, rest) = rest.split_first_chunk::<16>()?;
            (IpAddr::from(*octets), rest)
        }
        _ => return None,
    };
    let (port, rest) = rest.split_first_chunk::<2>()?;
    Some((
        Address::Ip(SocketAddr::new(ip, u16::from_be_bytes(*port))),
        rest,
    ))
}

pub struct FakeTuic {
    cert: TestCert,
    addr: SocketAddr,
    /// `None` on a socket it was given: it cannot restart.
    port: Option<ServerPort>,
    shared: Arc<Shared>,
    /// Every endpoint so far, with its accept loop: closed with the fake.
    endpoints: Mutex<Vec<(Endpoint, AbortOnDropHandle<()>)>>,
}

/// What the connections of every endpoint share.
struct Shared {
    script: TuicScript,
    /// The version it speaks, as the script says.
    version: u8,
    /// The server's side of every connection accepted, in order.
    connections: Mutex<Vec<Connection>>,
    auths: Mutex<Vec<RecordedTuicAuth>>,
    connects: Mutex<Vec<RecordedConnect>>,
    packets: Mutex<Vec<RecordedTuicPacket>>,
    dissociates: Mutex<Vec<RecordedDissociate>>,
    heartbeats: AtomicUsize,
    /// Each association's own socket, in the order they opened.
    outside: Mutex<Vec<SocketAddr>>,
    /// The fragment the next UDP answer in fragments goes without.
    drop_fragment: Mutex<Option<u8>>,
    /// Unidirectional streams that come are held unread
    /// (`FakeTuic::hold_streams`).
    holding: watch::Sender<bool>,
    /// The streams held so far.
    held: AtomicUsize,
}

impl Shared {
    fn connections(&self) -> MutexGuard<'_, Vec<Connection>> {
        self.connections.lock().expect("connections")
    }

    /// Lets a new unidirectional stream through: at once, unless the fake
    /// holds them — then once it lets them go, counted meanwhile.
    async fn let_through(&self) {
        let mut holding = self.holding.subscribe();
        if *holding.borrow() {
            self.held.fetch_add(1, Ordering::SeqCst);
            // `self` keeps the sender
            let _ = holding.wait_for(|&holding| !holding).await;
        }
    }

    /// `packet` as the fake's version writes it.
    fn encode(&self, packet: &TuicPacket) -> Vec<u8> {
        if self.version == V4 {
            packet.encode_v4()
        } else {
            packet.encode()
        }
    }

    /// A packet of the client's as the fake's version reads it.
    fn decode(&self, data: &[u8]) -> Option<TuicPacket> {
        if self.version == V4 {
            TuicPacket::decode_v4(data)
        } else {
            TuicPacket::decode(data)
        }
    }

    /// `answer` in datagrams of at most `max` bytes: whole when it fits;
    /// else a v4 answer is dropped (tuic 0.8.5's server fails to send it),
    /// and a v5 one goes in fragments of one size, only the first naming
    /// the source. The first answer split goes without the fragment the
    /// script drops.
    fn split(&self, answer: TuicPacket, max: usize) -> Vec<Vec<u8>> {
        let whole = self.encode(&answer);
        if whole.len() <= max {
            return vec![whole];
        }
        if self.version == V4 {
            return Vec::new();
        }
        let room = max - (whole.len() - answer.payload.len());
        let fragments = u8::try_from(answer.payload.len().div_ceil(room)).expect("255 fragments");
        let dropped = self.drop_fragment.lock().expect("drop").take();
        answer
            .payload
            .chunks(room)
            .zip(0..fragments)
            .filter(|&(_, fragment)| Some(fragment) != dropped)
            .map(|(chunk, fragment)| {
                TuicPacket {
                    association: answer.association,
                    packet: answer.packet,
                    fragments,
                    fragment,
                    address: if fragment == 0 {
                        answer.address.clone()
                    } else {
                        None
                    },
                    payload: chunk.to_vec(),
                }
                .encode()
            })
            .collect()
    }
}

impl FakeTuic {
    /// On a loopback port of its own, which it can take over again
    /// (`restart`).
    pub fn spawn(script: TuicScript) -> FakeTuic {
        let port = ServerPort::bind();
        let fake = FakeTuic::new(script, port.addr(), Some(port));
        fake.take_over();
        fake
    }

    /// On `socket` — a `MultiPort` for port hopping —, which it cannot
    /// take over again.
    pub fn spawn_on(script: TuicScript, socket: Arc<dyn AsyncUdpSocket>) -> FakeTuic {
        let addr = socket.local_addr().expect("the socket's address");
        let fake = FakeTuic::new(script, addr, None);
        fake.listen(endpoint(fake.server_config(), socket));
        fake
    }

    fn new(script: TuicScript, addr: SocketAddr, port: Option<ServerPort>) -> FakeTuic {
        FakeTuic {
            cert: TestCert::new(),
            addr,
            port,
            shared: Arc::new(Shared {
                version: if script.v4_token.is_some() { V4 } else { V5 },
                drop_fragment: Mutex::new(script.drop_fragment),
                script,
                connections: Mutex::default(),
                auths: Mutex::default(),
                connects: Mutex::default(),
                packets: Mutex::default(),
                dissociates: Mutex::default(),
                heartbeats: AtomicUsize::new(0),
                outside: Mutex::default(),
                holding: watch::Sender::new(false),
                held: AtomicUsize::new(0),
            }),
            endpoints: Mutex::default(),
        }
    }

    /// Its port: the first of them, on a socket of several.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Roots that trust its certificate (for `localhost` and `127.0.0.1`).
    pub fn roots(&self) -> Arc<RootCertStore> {
        self.cert.roots()
    }

    /// Every Authenticate it read, in arrival order.
    pub fn auths(&self) -> Vec<RecordedTuicAuth> {
        self.shared.auths.lock().expect("auths").clone()
    }

    /// Every Connect it read, in arrival order: before the authentication
    /// went through, as the commands that wait for it.
    pub fn connects(&self) -> Vec<RecordedConnect> {
        self.shared.connects.lock().expect("connects").clone()
    }

    /// Every Packet it read, in the order it read them: each fragment
    /// apart.
    pub fn udp_packets(&self) -> Vec<RecordedTuicPacket> {
        self.shared.packets.lock().expect("packets").clone()
    }

    /// Each association's own socket, in the order they opened: whoever
    /// sends to one is heard by the client.
    pub fn udp_outside(&self) -> Vec<SocketAddr> {
        self.shared.outside.lock().expect("outside").clone()
    }

    /// Every Dissociate it read, in arrival order.
    pub fn dissociates(&self) -> Vec<RecordedDissociate> {
        self.shared.dissociates.lock().expect("dissociates").clone()
    }

    /// The heartbeats it heard so far, on every connection: v5's in
    /// datagrams, v4's on streams.
    pub fn heartbeats(&self) -> usize {
        self.shared.heartbeats.load(Ordering::SeqCst)
    }

    /// Sends `packet`, as the fake's version writes it, to the client in a
    /// datagram of connection `n`.
    pub fn send_udp(&self, n: usize, packet: &TuicPacket) {
        self.connection(n)
            .send_datagram(self.shared.encode(packet).into())
            .expect("a datagram the client takes");
    }

    /// The connections accepted so far: QUIC's handshake done.
    pub fn connections(&self) -> usize {
        self.shared.connections().len()
    }

    /// The server's side of connection `n`, 0 for the first accepted.
    pub fn connection(&self, n: usize) -> Connection {
        self.shared.connections()[n].clone()
    }

    /// Closes every connection accepted so far — code 0, no reason —, as
    /// sing-box ends its sessions.
    pub fn close_connections(&self) {
        self.close_connections_with(0, b"");
    }

    /// Closes every connection accepted so far with `code` and `reason`.
    pub fn close_connections_with(&self, code: u32, reason: &[u8]) {
        for connection in self.shared.connections().iter() {
            connection.close(VarInt::from_u32(code), reason);
        }
    }

    /// Holds the unidirectional streams that come from now on, unread,
    /// until `release_streams`: what the client sends on one goes no
    /// further than the stream's window (`TuicScript::stream_window`)
    /// meanwhile.
    pub fn hold_streams(&self) {
        self.shared.holding.send_replace(true);
    }

    /// Reads the streams it holds, and those that come from now on.
    pub fn release_streams(&self) {
        self.shared.holding.send_replace(false);
    }

    /// The streams it held so far.
    pub fn held_streams(&self) -> usize {
        self.shared.held.load(Ordering::SeqCst)
    }

    /// A new endpoint takes the port over, as after a restart: the
    /// connections of the one before are gone, and the client learns it
    /// from a stateless reset of its next packet.
    pub fn restart(&self) {
        self.take_over();
    }

    /// QUIC with ALPN `h3`: datagrams included unless the script says
    /// none, quinn's stream window unless it sets one.
    fn server_config(&self) -> ServerConfig {
        let script = &self.shared.script;
        let mut transport = TransportConfig::default();
        if script.no_datagrams {
            transport.datagram_receive_buffer_size(None);
        }
        if let Some(window) = script.stream_window {
            transport.stream_receive_window(VarInt::from_u32(window));
        }
        let mut config = self.cert.server_config(&["h3"]);
        config.transport_config(Arc::new(transport));
        config
    }

    /// A new endpoint takes the port over; the one before falls silent.
    fn take_over(&self) {
        let port = self.port.as_ref().expect("a fake on a port of its own");
        self.listen(port.endpoint(self.server_config()));
    }

    /// Serves on `endpoint` until the fake is dropped.
    fn listen(&self, endpoint: Endpoint) {
        let accepting = tokio::spawn(accept(endpoint.clone(), self.shared.clone()));
        self.endpoints
            .lock()
            .expect("endpoints")
            .push((endpoint, AbortOnDropHandle::new(accepting)));
    }
}

impl Drop for FakeTuic {
    /// Every connection ends with the fake, and with them the tasks that
    /// serve them.
    fn drop(&mut self) {
        for (endpoint, _) in self.endpoints.get_mut().expect("endpoints").iter() {
            endpoint.close(CLOSE, b"");
        }
    }
}

/// The connections of `endpoint`, each served in a task of its own until
/// it ends.
async fn accept(endpoint: Endpoint, shared: Arc<Shared>) {
    while let Some(incoming) = endpoint.accept().await {
        let shared = shared.clone();
        tokio::spawn(async move {
            if let Ok(connection) = incoming.await {
                serve(connection, shared).await;
            }
        });
    }
}

/// One connection: its streams and its datagrams, each in a task of its
/// own that ends with the connection, as do its associations. The
/// commands wait for the authentication.
async fn serve(connection: Connection, shared: Arc<Shared>) {
    let number = {
        let mut connections = shared.connections();
        connections.push(connection.clone());
        connections.len() - 1
    };
    let authenticated = watch::Sender::new(false);
    let link = Arc::new(Link {
        connection: connection.clone(),
        number,
        associations: Mutex::default(),
        on_streams: Mutex::default(),
        shared: shared.clone(),
    });
    let mut streams = JoinSet::new();
    streams.spawn(read_datagrams(link.clone(), authenticated.subscribe()));
    loop {
        tokio::select! {
            stream = connection.accept_uni() => match stream {
                Ok(stream) => {
                    streams.spawn(serve_uni(stream, link.clone(), authenticated.clone()));
                }
                Err(_) => return,
            },
            stream = connection.accept_bi() => match stream {
                Ok((send, recv)) => {
                    streams.spawn(relay(
                        send,
                        recv,
                        number,
                        authenticated.subscribe(),
                        shared.clone(),
                    ));
                }
                Err(_) => return,
            },
            // a stream that ended; nothing to collect
            Some(_) = streams.join_next() => {}
        }
    }
}

/// A connection and its UDP associations, as its tasks share them.
struct Link {
    connection: Connection,
    /// The connection's number, 0 for the first accepted.
    number: usize,
    associations: Mutex<HashMap<u32, Association>>,
    /// v4: whether the connection's UDP comes on streams, as its first
    /// packet came — tuic 0.8.5's server holds the rest to it.
    on_streams: Mutex<Option<bool>>,
    shared: Arc<Shared>,
}

/// A unidirectional stream, read whole — the client finishes each after
/// its one command —: the Authenticate, or — once that went through — a
/// Packet (`quic` mode), a Dissociate, or a v4 heartbeat. A stream that
/// holds no whole command — one cut short — closes the connection, as
/// sing-box closes it on a stream it cannot read, and tuic 0.8.5 on one
/// whose command's header it cannot: every tunnel and association on it
/// ends. tuic 0.8.5 reads a packet's payload apart, and loses no more than
/// the packet when that is cut short. While the fake holds streams, it
/// waits unread.
async fn serve_uni(mut stream: RecvStream, link: Arc<Link>, authenticated: watch::Sender<bool>) {
    link.shared.let_through().await;
    let command = match stream.read_to_end(MAX_STREAM).await {
        Ok(command) => command,
        // nothing left to close
        Err(ReadToEndError::Read(ReadError::ConnectionLost(_))) => return,
        Err(_) => return link.unreadable(),
    };
    let version = link.shared.version;
    let kind = match command[..] {
        [v, kind, ..] if v == version => kind,
        _ => return link.unreadable(),
    };
    if kind == AUTHENTICATE {
        return authenticate(&command, &link, &authenticated);
    }
    if !matches!(kind, PACKET | DISSOCIATE | HEARTBEAT) {
        return link.unreadable();
    }
    if authenticated
        .subscribe()
        .wait_for(|&let_in| let_in)
        .await
        .is_err()
    {
        return;
    }
    match (version, kind, &command[2..]) {
        (_, PACKET, _) => match link.shared.decode(&command) {
            Some(packet) => link.relay_packet(packet, true).await,
            None if version == V4 && v4_header_whole(&command) => {}
            None => link.unreadable(),
        },
        (V5, DISSOCIATE, &[high, low]) => {
            link.dissociate(u32::from(u16::from_be_bytes([high, low])))
        }
        (V4, DISSOCIATE, &[a, b, c, d]) => link.dissociate(u32::from_be_bytes([a, b, c, d])),
        // v4's goes on a stream (v5's in a datagram)
        (V4, HEARTBEAT, []) => {
            link.shared.heartbeats.fetch_add(1, Ordering::SeqCst);
        }
        _ => link.unreadable(),
    }
}

/// Whether `command` holds a v4 Packet's header whole — `04 02 ‖ ASSOC_ID ‖
/// LEN ‖ ADDR` —, whatever follows it.
fn v4_header_whole(command: &[u8]) -> bool {
    command.get(8..).and_then(take_address).is_some()
}

/// The authentication, checked against what the fake lets in: v5's token
/// against what its own side of the TLS session exports for its user, with
/// the UUID's bytes as the label and the password as the context; v4's
/// against the BLAKE3 hash of its token. Let in, the connection's commands
/// go on; refused, the connection is closed as the reference servers close
/// it.
fn authenticate(command: &[u8], link: &Link, authenticated: &watch::Sender<bool>) {
    let script = &link.shared.script;
    let (uuid, token, expected, (code, reason)) = match &script.v4_token {
        Some(own) => {
            let Ok(token) = <[u8; 32]>::try_from(&command[2..]) else {
                return;
            };
            let expected = *blake3::hash(own.as_bytes()).as_bytes();
            (None, token, expected, AUTHENTICATION_FAILED)
        }
        None => {
            let Some((uuid, token)) = command[2..].split_first_chunk::<16>() else {
                return;
            };
            let Ok(token) = <[u8; 32]>::try_from(token) else {
                return;
            };
            let mut expected = [0u8; 32];
            link.connection
                .export_keying_material(&mut expected, &script.uuid, script.password.as_bytes())
                .expect("the keying material of a session whose handshake is done");
            (
                Some(*uuid),
                token,
                expected,
                (script.auth_failure_code, &b""[..]),
            )
        }
    };
    let accepted = token == expected && uuid.is_none_or(|uuid| uuid == script.uuid);
    link.shared
        .auths
        .lock()
        .expect("auths")
        .push(RecordedTuicAuth {
            connection: link.number,
            uuid,
            token,
            expected,
            accepted,
        });
    if accepted {
        authenticated.send_replace(true);
    } else {
        link.connection.close(VarInt::from_u32(code), reason);
    }
}

/// The client's datagrams, once the authentication went through — they
/// wait in QUIC's queue until then, as sing-box leaves them —: packets
/// relayed, v5's heartbeats counted. A v4 fake closes the connection on
/// anything but a packet, as tuic 0.8.5's server does.
async fn read_datagrams(link: Arc<Link>, mut authenticated: watch::Receiver<bool>) {
    if authenticated.wait_for(|&let_in| let_in).await.is_err() {
        return;
    }
    let version = link.shared.version;
    while let Ok(datagram) = link.connection.read_datagram().await {
        match (version, datagram.get(..2)) {
            (_, Some(&[v, PACKET])) if v == version => {
                if let Some(packet) = link.shared.decode(&datagram) {
                    link.relay_packet(packet, false).await;
                }
            }
            (V5, Some(&[V5, HEARTBEAT])) => {
                link.shared.heartbeats.fetch_add(1, Ordering::SeqCst);
            }
            (V4, _) => return link.close(BAD_COMMAND),
            _ => {}
        }
    }
}

impl Link {
    /// A packet of the client's: recorded, and sent on by its association —
    /// opened with its first packet, answering the way that came — once it
    /// is whole. A v4 fake closes the connection on a packet that does not
    /// come the way the connection's first came.
    async fn relay_packet(&self, packet: TuicPacket, on_stream: bool) {
        if self.shared.version == V4
            && *self
                .on_streams
                .lock()
                .expect("mode")
                .get_or_insert(on_stream)
                != on_stream
        {
            return self.close(BAD_COMMAND);
        }
        self.shared
            .packets
            .lock()
            .expect("packets")
            .push(RecordedTuicPacket {
                connection: self.number,
                association: packet.association,
                packet: packet.packet,
                fragments: packet.fragments,
                fragment: packet.fragment,
                address: packet.address.clone(),
                len: packet.payload.len(),
                on_stream,
            });
        let (socket, whole) = {
            let mut associations = self.associations.lock().expect("associations");
            let association = match associations.entry(packet.association) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    match Association::open(self, packet.association, on_stream) {
                        Ok(association) => entry.insert(association),
                        Err(_) => return,
                    }
                }
            };
            (association.socket.clone(), association.put_together(packet))
        };
        let Some((payload, address)) = whole else {
            return;
        };
        // never resolves: a name without `connect_to` is a dead end
        let to = self
            .shared
            .script
            .connect_to
            .or_else(|| address.parse().ok());
        if let Some(to) = to {
            let _ = socket.send_to(&payload, to).await;
        }
    }

    /// A Dissociate: recorded, and the association, if any, goes — its
    /// socket with it.
    fn dissociate(&self, association: u32) {
        self.shared
            .dissociates
            .lock()
            .expect("dissociates")
            .push(RecordedDissociate {
                connection: self.number,
                association,
            });
        self.associations
            .lock()
            .expect("associations")
            .remove(&association);
    }

    /// Closes the connection for a stream it cannot read: as sing-box
    /// closes it (v5), code 0 and no reason; as tuic 0.8.5 closes it (v4),
    /// with its protocol error.
    fn unreadable(&self) {
        if self.shared.version == V4 {
            self.close(PROTOCOL_ERROR);
        } else {
            self.connection.close(CLOSE, b"");
        }
    }

    /// Closes the connection with one of v4's codes and its reason.
    fn close(&self, (code, reason): (u32, &[u8])) {
        self.connection.close(VarInt::from_u32(code), reason);
    }
}

/// One of the client's associations on a connection: a loopback socket of
/// its own, which every packet of the association leaves by, and the
/// packets it is putting together, by id — side by side, as sing-box and
/// tuic-server put them together.
struct Association {
    socket: Arc<UdpSocket>,
    partial: HashMap<u16, Partial>,
    _answers: AbortOnDropHandle<()>,
}

/// A packet being put together: its fragments so far, and the first's
/// address.
struct Partial {
    parts: Vec<Option<Vec<u8>>>,
    address: Option<String>,
}

impl Association {
    /// Its socket, and the task that sends back whatever reaches it, the
    /// way the association's first packet came.
    fn open(link: &Link, association: u32, on_stream: bool) -> io::Result<Association> {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0")?;
        socket.set_nonblocking(true)?;
        let socket = Arc::new(UdpSocket::from_std(socket)?);
        link.shared
            .outside
            .lock()
            .expect("outside")
            .push(socket.local_addr()?);
        let answers = tokio::spawn(relay_answers(
            socket.clone(),
            link.connection.clone(),
            association,
            on_stream,
            link.shared.clone(),
        ));
        Ok(Association {
            socket,
            partial: HashMap::new(),
            _answers: AbortOnDropHandle::new(answers),
        })
    }

    /// The UDP packet `packet` makes whole, and its target: itself when it
    /// is no fragment. A fragment is dropped, as tuic-server drops it, when
    /// its count says it is none, or when it names an address and is not
    /// the first, or names none and is.
    fn put_together(&mut self, packet: TuicPacket) -> Option<(Vec<u8>, String)> {
        if packet.fragments <= 1 {
            return Some((packet.payload, packet.address?));
        }
        let (index, count) = (usize::from(packet.fragment), usize::from(packet.fragments));
        if index >= count || (index == 0) != packet.address.is_some() {
            return None;
        }
        let partial = self.partial.entry(packet.packet).or_insert(Partial {
            parts: Vec::new(),
            address: None,
        });
        if partial.parts.len() != count {
            partial.parts = vec![None; count];
        }
        if index == 0 {
            partial.address = packet.address;
        }
        partial.parts[index].get_or_insert(packet.payload);
        if partial.parts.iter().any(Option::is_none) {
            return None;
        }
        let whole = self.partial.remove(&packet.packet)?;
        Some((
            whole.parts.into_iter().flatten().flatten().collect(),
            whole.address?,
        ))
    }
}

/// Whatever reaches an association's socket goes back to the client under
/// its source — full cone; a v4 fake names an IPv4 source in IPv6 form, as
/// tuic 0.8.5's dual-stack socket hears it —, v5's packets counted from 0:
/// on a stream of its own each when the association's first packet came
/// so, else in datagrams, in fragments when one cannot hold a v5 one.
async fn relay_answers(
    socket: Arc<UdpSocket>,
    connection: Connection,
    association: u32,
    on_stream: bool,
    shared: Arc<Shared>,
) {
    let mut buf = vec![0u8; 65536];
    let mut packets = 0u16;
    loop {
        let (n, from) = match socket.recv_from(&mut buf).await {
            Ok(got) => got,
            // an ICMP "unreachable" for an earlier packet (Windows)
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(_) => return,
        };
        let from = match from {
            SocketAddr::V4(v4) if shared.version == V4 => {
                SocketAddr::new(IpAddr::V6(v4.ip().to_ipv6_mapped()), v4.port())
            }
            from => from,
        };
        let answer = TuicPacket::whole(association, packets, &from.to_string(), &buf[..n]);
        packets = packets.wrapping_add(1);
        if on_stream {
            let Ok(mut stream) = connection.open_uni().await else {
                return;
            };
            if stream.write_all(&shared.encode(&answer)).await.is_ok() {
                let _ = stream.finish();
            }
            continue;
        }
        let Some(max) = connection.max_datagram_size() else {
            return;
        };
        for datagram in shared.split(answer, max) {
            if let Err(SendDatagramError::ConnectionLost(_)) =
                connection.send_datagram(datagram.into())
            {
                return;
            }
        }
    }
}

/// A Connect: read and recorded, held until the authentication went
/// through, then relayed to its target, the half-closes too — a v4 fake
/// first answers that it reached it. One it cannot carry out — no Connect,
/// a name it has no `connect_to` for, a target that refuses — it ends as
/// sing-box ends one whose target it cannot reach: STOP_SENDING (code 0)
/// for what the client sends, FIN for what it reads; a v4 fake answers it
/// could not reach the target, as tuic 0.8.5's server does — unless its
/// script says no response —, and ends the stream.
async fn relay(
    mut send: SendStream,
    mut recv: RecvStream,
    number: usize,
    mut authenticated: watch::Receiver<bool>,
    shared: Arc<Shared>,
) {
    let Some(address) = read_connect(&mut recv, shared.version).await else {
        return refuse(send, recv);
    };
    let connect = RecordedConnect {
        connection: number,
        address: address.to_string(),
    };
    shared.connects.lock().expect("connects").push(connect);
    // ends with the connection when the authentication fails
    if authenticated.wait_for(|&let_in| let_in).await.is_err() {
        return;
    }
    if shared.script.silent {
        let _ = tokio::io::copy(&mut recv, &mut tokio::io::sink()).await;
        return;
    }
    // never resolves: a name without `connect_to` is a dead end
    let to = shared.script.connect_to.or(match address {
        Address::Ip(to) => Some(to),
        Address::Name(..) => None,
    });
    let upstream = match to {
        Some(to) => TcpStream::connect(to).await.ok(),
        None => None,
    };
    let v4 = shared.version == V4;
    let Some(mut upstream) = upstream else {
        if v4 {
            if !shared.script.no_response {
                let _ = send.write_all(&[V4, RESPONSE, FAILED]).await;
            }
            let _ = send.finish();
            return;
        }
        return refuse(send, recv);
    };
    if v4 && send.write_all(&[V4, RESPONSE, SUCCEEDED]).await.is_err() {
        return;
    }
    let mut client = tokio::io::join(recv, send);
    // in large pieces, so that a bulk transfer measures the client rather
    // than this relay
    let _ = copy_bidirectional_with_sizes(&mut client, &mut upstream, RELAY, RELAY).await;
}

/// Ends a stream the fake does not relay.
fn refuse(mut send: SendStream, mut recv: RecvStream) {
    let _ = recv.stop(CLOSE);
    let _ = send.finish();
}

/// A target as a command names it.
enum Address {
    Name(String, u16),
    Ip(SocketAddr),
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Address::Name(name, port) => write!(f, "{name}:{port}"),
            Address::Ip(addr) => addr.fmt(f),
        }
    }
}

/// The target of a Connect: `VER 01 ‖ ADDR`; `None` for anything else.
async fn read_connect(recv: &mut RecvStream, version: u8) -> Option<Address> {
    let mut header = [0u8; 2];
    recv.read_exact(&mut header).await.ok()?;
    if header != [version, CONNECT] {
        return None;
    }
    read_address(recv).await
}

/// ADDR, a field at a time (SPEC "Address"): `0x00` a name, its length in
/// a byte first; `0x01` an IPv4 address; `0x02` an IPv6 address; then the
/// port. `None` for none (`0xff`), which names no target, and for a type
/// it does not know.
async fn read_address(recv: &mut RecvStream) -> Option<Address> {
    let ip = match recv.read_u8().await.ok()? {
        0x00 => {
            let mut name = vec![0u8; usize::from(recv.read_u8().await.ok()?)];
            recv.read_exact(&mut name).await.ok()?;
            let name = String::from_utf8(name).ok()?;
            let port = recv.read_u16().await.ok()?;
            return Some(Address::Name(name, port));
        }
        0x01 => {
            let mut octets = [0u8; 4];
            recv.read_exact(&mut octets).await.ok()?;
            IpAddr::from(octets)
        }
        0x02 => {
            let mut octets = [0u8; 16];
            recv.read_exact(&mut octets).await.ok()?;
            IpAddr::from(octets)
        }
        _ => return None,
    };
    let port = recv.read_u16().await.ok()?;
    Some(Address::Ip(SocketAddr::new(ip, port)))
}
