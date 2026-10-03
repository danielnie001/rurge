//! UDP over TUIC (v5 SPEC, "Packet", "Dissociate"; v4's protocol README,
//! the same; phase 2 M7 design 6.2, 6.3): each carrier — one client
//! association on the outbound — is an association of its own on the
//! policy's connection, full cone: every target through it, and answers
//! from anyone, as the server keeps a UDP socket per association.
//!
//! - A v5 packet is `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE
//!   ‖ ADDR ‖ payload`, a v4 one `04 02 ‖ ASSOC_ID ‖ LEN ‖ ADDR ‖ payload`,
//!   big endian: SIZE and LEN are the payload of this message, ADDR (`addr`)
//!   names the target going out and the source coming back.
//! - Where the server takes QUIC datagrams (`native`), a packet goes in
//!   datagrams of the connection: whole when it fits one; else v5 splits it
//!   into fragments, only the first naming the address — the others name
//!   none —, all under the packet's id, and v4, which cannot, refuses it as
//!   too long, as the engine drops what an outbound's framing cannot carry.
//!   A carrier sends one packet at a time: Clash.Meta's first TUIC v5
//!   server put one packet of an association together at a time, as
//!   Hysteria's does, and lost both of two whose fragments came in turn.
//! - Where it takes none (`quic`), each packet goes whole on a
//!   unidirectional stream of its own, which the connection's writer opens,
//!   writes and finishes, one after the other: a send hands its packet over
//!   and waits for nothing — dropped when `OUTBOX` wait already —, so that
//!   a send given up on never leaves a packet cut short on the wire, a
//!   stream sing-box answers by closing the connection, every tunnel and
//!   association on it with it, as tuic 0.8.5 does when the cut is in the
//!   command's header. The server answers an association the way its first
//!   packet came (v4's holds the whole connection's UDP to that way, which
//!   one connection never leaves); answers are read either way.
//! - Association ids are per connection, past the ones in use: v5's count
//!   from 0 (16 bits), v4's are drawn at random (32 bits), as the reference
//!   clients' are. A carrier moves to the session's connection on its next
//!   packet once its own takes nothing new — closed, or dialled another way
//!   than the policy leads now (`Session::serves`: where the policy leads is
//!   asked at most once a second, the rest on every packet) —, under an id
//!   that connection gives it: the server takes it for a new association
//!   whatever its id.
//! - A carrier that goes, or moves, dissociates its association on the
//!   connection it leaves while that is open: `VER 03 ‖ ASSOC_ID` on a
//!   stream of its own, which the connection's writer sends after what was
//!   handed to it before — the association's last packets in `quic` mode
//!   among them. The drop waits for nothing: with `OUTBOX` waiting already,
//!   the Dissociate is dropped, and the server keeps the association until
//!   the connection ends.
//! - The connection's reader reads the server's packets — its datagrams,
//!   and its streams — into their associations' inboxes, putting v5's
//!   fragments together first: at most 8 packets of an association at a
//!   time, a ninth dropping the oldest. What finds no association, or a full
//!   inbox, is dropped.

use super::Version;
use super::addr::{self, NONE};
use super::auth::{self, Auth, Authenticated};
use super::heartbeat::{HEARTBEAT, Relaying};
use crate::session::{Lease, Session};
use bytes::Bytes;
use quinn::{Connection, RecvStream};
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use rurge_proto::OutboundError;
use std::collections::{HashMap, VecDeque, hash_map};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

/// Packets waiting for the engine, per carrier; more are dropped, as a
/// socket would.
pub(super) const INBOX: usize = 64;
/// Packets (`quic` mode) and Dissociates waiting for the connection's
/// writer; more are dropped, as a datagram the connection cannot take is.
pub(super) const OUTBOX: usize = 64;
/// How often a carrier asks where the policy leads (`Session::serves`): it
/// can cost a group's resolution, and every packet would ask.
const ROUTE_EVERY: Duration = Duration::from_secs(1);
/// The most fragments a packet goes in: their count is one byte.
const MAX_FRAGMENTS: usize = 255;
/// The longest UDP datagram: what a packet's SIZE allows, and the longest
/// packet put together.
const MAX_DATAGRAM: usize = 65_535;
/// An association's packets put together at a time (design 6.2).
const UNDER_WAY: usize = 8;
/// v5's `VER ‖ TYPE ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID ‖ SIZE`.
const HEADER: usize = 10;
/// v4's `VER ‖ TYPE ‖ ASSOC_ID ‖ LEN`.
const HEADER_V4: usize = 8;
/// The longest packet a stream carries: v5's header, the longer, the
/// longest address (a name of 255 bytes), the longest payload.
const MAX_MESSAGE: usize = HEADER + 1 + 1 + 255 + 2 + MAX_DATAGRAM;

// the commands' types
const PACKET: u8 = 0x02;
const DISSOCIATE: u8 = 0x03;

/// A packet for the engine, and where it came from.
type Answer = (Bytes, Target);

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("tuic: {what}"))
}

/// One v5 Packet command: `05 02 ‖ ASSOC_ID ‖ PKT_ID ‖ FRAG_TOTAL ‖ FRAG_ID
/// ‖ SIZE ‖ ADDR ‖ payload`, `address` the ADDR as `addr::put` writes it.
/// `payload` is at most 65535 bytes.
fn encode(
    association: u16,
    packet: u16,
    fragments: u8,
    fragment: u8,
    address: &[u8],
    payload: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + address.len() + payload.len());
    out.extend_from_slice(&[Version::V5.byte(), PACKET]);
    out.extend_from_slice(&association.to_be_bytes());
    out.extend_from_slice(&packet.to_be_bytes());
    out.extend_from_slice(&[fragments, fragment]);
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(address);
    out.extend_from_slice(payload);
    out
}

