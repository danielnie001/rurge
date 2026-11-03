//! `[MITM]` (manual: HTTP › MITM): what rurge decrypts, from which sources,
//! and which certificate authority it signs with.
//!
//! The section is read the way `[General]` is (`phase 4 M2 design 4.1`): the
//! first `[MITM]` section, one key per line, the later line of a key wins,
//! and key names are not case sensitive. The two host lists take their
//! default port 443 as the manual's page for them says (`Host List`), and
//! `hostname-disabled` removes the entries of `hostname` written the same
//! way (`phase 4 M2 design 4.2`). The CA is not checked here: what names a CA
//! that cannot be used is `W0039` at load time (`phase 4 M2 design 5`), and
//! the values are kept as written or dropped here.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::general::{bool_or, is_key_name, parse_host_list};
use crate::hostlist::HostList;
use crate::span::Span;
use crate::text::Section;
use crate::value::split_definition;
use ipnet::IpNet;
use std::fmt;
use std::net::IpAddr;

/// `[MITM]`, typed.
#[derive(Clone, PartialEq, Eq)]
pub struct MitmConfig {
    /// The hosts to decrypt, default port 443.
    pub hostname: HostList,
    /// The entries of `hostname` kept from taking effect, written the same way.
    pub hostname_disabled: HostList,
    /// `hostname` without the entries `hostname-disabled` names.
    pub effective: HostList,
    /// The PKCS#12 certificate authority, Base64, as written.
    pub ca_p12: Option<String>,
    /// Its passphrase; empty is taken (`phase 4 M2 design 4.1`).
    pub ca_passphrase: Option<String>,
    /// The `[Keystore]` item that holds the certificate authority instead.
    pub ca_keystore_name: Option<String>,
    /// Whether the certificate an origin presents is checked.
    pub skip_server_cert_verify: bool,
    /// Whether HTTP/2 is decrypted too; off by default (`phase 4 M2 design 10`).
    pub h2: bool,
    /// The sources allowed to be decrypted; `None` is every source.
    pub client_source_address: Option<SourceList>,
    /// Whether a QUIC flow to a name `[MITM]` decrypts is blocked so the client falls back to TCP; on by default (`phase 4 M5 design 6.1`).
    pub auto_quic_block: bool,
    /// The lines a problem found later is reported at.
    pub spans: MitmSpans,
}

impl Default for MitmConfig {
    fn default() -> Self {
        // An empty list still carries the default port 443 (`HostList`'s own
        // `Default` has none), so that an entry written later is read against
        // 443 and a list left out matches nothing anyway.
        MitmConfig {
            hostname: HostList::parse("", Some(443)),
            hostname_disabled: HostList::parse("", Some(443)),
            effective: HostList::parse("", Some(443)),
            ca_p12: None,
            ca_passphrase: None,
            ca_keystore_name: None,
            skip_server_cert_verify: false,
            h2: false,
            client_source_address: None,
            auto_quic_block: true,
            spans: MitmSpans::default(),
        }
    }
}

/// Without the two credentials, and without saying more of them than
/// whether each is there (`KeystoreItem`'s `Debug` does the same): a
/// passphrase of the CA and the Base64 of the PKCS#12 are what must not
/// leave the configuration.
impl fmt::Debug for MitmConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let MitmConfig {
            hostname,
            hostname_disabled,
            effective,
            ca_p12,
            ca_passphrase,
            ca_keystore_name,
            skip_server_cert_verify,
            h2,
            client_source_address,
            auto_quic_block,
            spans,
        } = self;
        f.debug_struct("MitmConfig")
            .field("hostname", hostname)
            .field("hostname_disabled", hostname_disabled)
            .field("effective", effective)
            .field("ca_p12", &ca_p12.as_ref().map(|_| "***"))
            .field("ca_passphrase", &ca_passphrase.as_ref().map(|_| "***"))
            .field("ca_keystore_name", ca_keystore_name)
            .field("skip_server_cert_verify", skip_server_cert_verify)
            .field("h2", h2)
            .field("client_source_address", client_source_address)
            .field("auto_quic_block", auto_quic_block)
            .field("spans", spans)
            .finish()
    }
}

/// The lines of `[MITM]` that a problem found later is reported at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MitmSpans {
    pub hostname: Option<Span>,
    pub ca_p12: Option<Span>,
    pub ca_keystore_name: Option<Span>,
}

/// `client-source-address`: the sources from which a connection may be
/// decrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceList {
    pub entries: Vec<SourceEntry>,
}

/// One item of `client-source-address`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceEntry {
    pub negate: bool,
    pub kind: SourceKind,
    pub raw: String,
}

