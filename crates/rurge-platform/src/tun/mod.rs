//! The virtual adapter (M4 design 8.1): whole IP packets in and out, with no
//! offloads, and the name the kernel gave it.
//!
//! Every platform's module gives the same four things, and this one thing
//! beside them:
//!
//! ```text
//! impl Device {
//!     /// Opens the adapter: the device, and the adapter the steps name (its
//!     /// name, and on Windows its interface index). Errors go through
//!     /// `frame::explained` in the platform's module, except the ones before
//!     /// the adapter exists (wintun.dll's own).
//!     pub(super) fn open(spec: &DeviceSpec) -> io::Result<(Device, Adapter)>;
//!     /// Waits until a packet is there, then reads it into `buf`: the IP
//!     /// packet's length. A datagram that is no IP packet is skipped here; an
//!     /// empty read is the device gone, and an error ([`frame::packet_len`]).
//!     pub(super) async fn read(&self, buf: &mut [u8]) -> io::Result<usize>;
//!     /// Reads a packet without waiting: `WouldBlock` when none is there.
//!     pub(super) fn try_read(&self, buf: &mut [u8]) -> io::Result<usize>;
//!     /// Writes one packet without waiting.
//!     pub(super) fn try_write(&self, packet: &[u8]) -> io::Result<()>;
//! }
//! /// Whether a failed write means the system had no room for the packet (M4c
//! /// plan P14): dropped and counted, and the TUN goes on.
//! pub(super) fn no_room(err: &io::Error) -> bool;
//! ```
//!
//! There is one platform's module at a time, chosen by the `cfg`s below; a
//! platform that has none in this version gets `unsupported` (nothing this
//! version is built for).
//!
//! Deciding when a packet has no room, counting the dropped ones and the two
//! log lines are [`write_all`]'s here, without a platform in them. Reading a
//! packet that is there and then moving what else is waiting is [`drain`]'s.
//!
//! `TunDevice::open` is what the platform's `System::open` calls, and no test
//! calls it (M4c plan P19). It needs a tokio runtime: the descriptor is
//! registered with the reactor of whichever thread's runtime is current, and a
//! `spawn_blocking` thread has one.

use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::enhanced::Adapter;

pub mod dll;
pub mod frame;
pub mod reader;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::{Device as Imp, no_room};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::{Device as Imp, no_room};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::{Device as Imp, no_room};

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
use unsupported::{Device as Imp, no_room};

/// The buffer a read goes into (M4c plan P14): larger than any MTU, so that a
/// packet is never cut short into something that is not the packet.
const RX_MAX: usize = 64 * 1024;

/// What to open.
pub struct DeviceSpec {
    /// Linux: the interface's name. macOS: unused (the kernel picks `utunN`).
    pub name: String,
    pub mtu: usize,
    /// Windows: where `wintun.dll` is; unused elsewhere.
    pub wintun_dll: Option<std::path::PathBuf>,
}

/// The virtual adapter (M4 design 8.1): packets in and out, without offloads.
pub struct TunDevice {
    imp: Imp,
    adapter: Adapter,
    mtu: usize,
    dropped: AtomicU64,
    /// Whether the one WARN about a packet with no room has been made.
    warned: AtomicBool,
    /// The buffer reads go into, kept here so that a read every millisecond
    /// does not allocate 64 KiB each time. Taken out for the length of a read
    /// and put back when it ends; a read that is dropped in the middle loses
    /// it, and the next one allocates.
    buffer: Mutex<Vec<u8>>,
}

impl TunDevice {
    /// Opens the adapter. Inside a tokio runtime (a `spawn_blocking` thread is):
    /// the descriptor is registered with its reactor. Only the platform's
    /// `System::open` calls it, and no test does (M4c plan P19).
    pub fn open(spec: &DeviceSpec) -> io::Result<TunDevice> {
        let (imp, adapter) = Imp::open(spec)?;
        Ok(TunDevice {
            imp,
            adapter,
            mtu: spec.mtu,
            dropped: AtomicU64::new(0),
            warned: AtomicBool::new(false),
            buffer: Mutex::new(Vec::new()),
        })
    }

    /// The name the kernel gave the adapter, and the index this platform's
    /// steps name it by (Linux and macOS: none).
    pub fn adapter(&self) -> &Adapter {
        &self.adapter
    }

    pub fn mtu(&self) -> usize {
        self.mtu
    }

