//! macOS: the tables of TCP connections and UDP sockets the kernel keeps
//! (`net.inet.tcp.pcblist_n` and `net.inet.udp.pcblist_n`), with the process
//! that last used each socket, and the path of the executable of a process
//! (`proc_pidpath`). Everything but the calls into the system is compiled on
//! every host: how the kernel lays a table out in memory is read by `walk`,
//! which is tested on bytes.

use super::{ReadError, Row, RowOwner};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

#[cfg(target_os = "macos")]
use super::{Protocol, Source, interface_addresses};
#[cfg(target_os = "macos")]
use std::ffi::CStr;

// A table is an `xinpgen` (24 bytes: its length, a count and two generation
// numbers), items, and an `xinpgen` again. An item is one of the kernel's
// structures and begins with its length and its kind (two words); the kernel
// writes each one up to a multiple of 8 bytes. The words are in the order of
// the machine, which is little-endian on both kinds of Mac; a port is two
// bytes in network order, and an address is its bytes in order. The offsets
// below are those of `xinpcb_n` and `xsocket_n`, which the kernel lays out with
// 4-byte packing, from the start of the item.

/// The length of an `xinpgen`, which opens a table and ends it. No item is as
/// short.
const XINPGEN: usize = 24;

/// The kind of an item that is an `xsocket_n` (`XSO_SOCKET`): the socket as a
/// process has it.
const XSO_SOCKET: u32 = 0x1;

/// The kind of an item that is an `xinpcb_n` (`XSO_INPCB`): the protocol
/// control block, with the ports and the addresses. It begins a record.
const XSO_INPCB: u32 = 0x10;

/// How much of an `xinpcb_n` is read: up to the end of the local address.
const INPCB_READ: usize = 80;

/// How much of an `xsocket_n` is read: up to the end of `so_last_pid`.
const SOCKET_READ: usize = 72;

/// `inp_vflag`, at offset 44 of an `xinpcb_n`: the socket is an IPv4 one.
const INP_IPV4: u8 = 0x1;

/// `inp_vflag`: the socket is an IPv6 one.
const INP_IPV6: u8 = 0x2;

/// How many times a table is read once the system has said how big it is: it
/// can grow between the question and the reading.
const READS: usize = 4;

/// `ENOMEM`: what `sysctlbyname` answers when the room it is given is smaller
/// than the table.
#[cfg(target_os = "macos")]
const ENOMEM: i32 = nix::libc::ENOMEM;
#[cfg(not(target_os = "macos"))]
const ENOMEM: i32 = 12; // the number it has on macOS, for the tests of `read_pcblist`

/// The room for a path, in bytes: the most `proc_pidpath` takes.
#[cfg(target_os = "macos")]
const PATH_BYTES: usize = nix::libc::PROC_PIDPATHINFO_MAXSIZE as usize;

/// The sockets of a table `sysctlbyname` gave, for TCP or (`udp`) for UDP.
///
/// The walk goes by the length of each item, not by the size of a record, which
/// is not the same in every version of Darwin. The table opens with an
/// `xinpgen`, which says in its first word how long it is. Every item after it
/// begins with its length and its kind, and the next one begins where it ends,
/// rounded up to a multiple of 8. The walk ends at an item that is no longer
/// than an `xinpgen` (the one that ends the table), when fewer than 8 bytes are
/// left, and at an item that runs past the end of `buf`.
///
/// A record is an `XSO_INPCB` item and the `XSO_SOCKET` item that follows it;
/// items of other kinds are skipped. The `XSO_INPCB` item has the foreign port
/// at offset 16, the local port at 18, `inp_vflag` at 44, and the foreign and
/// the local address, 16 bytes each, at 48 and 64. When `INP_IPV4` is set the
/// addresses are IPv4 ones, the last four bytes of the 16 (at 60 and 76); when
/// it is not and `INP_IPV6` is, they are IPv6 ones; a record with neither is
/// not used. The `XSO_SOCKET` item has `so_last_pid` at offset 68: the process
/// that last used the socket. An item too short to hold what is read makes its
/// record not used.
///
/// A table that has items and not one record whose two items are long enough
/// is not laid out the way this reads it (`Layout`); one without items is
/// empty. The rows are the IPv4 ones first and then the IPv6 ones, each in the
/// order of the table.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))] // read on macOS only
fn walk(buf: &[u8], udp: bool) -> Result<Vec<Row>, ReadError> {
    let mut rows = Vec::new();
    let mut walked = false;
    let mut complete = false;
    // the pcb item of the record being read, when it is long enough to be read
    let mut open: Option<&[u8; INPCB_READ]> = None;
    for (kind, item) in items(buf) {
        walked = true;
        match kind {
            XSO_INPCB => open = item.first_chunk(),
            XSO_SOCKET => {
                // the socket item ends the record the latest pcb item began
                let (Some(pcb), Some(socket)) = (open.take(), item.first_chunk::<SOCKET_READ>())
                else {
                    continue;
                };
                complete = true;
                let pid = u32::from_le_bytes([socket[68], socket[69], socket[70], socket[71]]);
                rows.extend(row_of(pcb, pid, udp));
            }
            _ => {}
        }
    }
    if walked && !complete {
        return Err(ReadError::Layout);
    }
    // the kernel lists the sockets of both families in one run
    rows.sort_by_key(|row| row.local.is_ipv6());
    Ok(rows)
}