/// The QUIC datagrams, none longer than `max`, that carry `payload` to
/// `address` as packet `packet` of association `association`: one when it
/// fits — fragment 0 of 1 —, else fragments, only the first naming the
/// address. Refused when 255 fragments cannot carry it, or when the first
/// leaves no room for any of it.
fn datagrams(
    association: u16,
    packet: u16,
    address: &[u8],
    payload: &[u8],
    max: usize,
) -> io::Result<Vec<Vec<u8>>> {
    if HEADER + address.len() + payload.len() <= max {
        return Ok(vec![encode(association, packet, 1, 0, address, payload)]);
    }
    let first = max.saturating_sub(HEADER + address.len());
    let rest = max.saturating_sub(HEADER + 1);
    if first == 0 || rest == 0 {
        return Err(invalid("the address leaves no room in a datagram"));
    }
    let longest = MAX_DATAGRAM.min(first + (MAX_FRAGMENTS - 1) * rest);
    if payload.len() > longest {
        return Err(invalid(&format!("a datagram longer than {longest} bytes")));
    }
    // longer than `first`: it did not fit whole
    let (head, tail) = payload.split_at(first);
    let chunks: Vec<&[u8]> = std::iter::once(head).chain(tail.chunks(rest)).collect();
    let fragments = chunks.len() as u8;
    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(fragment, chunk)| {
            let address = if fragment == 0 { address } else { &[NONE][..] };
            encode(
                association,
                packet,
                fragments,
                fragment as u8,
                address,
                chunk,
            )
        })
        .collect())
}

/// One v4 Packet command: `04 02 ‖ ASSOC_ID ‖ LEN ‖ ADDR ‖ payload`, as
/// `encode`.
fn encode_v4(association: u32, address: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_V4 + address.len() + payload.len());
    out.extend_from_slice(&[Version::V4.byte(), PACKET]);
    out.extend_from_slice(&association.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(address);
    out.extend_from_slice(payload);
    out
}

/// The one QUIC datagram, no longer than `max`, that carries `payload` to
/// `address` for v4's association `association`: refused when it does not
/// fit — v4 has no fragments.
fn datagram_v4(
    association: u32,
    address: &[u8],
    payload: &[u8],
    max: usize,
) -> io::Result<Vec<u8>> {
    let room = max.saturating_sub(HEADER_V4 + address.len());
    if payload.len() > room {
        return Err(if room == 0 {
            invalid("the address leaves no room in a datagram")
        } else {
            invalid(&format!("a datagram longer than {room} bytes"))
        });
    }
    Ok(encode_v4(association, address, payload))
}

/// A Packet as it came from the server, its SIZE (v5) or LEN (v4) exactly
/// what follows the address: as sing-box reads v5's, as tuic 0.8.5's server
/// writes v4's.
struct Message {
    association: u32,
    packet: u16,
    fragments: u8,
    fragment: u8,
    /// The address: the source, on the first fragment.
    source: Option<Target>,
    payload: Bytes,
}

impl Message {
    /// `None` for anything but a Packet a server of `version` writes. v4's
    /// has neither a packet id nor fragments: it reads as fragment 0 of 1.
    fn parse(version: Version, data: Bytes) -> Option<Message> {
        let start = match version {
            Version::V4 => HEADER_V4,
            Version::V5 => HEADER,
        };
        let header = data.get(..start)?;
        if header[..2] != [version.byte(), PACKET] {
            return None;
        }
        let be16 = |at: usize| u16::from_be_bytes([header[at], header[at + 1]]);
        let (association, packet, fragments, fragment, size) = match version {
            Version::V4 => {
                let association = u32::from_be_bytes(header[2..6].try_into().ok()?);
                (association, 0, 1, 0, be16(6))
            }
            Version::V5 => (u32::from(be16(2)), be16(4), header[6], header[7], be16(8)),
        };
        let (source, len) = addr::get(&data[start..])?;
        let start = start + len;
        if data.len() - start != usize::from(size) {
            return None;
        }
        Some(Message {
            association,
            packet,
            fragments,
            fragment,
            source,
            payload: data.slice(start..),
        })
    }
}

/// An association's packets being put together from their fragments
/// (design 6.2): at most `UNDER_WAY` at a time, a ninth dropping the one
/// whose first fragment came first; each whole once every fragment came,
/// never when one is lost. A fragment seen already is dropped; one whose
/// count differs from its packet's begins the packet anew (as sing-box).
#[derive(Default)]
struct Reassembly(VecDeque<Partial>);

struct Partial {
    packet: u16,
    /// By fragment: each its own bytes, not a slice of what QUIC received,
    /// which would stay with it.
    parts: Vec<Option<Vec<u8>>>,
    missing: usize,
    size: usize,
    /// The first fragment's address.
    source: Option<Target>,
}

