//! `[Host]` local DNS mapping entries.

use crate::diagnostic::{ParseError, codes};
use crate::general::EncryptedDns;
use crate::glob::{Glob, GlobOptions};
use crate::rule::{ParseCtx, ResourceRef};
use crate::span::Span;
use crate::value::{split_definition, split_list, strip_prefix_ci};
use std::net::{IpAddr, SocketAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemMode {
    System,
    Syslib,
    ForceSyslib,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsUpstream {
    Udp(SocketAddr),
    Encrypted(EncryptedDns),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostValue {
    Ips(Vec<IpAddr>),
    Alias(String),
    Servers(Vec<DnsUpstream>),
    System(SystemMode),
    Script(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKey {
    Pattern(Glob),
    Set(ResourceRef),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    pub key: HostKey,
    pub raw_key: String,
    pub value: HostValue,
    pub span: Span,
}

fn parse_upstream(item: &str) -> Option<DnsUpstream> {
    if let Some(enc) = EncryptedDns::parse(item) {
        return Some(DnsUpstream::Encrypted(enc));
    }
    if let Ok(sa) = item.parse::<SocketAddr>() {
        return Some(DnsUpstream::Udp(sa));
    }
    let bare = item
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(item);
    bare.parse::<IpAddr>()
        .ok()
        .map(|ip| DnsUpstream::Udp(SocketAddr::new(ip, 53)))
}

pub fn parse_host_entry(raw: &str, ctx: &ParseCtx, span: &Span) -> Result<HostEntry, ParseError> {
    let (key, value) = split_definition(raw).ok_or_else(|| {
        ParseError::new(
            codes::E_INVALID_DEFINITION,
            format!("expected `<host> = <value>`, found `{raw}`"),
        )
    })?;
    if value.is_empty() {
        return Err(ParseError::new(
            codes::E_SYNTAX,
            format!("`{key}`: empty value"),
        ));
    }
    let host_key = if let Some(r) =
        strip_prefix_ci(key, "DOMAIN-SET:").or_else(|| strip_prefix_ci(key, "RULE-SET:"))
    {
        HostKey::Set(ResourceRef::parse(r, ctx))
    } else {
        HostKey::Pattern(
            Glob::new(
                key,
                GlobOptions {
                    case_insensitive: true,
                    classes: false,
                },
            )
            .map_err(|e| ParseError::new(codes::E_SYNTAX, format!("`{key}`: {e}")))?,
        )
    };
    let host_value = if let Some(rest) = strip_prefix_ci(value, "server:") {
        match rest.trim().to_ascii_lowercase().as_str() {
            "system" => HostValue::System(SystemMode::System),
            "syslib" => HostValue::System(SystemMode::Syslib),
            "force-syslib" => HostValue::System(SystemMode::ForceSyslib),
            "" => {
                return Err(ParseError::new(
                    codes::E_SYNTAX,
                    format!("`{key}`: `server:` needs at least one server"),
                ));
            }
            _ => {
                let mut servers = Vec::new();
                for item in split_list(rest) {
                    servers.push(parse_upstream(&item).ok_or_else(|| {
                        ParseError::new(
                            codes::E_SYNTAX,
                            format!("`{key}`: invalid DNS server `{item}`"),
                        )
                    })?);
                }
                HostValue::Servers(servers)
            }
        }
    } else if let Some(name) = strip_prefix_ci(value, "script:") {
        HostValue::Script(name.trim().to_string())
    } else {
        let items = split_list(value);
        let ips: Vec<IpAddr> = items
            .iter()
            .filter_map(|i| i.parse::<IpAddr>().ok())
            .collect();
        if ips.len() == items.len() && !ips.is_empty() {
            HostValue::Ips(ips)
        } else if items.len() == 1
            && !items[0].contains(' ')
            && items[0].contains('.')
            && ips.is_empty()
        {
            HostValue::Alias(items[0].to_ascii_lowercase())
        } else {
            return Err(ParseError::new(
                codes::E_SYNTAX,
                format!("`{key}`: expected IP addresses, a hostname alias, `server:` or `script:`"),
            ));
        }
    };
    Ok(HostEntry {
        key: host_key,
        raw_key: key.to_string(),
        value: host_value,
        span: span.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::ParseCtx;
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::Arc;

    fn parse(raw: &str) -> Result<HostEntry, ParseError> {
        let inline = HashSet::new();
        let ctx = ParseCtx {
            inline_rulesets: &inline,
            base_dir: Path::new("/p"),
        };
        parse_host_entry(raw, &ctx, &Span::new(Arc::from(Path::new("h.conf")), 1))
    }

    #[test]
    fn manual_examples() {
        let e = parse("abc.com = 1.2.3.4, 5.6.7.8, ::1").unwrap();
        assert!(matches!(e.value, HostValue::Ips(ref v) if v.len() == 3));
        assert!(matches!(e.key, HostKey::Pattern(_)));
        let e = parse("*.dev = 6.7.8.9").unwrap();
        assert!(matches!(&e.key, HostKey::Pattern(g) if g.matches("x.dev")));
        assert_eq!(
            parse("foo.com = bar.com").unwrap().value,
            HostValue::Alias("bar.com".into())
        );
        let e = parse("bar.com = server:8.8.8.8,1.1.1.1").unwrap();
        assert!(matches!(e.value, HostValue::Servers(ref s) if s.len() == 2));
        let e = parse("example.com = server:https://cloudflare-dns.com/dns-query").unwrap();
        assert!(
            matches!(e.value, HostValue::Servers(ref s) if matches!(s[0], DnsUpstream::Encrypted(_)))
        );
        assert_eq!(
            parse("Macbook = server:system").unwrap().value,
            HostValue::System(SystemMode::System)
        );
        assert_eq!(
            parse("x = server:syslib").unwrap().value,
            HostValue::System(SystemMode::Syslib)
        );
        assert_eq!(
            parse("x = server:force-syslib").unwrap().value,
            HostValue::System(SystemMode::ForceSyslib)
        );
        assert_eq!(
            parse("*.example.com = script:dnspod").unwrap().value,
            HostValue::Script("dnspod".into())
        );
        let e = parse(
            "DOMAIN-SET:https://example.com/domains.txt = server:https://doh.example.com/dns-query",
        )
        .unwrap();
        assert!(matches!(e.key, HostKey::Set(ResourceRef::Url(_))));
        let e = parse("RULE-SET:https://example.com/rules.txt = 10.0.0.10").unwrap();
        assert!(matches!(e.key, HostKey::Set(ResourceRef::Url(_))));
        assert_eq!(e.value, HostValue::Ips(vec!["10.0.0.10".parse().unwrap()]));
    }

    #[test]
    fn errors() {
        assert_eq!(
            parse("no-equals").unwrap_err().code,
            codes::E_INVALID_DEFINITION
        );
        assert_eq!(parse("a.com = ").unwrap_err().code, codes::E_SYNTAX);
        assert_eq!(parse("a.com = server:").unwrap_err().code, codes::E_SYNTAX);
        assert_eq!(
            parse("a.com = server:not an ip").unwrap_err().code,
            codes::E_SYNTAX
        );
        assert_eq!(
            parse("a.com = 1.2.3.4, not-ip").unwrap_err().code,
            codes::E_SYNTAX
        );
    }

    #[test]
    fn non_ascii_host_key_does_not_panic() {
        let e = parse("中文中文中文 = 1.2.3.4").unwrap();
        assert!(matches!(e.key, HostKey::Pattern(_)));
    }
}
