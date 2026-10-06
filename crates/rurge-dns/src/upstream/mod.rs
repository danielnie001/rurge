//! Upstream transports (design §7.2). Every transport exchanges wire-format
//! DNS messages; message encoding lives in `crate::message`.

pub mod doh;
pub mod doh3;
pub mod doq;
mod session;
pub mod tcp;
pub mod udp;

use rurge_config::general::{DnsServer, EncryptedDns};
use rurge_config::host::DnsUpstream;
use rurge_net::BoxFuture;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::time::Instant;
use url::{Position, Url};

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
    Tcp {
        host: String,
        port: u16,
    },
    Tls {
        host: String,
        port: u16,
    },
    Https(Url),
    /// DNS over dedicated QUIC (RFC 9250): `quic://host[:port]`, port 853
    /// unless it says.
    Quic {
        host: String,
        port: u16,
    },
    /// DNS over HTTP/3: an `h3://` URL as the `https` one its requests go
    /// to — port 443 unless it says, the path `/dns-query` unless it names
    /// one.
    H3(Url),
}

/// The path of an `h3://` URL that names none, as sing-box, hickory and
/// CoreDNS take it (RFC 8484 itself sets none).
const DEFAULT_H3_PATH: &str = "/dns-query";

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

/// The refusal of a user name or a password in the upstream URL `s` —
/// `what` is it: "a quic:// URL" —, whose scheme `rest` follows. No
/// upstream has a place for them, and no text repeats them: this one quotes
/// only what follows the last `@`. A user name or a password may hold any
/// character — a `/`, `?` or `#` among them, which ends the authority
/// early —, so the `@` is looked for in all of `rest`.
fn userinfo_refused(s: &str, rest: &str, what: &str) -> String {
    let quoted = match rest.rfind('@') {
        Some(at) => format!("{}{}", &s[..s.len() - rest.len()], &rest[at + 1..]),
        None => s.to_string(),
    };
    format!("`{quoted}`: {what} cannot carry a user name or a password")
}

/// An `h3://` URL — `rest` follows the scheme — as the `https` one its
/// requests go to: no user name or password, which HTTP/3 has no place to
/// send (RFC 9114 4.3.1), no fragment, `DEFAULT_H3_PATH` when it names no
/// path.
fn h3_url(s: &str, rest: &str) -> Result<Url, String> {
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    if authority.contains('@') {
        return Err(userinfo_refused(s, rest, "an h3:// URL"));
    }
    let mut url = Url::parse(&format!("https://{rest}")).map_err(|e| {
        if rest.contains('@') {
            // a user name or a password that ended the authority early,
            // or a path with an `@`: nothing of it is quoted
            format!(
                "an h3:// URL cannot carry a user name or a password, and this one does not parse ({e})"
            )
        } else {
            format!("`{s}`: {e}")
        }
    })?;
    if url.path() == "/" {
        url.set_path(DEFAULT_H3_PATH);
    }
    url.set_fragment(None);
    Ok(url)
}

/// What an `H3` upstream is called: its URL with the scheme it was written
/// with.
fn h3_name(url: &Url) -> String {
    format!("h3://{}", &url[Position::BeforeHost..])
}

