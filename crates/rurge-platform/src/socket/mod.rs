//! Socket options whose spelling differs per platform (AR-02). Plain
//! functions: the binary adapts them to `rurge_net::socket::SocketHook`. The
//! `egress` module is the exception: it decides where rurge's own sockets go
//! while the enhanced mode is on, and it holds no platform branch of its own
//! (M4 design 7.1).

use socket2::Socket;
use std::io;
use std::net::IpAddr;
#[cfg(windows)]
use std::os::windows::io::AsRawSocket;
#[cfg(any(target_os = "macos", windows, test))]
use std::sync::{Arc, Mutex};
#[cfg(any(target_os = "macos", windows, test))]
use std::time::{Duration, Instant};

pub mod egress;

/// A value worth keeping for a short while: the interface table costs a
/// system call in the millisecond range, and `bind_interface` runs once per
/// connection attempt.
#[cfg(any(target_os = "macos", windows, test))]
struct Cached<T> {
    ttl: Duration,
    slot: Mutex<Option<(Instant, Arc<T>)>>,
}

#[cfg(any(target_os = "macos", windows, test))]
impl<T> Cached<T> {
    const fn new(ttl: Duration) -> Cached<T> {
        Cached {
            ttl,
            slot: Mutex::new(None),
        }
    }

    /// The cached value while it is fresh, else whatever `load` returns — a
    /// failure is returned and not remembered.
    fn get(&self, load: impl FnOnce() -> io::Result<T>) -> io::Result<Arc<T>> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, value)) = slot.as_ref()
            && at.elapsed() < self.ttl
        {
            return Ok(value.clone());
        }
        let value = Arc::new(load()?);
        *slot = Some((Instant::now(), value.clone()));
        Ok(value)
    }
}

/// An interface that appears or changes its address is seen within this long.
#[cfg(any(target_os = "macos", windows))]
const INTERFACE_TABLE_TTL: Duration = Duration::from_secs(5);

#[cfg(any(target_os = "macos", windows))]
fn interfaces() -> io::Result<Arc<Vec<if_addrs::Interface>>> {
    static TABLE: Cached<Vec<if_addrs::Interface>> = Cached::new(INTERFACE_TABLE_TTL);
    TABLE.get(if_addrs::get_if_addrs)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

fn routable(ip: &IpAddr, family: Family) -> bool {
    match (ip, family) {
        (IpAddr::V4(v4), Family::V4) => {
            !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified()
        }
        (IpAddr::V6(v6), Family::V6) => {
            // fe80::/10 is link-local
            !v6.is_loopback() && !v6.is_unspecified() && (v6.segments()[0] & 0xffc0) != 0xfe80
        }
        _ => false,
    }
}

/// The address to bind as the source so that traffic leaves through
/// `interface`: its first address of `family` that is neither loopback nor
/// link-local. This is how Windows binds to an interface without `unsafe`
/// (strong host model: a bound source address pins the outgoing interface).
pub fn pick_source(addrs: &[(String, IpAddr)], interface: &str, family: Family) -> Option<IpAddr> {
    addrs
        .iter()
        .filter(|(name, _)| name == interface)
        .map(|(_, ip)| *ip)
        .find(|ip| routable(ip, family))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn bind_interface(socket: &Socket, interface: &str, _family: Family) -> io::Result<()> {
    socket.bind_device(Some(interface.as_bytes()))
}

#[cfg(target_os = "macos")]
pub fn bind_interface(socket: &Socket, interface: &str, family: Family) -> io::Result<()> {
    // `Interface::index` spares us the unsafe `if_nametoindex`
    let index = interfaces()?
        .iter()
        .find(|i| i.name == interface)
        .and_then(|i| i.index)
        .and_then(std::num::NonZeroU32::new)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no such interface: {interface}"),
            )
        })?;
    match family {
        Family::V4 => socket.bind_device_by_index_v4(Some(index)),
        Family::V6 => socket.bind_device_by_index_v6(Some(index)),
    }
}

