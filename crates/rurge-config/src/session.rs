//! Per-session facts the rule engine matches against (M2 design §4.1).
//! Owned by `rurge-config` so every crate shares one definition.

use crate::rule::{HostnameType, ProtocolKind};
use crate::types::HostName;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Which listener accepted the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenerKind {
    Http,
    Socks5,
    Tun,
    Forward,
    /// Started by rurge itself (encrypted DNS, policy tests, script requests).
    Internal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Udp,
}

/// The process that opened a session: one the caller names (`rurge rule match
/// --process`), or one the engine found when a rule asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    /// `None` when the process was given rather than found
    /// (`rurge rule match --process`).
    pub pid: Option<u32>,
    /// The executable's file name: what `PROCESS-NAME` name patterns match.
    pub name: String,
    /// The executable's full path, when it could be read.
    pub path: Option<String>,
}

/// Gateway-mode client device; filled in from phase 7 on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: Option<String>,
    pub mac: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub src: SocketAddr,
    pub in_port: u16,
    /// rurge's end of the client's connection: for the HTTP and SOCKS5
    /// listeners the accepted connection's own local address (canonical; not
    /// the address bound, which may be unspecified), for a SOCKS5 UDP
    /// association its control connection's; for a TCP connection through the
    /// virtual adapter the endpoint the app connected to. `None` for the
    /// adapter's UDP sessions and rurge's own.
    pub local: Option<SocketAddr>,
    pub listener: ListenerKind,
    /// Lowercase domain without trailing dot, or an IP literal.
    pub dst_host: HostName,
    pub dst_port: u16,
    pub transport: Transport,
    /// Sniffed application protocol; `None` when unknown.
    pub protocol: Option<ProtocolKind>,
    pub sni: Option<String>,
    pub http_host: Option<String>,
    pub user_agent: Option<String>,
    /// Full URL; only known for plain HTTP or after MITM. The engine reads
    /// `listener == Http` together with `url.is_some()` as "the HTTP listener
    /// is forwarding a plain request" (absolute-form forwarding to an HTTP
    /// proxy): whoever sets `url` on another kind of session (MITM, phase 4)
    /// must revisit `Engine::dial`.
    pub url: Option<String>,
    /// A process the caller knew before the session was evaluated, which is
    /// what `rurge rule match --process` fills in. A session that came in
    /// through a listener has none: the engine finds its process when a rule
    /// asks for it, and hands it to the rule engine then.
    pub process: Option<ProcessInfo>,
    pub device: Option<DeviceInfo>,
}

impl SessionInfo {
    /// A TCP session from the loopback with every optional field empty.
    pub fn tcp(dst_host: HostName, dst_port: u16) -> SessionInfo {
        SessionInfo {
            src: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            in_port: 0,
            local: None,
            listener: ListenerKind::Http,
            dst_host,
            dst_port,
            transport: Transport::Tcp,
            protocol: None,
            sni: None,
            http_host: None,
            user_agent: None,
            url: None,
            process: None,
            device: None,
        }
    }

    /// A UDP flow from the loopback with every optional field empty.
    pub fn udp(dst_host: HostName, dst_port: u16) -> SessionInfo {
        SessionInfo {
            transport: Transport::Udp,
            ..SessionInfo::tcp(dst_host, dst_port)
        }
    }

    /// The `HOSTNAME-TYPE` classification of the destination.
    pub fn hostname_type(&self) -> HostnameType {
        match &self.dst_host {
            HostName::Ip(IpAddr::V4(_)) => HostnameType::IPv4,
            HostName::Ip(IpAddr::V6(_)) => HostnameType::IPv6,
            HostName::Domain(d) if !d.contains('.') => HostnameType::Simple,
            HostName::Domain(_) => HostnameType::Domain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_type_classifies_every_form() {
        let ty = |h: &str| SessionInfo::tcp(HostName::parse(h), 443).hostname_type();
        assert_eq!(ty("1.2.3.4"), HostnameType::IPv4);
        assert_eq!(ty("[::1]"), HostnameType::IPv6);
        assert_eq!(ty("nas"), HostnameType::Simple);
        assert_eq!(ty("www.example.com."), HostnameType::Domain);
    }

    #[test]
    fn tcp_constructor_defaults_are_empty() {
        let s = SessionInfo::tcp(HostName::parse("example.com"), 80);
        assert_eq!(s.transport, Transport::Tcp);
        assert_eq!(s.listener, ListenerKind::Http);
        assert_eq!(s.dst_port, 80);
        assert!(s.sni.is_none() && s.url.is_none() && s.process.is_none());
        // rurge's end of the connection is told by whoever accepted it
        assert!(s.local.is_none());
        assert!(
            SessionInfo::udp(HostName::parse("example.com"), 53)
                .local
                .is_none()
        );
    }
}
