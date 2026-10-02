//! UDP over Hysteria 2 (Hysteria 2 protocol, "UDP"; phase 2 M7 design
//! 5.3): every datagram in QUIC datagrams of the policy's connection, each
//! way.
//!
//! - A message is `u32 session ‖ u16 packet ‖ u8 fragment ‖ u8 fragments ‖
//!   varint length ‖ host:port ‖ payload`, big endian; the address names
//!   the target going out and the source coming back.
//! - Each carrier — one client association on the outbound — is a session
//!   of its own: every target through it, and answers from anyone (full
//!   cone, as the server keeps a socket per session). A carrier moves to
//!   the session's connection on its next datagram once its own takes
//!   nothing new — ended, going away, or dialled another way than the
//!   policy leads now (`Session::serves`: where the policy leads is asked
//!   at most once a second, the rest on every datagram). Session ids count
//!   from 1 per outbound, so it keeps its id there, where no other carrier
//!   holds it: the server takes it for a new session.
//! - A datagram that does not fit one QUIC datagram goes in fragments — at
//!   most 255, under a random packet id, each with the whole header —; one
//!   that fits goes whole, packet 0, fragment 0 of 1. A carrier sends one
//!   datagram at a time: the server puts one datagram of a session
//!   together at a time, so another's fragment among them would lose both.
//! - One task per connection reads its datagrams into their sessions'
//!   inboxes, putting fragments together first: one datagram at a time per
//!   session, a fragment of another dropping the one under way. What finds
//!   no session, or a full inbox, is dropped.
//! - The protocol has no message that ends a session: a carrier that goes
//!   only forgets its id, and the server reclaims the session once it
//!   idles.

use super::auth::{Auth, Authenticated};
use super::{MAX_ADDRESS, address};
use crate::session::{Lease, Session};
use crate::varint;
use bytes::Bytes;
use quinn::Connection;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use rurge_proto::OutboundError;
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::task::AbortOnDropHandle;

/// Datagrams waiting for the engine, per carrier; more are dropped, as a
/// socket would.
pub(super) const INBOX: usize = 64;
/// How often a carrier asks where the policy leads (`Session::serves`): it
/// can cost a group's resolution, and every datagram would ask.
const ROUTE_EVERY: Duration = Duration::from_secs(1);
/// The most fragments a datagram goes in: their count is one byte.
const MAX_FRAGMENTS: usize = 255;
/// The longest datagram put together: no UDP datagram is longer.
const MAX_DATAGRAM: usize = 65_535;
/// `u32 session ‖ u16 packet ‖ u8 fragment ‖ u8 fragments`.
const FIXED: usize = 8;

/// A datagram for the engine, and where it came from.
type Packet = (Bytes, Target);

/// The session ids of an outbound's carriers: from 1 upward, never 0.
#[derive(Default)]
pub(super) struct SessionIds(AtomicU32);

impl SessionIds {
    fn next(&self) -> u32 {
        loop {
            match self.0.fetch_add(1, Ordering::Relaxed).wrapping_add(1) {
                0 => continue,
                id => return id,
            }
        }
    }
}

fn invalid(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("hysteria2: {what}"))
}

/// The QUIC datagrams, none longer than `max`, that carry `payload` to
/// `address` in session `id`: one when it fits — packet 0, fragment 0 of 1
/// —, else fragments under a random packet id, each with the whole header
/// (the reference's `FragUDPMessage`). Refused when 255 fragments cannot
/// carry it.
fn datagrams(id: u32, address: &str, payload: &[u8], max: usize) -> io::Result<Vec<Vec<u8>>> {
    let mut header = Vec::with_capacity(FIXED + 2 + address.len());
    header.extend_from_slice(&id.to_be_bytes());
    // packet 0, fragment 0 of 1; a fragment says otherwise
    header.extend_from_slice(&[0, 0, 0, 1]);
    varint::put(&mut header, address.len() as u64);
    header.extend_from_slice(address.as_bytes());
    if header.len() + payload.len() <= max {
        header.extend_from_slice(payload);
        return Ok(vec![header]);
    }
    let room = max.saturating_sub(header.len());
    if room == 0 {
        return Err(invalid("the host name is too long for a datagram"));
    }
    if payload.len() > MAX_FRAGMENTS * room {
        return Err(invalid(&format!(
            "a datagram longer than {} bytes",
            MAX_FRAGMENTS * room
        )));
    }
    let fragments = payload.len().div_ceil(room) as u8;
    let packet = packet_id().to_be_bytes();
    Ok(payload
        .chunks(room)
        .enumerate()
        .map(|(fragment, chunk)| {
            let mut datagram = Vec::with_capacity(header.len() + chunk.len());
            datagram.extend_from_slice(&header);
            datagram[4..6].copy_from_slice(&packet);
            datagram[6] = fragment as u8;
            datagram[7] = fragments;
            datagram.extend_from_slice(chunk);
            datagram
        })
        .collect())
}

