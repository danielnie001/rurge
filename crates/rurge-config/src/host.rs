//! `[Host]` local DNS mapping entries.

use crate::diagnostic::{ParseError, codes};
use crate::general::EncryptedDns;
use crate::glob::{Glob, GlobOptions};
use crate::redact::url_without_credentials;
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
    /// `DOMAIN-SET:<url-or-path>` — file in DOMAIN-SET format.
    DomainSet(ResourceRef),
    /// `RULE-SET:<url-or-path>` — file in RULE-SET format; only domain entries match.
    RuleSet(ResourceRef),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    pub key: HostKey,
    /// The key as the views show it: the summary, the resolver's messages and
    /// lookups. A set's URL in it has `***` for the user name and the
    /// password it carries; nothing reads the key again from here, `key`
    /// holds the URL as written.
    pub raw_key: String,
    pub value: HostValue,
    pub span: Span,
}

/// The prefixes of the keys that name a set.
const SET_KEY_PREFIXES: [&str; 2] = ["DOMAIN-SET:", "RULE-SET:"];

/// `key` with the user name and the password of a set's URL gone.
fn shown_key(key: &str) -> String {
    for prefix in SET_KEY_PREFIXES {
        if let Some(url) = strip_prefix_ci(key, prefix) {
            return format!("{}{}", &key[..prefix.len()], ResourceRef::shown_url(url));
        }
    }
    key.to_string()
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
    // A host never holds whitespace: a key that does came from an `=` further
    // on (a URL's query) standing in for the missing one after the host. Nor
    // does it hold a `://`, unless it is the URL of a set: a share link pasted
    // here has its first `=` in its query, and the password that stands in
    // front of it would be a host. Nothing of the line is quoted — it can
    // carry a token (M3-D7).
    let (key, value) = split_definition(raw)
        .filter(|(key, _)| {
            !key.contains(char::is_whitespace)
                && (!key.contains("://")
                    || SET_KEY_PREFIXES
                        .iter()
                        .any(|prefix| strip_prefix_ci(key, prefix).is_some()))
        })
        .ok_or_else(|| {
            ParseError::new(codes::E_INVALID_DEFINITION, "expected `<host> = <value>`")
        })?;
    // a set's URL can carry a user name and a password
    let shown = shown_key(key);
    if value.is_empty() {
        return Err(ParseError::new(
            codes::E_SYNTAX,
            format!("`{shown}`: empty value"),
        ));
    }
    let host_key = if let Some(r) = strip_prefix_ci(key, "DOMAIN-SET:") {
        HostKey::DomainSet(ResourceRef::parse_external(r, ctx))
    } else if let Some(r) = strip_prefix_ci(key, "RULE-SET:") {
        HostKey::RuleSet(ResourceRef::parse(r, ctx))
    } else {
        HostKey::Pattern(
            Glob::new(
                key,
                GlobOptions {
                    case_insensitive: true,
                    classes: false,
                },
            )
            .map_err(|e| ParseError::new(codes::E_SYNTAX, format!("`{shown}`: {e}")))?,
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
                    format!("`{shown}`: `server:` needs at least one server"),
                ));
            }
            _ => {
                let mut servers = Vec::new();
                for item in split_list(rest) {
                    servers.push(parse_upstream(&item).ok_or_else(|| {
                        ParseError::new(
                            codes::E_SYNTAX,
                            format!(
                                "`{shown}`: invalid DNS server `{}`",
                                url_without_credentials(&item)
                            ),
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
                format!(
                    "`{shown}`: expected IP addresses, a hostname alias, `server:` or `script:`"
                ),
            ));
        }
    };
    Ok(HostEntry {
        key: host_key,
        raw_key: shown,
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

    /// A line without the `=` after its host can carry a token (a DoH
    /// server's URL, a `DOMAIN-SET:` link), and it may still hold an `=`
    /// further on, in a URL's query: either way the error names it by its
    /// line number only.
    #[test]
    fn a_line_without_equals_is_not_quoted() {
        for raw in [
            "example.com server:https://doh.test/dns-query?id=t0k3n",
            "example.com server:https://doh.test/t0k3n/dns-query",
        ] {
            let e = parse(raw).unwrap_err();
            assert_eq!(
                (e.code, e.message.as_str()),
                (codes::E_INVALID_DEFINITION, "expected `<host> = <value>`"),
                "{raw}"
            );
        }
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
        assert!(matches!(e.key, HostKey::DomainSet(ResourceRef::Url(_))));
        let e = parse("RULE-SET:https://example.com/rules.txt = 10.0.0.10").unwrap();
        assert!(matches!(e.key, HostKey::RuleSet(ResourceRef::Url(_))));
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

    /// The key of a set is the set's URL, and the URL may carry a user name
    /// and a password. What shows the key — the summary, the resolver's
    /// messages and lookups, the diagnostics below — shows `***` for them;
    /// the entry keeps the URL as written, for that is what is fetched.
    #[test]
    fn the_key_of_a_set_is_shown_without_the_user_info_of_its_url() {
        for (key, shown) in [
            (
                "DOMAIN-SET:https://al1ce:hunter2@sets.example/d.txt",
                "DOMAIN-SET:https://***@sets.example/d.txt",
            ),
            (
                "rule-set:HTTPS://al1ce:p@ss@sets.example:8443/r.list#top",
                "rule-set:HTTPS://***@sets.example:8443/r.list#top",
            ),
            // an `@` further on is no credential
            (
                "DOMAIN-SET:https://cdn.example/gh/user/repo@main/d.txt",
                "DOMAIN-SET:https://cdn.example/gh/user/repo@main/d.txt",
            ),
            ("DOMAIN-SET:sets/d.txt", "DOMAIN-SET:sets/d.txt"),
            ("example.com", "example.com"),
        ] {
            let e = parse(&format!("{key} = 10.0.0.1")).unwrap();
            assert_eq!(e.raw_key, shown, "{key}");
            assert!(!format!("{e:?}").contains("hunter2"), "{key}");
        }
        let e = parse("DOMAIN-SET:https://al1ce:hunter2@sets.example/d.txt = 10.0.0.1").unwrap();
        assert_eq!(
            e.key,
            HostKey::DomainSet(ResourceRef::Url(
                "https://al1ce:hunter2@sets.example/d.txt".into()
            ))
        );
        let key = "RULE-SET:https://al1ce:hunter2@sets.example/r.list";
        for (rest, expected) in [
            ("", "empty value"),
            ("server:", "`server:` needs at least one server"),
            ("server:not an ip", "invalid DNS server `not an ip`"),
            (
                "1.2.3.4, not-ip",
                "expected IP addresses, a hostname alias, `server:` or `script:`",
            ),
        ] {
            let e = parse(&format!("{key} = {rest}")).unwrap_err();
            assert_eq!(
                e.message,
                format!("`RULE-SET:https://***@sets.example/r.list`: {expected}"),
                "{rest}"
            );
        }
    }

    /// A share link pasted under `[Host]` is `<key> = <value>` with the first
    /// `=` of the link: the key has no white space, and the value can read as
    /// an alias, so that the line used to load, the link's password in the
    /// key of the entry. A key with a `://` in it is a host only as the URL of
    /// a set.
    #[test]
    fn a_link_is_no_host_key() {
        for raw in [
            "trojan://hunter2@t.example:443?sni=t.example#HK",
            "hysteria2://hunter2@h.example:443/?obfs=salamander&obfs-password=hunter3#HK",
            "vmess://eyJhZGQiOiJ2LmV4YW1wbGUiLCJpZCI6Imh1bnRlcjIifQ==",
            "https://hunter2@sub.example/path = 1.2.3.4",
        ] {
            let e = parse(raw).unwrap_err();
            assert_eq!(
                (e.code, e.message.as_str()),
                (codes::E_INVALID_DEFINITION, "expected `<host> = <value>`"),
                "{raw}"
            );
        }
        // a set's key is one, and so is a host that merely looks like a link
        assert!(parse("DOMAIN-SET:https://example.com/list.txt = server:8.8.8.8").is_ok());
        assert!(parse("RULE-SET:https://example.com/list.txt = 10.0.0.1").is_ok());
        assert!(parse("example.com = 1.2.3.4").is_ok());
        assert!(parse("a:/b.example = 1.2.3.4").is_ok());
    }

    /// The server a diagnostic quotes is shown without the user name and the
    /// password it carries (the resolver sends them as `Authorization: Basic`).
    #[test]
    fn an_invalid_dns_server_is_quoted_without_its_credentials() {
        for (raw, expected) in [
            (
                "corp.example = server:htps://al1ce:hunter2@dns.example/dns-query",
                "`corp.example`: invalid DNS server `htps://***@dns.example/dns-query`",
            ),
            (
                "corp.example = server:8.8.8.8, al1ce:hunter2@1.1.1.1",
                "`corp.example`: invalid DNS server `***@1.1.1.1`",
            ),
            (
                "corp.example = server:8.8.8.8, not-an-ip",
                "`corp.example`: invalid DNS server `not-an-ip`",
            ),
        ] {
            let e = parse(raw).unwrap_err();
            assert_eq!(e.message, expected, "{raw}");
        }
    }
}
