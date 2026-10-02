//! The fake's obfuscation (phase 2 M7 design 5.4), on its server socket, as
//! the reference server has it (`extras/obfs`): Salamander on every packet,
//! and with Gecko the long-header packets in fragments, either way. Written
//! apart from the client's — nothing shared but the protocol —, so that
//! each checks the other: the key through BLAKE2b's other interface, the
//! fragments put together as the reference does (at most 8 messages under
//! way per source, more refused), what it opens queued for quinn.

use blake2::Blake2bVar;
use blake2::digest::{Update, VariableOutput};
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

/// A fragment of the client's, as the fake read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedFragment {
    pub message: u8,
    pub index: u8,
    pub count: u8,
    /// Its UDP datagram's length, the salt included.
    pub datagram: usize,
}

/// A Gecko message being put together.
struct Message {
    count: u8,
    chunks: HashMap<u8, Vec<u8>>,
    until: Instant,
}

/// The fake's server socket, obfuscated: Salamander keyed with `password`,
/// Gecko over it when `gecko`.
pub(super) struct ServerObfs {
    inner: Arc<dyn AsyncUdpSocket>,
    password: Vec<u8>,
    gecko: bool,
    next_message: AtomicU8,
    /// Every Gecko fragment of the client's it read, for the tests.
    heard: Arc<Mutex<Vec<RecordedFragment>>>,
    state: Mutex<State>,
}

struct State {
    read: Vec<u8>,
    /// Packets opened, with how they came, waiting for the endpoint.
    ready: VecDeque<(Vec<u8>, RecvMeta)>,
    /// By source and message.
    messages: HashMap<(SocketAddr, u8), Message>,
}

impl ServerObfs {
    pub(super) fn new(
        inner: Arc<dyn AsyncUdpSocket>,
        password: &str,
        gecko: bool,
        heard: Arc<Mutex<Vec<RecordedFragment>>>,
    ) -> ServerObfs {
        let read = vec![0; 2048 * inner.max_receive_segments()];
        ServerObfs {
            inner,
            password: password.as_bytes().to_vec(),
            gecko,
            next_message: AtomicU8::new(0),
            heard,
            state: Mutex::new(State {
                read,
                ready: VecDeque::new(),
                messages: HashMap::new(),
            }),
        }
    }

    /// `BLAKE2b-256(password ‖ salt)`.
    fn key(&self, salt: &[u8]) -> [u8; 32] {
        let mut hash = Blake2bVar::new(32).expect("a digest length of 32");
        hash.update(&self.password);
        hash.update(salt);
        let mut key = [0; 32];
        hash.finalize_variable(&mut key).expect("32 bytes");
        key
    }

    fn seal(&self, payload: &[u8]) -> Vec<u8> {
        let mut salt = [0; 8];
        getrandom::fill(&mut salt).expect("randomness");
        let key = self.key(&salt);
        let mut datagram = salt.to_vec();
        datagram.extend(payload.iter().enumerate().map(|(i, b)| b ^ key[i % 32]));
        datagram
    }

    fn unseal(&self, datagram: &[u8]) -> Option<Vec<u8>> {
        if datagram.len() <= 8 {
            return None;
        }
        let key = self.key(&datagram[..8]);
        Some(
            datagram[8..]
                .iter()
                .enumerate()
                .map(|(i, b)| b ^ key[i % 32])
                .collect(),
        )
    }

    /// A long-header packet in 2 to 8 fragments, each padded to a datagram
    /// of 512 to 1200 bytes where it can be.
    fn fragments(&self, packet: &[u8]) -> Vec<Vec<u8>> {
        let count = 2 + random(7) as u8;
        let message = self.next_message.fetch_add(1, Ordering::Relaxed);
        let size = packet.len() / usize::from(count);
        (0..count)
            .map(|index| {
                let start = usize::from(index) * size;
                let chunk = if index == count - 1 {
                    &packet[start..]
                } else {
                    &packet[start..start + size]
                };
                let bare = 8 + 5 + chunk.len();
                let least = bare.max(512);
                let padding = if least > 1200 {
                    0
                } else {
                    least - bare + random(1200 - least + 1)
                };
                let mut frame = vec![0x80, message, index << 4 | count];
                frame.extend_from_slice(&(padding as u16).to_be_bytes());
                let mut pad = vec![0; padding];
                getrandom::fill(&mut pad).expect("randomness");
                frame.extend_from_slice(&pad);
                frame.extend_from_slice(chunk);
                self.seal(&frame)
            })
            .collect()
    }