impl Reassembly {
    /// The packet `message` makes whole, and its source: the message's own
    /// payload when it is no fragment; `None` while fragments are missing,
    /// and for a packet that names no source.
    fn feed(&mut self, message: Message) -> Option<Answer> {
        let count = usize::from(message.fragments);
        // a count of 0 too, as sing-box reads it
        if count <= 1 {
            return Some((message.payload, message.source?));
        }
        let fragment = usize::from(message.fragment);
        if fragment >= count {
            return None;
        }
        let at = match self.0.iter().position(|p| p.packet == message.packet) {
            Some(at) if self.0[at].parts.len() == count => at,
            known => {
                if let Some(at) = known {
                    self.0.remove(at);
                }
                if self.0.len() == UNDER_WAY {
                    self.0.pop_front();
                }
                self.0.push_back(Partial {
                    packet: message.packet,
                    parts: vec![None; count],
                    missing: count,
                    size: 0,
                    source: None,
                });
                self.0.len() - 1
            }
        };
        let partial = &mut self.0[at];
        if partial.parts[fragment].is_some() {
            return None;
        }
        partial.size += message.payload.len();
        if partial.size > MAX_DATAGRAM {
            self.0.remove(at);
            return None;
        }
        if fragment == 0 {
            partial.source = message.source;
        }
        partial.parts[fragment] = Some(message.payload.to_vec());
        partial.missing -= 1;
        if partial.missing > 0 {
            return None;
        }
        let partial = self.0.remove(at)?;
        let mut whole = Vec::with_capacity(partial.size);
        for part in partial.parts.into_iter().flatten() {
            whole.extend_from_slice(&part);
        }
        Some((whole.into(), partial.source?))
    }
}

/// A carrier's association on a connection.
struct Association {
    inbox: mpsc::Sender<Answer>,
    reassembly: Reassembly,
}

/// A connection's associations, by id, and where the count of ids goes on
/// (`join`).
struct Associations {
    version: Version,
    by_id: HashMap<u32, Association>,
    next: u16,
}

impl Associations {
    fn new(version: Version) -> Associations {
        Associations {
            version,
            by_id: HashMap::new(),
            next: 0,
        }
    }

    /// Takes `inbox` in under an id free, and says which: v5's count from 0
    /// (as sing-box's client counts them), v4's are drawn at random (as tuic
    /// 0.8.5's client draws them) — `drawn`, drawn before the lock was
    /// taken, when it is free; past one in use, or without it, drawn here,
    /// and counted as v5's without a random source. `None` when all v5's
    /// 65536 are in use.
    fn join(&mut self, inbox: &mpsc::Sender<Answer>, mut drawn: Option<u32>) -> Option<u32> {
        for _ in 0..=u16::MAX {
            let counted = self.next;
            self.next = counted.wrapping_add(1);
            let id = match self.version {
                Version::V4 => drawn
                    .take()
                    .or_else(|| getrandom::u32().ok())
                    .unwrap_or(u32::from(counted)),
                Version::V5 => u32::from(counted),
            };
            if let hash_map::Entry::Vacant(free) = self.by_id.entry(id) {
                free.insert(Association {
                    inbox: inbox.clone(),
                    reassembly: Reassembly::default(),
                });
                return Some(id);
            }
        }
        None
    }
}

/// A connection's UDP: its associations, the task that reads the server's
/// packets into them, and the writer that sends the packets that go on
/// streams (`quic` mode) and the Dissociates. The tasks end with the
/// connection, or when this is dropped.
pub(super) struct Udp {
    associations: Arc<Mutex<Associations>>,
    version: Version,
    /// The writer's queue (`queue`).
    outbox: mpsc::Sender<Vec<u8>>,
    _reader: AbortOnDropHandle<()>,
    _writer: AbortOnDropHandle<()>,
}

impl Udp {
    /// The tasks read and write `version`'s commands; the reader marks
    /// `relayed` for every packet the server relays.
    pub(super) fn start(
        connection: &Connection,
        version: Version,
        relayed: Arc<AtomicBool>,
    ) -> Udp {
        let associations = Arc::new(Mutex::new(Associations::new(version)));
        let reader = tokio::spawn(read(
            connection.clone(),
            version,
            associations.clone(),
            relayed,
        ));
        let (outbox, packets) = mpsc::channel(OUTBOX);
        let writer = tokio::spawn(write(connection.clone(), packets));
        Udp {
            associations,
            version,
            outbox,
            _reader: AbortOnDropHandle::new(reader),
            _writer: AbortOnDropHandle::new(writer),
        }
    }

    /// Takes `inbox` in under an id free on the connection
    /// (`Associations::join`). v4's id is drawn before the lock is taken:
    /// the reader takes it for every packet the server sends.
    fn join(&self, inbox: &mpsc::Sender<Answer>) -> Option<u32> {
        let drawn = match self.version {
            Version::V4 => getrandom::u32().ok(),
            Version::V5 => None,
        };
        self.associations
            .lock()
            .expect("associations")
            .join(inbox, drawn)
    }

    /// Forgets association `id`: what comes for it from now on is dropped.
    /// With `dissociate`, the server is told so: the Dissociate goes to the
    /// connection's writer (`queue`), after what waits there already;
    /// dropped as a packet is, it leaves the association to the server until
    /// the connection ends.
    fn leave(&self, id: u32, dissociate: bool) {
        self.associations
            .lock()
            .expect("associations")
            .by_id
            .remove(&id);
        if dissociate {
            self.queue(dissociation(self.version, id));
        }
    }

    /// Hands `message`, a whole packet or a Dissociate, to the connection's
    /// writer (`write`), never waiting: dropped when `OUTBOX` wait already,
    /// as a full send buffer drops a datagram, or when the connection ended
    /// just now — the next send finds it closed and binds anew.
    fn queue(&self, message: Vec<u8>) {
        if let Err(e) = self.outbox.try_send(message) {
            let why = match e {
                TrySendError::Full(_) => "too many wait for a stream",
                TrySendError::Closed(_) => "the connection is closed",
            };
            tracing::debug!("tuic: a message for the server is dropped: {why}");
        }
    }

