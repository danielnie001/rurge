//! Shadowsocks over UDP (phase 2 M6 design 3.3): every datagram goes to the
//! server by itself, sealed with the target's SOCKS5 address in front of
//! the payload; the server's answers name their source the same way. One
//! carrier to the server (`udp-port`, else the server's port) carries every
//! target of an association: full cone.
//!
//! - `none`: the address and the payload as they are.
//! - AEAD: a random salt of its own per packet, the session key under it,
//!   the nonce all zeros.
//! - SS 2022 (SIP022 3.2, SIP023): each carrier is a client session with a
//!   random id and packet ids counting from zero; the 16-byte separate
//!   header (session id, packet id) is one AES block under the first key,
//!   followed by the identity headers of a multi-user key, then the body
//!   sealed with a key of the user key and the session id, its nonce the
//!   last 12 bytes of the separate header. The server's packets come from
//!   sessions of its own and are checked for type, time, the client session
//!   they name and, per server session, replays.
//!
//! What does not decrypt or does not check out is dropped, as a socket
//! drops what it cannot use: a debug line, never the payload.

use super::cipher::{AeadCipher, AeadKind, MasterKey, TAG, aes_decrypt_block, aes_encrypt_block};
use super::kdf;
use super::s2022::{self, Identity};
use crate::OutboundError;
use crate::addr::{AddrError, parse_socks_addr, socks_addr};
use crate::socks5::from_relay;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedPacketSocket, PacketSocket, Target};
use std::io;
use std::net::IpAddr;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const CLIENT_PACKET: u8 = 0;
const SERVER_PACKET: u8 = 1;
/// The separate header: session id and packet id.
const SEPARATE: usize = 16;
/// A server body before its address: type, timestamp, the client's session
/// id, the padding's length.
const SERVER_FIXED: usize = 1 + 8 + 8 + 2;
/// Server sessions whose replay windows a carrier keeps; a new one pushes
/// out the oldest (a server has one per client session, a new one after a
/// restart).
const SERVER_SESSIONS: usize = 8;

/// How a method seals datagrams: one per policy, shared by its carriers.
/// No `Debug`: it holds keys.
pub(crate) enum Packets {
    Plain,
    Aead(Arc<MasterKey>),
    S2022(Arc<Keys2022>),
}

/// SS 2022's keys for UDP. No `Debug`.
pub(crate) struct Keys2022 {
    kind: AeadKind,
    /// The separate header's key: the first identity key, or the only key.
    first: Vec<u8>,
    /// The bodies' keys derive from it; the server's separate headers are
    /// under it.
    user: Vec<u8>,
    identity: Identity,
}

impl Keys2022 {
    /// `keys` as written: identity keys first, the user key last; never empty.
    pub(crate) fn new(kind: AeadKind, keys: &[Vec<u8>]) -> Keys2022 {
        Keys2022 {
            kind,
            first: keys[0].clone(),
            user: keys[keys.len() - 1].clone(),
            identity: Identity::new(keys),
        }
    }

    /// The cipher of the bodies of `session`'s packets.
    fn body_cipher(&self, session: &[u8; 8]) -> AeadCipher {
        AeadCipher::new(self.kind, &kdf::session_subkey_2022(&self.user, session))
    }

    /// Packet `packet` of client session `session` (its body cipher
    /// `cipher`) to `addr`, with `padding` bytes of padding.
    #[allow(clippy::too_many_arguments)]
    fn seal(
        &self,
        session: &[u8; 8],
        cipher: &AeadCipher,
        packet: u64,
        now: u64,
        padding: usize,
        addr: &[u8],
        payload: &[u8],
    ) -> Vec<u8> {
        let mut separate = [0u8; SEPARATE];
        separate[..8].copy_from_slice(session);
        separate[8..].copy_from_slice(&packet.to_be_bytes());
        let mut out = Vec::with_capacity(64 + padding + addr.len() + payload.len());
        let mut header = separate;
        aes_encrypt_block(&self.first, &mut header);
        out.extend_from_slice(&header);
        out.extend_from_slice(&self.identity.packet_headers(&separate));
        let body = out.len();
        out.push(CLIENT_PACKET);
        out.extend_from_slice(&now.to_be_bytes());
        let len = u16::try_from(padding).expect("at most 900 bytes of padding");
        out.extend_from_slice(&len.to_be_bytes());
        // zeros: sealed like the rest
        out.resize(out.len() + padding, 0);
        out.extend_from_slice(addr);
        out.extend_from_slice(payload);
        let tag = cipher.seal_in_place(&separate[4..], &mut out[body..]);
        out.extend_from_slice(&tag);
        out
    }
}

