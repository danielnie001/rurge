//! `[WireGuard <name>]` sections (manual: Policies › WireGuard): the key,
//! the tunnel addresses and the peers of the tunnel a `wireguard` policy's
//! `section-name` names. Every section is checked at load, whether a policy
//! uses it or not.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::span::Span;
use crate::spec::Secret;
use crate::text::Section;
use crate::types::HostName;
use crate::value::{parse_bool, parse_key_value, split_definition, split_list};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use ipnet::IpNet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;

/// `mtu` when the section has none.
pub const DEFAULT_MTU: u16 = 1280;
/// The `mtu` values the manual accepts.
pub const MTU_RANGE: RangeInclusive<u16> = 576..=1420;
/// The port of a `dns-server` written without one.
const DNS_PORT: u16 = 53;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WireGuardSection {
    /// What follows `WireGuard ` in the header: the name `section-name` gives.
    pub name: String,
    pub private_key: Secret<[u8; 32]>,
    pub self_ip: Option<Ipv4Addr>,
    pub self_ip_v6: Option<Ipv6Addr>,
    /// `dns-server`, in the order written; empty when there is none.
    pub dns_servers: Vec<TunnelDns>,
    pub prefer_ipv6: bool,
    pub mtu: u16,
    pub peers: Vec<WireGuardPeer>,
}

/// One entry of `dns-server`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TunnelDns {
    /// A resolver reached through the tunnel.
    Server(SocketAddr),
    /// `system`: rurge's own resolver, on this machine.
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WireGuardPeer {
    pub public_key: [u8; 32],
    /// The destinations routed to this peer, and the addresses it may send from.
    pub allowed_ips: Vec<IpNet>,
    pub endpoint: PeerEndpoint,
    pub preshared_key: Option<Secret<[u8; 32]>>,
    /// Persistent keepalive, in seconds; `None` when off (`0`).
    pub keepalive: Option<u16>,
    /// `client-id`: bytes 1–3 of every WireGuard message (WARP).
    pub client_id: Option<[u8; 3]>,
}

/// A peer's UDP address as written: a host name is resolved when the tunnel
/// starts.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PeerEndpoint {
    pub host: HostName,
    pub port: u16,
}

impl fmt::Display for PeerEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            HostName::Ip(IpAddr::V6(v6)) => write!(f, "[{v6}]:{}", self.port),
            host => write!(f, "{host}:{}", self.port),
        }
    }
}

/// Where problems of one section go: `E0023` for what makes it unusable,
/// `W0001` for what is ignored. Values that are secrets are never quoted.
struct Report<'a> {
    name: &'a str,
    diags: &'a mut Diagnostics,
    failed: bool,
}

impl Report<'_> {
    fn error(&mut self, span: &Span, message: impl fmt::Display) {
        self.failed = true;
        self.diags.push(
            Diagnostic::error(
                codes::E_WIREGUARD_SECTION,
                format!("[WireGuard {}]: {message}", self.name),
            )
            .at(span.clone()),
        );
    }

    fn warn(&mut self, span: &Span, message: impl fmt::Display) {
        self.diags.push(
            Diagnostic::warning(
                codes::W_UNKNOWN_KEY,
                format!("[WireGuard {}]: {message}", self.name),
            )
            .at(span.clone()),
        );
    }
}

/// A 32-byte key, the way WireGuard writes it (Base64) or as 64 hex digits.
fn key32(value: &str) -> Option<[u8; 32]> {
    let value = value.trim();
    let bytes = if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) {
        (0..32)
            .map(|i| u8::from_str_radix(&value[2 * i..2 * i + 2], 16).ok())
            .collect::<Option<Vec<u8>>>()?
    } else {
        STANDARD
            .decode(value)
            .or_else(|_| STANDARD_NO_PAD.decode(value))
            .ok()?
    };
    bytes.try_into().ok()
}