/// A fragmented datagram's packet id: random, 1 to 65535, as the reference
/// client draws it (`core/client/udp.go`); 1 without a random source.
fn packet_id() -> u16 {
    getrandom::u32().map_or(1, |random| (random % 65_535) as u16 + 1)
}

/// A message as it came. The address is one the reference reads (1 to 2048
/// bytes, `ParseUDPMessage`); the payload is whatever follows, nothing
/// included.
struct Message {
    session: u32,
    packet: u16,
    fragment: u8,
    fragments: u8,
    address: Bytes,
    payload: Bytes,
}

impl Message {
    fn parse(datagram: Bytes) -> Option<Message> {
        let fixed: [u8; FIXED] = datagram.get(..FIXED)?.try_into().ok()?;
        let (len, n) = varint::get(&datagram[FIXED..])?;
        let len = usize::try_from(len)
            .ok()
            .filter(|len| (1..=MAX_ADDRESS).contains(len))?;
        let start = FIXED + n;
        let end = start + len;
        if end > datagram.len() {
            return None;
        }
        Some(Message {
            session: u32::from_be_bytes([fixed[0], fixed[1], fixed[2], fixed[3]]),
            packet: u16::from_be_bytes([fixed[4], fixed[5]]),
            fragment: fixed[6],
            fragments: fixed[7],
            address: datagram.slice(start..end),
            payload: datagram.slice(end..),
        })
    }
}

/// Where a datagram came from: `host:port`, the host an address (IPv6 in
/// brackets) or a name; `None` for anything else.
fn source(address: &[u8]) -> Option<Target> {
    let (host, port) = std::str::from_utf8(address).ok()?.rsplit_once(':')?;
    Some(Target::new(HostName::from_wire(host)?, port.parse().ok()?))
}

/// A session's datagram put together from its fragments (design 5.3, the
/// reference's `Defragger`): one at a time — a fragment of another drops
/// the one under way —, whole once every fragment came, never when one is
/// lost.
#[derive(Default)]
enum Reassembly {
    #[default]
    Idle,
    Collecting {
        packet: u16,
        parts: Vec<Option<Bytes>>,
        missing: usize,
        size: usize,
    },
    /// Handed on, or dropped: whatever more comes of it is dropped too.
    Over { packet: u16, fragments: u8 },
}

impl Reassembly {
    /// The datagram `message` makes whole: the message's own payload when
    /// it is no fragment — the datagram under way stays —; `None` while
    /// fragments are missing.
    fn feed(&mut self, message: &Message) -> Option<Bytes> {
        let count = message.fragments;
        // a count of 0 too, as the reference reads it
        if count <= 1 {
            return Some(message.payload.clone());
        }
        if message.fragment >= count {
            return None;
        }
        let known = match self {
            Reassembly::Idle => false,
            Reassembly::Collecting { packet, parts, .. } => {
                *packet == message.packet && parts.len() == usize::from(count)
            }
            Reassembly::Over { packet, fragments } => {
                *packet == message.packet && *fragments == count
            }
        };
        if !known {
            // another datagram: the one under way is lost
            *self = Reassembly::Collecting {
                packet: message.packet,
                parts: vec![None; usize::from(count)],
                missing: usize::from(count),
                size: 0,
            };
        }
        let Reassembly::Collecting {
            parts,
            missing,
            size,
            ..
        } = self
        else {
            // of one handed on or dropped
            return None;
        };
        let part = &mut parts[usize::from(message.fragment)];
        if part.is_some() {
            // seen already
            return None;
        }
        *size += message.payload.len();
        if *size > MAX_DATAGRAM {
            *self = Reassembly::Over {
                packet: message.packet,
                fragments: count,
            };
            return None;
        }
        *part = Some(message.payload.clone());
        *missing -= 1;
        if *missing > 0 {
            return None;
        }
        let mut whole = Vec::with_capacity(*size);
        for part in parts.iter().flatten() {
            whole.extend_from_slice(part);
        }
        *self = Reassembly::Over {
            packet: message.packet,
            fragments: count,
        };
        Some(whole.into())
    }
}

/// A carrier's session on a connection.
struct Entry {
    inbox: mpsc::Sender<Packet>,
    reassembly: Reassembly,
}

type Sessions = Mutex<HashMap<u32, Entry>>;