/// An AEAD packet for `addr` under `salt`.
fn seal_aead(key: &MasterKey, salt: &[u8], addr: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut plain = Vec::with_capacity(addr.len() + payload.len());
    plain.extend_from_slice(addr);
    plain.extend_from_slice(payload);
    let mut out = Vec::with_capacity(salt.len() + plain.len() + TAG);
    out.extend_from_slice(salt);
    // a fresh session per packet: its first nonce is all zeros
    key.session(salt).seal(&plain, &mut out);
    out
}

/// The source of a packet whose plaintext `plain` starts at `start` of the
/// packet, and where its payload lies in the packet.
fn source(plain: &[u8], start: usize) -> Result<(Target, Range<usize>), &'static str> {
    let (from, used) = parse_socks_addr(plain).ok_or("no address")?;
    Ok((from, start + used..start + plain.len()))
}

/// An AEAD packet opened in place.
fn open_aead(key: &MasterKey, packet: &mut [u8]) -> Result<(Target, Range<usize>), &'static str> {
    let salt_len = key.salt_len();
    if packet.len() < salt_len + TAG {
        return Err("too short");
    }
    let (salt, sealed) = packet.split_at_mut(salt_len);
    let n = key.session(salt).open(sealed).ok_or("does not decrypt")?;
    source(&sealed[..n], salt_len)
}

/// Packet ids already seen from one server session: the last 8128 of them
/// (SIP022 3.2.4; a ring of 64-bit blocks as WireGuard's replay filter).
struct Window {
    last: u64,
    ring: [u64; RING_BLOCKS],
}

const BLOCK_BITS: u64 = 64;
const RING_BLOCKS: usize = 128;
/// Ids this far behind the newest are still told apart.
const WINDOW: u64 = (RING_BLOCKS as u64 - 1) * BLOCK_BITS;

impl Window {
    fn new() -> Window {
        Window {
            last: 0,
            ring: [0; RING_BLOCKS],
        }
    }

    /// Whether `packet` is new (then it is seen from now on): not seen
    /// before, and not too far behind the newest.
    fn accept(&mut self, packet: u64) -> bool {
        let block = packet / BLOCK_BITS;
        if packet > self.last {
            // the blocks the window moves over are cleared
            let current = self.last / BLOCK_BITS;
            let moved = (block - current).min(RING_BLOCKS as u64);
            for i in 1..=moved {
                self.ring[((current + i) % RING_BLOCKS as u64) as usize] = 0;
            }
            self.last = packet;
        } else if self.last - packet > WINDOW {
            return false;
        }
        let index = (block % RING_BLOCKS as u64) as usize;
        let bit = 1u64 << (packet % BLOCK_BITS);
        let old = self.ring[index];
        self.ring[index] = old | bit;
        old & bit == 0
    }
}

/// One server session a carrier heard from.
struct ServerSession {
    id: [u8; 8],
    cipher: AeadCipher,
    window: Window,
}

/// A carrier's SS 2022 client session. No `Debug`.
struct Session {
    id: [u8; 8],
    cipher: AeadCipher,
    /// The next packet id. A session sends 2^64 packets long after its
    /// carrier is gone.
    next: AtomicU64,
    /// Oldest first.
    servers: Mutex<Vec<ServerSession>>,
}