fn tunnel_dns(entry: &str) -> Result<TunnelDns, String> {
    let entry = entry.trim();
    if entry.eq_ignore_ascii_case("system") {
        return Ok(TunnelDns::System);
    }
    if entry.contains("://") {
        return Err(format!(
            "`dns-server` `{entry}`: an encrypted-DNS URL is not accepted here"
        ));
    }
    let addr = match entry.parse::<SocketAddr>() {
        Ok(addr) => addr,
        Err(_) => match entry.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, DNS_PORT),
            Err(_) => {
                return Err(format!(
                    "invalid `dns-server` `{entry}` (expected an IP address, an address with a port, or `system`)"
                ));
            }
        },
    };
    if addr.ip().is_multicast() {
        return Err(format!(
            "`dns-server` `{entry}`: a multicast address is not accepted"
        ));
    }
    if addr.ip().is_unspecified() {
        return Err(format!(
            "`dns-server` `{entry}`: an unspecified address is not accepted"
        ));
    }
    if addr.port() == 0 {
        return Err(format!("`dns-server` `{entry}`: port 0 is not accepted"));
    }
    Ok(TunnelDns::Server(addr))
}

fn endpoint(value: &str) -> Option<PeerEndpoint> {
    let value = value.trim();
    if let Ok(addr) = value.parse::<SocketAddr>() {
        return (addr.port() != 0).then(|| PeerEndpoint {
            host: HostName::Ip(addr.ip()),
            port: addr.port(),
        });
    }
    let (host, port) = value.rsplit_once(':')?;
    // an IPv6 address goes in brackets; anything else with a colon is no host
    if host.is_empty() || host.contains([':', '[', ']']) || host.contains(char::is_whitespace) {
        return None;
    }
    let port = port.parse::<u16>().ok().filter(|p| *p != 0)?;
    Some(PeerEndpoint {
        host: HostName::parse(host),
        port,
    })
}

/// `83/12/235`, three bytes in hex (`530ceb`) or four Base64 characters
/// (`Uwzr`).
fn client_id(value: &str) -> Option<[u8; 3]> {
    let value = value.trim();
    let parts: Vec<&str> = value.split('/').collect();
    if parts.len() == 3 {
        let decimals: Option<Vec<u8>> = parts.iter().map(|p| p.parse::<u8>().ok()).collect();
        if let Some(bytes) = decimals {
            return bytes.try_into().ok();
        }
    }
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        let bytes: Option<Vec<u8>> = (0..3)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
            .collect();
        return bytes?.try_into().ok();
    }
    if value.len() == 4 {
        return STANDARD.decode(value).ok()?.try_into().ok();
    }
    None
}

fn allowed_ips(value: &str) -> Result<Vec<IpNet>, String> {
    let mut out = Vec::new();
    for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let net = match entry.parse::<IpNet>() {
            Ok(net) => net.trunc(),
            // a bare address is a route to that address alone
            Err(_) => match entry.parse::<IpAddr>() {
                Ok(ip) => IpNet::from(ip),
                Err(_) => return Err(format!("invalid `allowed-ips` entry `{entry}`")),
            },
        };
        out.push(net);
    }
    if out.is_empty() {
        return Err("`allowed-ips` is empty".to_string());
    }
    Ok(out)
}