    /// Takes one datagram the client sent: what it opens to goes to quinn,
    /// once whole.
    fn take(&self, datagram: &[u8], meta: RecvMeta, state: &mut State) {
        let Some(plain) = self.unseal(datagram) else {
            return;
        };
        if !self.gecko || plain[0] & 0x80 == 0 {
            state.ready.push_back((plain, meta));
            return;
        }
        if plain.len() < 5 {
            return;
        }
        let (message, index, count) = (plain[1], plain[2] >> 4, plain[2] & 0x0f);
        let padding = usize::from(u16::from_be_bytes([plain[3], plain[4]]));
        if !(2..=8).contains(&count) || index >= count || plain.len() < 5 + padding {
            return;
        }
        self.heard.lock().expect("heard").push(RecordedFragment {
            message,
            index,
            count,
            datagram: datagram.len(),
        });
        let now = Instant::now();
        state.messages.retain(|_, message| message.until > now);
        let key = (meta.addr, message);
        if !state.messages.contains_key(&key) {
            let under_way = state
                .messages
                .keys()
                .filter(|(source, _)| *source == meta.addr)
                .count();
            if under_way >= 8 {
                return;
            }
            state.messages.insert(
                key,
                Message {
                    count,
                    chunks: HashMap::new(),
                    until: now + Duration::from_secs(8),
                },
            );
        }
        let message = state.messages.get_mut(&key).expect("the message");
        if message.count != count {
            return;
        }
        message
            .chunks
            .entry(index)
            .or_insert_with(|| plain[5 + padding..].to_vec());
        if message.chunks.len() == usize::from(count) {
            let message = state.messages.remove(&key).expect("the message");
            let packet = (0..count)
                .flat_map(|index| message.chunks[&index].clone())
                .collect();
            state.ready.push_back((packet, meta));
        }
    }
}

/// A random number below `n`.
fn random(n: usize) -> usize {
    getrandom::u32().expect("randomness") as usize % n
}

impl fmt::Debug for ServerObfs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerObfs")
            .field("gecko", &self.gecko)
            .finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for ServerObfs {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit<'_>) -> io::Result<()> {
        let datagrams = if self.gecko && transmit.contents.first().is_some_and(|b| b & 0x80 != 0) {
            self.fragments(transmit.contents)
        } else {
            vec![self.seal(transmit.contents)]
        };
        for (n, datagram) in datagrams.iter().enumerate() {
            let sent = self.inner.try_send(&Transmit {
                contents: datagram,
                segment_size: None,
                ..transmit.clone()
            });
            // the first may wait for room; a fragment after it may not
            if n == 0 {
                sent?;
            }
        }
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut guard = self.state.lock().expect("obfs");
        let state = &mut *guard;
        loop {
            if let Some((packet, came)) = state.ready.pop_front() {
                if packet.len() > bufs[0].len() {
                    continue;
                }
                bufs[0][..packet.len()].copy_from_slice(&packet);
                meta[0] = RecvMeta {
                    len: packet.len(),
                    stride: packet.len(),
                    ..came
                };
                return Poll::Ready(Ok(1));
            }
            let mut read = [RecvMeta::default()];
            let n = ready!(self.inner.poll_recv(
                cx,
                &mut [IoSliceMut::new(&mut state.read)],
                &mut read
            ))?;
            if n == 0 {
                continue;
            }
            let read = read[0];
            let bytes = state.read[..read.len].to_vec();
            for datagram in bytes.chunks(read.stride.max(1)) {
                self.take(datagram, read, state);
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}
