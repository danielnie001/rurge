//! The virtual adapter on Windows (M4 design 8.4, 8.5): Wintun's adapter, its
//! session, and the thread that carries the session's packets over to the
//! asynchronous side.
//!
//! The half before the adapter — finding `wintun.dll`, checking who signed it,
//! loading it — is [`wintun`]'s. Opening goes on in the order the design gives
//! it: the name is probed first (plan P8: an adapter another rurge has open is
//! not reused, so the answer is "in use" rather than a second adapter), then
//! the adapter is made, its interface index is waited for, a session is
//! started, its read event taken, and the reader thread started. A step that
//! fails takes back what the steps before it made, newest first.
//!
//! The reader blocks on the session's read event and hands each packet to a
//! bounded channel; [`super::reader`]'s loop is the whole of it. Sending does
//! not wait: the packet goes into the ring, and a full ring is
//! [`io::ErrorKind::WouldBlock`], which the TUN drops and counts.
//!
//! Closing is [`Drop`]'s, in one order that matters: the reader is told to stop
//! and its channel is closed, the reader is waited for, and only then is the
//! session ended — the reader uses it until it ends. The adapter goes last,
//! which removes it (`wintun.h`). The device is closed when the last `Arc` of
//! it is dropped (the phase 3 / M4d plan's execution record row 29), not by a
//! call from elsewhere.
//!
//! The handles are integers and the entry points are in an `Arc<Wintun>`, so
//! the device is `Send` and `Sync` without an `unsafe impl` (plan P4).

mod wintun;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio::sync::mpsc;
use windows_sys::core::GUID;

use super::DeviceSpec;
use super::frame::{self, Os};
use super::reader::{self, Ring};
use crate::enhanced::Adapter;
use wintun::Wintun;

/// The adapter's name, and the name the steps use of it: one machine has one
/// enhanced mode (M4-D12).
const NAME: &str = "rurge";

/// The tunnel type Wintun is told the adapter has (`wintun.h`'s own words for
/// the second name a `WintunCreateAdapter` takes).
const TUNNEL_TYPE: &str = "rurge";

/// rurge's adapter GUID: the same on every run, so that the adapter's own
/// settings — its DNS servers among them — come back with it (plan P12).
const GUID: GUID = GUID::from_u128(0xb4a5c1e2_7d3f_4c6a_8e91_0f2d3b4c5a6e);

/// The session's ring: 8 MiB (M4 design 8.4), a power of two as `wintun.h`
/// requires.
const RING: u32 = 0x800000;

/// How many packets the reader may have waiting for the asynchronous side
/// before it waits (M4d plan P13).
const PACKETS: usize = 1024;

/// How long the adapter's interface index takes to appear, and how often it is
/// asked (M4 design 8.4): the adapter's device is enumerated a moment after it
/// was made.
const INDEX_EVERY: Duration = Duration::from_millis(100);
const INDEX_WITHIN: Duration = Duration::from_secs(5);

/// The kernel's Wintun adapter: the loaded module, the adapter, its session,
/// and the reader that fills [`Device::packets`].
pub(super) struct Device {
    wintun: Arc<Wintun>,
    adapter: usize,
    /// The session the reader reads and `try_write` sends into.
    session: usize,
    /// Ends the reader: set on a close, read by the loop (plan P13).
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    /// The packets the reader passed on, in the order the ring gave them. A
    /// `tokio` lock because a read holds it across the wait for a packet.
    packets: Mutex<mpsc::Receiver<io::Result<Vec<u8>>>>,
}

/// The session as the reader loop sees it: a packet, or a wait on the session's
/// read event.
struct ReadRing {
    wintun: Arc<Wintun>,
    session: usize,
    event: usize,
}

/// What [`Device::start`] made: the adapter's index, its session, the reader
/// that reads it, and the packets the reader passes on.
struct Started {
    index: u32,
    session: usize,
    reader: JoinHandle<()>,
    packets: mpsc::Receiver<io::Result<Vec<u8>>>,
}

impl Ring for ReadRing {
    fn receive(&self) -> io::Result<Option<Vec<u8>>> {
        wintun::receive(&self.wintun, self.session)
    }