/// One `( … )` of a `peer` line; `n` counts the section's peers from 1.
fn peer(report: &mut Report<'_>, span: &Span, item: &str, n: usize) -> Option<WireGuardPeer> {
    let Some(inner) = item
        .trim()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
    else {
        report.error(
            span,
            format!("peer {n}: expected `(public-key = …, allowed-ips = …, endpoint = …)`"),
        );
        return None;
    };
    let failed_before = report.failed;
    let (mut public_key, mut ips, mut end, mut psk) = (None, Vec::new(), None, None);
    let (mut keepalive, mut id) = (None, None);
    // what was written, valid or not: only a field left out is "required"
    let mut given: Vec<String> = Vec::new();
    for field in split_list(inner) {
        let Some((key, value)) = parse_key_value(&field) else {
            // never quoted: it may be a key that lost its name
            report.warn(span, format!("peer {n}: a field without `=` ignored"));
            continue;
        };
        given.push(key.to_ascii_lowercase());
        match key.to_ascii_lowercase().as_str() {
            "public-key" => match key32(value) {
                Some(k) => public_key = Some(k),
                None => report.error(
                    span,
                    format!("peer {n}: `public-key` is not a 32-byte key in Base64 or hex"),
                ),
            },
            "allowed-ips" => match allowed_ips(value) {
                Ok(list) => ips = list,
                Err(why) => report.error(span, format!("peer {n}: {why}")),
            },
            "endpoint" => match endpoint(value) {
                Some(e) => end = Some(e),
                None => report.error(
                    span,
                    format!("peer {n}: invalid `endpoint` `{value}` (expected host:port)"),
                ),
            },
            "preshared-key" => match key32(value) {
                Some(k) => psk = Some(Secret::new(k)),
                None => report.error(
                    span,
                    format!("peer {n}: `preshared-key` is not a 32-byte key in Base64 or hex"),
                ),
            },
            "keepalive" => match value.trim().parse::<u16>() {
                Ok(secs) => keepalive = (secs > 0).then_some(secs),
                Err(_) => report.error(
                    span,
                    format!("peer {n}: invalid `keepalive` `{value}` (expected 0-65535 seconds)"),
                ),
            },
            "client-id" => match client_id(value) {
                Some(bytes) => id = Some(bytes),
                None => report.error(
                    span,
                    format!(
                        "peer {n}: invalid `client-id` `{value}` (expected `a/b/c`, three bytes in hex or four Base64 characters)"
                    ),
                ),
            },
            other => report.warn(span, format!("peer {n}: unknown field `{other}` ignored")),
        }
    }
    for field in ["public-key", "allowed-ips", "endpoint"] {
        if !given.iter().any(|g| g == field) {
            report.error(span, format!("peer {n}: `{field}` is required"));
        }
    }
    if report.failed != failed_before {
        return None;
    }
    Some(WireGuardPeer {
        public_key: public_key?,
        allowed_ips: ips,
        endpoint: end?,
        preshared_key: psk,
        keepalive,
        client_id: id,
    })
}

