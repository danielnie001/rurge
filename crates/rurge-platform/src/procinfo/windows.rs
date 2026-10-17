//! Windows: the tables of TCP connections and UDP sockets the system keeps,
//! with the process that owns each socket (`GetExtendedTcpTable` and
//! `GetExtendedUdpTable`), and the path of the executable of a process
//! (`QueryFullProcessImageNameW`). Everything but the calls into the system is
//! compiled on every host: how the system lays a table out in memory is read
//! by the parsers below, which are tested on bytes.

use super::{Row, RowOwner};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

#[cfg(windows)]
use super::{Protocol, ReadError, Source, interface_addresses};
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::net::IpAddr;
#[cfg(windows)]
use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;

/// The state of a TCP connection that has ended and is only waiting out the
/// network (`MIB_TCP_STATE_TIME_WAIT`).
const TIME_WAIT: u32 = 11;

/// How long a row is in each of the four tables.
const TCP4_ROW: usize = 24;
const TCP6_ROW: usize = 56;
const UDP4_ROW: usize = 12;
const UDP6_ROW: usize = 28;

// A table is a count of rows (a little-endian word at offset 0) and the rows,
// from offset 4. A row is made of words in the order of the machine, which is
// little-endian, except for what is a port or an address: a port is in
// network order in the first two bytes of its word, an IPv4 address is its
// four bytes in order, and so is an IPv6 address (16 bytes). The offsets below
// are those of the system's `MIB_..ROW_OWNER_PID` structures, from the start
// of a row.

/// `MIB_TCPROW_OWNER_PID`: state 0, local address 4, local port 8, remote
/// address 12, remote port 16, process 20.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // read on Windows only
fn parse_tcp4(table: &[u8]) -> Vec<Row> {
    rows(table, TCP4_ROW)
        .filter(|row| dword(row, 0) != TIME_WAIT && dword(row, 20) != 0)
        .map(|row| Row {
            local: SocketAddr::new(ipv4(row, 4).into(), port(row, 8)),
            remote: Some(SocketAddr::new(ipv4(row, 12).into(), port(row, 16))),
            owner: RowOwner::Pid(dword(row, 20)),
        })
        .collect()
}

/// `MIB_TCP6ROW_OWNER_PID`: local address 0, local scope id 16, local port 20,
/// remote address 24, remote scope id 40, remote port 44, state 48, process
/// 52. The scope id is not part of the address: a socket address that has one
/// would not be equal to the one a session comes from.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // read on Windows only
fn parse_tcp6(table: &[u8]) -> Vec<Row> {
    rows(table, TCP6_ROW)
        .filter(|row| dword(row, 48) != TIME_WAIT && dword(row, 52) != 0)
        .map(|row| Row {
            local: SocketAddr::new(ipv6(row, 0).into(), port(row, 20)),
            remote: Some(SocketAddr::new(ipv6(row, 24).into(), port(row, 44))),
            owner: RowOwner::Pid(dword(row, 52)),
        })
        .collect()
}

/// `MIB_UDPROW_OWNER_PID`: local address 0, local port 4, process 8.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // read on Windows only
fn parse_udp4(table: &[u8]) -> Vec<Row> {
    rows(table, UDP4_ROW)
        .filter(|row| dword(row, 8) != 0)
        .map(|row| Row {
            local: SocketAddr::new(ipv4(row, 0).into(), port(row, 4)),
            remote: None,
            owner: RowOwner::Pid(dword(row, 8)),
        })
        .collect()
}

/// `MIB_UDP6ROW_OWNER_PID`: local address 0, scope id 16, local port 20,
/// process 24.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // read on Windows only
fn parse_udp6(table: &[u8]) -> Vec<Row> {
    rows(table, UDP6_ROW)
        .filter(|row| dword(row, 24) != 0)
        .map(|row| Row {
            local: SocketAddr::new(ipv6(row, 0).into(), port(row, 20)),
            remote: None,
            owner: RowOwner::Pid(dword(row, 24)),
        })
        .collect()
}