    /// Test support: the ids of the associations, in order.
    #[cfg(test)]
    pub(super) fn associations(&self) -> Vec<u32> {
        let associations = self.associations.lock().expect("associations");
        let mut ids: Vec<u32> = associations.by_id.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Test support: whether the task still reads.
    #[cfg(test)]
    pub(super) fn is_reading(&self) -> bool {
        !self._reader.is_finished()
    }

    /// Test support: whether the writer still writes.
    #[cfg(test)]
    pub(super) fn is_writing(&self) -> bool {
        !self._writer.is_finished()
    }
}

/// Reads the server's `version` packets on `connection` — its datagrams,
/// and its unidirectional streams, each read by a task of its own — into
/// their associations' inboxes, until the connection ends: what came before
/// its end included. Each source tells the end only once what quinn holds
/// of it is read; the one that tells it first is left, the other read on,
/// and so are the streams under way, to their ends.
async fn read(
    connection: Connection,
    version: Version,
    associations: Arc<Mutex<Associations>>,
    relayed: Arc<AtomicBool>,
) {
    let mut streams = JoinSet::new();
    // each source until it ends
    let (mut datagrams, mut accepting) = (true, true);
    loop {
        tokio::select! {
            datagram = connection.read_datagram(), if datagrams => match datagram {
                Ok(datagram) => deliver(version, &associations, &relayed, datagram),
                Err(_) => datagrams = false,
            },
            stream = connection.accept_uni(), if accepting => match stream {
                Ok(stream) => {
                    streams.spawn(read_stream(stream));
                }
                Err(_) => accepting = false,
            },
            Some(done) = streams.join_next() => {
                if let Ok(Some(message)) = done {
                    deliver(version, &associations, &relayed, message);
                }
            }
            // both sources ended, and no stream is left to read
            else => return,
        }
    }
}

/// A packet on a unidirectional stream of the server's (`quic` mode), read
/// to the stream's end, which follows it (sing-box and tuic-server finish
/// such a stream after one packet); `None` for a stream longer than the
/// longest packet, or cut short.
async fn read_stream(mut stream: RecvStream) -> Option<Bytes> {
    stream.read_to_end(MAX_MESSAGE).await.ok().map(Bytes::from)
}

/// `VER 03 ‖ ASSOC_ID`, which the writer sends on a unidirectional stream
/// of its own, finished — exactly that: sing-box closes the connection on a
/// longer one —: the server frees the association's socket.
fn dissociation(version: Version, id: u32) -> Vec<u8> {
    let mut command = vec![version.byte(), DISSOCIATE];
    match version {
        Version::V4 => command.extend_from_slice(&id.to_be_bytes()),
        // counted in 16 bits (`Associations::join`)
        Version::V5 => command.extend_from_slice(&(id as u16).to_be_bytes()),
    }
    command
}

/// Hands what the server sent — a datagram, or a stream's packet — to its
/// association's inbox, once the packet it carries is whole. A v5 heartbeat
/// (sing-box sends one every 10 seconds from the connection's start) says
/// nothing. A packet tells the authentication went through: servers relay
/// none before (`relayed`). Dropped when it is no packet, for no
/// association of the connection (a carrier gone), or meets a full inbox:
/// the reader waits for no carrier, so that one that reads slowly holds up
/// no other.
fn deliver(
    version: Version,
    associations: &Mutex<Associations>,
    relayed: &AtomicBool,
    data: Bytes,
) {
    if data.starts_with(&[version.byte(), HEARTBEAT]) {
        return;
    }
    let Some(message) = Message::parse(version, data) else {
        tracing::trace!("tuic: what the server sent is no packet; dropped");
        return;
    };
    auth::mark(relayed);
    let mut associations = associations.lock().expect("associations");
    let Some(association) = associations.by_id.get_mut(&message.association) else {
        return;
    };
    if let Some(answer) = association.reassembly.feed(message) {
        let _ = association.inbox.try_send(answer);
    }
}

/// Where a carrier is: its association on one connection, which its lease
/// keeps, counted among the connection's relays. Dropped, the connection
/// forgets the association, and tells the server when it is still open.
struct Bound {
    id: u32,
    _relaying: Relaying,
    lease: Lease<Authenticated>,
}

impl Bound {
    /// Whether it is on `lease`'s connection.
    fn is_on(&self, lease: &Lease<Authenticated>) -> bool {
        // both leases keep their connections: no two have the same id
        self.lease.connection().stable_id() == lease.connection().stable_id()
    }
}

impl Drop for Bound {
    fn drop(&mut self) {
        let open = self.lease.connection().close_reason().is_none();
        self.lease.state().udp.leave(self.id, open);
    }
}

/// One client association's UDP through the outbound (design 6.2, 6.3): an
/// association on the policy's connection, then on whichever connection
/// replaces it. No `Debug`: its session holds the credentials.
pub(super) struct TuicUdp {
    session: Arc<Session<Auth>>,
    /// The budget of binding to a new connection, its dial included.
    opts: ConnectOpts,
    bound: Mutex<Option<Arc<Bound>>>,
    /// The id of the carrier's next v5 packet (`packet`).
    packets: AtomicU16,
    /// Held while a packet's fragments go out: concurrent sends of the
    /// carrier take turns.
    sending: Mutex<()>,
    /// When the policy's route was last compared (`route_due`).
    route_checked: Mutex<Option<Instant>>,
    /// What the connections it binds to fill `inbox` through.
    answers: mpsc::Sender<Answer>,
    inbox: tokio::sync::Mutex<mpsc::Receiver<Answer>>,
}

impl TuicUdp {
    /// A carrier on the session's connection, dialed when there is none.
    /// Nothing bounds the time it takes here.
    pub(super) async fn open(
        session: Arc<Session<Auth>>,
        opts: &ConnectOpts,
    ) -> Result<TuicUdp, OutboundError> {
        let (answers, inbox) = mpsc::channel(INBOX);
        let udp = TuicUdp {
            session,
            opts: opts.clone(),
            bound: Mutex::default(),
            packets: AtomicU16::new(0),
            sending: Mutex::default(),
            route_checked: Mutex::default(),
            answers,
            inbox: tokio::sync::Mutex::new(inbox),
        };
        udp.bind().await?;
        Ok(udp)
    }

