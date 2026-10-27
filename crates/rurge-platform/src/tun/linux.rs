//! The adapter on Linux (M4 design 8.2): `/dev/net/tun`, one IP packet per read
//! and one per write, and no packet information before either.
//!
//! The interface is not persistent: the descriptor's close takes it, with its
//! addresses and routes, away; so does the end of the process. One machine has
//! one enhanced mode (M4-D12), so a second rurge asking for `rurge` gets the
//! kernel's "busy" for that name.
//!
//! A read waits on the reactor ([`AsyncFd`]) and then reads without waiting. A
//! write is tried at once on the non-blocking descriptor, without asking the
//! reactor first: asking it would turn a good packet into "no room" whenever
//! the cached write readiness was not set yet — at the start, or after one
//! real `WouldBlock` — and the TUN's `write_all` would drop it as if the system
//! had no room. The descriptor is registered for reads only.
//!
//! Both reads take their lengths through [`frame::packet_len`]: an empty read
//! is the device gone, not a packet to skip over.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;

use nix::libc::{self, c_char, c_short};
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

use super::DeviceSpec;
use super::frame::{self, Os};
use crate::enhanced::Adapter;

/// The kernel's tun device: the descriptor, registered with the reactor for
/// reads.
pub(super) struct Device {
    fd: AsyncFd<File>,
}

impl Device {
    /// Opens `/dev/net/tun` and asks the kernel for an interface called
    /// `spec.name`, carrying bare IP packets. The adapter it gives back carries
    /// the name the interface has, and no index: Linux's steps name it by the
    /// name. A failure to open is put into the words of [`frame::explained`].
    pub(super) fn open(spec: &DeviceSpec) -> io::Result<(Device, Adapter)> {
        let name = frame::ifname(&spec.name)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            // std adds `O_CLOEXEC` on Unix; the reads and writes of this
            // device are the non-blocking kind
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/net/tun")
            .map_err(|e| frame::explained(e, Os::Linux))?;
        let name = set_iff(file.as_raw_fd(), &name)?;
        let fd = AsyncFd::with_interest(file, Interest::READABLE)?;
        Ok((Device { fd }, Adapter { name, index: None }))
    }

    /// Waits until a packet is there, then reads it into `buf`.
    pub(super) async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.fd.readable().await?;
            match guard.try_io(|fd| fd.get_ref().read(buf)) {
                Ok(read) => return frame::packet_len(read?),
                // nothing was there after all: `try_io` has cleared the
                // readiness, and the wait starts over
                Err(_would_block) => continue,
            }
        }
    }

    /// The packet that was already there, read without waiting.
    pub(super) fn try_read(&self, buf: &mut [u8]) -> io::Result<usize> {
        frame::packet_len(
            self.fd
                .try_io(Interest::READABLE, |mut file| file.read(buf))?,
        )
    }

    /// Writes one packet without waiting.
    pub(super) fn try_write(&self, packet: &[u8]) -> io::Result<()> {
        let written = self.fd.get_ref().write(packet)?;
        if written == packet.len() {
            Ok(())
        } else {
            Err(io::Error::other(
                "the virtual adapter took part of a packet",
            ))
        }
    }
}

/// Whether a failed write means the system had no room for the packet (M4c
/// plan P14): the packet is dropped and counted, and the TUN goes on. The
/// kernel says `EAGAIN` for a queue that is full, `ENOBUFS` and `EMSGSIZE` for
/// a packet it cannot carry, and `EINVAL` for one it cannot read.
pub(super) fn no_room(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::WouldBlock
        || matches!(
            err.raw_os_error(),
            Some(libc::ENOBUFS | libc::EMSGSIZE | libc::EINVAL)
        )
}

/// Asks the kernel for the interface of `name`'s name (NUL-terminated in the
/// 16 bytes) that carries bare IP packets, and gives back the name it has now.
#[allow(unsafe_code)]
fn set_iff(fd: RawFd, name: &[u8; 16]) -> io::Result<String> {
    let mut req = libc::ifreq {
        // `c_char` is signed on some of Linux's architectures and unsigned on
        // others: the byte goes over as it is on this one
        ifr_name: name.map(|byte| c_char::from_ne_bytes([byte])),
        ifr_ifru: libc::__c_anonymous_ifr_ifru {
            ifru_flags: (libc::IFF_TUN | libc::IFF_NO_PI) as c_short,
        },
    };
    // SAFETY: `fd` is an open `/dev/net/tun` descriptor, and `req` is a live
    // `ifreq` whose name ends in the NUL its 16th byte holds; `TUNSETIFF` reads
    // that and writes the interface's name back into the same bytes.
    let result = unsafe { libc::ioctl(fd, libc::TUNSETIFF, &mut req as *mut libc::ifreq) };
    if result < 0 {
        return Err(frame::explained(io::Error::last_os_error(), Os::Linux));
    }
    Ok(frame::name_of(&req.ifr_name.map(|c| c.to_ne_bytes()[0])))
}