/// The rows of a table, `size` bytes each. A count that is more than the
/// bytes hold is as many as they hold, and a row that is cut short is not one.
fn rows(table: &[u8], size: usize) -> impl Iterator<Item = &[u8]> {
    let count = table
        .first_chunk()
        .map_or(0, |count| u32::from_le_bytes(*count));
    table
        .get(4..)
        .unwrap_or_default()
        .chunks_exact(size)
        .take(count as usize)
}

// The readers below are given a whole row, of the size the parser asked for,
// and an offset that is inside it.

fn dword(row: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([row[at], row[at + 1], row[at + 2], row[at + 3]])
}

/// A port: the first two bytes of its word, in network order.
fn port(row: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([row[at], row[at + 1]])
}

fn ipv4(row: &[u8], at: usize) -> Ipv4Addr {
    Ipv4Addr::new(row[at], row[at + 1], row[at + 2], row[at + 3])
}

fn ipv6(row: &[u8], at: usize) -> Ipv6Addr {
    let mut octets = [0; 16];
    octets.copy_from_slice(&row[at..at + 16]);
    Ipv6Addr::from(octets)
}

/// The name of a program: what follows the last backslash of its path.
#[cfg_attr(not(any(windows, test)), allow(dead_code))] // read on Windows only
fn file_name(path: &str) -> &str {
    path.rsplit_once('\\').map_or(path, |(_, name)| name)
}

/// `AF_INET` and `AF_INET6`, as `ws2def.h` has them: two numbers are not worth
/// the `Win32_Networking_WinSock` feature of `windows-sys`.
#[cfg(windows)]
const AF_INET: u32 = 2;
#[cfg(windows)]
const AF_INET6: u32 = 23;

/// The process that is the kernel: it cannot be opened, and it has no file.
#[cfg(windows)]
const SYSTEM_PID: u32 = 4;

/// How many times a table is read once the system has said how big it is: it
/// can grow between the question and the reading.
#[cfg(windows)]
const READS: usize = 4;

/// The room for a path, in UTF-16 units: the longest path there is, and the
/// unit that ends it.
#[cfg(windows)]
const PATH_UNITS: u32 = 32_768;

/// Reads a table through `get`, which calls one of the system's table
/// functions with the room it is given and the size of that room in bytes. The
/// first call has no room at all: the system answers
/// `ERROR_INSUFFICIENT_BUFFER` and sets the size to what the table needs. The
/// calls after it have that much room, and go on with the new size if the
/// table has grown in the meantime: at most `READS` readings. `get` answers 0
/// when it has written the table; any other answer is the end of it.
///
/// The bytes are those of the table in memory (the system writes words in the
/// order of the machine, which is little-endian), and the room is made of
/// words, so that it is aligned as the system wants it.
#[cfg(windows)]
fn read_table(mut get: impl FnMut(&mut [u32], &mut u32) -> u32) -> io::Result<Vec<u8>> {
    let mut room: Vec<u32> = Vec::new();
    let mut size = 0;
    // the question, and then the readings
    for _ in 0..=READS {
        match get(&mut room, &mut size) {
            0 => return Ok(room.iter().flat_map(|word| word.to_le_bytes()).collect()),
            ERROR_INSUFFICIENT_BUFFER => {
                room = vec![0; size.div_ceil(4) as usize];
                size = (room.len() * 4) as u32;
            }
            error => return Err(io::Error::from_raw_os_error(error as i32)),
        }
    }
    Err(io::Error::from_raw_os_error(
        ERROR_INSUFFICIENT_BUFFER as i32,
    ))
}

