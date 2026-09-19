//! Socket options whose spelling differs per platform (AR-02). Plain
//! functions: the binary adapts them to `rurge_net::socket::SocketHook`.

use socket2::Socket;
use std::io;
use std::net::IpAddr;
#[cfg(any(target_os = "macos", windows, test))]
use std::sync::{Arc, Mutex};
#[cfg(any(target_os = "macos", windows, test))]
use std::time::{Duration, Instant};

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
    // `Interface::name` is the adapter's friendly name on Windows ("Wi-Fi")
    let addrs: Vec<(String, IpAddr)> = interfaces()?
        .iter()
        .map(|i| (i.name.clone(), i.ip()))
        .collect();
    let source = pick_source(&addrs, interface, family).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("interface {interface} has no usable address of this family"),
        )
    })?;
    socket.bind(&std::net::SocketAddr::new(source, 0).into())
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