impl Session {
    /// A server packet opened in place: checked, then counted against its
    /// session's replay window.
    fn open(
        &self,
        keys: &Keys2022,
        packet: &mut [u8],
        now: u64,
    ) -> Result<(Target, Range<usize>), &'static str> {
        if packet.len() < SEPARATE + SERVER_FIXED + TAG {
            return Err("too short");
        }
        let (header, sealed) = packet.split_at_mut(SEPARATE);
        let mut separate: [u8; SEPARATE] = (&*header).try_into().expect("16 bytes");
        // the server's separate headers are under the user key: no identity
        // headers come back
        aes_decrypt_block(&keys.user, &mut separate);
        let id: [u8; 8] = separate[..8].try_into().expect("8 bytes");
        let packet_id = u64::from_be_bytes(separate[8..].try_into().expect("8 bytes"));
        let (body, tag) = sealed.split_at_mut(sealed.len() - TAG);
        let tag: &[u8; TAG] = (&*tag).try_into().expect("16 bytes");
        let mut servers = self.servers.lock().expect("server sessions");
        let known = servers.iter().position(|s| s.id == id);
        let fresh = match known {
            Some(_) => None,
            None => Some(keys.body_cipher(&id)),
        };
        let cipher = match (known, &fresh) {
            (Some(i), _) => &servers[i].cipher,
            (None, fresh) => fresh.as_ref().expect("a new session's cipher"),
        };
        if !cipher.open_in_place(&separate[4..], body, tag) {
            return Err("does not decrypt");
        }
        if body[0] != SERVER_PACKET {
            return Err("not a server's packet");
        }
        let time = u64::from_be_bytes(body[1..9].try_into().expect("8 bytes"));
        if time.abs_diff(now) > s2022::TIME_WINDOW {
            return Err("the server's clock differs from ours by more than 30 seconds");
        }
        if body[9..17] != self.id {
            return Err("for another client session");
        }
        let padding = usize::from(u16::from_be_bytes([body[17], body[18]]));
        let start = SERVER_FIXED + padding;
        let plain = body.get(start..).ok_or("no address")?;
        let (from, range) = source(plain, SEPARATE + start)?;
        // only a packet that checked out moves the window
        let accepted = match known {
            Some(i) => servers[i].window.accept(packet_id),
            None => {
                if servers.len() == SERVER_SESSIONS {
                    servers.remove(0);
                }
                let mut window = Window::new();
                window.accept(packet_id);
                servers.push(ServerSession {
                    id,
                    cipher: fresh.expect("a new session's cipher"),
                    window,
                });
                true
            }
        };
        if !accepted {
            return Err("a replay");
        }
        Ok((from, range))
    }
}

/// One association's carrier to the server. No `Debug`.
pub(crate) struct SsUdp {
    /// The server as looked up when the carrier opened.
    server: Target,
    /// Its address, when the carrier resolved it to one: only datagrams from
    /// there are the server's. A chained carrier keeps names.
    server_ip: Option<IpAddr>,
    socket: BoxedPacketSocket,
    sealing: Sealing,
    now: fn() -> u64,
}

/// How one carrier seals: SS 2022 always with a client session of its own.
enum Sealing {
    Plain,
    Aead(Arc<MasterKey>),
    S2022 {
        keys: Arc<Keys2022>,
        session: Box<Session>,
    },
}

fn no_randomness() -> OutboundError {
    OutboundError::Proxy("ss: no randomness available".to_string())
}

impl SsUdp {
    /// The carrier to `server` through `socket`. The server is looked up
    /// once, here.
    pub(crate) async fn open(
        socket: BoxedPacketSocket,
        server: &Target,
        packets: Arc<Packets>,
        now: fn() -> u64,
    ) -> Result<SsUdp, OutboundError> {
        let server = socket.resolve(server).await.map_err(|e| {
            OutboundError::Proxy(format!("ss: cannot look up the server for UDP: {e}"))
        })?;
        let server_ip = match server.host {
            HostName::Ip(ip) => Some(ip),
            HostName::Domain(_) => None,
        };
        let sealing = match &*packets {
            Packets::Plain => Sealing::Plain,
            Packets::Aead(key) => Sealing::Aead(key.clone()),
            Packets::S2022(keys) => {
                let mut id = [0u8; 8];
                getrandom::fill(&mut id).map_err(|_| no_randomness())?;
                Sealing::S2022 {
                    keys: keys.clone(),
                    session: Box::new(Session {
                        id,
                        cipher: keys.body_cipher(&id),
                        next: AtomicU64::new(0),
                        servers: Mutex::new(Vec::new()),
                    }),
                }
            }
        };
        Ok(SsUdp {
            server,
            server_ip,
            socket,
            sealing,
            now,
        })
    }

