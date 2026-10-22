//! Linux: the tables of TCP connections and UDP sockets that the kernel prints
//! under `/proc/net` and, because those name a socket and not a process, the
//! finding of the process that has the socket open, by looking at the
//! descriptors of the processes (`/proc/<pid>/fd`). The path of a program is
//! where `/proc/<pid>/exe` points.
//!
//! All it asks of the system is to read a file, list a directory and read a
//! link, which is what [`Fs`] is. So all of it is compiled on every host and
//! tested against a `/proc` that is kept in memory; only [`RealFs`] touches
//! the system.

use super::{Protocol, ReadError, Row, RowOwner, Source, interface_addresses, lock};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;

/// How many processes are remembered as the ones a socket was found for
/// lately: the next socket of a process that was just found is likely to be
/// one of its own.
const RECENT: usize = 16;

/// What is asked of the file system: the text of a file, the names in a
/// directory and the place a link points to. Paths are absolute, written with
/// `/`.
pub(crate) trait Fs: Send + Sync {
    fn read(&self, path: &str) -> io::Result<String>;
    fn list(&self, path: &str) -> io::Result<Vec<String>>;
    fn link(&self, path: &str) -> io::Result<String>;
}

/// The file system of this machine.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))] // made by a finder on Linux only
pub(crate) struct RealFs;

impl Fs for RealFs {
    fn read(&self, path: &str) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn list(&self, path: &str) -> io::Result<Vec<String>> {
        Ok(std::fs::read_dir(path)?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect())
    }

    fn link(&self, path: &str) -> io::Result<String> {
        std::fs::read_link(path).map(|target| target.to_string_lossy().into_owned())
    }
}

/// The sockets of a table that the kernel prints under `/proc/net`: `tcp`,
/// `tcp6`, `udp` or `udp6` (`v6` and `udp` say which). The first line says what
/// the columns are. Each line after it is a socket, in columns told apart by
/// white space: the number of the line (with a colon), the local end, the
/// remote end, the state, the queues, the timer, the retransmits, the user,
/// the timeout and the inode of the socket, and then what the kernel adds.
///
/// An end is `address:port`. The port is a number in hexadecimal. An IPv4
/// address is the 32-bit number the kernel keeps — the four bytes of the
/// address as they go on the network — printed as that number is in the memory
/// of the machine: `127.0.0.1` is `0100007F` on a little-endian machine
/// (`little_endian`) and `7F000001` on a big-endian one. An IPv6 address is
/// four of those, in order.
///
/// A socket of inode 0 (one that only waits out the network, `TIME_WAIT` for
/// one) is read and is no row; a line that cannot be read is skipped. A table
/// of which, blank lines left out, there are lines and none can be read is not
/// laid out the way this reads it (`ReadError::Layout`), and it is not an empty
/// table: what rests on it would find nobody, without a word. The header alone
/// is an empty table. A UDP socket has no remote end, whatever the kernel
/// prints for one that is connected.
fn parse_proc_net(
    text: &str,
    v6: bool,
    little_endian: bool,
    udp: bool,
) -> Result<Vec<Row>, ReadError> {
    let mut rows = Vec::new();
    let (mut lines, mut read) = (0, 0);
    for line in text.lines().skip(1).filter(|line| !line.trim().is_empty()) {
        lines += 1;
        match socket_on(line, v6, little_endian, udp) {
            Some(Line::Socket(row)) => {
                read += 1;
                rows.push(row);
            }
            Some(Line::Waiting) => read += 1,
            None => {}
        }
    }
    if lines > 0 && read == 0 {
        return Err(ReadError::Layout);
    }
    Ok(rows)
}

/// A line of a table that can be read.
enum Line {
    /// A socket that a process may have open.
    Socket(Row),
    /// A connection that only waits out the network.
    Waiting,
}

/// The line of a table, or `None` when it cannot be read.
fn socket_on(line: &str, v6: bool, little_endian: bool, udp: bool) -> Option<Line> {
    let mut columns = line.split_whitespace();
    // the first column is the number of the line
    let local = columns.nth(1)?;
    let remote = columns.next()?;
    // the state, the queues, the timer, the retransmits, the user and the
    // timeout come before the inode
    let inode: u64 = columns.nth(6)?.parse().ok()?;
    let local = parse_endpoint(local, v6, little_endian)?;
    let remote = parse_endpoint(remote, v6, little_endian)?;
    if inode == 0 {
        return Some(Line::Waiting);
    }
    Some(Line::Socket(Row {
        local,
        remote: (!udp).then_some(remote),
        owner: RowOwner::Inode(inode),
    }))
}

/// `address:port`, as a table prints it.
fn parse_endpoint(text: &str, v6: bool, little_endian: bool) -> Option<SocketAddr> {
    let (address, port) = text.split_once(':')?;
    let ip = if v6 {
        IpAddr::V6(Ipv6Addr::from(address6(address, little_endian)?))
    } else {
        IpAddr::V4(Ipv4Addr::from(word(address, little_endian)?))
    };
    if !is_hex(port) {
        return None;
    }
    Some(SocketAddr::new(ip, u16::from_str_radix(port, 16).ok()?))
}

/// Hexadecimal digits and nothing else: `from_str_radix` also takes a sign.
fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The four bytes of the address that is printed as these eight digits.
fn word(digits: &str, little_endian: bool) -> Option<[u8; 4]> {
    if digits.len() != 8 || !is_hex(digits) {
        return None;
    }
    let number = u32::from_str_radix(digits, 16).ok()?;
    Some(if little_endian {
        number.to_le_bytes()
    } else {
        number.to_be_bytes()
    })
}

/// The sixteen bytes of the address that is printed as these 32 digits.
fn address6(digits: &str, little_endian: bool) -> Option<[u8; 16]> {
    if digits.len() != 32 {
        return None;
    }
    let mut octets = [0; 16];
    for (at, part) in octets.chunks_exact_mut(4).enumerate() {
        part.copy_from_slice(&word(digits.get(at * 8..at * 8 + 8)?, little_endian)?);
    }
    Some(octets)
}