#[cfg(windows)]
pub fn bind_interface(socket: &Socket, interface: &str, family: Family) -> io::Result<()> {
    // `Interface::name` is the adapter's friendly name on Windows ("Wi-Fi").
    // One table: the source address and the index of the interface it belongs
    // to have to come from the same one, and the table is cached for seconds.
    let table = interfaces()?;
    let addrs: Vec<(String, IpAddr)> = table.iter().map(|i| (i.name.clone(), i.ip())).collect();
    let source = pick_source(&addrs, interface, family).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("interface {interface} has no usable address of this family"),
        )
    })?;
    socket.bind(&std::net::SocketAddr::new(source, 0).into())?;
    // The index of that same address's interface: `IfIndex` for an IPv4 entry
    // and `Ipv6IfIndex` for an IPv6 one (`if-addrs`), 0 being none.
    let index = table
        .iter()
        .find(|i| i.name == interface && i.ip() == source)
        .and_then(|i| i.index);
    let Some(index) = index else {
        tracing::debug!(
            "{interface} has no interface index here; the source address alone \
             pins it"
        );
        return Ok(());
    };
    // The source address is the first layer (the strong host model); this is
    // the second. Without it the socket still leaves through the interface
    // that owns the address, so a failure is not the connection's.
    match unicast_if(socket.as_raw_socket(), family, index) {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::debug!("{interface}: the unicast interface could not be set: {e}");
            Ok(())
        }
    }
}

/// Sends this socket's traffic out of interface `index` (M4 design 7.4): one
/// `setsockopt`, `IP_UNICAST_IF` with the index in network byte order for
/// IPv4, `IPV6_UNICAST_IF` in host byte order for IPv6 (the Winsock
/// documentation of the two options).
#[cfg(windows)]
#[allow(unsafe_code)]
fn unicast_if(
    socket: std::os::windows::io::RawSocket,
    family: Family,
    index: u32,
) -> io::Result<()> {
    use windows_sys::Win32::Networking::WinSock::{
        IP_UNICAST_IF, IPPROTO_IP, IPPROTO_IPV6, IPV6_UNICAST_IF, SOCKET_ERROR, WSAGetLastError,
        setsockopt,
    };
    let (level, name, value) = match family {
        Family::V4 => (IPPROTO_IP, IP_UNICAST_IF, index.to_be()),
        Family::V6 => (IPPROTO_IPV6, IPV6_UNICAST_IF, index),
    };
    // SAFETY: `socket` is a live socket of the caller's; both options are
    // documented to take a DWORD, which `value` is, and it outlives the call.
    let set = unsafe {
        setsockopt(
            socket as usize,
            level,
            name,
            (&raw const value).cast(),
            size_of::<u32>() as i32,
        )
    };
    if set != SOCKET_ERROR {
        return Ok(());
    }
    // SAFETY: plain call, documented to report a socket call's own error.
    let error = unsafe { WSAGetLastError() };
    Err(io::Error::from_raw_os_error(error))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
pub fn bind_interface(_socket: &Socket, _interface: &str, _family: Family) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "binding to an interface is not supported on this platform",
    ))
}