    /// Waits for at least one packet, then moves up to `max` without waiting.
    /// Each packet is allocated at its own length (`rurge_tun::Device`'s contract).
    pub async fn recv(&self, into: &mut Vec<Vec<u8>>, max: usize) -> io::Result<()> {
        // the lock is taken and given back on either side of the await: a
        // `std` guard is not `Send`, and the future M4c's task 8 wants is
        let mut buffer = std::mem::take(&mut *self.buffer());
        let result = self.move_packets(&mut buffer, into, max).await;
        // on a cancelled read this is never reached, and the buffer is lost
        *self.buffer() = buffer;
        result
    }

    /// Writes `packets` in order and leaves it empty. A packet the system has
    /// no room for is dropped and counted (a warning the first time, debug lines after).
    pub async fn send(&self, packets: &mut Vec<Vec<u8>>) -> io::Result<()> {
        write_all(
            packets,
            &self.dropped,
            &self.warned,
            |packet| self.imp.try_write(packet),
            no_room,
        )
    }

    /// The packets `send` dropped.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The packet the read waited for, and then what else is waiting.
    async fn move_packets(
        &self,
        buffer: &mut Vec<u8>,
        into: &mut Vec<Vec<u8>>,
        max: usize,
    ) -> io::Result<()> {
        if buffer.len() < RX_MAX {
            buffer.resize(RX_MAX, 0);
        }
        let read = self.imp.read(buffer).await?;
        into.push(buffer[..read].to_vec());
        drain(into, max, buffer, |buf| self.imp.try_read(buf))
    }