    fn wait(&self, timeout: Duration) {
        // the wait is counted in whole milliseconds; `READ_WAIT` is 100 of them
        let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        wintun::wait_readable(self.event, milliseconds);
    }
}

impl Device {
    /// Opens the adapter in the design's order (M4 design 8.4), on a blocking
    /// thread: `loaded`'s answer comes back as it is (the phase 3 / M4d plan's
    /// execution record row 59), and the steps after it through
    /// [`frame::explained`]. Whatever went wrong after the adapter was made
    /// takes it — and a session, if one was started — back.
    pub(super) fn open(spec: &DeviceSpec) -> io::Result<(Device, Adapter)> {
        let Some(path) = spec.wintun_dll.as_deref() else {
            // the caller is `WindowsSystem`, which always has a path: opening
            // without one is a programming error, not a system's answer
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the Windows device needs to know where wintun.dll is",
            ));
        };
        // 1. find, check and load wintun.dll; the errors are its own words
        let wintun = wintun::loaded(path)?;
        // 2. probe: an adapter of that name that another process has open is
        // not reused (plan P8), and closing the handle we got is the whole of
        // the probe
        if let Some(adapter) = wintun::open_adapter(&wintun, NAME) {
            wintun::close_adapter(&wintun, adapter);
            let busy = io::Error::from(io::ErrorKind::ResourceBusy);
            return Err(frame::explained(busy, Os::Windows));
        }
        // 3. make the adapter
        let adapter = wintun::create_adapter(&wintun, NAME, TUNNEL_TYPE, &GUID)
            .map_err(|error| frame::explained(error, Os::Windows))?;
        // 4. to 7. the index, the session, its read event, and the reader
        let stop = Arc::new(AtomicBool::new(false));
        let opened = match Self::start(&wintun, adapter, &stop) {
            Ok(opened) => opened,
            Err(error) => {
                // nothing of the session is left: `start` ends it
                wintun::close_adapter(&wintun, adapter);
                return Err(error);
            }
        };
        Ok((
            Device {
                wintun,
                adapter,
                session: opened.session,
                stop,
                reader: Some(opened.reader),
                packets: Mutex::new(opened.packets),
            },
            Adapter {
                name: NAME.to_string(),
                index: Some(opened.index),
            },
        ))
    }

    /// The steps after the adapter was made: the index, the session, its read
    /// event, and the reader thread. A session this started is ended here when
    /// a later step fails; the adapter is the caller's to close. `stop` is the
    /// device's own flag, which the thread it starts watches: the device sets
    /// it on a close, and the thread reads it.
    fn start(wintun: &Arc<Wintun>, adapter: usize, stop: &Arc<AtomicBool>) -> io::Result<Started> {
        let index = index(wintun, adapter).map_err(|error| frame::explained(error, Os::Windows))?;
        let session = wintun::start_session(wintun, adapter, RING)
            .map_err(|error| frame::explained(error, Os::Windows))?;
        let event = match wintun::read_event(wintun, session) {
            Ok(event) => event,
            Err(error) => {
                wintun::end_session(wintun, session);
                return Err(frame::explained(error, Os::Windows));
            }
        };
        let (tx, rx) = mpsc::channel(PACKETS);
        let ring = ReadRing {
            wintun: Arc::clone(wintun),
            session,
            event,
        };
        let thread_stop = Arc::clone(stop);
        let reader = std::thread::Builder::new()
            .name("rurge-wintun-reader".to_string())
            .spawn(move || reader::read_loop(&ring, &tx, &thread_stop, reader::READ_WAIT));
        match reader {
            Ok(reader) => Ok(Started {
                index,
                session,
                reader,
                packets: rx,
            }),
            Err(error) => {
                // the thread did not start, so nothing is reading the session
                wintun::end_session(wintun, session);
                let error = io::Error::new(
                    error.kind(),
                    format!("cannot start the adapter's reader: {error}"),
                );
                Err(frame::explained(error, Os::Windows))
            }
        }
    }

    /// Waits until the reader has passed a packet on, then reads it into `buf`:
    /// the TUN's own `recv` moves what else is waiting. A packet the reader
    /// passed on as an error is that error, and ends the TUN; a channel with
    /// its sending half gone is the adapter closed under us.
    pub(super) async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut packets = self.packets.lock().await;
        let packet = match packets.recv().await {
            Some(packet) => packet?,
            None => return Err(reader_ended()),
        };
        copy(&packet, buf)
    }

    /// The packet that was already passed on, read without waiting.
    pub(super) fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let Ok(mut packets) = self.packets.try_lock() else {
            // a read is holding the receiving half: nothing is there for this
            // one, and waiting is the other kind of read
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        };
        match packets.try_recv() {
            Ok(packet) => copy(&packet?, buf),
            Err(mpsc::error::TryRecvError::Empty) => {
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            }
            Err(mpsc::error::TryRecvError::Disconnected) => Err(reader_ended()),
        }
    }

    /// Writes one packet into the session's ring without waiting. A full ring
    /// is `WouldBlock`, which the TUN drops and counts (M4c plan P14).
    pub(super) fn try_write(&self, packet: &[u8]) -> io::Result<()> {
        match wintun::send(&self.wintun, self.session, packet) {
            Ok(true) => Ok(()),
            Ok(false) => Err(io::Error::from(io::ErrorKind::WouldBlock)),
            Err(error) => Err(error),
        }
    }
}