    /// Where the carrier is, while that connection takes what is new
    /// (`Session::serves`): that it is open is looked at on every call,
    /// where the policy leads at most once a second. A connection the
    /// server closed as it closes one whose authentication it refused fails
    /// the send that finds it so — the carrier then is bound nowhere, and
    /// the next send binds anew, dialing.
    fn current(&self) -> io::Result<Option<Arc<Bound>>> {
        let Some(bound) = self.bound.lock().expect("bound").clone() else {
            return Ok(None);
        };
        let serves = if self.route_due() {
            self.session.serves(&bound.lease)
        } else {
            self.session.serves_without_route(&bound.lease)
        };
        if serves {
            return Ok(Some(bound));
        }
        let state = bound.lease.state();
        match bound.lease.connection().close_reason() {
            Some(end) if state.refused(&end) => {
                self.unbind(&bound);
                Err(state.ended(&end))
            }
            _ => Ok(None),
        }
    }

    /// The id of a v5 packet: one for every packet, whole or not, counted
    /// from 0. v4's packets have none.
    fn packet(&self) -> u16 {
        self.packets.fetch_add(1, Ordering::Relaxed)
    }

    /// Whether the policy's route is to be compared now: it is when it was
    /// not within the last second, or at all, and this then counts as the
    /// comparison.
    fn route_due(&self) -> bool {
        let mut checked = self.route_checked.lock().expect("route checked");
        let now = Instant::now();
        if checked.is_some_and(|at| now.duration_since(at) < ROUTE_EVERY) {
            return false;
        }
        *checked = Some(now);
        true
    }

    /// Binds the carrier to the session's connection — a lease on it, the
    /// connection dialed when there is none — under an association id that
    /// connection gives it.
    async fn bind(&self) -> Result<Arc<Bound>, OutboundError> {
        let lease = self.session.lease(&self.opts).await?;
        let mut bound = self.bound.lock().expect("bound");
        // a send bound it meanwhile, to this very connection
        if let Some(current) = bound.as_ref().filter(|bound| bound.is_on(&lease)) {
            return Ok(current.clone());
        }
        let Some(id) = lease.state().udp.join(&self.answers) else {
            return Err(OutboundError::Proxy(
                "tuic: no association id is free on the connection".to_string(),
            ));
        };
        if let Some(old) = bound.as_ref() {
            tracing::debug!(
                "tuic: UDP association {} moves to a new connection, as association {id}",
                old.id
            );
        }
        let new = Arc::new(Bound {
            id,
            _relaying: lease.state().relaying(),
            lease,
        });
        let old = bound.replace(new.clone());
        drop(bound);
        // the connection before forgets the association, out of the lock
        drop(old);
        Ok(new)
    }

    /// `bind` for a send: within the carrier's budget, failing as a send
    /// fails.
    async fn rebind(&self) -> io::Result<Arc<Bound>> {
        match tokio::time::timeout(self.opts.timeout, self.bind()).await {
            Ok(Ok(bound)) => Ok(bound),
            Ok(Err(e)) => Err(io::Error::other(e.to_string())),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                OutboundError::Timeout.to_string(),
            )),
        }
    }

    /// Lets go of `bound`, unless the carrier is bound elsewhere already.
    /// The caller's hold on it keeps it past the lock.
    fn unbind(&self, bound: &Arc<Bound>) {
        let mut current = self.bound.lock().expect("bound");
        if current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, bound))
        {
            *current = None;
        }
    }
}

/// Hands `datagrams` to QUIC in order, never waiting: a full send buffer
/// makes room by dropping the oldest datagrams in it. A path that has
/// narrowed since their size was taken drops the packet they carry, as any
/// hop may, and so does a connection that closed just now — the next send
/// finds it closed and binds anew.
fn transmit(connection: &Connection, datagrams: Vec<Vec<u8>>) {
    for datagram in datagrams {
        if let Err(e) = connection.send_datagram(datagram.into()) {
            // the rest is no use without this one
            tracing::debug!("tuic: a datagram for the server is dropped: {e}");
            return;
        }
    }
}

/// The connection's writer: each message handed to it — a packet (`quic`
/// mode), a Dissociate — goes on a stream of its own, one after the other,
/// in the order they came, until the connection ends; a Dissociate so goes
/// after the packets of its association handed over before it. No send
/// waits for it, so no send given up on cuts a packet short: a stream that
/// ends early is one sing-box cannot read — nor tuic 0.8.5, when its header
/// is cut —, and either closes the connection for that.
async fn write(connection: Connection, mut packets: mpsc::Receiver<Vec<u8>>) {
    loop {
        let message = tokio::select! {
            biased;
            _ = connection.closed() => return,
            message = packets.recv() => message,
        };
        // the connection's state, and the queue with it, is gone
        let Some(message) = message else {
            return;
        };
        send_on_stream(&connection, &message).await;
    }
}

/// `message` on a unidirectional stream of its own, finished: the writer's
/// (`write`). It waits for QUIC to open the stream — the server's limit on
/// open streams — and to take the message, however long that takes. A
/// stream that fails loses its message alone, as any hop may lose a packet:
/// the server stopped it, or the connection ended — the next send then
/// finds it closed and binds anew.
async fn send_on_stream(connection: &Connection, message: &[u8]) {
    let mut stream = match connection.open_uni().await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::debug!("tuic: a message for the server is dropped: {e}");
            return;
        }
    };
    match stream.write_all(message).await {
        Ok(()) => {
            // fails only for a stream finished or reset already
            let _ = stream.finish();
        }
        Err(e) => tracing::debug!("tuic: a message for the server is dropped: {e}"),
    }
}

