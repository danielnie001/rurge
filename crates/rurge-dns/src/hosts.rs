//! `[Host]` mapping chain and the system hosts file (design §7.5).

use crate::upstream::UpstreamSpec;
use arc_swap::ArcSwap;
use rurge_config::host::{HostEntry, HostKey, HostValue, SystemMode};
use rurge_config::session::SessionInfo;
use rurge_config::{Diagnostic, Diagnostics, Glob, HostName, codes};
use rurge_rules::matcher::{EvalCtx, NoGeo, Verdict};
use rurge_rules::registry::SetRegistry;
use rurge_rules::set::SetHandle;
use rurge_rules::set_format::SetKind;
use std::borrow::Cow;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostAction {
    Ips(Vec<IpAddr>),
    Alias(String),
    Servers(Vec<UpstreamSpec>),
    /// `server:system` / `server:syslib` / `server:force-syslib`. In M2 the
    /// resolver treats every mode identically: all three go to the system's
    /// configured upstream servers. The distinction is preserved for M3.
    System(SystemMode),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostLookup {
    pub action: HostAction,
    /// The `[Host]` key text (or the matched hosts-file name).
    pub raw: String,
    pub etc_hosts: bool,
}

enum HostMatcher {
    Glob(Glob),
    Set(SetHandle),
}

struct HostRule {
    matcher: HostMatcher,
    action: HostAction,
    raw: String,
}

pub struct HostMap {
    rules: Vec<HostRule>,
    etc_hosts: ArcSwap<HashMap<String, Vec<IpAddr>>>,
}

impl HostMap {
    /// Compiles the `[Host]` section. `script:` values are skipped with W0027;
    /// unsupported `server:` upstreams are skipped with W0026.
    pub fn build(entries: &[HostEntry], sets: &SetRegistry, diags: &mut Diagnostics) -> HostMap {
        let mut rules = Vec::with_capacity(entries.len());
        for e in entries {
            let action = match &e.value {
                HostValue::Ips(v) => HostAction::Ips(v.clone()),
                HostValue::Alias(a) => {
                    HostAction::Alias(a.trim_end_matches('.').to_ascii_lowercase())
                }
                HostValue::Servers(list) => {
                    let mut specs = Vec::new();
                    for u in list {
                        match UpstreamSpec::from_dns_upstream(u) {
                            Ok(s) => specs.push(s),
                            Err(msg) => diags.push(
                                Diagnostic::warning(
                                    codes::W_DNS_UPSTREAM_UNSUPPORTED,
                                    format!("[Host] `{}`: {msg}", e.raw_key),
                                )
                                .at(e.span.clone()),
                            ),
                        }
                    }
                    if specs.is_empty() {
                        continue;
                    }
                    HostAction::Servers(specs)
                }
                HostValue::System(mode) => HostAction::System(*mode),
                HostValue::Script(name) => {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_HOST_SCRIPT_SKIPPED,
                            format!("[Host] `{}`: script `{name}` is not supported in this version; entry skipped", e.raw_key),
                        )
                        .at(e.span.clone()),
                    );
                    continue;
                }
            };
            let matcher = match &e.key {
                HostKey::Pattern(g) => HostMatcher::Glob(g.clone()),
                HostKey::DomainSet(r) => HostMatcher::Set(sets.get(r, SetKind::DomainSet)),
                HostKey::RuleSet(r) => HostMatcher::Set(sets.get(r, SetKind::RuleSet)),
            };
            rules.push(HostRule {
                matcher,
                action,
                raw: e.raw_key.clone(),
            });
        }
        HostMap {
            rules,
            etc_hosts: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    pub fn rules_len(&self) -> usize {
        self.rules.len()
    }

    pub fn etc_hosts_len(&self) -> usize {
        self.etc_hosts.load().len()
    }

    /// Replaces the hosts-file entries (called at start and on file change).
    pub fn set_etc_hosts(&self, entries: Vec<(String, IpAddr)>) {
        let mut map: HashMap<String, Vec<IpAddr>> = HashMap::new();
        for (name, ip) in entries {
            let v = map.entry(name).or_default();
            if !v.contains(&ip) {
                v.push(ip);
            }
        }
        self.etc_hosts.store(Arc::new(map));
    }

    /// `[Host]` rules in order, then the hosts file. `name` is normalised
    /// (lowercased, trailing dot stripped) before matching, so callers may
    /// pass it in any case or with a trailing dot.
    pub fn lookup(&self, name: &str) -> Option<HostLookup> {
        let name = normalize_name(name);
        let name = name.as_ref();
        for rule in &self.rules {
            let hit = match &rule.matcher {
                HostMatcher::Glob(g) => g.matches(name),
                HostMatcher::Set(handle) => set_matches(handle, name),
            };
            if hit {
                return Some(HostLookup {
                    action: rule.action.clone(),
                    raw: rule.raw.clone(),
                    etc_hosts: false,
                });
            }
        }
        self.etc_hosts.load().get(name).map(|ips| HostLookup {
            action: HostAction::Ips(ips.clone()),
            raw: name.to_string(),
            etc_hosts: true,
        })
    }
}

/// Lowercases and strips a trailing dot; borrows `name` unchanged when it is
/// already normalised, so an already-lowercase, dot-free name does not allocate.
fn normalize_name(name: &str) -> Cow<'_, str> {
    if name.ends_with('.') || name.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(name.trim_end_matches('.').to_ascii_lowercase())
    } else {
        Cow::Borrowed(name)
    }
}