/// The inode of the socket that a descriptor is, from where its link points:
/// `socket:[12345]`.
fn socket_inode(target: &str) -> Option<u64> {
    target
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// The name of a program: what follows the last slash of its path.
fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// The tables the kernel prints and the processes that have the sockets open,
/// as `F` shows them.
///
/// The table of inodes is only taken for the time it takes to look at or
/// change it, never while the file system is asked: a lookup that scans every
/// process does not hold up the read of the other protocol's table.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))] // made by a finder on Linux only
pub(crate) struct Tables<F: Fs> {
    fs: F,
    owners: Mutex<Owners>,
}

/// What the scans have found out, and what the tables say there is to find.
#[derive(Default)]
struct Owners {
    /// The process that has the socket of an inode open, as far as a scan has
    /// seen it. A scan notes every socket it sees, so that a burst of new
    /// connections shares it. A socket keeps the process it was first seen
    /// in.
    pids: HashMap<u64, u32>,
    /// The processes a socket was found for lately, the one found last first:
    /// where to look first for a socket that no scan has seen.
    recent: Vec<u32>,
    /// The inodes in the latest read of each protocol's tables.
    tcp: HashSet<u64>,
    udp: HashSet<u64>,
}

impl Owners {
    /// The process noted for an inode, which is then the one found last.
    fn find(&mut self, inode: u64) -> Option<u32> {
        let pid = *self.pids.get(&inode)?;
        self.recent.retain(|&recent| recent != pid);
        self.recent.insert(0, pid);
        self.recent.truncate(RECENT);
        Some(pid)
    }

    /// Notes that `pid` has these sockets open.
    fn note(&mut self, pid: u32, inodes: &[u64]) {
        for &inode in inodes {
            self.pids.entry(inode).or_insert(pid);
        }
    }

    /// Keeps the sockets that the latest read of either protocol listed.
    fn forget_unlisted(&mut self) {
        self.pids
            .retain(|inode, _| self.tcp.contains(inode) || self.udp.contains(inode));
    }
}

impl<F: Fs> Tables<F> {
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))] // called by a finder on Linux only
    pub(crate) fn new(fs: F) -> Tables<F> {
        Tables {
            fs,
            owners: Mutex::new(Owners::default()),
        }
    }

    /// The inodes of the sockets process `pid` has open. None when its
    /// descriptors cannot be listed: it has gone, or it is not this user's to
    /// look at. A descriptor that cannot be followed is not counted.
    fn sockets_of(&self, pid: u32) -> Vec<u64> {
        let dir = format!("/proc/{pid}/fd");
        let Ok(descriptors) = self.fs.list(&dir) else {
            return Vec::new();
        };
        descriptors
            .iter()
            .filter_map(|descriptor| self.fs.link(&format!("{dir}/{descriptor}")).ok())
            .filter_map(|target| socket_inode(&target))
            .collect()
    }

    /// The process that has the socket of `inode` open: the one a scan noted
    /// for it; or the one a scan finds, looking first in the processes found
    /// for a socket lately and then in all of them.
    fn find_pid(&self, inode: u64) -> Option<u32> {
        let recent = {
            let mut owners = lock(&self.owners);
            if let Some(pid) = owners.find(inode) {
                return Some(pid);
            }
            owners.recent.clone()
        };
        for &pid in &recent {
            let sockets = self.sockets_of(pid);
            let mut owners = lock(&self.owners);
            owners.note(pid, &sockets);
            if let Some(found) = owners.find(inode) {
                return Some(found);
            }
        }

        // Every process, the sockets of all of them noted: the lookups that
        // come next are for the connections that were opened with this one.
        // The directory of `/proc` that is not a process is not a number.
        let others: Vec<u32> = self
            .fs
            .list("/proc")
            .ok()?
            .iter()
            .filter_map(|name| name.parse().ok())
            .filter(|pid| !recent.contains(pid))
            .collect();
        let seen: Vec<(u32, Vec<u64>)> = others
            .into_iter()
            .map(|pid| (pid, self.sockets_of(pid)))
            .collect();
        let mut owners = lock(&self.owners);
        for (pid, sockets) in &seen {
            owners.note(*pid, sockets);
        }
        owners.find(inode)
    }

    /// Notes the inodes in the latest read of a protocol's tables, and forgets
    /// the sockets of the scans that neither protocol's latest read lists.
    fn remember(&self, protocol: Protocol, inodes: HashSet<u64>) {
        let mut owners = lock(&self.owners);
        match protocol {
            Protocol::Tcp => owners.tcp = inodes,
            Protocol::Udp => owners.udp = inodes,
        }
        owners.forget_unlisted();
    }
}