impl PacketSocket for TuicUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // checked first: nothing is dialed for what cannot be sent
            let mut address = Vec::new();
            addr::put(&mut address, to)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
            if buf.len() > MAX_DATAGRAM {
                return Err(invalid(&format!(
                    "a datagram longer than {MAX_DATAGRAM} bytes"
                )));
            }
            let bound = match self.current()? {
                Some(bound) => bound,
                // its connection takes nothing new: the session's, under
                // the id that one gives
                None => self.rebind().await?,
            };
            let connection = bound.lease.connection();
            let version = bound.lease.state().version();
            // v5's ids are counted in 16 bits (`Associations::join`)
            let (id, short_id) = (bound.id, bound.id as u16);
            match connection.max_datagram_size() {
                Some(max) => {
                    let out = match version {
                        Version::V4 => vec![datagram_v4(id, &address, buf, max)?],
                        Version::V5 => datagrams(short_id, self.packet(), &address, buf, max)?,
                    };
                    // all of its fragments in a row, with nothing awaited
                    // meanwhile
                    let sending = self.sending.lock().expect("sending");
                    transmit(connection, out);
                    drop(sending);
                }
                None => {
                    let message = match version {
                        Version::V4 => encode_v4(id, &address, buf),
                        Version::V5 => encode(short_id, self.packet(), 1, 0, &address, buf),
                    };
                    bound.lease.state().udp.queue(message);
                }
            }
            Ok(())
        })
    }

    /// A packet longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            loop {
                // `self` holds a sender: the inbox never closes
                let Some((packet, from)) = inbox.recv().await else {
                    return Err(io::ErrorKind::BrokenPipe.into());
                };
                if let Some(space) = buf.get_mut(..packet.len()) {
                    space.copy_from_slice(&packet);
                    return Ok((packet.len(), from));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::HostName;
    use std::collections::BTreeSet;

    /// `10.0.0.1:53` as ADDR.
    const ADDR: [u8; 7] = [0x01, 10, 0, 0, 1, 0, 53];

    fn source() -> Target {
        Target::new(HostName::parse("10.0.0.1"), 53)
    }

    /// `data` as a v5 server's packet.
    fn parse(data: &[u8]) -> Option<Message> {
        Message::parse(Version::V5, Bytes::copy_from_slice(data))
    }

    /// `data` as a v4 server's packet.
    fn parse_v4(data: &[u8]) -> Option<Message> {
        Message::parse(Version::V4, Bytes::copy_from_slice(data))
    }

    /// `(association, packet, fragments, fragment)` of `message`.
    fn ids(message: &Message) -> (u32, u16, u8, u8) {
        (
            message.association,
            message.packet,
            message.fragments,
            message.fragment,
        )
    }

    /// TUIC v5 SPEC "Packet": the header, its fields big endian, SIZE the
    /// payload's length, then the address and the payload.
    #[test]
    fn a_packet_that_fits_goes_whole() {
        let mut expected = vec![0x05, 0x02, 0x12, 0x34, 0x56, 0x78, 1, 0, 0, 5];
        expected.extend_from_slice(&ADDR);
        expected.extend_from_slice(b"query");
        assert_eq!(
            datagrams(0x1234, 0x5678, &ADDR, b"query", 1200).unwrap(),
            [expected]
        );
        // exactly as long as the limit
        let payload = vec![7u8; 1200 - HEADER - ADDR.len()];
        let whole = datagrams(1, 1, &ADDR, &payload, 1200).unwrap();
        assert_eq!((whole.len(), whole[0].len()), (1, 1200));
        // nothing to carry is a packet too
        assert_eq!(datagrams(1, 1, &ADDR, b"", 1200).unwrap()[0].len(), 17);
    }

    /// Design 6.2: what does not fit goes in fragments under one packet
    /// id, the first naming the address and the others none (`0xff`), each
    /// as long as the limit allows.
    #[test]
    fn what_does_not_fit_goes_in_fragments_only_the_first_naming_the_address() {
        let payload: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
        let fragments = datagrams(9, 77, &ADDR, &payload, 1000).unwrap();
        // 983 bytes in the first, 989 in each of the others
        assert_eq!(fragments.len(), 4);
        let mut whole = Vec::new();
        for (i, datagram) in fragments.iter().enumerate() {
            let message = parse(datagram).unwrap();
            assert_eq!(ids(&message), (9, 77, 4, i as u8));
            if i == 0 {
                assert_eq!(message.source, Some(source()));
            } else {
                assert_eq!(message.source, None);
                assert_eq!(datagram[HEADER], 0xff);
            }
            assert_eq!(datagram.len(), if i < 3 { 1000 } else { 50 });
            whole.extend_from_slice(&message.payload);
        }
        assert_eq!(whole, payload);
        // a byte over the limit: two fragments
        let over = vec![1u8; 1000 - HEADER - ADDR.len() + 1];
        assert_eq!(datagrams(1, 1, &ADDR, &over, 1000).unwrap().len(), 2);
    }

    #[test]
    fn a_packet_255_fragments_cannot_carry_is_refused() {
        // 200 - 17 bytes in the first, 200 - 11 in each of the others
        let longest = 183 + 254 * 189;
        let mut payload = vec![0u8; longest];
        assert_eq!(datagrams(1, 1, &ADDR, &payload, 200).unwrap().len(), 255);
        payload.push(0);
        let err = datagrams(1, 1, &ADDR, &payload, 200).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            format!("tuic: a datagram longer than {longest} bytes")
        );
        // a first fragment with no room past its address
        let err = datagrams(1, 1, &ADDR, b"x", HEADER + ADDR.len()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "tuic: the address leaves no room in a datagram"
        );
    }

    /// A packet is read as sing-box reads one: its SIZE exactly what
    /// follows the address.
    #[test]
    fn a_packet_is_read_as_sing_box_reads_it() {
        let mut data = vec![0x05, 0x02, 0, 7, 0x12, 0x34, 3, 1, 0, 7];
        data.extend_from_slice(&ADDR);
        data.extend_from_slice(b"payload");
        let message = parse(&data).unwrap();
        assert_eq!(ids(&message), (7, 0x1234, 3, 1));
        assert_eq!(message.source, Some(source()));
        assert_eq!(&message.payload[..], b"payload");
        // none for an address, no payload at all
        let message = parse(&[0x05, 0x02, 0, 1, 0, 2, 2, 1, 0, 0, 0xff]).unwrap();
        assert_eq!((message.source, message.payload.len()), (None, 0));
        let mut short = data.clone();
        short.pop();
        let mut long = data.clone();
        long.push(0);
        let mut v4 = data.clone();
        v4[0] = 0x04;
        let mut dissociate = data.clone();
        dissociate[1] = 0x03;
        for bad in [
            &data[..9],
            // SIZE says otherwise
            &short[..],
            &long[..],
            // another version, another command
            &v4[..],
            &dissociate[..],
            // an address cut short
            &data[..12],
        ] {
            assert!(parse(bad).is_none(), "{bad:?}");
        }
    }

    fn fragment(packet: u16, fragment: u8, fragments: u8, payload: &[u8]) -> Message {
        Message {
            association: 0,
            packet,
            fragments,
            fragment,
            source: (fragment == 0).then(source),
            payload: Bytes::copy_from_slice(payload),
        }
    }

    fn whole(payload: &[u8]) -> Option<Answer> {
        Some((Bytes::copy_from_slice(payload), source()))
    }

    #[test]
    fn fragments_make_their_packet_whole_in_any_order() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(fragment(5, 2, 3, b"ef")), None);
        assert_eq!(reassembly.feed(fragment(5, 0, 3, b"ab")), None);
        assert_eq!(reassembly.feed(fragment(5, 1, 3, b"cd")), whole(b"abcdef"));
        assert!(reassembly.0.is_empty());
    }

    /// Unlike Hysteria 2's, two packets of an association come together at
    /// once (as sing-box and tuic-server put them together), and a whole
    /// packet passes them both.
    #[test]
    fn packets_are_put_together_side_by_side() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(fragment(1, 0, 2, b"one-")), None);
        assert_eq!(reassembly.feed(fragment(2, 0, 2, b"two-")), None);
        assert_eq!(
            reassembly.feed(fragment(9, 0, 1, b"whole")),
            whole(b"whole")
        );
        // a count of 0 is a whole packet too
        assert_eq!(reassembly.feed(fragment(9, 0, 0, b"zero")), whole(b"zero"));
        assert_eq!(reassembly.feed(fragment(2, 1, 2, b"b")), whole(b"two-b"));
        assert_eq!(reassembly.feed(fragment(1, 1, 2, b"a")), whole(b"one-a"));
    }

    /// Design 6.2: at most 8 packets under way; the one whose first
    /// fragment came first is dropped for a ninth, and its last fragment,
    /// come late, makes nothing whole.
    #[test]
    fn a_ninth_packet_under_way_drops_the_oldest() {
        let mut reassembly = Reassembly::default();
        for packet in 1..=9 {
            assert_eq!(reassembly.feed(fragment(packet, 0, 2, b"x")), None);
        }
        assert_eq!(reassembly.0.len(), UNDER_WAY);
        for packet in 2..=9 {
            assert_eq!(reassembly.feed(fragment(packet, 1, 2, b"y")), whole(b"xy"));
        }
        assert_eq!(reassembly.feed(fragment(1, 1, 2, b"y")), None);
    }

    #[test]
    fn duplicates_and_fragments_past_the_count_are_dropped() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(fragment(4, 2, 2, b"x")), None);
        assert_eq!(reassembly.feed(fragment(4, 0, 2, b"a")), None);
        // the first one stays
        assert_eq!(reassembly.feed(fragment(4, 0, 2, b"A")), None);
        assert_eq!(reassembly.feed(fragment(4, 1, 2, b"b")), whole(b"ab"));
        // a fragment counting otherwise begins its packet anew
        assert_eq!(reassembly.feed(fragment(6, 0, 3, b"a")), None);
        assert_eq!(reassembly.feed(fragment(6, 1, 2, b"b")), None);
        assert_eq!(reassembly.feed(fragment(6, 0, 2, b"c")), whole(b"cb"));
    }

    /// The source is the first fragment's: a packet without it, or a whole
    /// one that names none, goes nowhere.
    #[test]
    fn a_packet_that_names_no_source_is_dropped() {
        let mut reassembly = Reassembly::default();
        let mut first = fragment(3, 0, 2, b"a");
        first.source = None;
        assert_eq!(reassembly.feed(first), None);
        assert_eq!(reassembly.feed(fragment(3, 1, 2, b"b")), None);
        let mut lone = fragment(4, 0, 1, b"lone");
        lone.source = None;
        assert_eq!(reassembly.feed(lone), None);
    }

    #[test]
    fn a_packet_longer_than_udp_allows_is_dropped() {
        let mut reassembly = Reassembly::default();
        let half = vec![0u8; 32_768];
        assert_eq!(reassembly.feed(fragment(6, 0, 3, &half)), None);
        assert_eq!(reassembly.feed(fragment(6, 1, 3, &half)), None);
        assert!(reassembly.0.is_empty());
        // the longest there is goes through
        assert_eq!(reassembly.feed(fragment(7, 0, 2, &vec![0; 65_000])), None);
        let (packet, _) = reassembly.feed(fragment(7, 1, 2, &[0; 535])).unwrap();
        assert_eq!(packet.len(), MAX_DATAGRAM);
    }

    /// Design 6.2: ids count per connection from 0, past the ones in use;
    /// one let go of is given again once the count comes round, and none
    /// is left once all 65536 are in use.
    #[test]
    fn association_ids_count_from_0_past_the_ones_in_use() {
        let mut associations = Associations::new(Version::V5);
        let (inbox, _rx) = mpsc::channel(1);
        assert_eq!(
            (
                associations.join(&inbox, None),
                associations.join(&inbox, None)
            ),
            (Some(0), Some(1))
        );
        associations.next = u16::MAX;
        assert_eq!(associations.join(&inbox, None), Some(u32::from(u16::MAX)));
        // round again: 0 and 1 are in use
        assert_eq!(associations.join(&inbox, None), Some(2));
        associations.by_id.remove(&0);
        associations.next = u16::MAX;
        assert_eq!(associations.join(&inbox, None), Some(0));
        while associations.by_id.len() < 1 << 16 {
            associations.join(&inbox, None).unwrap();
        }
        assert_eq!(associations.join(&inbox, None), None);
    }

    /// Design 6.3: v4's ids are drawn at random from all 32 bits, as tuic
    /// 0.8.5's client draws them, never one in use: the one drawn before
    /// the lock is the association's when it is free, else another is.
    #[test]
    fn v4_association_ids_are_drawn_at_random_past_the_ones_in_use() {
        let mut associations = Associations::new(Version::V4);
        let (inbox, _rx) = mpsc::channel(1);
        let ids: BTreeSet<u32> = (0..1000)
            .map(|_| associations.join(&inbox, getrandom::u32().ok()).unwrap())
            .collect();
        assert_eq!(ids.len(), 1000);
        assert_eq!(associations.by_id.len(), 1000);
        // a thousand below 65536 would be counted, not drawn
        assert!(ids.iter().any(|&id| id > u32::from(u16::MAX)), "{ids:?}");
        let free = (0u32..).find(|id| !ids.contains(id)).unwrap();
        assert_eq!(associations.join(&inbox, Some(free)), Some(free));
        // one in use gives way to another, drawn anew
        for &in_use in ids.iter().chain([&free]) {
            let joined = associations.by_id.len();
            let id = associations.join(&inbox, Some(in_use)).unwrap();
            assert_ne!(id, in_use);
            // taken in beside the one in use, not in its place
            assert_eq!(associations.by_id.len(), joined + 1);
        }
    }

    /// v4's Packet (protocol README): `04 02 ‖ ASSOC_ID ‖ LEN ‖ ADDR ‖
    /// payload`, its fields big endian, LEN the payload's length — in one
    /// datagram when it fits, else not at all: v4 has no fragments.
    #[test]
    fn a_v4_packet_goes_whole_or_not_at_all() {
        let mut expected = vec![0x04, 0x02, 0x12, 0x34, 0x56, 0x78, 0, 5];
        expected.extend_from_slice(&ADDR);
        expected.extend_from_slice(b"query");
        assert_eq!(
            datagram_v4(0x1234_5678, &ADDR, b"query", 1200).unwrap(),
            expected
        );
        assert_eq!(encode_v4(0x1234_5678, &ADDR, b"query"), expected);
        // exactly as long as the limit
        let room = 1200 - HEADER_V4 - ADDR.len();
        let whole = datagram_v4(1, &ADDR, &vec![7u8; room], 1200).unwrap();
        assert_eq!(whole.len(), 1200);
        let err = datagram_v4(1, &ADDR, &vec![7u8; room + 1], 1200).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            format!("tuic: a datagram longer than {room} bytes")
        );
        // an address that leaves no room
        let err = datagram_v4(1, &ADDR, b"x", HEADER_V4 + ADDR.len()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "tuic: the address leaves no room in a datagram"
        );
    }

    /// A v4 packet is read whole, its LEN exactly what follows the address:
    /// fragment 0 of 1, with no packet id.
    #[test]
    fn a_v4_packet_is_read_whole_with_its_length_exactly() {
        let mut data = vec![0x04, 0x02, 0xde, 0xad, 0xbe, 0xef, 0, 7];
        data.extend_from_slice(&ADDR);
        data.extend_from_slice(b"payload");
        let message = parse_v4(&data).unwrap();
        assert_eq!(ids(&message), (0xdead_beef, 0, 1, 0));
        assert_eq!(message.source, Some(source()));
        assert_eq!(&message.payload[..], b"payload");
        let mut short = data.clone();
        short.pop();
        let mut long = data.clone();
        long.push(0);
        let mut dissociate = data.clone();
        dissociate[1] = 0x03;
        let mut v5 = vec![0x05, 0x02, 0, 7, 0x12, 0x34, 1, 0, 0, 7];
        v5.extend_from_slice(&ADDR);
        v5.extend_from_slice(b"payload");
        for bad in [
            &data[..7],
            // LEN says otherwise
            &short[..],
            &long[..],
            // another command, another version
            &dissociate[..],
            &v5[..],
            // an address cut short
            &data[..10],
        ] {
            assert!(parse_v4(bad).is_none(), "{bad:?}");
        }
        // and the other way round
        assert!(parse(&data).is_none());
    }
}
