//! The adapter on macOS (M4 design 8.3): utun, one IP packet per read and one
//! per write, with the packet's address family in the four bytes before it.
//!
//! The kernel picks the unit, and with it the name (`utunN`): nothing here asks
//! for a name, and a second rurge gets the next unit (M4-D12). The socket is the
//! kernel's utun control, `com.apple.net.utun_control`, connected to a unit of
//! the kernel's choosing — wireguard-go's `tun_darwin.go` (MIT) sets `sc_unit`
//! to 0 for that and reads the name back.
//!
//! A read waits on the reactor ([`AsyncFd`]) and then reads without waiting,
//! and the datagram it gets — the family and then the packet — is moved to the
//! front of the buffer for the device's contract ([`frame::utun_to_front`]).
//! A write is tried at once on the non-blocking descriptor, without asking the
//! reactor first (M4c plan task 6's first ruling, which Linux's module states in
//! full), and it goes out as two buffers: the family and the packet, without a
//! copy of the packet.
//!
//! Both reads take their lengths through [`frame::packet_len`]: an empty read
//! is the device gone, not a datagram to skip over.

use std::io::{self, IoSlice, Read};
use std::mem::size_of;
use std::os::fd::{AsRawFd, RawFd};

use nix::libc::{self, c_char, c_void, socklen_t};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

use super::DeviceSpec;
use super::frame::{self, Os};

/// The kernel control the utun sockets are units of (`<net/if_utun.h>`).
const UTUN_CONTROL_NAME: &[u8] = b"com.apple.net.utun_control";

/// The kernel's utun socket: the descriptor, registered with the reactor for
/// reads.
pub(super) struct Device {
    fd: AsyncFd<Socket>,
}

impl Device {
    /// Opens the next unit of utun. The kernel picks the name, so `spec`'s is
    /// not used; what comes back is the one the adapter has. A failure to open
    /// is put into the words of [`frame::explained`].
    pub(super) fn open(_spec: &DeviceSpec) -> io::Result<(Device, String)> {
        // socket2 makes it close-on-exec, and the reads and writes of this
        // socket are the non-blocking kind
        let socket = Socket::new(
            Domain::from(libc::AF_SYSTEM),
            Type::DGRAM,
            Some(Protocol::from(libc::SYSPROTO_CONTROL)),
        )
        .map_err(|e| frame::explained(e, Os::MacOs))?;
        let name = utun_connect(socket.as_raw_fd())?;
        socket.set_nonblocking(true)?;
        let fd = AsyncFd::with_interest(socket, Interest::READABLE)?;
        Ok((Device { fd }, name))
    }

    /// Waits until a packet is there, then reads it into `buf`.
    pub(super) async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable().await?;
            let read = match guard.try_io(|fd| fd.get_ref().read(buf)) {
                Ok(read) => frame::packet_len(read?)?,
                // nothing was there after all: `try_io` has cleared the
                // readiness, and the wait starts over
                Err(_would_block) => continue,
            };
            match frame::utun_to_front(buf, read) {
                Some(len) => return Ok(len),
                None => skipped(read),
            }
        }
    }

    /// The packet that was already there, read without waiting.
    pub(super) fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let read = self
                .fd
                .try_io(Interest::READABLE, |mut socket| socket.read(buf))?;
            let read = frame::packet_len(read)?;
            match frame::utun_to_front(buf, read) {
                Some(len) => return Ok(len),
                None => skipped(read),
            }
        }
    }

    /// Writes one packet without waiting, as the family and then the packet.
    pub(super) fn try_write(&self, packet: &[u8]) -> io::Result<()> {
        // neither IPv4 nor IPv6: what the kernel refuses such a packet with, so
        // that it is dropped and counted like the packets with no room
        // (`no_room`)
        let header =
            frame::utun_header(packet).ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
        let written = self
            .fd
            .get_ref()
            .send_vectored(&[IoSlice::new(&header), IoSlice::new(packet)])?;
        if written == header.len() + packet.len() {
            Ok(())
        } else {
            Err(io::Error::other(
                "the virtual adapter took part of a packet",
            ))
        }
    }
}