/// What one item of `client-source-address` matches: an address or a network,
/// or a MAC address (which matches no source until phase 7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceKind {
    Net(IpNet),
    Mac([u8; 6]),
}

impl SourceList {
    /// The list and the entries that are neither an IP, a CIDR nor a MAC.
    ///
    /// An item is trimmed, a leading `-` makes it an exclusion, and what is
    /// left is read as an address (a `/32` or `/128` network), then as a
    /// network, then as a MAC address of six pairs of hex digits separated by
    /// `:` or by `-` (`phase 4 M2 design 4.3`).
    pub fn parse(text: &str) -> (SourceList, Vec<String>) {
        let mut list = SourceList {
            entries: Vec::new(),
        };
        let mut invalid = Vec::new();
        for item in text.split(',') {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            let (negate, body) = match item.strip_prefix('-') {
                Some(rest) => (true, rest.trim()),
                None => (false, item),
            };
            let kind = if let Ok(ip) = body.parse::<IpAddr>() {
                Some(SourceKind::Net(IpNet::from(ip)))
            } else if let Ok(net) = body.parse::<IpNet>() {
                Some(SourceKind::Net(net))
            } else {
                parse_mac(body).map(SourceKind::Mac)
            };
            match kind {
                Some(kind) => list.entries.push(SourceEntry {
                    negate,
                    kind,
                    raw: item.to_string(),
                }),
                None => invalid.push(item.to_string()),
            }
        }
        (list, invalid)
    }

    pub fn has_mac(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e.kind, SourceKind::Mac(_)))
    }

    /// The first entry that matches decides; none matches is no.
    ///
    /// The address is canonicalized first, so an IPv4 address written in its
    /// IPv6 form is compared with the IPv4 entries. A MAC entry matches no
    /// source: nothing tells rurge which device a connection comes from
    /// before phase 7.
    pub fn allows(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        for e in &self.entries {
            if let SourceKind::Net(net) = &e.kind
                && net.contains(&ip)
            {
                return !e.negate;
            }
        }
        false
    }
}

/// Six pairs of hex digits, separated by `:` throughout or by `-` throughout.
fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let separator = if text.contains(':') {
        ':'
    } else if text.contains('-') {
        '-'
    } else {
        return None;
    };
    let mut mac = [0u8; 6];
    let mut parts = 0;
    for part in text.split(separator) {
        if parts == mac.len() || part.len() != 2 {
            return None;
        }
        if !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        mac[parts] = u8::from_str_radix(part, 16).ok()?;
        parts += 1;
    }
    (parts == mac.len()).then_some(mac)
}

/// Reads `[MITM]`.
pub fn parse_mitm(section: Option<&Section>, diags: &mut Diagnostics) -> MitmConfig {
    let mut m = MitmConfig::default();
    let Some(section) = section else { return m };
    for e in section.active_entries() {
        let span = &e.span;
        let Some((key, value)) = split_definition(&e.raw) else {
            // nothing of the line: it can hold the passphrase of a CA
            diags.push(
                Diagnostic::error(codes::E_INVALID_DEFINITION, "expected `key = value`")
                    .at(span.clone()),
            );
            continue;
        };
        let key = key.to_ascii_lowercase();
        let key = key.as_str();
        match key {
            "hostname" => {
                m.hostname = parse_host_list(diags, span, key, value, DEFAULT_PORT);
                m.spans.hostname = Some(span.clone());
            }
            "hostname-disabled" => {
                m.hostname_disabled = parse_host_list(diags, span, key, value, DEFAULT_PORT)
            }
            "ca-p12" => {
                m.ca_p12 = written(value);
                m.spans.ca_p12 = Some(span.clone());
            }
            "ca-passphrase" => {
                // an empty passphrase is a passphrase rurge takes (Surge does not)
                m.ca_passphrase = Some(value.to_string());
            }
            "ca-keystore-name" => {
                m.ca_keystore_name = written(value);
                m.spans.ca_keystore_name = Some(span.clone());
            }
            "skip-server-cert-verify" => {
                m.skip_server_cert_verify =
                    bool_or(diags, span, key, value, m.skip_server_cert_verify)
            }
            "h2" => m.h2 = bool_or(diags, span, key, value, m.h2),
            "auto-quic-block" => {
                m.auto_quic_block = bool_or(diags, span, key, value, m.auto_quic_block)
            }
            "client-source-address" => m.client_source_address = source_list(diags, span, value),
            _ => {
                let told = if is_key_name(key) {
                    format!(" `{key}`")
                } else {
                    String::new()
                };
                diags.push(
                    Diagnostic::warning(
                        codes::W_UNKNOWN_KEY,
                        format!("unknown [MITM] key{told} ignored"),
                    )
                    .at(span.clone()),
                );
            }
        }
    }
    // whatever the order of the two lists, the effective one is the one left
    m.effective = m.hostname.without(&m.hostname_disabled);
    m
}