    fn seal(&self, addr: &[u8], payload: &[u8]) -> io::Result<Vec<u8>> {
        let random = |_| io::Error::other("ss: no randomness available");
        Ok(match &self.sealing {
            Sealing::Aead(key) => {
                let mut salt = vec![0u8; key.salt_len()];
                getrandom::fill(&mut salt).map_err(random)?;
                seal_aead(key, &salt, addr, payload)
            }
            Sealing::S2022 { keys, session } => {
                let packet = session.next.fetch_add(1, Ordering::Relaxed);
                let padding = s2022::padding_len(payload.len()).map_err(random)?;
                keys.seal(
                    &session.id,
                    &session.cipher,
                    packet,
                    (self.now)(),
                    padding,
                    addr,
                    payload,
                )
            }
            Sealing::Plain => [addr, payload].concat(),
        })
    }

    fn unseal(&self, packet: &mut [u8]) -> Result<(Target, Range<usize>), &'static str> {
        match &self.sealing {
            Sealing::Aead(key) => open_aead(key, packet),
            Sealing::S2022 { keys, session } => session.open(keys, packet, (self.now)()),
            Sealing::Plain => source(packet, 0),
        }
    }
}

impl PacketSocket for SsUdp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let addr = socks_addr(to).map_err(|e| {
                io::Error::other(match e {
                    AddrError::Unsendable => "ss: the host name cannot be sent to the server",
                    AddrError::TooLong => "ss: the host name is longer than 255 bytes",
                })
            })?;
            let packet = self.seal(&addr, buf)?;
            self.socket.send_to(&packet, &self.server).await
        })
    }

    /// `buf` takes the whole packet, the payload's address and the
    /// protocol's headers too: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            loop {
                let (n, sender) = self.socket.recv_from(buf).await?;
                if !from_relay(self.server_ip, &sender) {
                    continue;
                }
                match self.unseal(&mut buf[..n]) {
                    Ok((from, payload)) => {
                        let len = payload.len();
                        buf.copy_within(payload, 0);
                        return Ok((len, from));
                    }
                    Err(why) => {
                        tracing::debug!("ss: a UDP packet from the server was dropped: {why}")
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    const NOW: u64 = 1_700_000_000;

    /// 127.0.0.1:53, the targets' address in the vectors.
    const QUERY_TO: [u8; 7] = [1, 127, 0, 0, 1, 0, 53];

    fn eight_eight() -> Target {
        Target::new(HostName::parse("8.8.8.8"), 53)
    }

    /// Computed with Python's `hashlib` / `hmac` and `cryptography`'s
    /// AES-GCM and ChaCha20-Poly1305: password "password", salt `00 01 …`
    /// out and `40 41 …` back, the answer from 8.8.8.8:53.
    #[test]
    fn aead_packets_are_the_known_answers() {
        for (kind, request, answer) in [
            (
                AeadKind::Aes128Gcm,
                "000102030405060708090a0b0c0d0e0f5d51eb7401796b37cf59a6924a104e437bf11e7f91878ac86cd6979b",
                "404142434445464748494a4b4c4d4e4fd3aff9990f2d96949689928d39629a307b8f95ba59bcb26f84533ef88f",
            ),
            (
                AeadKind::Aes256Gcm,
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f7fda93a3e4da07282516f07cd043bdbe38981f72ebd4fa923ff5e529",
                "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5fa1934dbc2a99e8a6b43db9be547e396ba796e9e9fd20ad591c2a321682",
            ),
            (
                AeadKind::ChaCha20Poly1305,
                "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1fac378ef1ffba4729e383117a3abb36719c11848971cb61c67ac52c96",
                "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f560961fc5e043ae2f29829b22dbac6afccd63ae2aa2efeb6e1b2534f56",
            ),
        ] {
            let key = MasterKey::from_password(kind, "password");
            let salt: Vec<u8> = (0..kind.key_len() as u8).collect();
            assert_eq!(
                seal_aead(&key, &salt, &QUERY_TO, b"query"),
                hex(request),
                "{kind:?}"
            );
            let mut packet = hex(answer);
            let (from, payload) = open_aead(&key, &mut packet).unwrap();
            assert_eq!((from, &packet[payload]), (eight_eight(), &b"answer"[..]));
            let mut flipped = hex(answer);
            flipped[40] ^= 1;
            assert_eq!(
                open_aead(&key, &mut flipped).err(),
                Some("does not decrypt")
            );
            assert_eq!(open_aead(&key, &mut [0u8; 20]).err(), Some("too short"));
        }
    }

    /// Keys for the vectors: aes-128 with one identity key (`00 …`) before
    /// the user key (`20 …`), aes-256 with the single key `20 …`.
    fn keys_2022() -> [(AeadKind, Vec<Vec<u8>>); 2] {
        [
            (
                AeadKind::Aes128Gcm,
                vec![(0..16).collect(), (0x20..0x30).collect()],
            ),
            (AeadKind::Aes256Gcm, vec![(0x20..0x40).collect()]),
        ]
    }

    const CLIENT_SESSION: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

    fn session(keys: &Keys2022) -> Session {
        Session {
            id: CLIENT_SESSION,
            cipher: keys.body_cipher(&CLIENT_SESSION),
            next: AtomicU64::new(0),
            servers: Mutex::new(Vec::new()),
        }
    }

    /// Computed with a BLAKE3 written in Python from its specification and
    /// `cryptography`'s AES-ECB / AES-GCM: client session `01 … 08`, packet
    /// 5, no padding; the answer from server session `a1 … a8`, packet 9,
    /// three bytes of padding, from 8.8.8.8:53; time 1 700 000 000.
    #[test]
    fn ss_2022_packets_are_the_known_answers() {
        let vectors = [
            (
                "8e27435dfc522793dbd676d13a03be50130d1ed6480648b9bd16081cec953f96527df4ce550b12f9075b09f9b72cfa5a62cb9b6a916fe1ee1e6f8fb1f8937e9ae9ac17284a7566",
                "badae3d7318fe1f1771801dcec420eeefa394afd953ad174e0d8171afe4dd7cba7ea77116f0e9d0e64052fd7259c52598586497335744f7423071adb0dc72b2a2a63a0",
            ),
            (
                "a05d60bef664d31d3700189cd527895a5027f8a748e831207a32aef447d7782981382f558654abc07b36ecc39f961b53666bc4f6f2d3af",
                "a944fe53b01aac08fe3f5c095862f076bc6c1a168b81bfcc4b1eaeba3ab16bdc89bc12709667e2e81213fcb068e92bcd3d472c2a315bca128b932910bdbd28bc9d83de",
            ),
        ];
        for ((kind, keys), (request, answer)) in keys_2022().into_iter().zip(vectors) {
            let keys = Keys2022::new(kind, &keys);
            let session = session(&keys);
            let sealed = keys.seal(
                &CLIENT_SESSION,
                &session.cipher,
                5,
                NOW,
                0,
                &QUERY_TO,
                b"query",
            );
            assert_eq!(sealed, hex(request), "{kind:?}");
            let mut packet = hex(answer);
            let (from, payload) = session.open(&keys, &mut packet, NOW).unwrap();
            assert_eq!((from, &packet[payload]), (eight_eight(), &b"answer"[..]));
            // the same packet again is a replay
            let mut again = hex(answer);
            assert_eq!(session.open(&keys, &mut again, NOW).err(), Some("a replay"));
        }
    }

    #[test]
    fn a_2022_answer_is_checked_before_it_counts() {
        let (kind, keys) = keys_2022().into_iter().nth(1).unwrap();
        let keys = Keys2022::new(kind, &keys);
        let answer = "a944fe53b01aac08fe3f5c095862f076bc6c1a168b81bfcc4b1eaeba3ab16bdc89bc12709667e2e81213fcb068e92bcd3d472c2a315bca128b932910bdbd28bc9d83de";
        let open = |session: &Session, packet: &mut [u8], now| {
            session.open(&keys, packet, now).map(|_| ()).err()
        };
        let session = session(&keys);
        // off the clock: dropped, and the window has not moved
        assert_eq!(
            open(&session, &mut hex(answer), NOW + 31),
            Some("the server's clock differs from ours by more than 30 seconds")
        );
        assert_eq!(
            open(&session, &mut hex(answer), NOW - 30),
            None,
            "30 s is fine"
        );
        let mut flipped = hex(answer);
        flipped[20] ^= 1;
        assert_eq!(open(&session, &mut flipped, NOW), Some("does not decrypt"));
        assert_eq!(open(&session, &mut [0u8; 50], NOW), Some("too short"));
        // an answer for someone else's session
        let other = Session {
            id: [9; 8],
            ..self::session(&keys)
        };
        assert_eq!(
            open(&other, &mut hex(answer), NOW),
            Some("for another client session")
        );
        // our own request played back is no answer (a single key: its
        // separate header is under the key the answers use)
        let mut request = keys.seal(
            &[9; 8],
            &keys.body_cipher(&[9; 8]),
            1,
            NOW,
            0,
            &QUERY_TO,
            b"q",
        );
        assert_eq!(
            open(&other, &mut request, NOW),
            Some("not a server's packet")
        );
    }

    #[test]
    fn padding_and_identity_headers_take_their_places() {
        let (kind, keys) = keys_2022().into_iter().next().unwrap();
        let keys = Keys2022::new(kind, &keys);
        let session = session(&keys);
        let sealed = keys.seal(&CLIENT_SESSION, &session.cipher, 7, NOW, 3, &QUERY_TO, b"");
        // separate header, one identity header, body (11 + 3 + 7), tag
        assert_eq!(sealed.len(), 16 + 16 + 21 + TAG);
        let mut separate: [u8; 16] = sealed[..16].try_into().unwrap();
        aes_decrypt_block(&keys.first, &mut separate);
        assert_eq!(separate[..8], CLIENT_SESSION);
        assert_eq!(separate[8..], 7u64.to_be_bytes());
        let mut identity: [u8; 16] = sealed[16..32].try_into().unwrap();
        aes_decrypt_block(&keys.first, &mut identity);
        for (byte, mask) in identity.iter_mut().zip(separate) {
            *byte ^= mask;
        }
        assert_eq!(identity, kdf::identity_hash(&keys.user));
        let mut body = sealed[32..].to_vec();
        let (plain, tag) = body.split_at_mut(21);
        assert!(
            session
                .cipher
                .open_in_place(&separate[4..], plain, (&*tag).try_into().unwrap())
        );
        assert_eq!(plain[..1], [CLIENT_PACKET]);
        assert_eq!(plain[1..9], NOW.to_be_bytes());
        assert_eq!(plain[9..14], [0, 3, 0, 0, 0]);
        assert_eq!(plain[14..], QUERY_TO);
    }

    #[test]
    fn a_plain_packet_is_the_address_and_the_payload() {
        let mut packet = [&[1, 8, 8, 8, 8, 0, 53][..], b"answer"].concat();
        let (from, payload) = source(&packet, 0).unwrap();
        assert_eq!((from, &packet[payload]), (eight_eight(), &b"answer"[..]));
        packet[0] = 9;
        assert_eq!(source(&packet, 0).err(), Some("no address"));
    }

    #[test]
    fn the_window_takes_each_id_once_in_any_order() {
        let mut window = Window::new();
        assert!(window.accept(0), "the first id");
        assert!(!window.accept(0), "a duplicate");
        for id in 1..100 {
            assert!(window.accept(id));
        }
        // out of order, within the window
        assert!(window.accept(150));
        assert!(window.accept(120));
        assert!(!window.accept(120));
        assert!(window.accept(149));
        assert!(!window.accept(99));
    }

    #[test]
    fn the_window_forgets_what_falls_behind() {
        let mut window = Window::new();
        assert!(window.accept(10_000));
        assert!(window.accept(10_000 - WINDOW), "at the window's edge");
        assert!(!window.accept(10_000 - WINDOW - 1), "too old");
        assert!(!window.accept(0), "too old");
        // a jump far ahead clears the ring: ids that shared a block with
        // old ones are new
        assert!(window.accept(10_000 + 64 * 1000));
        assert!(window.accept(10_000 + 64 * 1000 - 64 * 3));
        assert!(!window.accept(10_000 + 64 * 1000 - 64 * 3));
        assert!(!window.accept(10_000), "far behind now");
    }

    #[test]
    fn a_window_jump_of_every_size_keeps_the_newest() {
        for jump in [1, 63, 64, 65, 127 * 64, 128 * 64, 129 * 64, u64::MAX / 2] {
            let mut window = Window::new();
            for id in 0..200 {
                assert!(window.accept(id));
            }
            let top = 199 + jump;
            assert!(window.accept(top), "{jump}");
            assert!(!window.accept(top), "{jump}");
            if jump <= WINDOW {
                assert!(!window.accept(199), "{jump}: still seen");
            }
            if jump > 1 {
                assert!(window.accept(top - 1), "{jump}");
            }
        }
    }
}