impl<F: Fs> Source for Tables<F> {
    /// The tables of the protocol: IPv4 first. A machine without IPv6 has no
    /// tables for it, which is as if they were empty. A table of either family
    /// that cannot be read at all fails the read (`ReadError::Layout`).
    fn read(&self, protocol: Protocol) -> Result<Vec<Row>, ReadError> {
        let (name, udp) = match protocol {
            Protocol::Tcp => ("tcp", false),
            Protocol::Udp => ("udp", true),
        };
        let little_endian = cfg!(target_endian = "little");
        let text = self
            .fs
            .read(&format!("/proc/net/{name}"))
            .map_err(|error| unreadable("IPv4", &error))?;
        let mut rows = parse_proc_net(&text, false, little_endian, udp)?;
        match self.fs.read(&format!("/proc/net/{name}6")) {
            Ok(text) => rows.extend(parse_proc_net(&text, true, little_endian, udp)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(unreadable("IPv6", &error)),
        }
        let inodes = rows
            .iter()
            .filter_map(|row| match row.owner {
                RowOwner::Inode(inode) => Some(inode),
                RowOwner::Pid(_) => None,
            })
            .collect();
        self.remember(protocol, inodes);
        Ok(rows)
    }

    fn pid(&self, owner: RowOwner) -> Option<u32> {
        match owner {
            RowOwner::Pid(pid) => Some(pid),
            RowOwner::Inode(inode) => self.find_pid(inode),
        }
    }

    /// The link to the file of the program says where it is, and its name is
    /// the name of the file. When the link cannot be read (the process is
    /// another user's, or the kernel's own) the name is the one the kernel
    /// gives the process, and the path is not known.
    fn describe(&self, pid: u32) -> Option<(String, Option<String>)> {
        if let Ok(target) = self.fs.link(&format!("/proc/{pid}/exe")) {
            // the file was removed or replaced while the program ran
            let path = target.strip_suffix(" (deleted)").unwrap_or(&target);
            return Some((file_name(path).to_string(), Some(path.to_string())));
        }
        let comm = self.fs.read(&format!("/proc/{pid}/comm")).ok()?;
        Some((comm.strip_suffix('\n').unwrap_or(&comm).to_string(), None))
    }

    fn addresses(&self) -> Vec<IpAddr> {
        interface_addresses()
    }
}

/// Why a table could not be read, in the system's own words.
fn unreadable(family: &str, error: &io::Error) -> ReadError {
    ReadError::Os(format!("{family}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procinfo::{ADDRESS_TTL, Finder, Owner, Query};
    use std::sync::{Arc, MutexGuard};
    use std::time::Instant;

    fn addr(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    /// The row of a socket: its two ends and its inode.
    fn socket(local: &str, remote: Option<&str>, inode: u64) -> Row {
        Row {
            local: addr(local),
            remote: remote.map(addr),
            owner: RowOwner::Inode(inode),
        }
    }

    // The tables below are written after the format in which the kernel prints
    // `/proc/net/tcp`, `tcp6`, `udp` and `udp6`, by hand: none of them was
    // captured from a machine. The first line says what the columns are. Then
    // a line is a socket: its number, its two ends (`address:port`, in
    // hexadecimal), its state, the queues, the timer, the retransmits, the
    // user, the timeout and its inode, and what the kernel adds after that.
    // The kernel prints an IPv4 address as the 32-bit number it is in the
    // memory of the machine: `127.0.0.1` is `0100007F` on a little-endian
    // machine and `7F000001` on a big-endian one; an IPv6 address is four of
    // those. Each table is spelled for both kinds of machine.

    /// What a table of IPv4 TCP sockets starts with.
    const HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";

    /// One socket, as a little-endian machine prints it: a connection from
    /// 127.0.0.1:7890 to 127.0.0.1:50000.
    const GOOD: &str = "   0: 0100007F:1ED2 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0000000000000000 20 4 30 10 -1";

    // rurge's listening socket (127.0.0.1:7890), the client's end of a
    // connection to it (from port 50000) and rurge's end of that connection, a
    // connection to a server (203.0.113.7:443), and one that only waits out
    // the network, which has no inode.
    const TCP_LITTLE: &str = concat!(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 0100007F:1ED2 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0000000000000000 100 0 0 10 0\n",
        "   1: 0100007F:C350 0100007F:1ED2 01 00000000:00000000 00:00000000 00000000  1000        0 22222 1 0000000000000000 20 4 30 10 -1\n",
        "   2: 0100007F:1ED2 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 33333 1 0000000000000000 20 4 30 10 -1\n",
        "   3: 0100000A:A028 077100CB:01BB 01 00000000:00000000 02:000A7C8E 00000000  1000        0 44444 2 0000000000000000 20 4 29 10 -1\n",
        "   4: 0100000A:A029 077100CB:01BB 06 00000000:00000000 03:000019BE 00000000     0        0 0 3 0000000000000000\n",
    );

    const TCP_BIG: &str = concat!(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 7F000001:1ED2 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11111 1 0000000000000000 100 0 0 10 0\n",
        "   1: 7F000001:C350 7F000001:1ED2 01 00000000:00000000 00:00000000 00000000  1000        0 22222 1 0000000000000000 20 4 30 10 -1\n",
        "   2: 7F000001:1ED2 7F000001:C350 01 00000000:00000000 00:00000000 00000000  1000        0 33333 1 0000000000000000 20 4 30 10 -1\n",
        "   3: 0A000001:A028 CB007107:01BB 01 00000000:00000000 02:000A7C8E 00000000  1000        0 44444 2 0000000000000000 20 4 29 10 -1\n",
        "   4: 0A000001:A029 CB007107:01BB 06 00000000:00000000 03:000019BE 00000000     0        0 0 3 0000000000000000\n",
    );

    fn tcp_rows() -> Vec<Row> {
        vec![
            socket("127.0.0.1:7890", Some("0.0.0.0:0"), 11111),
            socket("127.0.0.1:50000", Some("127.0.0.1:7890"), 22222),
            socket("127.0.0.1:7890", Some("127.0.0.1:50000"), 33333),
            socket("10.0.0.1:41000", Some("203.0.113.7:443"), 44444),
        ]
    }

    // The same, over IPv6: listening on [::1]:7890, a client's end from port
    // 50001 and rurge's end, a client that connected to rurge's IPv4 address
    // from a socket of both families (which is in this table in its mapped
    // form, `::ffff:127.0.0.1`), a connection from 2001:db8::1 to 2001:db8::7,
    // and one that waits out the network.
    const TCP6_LITTLE: &str = concat!(
        "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 00000000000000000000000001000000:1ED2 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 55555 1 0000000000000000 100 0 0 10 0\n",
        "   1: 00000000000000000000000001000000:C351 00000000000000000000000001000000:1ED2 01 00000000:00000000 00:00000000 00000000  1000        0 66666 1 0000000000000000 20 4 30 10 -1\n",
        "   2: 00000000000000000000000001000000:1ED2 00000000000000000000000001000000:C351 01 00000000:00000000 00:00000000 00000000  1000        0 77777 1 0000000000000000 20 4 30 10 -1\n",
        "   3: 0000000000000000FFFF00000100007F:C352 0000000000000000FFFF00000100007F:1ED2 01 00000000:00000000 00:00000000 00000000  1000        0 88888 1 0000000000000000 20 4 30 10 -1\n",
        "   4: B80D0120000000000000000001000000:A410 B80D0120000000000000000007000000:01BB 01 00000000:00000000 02:000A7C8E 00000000  1000        0 99999 2 0000000000000000 20 4 29 10 -1\n",
        "   5: B80D0120000000000000000001000000:A411 B80D0120000000000000000007000000:01BB 06 00000000:00000000 03:000019BE 00000000     0        0 0 3 0000000000000000\n",
    );

    const TCP6_BIG: &str = concat!(
        "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 00000000000000000000000000000001:1ED2 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 55555 1 0000000000000000 100 0 0 10 0\n",
        "   1: 00000000000000000000000000000001:C351 00000000000000000000000000000001:1ED2 01 00000000:00000000 00:00000000 00000000  1000        0 66666 1 0000000000000000 20 4 30 10 -1\n",
        "   2: 00000000000000000000000000000001:1ED2 00000000000000000000000000000001:C351 01 00000000:00000000 00:00000000 00000000  1000        0 77777 1 0000000000000000 20 4 30 10 -1\n",
        "   3: 00000000000000000000FFFF7F000001:C352 00000000000000000000FFFF7F000001:1ED2 01 00000000:00000000 00:00000000 00000000  1000        0 88888 1 0000000000000000 20 4 30 10 -1\n",
        "   4: 20010DB8000000000000000000000001:A410 20010DB8000000000000000000000007:01BB 01 00000000:00000000 02:000A7C8E 00000000  1000        0 99999 2 0000000000000000 20 4 29 10 -1\n",
        "   5: 20010DB8000000000000000000000001:A411 20010DB8000000000000000000000007:01BB 06 00000000:00000000 03:000019BE 00000000     0        0 0 3 0000000000000000\n",
    );

    fn tcp6_rows() -> Vec<Row> {
        vec![
            socket("[::1]:7890", Some("[::]:0"), 55555),
            socket("[::1]:50001", Some("[::1]:7890"), 66666),
            socket("[::1]:7890", Some("[::1]:50001"), 77777),
            socket(
                "[::ffff:127.0.0.1]:50002",
                Some("[::ffff:127.0.0.1]:7890"),
                88888,
            ),
            socket("[2001:db8::1]:42000", Some("[2001:db8::7]:443"), 99999),
        ]
    }

    // UDP: mDNS on all addresses (port 5353), a socket on the loopback address
    // (58528), DHCP as root (68), and a socket that is connected to a server
    // (10.0.0.1:44000 to 203.0.113.7:53). The number of the line is the
    // bucket of the hash table, in five places.
    const UDP_LITTLE: &str = concat!(
        "   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n",
        "  583: 00000000:14E9 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 12121 2 0000000000000000 0\n",
        "  912: 0100007F:E4A0 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 13131 2 0000000000000000 0\n",
        " 4096: 0100000A:0044 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 14141 2 0000000000000000 0\n",
        " 7000: 0100000A:ABE0 077100CB:0035 01 00000000:00000000 00:00000000 00000000  1000        0 17171 2 0000000000000000 0\n",
    );

    const UDP_BIG: &str = concat!(
        "   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n",
        "  583: 00000000:14E9 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 12121 2 0000000000000000 0\n",
        "  912: 7F000001:E4A0 00000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 13131 2 0000000000000000 0\n",
        " 4096: 0A000001:0044 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 14141 2 0000000000000000 0\n",
        " 7000: 0A000001:ABE0 CB007107:0035 01 00000000:00000000 00:00000000 00000000  1000        0 17171 2 0000000000000000 0\n",
    );

    fn udp_rows() -> Vec<Row> {
        vec![
            socket("0.0.0.0:5353", None, 12121),
            socket("127.0.0.1:58528", None, 13131),
            socket("10.0.0.1:68", None, 14141),
            socket("10.0.0.1:44000", None, 17171),
        ]
    }

    // UDP over IPv6: mDNS on all addresses and a socket on [::1] (58529).
    const UDP6_LITTLE: &str = concat!(
        "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n",
        "  583: 00000000000000000000000000000000:14E9 00000000000000000000000000000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 15151 2 0000000000000000 0\n",
        "  912: 00000000000000000000000001000000:E4A1 00000000000000000000000000000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 16161 2 0000000000000000 0\n",
    );

    const UDP6_BIG: &str = concat!(
        "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n",
        "  583: 00000000000000000000000000000000:14E9 00000000000000000000000000000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 15151 2 0000000000000000 0\n",
        "  912: 00000000000000000000000000000001:E4A1 00000000000000000000000000000000:0000 07 00000000:00000000 00:00000000 00000000  1000        0 16161 2 0000000000000000 0\n",
    );

    fn udp6_rows() -> Vec<Row> {
        vec![
            socket("[::]:5353", None, 15151),
            socket("[::1]:58529", None, 16161),
        ]
    }

    /// The spelling of a table for the machine the test runs on.
    fn native(little: &'static str, big: &'static str) -> &'static str {
        if cfg!(target_endian = "little") {
            little
        } else {
            big
        }
    }

    /// A table of IPv4 TCP sockets with these inodes, as this machine prints it.
    fn listing(inodes: &[u64]) -> String {
        let loopback = native("0100007F", "7F000001");
        let mut text = HEADER.to_string();
        for (at, inode) in inodes.iter().enumerate() {
            let port = 40000 + at;
            text += &format!(
                "{at:4}: {loopback}:{port:04X} {loopback}:1ED2 01 00000000:00000000 00:00000000 00000000  1000        0 {inode} 1 0000000000000000 20 4 30 10 -1\n"
            );
        }
        text
    }

    // ---- reading a table ----

    #[test]
    fn a_tcp_table_is_read_in_either_byte_order() {
        assert_eq!(
            parse_proc_net(TCP_LITTLE, false, true, false),
            Ok(tcp_rows())
        );
        assert_eq!(parse_proc_net(TCP_BIG, false, false, false), Ok(tcp_rows()));
    }

    #[test]
    fn a_tcp6_table_is_read_in_either_byte_order() {
        assert_eq!(
            parse_proc_net(TCP6_LITTLE, true, true, false),
            Ok(tcp6_rows())
        );
        assert_eq!(
            parse_proc_net(TCP6_BIG, true, false, false),
            Ok(tcp6_rows())
        );
    }

    #[test]
    fn the_sockets_of_a_udp_table_have_no_remote_end() {
        // not even the one that is connected to a server
        assert_eq!(
            parse_proc_net(UDP_LITTLE, false, true, true),
            Ok(udp_rows())
        );
        assert_eq!(parse_proc_net(UDP_BIG, false, false, true), Ok(udp_rows()));
        assert_eq!(
            parse_proc_net(UDP6_LITTLE, true, true, true),
            Ok(udp6_rows())
        );
        assert_eq!(parse_proc_net(UDP6_BIG, true, false, true), Ok(udp6_rows()));
        // the same lines as a table of connections do have one
        let rows = parse_proc_net(UDP_LITTLE, false, true, false).unwrap();
        assert_eq!(rows[3].remote, Some(addr("203.0.113.7:53")));
    }

    #[test]
    fn an_ipv4_address_in_its_ipv6_form_is_read_as_that() {
        let line = |address: &str| {
            format!(
                "{HEADER}   0: {address}:C352 {address}:1ED2 01 00000000:00000000 00:00000000 00000000  1000        0 88888 1 0000000000000000 20 4 30 10 -1\n"
            )
        };
        let little =
            parse_proc_net(&line("0000000000000000FFFF00000100007F"), true, true, false).unwrap();
        let big = parse_proc_net(
            &line("00000000000000000000FFFF7F000001"),
            true,
            false,
            false,
        )
        .unwrap();
        let mapped = Ipv4Addr::LOCALHOST.to_ipv6_mapped();
        for rows in [little, big] {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].local, SocketAddr::new(mapped.into(), 50002));
            assert_eq!(rows[0].remote, Some(SocketAddr::new(mapped.into(), 7890)));
            assert_eq!(
                rows[0].local.ip().to_canonical(),
                IpAddr::from(Ipv4Addr::LOCALHOST)
            );
        }
    }

    #[test]
    fn a_socket_with_no_inode_is_not_read() {
        // the kernel prints 0 for a connection that only waits out the network
        let waiting = GOOD.replace(" 11111 ", " 0 ");
        assert_ne!(waiting, GOOD);
        assert_eq!(
            parse_proc_net(&format!("{HEADER}{waiting}\n"), false, true, false),
            Ok(vec![])
        );
        // a user of number 0 is a user like the others
        let root = GOOD.replace("1000        0 11111", "   0        0 11111");
        assert_ne!(root, GOOD);
        let rows = parse_proc_net(&format!("{HEADER}{root}\n"), false, true, false).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn the_first_line_is_the_header_and_is_not_read() {
        assert_eq!(parse_proc_net("", false, true, false), Ok(vec![]));
        assert_eq!(parse_proc_net(HEADER, false, true, false), Ok(vec![]));
        // whatever it says: the sockets start on the second line
        assert_eq!(parse_proc_net(GOOD, false, true, false), Ok(vec![]));
        let rows = parse_proc_net(&format!("{HEADER}{GOOD}"), false, true, false).unwrap();
        assert_eq!(
            rows,
            [socket("127.0.0.1:7890", Some("127.0.0.1:50000"), 11111)]
        );
    }

    #[test]
    fn lines_that_cannot_be_read_are_skipped() {
        // each is the good line with one thing wrong
        let wrong = |from: &str, to: &str| {
            let line = GOOD.replacen(from, to, 1);
            assert_ne!(line, GOOD, "{from} is not in the line");
            line
        };
        let lines = [
            String::new(),
            "   ".to_string(),
            // cut short: no inode
            GOOD[..GOOD.find(" 11111").unwrap()].to_string(),
            wrong("0100007F:1ED2", "0100007F1ED2"),
            wrong("0100007F:1ED2", "0100007G:1ED2"),
            wrong("0100007F:1ED2", "0100007:1ED2"),
            wrong("0100007F:1ED2", "0100007F0:1ED2"),
            wrong("0100007F:1ED2", "00000000000000000000000001000000:1ED2"),
            wrong("0100007F:1ED2", "0100007F:1EDZ"),
            wrong("0100007F:1ED2", "0100007F:+1ED2"),
            wrong("0100007F:1ED2", "0100007F:11ED2"),
            wrong("0100007F:1ED2", "0100007F:"),
            wrong("0100007F:C350", "0100007F:C35G"),
            wrong("0100007F:C350", "0100007F"),
            wrong(" 11111 ", " abc "),
            wrong(" 11111 ", " -5 "),
            wrong(" 11111 ", " 18446744073709551616 "),
            // full-width characters look like digits and are not (and are
            // three bytes each)
            wrong("0100007F:1ED2", "０１００００７Ｆ:１ＥＤ２"),
        ];
        for line in lines {
            let text = format!("{HEADER}{line}\n{GOOD}\n");
            assert_eq!(
                parse_proc_net(&text, false, true, false),
                Ok(vec![socket(
                    "127.0.0.1:7890",
                    Some("127.0.0.1:50000"),
                    11111
                )]),
                "{line:?}"
            );
        }
    }

    /// A table with lines in it of which not one can be read is not laid out
    /// the way this reads it, and that is not the same as a table with nothing
    /// in it: every lookup that rests on it would find nobody, without a word.
    #[test]
    fn a_table_whose_lines_cannot_be_read_is_a_layout_error() {
        let wrong = |from: &str, to: &str| {
            let line = GOOD.replacen(from, to, 1);
            assert_ne!(line, GOOD, "{from} is not in the line");
            line
        };
        let unreadable = [
            // cut short: no inode
            GOOD[..GOOD.find(" 11111").unwrap()].to_string(),
            wrong("0100007F:1ED2", "0100007F1ED2"),
            wrong("0100007F:1ED2", "00000000000000000000000001000000:1ED2"),
            wrong("0100007F:1ED2", "0100007F:1EDZ"),
            wrong(" 11111 ", " abc "),
            // another layout altogether: a line of `netstat`
            "tcp        0      0 127.0.0.1:7890          127.0.0.1:50000         ESTABLISHED"
                .to_string(),
        ];
        let table = |lines: &[String]| format!("{HEADER}{}\n", lines.join("\n"));
        for line in &unreadable {
            assert_eq!(
                parse_proc_net(&table(std::slice::from_ref(line)), false, true, false),
                Err(ReadError::Layout),
                "{line:?}"
            );
        }
        // all of them in one table, and with the blank lines a table may end in
        assert_eq!(
            parse_proc_net(&table(&unreadable), false, true, false),
            Err(ReadError::Layout)
        );
        let blank_ended = format!("{}   \n\n", table(&unreadable[..1]));
        assert_eq!(
            parse_proc_net(&blank_ended, false, true, false),
            Err(ReadError::Layout)
        );
    }

    /// A connection that only waits out the network (`TIME_WAIT`) is printed
    /// with the inode 0. It is a line of this layout like any other: read, and
    /// no row, since no process has the socket open.
    #[test]
    fn time_wait_lines_are_read() {
        let v4 = TCP_LITTLE.lines().last().unwrap();
        let v6 = TCP6_LITTLE.lines().last().unwrap();
        // the state is 06, and the inode 0
        for line in [v4, v6] {
            let columns: Vec<&str> = line.split_whitespace().collect();
            assert_eq!((columns[3], columns[9]), ("06", "0"), "{line}");
        }
        let table = |header: &str, lines: &[&str]| format!("{header}\n{}\n", lines.join("\n"));
        let header4 = HEADER.trim_end();
        let header6 = TCP6_LITTLE.lines().next().unwrap();
        // a table of nothing else is read, and is empty
        assert_eq!(
            parse_proc_net(&table(header4, &[v4, v4]), false, true, false),
            Ok(vec![])
        );
        assert_eq!(
            parse_proc_net(&table(header6, &[v6]), true, true, false),
            Ok(vec![])
        );
        // next to a line that cannot be read, it is the line that was read
        assert_eq!(
            parse_proc_net(&table(header4, &["garbage", v4]), false, true, false),
            Ok(vec![])
        );
        // a line of the other address family is not a line of this layout
        assert_eq!(
            parse_proc_net(&table(header4, &[v6]), false, true, false),
            Err(ReadError::Layout)
        );
        // and the sockets next to it are rows as ever
        assert_eq!(
            parse_proc_net(&table(header4, &[v4, GOOD]), false, true, false),
            Ok(vec![socket(
                "127.0.0.1:7890",
                Some("127.0.0.1:50000"),
                11111
            )])
        );
    }

    /// A machine with no connections prints the line that says what the
    /// columns are, and nothing else: that is an empty table, not a table that
    /// cannot be read.
    #[test]
    fn a_table_with_only_its_header_is_empty() {
        let tables = [
            (HEADER.trim_end(), false, false),
            (TCP6_LITTLE.lines().next().unwrap(), true, false),
            (UDP_LITTLE.lines().next().unwrap(), false, true),
        ];
        for (header, v6, udp) in tables {
            // the line break after it, and the blank lines after that
            for end in ["", "\n", "\n\n", "\n  \n"] {
                assert_eq!(
                    parse_proc_net(&format!("{header}{end}"), v6, true, udp),
                    Ok(vec![]),
                    "{header}{end:?}"
                );
            }
        }
    }

    #[test]
    fn an_address_of_another_family_or_width_is_not_read() {
        let layout = Err(ReadError::Layout);
        assert_eq!(parse_proc_net(TCP_LITTLE, true, true, false), layout);
        assert_eq!(parse_proc_net(TCP6_LITTLE, false, true, false), layout);
        assert_eq!(parse_proc_net(UDP_LITTLE, true, true, true), layout);
        assert_eq!(parse_proc_net(UDP6_LITTLE, false, true, true), layout);

        // an IPv6 address of one digit too few or too many
        let mut lines = TCP6_LITTLE.lines();
        let header = lines.next().unwrap();
        let socket = lines.next().unwrap();
        for width in ["0100000:1ED2", "010000000:1ED2"] {
            let wrong = socket.replacen("01000000:1ED2", width, 1);
            assert_ne!(wrong, socket);
            let text = format!("{header}\n{wrong}\n");
            assert_eq!(
                parse_proc_net(&text, true, true, false),
                Err(ReadError::Layout),
                "{wrong}"
            );
        }
    }

    // ---- an in-memory /proc ----

    /// A `/proc` kept in memory. It remembers which directories were listed,
    /// and can run something at the start of every call.
    #[derive(Default)]
    struct MemFs {
        state: Mutex<MemState>,
        probe: Mutex<Option<Probe>>,
    }

    type Probe = Arc<dyn Fn(&str) + Send + Sync>;

    #[derive(Default)]
    struct MemState {
        files: HashMap<String, String>,
        /// The names in each directory, in the order a listing gives them.
        dirs: HashMap<String, Vec<String>>,
        links: HashMap<String, String>,
        /// What exists and is not for this user to read, list or follow.
        closed: HashSet<String>,
        /// Every directory that was listed, in order.
        listed: Vec<String>,
    }

    impl Fs for Arc<MemFs> {
        fn read(&self, path: &str) -> io::Result<String> {
            self.called("read");
            let state = self.state();
            if state.closed.contains(path) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            state
                .files
                .get(path)
                .cloned()
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }

        fn list(&self, path: &str) -> io::Result<Vec<String>> {
            self.called("list");
            let mut state = self.state();
            state.listed.push(path.to_string());
            if state.closed.contains(path) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            state
                .dirs
                .get(path)
                .cloned()
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }

        fn link(&self, path: &str) -> io::Result<String> {
            self.called("link");
            let state = self.state();
            if state.closed.contains(path) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            state
                .links
                .get(path)
                .cloned()
                .ok_or_else(|| io::ErrorKind::NotFound.into())
        }
    }

    impl MemFs {
        fn new() -> Arc<MemFs> {
            Arc::new(MemFs::default())
        }

        fn state(&self) -> MutexGuard<'_, MemState> {
            lock(&self.state)
        }

        /// Runs `probe` at the start of every call, with the kind of call.
        fn on_call(&self, probe: impl Fn(&str) + Send + Sync + 'static) {
            *lock(&self.probe) = Some(Arc::new(probe));
        }

        fn called(&self, kind: &str) {
            let probe = lock(&self.probe).clone();
            if let Some(probe) = probe {
                probe(kind);
            }
        }

        fn file(&self, path: &str, text: &str) {
            self.state()
                .files
                .insert(path.to_string(), text.to_string());
        }

        fn close(&self, path: &str) {
            self.state().closed.insert(path.to_string());
        }

        /// Entries of `/proc` that are not processes, listed before any process.
        fn entries_of_proc(&self, names: &[&str]) {
            let mut state = self.state();
            let proc = state.dirs.entry("/proc".to_string()).or_default();
            proc.extend(names.iter().map(|name| name.to_string()));
        }

        /// A process: `/proc` lists it, and it has the descriptors of a
        /// process that are not sockets, and one for each of these sockets.
        fn process(&self, pid: u32, inodes: &[u64]) {
            {
                let mut state = self.state();
                let name = pid.to_string();
                let proc = state.dirs.entry("/proc".to_string()).or_default();
                if !proc.contains(&name) {
                    proc.push(name);
                }
                let dir = format!("/proc/{pid}/fd");
                let names = ["0", "1", "2"].map(String::from).to_vec();
                state.dirs.insert(dir.clone(), names);
                for (name, target) in [
                    ("0", "/dev/null"),
                    ("1", "pipe:[4242]"),
                    ("2", "anon_inode:[eventpoll]"),
                ] {
                    state
                        .links
                        .insert(format!("{dir}/{name}"), target.to_string());
                }
            }
            for &inode in inodes {
                self.open_socket(pid, inode);
            }
        }

        /// The process opens a socket.
        fn open_socket(&self, pid: u32, inode: u64) {
            let mut state = self.state();
            let dir = format!("/proc/{pid}/fd");
            let descriptors = state.dirs.entry(dir.clone()).or_default();
            let descriptor = descriptors.len();
            descriptors.push(descriptor.to_string());
            state
                .links
                .insert(format!("{dir}/{descriptor}"), format!("socket:[{inode}]"));
        }

        /// The process closes a socket between the listing of its descriptors
        /// and the reading of the link: the name is still listed.
        fn lose_descriptor_of(&self, pid: u32, inode: u64) {
            let prefix = format!("/proc/{pid}/fd/");
            let target = format!("socket:[{inode}]");
            self.state()
                .links
                .retain(|path, to| !(path.starts_with(&prefix) && *to == target));
        }

        fn exit(&self, pid: u32) {
            let mut state = self.state();
            let name = pid.to_string();
            if let Some(proc) = state.dirs.get_mut("/proc") {
                proc.retain(|entry| *entry != name);
            }
            let prefix = format!("/proc/{pid}/");
            state.dirs.retain(|path, _| !path.starts_with(&prefix));
            state.links.retain(|path, _| !path.starts_with(&prefix));
            state.files.retain(|path, _| !path.starts_with(&prefix));
        }

        fn exe(&self, pid: u32, target: &str) {
            self.state()
                .links
                .insert(format!("/proc/{pid}/exe"), target.to_string());
        }

        fn comm(&self, pid: u32, text: &str) {
            self.file(&format!("/proc/{pid}/comm"), text);
        }

        fn listed(&self) -> Vec<String> {
            self.state().listed.clone()
        }

        fn forget_listed(&self) {
            self.state().listed.clear();
        }
    }

    fn tables_on(fs: &Arc<MemFs>) -> Tables<Arc<MemFs>> {
        Tables::new(fs.clone())
    }

    fn fd_dir(pid: u32) -> String {
        format!("/proc/{pid}/fd")
    }

    // ---- the tables of the kernel ----

    #[test]
    fn a_read_gives_the_ipv4_sockets_and_then_the_ipv6_ones() {
        let fs = MemFs::new();
        fs.file("/proc/net/tcp", native(TCP_LITTLE, TCP_BIG));
        fs.file("/proc/net/tcp6", native(TCP6_LITTLE, TCP6_BIG));
        fs.file("/proc/net/udp", native(UDP_LITTLE, UDP_BIG));
        fs.file("/proc/net/udp6", native(UDP6_LITTLE, UDP6_BIG));
        let tables = tables_on(&fs);

        let mut expected = tcp_rows();
        expected.extend(tcp6_rows());
        assert_eq!(tables.read(Protocol::Tcp).unwrap(), expected);

        let mut expected = udp_rows();
        expected.extend(udp6_rows());
        assert_eq!(tables.read(Protocol::Udp).unwrap(), expected);
    }

    #[test]
    fn a_machine_without_ipv6_tables_has_none_to_read() {
        let fs = MemFs::new();
        fs.file("/proc/net/tcp", native(TCP_LITTLE, TCP_BIG));
        fs.file("/proc/net/udp", native(UDP_LITTLE, UDP_BIG));
        let tables = tables_on(&fs);
        assert_eq!(tables.read(Protocol::Tcp).unwrap(), tcp_rows());
        assert_eq!(tables.read(Protocol::Udp).unwrap(), udp_rows());
    }

    #[test]
    fn a_table_of_either_family_that_cannot_be_read_at_all_fails_the_read() {
        let fs = MemFs::new();
        let (tcp, tcp6) = (native(TCP_LITTLE, TCP_BIG), native(TCP6_LITTLE, TCP6_BIG));
        let garbled = format!("{HEADER}this is no socket\n");
        let tables = tables_on(&fs);
        // whichever of the two it is, however good the other is
        for (v4, v6) in [(garbled.as_str(), tcp6), (tcp, garbled.as_str())] {
            fs.file("/proc/net/tcp", v4);
            fs.file("/proc/net/tcp6", v6);
            assert_eq!(tables.read(Protocol::Tcp), Err(ReadError::Layout));
        }
        // a machine with no connections prints the headers and nothing else
        fs.file("/proc/net/tcp", HEADER);
        fs.file("/proc/net/tcp6", TCP6_LITTLE.lines().next().unwrap());
        assert_eq!(tables.read(Protocol::Tcp), Ok(vec![]));
    }

    #[test]
    fn a_table_that_cannot_be_read_is_an_error_in_the_systems_words() {
        let fs = MemFs::new();
        let tables = tables_on(&fs);
        let ReadError::Os(words) = tables.read(Protocol::Tcp).unwrap_err() else {
            panic!("the system said no");
        };
        assert!(words.starts_with("IPv4: "), "{words}");
        assert!(!words.contains("/proc"), "{words}");

        // an IPv6 table that is not for this user to read is not an absent one
        fs.file("/proc/net/tcp", native(TCP_LITTLE, TCP_BIG));
        fs.file("/proc/net/tcp6", native(TCP6_LITTLE, TCP6_BIG));
        fs.close("/proc/net/tcp6");
        let ReadError::Os(words) = tables.read(Protocol::Tcp).unwrap_err() else {
            panic!("the system said no");
        };
        assert!(words.starts_with("IPv6: "), "{words}");
        assert!(!words.contains("/proc"), "{words}");

        // UDP has tables of its own
        assert!(tables.read(Protocol::Udp).is_err());
    }

    // ---- who has a socket open ----

    #[test]
    fn a_row_that_names_its_process_has_that_pid() {
        let tables = tables_on(&MemFs::new());
        assert_eq!(tables.pid(RowOwner::Pid(7)), Some(7));
    }

    #[test]
    fn a_socket_is_looked_for_first_in_the_processes_found_lately() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        fs.process(200, &[22]);
        let tables = tables_on(&fs);

        // nothing is known yet: every process is looked at
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
        assert_eq!(fs.listed(), ["/proc", "/proc/100/fd", "/proc/200/fd"]);

        // a socket that process has opened since is looked for in it alone
        fs.open_socket(100, 12);
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(12)), Some(100));
        assert_eq!(fs.listed(), ["/proc/100/fd"]);
    }

    #[test]
    fn the_process_used_last_is_looked_in_first() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        fs.process(200, &[22]);
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
        // answered from what the scan noted, and the process is used all the same
        assert_eq!(tables.pid(RowOwner::Inode(22)), Some(200));

        fs.open_socket(200, 23);
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(23)), Some(200));
        assert_eq!(fs.listed(), ["/proc/200/fd"]);
    }

    #[test]
    fn the_same_process_is_remembered_once() {
        let fs = MemFs::new();
        let sockets: Vec<u64> = (11..=27).collect();
        fs.process(100, &sockets);
        fs.process(200, &[88]);
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(88)), Some(200));
        // more sockets of another process, one after the other, than there are
        // places for processes
        for &inode in &sockets {
            assert_eq!(tables.pid(RowOwner::Inode(inode)), Some(100));
        }

        // the process before it is still among those looked in first
        fs.open_socket(200, 89);
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(89)), Some(200));
        assert_eq!(fs.listed(), [fd_dir(100), fd_dir(200)]);
    }

    #[test]
    fn a_socket_two_processes_have_open_is_noted_for_the_first_one_seen() {
        // a socket that was inherited, or passed on
        let fs = MemFs::new();
        fs.process(100, &[11]);
        fs.process(200, &[11]);
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
    }

    #[test]
    fn a_socket_of_no_recent_process_is_looked_for_in_every_process() {
        let fs = MemFs::new();
        fs.entries_of_proc(&["self", "thread-self", "net", "version"]);
        fs.process(100, &[11]);
        fs.process(200, &[22]);
        fs.process(300, &[33]);
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));

        fs.open_socket(200, 23);
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(23)), Some(200));
        // the process found lately first; then all the others, in the order
        // `/proc` lists them, the one it was found in as well as those after
        // it, and none twice; the entries that are not numbers are not processes
        assert_eq!(
            fs.listed(),
            ["/proc/100/fd", "/proc", "/proc/200/fd", "/proc/300/fd"]
        );
    }

    #[test]
    fn one_scan_notes_every_socket_it_sees() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        fs.process(200, &[22, 23]);
        fs.process(300, &[33]);
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(22)), Some(200));
        fs.forget_listed();

        // the sockets that were open then are answered without looking again
        for (inode, pid) in [(11, 100), (23, 200), (33, 300)] {
            assert_eq!(tables.pid(RowOwner::Inode(inode)), Some(pid));
        }
        assert!(fs.listed().is_empty());
    }

    #[test]
    fn what_nobody_has_open_is_not_found() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(99)), None);
        assert_eq!(fs.listed(), ["/proc", "/proc/100/fd"]);
        // the scan noted what it saw all the same
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
        assert!(fs.listed().is_empty());
    }

    #[test]
    fn a_directory_that_cannot_be_listed_is_skipped() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        // another user's process: its descriptors are not for this user to see
        fs.process(200, &[22]);
        fs.close(&fd_dir(200));
        fs.process(300, &[33]);
        let tables = tables_on(&fs);

        assert_eq!(tables.pid(RowOwner::Inode(33)), Some(300));
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
        assert_eq!(tables.pid(RowOwner::Inode(22)), None);
        assert!(fs.listed().contains(&fd_dir(200)));
    }

    #[test]
    fn a_descriptor_that_cannot_be_followed_is_skipped() {
        let fs = MemFs::new();
        fs.process(400, &[44, 45]);
        fs.lose_descriptor_of(400, 44);
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(45)), Some(400));
        assert_eq!(tables.pid(RowOwner::Inode(44)), None);
    }

    #[test]
    fn a_process_that_has_gone_does_not_stop_the_scan() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        fs.process(200, &[22]);
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));

        fs.exit(100);
        fs.open_socket(200, 23);
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(23)), Some(200));
        assert_eq!(fs.listed(), ["/proc/100/fd", "/proc", "/proc/200/fd"]);
    }

    #[test]
    fn at_most_sixteen_processes_are_remembered() {
        let fs = MemFs::new();
        for k in 0..17 {
            fs.process(1000 + k, &[5000 + u64::from(k)]);
        }
        let tables = tables_on(&fs);
        // 17 processes, each used once: the first is the one forgotten
        for k in 0..17 {
            assert_eq!(
                tables.pid(RowOwner::Inode(5000 + u64::from(k))),
                Some(1000 + k)
            );
        }

        // the sixteenth most recent is still looked in, the others first
        fs.open_socket(1001, 6001);
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(6001)), Some(1001));
        let recent: Vec<String> = (1001..=1016).rev().map(fd_dir).collect();
        assert_eq!(fs.listed(), recent);

        // the one before them is looked for in every process
        fs.open_socket(1000, 6000);
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(6000)), Some(1000));
        let mut expected: Vec<String> = std::iter::once(1001)
            .chain((1002..=1016).rev())
            .map(fd_dir)
            .collect();
        expected.push("/proc".to_string());
        expected.push(fd_dir(1000));
        assert_eq!(fs.listed(), expected);
    }

    #[test]
    fn a_read_drops_the_sockets_that_no_table_lists() {
        let fs = MemFs::new();
        // 99 is a socket of another kind: it is in no table
        fs.process(100, &[11, 99]);
        fs.process(200, &[22]);
        fs.file("/proc/net/tcp", &listing(&[11, 22]));
        let tables = tables_on(&fs);
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));

        tables.read(Protocol::Tcp).unwrap();
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(22)), Some(200));
        assert!(fs.listed().is_empty());
        assert_eq!(tables.pid(RowOwner::Inode(99)), Some(100));
        assert!(!fs.listed().is_empty());
    }

    #[test]
    fn what_either_protocol_lists_is_kept() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        fs.process(300, &[33]);
        fs.file("/proc/net/tcp", &listing(&[11]));
        fs.file("/proc/net/udp", &listing(&[33]));
        let tables = tables_on(&fs);
        tables.read(Protocol::Tcp).unwrap();
        tables.read(Protocol::Udp).unwrap();
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
        fs.forget_listed();

        // a read of one protocol keeps what the latest read of the other lists
        tables.read(Protocol::Tcp).unwrap();
        assert_eq!(tables.pid(RowOwner::Inode(33)), Some(300));
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
        assert!(fs.listed().is_empty());

        // and what the latest read of neither lists is gone
        fs.file("/proc/net/udp", &listing(&[]));
        tables.read(Protocol::Udp).unwrap();
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
        assert!(fs.listed().is_empty());
        assert_eq!(tables.pid(RowOwner::Inode(33)), Some(300));
        assert!(!fs.listed().is_empty());
    }

    #[test]
    fn a_read_that_fails_leaves_the_sockets_alone() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        fs.file("/proc/net/tcp", &listing(&[11]));
        let tables = tables_on(&fs);
        tables.read(Protocol::Tcp).unwrap();
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));

        fs.close("/proc/net/tcp");
        assert!(tables.read(Protocol::Tcp).is_err());
        fs.forget_listed();
        assert_eq!(tables.pid(RowOwner::Inode(11)), Some(100));
        assert!(fs.listed().is_empty());
    }

    #[test]
    fn nothing_of_the_tables_is_held_while_the_file_system_is_asked() {
        let fs = MemFs::new();
        fs.process(100, &[11]);
        fs.process(200, &[22]);
        fs.exe(200, "/usr/bin/curl");
        fs.file("/proc/net/tcp", &listing(&[11, 22]));
        let tables = Arc::new(tables_on(&fs));

        // at every call: can the tables be taken, as another thread would?
        let calls = Arc::new(Mutex::new(Vec::new()));
        let tables_again = Arc::downgrade(&tables);
        fs.on_call({
            let calls = calls.clone();
            move |kind| {
                let free = tables_again
                    .upgrade()
                    .is_some_and(|tables| tables.owners.try_lock().is_ok());
                lock(&calls).push((kind.to_string(), free));
            }
        });
        tables.read(Protocol::Tcp).unwrap();
        // every process is looked at ...
        assert_eq!(tables.pid(RowOwner::Inode(22)), Some(200));
        // ... and, for a socket the scan has not seen, the process found lately
        fs.open_socket(200, 23);
        assert_eq!(tables.pid(RowOwner::Inode(23)), Some(200));
        assert!(tables.describe(200).is_some());

        let calls = lock(&calls);
        for kind in ["read", "list", "link"] {
            assert!(calls.iter().any(|(called, _)| called == kind), "{kind}");
        }
        assert!(calls.iter().all(|(_, free)| *free), "{calls:?}");
    }

    #[test]
    fn only_socket_descriptors_name_an_inode() {
        assert_eq!(socket_inode("socket:[12345]"), Some(12345));
        assert_eq!(
            socket_inode("socket:[18446744073709551615]"),
            Some(u64::MAX)
        );
        for other in [
            "",
            "/dev/null",
            "/home/user/socket:[1]",
            "pipe:[12345]",
            "anon_inode:[eventpoll]",
            "socket:[]",
            "socket:[12",
            "socket:12]",
            "socket:[1x]",
            "socket:[-1]",
            "socket:[18446744073709551616]",
        ] {
            assert_eq!(socket_inode(other), None, "{other:?}");
        }
    }

    // ---- what a process is ----

    #[test]
    fn a_program_is_where_the_link_to_its_file_points() {
        let fs = MemFs::new();
        fs.exe(100, "/usr/bin/curl");
        fs.comm(100, "curl\n");
        // the file was replaced while the program ran
        fs.exe(200, "/opt/my app/bin/server (deleted)");
        // a name of the kernel's is not a name to go by when the file is known
        fs.exe(300, "/usr/bin/python3.12");
        fs.comm(300, "python3\n");
        let tables = tables_on(&fs);
        assert_eq!(
            tables.describe(100),
            Some(("curl".to_string(), Some("/usr/bin/curl".to_string())))
        );
        assert_eq!(
            tables.describe(200),
            Some((
                "server".to_string(),
                Some("/opt/my app/bin/server".to_string())
            ))
        );
        assert_eq!(
            tables.describe(300),
            Some((
                "python3.12".to_string(),
                Some("/usr/bin/python3.12".to_string())
            ))
        );
    }

    #[test]
    fn only_the_end_of_a_path_loses_the_mark_of_a_deleted_file() {
        let fs = MemFs::new();
        fs.exe(100, "/tmp/a (deleted)/b (deleted)");
        let tables = tables_on(&fs);
        assert_eq!(
            tables.describe(100),
            Some(("b".to_string(), Some("/tmp/a (deleted)/b".to_string())))
        );
    }

    #[test]
    fn a_program_without_a_readable_file_is_known_by_its_comm() {
        let fs = MemFs::new();
        // a name of the kernel's can have a slash in it, and is no path
        fs.comm(100, "kworker/u8:3\n");
        // the kernel keeps 15 bytes of it
        fs.comm(200, "0123456789abcde\n");
        // another user's process: this user may not read where its link points
        fs.comm(300, "sshd\n");
        fs.exe(300, "/usr/sbin/sshd");
        fs.close("/proc/300/exe");
        let tables = tables_on(&fs);
        assert_eq!(
            tables.describe(100),
            Some(("kworker/u8:3".to_string(), None))
        );
        assert_eq!(
            tables.describe(200),
            Some(("0123456789abcde".to_string(), None))
        );
        assert_eq!(tables.describe(300), Some(("sshd".to_string(), None)));
    }

    #[test]
    fn a_process_nothing_can_be_read_of_is_not_described() {
        let fs = MemFs::new();
        fs.exe(200, "/usr/sbin/sshd");
        fs.close("/proc/200/exe");
        fs.comm(200, "sshd\n");
        fs.close("/proc/200/comm");
        let tables = tables_on(&fs);
        assert_eq!(tables.describe(100), None);
        assert_eq!(tables.describe(200), None);
    }

    // ---- the finder on top ----

    #[test]
    fn the_finder_finds_the_process_of_a_clients_socket() {
        let fs = MemFs::new();
        fs.file("/proc/net/tcp", native(TCP_LITTLE, TCP_BIG));
        fs.file("/proc/net/udp", native(UDP_LITTLE, UDP_BIG));
        // the client, and rurge with its listening socket and its end
        fs.process(100, &[22222, 13131]);
        fs.exe(100, "/usr/bin/curl");
        fs.process(200, &[11111, 33333]);
        fs.exe(200, "/usr/local/bin/rurge");
        let finder = Finder::with_source(Box::new(Tables::new(fs.clone())), ADDRESS_TTL);

        let tcp = Query {
            protocol: Protocol::Tcp,
            local: addr("127.0.0.1:50000"),
            remote: Some(addr("127.0.0.1:7890")),
            since: Instant::now(),
        };
        let curl = Owner {
            pid: 100,
            name: "curl".to_string(),
            path: Some("/usr/bin/curl".to_string()),
        };
        assert_eq!(finder.owner(&tcp), Some(curl.clone()));

        let udp = Query {
            protocol: Protocol::Udp,
            local: addr("127.0.0.1:58528"),
            remote: None,
            since: Instant::now(),
        };
        assert_eq!(finder.owner(&udp), Some(curl));
    }

    #[test]
    fn the_finder_finds_a_client_of_both_families_by_its_ipv4_address() {
        let fs = MemFs::new();
        fs.file("/proc/net/tcp", HEADER);
        fs.file("/proc/net/tcp6", native(TCP6_LITTLE, TCP6_BIG));
        fs.process(300, &[88888]);
        fs.exe(300, "/usr/bin/python3.12");
        let finder = Finder::with_source(Box::new(Tables::new(fs.clone())), ADDRESS_TTL);

        // the client's end is in the IPv6 table, as `::ffff:127.0.0.1`
        let query = Query {
            protocol: Protocol::Tcp,
            local: addr("127.0.0.1:50002"),
            remote: Some(addr("127.0.0.1:7890")),
            since: Instant::now(),
        };
        let owner = finder.owner(&query).expect("the client is found");
        assert_eq!(owner.pid, 300);
        assert_eq!(owner.name, "python3.12");

        // and a client on the IPv6 loopback address in its own right
        fs.process(400, &[66666]);
        fs.exe(400, "/usr/bin/nc");
        let query = Query {
            protocol: Protocol::Tcp,
            local: addr("[::1]:50001"),
            remote: Some(addr("[::1]:7890")),
            since: Instant::now(),
        };
        let owner = finder.owner(&query).expect("the client is found");
        assert_eq!(owner.pid, 400);
        assert_eq!(owner.name, "nc");
    }

    // ---- the real file system ----

    #[test]
    fn the_real_file_system_reads_and_lists() {
        let directory = tempfile::tempdir().unwrap();
        let path = |name: &str| directory.path().join(name).to_str().unwrap().to_string();
        std::fs::write(path("comm"), "curl\n").unwrap();
        std::fs::create_dir(path("fd")).unwrap();
        std::fs::write(directory.path().join("fd").join("3"), "").unwrap();
        std::fs::write(directory.path().join("fd").join("4"), "").unwrap();

        assert_eq!(RealFs.read(&path("comm")).unwrap(), "curl\n");
        assert_eq!(
            RealFs.read(&path("missing")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let mut names = RealFs.list(&path("fd")).unwrap();
        names.sort();
        assert_eq!(names, ["3", "4"]);
        assert_eq!(
            RealFs.list(&path("missing")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        // a file is not a link
        assert!(RealFs.link(&path("comm")).is_err());
    }

    #[cfg(target_os = "linux")]
    mod system {
        use super::*;
        use std::net::{TcpListener, TcpStream, UdpSocket};
        use std::time::Duration;

        /// The longest a test waits for the system.
        const LIMIT: Duration = Duration::from_secs(10);

        fn assert_this_process(owner: Option<Owner>, what: &str) {
            let owner = owner.unwrap_or_else(|| panic!("{what}: no owner found"));
            assert_eq!(owner.pid, std::process::id(), "{what}");
            // the kernel says where the program is as a path with no link in it
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
                remote: Some(server),
                since: Instant::now(),
            });
            assert_this_process(
                tcp,
                &format!("the client's end of a TCP connection on {ip}"),
            );
            let udp = finder.owner(&Query {
                protocol: Protocol::Udp,
                local: datagrams.local_addr().unwrap(),
                remote: None,
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
    }
}
