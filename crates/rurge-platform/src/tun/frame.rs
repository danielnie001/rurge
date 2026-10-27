//! The frames and names of the virtual adapter, and what failing to open it
//! means to the user (M4 design 8.1, 8.3, 8.5).
//!
//! Nothing here belongs to one platform, so all of it is compiled and tested
//! on every one (M4c plan P2): the name an `ifreq` holds (Linux), the address
//! family utun puts before each packet (macOS), and the words a failed open
//! gets. The two platforms' `open`s call these; the tests are the only place
//! they are exercised on the development machine.

use std::io;

/// Darwin's address family for IPv4, which utun puts before each packet in
/// network order: the value the system's headers give it.
pub const DARWIN_AF_INET: u32 = 2;
/// Darwin's address family for IPv6: the value the system's headers give it.
pub const DARWIN_AF_INET6: u32 = 30;

/// `name` as `ifreq`'s name: 1 to 15 bytes, no `/`, `:`, whitespace or NUL,
/// NUL-padded.
///
/// The kernel's own rules for an interface name are what these are; anything
/// else is refused here rather than by the kernel, so the error says what is
/// wrong with the name.
pub fn ifname(name: &str) -> io::Result<[u8; 16]> {
    let bytes = name.as_bytes();
    let fits = !bytes.is_empty() && bytes.len() <= 15;
    if !fits
        || bytes
            .iter()
            .any(|byte| matches!(byte, b'/' | b':' | b'\0') || byte.is_ascii_whitespace())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an interface name is 1 to 15 bytes, without `/`, `:`, whitespace or NUL",
        ));
    }
    let mut ifr_name = [0u8; 16];
    ifr_name[..bytes.len()].copy_from_slice(bytes);
    Ok(ifr_name)
}

/// The name in a NUL-terminated buffer the kernel filled in: what is before
/// the first NUL (all of it, when there is none).
pub fn name_of(buf: &[u8]) -> String {
    let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// What a read of `read` bytes from the adapter gave: the length of the packet
/// it read, or — nothing read — the end of the device.
///
/// Neither kernel gives an empty read for a buffer that is not empty, so an
/// empty one is the device gone, and reading again would only spin on a
/// descriptor whose readiness never goes away (M4 design 8.1). Both platforms'
/// devices take their reads through this.
pub fn packet_len(read: usize) -> io::Result<usize> {
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the virtual adapter closed",
        ));
    }
    Ok(read)
}

/// The four bytes utun wants before `packet`, from its IP version; `None` for
/// neither 4 nor 6.
pub fn utun_header(packet: &[u8]) -> Option<[u8; 4]> {
    match packet.first()? >> 4 {
        4 => Some(DARWIN_AF_INET.to_be_bytes()),
        6 => Some(DARWIN_AF_INET6.to_be_bytes()),
        _ => None,
    }
}

/// The IP packet in what a utun socket gave: after a known family, not empty.
///
/// utun puts the family of the packet it is handing over in the four bytes
/// before it, in network order; a datagram with any other family, or with
/// nothing after it, is not a packet this device can use.
pub fn utun_packet(datagram: &[u8]) -> Option<&[u8]> {
    let (family, packet) = datagram.split_first_chunk::<4>()?;
    let family = u32::from_be_bytes(*family);
    if family != DARWIN_AF_INET && family != DARWIN_AF_INET6 {
        return None;
    }
    if packet.is_empty() {
        return None;
    }
    Some(packet)
}

/// The platform whose words a failed open is explained in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
}