/// Where the system is to write a table: nowhere when there is no room, which
/// is how it is asked how much room it needs.
#[cfg(windows)]
fn room_pointer(room: &mut [u32]) -> *mut std::ffi::c_void {
    if room.is_empty() {
        std::ptr::null_mut()
    } else {
        room.as_mut_ptr().cast()
    }
}

/// The table of the TCP connections of one address family (`AF_INET` or
/// `AF_INET6`), listening sockets left out, with the process of each, as
/// `read_table` gives it. One of the `unsafe` exceptions of this crate: the
/// call into the system and nothing else.
///
/// Any `family` is safe: the system answers one it does not know with an error
/// code. The room and the size are the ones `read_table` has made to fit each
/// other, and the call keeps neither.
#[cfg(windows)]
#[allow(unsafe_code)]
fn tcp_table(family: u32) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_CONNECTIONS,
    };
    read_table(|room, size| {
        // SAFETY: the pointer is null or the start of `room`, which holds
        // `*size` bytes (whole words, so aligned) that the call may write;
        // `size` is a live word. Nothing is kept past the call.
        unsafe {
            GetExtendedTcpTable(
                room_pointer(room),
                size,
                0,
                family,
                TCP_TABLE_OWNER_PID_CONNECTIONS,
                0,
            )
        }
    })
}

/// The table of the UDP sockets of one address family (`AF_INET` or
/// `AF_INET6`) with the process of each, as `read_table` gives it. One of the
/// `unsafe` exceptions of this crate: the call into the system and nothing
/// else.
///
/// Any `family` is safe: the system answers one it does not know with an error
/// code. The room and the size are the ones `read_table` has made to fit each
/// other, and the call keeps neither.
#[cfg(windows)]
#[allow(unsafe_code)]
fn udp_table(family: u32) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedUdpTable, UDP_TABLE_OWNER_PID,
    };
    read_table(|room, size| {
        // SAFETY: the pointer is null or the start of `room`, which holds
        // `*size` bytes (whole words, so aligned) that the call may write;
        // `size` is a live word. Nothing is kept past the call.
        unsafe { GetExtendedUdpTable(room_pointer(room), size, 0, family, UDP_TABLE_OWNER_PID, 0) }
    })
}

/// The path of the executable of process `pid`, when the system says: `None`
/// when the process cannot be opened (it has exited, or it is one that a user
/// of this kind may not look at) or will not say. One of the `unsafe`
/// exceptions of this crate: opening the process, the handle it makes and the
/// asking, and nothing else.
///
/// Any `pid` is safe: the system answers one that is not a process with an
/// error code.
#[cfg(windows)]
#[allow(unsafe_code)]
fn image_path(pid: u32) -> Option<String> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    // SAFETY: plain call; a non-null result is a new handle nobody else owns.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    // SAFETY: `process` is a valid handle owned by nothing else; it is closed
    // when this goes out of scope.
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    let mut path = vec![0u16; PATH_UNITS as usize];
    let mut length = PATH_UNITS;
    // SAFETY: the handle is valid and outlives the call; `path` holds
    // `length` units that the call may write, and `length` is a live word.
    let asked = unsafe {
        QueryFullProcessImageNameW(
            process.as_raw_handle(),
            PROCESS_NAME_WIN32,
            path.as_mut_ptr(),
            &mut length,
        )
    };
    if asked == 0 {
        return None;
    }
    // `length` is the units written, without the one that ends the path
    Some(String::from_utf16_lossy(path.get(..length as usize)?))
}

/// The tables the system keeps.
#[cfg(windows)]
pub(crate) struct Tables;

#[cfg(windows)]
impl Source for Tables {
    fn read(&self, protocol: Protocol) -> Result<Vec<Row>, ReadError> {
        let mut rows = match protocol {
            Protocol::Tcp => tcp_table(AF_INET).map(|table| parse_tcp4(&table)),
            Protocol::Udp => udp_table(AF_INET).map(|table| parse_udp4(&table)),
        }
        .map_err(|error| unreadable("IPv4", &error))?;
        rows.extend(
            match protocol {
                Protocol::Tcp => tcp_table(AF_INET6).map(|table| parse_tcp6(&table)),
                Protocol::Udp => udp_table(AF_INET6).map(|table| parse_udp6(&table)),
            }
            .map_err(|error| unreadable("IPv6", &error))?,
        );
        Ok(rows)
    }