/// The row of a record: where the socket is, from its `xinpcb_n`, and its
/// process. `None`: the item does not say whether the socket is an IPv4 or an
/// IPv6 one, so there are no addresses to read.
fn row_of(pcb: &[u8; INPCB_READ], pid: u32, udp: bool) -> Option<Row> {
    let flags = pcb[44];
    let (local, foreign) = if flags & INP_IPV4 != 0 {
        (IpAddr::V4(ipv4(pcb, 76)), IpAddr::V4(ipv4(pcb, 60)))
    } else if flags & INP_IPV6 != 0 {
        (IpAddr::V6(ipv6(pcb, 64)), IpAddr::V6(ipv6(pcb, 48)))
    } else {
        return None;
    };
    Some(Row {
        local: SocketAddr::new(local, port(pcb, 18)),
        remote: (!udp).then_some(SocketAddr::new(foreign, port(pcb, 16))),
        owner: RowOwner::Pid(pid),
    })
}

// The readers below are given the whole of what is read of an `xinpcb_n`, and
// an offset that leaves room in it for what they read.

/// A port: two bytes in network order.
fn port(pcb: &[u8; INPCB_READ], at: usize) -> u16 {
    u16::from_be_bytes([pcb[at], pcb[at + 1]])
}

fn ipv4(pcb: &[u8; INPCB_READ], at: usize) -> Ipv4Addr {
    Ipv4Addr::new(pcb[at], pcb[at + 1], pcb[at + 2], pcb[at + 3])
}

fn ipv6(pcb: &[u8; INPCB_READ], at: usize) -> Ipv6Addr {
    let mut octets = [0; 16];
    octets.copy_from_slice(&pcb[at..at + 16]);
    Ipv6Addr::from(octets)
}

/// The items of a table, in order: the kind of each, and its bytes, which begin
/// with its length and its kind. The items end where the walk does.
struct Items<'a> {
    rest: &'a [u8],
}

/// The items of `table`, which opens with an `xinpgen`.
fn items(table: &[u8]) -> Items<'_> {
    // the first word of the `xinpgen` is its length
    let rest = word(table, 0)
        .and_then(|length| table.get(round_up(length as usize)?..))
        .unwrap_or_default();
    Items { rest }
}

impl<'a> Iterator for Items<'a> {
    type Item = (u32, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        // fewer than 8 bytes is no item
        let length = word(self.rest, 0)? as usize;
        let kind = word(self.rest, 4)?;
        // an `xinpgen` ends the table; no item is so short
        if length <= XINPGEN {
            return None;
        }
        let item = self.rest.get(..length)?;
        self.rest = self.rest.get(round_up(length)?..).unwrap_or_default();
        Some((kind, item))
    }
}

/// The length of an item up to a multiple of 8, which is where the next one
/// begins.
fn round_up(length: usize) -> Option<usize> {
    length.checked_next_multiple_of(8)
}

/// The little-endian word at `at`, when `bytes` hold one.
fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..)?.first_chunk()?;
    Some(u32::from_le_bytes(*word))
}

/// Reads a table through `get`, which calls `sysctlbyname` with the room it is
/// given and answers how many bytes the call came to. The first call has no
/// room at all: the system answers how big the table is. The room made after
/// that is an eighth bigger, for the table can grow in between; and if it was
/// not enough after all (`ENOMEM`), the room is made twice as big and the table
/// is read again, at most `READS` times. Any other error is the end of it.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))] // read on macOS only
fn read_pcblist(mut get: impl FnMut(&mut [u8]) -> io::Result<usize>) -> io::Result<Vec<u8>> {
    let size = get(&mut [])?;
    let mut room = size.saturating_add(size / 8);
    for _ in 0..READS {
        let mut table = vec![0; room];
        match get(&mut table) {
            Ok(length) => {
                table.truncate(length);
                return Ok(table);
            }
            Err(error) if error.raw_os_error() == Some(ENOMEM) => {
                room = room.saturating_mul(2);
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from_raw_os_error(ENOMEM))
}

/// The table of the sockets of one protocol, as `read_pcblist` gives it:
/// `name` is `net.inet.tcp.pcblist_n` or `net.inet.udp.pcblist_n`. One of the
/// `unsafe` exceptions of this crate: the call into the system and nothing
/// else.
///
/// Any `name` is safe: the system answers one it does not know with an error
/// code. The room is the one `read_pcblist` has made, and the call keeps no
/// part of it.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn pcblist(name: &CStr) -> io::Result<Vec<u8>> {
    read_pcblist(|room| {
        // no room: the call says how much it needs
        let pointer: *mut std::ffi::c_void = if room.is_empty() {
            std::ptr::null_mut()
        } else {
            room.as_mut_ptr().cast()
        };
        let mut length = room.len();
        // SAFETY: `name` is a NUL-terminated string; `pointer` is null or the
        // start of `room`, which holds `length` bytes that the call may write;
        // `length` is a live word; no new value is set (`newp` is null and
        // `newlen` 0). Nothing is kept past the call.
        let answered = unsafe {
            nix::libc::sysctlbyname(name.as_ptr(), pointer, &mut length, std::ptr::null_mut(), 0)
        };
        if answered == 0 {
            Ok(length)
        } else {
            Err(io::Error::last_os_error())
        }
    })
}

/// The path of the executable of process `pid`, when the system says: `None`
/// when `pid` is not a process (it has exited) or the system will not say. One
/// of the `unsafe` exceptions of this crate: the call into the system and
/// nothing else.
///
/// Any `pid` is safe: the system answers one that is not a process with an
/// error code.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn pid_path(pid: u32) -> Option<String> {
    let pid = i32::try_from(pid).ok()?;
    let mut path = vec![0u8; PATH_BYTES];
    // SAFETY: `path` holds `PATH_BYTES` bytes, which is the size the call is
    // told, and the call writes no more than that. Nothing is kept past the
    // call.
    let length =
        unsafe { nix::libc::proc_pidpath(pid, path.as_mut_ptr().cast(), PATH_BYTES as u32) };
    path_written(&path, length)
}

