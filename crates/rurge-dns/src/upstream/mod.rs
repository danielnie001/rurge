//! Upstream transports (design §7.2). Every transport exchanges wire-format
//! DNS messages; message encoding lives in `crate::message`.

pub mod doh;
pub mod tcp;
pub mod udp;

use rurge_config::general::{DnsServer, EncryptedDns, EncryptedDnsScheme};
use rurge_config::host::DnsUpstream;
use rurge_net::BoxFuture;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::time::Instant;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpstreamError {
    Timeout,
    Io(String),
    Tls(String),
    Http(String),
    Bootstrap(String),
    BadResponse(String),
}

impl fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpstreamError::Timeout => f.write_str("timeout"),
            UpstreamError::Io(e) => write!(f, "io: {e}"),
            UpstreamError::Tls(e) => write!(f, "tls: {e}"),
            UpstreamError::Http(e) => write!(f, "http: {e}"),
            UpstreamError::Bootstrap(e) => write!(f, "bootstrap: {e}"),
            UpstreamError::BadResponse(e) => write!(f, "bad response: {e}"),
        }
    }
}

/// One DNS server reachable through one transport.
pub trait Upstream: Send + Sync {
    /// Display name, e.g. `udp://1.1.1.1:53`.
    fn name(&self) -> &str;
    /// Sends one wire-format query and returns one wire-format response.
    fn query<'a>(
        &'a self,
        wire: &'a [u8],
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>>;
}