/// A connection's UDP sessions, when its server relays UDP: the inbox of
/// each, and the task that reads the connection's datagrams into them. The
/// task ends with the connection, or when this is dropped.
pub(super) struct Datagrams {
    sessions: Arc<Sessions>,
    _reader: AbortOnDropHandle<()>,
}

impl Datagrams {
    pub(super) fn start(connection: &Connection) -> Datagrams {
        let sessions = Arc::new(Sessions::default());
        let reader = tokio::spawn(read(connection.clone(), sessions.clone()));
        Datagrams {
            sessions,
            _reader: AbortOnDropHandle::new(reader),
        }
    }

    /// Takes `inbox` in as session `id` — or, should a carrier hold `id`
    /// here, which only an outbound that went through every id can cause,
    /// as the next id of `ids` free here —, and says which.
    fn join(&self, mut id: u32, inbox: &mpsc::Sender<Packet>, ids: &SessionIds) -> u32 {
        let mut sessions = self.sessions.lock().expect("sessions");
        while sessions.contains_key(&id) {
            id = ids.next();
        }
        sessions.insert(
            id,
            Entry {
                inbox: inbox.clone(),
                reassembly: Reassembly::default(),
            },
        );
        id
    }

    /// Forgets session `id`: what comes for it from now on is dropped.
    fn leave(&self, id: u32) {
        self.sessions.lock().expect("sessions").remove(&id);
    }

    /// Test support: the ids of the sessions, in order.
    #[cfg(test)]
    pub(super) fn sessions(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .sessions
            .lock()
            .expect("sessions")
            .keys()
            .copied()
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Test support: whether the task still reads.
    #[cfg(test)]
    pub(super) fn is_reading(&self) -> bool {
        !self._reader.is_finished()
    }
}

/// Reads `connection`'s datagrams into their sessions' inboxes until it
/// ends — those that came before its end included.
async fn read(connection: Connection, sessions: Arc<Sessions>) {
    while let Ok(datagram) = connection.read_datagram().await {
        deliver(&sessions, datagram);
    }
}

/// Hands `datagram` to its session's inbox, once the datagram it carries
/// is whole. Dropped when it is no message, for no session of the
/// connection (a carrier gone), or meets a full inbox: the reader waits for
/// no carrier, so that one that reads slowly holds up no other.
fn deliver(sessions: &Sessions, datagram: Bytes) {
    let Some(message) = Message::parse(datagram) else {
        tracing::trace!("hysteria2: a datagram that is no UDP message is dropped");
        return;
    };
    let mut sessions = sessions.lock().expect("sessions");
    let Some(entry) = sessions.get_mut(&message.session) else {
        return;
    };
    let Some(whole) = entry.reassembly.feed(&message) else {
        return;
    };
    if let Some(from) = source(&message.address) {
        let _ = entry.inbox.try_send((whole, from));
    }
}

/// Where a carrier is: its session id on one connection, which its lease
/// keeps. Dropped, the connection forgets the session.
struct Bound {
    id: u32,
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
        if let Some(udp) = &self.lease.state().udp {
            udp.leave(self.id);
        }
    }
}

fn refused(what: &str) -> OutboundError {
    OutboundError::Proxy(format!("hysteria2: {what}"))
}

/// One client association's UDP through the outbound (design 5.3): a
/// session on the policy's connection, then on whichever connection
/// replaces it. No `Debug`: its session holds the password.
pub(super) struct Hysteria2Udp {
    session: Arc<Session<Auth>>,
    ids: Arc<SessionIds>,
    /// The budget of binding to a new connection, its dial included.
    opts: ConnectOpts,
    bound: Mutex<Option<Arc<Bound>>>,
    /// Held while a datagram's fragments go out: concurrent sends of the
    /// carrier take turns, as the server puts one datagram of the session
    /// together at a time.
    sending: Mutex<()>,
    /// When the policy's route was last compared (`route_due`).
    route_checked: Mutex<Option<Instant>>,
    /// What the connections it binds to fill `inbox` through.
    answers: mpsc::Sender<Packet>,
    inbox: tokio::sync::Mutex<mpsc::Receiver<Packet>>,
}

impl Hysteria2Udp {
    /// A carrier on the session's connection, dialed when there is none.
    /// Nothing bounds the time it takes here.
    pub(super) async fn open(
        session: Arc<Session<Auth>>,
        ids: Arc<SessionIds>,
        opts: &ConnectOpts,
    ) -> Result<Hysteria2Udp, OutboundError> {
        let (answers, inbox) = mpsc::channel(INBOX);
        let udp = Hysteria2Udp {
            session,
            ids,
            opts: opts.clone(),
            bound: Mutex::default(),
            sending: Mutex::default(),
            route_checked: Mutex::default(),
            answers,
            inbox: tokio::sync::Mutex::new(inbox),
        };
        udp.bind().await?;
        Ok(udp)
    }