/// The section, or `None` after its errors went to `diags`.
pub fn parse_section(section: &Section, diags: &mut Diagnostics) -> Option<WireGuardSection> {
    let name = section.name["WireGuard ".len()..].trim();
    let mut report = Report {
        name,
        diags,
        failed: false,
    };
    if name.is_empty() {
        report.error(&section.span, "the section needs a name");
        return None;
    }
    let mut private_key = None;
    let (mut self_ip, mut self_ip_v6) = (None, None);
    let mut dns_servers = Vec::new();
    let mut prefer_ipv6 = false;
    let mut mtu = DEFAULT_MTU;
    let mut peers = Vec::new();
    // peers are numbered as written, broken ones included
    let mut written = 0;
    // what was written, valid or not: only a key left out is "required"
    let mut given: Vec<String> = Vec::new();
    for entry in section.active_entries() {
        let span = &entry.span;
        let Some((key, value)) = split_definition(&entry.raw) else {
            report.error(span, "expected `key = value`");
            continue;
        };
        given.push(key.to_ascii_lowercase());
        match key.to_ascii_lowercase().as_str() {
            "private-key" => match key32(value) {
                Some(k) => private_key = Some(Secret::new(k)),
                None => report.error(span, "`private-key` is not a 32-byte key in Base64 or hex"),
            },
            "self-ip" => match value.parse::<Ipv4Addr>() {
                Ok(ip) if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() => report
                    .error(
                        span,
                        format!("invalid `self-ip` `{value}` (expected a unicast IPv4 address)"),
                    ),
                Ok(ip) => self_ip = Some(ip),
                Err(_) => report.error(
                    span,
                    format!("invalid `self-ip` `{value}` (expected an IPv4 address, not a prefix)"),
                ),
            },
            "self-ip-v6" => match value.parse::<Ipv6Addr>() {
                Ok(ip) if ip.is_unspecified() || ip.is_multicast() => report.error(
                    span,
                    format!("invalid `self-ip-v6` `{value}` (expected a unicast IPv6 address)"),
                ),
                Ok(ip) => self_ip_v6 = Some(ip),
                Err(_) => report.error(
                    span,
                    format!(
                        "invalid `self-ip-v6` `{value}` (expected an IPv6 address, not a prefix)"
                    ),
                ),
            },
            "dns-server" => {
                for item in split_list(value) {
                    match tunnel_dns(&item) {
                        Ok(dns) => dns_servers.push(dns),
                        Err(why) => report.error(span, why),
                    }
                }
            }
            "prefer-ipv6" => match parse_bool(value) {
                Some(b) => prefer_ipv6 = b,
                None => report.error(
                    span,
                    format!("invalid `prefer-ipv6` `{value}` (expected true or false)"),
                ),
            },
            "mtu" => match value.parse::<u16>() {
                Ok(n) if MTU_RANGE.contains(&n) => mtu = n,
                _ => report.error(
                    span,
                    format!(
                        "invalid `mtu` `{value}` (expected {}-{})",
                        MTU_RANGE.start(),
                        MTU_RANGE.end()
                    ),
                ),
            },
            "peer" => {
                for item in split_list(value) {
                    written += 1;
                    peers.extend(peer(&mut report, span, &item, written));
                }
            }
            other => report.warn(span, format!("unknown key `{other}` ignored")),
        }
    }
    let given = |key: &str| given.iter().any(|g| g == key);
    if !given("private-key") {
        report.error(&section.span, "`private-key` is required");
    }
    if !given("self-ip") && !given("self-ip-v6") {
        report.error(
            &section.span,
            "at least one of `self-ip` and `self-ip-v6` is required",
        );
    }
    if written == 0 {
        report.error(&section.span, "at least one `peer` is required");
    }
    if report.failed {
        return None;
    }
    Some(WireGuardSection {
        name: name.to_string(),
        private_key: private_key?,
        self_ip,
        self_ip_v6,
        dns_servers,
        prefer_ipv6,
        mtu,
        peers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    /// Keys from the WireGuard documentation; they guard nothing.
    const PRIVATE: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    const PUBLIC: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";
    const PRIVATE_HEX: &str = "c809f3e5317e9575c9b5ed78b638b7ce530dabe85ddab614220241801ddf0669";

    fn parse(text: &str) -> (Option<WireGuardSection>, Vec<Diagnostic>) {
        let (profile, d) = parse_str(text, Arc::from(Path::new("w.conf")), Origin::Main);
        assert!(d.is_empty(), "{:?}", d.into_vec());
        let mut diags = Diagnostics::default();
        let section = parse_section(&profile.sections[0], &mut diags);
        (section, diags.into_vec())
    }

    fn ok(text: &str) -> WireGuardSection {
        let (section, diags) = parse(text);
        assert!(diags.is_empty(), "{diags:?}");
        section.expect("a section")
    }

    fn errors(text: &str) -> Vec<String> {
        let (section, diags) = parse(text);
        assert!(section.is_none(), "{text}");
        diags
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .inspect(|d| assert_eq!(d.code, codes::E_WIREGUARD_SECTION))
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn the_manuals_warp_example() {
        let s = ok(&format!(
            "[WireGuard warp]\nprivate-key = {PRIVATE}\nself-ip = 172.16.0.2\nself-ip-v6 = 2606:4700:110:0000::2\n\
dns-server = 1.1.1.1, 2606:4700:4700::1111\n\
peer = (public-key = {PUBLIC}, allowed-ips = \"0.0.0.0/0, ::/0\", endpoint = engage.cloudflareclient.com:2408, client-id = 83/12/235)\n"
        ));
        assert_eq!(s.name, "warp");
        assert_eq!(s.private_key.expose(), &key32(PRIVATE_HEX).unwrap());
        assert_eq!(s.self_ip, Some(Ipv4Addr::new(172, 16, 0, 2)));
        assert_eq!(s.self_ip_v6, Some("2606:4700:110::2".parse().unwrap()));
        assert_eq!(
            s.dns_servers,
            [
                TunnelDns::Server("1.1.1.1:53".parse().unwrap()),
                TunnelDns::Server("[2606:4700:4700::1111]:53".parse().unwrap()),
            ]
        );
        assert!(!s.prefer_ipv6);
        assert_eq!(s.mtu, DEFAULT_MTU);
        let p = &s.peers[0];
        assert_eq!(p.public_key, key32(PUBLIC).unwrap());
        assert_eq!(
            p.allowed_ips,
            [
                "0.0.0.0/0".parse::<IpNet>().unwrap(),
                "::/0".parse().unwrap()
            ]
        );
        assert_eq!(p.endpoint.to_string(), "engage.cloudflareclient.com:2408");
        assert_eq!(p.client_id, Some([83, 12, 235]));
        assert_eq!((p.keepalive, &p.preshared_key), (None, &None));
    }

    #[test]
    fn keys_are_base64_or_hex() {
        let hex = ok(&format!(
            "[WireGuard h]\nprivate-key = {PRIVATE_HEX}\nself-ip = 10.0.0.2\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/24, endpoint = 192.0.2.1:51820, preshared-key = {PRIVATE_HEX})\n"
        ));
        let b64 = ok(&format!(
            "[WireGuard h]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/24, endpoint = 192.0.2.1:51820, preshared-key = {PRIVATE})\n"
        ));
        assert_eq!(hex, b64);
        assert_eq!(key32(&PRIVATE[..43]), key32(PRIVATE), "padding is optional");
        for bad in ["", "AAAA", &PRIVATE_HEX[..62], &format!("{PRIVATE_HEX}00")] {
            assert_eq!(key32(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_client_id_is_written_three_ways() {
        for form in ["83/12/235", "530ceb", "0x530CEB", "Uwzr"] {
            assert_eq!(client_id(form), Some([83, 12, 235]), "{form}");
        }
        for bad in ["83/12", "83/12/256", "530ce", "Uwz", "Uwzr=", "1/2/3/4"] {
            assert_eq!(client_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn dns_server_entries() {
        let s = ok(&format!(
            "[WireGuard d]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
dns-server = 10.20.0.1, fd00:20::1, 10.20.0.2:5353, [fd00:20::2]:5353, system\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.20.0.0/16, endpoint = 192.0.2.1:51820)\n"
        ));
        assert_eq!(
            s.dns_servers,
            [
                TunnelDns::Server("10.20.0.1:53".parse().unwrap()),
                TunnelDns::Server("[fd00:20::1]:53".parse().unwrap()),
                TunnelDns::Server("10.20.0.2:5353".parse().unwrap()),
                TunnelDns::Server("[fd00:20::2]:5353".parse().unwrap()),
                TunnelDns::System,
            ]
        );
        let found = errors(&format!(
            "[WireGuard d]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
dns-server = 224.0.0.251, https://dns.test/dns-query, nope\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.20.0.0/16, endpoint = 192.0.2.1:51820)\n"
        ));
        assert_eq!(
            found,
            [
                "[WireGuard d]: `dns-server` `224.0.0.251`: a multicast address is not accepted",
                "[WireGuard d]: `dns-server` `https://dns.test/dns-query`: an encrypted-DNS URL is not accepted here",
                "[WireGuard d]: invalid `dns-server` `nope` (expected an IP address, an address with a port, or `system`)",
            ]
        );
    }

    #[test]
    fn dns_servers_are_addresses_a_question_can_go_to() {
        let found = errors(&format!(
            "[WireGuard d]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
dns-server = ff02::fb, [ff02::fb]:53, 0.0.0.0, ::, 10.20.0.1:0\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.20.0.0/16, endpoint = 192.0.2.1:51820)\n"
        ));
        assert_eq!(
            found,
            [
                "[WireGuard d]: `dns-server` `ff02::fb`: a multicast address is not accepted",
                "[WireGuard d]: `dns-server` `[ff02::fb]:53`: a multicast address is not accepted",
                "[WireGuard d]: `dns-server` `0.0.0.0`: an unspecified address is not accepted",
                "[WireGuard d]: `dns-server` `::`: an unspecified address is not accepted",
                "[WireGuard d]: `dns-server` `10.20.0.1:0`: port 0 is not accepted",
            ]
        );
    }

    #[test]
    fn endpoints_name_a_host_and_a_udp_port() {
        for (text, shown) in [
            ("vpn.example.com:51820", "vpn.example.com:51820"),
            ("192.0.2.1:51820", "192.0.2.1:51820"),
            ("[2001:db8::10]:51820", "[2001:db8::10]:51820"),
            ("VPN.Example.COM:1", "vpn.example.com:1"),
        ] {
            assert_eq!(endpoint(text).unwrap().to_string(), shown, "{text}");
        }
        for bad in [
            "vpn.example.com",
            "vpn.example.com:0",
            "vpn.example.com:65536",
            "2001:db8::10:51820",
            ":51820",
            "a b:1",
        ] {
            assert_eq!(endpoint(bad), None, "{bad}");
        }
    }

    #[test]
    fn peers_accumulate_over_lines_and_within_one() {
        let s = ok(&format!(
            "[WireGuard m]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\nmtu = 1420\nprefer-ipv6 = true\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.10.0.0/16, endpoint = a.test:51820), (public-key = {PRIVATE}, allowed-ips = \"10.20.0.0/16, 10.30.0.1\", endpoint = b.test:51820, keepalive = 25)\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.40.1.9/16, endpoint = c.test:51820, keepalive = 0)\n"
        ));
        assert_eq!((s.mtu, s.prefer_ipv6), (1420, true));
        let hosts: Vec<String> = s.peers.iter().map(|p| p.endpoint.to_string()).collect();
        assert_eq!(hosts, ["a.test:51820", "b.test:51820", "c.test:51820"]);
        assert_eq!(
            s.peers[1].allowed_ips,
            [
                "10.20.0.0/16".parse::<IpNet>().unwrap(),
                "10.30.0.1/32".parse().unwrap()
            ]
        );
        assert_eq!(s.peers[1].keepalive, Some(25));
        assert_eq!(s.peers[2].keepalive, None, "0 is off");
        // a prefix written with host bits set is the prefix itself
        assert_eq!(
            s.peers[2].allowed_ips,
            ["10.40.0.0/16".parse::<IpNet>().unwrap()]
        );
    }

    #[test]
    fn what_a_section_must_have() {
        assert_eq!(
            errors("[WireGuard e]\nmtu = 1280\n"),
            [
                "[WireGuard e]: `private-key` is required",
                "[WireGuard e]: at least one of `self-ip` and `self-ip-v6` is required",
                "[WireGuard e]: at least one `peer` is required",
            ]
        );
        assert_eq!(
            errors(&format!(
                "[WireGuard e]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\npeer = (keepalive = 5), (public-key = {PUBLIC})\n"
            )),
            [
                "[WireGuard e]: peer 1: `public-key` is required",
                "[WireGuard e]: peer 1: `allowed-ips` is required",
                "[WireGuard e]: peer 1: `endpoint` is required",
                "[WireGuard e]: peer 2: `allowed-ips` is required",
                "[WireGuard e]: peer 2: `endpoint` is required",
            ]
        );
    }

    #[test]
    fn values_out_of_range_are_errors() {
        let found = errors(&format!(
            "[WireGuard v]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2/32\nself-ip-v6 = 10.0.0.2\nmtu = 1500\nprefer-ipv6 = maybe\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/33, endpoint = a.test:51820, keepalive = 70000, client-id = 1/2)\n\
peer = public-key = {PUBLIC}\n"
        ));
        assert_eq!(
            found,
            [
                "[WireGuard v]: invalid `self-ip` `10.0.0.2/32` (expected an IPv4 address, not a prefix)",
                "[WireGuard v]: invalid `self-ip-v6` `10.0.0.2` (expected an IPv6 address, not a prefix)",
                "[WireGuard v]: invalid `mtu` `1500` (expected 576-1420)",
                "[WireGuard v]: invalid `prefer-ipv6` `maybe` (expected true or false)",
                "[WireGuard v]: peer 1: invalid `allowed-ips` entry `10.0.0.0/33`",
                "[WireGuard v]: peer 1: invalid `keepalive` `70000` (expected 0-65535 seconds)",
                "[WireGuard v]: peer 1: invalid `client-id` `1/2` (expected `a/b/c`, three bytes in hex or four Base64 characters)",
                "[WireGuard v]: peer 2: expected `(public-key = …, allowed-ips = …, endpoint = …)`",
            ]
        );
    }

    #[test]
    fn self_ips_are_unicast_addresses() {
        for (line, message) in [
            (
                "self-ip = 224.0.0.1",
                "invalid `self-ip` `224.0.0.1` (expected a unicast IPv4 address)",
            ),
            (
                "self-ip = 255.255.255.255",
                "invalid `self-ip` `255.255.255.255` (expected a unicast IPv4 address)",
            ),
            (
                "self-ip = 0.0.0.0",
                "invalid `self-ip` `0.0.0.0` (expected a unicast IPv4 address)",
            ),
            (
                "self-ip-v6 = ff02::1",
                "invalid `self-ip-v6` `ff02::1` (expected a unicast IPv6 address)",
            ),
            (
                "self-ip-v6 = ::",
                "invalid `self-ip-v6` `::` (expected a unicast IPv6 address)",
            ),
        ] {
            assert_eq!(
                errors(&format!(
                    "[WireGuard s]\nprivate-key = {PRIVATE}\n{line}\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/24, endpoint = 192.0.2.1:51820)\n"
                )),
                [format!("[WireGuard s]: {message}")],
                "{line}"
            );
        }
    }

    /// A key that does not decode is named, never quoted; nor does a
    /// section's `Debug` show one.
    #[test]
    fn keys_are_never_quoted() {
        let (section, diags) = parse(
            "[WireGuard k]\nprivate-key = s3cretPrivate\nself-ip = 10.0.0.2\n\
peer = (public-key = n0tAKey, allowed-ips = 10.0.0.0/8, endpoint = a.test:1, preshared-key = s3cretShared, s3cretLoose)\n",
        );
        assert!(section.is_none());
        let shown: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
        assert_eq!(shown.len(), 4, "{shown:?}");
        for message in &shown {
            for secret in ["s3cretPrivate", "n0tAKey", "s3cretShared", "s3cretLoose"] {
                assert!(!message.contains(secret), "{message}");
            }
        }
        assert!(
            shown[3].ends_with("peer 1: a field without `=` ignored"),
            "{shown:?}"
        );
        let s = ok(&format!(
            "[WireGuard k]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/8, endpoint = a.test:1, preshared-key = {PRIVATE})\n"
        ));
        let debug = format!("{s:?}");
        assert!(debug.contains("Secret(***)"), "{debug}");
        let private = format!("{:?}", s.private_key.expose());
        assert!(!debug.contains(&private[1..20]), "{debug}");
    }

    #[test]
    fn unknown_keys_and_fields_are_warnings() {
        let (section, diags) = parse(&format!(
            "[WireGuard u]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\nlisten-port = 51820\n\
peer = (public-key = {PUBLIC}, allowed-ips = 10.0.0.0/8, endpoint = a.test:1, persistent = 1)\n"
        ));
        assert!(section.is_some());
        let found: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_UNKNOWN_KEY,
                    "[WireGuard u]: unknown key `listen-port` ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "[WireGuard u]: peer 1: unknown field `persistent` ignored"
                ),
            ]
        );
        assert_eq!(diags[0].span.as_ref().map(|s| s.line), Some(4));
    }
}