    fn describe(&self, pid: u32) -> Option<(String, Option<String>)> {
        if pid == SYSTEM_PID {
            // not opened: it is the kernel's, and it has no file
            return Some(("System".to_string(), None));
        }
        let path = image_path(pid)?;
        Some((file_name(&path).to_string(), Some(path)))
    }

    fn addresses(&self) -> Vec<IpAddr> {
        interface_addresses()
    }
}

/// Why the table of an address family could not be read, in the system's own
/// words.
#[cfg(windows)]
fn unreadable(family: &str, error: &io::Error) -> ReadError {
    ReadError::Os(format!("{family}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    fn v6(text: &str) -> [u8; 16] {
        text.parse::<Ipv6Addr>().unwrap().octets()
    }

    /// A table as the system lays it out in memory: the number of rows it says
    /// it has, then the rows.
    fn table(count: u32, rows: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = count.to_le_bytes().to_vec();
        for row in rows {
            bytes.extend_from_slice(row);
        }
        bytes
    }

    fn word(value: u32) -> [u8; 4] {
        value.to_le_bytes()
    }

    /// The word that holds a port: the port in network byte order in its first
    /// two bytes, the other two bytes empty.
    fn port_word(port: u16) -> [u8; 4] {
        let [high, low] = port.to_be_bytes();
        [high, low, 0, 0]
    }

    const ESTABLISHED: u32 = 5;
    const TIME_WAIT: u32 = 11;

    fn tcp4(state: u32, local: ([u8; 4], u16), remote: ([u8; 4], u16), pid: u32) -> Vec<u8> {
        [
            word(state),
            local.0,
            port_word(local.1),
            remote.0,
            port_word(remote.1),
            word(pid),
        ]
        .concat()
    }

    /// The addresses are written as text, with a scope id: (address, scope id,
    /// port).
    fn tcp6(state: u32, local: (&str, u32, u16), remote: (&str, u32, u16), pid: u32) -> Vec<u8> {
        [
            &v6(local.0)[..],
            &word(local.1),
            &port_word(local.2),
            &v6(remote.0),
            &word(remote.1),
            &port_word(remote.2),
            &word(state),
            &word(pid),
        ]
        .concat()
    }

    fn udp4(local: ([u8; 4], u16), pid: u32) -> Vec<u8> {
        [local.0, port_word(local.1), word(pid)].concat()
    }

    fn udp6(local: (&str, u32, u16), pid: u32) -> Vec<u8> {
        [
            &v6(local.0)[..],
            &word(local.1),
            &port_word(local.2),
            &word(pid),
        ]
        .concat()
    }

    fn pid_of(rows: &[Row]) -> Vec<u32> {
        rows.iter()
            .map(|row| match row.owner {
                RowOwner::Pid(pid) => pid,
                RowOwner::Inode(_) => panic!("a Windows row names a process, not an inode"),
            })
            .collect()
    }

    #[test]
    fn a_tcp4_row_is_read_field_by_field() {
        let bytes: [u8; 28] = [
            1, 0, 0, 0, // one row
            5, 0, 0, 0, // state: established
            127, 0, 0, 1, // local address
            0xC3, 0x50, 0, 0, // local port 50000
            10, 1, 2, 3, // remote address
            0x18, 0x08, 0, 0, // remote port 6152
            0xD2, 0x04, 0, 0, // process 1234
        ];
        assert_eq!(
            parse_tcp4(&bytes),
            vec![Row {
                local: addr("127.0.0.1:50000"),
                remote: Some(addr("10.1.2.3:6152")),
                owner: RowOwner::Pid(1234),
            }]
        );
    }

    #[test]
    fn a_tcp6_row_is_read_field_by_field() {
        let bytes: [u8; 60] = [
            1, 0, 0, 0, // one row
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, // local address ::1
            0, 0, 0, 0, // local scope id
            0xC3, 0x50, 0, 0, // local port 50000
            0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, // remote 2001:db8::7
            0, 0, 0, 0, // remote scope id
            0x18, 0x08, 0, 0, // remote port 6152
            5, 0, 0, 0, // state: established
            0xD2, 0x04, 0, 0, // process 1234
        ];
        assert_eq!(
            parse_tcp6(&bytes),
            vec![Row {
                local: addr("[::1]:50000"),
                remote: Some(addr("[2001:db8::7]:6152")),
                owner: RowOwner::Pid(1234),
            }]
        );
    }

    #[test]
    fn a_udp4_row_is_read_field_by_field() {
        let bytes: [u8; 16] = [
            1, 0, 0, 0, // one row
            192, 168, 1, 20, // local address
            0x14, 0xE9, 0, 0, // local port 5353
            0xD2, 0x04, 0, 0, // process 1234
        ];
        assert_eq!(
            parse_udp4(&bytes),
            vec![Row {
                local: addr("192.168.1.20:5353"),
                remote: None,
                owner: RowOwner::Pid(1234),
            }]
        );
    }

    #[test]
    fn a_udp6_row_is_read_field_by_field() {
        let bytes: [u8; 32] = [
            1, 0, 0, 0, // one row
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // local address ::
            0, 0, 0, 0, // scope id
            0x14, 0xE9, 0, 0, // local port 5353
            0xD2, 0x04, 0, 0, // process 1234
        ];
        assert_eq!(
            parse_udp6(&bytes),
            vec![Row {
                local: addr("[::]:5353"),
                remote: None,
                owner: RowOwner::Pid(1234),
            }]
        );
    }

    #[test]
    fn a_port_is_the_first_two_bytes_of_its_word_in_network_order() {
        // 6152 is 0x1808: 0x18 first. The other two bytes of the word are not
        // part of the port, whatever they hold.
        let row = [
            word(ESTABLISHED),
            [10, 0, 0, 1],
            [0x18, 0x08, 0xFF, 0xFF],
            [10, 0, 0, 2],
            [0x01, 0x02, 0xFF, 0xFF],
            word(7),
        ]
        .concat();
        let rows = parse_tcp4(&table(1, &[row]));
        assert_eq!(rows[0].local, addr("10.0.0.1:6152"));
        assert_eq!(rows[0].remote, Some(addr("10.0.0.2:258")));
    }

    #[test]
    fn rows_come_in_the_order_of_the_table() {
        let rows = [
            tcp4(ESTABLISHED, ([10, 0, 0, 1], 1000), ([10, 0, 0, 9], 80), 30),
            tcp4(ESTABLISHED, ([10, 0, 0, 1], 1001), ([10, 0, 0, 9], 80), 10),
            tcp4(ESTABLISHED, ([10, 0, 0, 1], 1002), ([10, 0, 0, 9], 80), 20),
        ];
        let parsed = parse_tcp4(&table(3, &rows));
        assert_eq!(pid_of(&parsed), [30, 10, 20]);
        assert_eq!(
            parsed
                .iter()
                .map(|row| row.local.port())
                .collect::<Vec<_>>(),
            [1000, 1001, 1002]
        );
    }

    #[test]
    fn a_count_larger_than_the_rows_that_fit_reads_the_rows_that_fit() {
        let local4 = ([127, 0, 0, 1], 1);
        let remote4 = ([127, 0, 0, 1], 2);
        let rows = [
            tcp4(ESTABLISHED, local4, remote4, 11),
            tcp4(ESTABLISHED, local4, remote4, 12),
        ];
        assert_eq!(pid_of(&parse_tcp4(&table(3, &rows))), [11, 12]);

        let rows = [
            tcp6(ESTABLISHED, ("::1", 0, 1), ("::1", 0, 2), 21),
            tcp6(ESTABLISHED, ("::1", 0, 1), ("::1", 0, 2), 22),
        ];
        assert_eq!(pid_of(&parse_tcp6(&table(3, &rows))), [21, 22]);

        let rows = [udp4(local4, 31), udp4(local4, 32)];
        assert_eq!(pid_of(&parse_udp4(&table(3, &rows))), [31, 32]);

        let rows = [udp6(("::1", 0, 1), 41), udp6(("::1", 0, 1), 42)];
        assert_eq!(pid_of(&parse_udp6(&table(3, &rows))), [41, 42]);
    }

    #[test]
    fn a_count_that_no_buffer_could_hold_does_not_panic() {
        let local = ([127, 0, 0, 1], 1);
        let rows = [udp4(local, 5)];
        assert_eq!(pid_of(&parse_udp4(&table(u32::MAX, &rows))), [5]);
        assert!(parse_tcp6(&table(u32::MAX, &[])).is_empty());
    }

    #[test]
    fn a_row_that_is_cut_short_is_not_read() {
        let local = ([127, 0, 0, 1], 1);
        let remote = ([127, 0, 0, 1], 2);
        let mut bytes = table(2, &[tcp4(ESTABLISHED, local, remote, 11)]);
        bytes.extend_from_slice(&tcp4(ESTABLISHED, local, remote, 12)[..23]);
        assert_eq!(pid_of(&parse_tcp4(&bytes)), [11]);
    }

    #[test]
    fn a_table_without_rows_is_empty() {
        let row = udp4(([127, 0, 0, 1], 1), 5);
        // a count of 0, even with a row after it
        assert!(parse_udp4(&table(0, &[row])).is_empty());
        for bytes in [
            &b""[..],
            &[1][..],
            &[1, 0][..],
            &[1, 0, 0][..],
            &[1, 0, 0, 0][..],
        ] {
            assert!(parse_tcp4(bytes).is_empty(), "{bytes:?}");
            assert!(parse_tcp6(bytes).is_empty(), "{bytes:?}");
            assert!(parse_udp4(bytes).is_empty(), "{bytes:?}");
            assert!(parse_udp6(bytes).is_empty(), "{bytes:?}");
        }
    }

    #[test]
    fn tcp_rows_in_time_wait_and_rows_of_process_0_are_skipped() {
        let local = ([10, 0, 0, 1], 1000);
        let remote = ([10, 0, 0, 9], 80);
        let rows = [
            tcp4(ESTABLISHED, local, remote, 1),
            tcp4(TIME_WAIT, local, remote, 2),
            tcp4(ESTABLISHED, local, remote, 0),
            tcp4(TIME_WAIT, local, remote, 0),
            tcp4(2, local, remote, 3),
        ];
        assert_eq!(pid_of(&parse_tcp4(&table(5, &rows))), [1, 3]);

        let rows = [
            tcp6(ESTABLISHED, ("::1", 0, 1000), ("::1", 0, 80), 1),
            tcp6(TIME_WAIT, ("::1", 0, 1000), ("::1", 0, 80), 2),
            tcp6(ESTABLISHED, ("::1", 0, 1000), ("::1", 0, 80), 0),
            tcp6(TIME_WAIT, ("::1", 0, 1000), ("::1", 0, 80), 0),
            tcp6(2, ("::1", 0, 1000), ("::1", 0, 80), 3),
        ];
        assert_eq!(pid_of(&parse_tcp6(&table(5, &rows))), [1, 3]);
    }

    #[test]
    fn udp_rows_of_process_0_are_skipped() {
        let rows = [
            udp4(([0, 0, 0, 0], 5353), 1),
            udp4(([0, 0, 0, 0], 5354), 0),
            udp4(([0, 0, 0, 0], 5355), 3),
        ];
        assert_eq!(pid_of(&parse_udp4(&table(3, &rows))), [1, 3]);

        let rows = [
            udp6(("::", 0, 5353), 1),
            udp6(("::", 0, 5354), 0),
            udp6(("::", 0, 5355), 3),
        ];
        assert_eq!(pid_of(&parse_udp6(&table(3, &rows))), [1, 3]);
    }

    #[test]
    fn the_scope_id_of_an_ipv6_row_is_not_in_its_address() {
        let rows = parse_tcp6(&table(
            1,
            &[tcp6(
                ESTABLISHED,
                ("fe80::1234", 12, 80),
                ("fe80::5678", 12, 443),
                7,
            )],
        ));
        // a socket address compares its scope id too
        assert_eq!(rows[0].local, addr("[fe80::1234]:80"));
        assert_eq!(rows[0].remote, Some(addr("[fe80::5678]:443")));
        let SocketAddr::V6(local) = rows[0].local else {
            panic!("an IPv6 row has an IPv6 address");
        };
        assert_eq!(local.scope_id(), 0);
        assert_eq!(local.flowinfo(), 0);

        let rows = parse_udp6(&table(1, &[udp6(("fe80::1234", 12, 5353), 7)]));
        assert_eq!(rows[0].local, addr("[fe80::1234]:5353"));
    }

    #[test]
    fn the_name_of_a_process_is_what_follows_the_last_backslash() {
        assert_eq!(file_name(r"C:\Program Files\App\app.exe"), "app.exe");
        assert_eq!(file_name(r"\\server\share\tool.exe"), "tool.exe");
        assert_eq!(file_name("app.exe"), "app.exe");
        assert_eq!(file_name(r"D:\a b\C.EXE"), "C.EXE");
    }

    #[cfg(windows)]
    mod system {
        use super::super::*;
        use crate::procinfo::{Finder, Owner, Query};
        use std::net::{TcpListener, TcpStream, UdpSocket};
        use std::time::{Duration, Instant};

        /// The longest a test waits for the system.
        const LIMIT: Duration = Duration::from_secs(10);

        #[test]
        fn a_table_is_asked_for_its_size_with_no_room_and_then_read() {
            let mut calls = Vec::new();
            let bytes = read_table(|room, size| {
                calls.push((room.len() * 4, *size));
                if room.is_empty() {
                    *size = 8;
                    return ERROR_INSUFFICIENT_BUFFER;
                }
                room[0] = 1;
                // the bytes 1, 2, 3, 4 in memory
                room[1] = 0x0403_0201;
                0
            })
            .unwrap();
            assert_eq!(bytes, [1, 0, 0, 0, 1, 2, 3, 4]);
            assert_eq!(calls, [(0, 0), (8, 8)]);
        }

        #[test]
        fn a_table_that_grows_between_the_calls_is_asked_for_again() {
            let mut calls = Vec::new();
            let bytes = read_table(|room, size| {
                calls.push((room.len() * 4, *size));
                match room.len() * 4 {
                    0 => *size = 8,
                    8 => *size = 16,
                    _ => {
                        room.fill(0x0101_0101);
                        return 0;
                    }
                }
                ERROR_INSUFFICIENT_BUFFER
            })
            .unwrap();
            assert_eq!(bytes, [1u8; 16]);
            assert_eq!(calls, [(0, 0), (8, 8), (16, 16)]);
        }

        #[test]
        fn a_table_that_keeps_growing_is_given_up_after_four_readings() {
            let mut calls = 0;
            let error = read_table(|room, size| {
                calls += 1;
                *size = (room.len() as u32 + 1) * 4;
                ERROR_INSUFFICIENT_BUFFER
            })
            .unwrap_err();
            // the question, and four readings
            assert_eq!(calls, 5);
            assert_eq!(error.raw_os_error(), Some(ERROR_INSUFFICIENT_BUFFER as i32));
        }

        #[test]
        fn any_other_answer_is_the_systems_error() {
            let mut calls = 0;
            let error = read_table(|_, _| {
                calls += 1;
                87
            })
            .unwrap_err();
            assert_eq!(calls, 1);
            assert_eq!(error.raw_os_error(), Some(87));

            // also after the question was answered
            let error = read_table(|room, size| {
                if room.is_empty() {
                    *size = 8;
                    ERROR_INSUFFICIENT_BUFFER
                } else {
                    5
                }
            })
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(5));
        }

        fn assert_this_process(owner: Option<Owner>, what: &str) {
            let owner = owner.unwrap_or_else(|| panic!("{what}: no owner found"));
            assert_eq!(owner.pid, std::process::id(), "{what}");
            let exe = std::env::current_exe().unwrap();
            let path = exe.to_str().unwrap();
            let name = exe.file_name().unwrap().to_str().unwrap();
            assert!(owner.name.eq_ignore_ascii_case(name), "{what}: {owner:?}");
            let found = owner.path.as_deref().unwrap_or_default();
            assert!(found.eq_ignore_ascii_case(path), "{what}: {owner:?}");
        }

        /// A TCP connection and a UDP socket of this process on the loopback
        /// address `ip`, and what the system's own tables say of them. `false`:
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
        fn the_system_process_has_no_path() {
            assert_eq!(Tables.describe(4), Some(("System".to_string(), None)));
        }

        #[test]
        fn a_process_that_does_not_exist_is_not_described() {
            assert_eq!(Tables.describe(u32::MAX), None);
        }

        #[test]
        fn the_addresses_of_the_machine_include_its_loopback_address() {
            assert!(
                Tables
                    .addresses()
                    .contains(&IpAddr::V4(Ipv4Addr::LOCALHOST))
            );
        }

        /// A measurement, not a gate: a new lookup — a read of the tables and
        /// the asking after the path — with a few hundred loopback connections
        /// open, its parts, and the cost of listing the machine's addresses.
        #[test]
        #[ignore = "a measurement, not a gate: run on its own with --nocapture"]
        fn a_lookup_with_many_connections_open() {
            const CONNECTIONS: usize = 500;
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let server = listener.local_addr().unwrap();
            let mut held = Vec::new();
            for _ in 0..CONNECTIONS {
                let client = TcpStream::connect_timeout(&server, LIMIT).unwrap();
                let (accepted, _) = listener.accept().unwrap();
                held.push((client, accepted));
            }
            let last = held.last().unwrap().0.local_addr().unwrap();

            let started = Instant::now();
            let owner = Finder::new().owner(&Query {
                protocol: Protocol::Tcp,
                local: last,
                remote_port: Some(server.port()),
                since: Instant::now(),
            });
            let lookup = started.elapsed();
            assert_this_process(owner, "the last connection");

            let started = Instant::now();
            let rows = Tables.read(Protocol::Tcp).unwrap();
            let read = started.elapsed();
            let started = Instant::now();
            let described = Tables.describe(std::process::id());
            let describe = started.elapsed();
            assert!(described.is_some());
            let started = Instant::now();
            let addresses = interface_addresses();
            let first_listing = started.elapsed();
            let started = Instant::now();
            let _ = interface_addresses();
            let second_listing = started.elapsed();

            println!(
                "{CONNECTIONS} loopback connections open ({} TCP rows in the tables)",
                rows.len()
            );
            println!("a new lookup: {lookup:?}");
            println!("  reading both TCP tables: {read:?}");
            println!("  asking after the path of one process: {describe:?}");
            println!(
                "listing the machine's {} addresses: {first_listing:?}, again: {second_listing:?}",
                addresses.len()
            );
        }
    }
}