pub type UpstreamRef = Arc<dyn Upstream>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UpstreamSpec {
    Udp(SocketAddr),
    Tcp { host: String, port: u16 },
    Tls { host: String, port: u16 },
    Https(Url),
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

/// `host`, `host:port`, `[v6]`, `[v6]:port`; anything after `/` is ignored.
fn host_port(rest: &str, default_port: u16) -> Result<(String, u16), String> {
    let rest = rest.split('/').next().unwrap_or("").trim();
    if rest.is_empty() {
        return Err("missing host".to_string());
    }
    let (host, port) = if let Some(r) = rest.strip_prefix('[') {
        let (h, tail) = r
            .split_once(']')
            .ok_or_else(|| "unterminated IPv6 literal".to_string())?;
        (h.to_string(), tail.strip_prefix(':'))
    } else if rest.matches(':').count() == 1 {
        let (h, p) = rest.split_once(':').expect("one colon");
        (h.to_string(), Some(p))
    } else {
        (rest.to_string(), None)
    };
    let port = match port {
        Some(p) => p
            .parse::<u16>()
            .map_err(|_| format!("invalid port `{p}`"))?,
        None => default_port,
    };
    Ok((host.trim_end_matches('.').to_ascii_lowercase(), port))
}

impl UpstreamSpec {
    pub fn parse(s: &str) -> Result<UpstreamSpec, String> {
        let s = s.trim();
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("https://") {
            let url = Url::parse(s).map_err(|e| format!("`{s}`: {e}"))?;
            if url.host_str().is_none() {
                return Err(format!("`{s}`: missing host"));
            }
            return Ok(UpstreamSpec::Https(url));
        }
        if lower.starts_with("h3://") || lower.starts_with("quic://") {
            return Err(format!(
                "`{s}`: h3:// and quic:// upstreams are not supported until phase 2"
            ));
        }
        if let Some(rest) = strip_prefix_ci(s, "tls://") {
            let (host, port) = host_port(rest, 853).map_err(|e| format!("`{s}`: {e}"))?;
            return Ok(UpstreamSpec::Tls { host, port });
        }
        if let Some(rest) = strip_prefix_ci(s, "tcp://") {
            let (host, port) = host_port(rest, 53).map_err(|e| format!("`{s}`: {e}"))?;
            return Ok(UpstreamSpec::Tcp { host, port });
        }
        if lower == "system" {
            return Err("`system` is not an upstream; the resolver expands it".to_string());
        }
        if let Ok(addr) = s.parse::<SocketAddr>() {
            return Ok(UpstreamSpec::Udp(addr));
        }
        let bare = s.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return Ok(UpstreamSpec::Udp(SocketAddr::new(ip, 53)));
        }
        Err(format!(
            "`{s}`: expected ip[:port], tcp://, tls:// or https://"
        ))
    }

    pub fn from_dns_server(s: &DnsServer) -> Option<UpstreamSpec> {
        match s {
            DnsServer::System => None,
            DnsServer::Udp(addr) => Some(UpstreamSpec::Udp(*addr)),
        }
    }

    pub fn from_encrypted(e: &EncryptedDns) -> Result<UpstreamSpec, String> {
        match e.scheme {
            EncryptedDnsScheme::H3 | EncryptedDnsScheme::Quic => Err(format!(
                "`{}`: h3:// and quic:// upstreams are not supported until phase 2",
                e.url
            )),
            _ => UpstreamSpec::parse(&e.url),
        }
    }

    pub fn from_dns_upstream(u: &DnsUpstream) -> Result<UpstreamSpec, String> {
        match u {
            DnsUpstream::Udp(addr) => Ok(UpstreamSpec::Udp(*addr)),
            DnsUpstream::Encrypted(e) => UpstreamSpec::from_encrypted(e),
        }
    }

    /// Plain UDP servers are the "traditional" upstreams used for bootstrap.
    pub fn is_traditional(&self) -> bool {
        matches!(self, UpstreamSpec::Udp(_))
    }

    pub fn name(&self) -> String {
        match self {
            UpstreamSpec::Udp(addr) => format!("udp://{addr}"),
            UpstreamSpec::Tcp { host, port } => format!("tcp://{host}:{port}"),
            UpstreamSpec::Tls { host, port } => format!("tls://{host}:{port}"),
            UpstreamSpec::Https(url) => url.as_str().to_string(),
        }
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            UpstreamSpec::Udp(_) => None,
            UpstreamSpec::Tcp { host, .. } | UpstreamSpec::Tls { host, .. } => Some(host),
            UpstreamSpec::Https(url) => url.host_str(),
        }
    }

    /// URL-type upstreams whose host is a name (not an IP literal).
    pub fn needs_bootstrap(&self) -> bool {
        self.host()
            .map(|h| {
                h.trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<IpAddr>()
                    .is_err()
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_form() {
        assert_eq!(
            UpstreamSpec::parse("1.1.1.1").unwrap(),
            UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap())
        );
        assert_eq!(
            UpstreamSpec::parse("192.0.2.53:5353").unwrap(),
            UpstreamSpec::Udp("192.0.2.53:5353".parse().unwrap())
        );
        assert_eq!(
            UpstreamSpec::parse("::1").unwrap(),
            UpstreamSpec::Udp("[::1]:53".parse().unwrap())
        );
        assert_eq!(
            UpstreamSpec::parse("[2001:db8::1]:5353").unwrap(),
            UpstreamSpec::Udp("[2001:db8::1]:5353".parse().unwrap())
        );
        assert_eq!(
            UpstreamSpec::parse("tcp://dns.Example.com").unwrap(),
            UpstreamSpec::Tcp {
                host: "dns.example.com".into(),
                port: 53
            }
        );
        assert_eq!(
            UpstreamSpec::parse("TLS://dns.example.com:8853/").unwrap(),
            UpstreamSpec::Tls {
                host: "dns.example.com".into(),
                port: 8853
            }
        );
        assert_eq!(
            UpstreamSpec::parse("tls://[::1]:853").unwrap(),
            UpstreamSpec::Tls {
                host: "::1".into(),
                port: 853
            }
        );
        assert!(matches!(
            UpstreamSpec::parse("https://dns.example.com/dns-query").unwrap(),
            UpstreamSpec::Https(_)
        ));
        assert!(
            UpstreamSpec::parse("h3://dns.example.com/dns-query")
                .unwrap_err()
                .contains("phase 2")
        );
        assert!(
            UpstreamSpec::parse("quic://dns.example.com")
                .unwrap_err()
                .contains("phase 2")
        );
        assert!(UpstreamSpec::parse("system").is_err());
        assert!(
            UpstreamSpec::parse("dns.example.com").is_err(),
            "hostnames are not allowed as plain servers"
        );
        assert!(UpstreamSpec::parse("tcp://").is_err());
        assert!(UpstreamSpec::parse("tls://host:99999").is_err());
    }

    #[test]
    fn names_hosts_and_bootstrap_need() {
        let udp = UpstreamSpec::parse("1.1.1.1").unwrap();
        assert_eq!(udp.name(), "udp://1.1.1.1:53");
        assert!(udp.is_traditional() && udp.host().is_none() && !udp.needs_bootstrap());
        let tls = UpstreamSpec::parse("tls://dns.example.com").unwrap();
        assert_eq!(tls.name(), "tls://dns.example.com:853");
        assert_eq!(tls.host(), Some("dns.example.com"));
        assert!(tls.needs_bootstrap() && !tls.is_traditional());
        let tcp_ip = UpstreamSpec::parse("tcp://9.9.9.9:53").unwrap();
        assert!(!tcp_ip.needs_bootstrap());
        let doh = UpstreamSpec::parse("https://1.1.1.1/dns-query").unwrap();
        assert_eq!(doh.name(), "https://1.1.1.1/dns-query");
        assert!(!doh.needs_bootstrap());
        assert!(
            UpstreamSpec::parse("https://dns.example.com/dns-query")
                .unwrap()
                .needs_bootstrap()
        );
    }

    #[test]
    fn conversions_from_config_types() {
        assert_eq!(UpstreamSpec::from_dns_server(&DnsServer::System), None);
        assert_eq!(
            UpstreamSpec::from_dns_server(&DnsServer::Udp("8.8.8.8:53".parse().unwrap())),
            Some(UpstreamSpec::Udp("8.8.8.8:53".parse().unwrap()))
        );
        let e = EncryptedDns::parse("https://dns.example.com/dns-query").unwrap();
        assert!(matches!(
            UpstreamSpec::from_encrypted(&e).unwrap(),
            UpstreamSpec::Https(_)
        ));
        let h3 = EncryptedDns::parse("h3://dns.example.com/dns-query").unwrap();
        assert!(UpstreamSpec::from_encrypted(&h3).is_err());
        assert_eq!(
            UpstreamSpec::from_dns_upstream(&DnsUpstream::Udp("1.1.1.1:53".parse().unwrap()))
                .unwrap(),
            UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap())
        );
    }
}