    fn buffer(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        // a read that panicked has lost its buffer, which the next one
        // allocates: there is no harm left to keep from spreading
        self.buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Moves what is already there into `into`: the packets `try_read` gives
/// without waiting, until `into` holds `max` of them or it has none. Anything
/// but a `WouldBlock` read ends the round with that error.
///
/// The packet a read waited for is already in `into` by the time this is
/// called. A read that finds nothing is the usual end, not a failure.
fn drain<F>(into: &mut Vec<Vec<u8>>, max: usize, buf: &mut [u8], mut try_read: F) -> io::Result<()>
where
    F: FnMut(&mut [u8]) -> io::Result<usize>,
{
    while into.len() < max {
        match try_read(buf) {
            // a packet takes its own length: a waiting packet's capacity is
            // what the TUN counts
            Ok(read) => into.push(buf[..read].to_vec()),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Writes `packets` in order and leaves them out of it. A packet the system
/// has no room for (`no_room`) is dropped and counted: one WARN the first
/// time, a DEBUG line each time after, and the packets after it still go out
/// (M4 design 8.1, M4c plan P14). Any other error ends the send with it, and
/// with the TUN: the device is broken, not full.
///
/// Neither line says which adapter or which address: a WARN is a line every
/// user of the enhanced mode reads (M4 design 13).
fn write_all<F, N>(
    packets: &mut Vec<Vec<u8>>,
    dropped: &AtomicU64,
    warned: &AtomicBool,
    mut try_write: F,
    no_room: N,
) -> io::Result<()>
where
    F: FnMut(&[u8]) -> io::Result<()>,
    N: Fn(&io::Error) -> bool,
{
    for packet in packets.iter() {
        let Err(error) = try_write(packet) else {
            continue;
        };
        if !no_room(&error) {
            return Err(error);
        }
        dropped.fetch_add(1, Ordering::Relaxed);
        if warned.swap(true, Ordering::Relaxed) {
            tracing::debug!(%error, "a packet with no room in the system is dropped");
        } else {
            tracing::warn!(
                %error,
                "the system had no room for a packet; the enhanced mode drops such packets \
                 and counts them"
            );
        }
    }
    packets.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    /// What a `try_read` finds, in order, and then nothing: `WouldBlock`, as a
    /// read of an adapter with no packet waiting says.
    struct Pending {
        packets: std::vec::IntoIter<Vec<u8>>,
    }

    impl Pending {
        fn of(packets: &[&[u8]]) -> Pending {
            Pending {
                packets: packets
                    .iter()
                    .map(|packet| packet.to_vec())
                    .collect::<Vec<_>>()
                    .into_iter(),
            }
        }

        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(packet) = self.packets.next() else {
                return Err(io::Error::from(ErrorKind::WouldBlock));
            };
            buf[..packet.len()].copy_from_slice(&packet);
            Ok(packet.len())
        }
    }

    /// One `recv` over `packets`: the first is the packet the read waited for,
    /// the rest are what the reads that do not wait find.
    fn recv_of(max: usize, packets: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut pending = Pending::of(packets);
        let mut buf = vec![0u8; RX_MAX];
        let mut into = Vec::new();
        let first = pending.read(&mut buf).unwrap();
        into.push(buf[..first].to_vec());
        drain(&mut into, max, &mut buf, |buf| pending.read(buf)).unwrap();
        into
    }

    /// A read waits for one packet and then moves what is already there, up to
    /// `max` packets in all (M4 design 8.1); a read that finds nothing is not
    /// an error, and each packet is allocated at its own length.
    #[test]
    fn recv_moves_what_is_there_up_to_max_without_waiting() {
        let packets: &[&[u8]] = &[b"first", b"second", b"third"];

        // `max` 2: the packet the read waited for, and the one after it
        assert_eq!(
            recv_of(2, packets),
            vec![b"first".to_vec(), b"second".to_vec()]
        );
        // above what is there: all of it, and the empty read ends the drain
        assert_eq!(
            recv_of(3, packets),
            vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()]
        );
        assert_eq!(recv_of(256, packets).len(), 3);

        // the TUN counts what a waiting packet holds by its capacity
        for packet in recv_of(256, packets) {
            assert_eq!(packet.capacity(), packet.len());
        }
    }

    /// A packet the system has no room for is dropped and counted, and the
    /// packets after it still go out (M4c plan P14).
    #[test]
    fn a_write_with_no_room_is_dropped_and_counted() {
        let mut packets = vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()];
        let dropped = AtomicU64::new(0);
        let warned = AtomicBool::new(false);
        let mut written = Vec::new();
        let result = write_all(
            &mut packets,
            &dropped,
            &warned,
            |packet| {
                if packet == b"two" {
                    return Err(io::Error::new(ErrorKind::WouldBlock, "no room"));
                }
                written.push(packet.to_vec());
                Ok(())
            },
            |error| error.kind() == ErrorKind::WouldBlock,
        );

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(written, vec![b"one".to_vec(), b"three".to_vec()]);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert!(warned.load(Ordering::Relaxed), "the first drop is a WARN");
        assert!(packets.is_empty(), "what went out is taken out");

        // and the next ones are counted too, with no second WARN
        let mut packets = vec![b"four".to_vec(), b"five".to_vec()];
        let result = write_all(
            &mut packets,
            &dropped,
            &warned,
            |_| Err(io::Error::new(ErrorKind::WouldBlock, "no room")),
            |_| true,
        );
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(dropped.load(Ordering::Relaxed), 3);
        assert!(packets.is_empty());
    }

    /// Any other failure to write ends the send: the packets after it are not
    /// written, and the error is the one the system gave.
    #[test]
    fn another_write_error_ends_the_send() {
        let mut packets = vec![b"one".to_vec(), b"two".to_vec()];
        let dropped = AtomicU64::new(0);
        let warned = AtomicBool::new(false);
        let mut written = Vec::new();
        let result = write_all(
            &mut packets,
            &dropped,
            &warned,
            |packet| {
                if packet == b"one" {
                    return Err(io::Error::new(ErrorKind::BrokenPipe, "the adapter is gone"));
                }
                written.push(packet.to_vec());
                Ok(())
            },
            |_| false,
        );

        let error = result.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
        assert_eq!(error.to_string(), "the adapter is gone");
        assert!(written.is_empty(), "the packet after it is not written");
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert!(!warned.load(Ordering::Relaxed), "nothing was dropped");
    }

    /// A send that goes through writes in order, counts nothing and leaves
    /// nothing behind.
    #[test]
    fn every_packet_is_written_in_order_and_taken_out() {
        let mut packets = vec![b"one".to_vec(), b"two".to_vec()];
        let dropped = AtomicU64::new(0);
        let warned = AtomicBool::new(false);
        let mut written = Vec::new();
        let result = write_all(
            &mut packets,
            &dropped,
            &warned,
            |packet| {
                written.push(packet.to_vec());
                Ok(())
            },
            |_| false,
        );

        assert!(result.is_ok(), "{result:?}");
        assert_eq!(written, vec![b"one".to_vec(), b"two".to_vec()]);
        assert!(packets.is_empty());
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
        assert!(!warned.load(Ordering::Relaxed));
    }
}