/// What an error of opening the adapter means to the user (M4 design 8.5):
/// the missing permission or the missing module, in the platform's words, or
/// what the system said. The error's kind is the one the system gave.
///
/// A platform is named rather than chosen by `cfg`, so that either platform's
/// words can be tested wherever this crate is built (M4c plan P2, P16).
pub fn explained(err: io::Error, os: Os) -> io::Error {
    let kind = err.kind();
    let text = match (os, kind) {
        (Os::Linux, io::ErrorKind::PermissionDenied) => {
            "the enhanced mode needs root or CAP_NET_ADMIN"
        }
        (Os::Linux, io::ErrorKind::NotFound) => {
            "there is no /dev/net/tun: the kernel's tun module is not loaded"
        }
        (Os::Linux, io::ErrorKind::ResourceBusy) => {
            "the virtual adapter is in use: is another rurge running with the enhanced mode on?"
        }
        (Os::MacOs, io::ErrorKind::PermissionDenied) => "the enhanced mode needs root",
        _ => return io::Error::new(kind, format!("cannot open the virtual adapter: {err}")),
    };
    io::Error::new(kind, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    /// The name goes into an `ifreq`'s 16 bytes, NUL-padded after it. Anything
    /// the kernel would not take as an interface name is a failure, not a name.
    #[test]
    fn ifname_takes_up_to_fifteen_bytes() {
        let mut expected = [0u8; 16];
        expected[..5].copy_from_slice(b"rurge");
        assert_eq!(ifname("rurge").unwrap(), expected);

        // the longest a name may be
        let fifteen = "abcdefghijklmno";
        let mut expected = [0u8; 16];
        expected[..15].copy_from_slice(fifteen.as_bytes());
        assert_eq!(ifname(fifteen).unwrap(), expected);

        for bad in [
            "",
            "abcdefghijklmnop",
            "ru/rge",
            "ru:rge",
            "ru rge",
            "ru\trge",
            "\nrurge",
            "rur\0ge",
        ] {
            let error = ifname(bad).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidInput, "{bad:?}");
        }
    }

    /// The kernel gives back the name NUL-terminated in a fixed buffer; what
    /// comes after the NUL is nothing.
    #[test]
    fn name_of_stops_at_the_first_nul() {
        let mut buf = [0u8; 16];
        buf[..5].copy_from_slice(b"utun3");
        assert_eq!(name_of(&buf), "utun3");

        // a buffer with no NUL at all is all name
        assert_eq!(name_of(b"tun9"), "tun9");
        assert_eq!(name_of(&[]), "");
        assert_eq!(name_of(b"ut\0un7"), "ut");
    }

    /// utun puts the address family in network order before the packet, and
    /// the family is the packet's IP version.
    #[test]
    fn utun_header_follows_the_ip_version() {
        assert_eq!(utun_header(&[0x45, 0, 0, 20]), Some([0, 0, 0, 2]));
        assert_eq!(utun_header(&[0x60, 0, 0, 0]), Some([0, 0, 0, 30]));
        // neither 4 nor 6: not an IP packet
        assert_eq!(utun_header(&[]), None);
        assert_eq!(utun_header(&[0x00]), None);
        assert_eq!(utun_header(&[0x50, 0, 0, 20]), None);
    }

    /// What a utun socket gave is a known family and then the packet: the rest
    /// once the four bytes are taken off, and nothing for anything else.
    #[test]
    fn utun_packet_strips_a_known_family() {
        let packet = [0x45u8, 0, 0, 20];

        let mut v4 = DARWIN_AF_INET.to_be_bytes().to_vec();
        v4.extend_from_slice(&packet);
        assert_eq!(utun_packet(&v4), Some(&packet[..]));

        let mut v6 = DARWIN_AF_INET6.to_be_bytes().to_vec();
        v6.extend_from_slice(&packet);
        assert_eq!(utun_packet(&v6), Some(&packet[..]));

        // a family utun does not give: 1 (Unix)
        let mut other = 1u32.to_be_bytes().to_vec();
        other.extend_from_slice(&packet);
        assert_eq!(utun_packet(&other), None);

        // too short for the four bytes, or for them and a packet
        assert_eq!(utun_packet(&[]), None);
        assert_eq!(utun_packet(&[0, 0, 0]), None);
        assert_eq!(utun_packet(&DARWIN_AF_INET.to_be_bytes()), None);
    }

    /// A read of the adapter gave a packet of that length; a read that gave
    /// nothing is the device gone (M4 design 8.1).
    #[test]
    fn a_read_of_nothing_is_the_device_gone() {
        let error = packet_len(0).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
        assert_eq!(error.to_string(), "the virtual adapter closed");

        assert_eq!(packet_len(20).unwrap(), 20);
        assert_eq!(packet_len(1).unwrap(), 1);
    }

    /// Every failure says what is missing, under the platform's words, and the
    /// kind the system gave is kept (M4 design 8.5).
    #[test]
    fn open_errors_say_what_is_missing() {
        for (os, kind, text) in [
            (
                Os::Linux,
                ErrorKind::PermissionDenied,
                "the enhanced mode needs root or CAP_NET_ADMIN",
            ),
            (
                Os::Linux,
                ErrorKind::NotFound,
                "there is no /dev/net/tun: the kernel's tun module is not loaded",
            ),
            (
                Os::Linux,
                ErrorKind::ResourceBusy,
                "the virtual adapter is in use: is another rurge running with the enhanced mode on?",
            ),
            (
                Os::MacOs,
                ErrorKind::PermissionDenied,
                "the enhanced mode needs root",
            ),
        ] {
            let error = explained(io::Error::from(kind), os);
            assert_eq!(error.kind(), kind, "{os:?}");
            assert_eq!(error.to_string(), text, "{os:?}");
        }

        // anything else keeps the kind and says what the system said
        for os in [Os::Linux, Os::MacOs] {
            let error = explained(
                io::Error::new(ErrorKind::InvalidData, "the system's own words"),
                os,
            );
            assert_eq!(error.kind(), ErrorKind::InvalidData, "{os:?}");
            assert_eq!(
                error.to_string(),
                "cannot open the virtual adapter: the system's own words",
                "{os:?}"
            );
        }
    }
}