/// Only domain entries of a set can match here (matching happens before any
/// resolution), so the set is evaluated with `no-resolve` forced.
fn set_matches(handle: &SetHandle, name: &str) -> bool {
    let session = SessionInfo::tcp(HostName::Domain(name.to_string()), 0);
    let mut ctx = EvalCtx::new(&NoGeo);
    handle.load().eval(&session, &mut ctx, true, false).verdict == Verdict::Match
}

/// `/etc/hosts` format: `<ip> <name> [<name>...]`, `#` comments.
pub fn parse_hosts_file(text: &str) -> Vec<(String, IpAddr)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(ip) = parts.next().and_then(|s| s.parse::<IpAddr>().ok()) else {
            continue;
        };
        for name in parts {
            let n = name.trim_end_matches('.').to_ascii_lowercase();
            if !n.is_empty() {
                out.push((n, ip));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::http::{HttpClient, HttpClientConfig};
    use rurge_net::resource::{ResourceManager, ResourceOptions};
    use std::path::Path;

    fn build(dir: &Path, host_section: &str) -> (HostMap, Diagnostics) {
        let text = format!("[Host]\n{host_section}\n[Rule]\nFINAL,DIRECT\n");
        let loaded = from_text(&text, &dir.join("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let cfg = loaded.config;
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        let resources = ResourceManager::with_options(
            dir.to_path_buf(),
            client,
            ResourceOptions {
                offline: true,
                ..ResourceOptions::default()
            },
        );
        let (sets, _) = SetRegistry::build(&cfg, resources, dir);
        let mut diags = Diagnostics::default();
        let map = HostMap::build(&cfg.hosts, sets.as_ref(), &mut diags);
        (map, diags)
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn first_match_wins_and_wildcards_follow_the_manual() {
        let dir = tempfile::tempdir().unwrap();
        let (map, diags) = build(
            dir.path(),
            "exact.com = 1.2.3.4\n*google.com = 5.6.7.8\n*.dev = 6.7.8.9, ::1\nalias.com = exact.com\n",
        );
        assert!(diags.is_empty());
        assert_eq!(map.rules_len(), 4);
        assert_eq!(
            map.lookup("exact.com").unwrap().action,
            HostAction::Ips(vec![ip("1.2.3.4")])
        );
        assert_eq!(
            map.lookup("google.com").unwrap().action,
            HostAction::Ips(vec![ip("5.6.7.8")])
        );
        assert_eq!(
            map.lookup("foo.google.com").unwrap().action,
            HostAction::Ips(vec![ip("5.6.7.8")])
        );
        assert_eq!(
            map.lookup("bargoogle.com").unwrap().action,
            HostAction::Ips(vec![ip("5.6.7.8")])
        );
        assert_eq!(
            map.lookup("app.dev").unwrap().action,
            HostAction::Ips(vec![ip("6.7.8.9"), ip("::1")])
        );
        assert!(
            map.lookup("dev").is_none(),
            "*.dev must not match the bare name"
        );
        assert_eq!(
            map.lookup("alias.com").unwrap().action,
            HostAction::Alias("exact.com".into())
        );
        assert_eq!(map.lookup("alias.com").unwrap().raw, "alias.com");
    }

    #[tokio::test]
    async fn server_system_and_script_values() {
        let dir = tempfile::tempdir().unwrap();
        let (map, diags) = build(
            dir.path(),
            "a.com = server:1.1.1.1, tls://dns.example.com\nb.com = server:system\nc.com = server:syslib\nd.com = script:my-script\ne.com = server:h3://dns.example.com/dns-query\nf.com = server:force-syslib\n",
        );
        let codes: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert!(codes.contains(&codes::W_HOST_SCRIPT_SKIPPED));
        assert!(codes.contains(&codes::W_DNS_UPSTREAM_UNSUPPORTED));
        assert_eq!(
            map.rules_len(),
            4,
            "script entry and all-unsupported entry are skipped"
        );
        match map.lookup("a.com").unwrap().action {
            HostAction::Servers(specs) => {
                assert_eq!(specs.len(), 2);
                assert_eq!(specs[0], UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap()));
                assert_eq!(specs[1].name(), "tls://dns.example.com:853");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(
            map.lookup("b.com").unwrap().action,
            HostAction::System(SystemMode::System)
        );
        assert_eq!(
            map.lookup("c.com").unwrap().action,
            HostAction::System(SystemMode::Syslib)
        );
        assert!(map.lookup("d.com").is_none());
        assert!(map.lookup("e.com").is_none());
        assert_eq!(
            map.lookup("f.com").unwrap().action,
            HostAction::System(SystemMode::ForceSyslib)
        );
    }

    #[tokio::test]
    async fn set_keys_match_domain_entries_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("d.txt"), ".set.example\nexact.example\n").unwrap();
        std::fs::write(
            dir.path().join("r.list"),
            "DOMAIN-SUFFIX,rule.example\nIP-CIDR,10.0.0.0/8\n",
        )
        .unwrap();
        let (map, diags) = build(
            dir.path(),
            "DOMAIN-SET:d.txt = 1.1.1.1\nRULE-SET:r.list = 2.2.2.2\n",
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| d.code).collect::<Vec<_>>()
        );
        assert_eq!(
            map.lookup("a.set.example").unwrap().action,
            HostAction::Ips(vec![ip("1.1.1.1")])
        );
        assert_eq!(
            map.lookup("exact.example").unwrap().action,
            HostAction::Ips(vec![ip("1.1.1.1")])
        );
        assert_eq!(
            map.lookup("x.rule.example").unwrap().action,
            HostAction::Ips(vec![ip("2.2.2.2")])
        );
        assert!(map.lookup("other.example").is_none());
    }

    #[tokio::test]
    async fn etc_hosts_come_after_host_rules_and_merge_families() {
        let dir = tempfile::tempdir().unwrap();
        let (map, _) = build(dir.path(), "dup.example = 9.9.9.9\n");
        let parsed = parse_hosts_file(
            "# comment\n127.0.0.1 localhost\n::1 localhost\n10.0.0.5   nas.lan nas   # trailing\nnot-an-ip name\n192.168.1.7 Printer.LAN.\n",
        );
        assert_eq!(parsed.len(), 5);
        map.set_etc_hosts(parsed);
        assert_eq!(map.etc_hosts_len(), 4);
        let hit = map.lookup("localhost").unwrap();
        assert!(hit.etc_hosts);
        assert_eq!(
            hit.action,
            HostAction::Ips(vec![ip("127.0.0.1"), ip("::1")])
        );
        assert_eq!(
            map.lookup("nas").unwrap().action,
            HostAction::Ips(vec![ip("10.0.0.5")])
        );
        assert_eq!(
            map.lookup("printer.lan").unwrap().action,
            HostAction::Ips(vec![ip("192.168.1.7")])
        );
        map.set_etc_hosts(vec![("dup.example".into(), ip("1.1.1.1"))]);
        assert_eq!(
            map.lookup("dup.example").unwrap().action,
            HostAction::Ips(vec![ip("9.9.9.9")]),
            "[Host] wins over hosts file"
        );
        assert!(map.lookup("gone.example").is_none());
        assert!(
            map.lookup("localhost").is_none(),
            "the second set_etc_hosts call must replace the table, not merge into it"
        );
    }

    #[tokio::test]
    async fn profile_order_decides_between_a_domain_set_and_a_pattern_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shared.txt"), "shared.example\n").unwrap();

        let (set_first, diags) = build(
            dir.path(),
            "DOMAIN-SET:shared.txt = 1.1.1.1\nshared.example = 2.2.2.2\n",
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| d.code).collect::<Vec<_>>()
        );
        assert_eq!(
            set_first.lookup("shared.example").unwrap().action,
            HostAction::Ips(vec![ip("1.1.1.1")]),
            "the DOMAIN-SET key listed first must win"
        );

        let (pattern_first, diags) = build(
            dir.path(),
            "shared.example = 2.2.2.2\nDOMAIN-SET:shared.txt = 1.1.1.1\n",
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| d.code).collect::<Vec<_>>()
        );
        assert_eq!(
            pattern_first.lookup("shared.example").unwrap().action,
            HostAction::Ips(vec![ip("2.2.2.2")]),
            "the pattern key listed first must win"
        );
    }

    #[tokio::test]
    async fn lookup_normalises_the_queried_name() {
        let dir = tempfile::tempdir().unwrap();
        let (map, diags) = build(dir.path(), "exact.example = 1.2.3.4\n");
        assert!(diags.is_empty());
        map.set_etc_hosts(parse_hosts_file("127.0.0.1 localhost\n"));
        assert_eq!(
            map.lookup("EXACT.Example.").unwrap().action,
            HostAction::Ips(vec![ip("1.2.3.4")]),
            "an uppercase, trailing-dot query must still hit a literal [Host] pattern"
        );
        let hit = map.lookup("Localhost.").unwrap();
        assert!(
            hit.etc_hosts,
            "an uppercase, trailing-dot query must still hit the hosts file"
        );
        assert_eq!(hit.action, HostAction::Ips(vec![ip("127.0.0.1")]));
    }
}