impl Drop for Device {
    /// Closing, in the design's order (M4 design 8.4, M4d plan P13): the reader
    /// is told to stop, the channel's receiving half is closed — which is what
    /// wakes a reader held in `blocking_send` on a full channel — and the
    /// reader is waited for. Only then is the session ended: the reader uses it
    /// until it has ended. The adapter goes last, and with it the adapter
    /// itself is removed (`wintun.h`).
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // `&mut self`: the receiving half is reached without the lock
        self.packets.get_mut().close();
        if let Some(reader) = self.reader.take()
            && let Err(panicked) = reader.join()
        {
            let said = panicked
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| panicked.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("no message");
            tracing::debug!(
                panic = said,
                "the virtual adapter's reader ended with a panic"
            );
        }
        wintun::end_session(&self.wintun, self.session);
        wintun::close_adapter(&self.wintun, self.adapter);
    }
}

/// The adapter's interface index (M4 design 8.4): asked every [`INDEX_EVERY`]
/// for at most [`INDEX_WITHIN`], because the system enumerates the adapter's
/// device a moment after it was made. The last error is kept, and says how long
/// the wait was.
fn index(wintun: &Wintun, adapter: usize) -> io::Result<u32> {
    let deadline = Instant::now() + INDEX_WITHIN;
    loop {
        match wintun::adapter_index(wintun, adapter) {
            Ok(index) => return Ok(index),
            Err(error) if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "the virtual adapter has no interface index 5 seconds after it was made: \
                         {error}"
                    ),
                ));
            }
            Err(_) => std::thread::sleep(INDEX_EVERY),
        }
    }
}

/// `packet` in `buf`, its length. A packet longer than the buffer is refused
/// rather than cut short: what the first `buf.len()` bytes of it would be is
/// not that packet. Wintun's packets are at most `0xFFFF` bytes (`wintun.h`
/// has the ring's limit), so the TUN's read buffer is never too small.
fn copy(packet: &[u8], buf: &mut [u8]) -> io::Result<usize> {
    if packet.len() > buf.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "a packet from the virtual adapter is longer than the buffer for it",
        ));
    }
    buf[..packet.len()].copy_from_slice(packet);
    Ok(packet.len())
}

/// What a read answers when the reader's sending half is gone: the adapter is
/// closed (the TUN's "the device is gone").
fn reader_ended() -> io::Error {
    io::Error::other("the virtual adapter's reader ended")
}

/// Whether a failed write means the system had no room for the packet (M4c
/// plan P14): dropped and counted, and the TUN goes on. Wintun says so with a
/// full ring, which [`wintun::send`] answers as `Ok(false)` and this module as
/// `WouldBlock`.
pub(super) fn no_room(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
}