/// A datagram that was no IP packet: it is skipped, and said at DEBUG. Neither
/// the adapter nor anything in the datagram is named (M4 design 13).
fn skipped(read: usize) {
    tracing::debug!(
        read,
        "a datagram from the virtual adapter that is no IP packet is skipped"
    );
}

/// Whether a failed write means the system had no room for the packet (M4c
/// plan P14): the packet is dropped and counted, and the TUN goes on. The
/// kernel says `EAGAIN` for a queue that is full, `ENOBUFS` and `EMSGSIZE` for
/// a packet it cannot carry, and `EINVAL` for one it cannot read — the last
/// being how `try_write` drops a packet that is no IP packet.
pub(super) fn no_room(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
        || matches!(
            err.raw_os_error(),
            Some(libc::ENOBUFS | libc::EMSGSIZE | libc::EINVAL)
        )
}

/// Connects `fd` — a utun control socket — to the next unit the kernel gives,
/// and reads back the name it has.
#[allow(unsafe_code)]
fn utun_connect(fd: RawFd) -> io::Result<String> {
    let mut info = libc::ctl_info {
        ctl_id: 0,
        ctl_name: [0; libc::MAX_KCTL_NAME],
    };
    // the name goes in NUL-terminated: the array is longer than it, and the
    // bytes after it are already NUL
    for (slot, byte) in info.ctl_name.iter_mut().zip(UTUN_CONTROL_NAME) {
        // `c_char` is signed on some of the kernel's architectures and
        // unsigned on others: the byte goes over as it is on this one
        *slot = c_char::from_ne_bytes([*byte]);
    }
    // SAFETY: `fd` is an open socket of the `AF_SYSTEM` / `SYSPROTO_CONTROL`
    // kind, and `info` is a live `ctl_info` whose name ends in a NUL;
    // `CTLIOCGINFO` reads the name and writes the control's ID back into the
    // same struct.
    let result = unsafe { libc::ioctl(fd, libc::CTLIOCGINFO, &mut info as *mut libc::ctl_info) };
    if result < 0 {
        return Err(frame::explained(io::Error::last_os_error(), Os::MacOs));
    }

    let addr = libc::sockaddr_ctl {
        sc_len: size_of::<libc::sockaddr_ctl>() as u8,
        sc_family: libc::AF_SYSTEM as u8,
        ss_sysaddr: libc::AF_SYS_CONTROL as u16,
        sc_id: info.ctl_id,
        // the kernel picks the unit, and with it the name (wireguard-go's
        // `tun_darwin.go`)
        sc_unit: 0,
        sc_reserved: [0; 5],
    };
    // SAFETY: `fd` is the socket the control ID above was read for, and `addr`
    // is a live `sockaddr_ctl` whose length is the one it carries; `connect`
    // reads exactly those bytes of it, in the length given.
    let result = unsafe {
        libc::connect(
            fd,
            (&addr as *const libc::sockaddr_ctl).cast::<libc::sockaddr>(),
            size_of::<libc::sockaddr_ctl>() as socklen_t,
        )
    };
    if result < 0 {
        return Err(frame::explained(io::Error::last_os_error(), Os::MacOs));
    }

    let mut name = [0u8; libc::IFNAMSIZ];
    let mut len = name.len() as socklen_t;
    // SAFETY: `fd` is the connected utun socket, and `name` and `len` are a
    // live buffer with the room it says it has; `getsockopt` writes at most
    // that many bytes into it and the length it wrote into `len`.
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SYSPROTO_CONTROL,
            libc::UTUN_OPT_IFNAME,
            name.as_mut_ptr().cast::<c_void>(),
            &mut len,
        )
    };
    if result < 0 {
        return Err(frame::explained(io::Error::last_os_error(), Os::MacOs));
    }
    // the buffer was zeroed: the name ends at the first NUL, whatever `len` says
    Ok(frame::name_of(&name))
}