const DEFAULT_PORT: Option<u16> = Some(443);

/// A value that is not there at all is `None`: an empty `ca-p12` names no CA.
fn written(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn source_list(diags: &mut Diagnostics, span: &Span, value: &str) -> Option<SourceList> {
    if value.is_empty() {
        return None;
    }
    let (list, invalid) = SourceList::parse(value);
    for bad in &invalid {
        diags.push(
            Diagnostic::warning(
                codes::W_INVALID_HOST_LIST_ENTRY,
                format!("`client-source-address`: invalid entry `{bad}` ignored"),
            )
            .at(span.clone()),
        );
    }
    if list.has_mac() {
        diags.push(
            Diagnostic::warning(
                codes::W_PARAM_NOT_EFFECTIVE,
                "`client-source-address`: MAC address entries match no client in this version",
            )
            .at(span.clone()),
        );
    }
    Some(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    fn parse(text: &str) -> (MitmConfig, Diagnostics) {
        let (p, mut d) = parse_str(text, Arc::from(Path::new("m.conf")), Origin::Main);
        let m = parse_mitm(p.section("MITM"), &mut d);
        (m, d)
    }

    fn codes_of(d: &Diagnostics) -> Vec<&'static str> {
        d.iter().map(|x| x.code).collect()
    }

    fn raw_entries(list: &HostList) -> Vec<&str> {
        list.entries.iter().map(|e| e.raw.as_str()).collect()
    }

    /// The nine keys are read into their fields, and a section that writes
    /// none of them — or is not there at all — gives the defaults.
    #[test]
    fn the_nine_keys_are_read_and_default() {
        let (m, d) = parse(
            "[MITM]\nhostname = *.example.com, example.com:8443\n\
hostname-disabled = a.example.com\nca-p12 = AAAA\nca-passphrase = pw\n\
ca-keystore-name = mine\nskip-server-cert-verify = true\nh2 = true\n\
client-source-address = 192.168.1.0/24\nauto-quic-block = false\n",
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(m.hostname.default_port, Some(443));
        assert_eq!(
            raw_entries(&m.hostname),
            ["*.example.com", "example.com:8443"]
        );
        assert_eq!(raw_entries(&m.hostname_disabled), ["a.example.com"]);
        // `a.example.com` is not written in `hostname`, so nothing is removed
        assert_eq!(raw_entries(&m.effective), raw_entries(&m.hostname));
        assert_eq!(m.ca_p12.as_deref(), Some("AAAA"));
        assert_eq!(m.ca_passphrase.as_deref(), Some("pw"));
        assert_eq!(m.ca_keystore_name.as_deref(), Some("mine"));
        assert!(m.skip_server_cert_verify && m.h2);
        assert!(!m.auto_quic_block);
        assert!(m.client_source_address.is_some());
        assert_eq!(m.spans.hostname.as_ref().map(|s| s.line), Some(2));
        assert_eq!(m.spans.ca_p12.as_ref().map(|s| s.line), Some(4));
        assert_eq!(m.spans.ca_keystore_name.as_ref().map(|s| s.line), Some(6));

        let (m, d) = parse("[MITM]\n");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(m, MitmConfig::default());

        let (m, d) = parse("");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(m, MitmConfig::default());
        assert_eq!(m.hostname.default_port, Some(443));
        assert_eq!(m.hostname_disabled.default_port, Some(443));
        assert!(m.hostname.is_empty() && m.hostname_disabled.is_empty() && m.effective.is_empty());
        assert!(m.auto_quic_block);
        assert!(!m.h2 && !m.skip_server_cert_verify);
        assert!(m.client_source_address.is_none());
    }

    #[test]
    fn a_key_written_twice_takes_the_later_value_and_names_ignore_case() {
        let (m, d) = parse("[MITM]\nH2 = true\nh2 = false\n");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert!(!m.h2);
    }

    #[test]
    fn bad_values_warn_and_keep_the_default() {
        let (m, d) = parse(
            "[MITM]\nh2 = maybe\nskip-server-cert-verify = 2\nauto-quic-block = x\n\
hostname = a.com, [bad\n",
        );
        assert_eq!(
            codes_of(&d),
            [
                codes::W_INVALID_VALUE,
                codes::W_INVALID_VALUE,
                codes::W_INVALID_VALUE,
                codes::W_INVALID_HOST_LIST_ENTRY,
            ]
        );
        assert!(!m.h2 && !m.skip_server_cert_verify && m.auto_quic_block);
        assert_eq!(raw_entries(&m.hostname), ["a.com"]);
        let said: Vec<&str> = d.iter().map(|x| x.message.as_str()).collect();
        assert_eq!(said[3], "`hostname`: invalid entry `[bad` ignored");
    }

    #[test]
    fn unknown_keys_are_w0001_tcp_connection_included() {
        let (m, d) = parse("[MITM]\ntcp-connection = true\n");
        assert_eq!(codes_of(&d), [codes::W_UNKNOWN_KEY]);
        assert_eq!(
            d.iter().next().unwrap().message,
            "unknown [MITM] key `tcp-connection` ignored"
        );
        assert_eq!(m, MitmConfig::default());
    }

    /// A line without its `=` can carry a secret: the error quotes none of it.
    #[test]
    fn a_line_without_equals_is_an_error_that_quotes_nothing() {
        let (_, d) = parse("[MITM]\nca-passphrase hunter2\n");
        assert_eq!(codes_of(&d), [codes::E_INVALID_DEFINITION]);
        for x in d.iter() {
            assert_eq!(x.message, "expected `key = value`");
            assert!(!x.message.contains("hunter2"), "{x}");
            assert!(x.hint.is_none_or(|h| !h.contains("hunter2")), "{x}");
        }
    }

    /// `hostname-disabled` removes the entries written the same way, minus the
    /// `-` and the case; an entry written differently and an entry `hostname`
    /// does not have are left alone, the last without a warning.
    #[test]
    fn hostname_disabled_removes_entries_written_the_same() {
        let (m, d) = parse(
            "[MITM]\nhostname = *.Example.com, example.com, -a.com, b.com:443, c.com\n\
hostname-disabled = *.example.com, example.com:443, a.com, d.com\n",
        );
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(
            raw_entries(&m.effective),
            ["example.com", "-a.com", "b.com:443", "c.com"]
        );
        assert_eq!(m.effective.default_port, Some(443));
    }

    #[test]
    fn the_source_list_reads_ips_cidrs_exclusions_and_macs() {
        let (m, d) = parse(
            "[MITM]\nclient-source-address = -192.168.1.5, 192.168.1.0/24, ::1, 10.0.0.1, AA:BB:CC:DD:EE:FF, aa-bb-cc-dd-ee-ff\n",
        );
        assert_eq!(codes_of(&d), [codes::W_PARAM_NOT_EFFECTIVE]);
        assert_eq!(
            d.iter().next().unwrap().message,
            "`client-source-address`: MAC address entries match no client in this version"
        );
        let list = m.client_source_address.expect("a list");
        assert!(list.has_mac());
        assert!(!list.allows("192.168.1.5".parse().unwrap()));
        assert!(list.allows("192.168.1.9".parse().unwrap()));
        assert!(list.allows("10.0.0.1".parse().unwrap()));
        assert!(list.allows("::ffff:10.0.0.1".parse().unwrap()));
        assert!(!list.allows("10.0.0.2".parse().unwrap()));
        assert!(list.allows("::1".parse().unwrap()));
    }

    /// An empty value is no list at all (every source); a value whose items
    /// are all invalid is a list that allows nobody.
    #[test]
    fn an_empty_source_list_is_as_if_unwritten() {
        let (m, d) = parse("[MITM]\nclient-source-address =\n");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert!(m.client_source_address.is_none());

        let (m, d) = parse("[MITM]\nclient-source-address = bogus\n");
        assert_eq!(codes_of(&d), [codes::W_INVALID_HOST_LIST_ENTRY]);
        assert_eq!(
            d.iter().next().unwrap().message,
            "`client-source-address`: invalid entry `bogus` ignored"
        );
        let list = m.client_source_address.expect("a list");
        assert!(!list.allows("10.0.0.1".parse().unwrap()));
        assert!(!list.allows("::1".parse().unwrap()));
    }

    #[test]
    fn an_empty_passphrase_is_kept() {
        let (m, d) = parse("[MITM]\nca-passphrase =\n");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(m.ca_passphrase.as_deref(), Some(""));
    }

    /// The two credentials are what must not leave the configuration: the
    /// `Debug` says whether each is set and never what it is.
    #[test]
    fn the_debug_shows_neither_the_p12_nor_the_passphrase() {
        let (m, d) = parse("[MITM]\nca-p12 = SECRETP12\nca-passphrase = SECRETPW\n");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        for shown in [format!("{m:?}"), format!("{m:#?}")] {
            assert!(!shown.contains("SECRETP12"), "{shown}");
            assert!(!shown.contains("SECRETPW"), "{shown}");
            assert!(shown.contains("ca_p12"), "{shown}");
        }
    }
}