/// `host:port`, an IPv6 address in brackets.
fn host_port_text(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
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
        if let Some(rest) = strip_prefix_ci(s, "h3://") {
            return h3_url(s, rest).map(UpstreamSpec::H3);
        }
        // a host and a port — no `@` anywhere: a user name or a password
        // may hold the characters that end an authority, and there is no
        // telling where it does
        if let Some(rest) = strip_prefix_ci(s, "quic://") {
            if rest.contains('@') {
                return Err(userinfo_refused(s, rest, "a quic:// URL"));
            }
            // at most the one `/` of a URL that ends there
            if rest
                .strip_suffix('/')
                .unwrap_or(rest)
                .contains(['/', '?', '#'])
            {
                return Err(format!(
                    "`{s}`: a quic:// URL names a host and a port, nothing else"
                ));
            }
            let (host, port) = host_port(rest, 853).map_err(|e| format!("`{s}`: {e}"))?;
            return Ok(UpstreamSpec::Quic { host, port });
        }
        if let Some(rest) = strip_prefix_ci(s, "tls://") {
            if rest.contains('@') {
                return Err(userinfo_refused(s, rest, "a tls:// URL"));
            }
            let (host, port) = host_port(rest, 853).map_err(|e| format!("`{s}`: {e}"))?;
            return Ok(UpstreamSpec::Tls { host, port });
        }
        if let Some(rest) = strip_prefix_ci(s, "tcp://") {
            if rest.contains('@') {
                return Err(userinfo_refused(s, rest, "a tcp:// URL"));
            }
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
            "`{s}`: expected ip[:port], tcp://, tls://, https://, h3:// or quic://"
        ))
    }

    pub fn from_dns_server(s: &DnsServer) -> Option<UpstreamSpec> {
        match s {
            DnsServer::System => None,
            DnsServer::Udp(addr) => Some(UpstreamSpec::Udp(*addr)),
        }
    }

    pub fn from_encrypted(e: &EncryptedDns) -> Result<UpstreamSpec, String> {
        UpstreamSpec::parse(&e.url)
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
            UpstreamSpec::Quic { host, port } => format!("quic://{}", host_port_text(host, *port)),
            UpstreamSpec::H3(url) => h3_name(url),
        }
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            UpstreamSpec::Udp(_) => None,
            UpstreamSpec::Tcp { host, .. }
            | UpstreamSpec::Tls { host, .. }
            | UpstreamSpec::Quic { host, .. } => Some(host),
            UpstreamSpec::Https(url) | UpstreamSpec::H3(url) => url.host_str(),
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
        assert!(matches!(
            UpstreamSpec::parse("H3://dns.example.com/dns-query").unwrap(),
            UpstreamSpec::H3(_)
        ));
        assert!(matches!(
            UpstreamSpec::parse("Quic://dns.example.com").unwrap(),
            UpstreamSpec::Quic { .. }
        ));
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
        assert_eq!(
            UpstreamSpec::from_encrypted(&h3).unwrap(),
            UpstreamSpec::parse("h3://dns.example.com/dns-query").unwrap()
        );
        let doq = EncryptedDns::parse("quic://dns.example.com").unwrap();
        assert_eq!(
            UpstreamSpec::from_encrypted(&doq).unwrap(),
            UpstreamSpec::Quic {
                host: "dns.example.com".into(),
                port: 853
            }
        );
        assert_eq!(
            UpstreamSpec::from_dns_upstream(&DnsUpstream::Udp("1.1.1.1:53".parse().unwrap()))
                .unwrap(),
            UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap())
        );
    }

    /// `quic://` names a host and a port — 853 unless it says —, nothing
    /// else (RFC 9250 4.1.1).
    #[test]
    fn a_quic_url_is_a_host_and_a_port() {
        for (url, host, port) in [
            ("quic://dns.Example.com", "dns.example.com", 853),
            ("quic://dns.example.com:8853", "dns.example.com", 8853),
            ("quic://dns.example.com/", "dns.example.com", 853),
            ("quic://[::1]:853", "::1", 853),
            ("quic://127.0.0.1", "127.0.0.1", 853),
        ] {
            assert_eq!(
                UpstreamSpec::parse(url).unwrap(),
                UpstreamSpec::Quic {
                    host: host.into(),
                    port
                },
                "{url}"
            );
        }
        for url in [
            "quic://dns.example.com/dns-query",
            "quic://dns.example.com:853/x/",
            "quic://dns.example.com//",
            "quic://dns.example.com?x=1",
            "quic://",
            "quic://dns.example.com:99999",
        ] {
            assert!(UpstreamSpec::parse(url).is_err(), "{url}");
        }
        assert!(
            UpstreamSpec::parse("quic://dns.example.com/dns-query")
                .unwrap_err()
                .contains("names a host and a port, nothing else")
        );
    }

    /// `h3://` is the `https://` URL of its requests: port 443 unless it
    /// says, the path `/dns-query` unless it names one, never a user name
    /// or a password — which no error repeats.
    #[test]
    fn an_h3_url_defaults_to_port_443_and_the_dns_query_path() {
        let spec = |url: &str| match UpstreamSpec::parse(url).unwrap() {
            UpstreamSpec::H3(url) => url,
            other => panic!("{other:?}"),
        };
        for url in [
            "h3://dns.example.com",
            "h3://DNS.example.com/",
            "h3://dns.example.com:443",
        ] {
            let parsed = spec(url);
            assert_eq!(
                parsed.as_str(),
                "https://dns.example.com/dns-query",
                "{url}"
            );
            assert_eq!(parsed.port_or_known_default(), Some(443));
        }
        let parsed = spec("h3://dns.example.com:8443/q?x=1#part");
        assert_eq!(parsed.as_str(), "https://dns.example.com:8443/q?x=1");
        assert_eq!(spec("h3://[::1]/dns-query").host_str(), Some("[::1]"));
        let err = UpstreamSpec::parse("h3://user:secret@dns.example.com/dns-query").unwrap_err();
        assert_eq!(
            err,
            "`h3://dns.example.com/dns-query`: an h3:// URL cannot carry a user name or a password"
        );
        assert!(UpstreamSpec::parse("h3://user@dns.example.com").is_err());
        assert!(UpstreamSpec::parse("h3://").is_err());
    }

    /// No upstream URL has a place for a user name or a password, and no
    /// text says one: not the error, which quotes only what follows the
    /// last `@` — a `/`, `?` or `#` in a password ends the authority early,
    /// and the `@` is looked for beyond it —, nor an upstream's name.
    #[test]
    fn a_user_name_or_a_password_is_refused_and_never_quoted() {
        let secrets = ["alice", "hunter2", "hun/ter2", "hun?ter2", "hun#ter2"];
        for password in ["hunter2", "hun/ter2", "hun?ter2", "hun#ter2"] {
            for (scheme, server) in [
                ("quic", "dns.example"),
                ("quic", "dns.example:853"),
                ("QUIC", "dns.example/"),
                ("tls", "dns.example"),
                ("tls", "dns.example:8853"),
                ("tcp", "[::1]:53"),
                ("h3", "dns.example"),
                ("h3", "dns.example/dns-query"),
                ("H3", "dns.example:8443"),
            ] {
                for userinfo in [format!("alice:{password}"), "alice".to_string()] {
                    let url = format!("{scheme}://{userinfo}@{server}");
                    let err = UpstreamSpec::parse(&url).unwrap_err();
                    assert!(
                        err.contains("cannot carry a user name or a password"),
                        "{url}: {err}"
                    );
                    for secret in secrets {
                        assert!(!err.contains(secret), "{url}: {err}");
                    }
                }
            }
        }
        // what follows the last `@` is quoted, the scheme as written
        for (url, error) in [
            (
                "quic://user:pass@dns.example:853",
                "`quic://dns.example:853`: a quic:// URL cannot carry a user name or a password",
            ),
            (
                "TLS://user:p/ss@dns.example",
                "`TLS://dns.example`: a tls:// URL cannot carry a user name or a password",
            ),
            (
                "tcp://user:pass@dns.example",
                "`tcp://dns.example`: a tcp:// URL cannot carry a user name or a password",
            ),
        ] {
            assert_eq!(UpstreamSpec::parse(url).unwrap_err(), error);
        }
    }

    /// An `@` in the path of an `h3://` URL is no user name's. A URL that
    /// has one and does not parse might be a password with a `/` in it —
    /// the authority ends there —, and none of it is quoted.
    #[test]
    fn an_at_sign_in_an_h3_path_is_no_user_name() {
        match UpstreamSpec::parse("h3://dns.example/@doh/dns-query").unwrap() {
            UpstreamSpec::H3(url) => assert_eq!(url.as_str(), "https://dns.example/@doh/dns-query"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            UpstreamSpec::parse("h3://user:p/ss@dns.example").unwrap_err(),
            "an h3:// URL cannot carry a user name or a password, and this one does not parse (invalid port number)"
        );
    }

    #[test]
    fn quic_and_h3_names_hosts_and_bootstrap_need() {
        let doq = UpstreamSpec::parse("quic://dns.example.com").unwrap();
        assert_eq!(doq.name(), "quic://dns.example.com:853");
        assert_eq!(doq.host(), Some("dns.example.com"));
        assert!(doq.needs_bootstrap() && !doq.is_traditional());
        let doq_v6 = UpstreamSpec::parse("quic://[::1]:8853").unwrap();
        assert_eq!(doq_v6.name(), "quic://[::1]:8853");
        assert!(!doq_v6.needs_bootstrap());
        let doh3 = UpstreamSpec::parse("h3://dns.example.com").unwrap();
        assert_eq!(doh3.name(), "h3://dns.example.com/dns-query");
        assert_eq!(doh3.host(), Some("dns.example.com"));
        assert!(doh3.needs_bootstrap());
        let doh3_ip = UpstreamSpec::parse("h3://127.0.0.1:8443/q").unwrap();
        assert_eq!(doh3_ip.name(), "h3://127.0.0.1:8443/q");
        assert!(!doh3_ip.needs_bootstrap());
    }
}