    /// Where the carrier is, while that connection takes what is new
    /// (`Session::serves`): that it is open and not going away is looked at
    /// on every call, where the policy leads at most once a second.
    fn current(&self) -> Option<Arc<Bound>> {
        let bound = self.bound.lock().expect("bound").clone()?;
        let serves = if self.route_due() {
            self.session.serves(&bound.lease)
        } else {
            self.session.serves_without_route(&bound.lease)
        };
        serves.then_some(bound)
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
    /// connection dialed when there is none — under the session id it had,
    /// a new one the first time. Refused when that connection's server
    /// relays no UDP.
    async fn bind(&self) -> Result<Arc<Bound>, OutboundError> {
        let lease = self.session.lease(&self.opts).await?;
        let Some(udp) = &lease.state().udp else {
            return Err(refused("the server does not allow UDP"));
        };
        if lease.connection().max_datagram_size().is_none() {
            return Err(refused("the server does not support QUIC datagrams"));
        }
        let mut bound = self.bound.lock().expect("bound");
        // a send bound it meanwhile, to this very connection
        if let Some(current) = bound.as_ref().filter(|bound| bound.is_on(&lease)) {
            return Ok(current.clone());
        }
        let id = bound.as_ref().map_or_else(|| self.ids.next(), |old| old.id);
        let id = udp.join(id, &self.answers, &self.ids);
        if bound.is_some() {
            tracing::debug!("hysteria2: UDP session {id} moves to a new connection");
        }
        let new = Arc::new(Bound { id, lease });
        let old = bound.replace(new.clone());
        drop(bound);
        // the connection before forgets the session, out of the lock
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
}

/// Hands `datagrams` to QUIC in order, never waiting: a full send buffer
/// makes room by dropping the oldest datagrams in it. A path that has
/// narrowed since their size was taken drops the datagram they carry, as
/// any hop may, and so does a connection that closed just now — the next
/// send finds it closed and binds anew.
fn transmit(connection: &Connection, datagrams: Vec<Vec<u8>>) {
    for datagram in datagrams {
        if let Err(e) = connection.send_datagram(datagram.into()) {
            // the rest is no use without this one
            tracing::debug!("hysteria2: a datagram for the server is dropped: {e}");
            return;
        }
    }
}

impl PacketSocket for Hysteria2Udp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // checked first: nothing is dialed for what cannot be sent
            let Some(address) = address(to) else {
                return Err(invalid("the host name cannot be sent to the server"));
            };
            let bound = match self.current() {
                Some(bound) => bound,
                // its connection takes nothing new: the session's, the
                // same id
                None => self.rebind().await?,
            };
            let connection = bound.lease.connection();
            let Some(max) = connection.max_datagram_size() else {
                return Err(io::Error::other(
                    "hysteria2: the server does not support QUIC datagrams",
                ));
            };
            let out = datagrams(bound.id, &address, buf, max)?;
            // all of its fragments in a row, with nothing awaited meanwhile
            let sending = self.sending.lock().expect("sending");
            transmit(connection, out);
            drop(sending);
            Ok(())
        })
    }

    /// A datagram longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            loop {
                // `self` holds a sender: the inbox never closes
                let Some((datagram, from)) = inbox.recv().await else {
                    return Err(io::ErrorKind::BrokenPipe.into());
                };
                if let Some(space) = buf.get_mut(..datagram.len()) {
                    space.copy_from_slice(&datagram);
                    return Ok((datagram.len(), from));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hysteria2::Hysteria2Outbound;
    use crate::testing::{FakeHysteria2, Hysteria2Script};
    use rurge_config::Span;
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::hysteria2::read_hysteria2;
    use rurge_config::spec::{CommonOpts, Hysteria2Spec, ParamReader, Tristate};
    use rurge_net::connector::{
        BoxedDatagram, BoxedStream, Connector, DirectConnector, Route, SystemResolve,
    };
    use rurge_proto::Outbound;
    use std::collections::HashSet;
    use std::hash::{DefaultHasher, Hash, Hasher};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Barrier;
    use tokio::task::JoinSet;

    const SOURCE: &str = "192.0.2.1:53";

    fn parse(datagram: &[u8]) -> Option<Message> {
        Message::parse(Bytes::copy_from_slice(datagram))
    }

    /// `(session, packet, fragment, fragments)` of `message`.
    fn ids(message: &Message) -> (u32, u16, u8, u8) {
        (
            message.session,
            message.packet,
            message.fragment,
            message.fragments,
        )
    }

    #[test]
    fn a_datagram_that_fits_goes_whole() {
        let mut expected = vec![1, 2, 3, 4, 0, 0, 0, 1, 12];
        expected.extend_from_slice(b"192.0.2.1:53query");
        assert_eq!(
            datagrams(0x0102_0304, SOURCE, b"query", 1200).unwrap(),
            [expected]
        );
        // exactly as long as the limit
        let payload = vec![7u8; 1200 - 8 - 1 - SOURCE.len()];
        let whole = datagrams(1, SOURCE, &payload, 1200).unwrap();
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].len(), 1200);
        // nothing to carry is a datagram too
        assert_eq!(datagrams(1, SOURCE, b"", 1200).unwrap()[0].len(), 21);
        // an address of 64 bytes or more: its length in two bytes
        let name = format!("{}.test:53", "a".repeat(80));
        assert_eq!(
            &datagrams(1, &name, b"x", 1200).unwrap()[0][8..10],
            [0x40, 88]
        );
    }

    #[test]
    fn what_does_not_fit_goes_in_fragments_each_with_the_whole_header() {
        let payload: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
        let address = "[2001:db8::1]:443";
        // 8 + 1 + 17 bytes of header: 974 of payload each
        let fragments = datagrams(9, address, &payload, 1000).unwrap();
        assert_eq!(fragments.len(), 4);
        let packet = parse(&fragments[0]).unwrap().packet;
        assert_ne!(packet, 0);
        let mut whole = Vec::new();
        for (i, datagram) in fragments.iter().enumerate() {
            assert!(datagram.len() <= 1000, "{}", datagram.len());
            let message = parse(datagram).unwrap();
            assert_eq!(ids(&message), (9, packet, i as u8, 4));
            assert_eq!(&message.address[..], address.as_bytes());
            whole.extend_from_slice(&message.payload);
        }
        assert_eq!(whole, payload);
        // a byte over the limit: two fragments
        let over = vec![1u8; 1000 - 8 - 1 - SOURCE.len() + 1];
        assert_eq!(datagrams(1, SOURCE, &over, 1000).unwrap().len(), 2);
        // the packet id is drawn anew for every datagram
        let drawn: HashSet<u16> = (0..50)
            .map(|_| {
                parse(&datagrams(1, SOURCE, &over, 1000).unwrap()[0])
                    .unwrap()
                    .packet
            })
            .collect();
        assert!(drawn.len() > 1, "{drawn:?}");
        assert!(!drawn.contains(&0));
    }

    #[test]
    fn a_datagram_255_fragments_cannot_carry_is_refused() {
        // 1000 - (8 + 1 + 12) = 979 bytes of payload in each fragment
        let mut payload = vec![0u8; 255 * 979];
        assert_eq!(datagrams(1, SOURCE, &payload, 1000).unwrap().len(), 255);
        payload.push(0);
        let err = datagrams(1, SOURCE, &payload, 1000).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "hysteria2: a datagram longer than 249645 bytes"
        );
        // a header that leaves no room, nothing to carry included
        let long = format!("{}:53", "a".repeat(990));
        for payload in [&b"x"[..], b""] {
            let err = datagrams(1, &long, payload, 1000).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                err.to_string(),
                "hysteria2: the host name is too long for a datagram"
            );
        }
    }

    #[test]
    fn a_message_is_read_as_the_reference_reads_it() {
        // the address's length in two bytes, as QUIC allows
        let mut datagram = vec![0, 0, 0, 7, 0x12, 0x34, 1, 3, 0x40, 9];
        datagram.extend_from_slice(b"1.2.3.4:5payload");
        let message = parse(&datagram).unwrap();
        assert_eq!(ids(&message), (7, 0x1234, 1, 3));
        assert_eq!(&message.address[..], b"1.2.3.4:5");
        assert_eq!(&message.payload[..], b"payload");
        // no payload at all
        let message = parse(&datagram[..datagram.len() - 7]).unwrap();
        assert!(message.payload.is_empty());
        // the longest address the reference reads
        let mut longest = vec![0, 0, 0, 1, 0, 0, 0, 1, 0x48, 0x00];
        longest.extend_from_slice(&[b'a'; 2048]);
        assert_eq!(parse(&longest).unwrap().address.len(), 2048);
        for bad in [
            &[0u8; 7][..],
            // no address length, an address of none or of 2049 bytes
            &[0, 0, 0, 1, 0, 0, 0, 1][..],
            &[0, 0, 0, 1, 0, 0, 0, 1, 0][..],
            &[0, 0, 0, 1, 0, 0, 0, 1, 0x48, 0x01][..],
            // an address cut short
            &[0, 0, 0, 1, 0, 0, 0, 1, 5, b'a', b'b'][..],
            &[0, 0, 0, 1, 0, 0, 0, 1, 0x40][..],
        ] {
            assert!(parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn the_source_is_a_host_and_a_port() {
        for (address, host, port) in [
            ("192.0.2.1:53", "192.0.2.1", 53),
            ("[2001:db8::1]:443", "2001:db8::1", 443),
            ("dns.example:53", "dns.example", 53),
        ] {
            assert_eq!(
                source(address.as_bytes()),
                Some(Target::new(HostName::parse(host), port)),
                "{address}"
            );
        }
        for bad in [
            &b""[..],
            b"192.0.2.1",
            b":53",
            b"192.0.2.1:65536",
            b"192.0.2.1:",
            b"a b.example:53",
            b"a\r\nb:53",
            b"\xff\xfe:53",
        ] {
            assert_eq!(source(bad), None, "{bad:?}");
        }
    }

    fn fragment(packet: u16, fragment: u8, fragments: u8, payload: &[u8]) -> Message {
        Message {
            session: 1,
            packet,
            fragment,
            fragments,
            address: Bytes::from_static(SOURCE.as_bytes()),
            payload: Bytes::copy_from_slice(payload),
        }
    }

    #[test]
    fn fragments_make_their_datagram_whole_in_any_order() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(5, 2, 3, b"ef")), None);
        assert_eq!(reassembly.feed(&fragment(5, 0, 3, b"ab")), None);
        assert_eq!(
            reassembly.feed(&fragment(5, 1, 3, b"cd")).as_deref(),
            Some(&b"abcdef"[..])
        );
        // seen again once whole: no datagram anew
        assert_eq!(reassembly.feed(&fragment(5, 1, 3, b"cd")), None);
        assert_eq!(reassembly.feed(&fragment(5, 0, 3, b"ab")), None);
    }

    #[test]
    fn a_fragment_of_another_datagram_drops_the_one_under_way() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(7, 0, 2, b"old-")), None);
        assert_eq!(reassembly.feed(&fragment(8, 0, 2, b"new-")), None);
        assert_eq!(
            reassembly.feed(&fragment(8, 1, 2, b"one")).as_deref(),
            Some(&b"new-one"[..])
        );
        // the old one's rest, late, makes nothing whole
        assert_eq!(reassembly.feed(&fragment(7, 1, 2, b"late")), None);
        // nor does the same packet id with another count
        assert_eq!(reassembly.feed(&fragment(9, 0, 2, b"a")), None);
        assert_eq!(reassembly.feed(&fragment(9, 1, 3, b"b")), None);
        assert_eq!(reassembly.feed(&fragment(9, 1, 2, b"c")), None);
    }

    #[test]
    fn whole_datagrams_pass_while_one_is_put_together() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(3, 0, 2, b"a")), None);
        // a count of 1 — or 0 — makes a whole datagram, whatever the ids
        assert_eq!(
            reassembly.feed(&fragment(9, 4, 1, b"whole")).as_deref(),
            Some(&b"whole"[..])
        );
        assert_eq!(
            reassembly.feed(&fragment(0, 0, 0, b"zero")).as_deref(),
            Some(&b"zero"[..])
        );
        assert_eq!(
            reassembly.feed(&fragment(3, 1, 2, b"b")).as_deref(),
            Some(&b"ab"[..])
        );
    }

    #[test]
    fn duplicates_and_fragments_past_the_count_are_dropped() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(4, 2, 2, b"x")), None);
        assert_eq!(reassembly.feed(&fragment(4, 0, 2, b"a")), None);
        // the first one stays
        assert_eq!(reassembly.feed(&fragment(4, 0, 2, b"A")), None);
        assert_eq!(reassembly.feed(&fragment(4, 7, 2, b"x")), None);
        assert_eq!(
            reassembly.feed(&fragment(4, 1, 2, b"b")).as_deref(),
            Some(&b"ab"[..])
        );
    }

    #[test]
    fn a_datagram_longer_than_udp_allows_is_dropped() {
        let mut reassembly = Reassembly::default();
        let half = vec![0u8; 32_768];
        assert_eq!(reassembly.feed(&fragment(6, 0, 3, &half)), None);
        assert_eq!(reassembly.feed(&fragment(6, 1, 3, &half)), None);
        // the rest of it too
        assert_eq!(reassembly.feed(&fragment(6, 2, 3, b"x")), None);
        // the longest there is goes through
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.feed(&fragment(6, 0, 2, &vec![0; 65_000])), None);
        let whole = reassembly.feed(&fragment(6, 1, 2, &[0; 535])).unwrap();
        assert_eq!(whole.len(), MAX_DATAGRAM);
    }

    #[test]
    fn session_ids_count_from_1_and_never_give_0() {
        let ids = SessionIds::default();
        assert_eq!((ids.next(), ids.next(), ids.next()), (1, 2, 3));
        let ids = SessionIds(AtomicU32::new(u32::MAX - 1));
        assert_eq!(ids.next(), u32::MAX);
        assert_eq!(ids.next(), 1, "0 passed over");
    }

    /// A connection's sessions with one, `id`, whose inbox takes
    /// `capacity`.
    fn one_session(id: u32, capacity: usize) -> (Sessions, mpsc::Receiver<Packet>) {
        let (inbox, rx) = mpsc::channel(capacity);
        let sessions = Sessions::default();
        let entry = Entry {
            inbox,
            reassembly: Reassembly::default(),
        };
        sessions.lock().unwrap().insert(id, entry);
        (sessions, rx)
    }

    fn whole(id: u32, address: &str, payload: &[u8]) -> Bytes {
        datagrams(id, address, payload, 1200)
            .unwrap()
            .remove(0)
            .into()
    }

    #[test]
    fn a_full_inbox_drops_and_never_waits() {
        let (sessions, mut inbox) = one_session(1, INBOX);
        for i in 0..INBOX as u32 + 10 {
            deliver(&sessions, whole(1, SOURCE, &i.to_be_bytes()));
        }
        let mut got = Vec::new();
        while let Ok((datagram, from)) = inbox.try_recv() {
            assert_eq!(from, Target::new(HostName::parse("192.0.2.1"), 53));
            got.push(u32::from_be_bytes(datagram[..].try_into().unwrap()));
        }
        // the first that came; the rest dropped
        assert_eq!(got, (0..INBOX as u32).collect::<Vec<_>>());
    }

    #[test]
    fn what_no_session_takes_is_dropped() {
        let (sessions, mut inbox) = one_session(1, INBOX);
        // for another session, no message at all, from no host and port
        deliver(&sessions, whole(2, SOURCE, b"elsewhere"));
        deliver(&sessions, Bytes::from_static(&[0, 0, 0, 1, 0, 0]));
        deliver(&sessions, whole(1, "nowhere", b"lost"));
        deliver(&sessions, whole(1, SOURCE, b"kept"));
        assert_eq!(&inbox.try_recv().unwrap().0[..], b"kept");
        assert!(inbox.try_recv().is_err());
    }

    const BOUND: Duration = Duration::from_secs(10);

    /// `hysteria2, 127.0.0.1, <port>, password=pw`, as the configuration
    /// reads it.
    fn spec(port: u16) -> Hysteria2Spec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let line = format!("hysteria2, 127.0.0.1, {port}, password=pw");
        let policy = parse_policy("H", &line, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_hysteria2(&mut r, &mut CommonOpts::default(), &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        spec
    }

    fn spawn_fake() -> FakeHysteria2 {
        FakeHysteria2::spawn(Hysteria2Script {
            password: "pw".to_string(),
            ..Hysteria2Script::default()
        })
    }

    /// The outbound to `fake`, its connections dialled through `connector`.
    fn outbound(fake: &FakeHysteria2, connector: Arc<dyn Connector>) -> Hysteria2Outbound {
        let port = fake.addr().port();
        Hysteria2Outbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), port),
            &spec(port),
            Tristate::Auto,
            &[],
            fake.roots(),
            connector,
        )
        .unwrap()
    }

    fn direct() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    async fn open_carrier(out: &Hysteria2Outbound) -> Arc<dyn PacketSocket> {
        let carrier = tokio::time::timeout(BOUND, out.open_udp(&ConnectOpts::default()))
            .await
            .expect("an outcome within the bound")
            .expect("a carrier");
        Arc::from(carrier)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    /// What the digest target answers for `payload`.
    fn digest(payload: &[u8]) -> Vec<u8> {
        let mut hasher = DefaultHasher::new();
        payload.hash(&mut hasher);
        hasher.finish().to_be_bytes().to_vec()
    }

    /// A loopback UDP target that answers each datagram with a digest of
    /// it: what comes back is small and whole, so it is lost to nothing but
    /// what went out broken.
    async fn digest_target() -> SocketAddr {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let (n, from) = match socket.recv_from(&mut buf).await {
                    Ok(got) => got,
                    // an ICMP "unreachable" for an earlier datagram (Windows)
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(_) => return,
                };
                let _ = socket.send_to(&digest(&buf[..n]), from).await;
            }
        });
        addr
    }

    /// 3000 bytes, three fragments or more, that no other round's or
    /// task's resembles.
    fn payload(round: u8, task: u8) -> Vec<u8> {
        (0..3000u32)
            .map(|i| (i as u8) ^ (round << 4 | task))
            .collect()
    }

    /// The server puts one datagram of a session together at a time (the
    /// reference's `Defragger`): the fragments of two datagrams in turn
    /// lose both. Tasks that send through one carrier at once, on threads
    /// of their own, must see to it that each datagram's fragments go out
    /// together: every datagram reaches the target whole.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_sends_keep_each_datagrams_fragments_together() {
        const TASKS: u8 = 8;
        const ROUNDS: u8 = 5;
        let to = target(digest_target().await);
        let fake = spawn_fake();
        let out = outbound(&fake, direct());
        let carrier = open_carrier(&out).await;
        let mut buf = vec![0u8; 65536];
        for round in 0..ROUNDS {
            let payloads: Vec<Vec<u8>> = (0..TASKS).map(|task| payload(round, task)).collect();
            // all of them at once
            let start = Arc::new(Barrier::new(payloads.len()));
            let mut sending = JoinSet::new();
            for payload in payloads.clone() {
                let (carrier, start, to) = (carrier.clone(), start.clone(), to.clone());
                sending.spawn(async move {
                    start.wait().await;
                    carrier.send_to(&payload, &to).await.unwrap();
                });
            }
            while let Some(sent) = sending.join_next().await {
                sent.unwrap();
            }
            let mut expected: Vec<Vec<u8>> = payloads.iter().map(|p| digest(p)).collect();
            let mut got = Vec::new();
            let deadline = Instant::now() + BOUND;
            while got.len() < expected.len() {
                let Ok(answer) =
                    tokio::time::timeout_at(deadline, carrier.recv_from(&mut buf)).await
                else {
                    break;
                };
                let (n, _) = answer.unwrap();
                got.push(buf[..n].to_vec());
            }
            expected.sort();
            got.sort();
            assert_eq!(got, expected, "round {round}");
        }
    }

    /// Dials directly, leads where it is told to, and counts the times it
    /// is asked where it leads.
    struct Counted {
        inner: DirectConnector,
        route: Mutex<Arc<u8>>,
        asked: AtomicUsize,
    }

    impl Counted {
        fn new() -> Counted {
            Counted {
                inner: DirectConnector::new(Arc::new(SystemResolve)),
                route: Mutex::new(Arc::new(1)),
                asked: AtomicUsize::new(0),
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::Relaxed)
        }
    }

    impl Connector for Counted {
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
            self.inner.connect_udp(target, opts)
        }

        fn route(&self) -> Option<Route> {
            self.asked.fetch_add(1, Ordering::Relaxed);
            Some(Route::of(&self.route.lock().unwrap()))
        }
    }

    /// Where the policy leads can cost a group's resolution: a carrier asks
    /// at most once a second, however many datagrams it sends meanwhile.
    #[tokio::test]
    async fn the_route_is_compared_at_most_once_a_second_on_udp() {
        let to = target(digest_target().await);
        let fake = spawn_fake();
        let counted = Arc::new(Counted::new());
        let out = outbound(&fake, counted.clone());
        let carrier = open_carrier(&out).await;
        let before = counted.asked();
        for i in 0..50u8 {
            carrier.send_to(&[i], &to).await.unwrap();
        }
        let asked = counted.asked() - before;
        assert!(asked <= 3, "asked {asked} times for 50 datagrams");
    }

    /// A carrier finds out for itself, at its next comparison of the
    /// route, that the policy leads elsewhere now: its datagrams then go on
    /// a new connection, under the session id it had. Until then — a second
    /// at most — they go on the one before.
    #[tokio::test]
    async fn a_carrier_finds_a_changed_route_by_itself() {
        let to = target(digest_target().await);
        let fake = spawn_fake();
        let counted = Arc::new(Counted::new());
        let out = outbound(&fake, counted.clone());
        let carrier = open_carrier(&out).await;
        carrier.send_to(b"first", &to).await.unwrap();
        *counted.route.lock().unwrap() = Arc::new(2);
        let deadline = Instant::now() + BOUND;
        while !fake.udp_messages().iter().any(|m| m.connection == 1) {
            assert!(
                Instant::now() < deadline,
                "the carrier never left its connection"
            );
            carrier.send_to(b"probe", &to).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let messages = fake.udp_messages();
        assert!(messages.iter().all(|m| m.session == 1), "{messages:?}");
        assert_eq!(fake.connections(), 2);
    }
}