pub fn set_tos(socket: &Socket, family: Family, tos: u8) -> io::Result<()> {
    match family {
        Family::V4 => socket.set_tos_v4(u32::from(tos)),
        Family::V6 => set_traffic_class(socket, tos),
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn set_traffic_class(socket: &Socket, tos: u8) -> io::Result<()> {
    socket.set_tclass_v6(u32::from(tos))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn set_traffic_class(_socket: &Socket, _tos: u8) -> io::Result<()> {
    tracing::debug!("the IPv6 traffic class cannot be set on this platform");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<(String, IpAddr)> {
        [
            ("Wi-Fi", "fe80::1"),
            ("Wi-Fi", "169.254.10.1"),
            ("Wi-Fi", "192.168.1.20"),
            ("Wi-Fi", "2001:db8::20"),
            ("Ethernet", "10.0.0.5"),
            ("Loopback", "127.0.0.1"),
        ]
        .into_iter()
        .map(|(name, ip)| (name.to_string(), ip.parse().unwrap()))
        .collect()
    }

    #[test]
    fn the_source_address_is_the_first_routable_one_of_the_family() {
        let t = table();
        assert_eq!(
            pick_source(&t, "Wi-Fi", Family::V4),
            Some("192.168.1.20".parse().unwrap())
        );
        assert_eq!(
            pick_source(&t, "Wi-Fi", Family::V6),
            Some("2001:db8::20".parse().unwrap())
        );
        assert_eq!(
            pick_source(&t, "Ethernet", Family::V4),
            Some("10.0.0.5".parse().unwrap())
        );
        // no address of that family, only a loopback one, or no such interface
        assert_eq!(pick_source(&t, "Ethernet", Family::V6), None);
        assert_eq!(pick_source(&t, "Loopback", Family::V4), None);
        assert_eq!(
            pick_source(&t, "wi-fi", Family::V4),
            None,
            "names are matched exactly"
        );
    }

    #[test]
    fn binding_to_an_interface_that_does_not_exist_fails() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        assert!(bind_interface(&socket, "rurge-no-such-if0", Family::V4).is_err());
    }

    #[test]
    fn the_tos_is_set_on_an_ipv4_socket() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        set_tos(&socket, Family::V4, 0x28).unwrap();
        assert_eq!(socket.tos_v4().unwrap(), 0x28);
    }

    /// The index of the interface that owns `ip`, found the way
    /// `bind_interface` finds it: by address, not by name (the loopback's
    /// name follows the system language on Windows).
    #[cfg(windows)]
    fn index_of(ip: &str) -> u32 {
        let ip: IpAddr = ip.parse().unwrap();
        let table = interfaces().expect("the interface table");
        let entry = table
            .iter()
            .find(|i| i.ip() == ip)
            .unwrap_or_else(|| panic!("no interface has {ip}"));
        entry
            .index
            .unwrap_or_else(|| panic!("the interface that has {ip} has no index"))
    }

    /// A datagram from a socket whose unicast interface is the loopback's own
    /// index still arrives: the option does not break a socket that only
    /// talks to the loopback. Both ends are this test's own; a wait for the
    /// datagram is bounded by the receiver's read timeout.
    #[cfg(windows)]
    fn reaches_the_loopback(addr: &str, ip: &str, family: Family) {
        use std::net::UdpSocket;
        use std::os::windows::io::AsRawSocket;
        // the receiver is this test's own, on the loopback
        let receiver = match UdpSocket::bind(addr) {
            Ok(socket) => socket,
            Err(e) if family == Family::V6 => {
                eprintln!("no ::1 on this machine, so this is not tested: {e}");
                return;
            }
            Err(e) => panic!("could not bind {addr}: {e}"),
        };
        receiver
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let sender = UdpSocket::bind(addr).unwrap();
        unicast_if(sender.as_raw_socket(), family, index_of(ip))
            .expect("the unicast interface is set on a loopback socket");
        sender
            .send_to(b"rurge", receiver.local_addr().unwrap())
            .unwrap();
        let mut buf = [0u8; 16];
        let (len, _) = receiver
            .recv_from(&mut buf)
            .expect("the datagram comes back");
        assert_eq!(&buf[..len], b"rurge");
    }

    #[cfg(windows)]
    #[test]
    fn unicast_if_on_a_loopback_socket_still_reaches_the_loopback() {
        reaches_the_loopback("127.0.0.1:0", "127.0.0.1", Family::V4);
    }

    #[cfg(windows)]
    #[test]
    fn unicast_if_on_a_loopback_socket_still_reaches_the_loopback_over_ipv6() {
        reaches_the_loopback("[::1]:0", "::1", Family::V6);
    }

    #[test]
    fn the_interface_table_is_cached_for_a_while() {
        use std::cell::Cell;
        let loads = Cell::new(0u32);
        let load = || -> io::Result<u32> {
            loads.set(loads.get() + 1);
            Ok(loads.get())
        };
        let cache = Cached::new(std::time::Duration::from_secs(60));
        assert_eq!(*cache.get(load).unwrap(), 1);
        assert_eq!(*cache.get(load).unwrap(), 1, "served from the cache");
        assert_eq!(loads.get(), 1);

        let never = Cached::new(std::time::Duration::ZERO);
        assert_eq!(*never.get(load).unwrap(), 2);
        assert_eq!(*never.get(load).unwrap(), 3, "a zero TTL always reloads");

        // a failure is reported, not remembered
        let failing: Cached<u32> = Cached::new(std::time::Duration::from_secs(60));
        assert!(failing.get(|| Err(io::Error::other("no table"))).is_err());
        assert_eq!(*failing.get(|| Ok(7)).unwrap(), 7);
    }
}