/// The path `proc_pidpath` wrote into `buffer`: it answered `length`, the
/// number of bytes of the path. Not a path when it answered none (0 or less) or
/// more than the buffer holds.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))] // read on macOS only
fn path_written(buffer: &[u8], length: i32) -> Option<String> {
    let length = usize::try_from(length).ok().filter(|&length| length > 0)?;
    Some(String::from_utf8_lossy(buffer.get(..length)?).into_owned())
}

/// The name of a program: what follows the last slash of its path.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))] // read on macOS only
fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// The tables the kernel keeps.
#[cfg(target_os = "macos")]
pub(crate) struct Tables;

#[cfg(target_os = "macos")]
impl Source for Tables {
    fn read(&self, protocol: Protocol) -> Result<Vec<Row>, ReadError> {
        let (name, udp) = match protocol {
            Protocol::Tcp => (c"net.inet.tcp.pcblist_n", false),
            Protocol::Udp => (c"net.inet.udp.pcblist_n", true),
        };
        let table = pcblist(name).map_err(|error| ReadError::Os(error.to_string()))?;
        walk(&table, udp)
    }

    fn describe(&self, pid: u32) -> Option<(String, Option<String>)> {
        let path = pid_path(pid)?;
        Some((file_name(&path).to_string(), Some(path)))
    }

    fn addresses(&self) -> Vec<IpAddr> {
        interface_addresses()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tables below are built byte by byte after the layout `walk` reads,
    // not captured from a machine. What an item holds beyond what is read is
    // `NOISE`, so that a read of the wrong place shows.

    const NOISE: u8 = 0xA5;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn put(bytes: &mut [u8], at: usize, value: &[u8]) {
        bytes[at..at + value.len()].copy_from_slice(value);
    }

    /// An item of `length` bytes: its length and its kind, and noise.
    fn item(kind: u32, length: usize) -> Vec<u8> {
        let mut bytes = vec![NOISE; length];
        put(&mut bytes, 0, &(length as u32).to_le_bytes());
        put(&mut bytes, 4, &kind.to_le_bytes());
        bytes
    }

    /// An item filled up with empty bytes to a multiple of 8, as the kernel
    /// writes them.
    fn padded(mut bytes: Vec<u8>) -> Vec<u8> {
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        bytes
    }

    /// An `xinpgen`: its length (24), a count, two generation numbers.
    fn xinpgen(count: u32) -> Vec<u8> {
        let mut bytes = vec![0; 24];
        put(&mut bytes, 0, &24u32.to_le_bytes());
        put(&mut bytes, 4, &count.to_le_bytes());
        put(&mut bytes, 8, &5u64.to_le_bytes());
        put(&mut bytes, 16, &6u64.to_le_bytes());
        bytes
    }

    /// A table: the `xinpgen` that opens it, `parts` (each of them one item or
    /// several), the `xinpgen` that ends it.
    fn table(parts: &[Vec<u8>]) -> Vec<u8> {
        [xinpgen(0), parts.concat(), xinpgen(0)].concat()
    }

    /// The 16 bytes the kernel keeps an address in: an IPv4 address is the
    /// last four of them, after twelve empty ones.
    fn sixteen(ip: IpAddr) -> [u8; 16] {
        match ip {
            IpAddr::V4(ip) => {
                let mut bytes = [0; 16];
                put(&mut bytes, 12, &ip.octets());
                bytes
            }
            IpAddr::V6(ip) => ip.octets(),
        }
    }

    /// An `XSO_INPCB` item of `length` bytes (at least 80) for a socket
    /// between `local` and `foreign`, with `flags` as its `inp_vflag`.
    fn inpcb(length: usize, flags: u8, local: &str, foreign: &str) -> Vec<u8> {
        let (local, foreign) = (addr(local), addr(foreign));
        let mut bytes = item(0x10, length);
        put(&mut bytes, 16, &foreign.port().to_be_bytes());
        put(&mut bytes, 18, &local.port().to_be_bytes());
        bytes[44] = flags;
        put(&mut bytes, 48, &sixteen(foreign.ip()));
        put(&mut bytes, 64, &sixteen(local.ip()));
        bytes
    }

    /// An `XSO_SOCKET` item of `length` bytes (at least 72) of a socket that
    /// process `pid` used last. Where the item has room for it, `so_e_pid` is
    /// another process, `pid + 1`.
    fn xsocket(length: usize, pid: u32) -> Vec<u8> {
        let mut bytes = item(0x1, length);
        put(&mut bytes, 68, &pid.to_le_bytes());
        if length >= 76 {
            put(&mut bytes, 72, &(pid + 1).to_le_bytes());
        }
        bytes
    }

    /// A record as the kernel writes it: the two items.
    fn record(flags: u8, local: &str, foreign: &str, pid: u32) -> Vec<u8> {
        [inpcb(136, flags, local, foreign), xsocket(96, pid)].concat()
    }

    fn row(local: &str, remote: Option<&str>, pid: u32) -> Row {
        Row {
            local: addr(local),
            remote: remote.map(addr),
            owner: RowOwner::Pid(pid),
        }
    }

    #[test]
    fn a_tcp_record_is_read_field_by_field() {
        let mut pcb = vec![0u8; 80];
        put(&mut pcb, 0, &80u32.to_le_bytes()); // length
        put(&mut pcb, 4, &0x10u32.to_le_bytes()); // kind: XSO_INPCB
        put(&mut pcb, 16, &[0x18, 0x08]); // foreign port 6152
        put(&mut pcb, 18, &[0xC3, 0x50]); // local port 50000
        pcb[44] = 0x1; // inp_vflag: INP_IPV4
        put(&mut pcb, 60, &[10, 1, 2, 3]); // foreign address
        put(&mut pcb, 76, &[127, 0, 0, 1]); // local address
        let mut socket = vec![0u8; 72];
        put(&mut socket, 0, &72u32.to_le_bytes()); // length
        put(&mut socket, 4, &0x1u32.to_le_bytes()); // kind: XSO_SOCKET
        put(&mut socket, 68, &[0xD2, 0x04, 0, 0]); // so_last_pid 1234
        assert_eq!(
            walk(&table(&[pcb, socket]), false),
            Ok(vec![row("127.0.0.1:50000", Some("10.1.2.3:6152"), 1234)])
        );
    }

    #[test]
    fn an_ipv6_record_is_read_field_by_field() {
        let mut pcb = vec![0u8; 80];
        put(&mut pcb, 0, &80u32.to_le_bytes()); // length
        put(&mut pcb, 4, &0x10u32.to_le_bytes()); // kind: XSO_INPCB
        put(&mut pcb, 16, &[0x18, 0x08]); // foreign port 6152
        put(&mut pcb, 18, &[0xC3, 0x50]); // local port 50000
        pcb[44] = 0x2; // inp_vflag: INP_IPV6
        // foreign address 2001:db8::7
        put(&mut pcb, 48, &[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0]);
        put(&mut pcb, 56, &[0, 0, 0, 0, 0, 0, 0, 7]);
        // local address ::1
        put(&mut pcb, 64, &[0, 0, 0, 0, 0, 0, 0, 0]);
        put(&mut pcb, 72, &[0, 0, 0, 0, 0, 0, 0, 1]);
        let mut socket = vec![0u8; 72];
        put(&mut socket, 0, &72u32.to_le_bytes()); // length
        put(&mut socket, 4, &0x1u32.to_le_bytes()); // kind: XSO_SOCKET
        put(&mut socket, 68, &[0xD2, 0x04, 0, 0]); // so_last_pid 1234
        assert_eq!(
            walk(&table(&[pcb, socket]), false),
            Ok(vec![row("[::1]:50000", Some("[2001:db8::7]:6152"), 1234)])
        );
    }

    #[test]
    fn a_udp_socket_has_no_remote_end() {
        let bytes = table(&[
            record(0x1, "0.0.0.0:5353", "0.0.0.0:0", 11),
            record(0x2, "[::]:5353", "[::]:0", 12),
        ]);
        assert_eq!(
            walk(&bytes, true),
            Ok(vec![
                row("0.0.0.0:5353", None, 11),
                row("[::]:5353", None, 12)
            ])
        );
        // the same table, read as TCP, has the other end
        assert_eq!(
            walk(&bytes, false),
            Ok(vec![
                row("0.0.0.0:5353", Some("0.0.0.0:0"), 11),
                row("[::]:5353", Some("[::]:0"), 12)
            ])
        );
    }

    #[test]
    fn the_sizes_of_the_items_do_not_change_what_is_read() {
        // the sizes the structures have had in different versions of Darwin
        let sizes = [(136, 96), (160, 104)];
        let tables = sizes.map(|(inpcb_size, socket_size)| {
            table(&[
                [
                    inpcb(inpcb_size, 0x1, "127.0.0.1:50000", "10.1.2.3:6152"),
                    xsocket(socket_size, 1234),
                ]
                .concat(),
                [
                    inpcb(inpcb_size, 0x2, "[::1]:50001", "[2001:db8::7]:6152"),
                    xsocket(socket_size, 77),
                ]
                .concat(),
            ])
        });
        let rows = vec![
            row("127.0.0.1:50000", Some("10.1.2.3:6152"), 1234),
            row("[::1]:50001", Some("[2001:db8::7]:6152"), 77),
        ];
        for bytes in tables {
            assert_eq!(walk(&bytes, false), Ok(rows.clone()), "{}", bytes.len());
        }
    }

    #[test]
    fn the_process_is_the_one_that_last_used_the_socket() {
        // so_e_pid, which an item of 104 bytes has, is another process
        let bytes = table(&[[
            inpcb(160, 0x1, "127.0.0.1:1", "10.0.0.9:80"),
            xsocket(104, 1234),
        ]
        .concat()]);
        assert_eq!(
            walk(&bytes, false),
            Ok(vec![row("127.0.0.1:1", Some("10.0.0.9:80"), 1234)])
        );
    }

    #[test]
    fn kinds_that_are_not_known_are_skipped() {
        let bytes = table(&[
            item(0x4, 64),
            inpcb(136, 0x1, "127.0.0.1:1", "10.0.0.9:80"),
            // what the kernel writes between the two items of a record, and
            // after them
            item(0x2, 48),
            xsocket(96, 10),
            item(0x4, 48),
            item(0x8, 104),
            item(0x20, 232),
            item(0xDEAD, 32),
            // kinds are told apart as numbers, not as bits
            item(0x11, 96),
            item(0x30, 96),
            item(0x3, 96),
            record(0x1, "127.0.0.1:2", "10.0.0.9:80", 20),
        ]);
        assert_eq!(
            walk(&bytes, false),
            Ok(vec![
                row("127.0.0.1:1", Some("10.0.0.9:80"), 10),
                row("127.0.0.1:2", Some("10.0.0.9:80"), 20),
            ])
        );
    }

    #[test]
    fn the_next_item_begins_where_this_one_ends_rounded_up_to_8() {
        for length in [25, 30, 31, 32, 33, 39, 40, 100, 103, 104, 4097] {
            let bytes = table(&[
                padded(item(0x2, length)),
                record(0x1, "127.0.0.1:2", "10.0.0.9:80", 20),
            ]);
            assert_eq!(
                walk(&bytes, false),
                Ok(vec![row("127.0.0.1:2", Some("10.0.0.9:80"), 20)]),
                "an item of {length} bytes"
            );
        }
    }

    #[test]
    fn the_xinpgen_that_opens_the_table_says_how_long_it_is() {
        let good = record(0x1, "127.0.0.1:1", "10.0.0.9:80", 10);
        let rows = Ok(vec![row("127.0.0.1:1", Some("10.0.0.9:80"), 10)]);
        // what is read next is where it says it ends, not at the 24th byte
        for length in [24u32, 32, 40] {
            let mut header = xinpgen(0x10);
            header.resize(length as usize, 0);
            put(&mut header, 0, &length.to_le_bytes());
            let bytes = [header, good.clone(), xinpgen(0)].concat();
            assert_eq!(walk(&bytes, false), rows, "an xinpgen of {length} bytes");
        }
    }

    #[test]
    fn the_xinpgen_that_ends_the_table_ends_the_walk() {
        let before = record(0x1, "127.0.0.1:1", "10.0.0.9:80", 10);
        let after = record(0x1, "127.0.0.1:2", "10.0.0.9:80", 20);
        // the count of an xinpgen is where the kind of an item would be: a
        // count that looks like the kind of an item does not make it one
        for count in [0, 0x1, 0x10, 0xDEAD] {
            let bytes = [xinpgen(0), before.clone(), xinpgen(count), after.clone()].concat();
            assert_eq!(
                walk(&bytes, false),
                Ok(vec![row("127.0.0.1:1", Some("10.0.0.9:80"), 10)]),
                "a count of {count}"
            );
        }
    }

    #[test]
    fn inp_vflag_says_which_addresses_a_record_has() {
        let good = record(0x1, "127.0.0.1:3", "10.0.0.9:80", 30);
        let good_row = row("127.0.0.1:3", Some("10.0.0.9:80"), 30);
        // the flags, the two ends as they are written, and what is read
        let cases = [
            (
                0x1,
                "127.0.0.1:1000",
                "127.0.0.2:80",
                Some(row("127.0.0.1:1000", Some("127.0.0.2:80"), 7)),
            ),
            (
                0x2,
                "[::1]:1000",
                "[::2]:80",
                Some(row("[::1]:1000", Some("[::2]:80"), 7)),
            ),
            // both: the IPv4 addresses, the last four bytes of the 16
            (
                0x3,
                "[2001:db8::a00:1]:1000",
                "[2001:db8::a00:2]:80",
                Some(row("10.0.0.1:1000", Some("10.0.0.2:80"), 7)),
            ),
            // other flags do not matter
            (
                0x81,
                "127.0.0.1:1000",
                "127.0.0.2:80",
                Some(row("127.0.0.1:1000", Some("127.0.0.2:80"), 7)),
            ),
            (
                0xFE,
                "[::1]:1000",
                "[::2]:80",
                Some(row("[::1]:1000", Some("[::2]:80"), 7)),
            ),
            // neither: no addresses to read, no row
            (0x0, "127.0.0.1:1000", "127.0.0.2:80", None),
            (0x4, "127.0.0.1:1000", "127.0.0.2:80", None),
            (0xFC, "[::1]:1000", "[::2]:80", None),
        ];
        for (flags, local, foreign, expected) in cases {
            // next to a record that is read, so that what there is to say of
            // a table that has none does not come into it
            let bytes = table(&[record(flags, local, foreign, 7), good.clone()]);
            let mut rows: Vec<Row> = expected.into_iter().collect();
            rows.push(good_row.clone());
            rows.sort_by_key(|row| row.local.is_ipv6());
            assert_eq!(walk(&bytes, false), Ok(rows), "inp_vflag {flags:#x}");
        }
    }

    #[test]
    fn an_item_that_is_too_short_makes_its_record_not_used() {
        let good = record(0x1, "127.0.0.1:3", "10.0.0.9:80", 30);
        let rows = Ok(vec![row("127.0.0.1:3", Some("10.0.0.9:80"), 30)]);
        let socket = xsocket(72, 11);
        let pcb = inpcb(80, 0x1, "127.0.0.1:4", "10.0.0.9:80");
        let cases = [
            // the item of the pcb has to reach the end of the local address
            (
                "an XSO_INPCB item of 79 bytes",
                [padded(item(0x10, 79)), socket.clone()],
            ),
            (
                "an XSO_INPCB item of 72 bytes",
                [item(0x10, 72), socket.clone()],
            ),
            // and the item of the socket the end of so_last_pid
            (
                "an XSO_SOCKET item of 71 bytes",
                [pcb.clone(), padded(item(0x1, 71))],
            ),
            (
                "an XSO_SOCKET item of 64 bytes",
                [pcb.clone(), item(0x1, 64)],
            ),
        ];
        for (what, short) in cases {
            let bytes = table(&[short.concat(), good.clone()]);
            assert_eq!(walk(&bytes, false), rows, "{what}");
        }
        // and the least that is long enough is
        let least = [
            inpcb(80, 0x1, "127.0.0.1:3", "10.0.0.9:80"),
            xsocket(72, 30),
        ]
        .concat();
        assert_eq!(walk(&table(&[least]), false), rows);
    }

    #[test]
    fn a_socket_item_belongs_to_the_record_the_latest_pcb_item_began() {
        let a = inpcb(80, 0x1, "127.0.0.1:1", "10.0.0.9:80");
        let b = inpcb(80, 0x1, "127.0.0.1:2", "10.0.0.9:80");
        let cases = [
            // a socket item with no record before it
            (
                vec![xsocket(72, 5), a.clone(), xsocket(72, 6)],
                vec![row("127.0.0.1:1", Some("10.0.0.9:80"), 6)],
            ),
            // a second one, after the record is complete
            (
                vec![a.clone(), xsocket(72, 6), xsocket(72, 7)],
                vec![row("127.0.0.1:1", Some("10.0.0.9:80"), 6)],
            ),
            // a record that never had its socket item
            (
                vec![a.clone(), b.clone(), xsocket(72, 8)],
                vec![row("127.0.0.1:2", Some("10.0.0.9:80"), 8)],
            ),
            // other items between the two
            (
                vec![a.clone(), item(0x2, 48), item(0x4, 48), xsocket(72, 9)],
                vec![row("127.0.0.1:1", Some("10.0.0.9:80"), 9)],
            ),
        ];
        for (parts, rows) in cases {
            assert_eq!(walk(&table(&parts), false), Ok(rows));
        }
    }

    #[test]
    fn no_complete_record_among_the_items_is_a_layout_that_is_not_known() {
        let pcb = inpcb(80, 0x1, "127.0.0.1:1", "10.0.0.9:80");
        // the socket item runs past the end of what there is
        let cut = [xinpgen(0), pcb.clone(), item(0x1, 96)[..40].to_vec()].concat();
        assert_eq!(walk(&cut, false), Err(ReadError::Layout));
        let cases = [
            // kinds that are not the two
            vec![item(0x2, 48)],
            vec![item(0x20, 232), item(0x8, 104)],
            // a pcb item with nothing after it, and a socket item with
            // nothing before it
            vec![pcb.clone()],
            vec![xsocket(72, 5)],
            vec![xsocket(72, 5), pcb.clone()],
            // an item too short
            vec![padded(item(0x10, 72)), xsocket(72, 5)],
            vec![pcb.clone(), item(0x1, 64)],
        ];
        for parts in cases {
            assert_eq!(walk(&table(&parts), true), Err(ReadError::Layout));
            assert_eq!(walk(&table(&parts), false), Err(ReadError::Layout));
        }
    }

    #[test]
    fn a_table_without_items_is_empty() {
        assert_eq!(walk(&table(&[]), false), Ok(Vec::new()));
        assert_eq!(walk(&xinpgen(0), false), Ok(Vec::new()));
        // not even a table
        for length in 0..40 {
            assert_eq!(walk(&vec![0; length], true), Ok(Vec::new()), "{length}");
        }
        // an xinpgen, and less than an item
        let bytes = [xinpgen(0), vec![1, 2, 3, 4, 5, 6, 7]].concat();
        assert_eq!(walk(&bytes, false), Ok(Vec::new()));
    }

    #[test]
    fn an_item_that_runs_past_the_buffer_ends_the_walk() {
        let before = record(0x1, "127.0.0.1:1", "10.0.0.9:80", 10);
        let rows = Ok(vec![row("127.0.0.1:1", Some("10.0.0.9:80"), 10)]);
        let tails = [
            // longer than the bytes there are, by a lot and by one
            item(0x10, 4096)[..100].to_vec(),
            inpcb(136, 0x1, "127.0.0.1:2", "10.0.0.9:80")[..135].to_vec(),
            [u32::MAX.to_le_bytes(), 0x10u32.to_le_bytes()].concat(),
            // an item whose bytes would be enough for what is read of it, if
            // it were not cut short
            [
                inpcb(80, 0x1, "127.0.0.1:2", "10.0.0.9:80"),
                xsocket(96, 20)[..76].to_vec(),
            ]
            .concat(),
            // less than a header
            vec![0xFF; 7],
        ];
        for tail in tails {
            let bytes = [xinpgen(0), before.clone(), tail].concat();
            assert_eq!(walk(&bytes, false), rows);
        }
        // an item with all its bytes, and the next one cut
        let bytes = [
            xinpgen(0),
            before.clone(),
            inpcb(136, 0x1, "127.0.0.1:2", "10.0.0.9:80"),
            item(0x1, 96)[..95].to_vec(),
        ]
        .concat();
        assert_eq!(walk(&bytes, false), rows);
        // an xinpgen that says it is longer than the table
        for length in [4096u32, u32::MAX] {
            let mut bytes = [xinpgen(0), before.clone(), xinpgen(0)].concat();
            put(&mut bytes, 0, &length.to_le_bytes());
            assert_eq!(walk(&bytes, false), Ok(Vec::new()), "{length}");
        }
    }

    #[test]
    fn no_bytes_make_the_walk_panic() {
        let good = table(&[
            record(0x1, "127.0.0.1:1", "10.0.0.9:80", 10),
            item(0x8, 104),
            record(0x2, "[::1]:2", "[2001:db8::7]:80", 20),
            [
                inpcb(160, 0x3, "[2001:db8::a00:1]:3", "[2001:db8::a00:2]:80"),
                xsocket(104, 30),
            ]
            .concat(),
        ]);
        for udp in [false, true] {
            // a table cut anywhere
            for cut in 0..=good.len() {
                let _ = walk(&good[..cut], udp);
            }
            // and with any one of its bytes changed
            for at in 0..good.len() {
                for value in [0x00, 0x01, 0x07, 0x7F, 0x80, 0xFF] {
                    let mut bytes = good.clone();
                    bytes[at] = value;
                    let _ = walk(&bytes, udp);
                }
            }
        }
        // and bytes that are not a table: items of lengths and kinds that are
        // often those of a table, with whatever in them
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let lengths = [0u32, 7, 8, 16, 24, 25, 32, 72, 80, 96, 136, 4096, u32::MAX];
        let kinds = [0u32, 1, 2, 0x10, 0x20, 0xFF, u32::MAX];
        for _ in 0..3000 {
            let mut bytes = xinpgen(0);
            for _ in 0..(next() % 6) {
                let length = lengths[(next() % lengths.len() as u64) as usize];
                let kind = kinds[(next() % kinds.len() as u64) as usize];
                bytes.extend_from_slice(&length.to_le_bytes());
                bytes.extend_from_slice(&kind.to_le_bytes());
                for _ in 0..(next() % 150) {
                    bytes.push(next() as u8);
                }
            }
            let _ = walk(&bytes, next() % 2 == 0);
        }
    }

    #[test]
    fn the_ipv4_rows_come_before_the_ipv6_ones() {
        let bytes = table(&[
            record(0x2, "[::1]:1", "[::2]:80", 1),
            record(0x1, "127.0.0.1:2", "127.0.0.2:80", 2),
            record(0x2, "[::1]:3", "[::2]:80", 3),
            record(0x1, "127.0.0.1:4", "127.0.0.2:80", 4),
        ]);
        let ports: Vec<_> = walk(&bytes, false)
            .unwrap()
            .iter()
            .map(|row| row.local.port())
            .collect();
        // each family in the order of the table
        assert_eq!(ports, [2, 4, 1, 3]);
    }

    #[test]
    fn a_table_is_asked_for_its_size_with_no_room_and_then_read_with_an_eighth_more() {
        let mut calls = Vec::new();
        let bytes = read_pcblist(|room| {
            calls.push(room.len());
            if room.is_empty() {
                return Ok(800);
            }
            room[..800].fill(7);
            // the table is shorter than the room it was given
            Ok(800)
        })
        .unwrap();
        assert_eq!(bytes, vec![7; 800]);
        assert_eq!(calls, [0, 900]);
    }

    #[test]
    fn a_table_that_grows_between_the_calls_is_read_again_with_double_the_room() {
        let mut calls = Vec::new();
        let bytes = read_pcblist(|room| {
            calls.push(room.len());
            match room.len() {
                0 => Ok(800),
                900 => Err(io::Error::from_raw_os_error(ENOMEM)),
                _ => {
                    room.fill(9);
                    Ok(1000)
                }
            }
        })
        .unwrap();
        assert_eq!(bytes, vec![9; 1000]);
        assert_eq!(calls, [0, 900, 1800]);
    }

    #[test]
    fn a_table_that_keeps_growing_is_given_up_after_four_readings() {
        let mut calls = Vec::new();
        let error = read_pcblist(|room| {
            calls.push(room.len());
            if room.is_empty() {
                Ok(800)
            } else {
                Err(io::Error::from_raw_os_error(ENOMEM))
            }
        })
        .unwrap_err();
        // the question, and four readings
        assert_eq!(calls, [0, 900, 1800, 3600, 7200]);
        assert_eq!(error.raw_os_error(), Some(ENOMEM));
    }

    #[test]
    fn any_other_error_is_the_systems_and_ends_the_reading() {
        // on the question
        let mut calls = 0;
        let error = read_pcblist(|_| {
            calls += 1;
            Err(io::Error::from_raw_os_error(1))
        })
        .unwrap_err();
        assert_eq!((calls, error.raw_os_error()), (1, Some(1)));

        // and on the first reading
        let mut calls = 0;
        let error = read_pcblist(|room| {
            calls += 1;
            if room.is_empty() {
                Ok(800)
            } else {
                Err(io::Error::from_raw_os_error(1))
            }
        })
        .unwrap_err();
        assert_eq!((calls, error.raw_os_error()), (2, Some(1)));
    }

    #[test]
    fn a_path_is_the_bytes_the_system_says_it_wrote() {
        let mut buffer = vec![0u8; 64];
        put(&mut buffer, 0, b"/usr/sbin/mDNSResponder");
        assert_eq!(
            path_written(&buffer, 23),
            Some("/usr/sbin/mDNSResponder".to_string())
        );
        // what is not UTF-8 is not lost, nor is it a failure
        put(&mut buffer, 0, b"/tmp/\xFF\xFEx");
        assert_eq!(
            path_written(&buffer, 8),
            Some("/tmp/\u{FFFD}\u{FFFD}x".to_string())
        );
    }

    #[test]
    fn no_path_is_a_length_that_is_not_a_path() {
        let buffer = vec![b'a'; 64];
        assert_eq!(path_written(&buffer, 0), None);
        assert_eq!(path_written(&buffer, -1), None);
        assert_eq!(path_written(&buffer, i32::MIN), None);
        // more than the buffer holds
        assert_eq!(path_written(&buffer, 65), None);
        assert_eq!(path_written(&buffer, i32::MAX), None);
        assert_eq!(path_written(&buffer, 64).as_deref().map(str::len), Some(64));
    }

    #[test]
    fn the_name_of_a_program_is_what_follows_the_last_slash() {
        assert_eq!(file_name("/Applications/App.app/Contents/MacOS/App"), "App");
        assert_eq!(file_name("/usr/bin/curl"), "curl");
        assert_eq!(file_name("curl"), "curl");
        assert_eq!(file_name("/opt/my tools/a b"), "a b");
    }

    #[cfg(target_os = "macos")]
    mod system {
        use super::super::*;
        use crate::procinfo::{Finder, Owner, Query};
        use std::net::{TcpListener, TcpStream, UdpSocket};
        use std::time::{Duration, Instant};

        /// The longest a test waits for the system.
        const LIMIT: Duration = Duration::from_secs(10);

        fn assert_this_process(owner: Option<Owner>, what: &str) {
            let owner = owner.unwrap_or_else(|| panic!("{what}: no owner found"));
            assert_eq!(owner.pid, std::process::id(), "{what}");
            // the system says where the program is as a path with no link in it
            let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
            let path = exe.to_str().unwrap();
            let name = exe.file_name().unwrap().to_str().unwrap();
            assert_eq!(owner.name, name, "{what}: {owner:?}");
            assert_eq!(owner.path.as_deref(), Some(path), "{what}: {owner:?}");
        }

        /// A TCP connection and a UDP socket of this process on the loopback
        /// address `ip`, and what the kernel's own tables say of them. `false`:
        /// the machine cannot bind `ip`, and nothing was asked.
        fn this_process_owns_sockets_on(ip: IpAddr) -> bool {
            let Ok(listener) = TcpListener::bind((ip, 0)) else {
                return false;
            };
            let Ok(datagrams) = UdpSocket::bind((ip, 0)) else {
                return false;
            };
            let server = listener.local_addr().unwrap();
            let client = TcpStream::connect_timeout(&server, LIMIT).unwrap();

            let finder = Finder::new();
            let tcp = finder.owner(&Query {
                protocol: Protocol::Tcp,
                local: client.local_addr().unwrap(),
                remote_port: Some(server.port()),
                since: Instant::now(),
            });
            assert_this_process(
                tcp,
                &format!("the client's end of a TCP connection on {ip}"),
            );
            let udp = finder.owner(&Query {
                protocol: Protocol::Udp,
                local: datagrams.local_addr().unwrap(),
                remote_port: None,
                since: Instant::now(),
            });
            assert_this_process(udp, &format!("a UDP socket on {ip}"));
            true
        }

        #[test]
        fn this_process_owns_its_loopback_sockets() {
            assert!(this_process_owns_sockets_on(Ipv4Addr::LOCALHOST.into()));
            if !this_process_owns_sockets_on(Ipv6Addr::LOCALHOST.into()) {
                eprintln!("[::1] cannot be bound on this machine: its half of the test is skipped");
            }
        }

        #[test]
        fn a_process_that_does_not_exist_is_not_described() {
            assert_eq!(Tables.describe(u32::MAX), None);
        }
    }
}
