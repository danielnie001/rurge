//! `[SSID Setting]`: settings that hold while the machine is on a given
//! network (phase 3 M1 design 7.1).
//!
//! A line is `<subnet expression> key=value, key=value`. The Surge manual
//! does not say where the expression ends, nor how the next item of a list
//! (`dns-server`, `encrypted-dns-server`) is told from the next parameter,
//! both being separated by commas; this is rurge's reading of it:
//!
//! - the parameters start at the first place that has white space in front of
//!   it and reads `<known key>`, blanks if any, `=`; the expression is what
//!   comes before, so an SSID may have blanks in it;
//! - the parameters are cut into items the way every list of the profile is
//!   (`value::split_fields`); an item that starts `<known key>=` starts a
//!   parameter, any other item goes on with the list of the parameter before
//!   it, if that was a list. A URL such as `https://a.example/dns-query?x=1`
//!   has an `=` in it and is an item, because what stands in front of its `=`
//!   is no key. What is written `<name>=<value>` with a plain name is not an
//!   item either — no server and no URL is — but a parameter nobody knows.
//!
//! A line whose expression is wrong is an error and is left out; any other
//! line is kept, with a warning for each of its parameters that does nothing.
//! A diagnostic does not quote what a line says, with two exceptions that are
//! the other places' as well: a wrong expression, quoted only when it can be
//! one (`SubnetExpr::parse`; it is everything in front of the first known key,
//! so a misspelt key puts the whole parameter into it), and an item of a DNS
//! list that is none, told as `[General]` tells it (a URL without its user
//! name and its password).

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::general::{
    DnsItem, DnsServer, EncryptedDns, EncryptedItem, ios_only, read_dns_server_item,
    read_encrypted_dns_item,
};
use crate::policy::SubnetExpr;
use crate::span::Span;
use crate::text::Entry;
use crate::value::{parse_bool, split_fields};
use std::fmt;

/// The values of `tfo-behaviour`. It is read and does nothing: rurge does not
/// do TCP Fast Open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TfoBehaviour {
    Auto,
    ForceEnabled,
    ForceDisabled,
}

impl TfoBehaviour {
    fn parse(value: &str) -> Option<TfoBehaviour> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(TfoBehaviour::Auto),
            "force-enabled" => Some(TfoBehaviour::ForceEnabled),
            "force-disabled" => Some(TfoBehaviour::ForceDisabled),
            _ => None,
        }
    }
}

/// One line of `[SSID Setting]`.
#[derive(Clone, PartialEq, Eq)]
pub struct SsidSetting {
    pub expr: SubnetExpr,
    /// Pause the takeover while the network matches; not written is `false`.
    pub suspend: bool,
    /// The servers `dns-server` lists, when it lists any: they replace the
    /// profile's.
    pub dns_server: Option<Vec<DnsServer>>,
    /// The URLs `encrypted-dns-server` lists, and those written under
    /// `dns-server`, when there are any: they replace the profile's. `off` is
    /// `Some` of nothing — encrypted DNS is off on this network.
    pub encrypted_dns_server: Option<Vec<EncryptedDns>>,
    pub tfo_behaviour: Option<TfoBehaviour>,
    pub span: Span,
}

/// The DNS servers go through their own `Debug`, which leaves a URL's user
/// name and password out. The expression is everything in front of the first
/// known key, so when that key is misspelt it takes in the whole parameter, a
/// URL with its password among it: it is shown only when it can be what
/// somebody wrote as one expression (`SubnetExpr::is_quotable`), else `***`.
impl fmt::Debug for SsidSetting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let SsidSetting {
            expr,
            suspend,
            dns_server,
            encrypted_dns_server,
            tfo_behaviour,
            span,
        } = self;
        let expr: &dyn fmt::Debug = if expr.is_quotable() { expr } else { &"***" };
        f.debug_struct("SsidSetting")
            .field("expr", expr)
            .field("suspend", suspend)
            .field("dns_server", dns_server)
            .field("encrypted_dns_server", encrypted_dns_server)
            .field("tfo_behaviour", tfo_behaviour)
            .field("span", span)
            .finish()
    }
}

/// The keys a line can carry.
#[derive(Clone, Copy)]
enum Key {
    Suspend,
    DnsServer,
    EncryptedDnsServer,
    TfoBehaviour,
    CellularFallback,
    CellularMode,
}

impl Key {
    const ALL: [(&'static str, Key); 6] = [
        ("suspend", Key::Suspend),
        ("dns-server", Key::DnsServer),
        ("encrypted-dns-server", Key::EncryptedDnsServer),
        ("tfo-behaviour", Key::TfoBehaviour),
        ("cellular-fallback", Key::CellularFallback),
        ("cellular-mode", Key::CellularMode),
    ];

    /// The key `name` spells, without regard to case.
    fn parse(name: &str) -> Option<Key> {
        Key::ALL
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name.trim()))
            .map(|(_, key)| *key)
    }
}

/// The lines of `entries`, in the order written. A line whose expression is
/// wrong is an error (`E0011`) and is left out.
pub fn parse<'a>(
    entries: impl Iterator<Item = &'a Entry>,
    diags: &mut Diagnostics,
) -> Vec<SsidSetting> {
    entries
        .filter_map(|entry| parse_line(entry, diags))
        .collect()
}

fn parse_line(entry: &Entry, diags: &mut Diagnostics) -> Option<SsidSetting> {
    let raw = entry.raw.as_str();
    let (expression, parameters) = match parameters_start(raw) {
        Some(at) => (raw[..at].trim(), Some(&raw[at..])),
        None => (raw.trim(), None),
    };
    let expr = match SubnetExpr::parse(expression) {
        Ok(expr) => expr,
        Err(e) => {
            diags.push(Diagnostic::from_parse(e, entry.span.clone()));
            return None;
        }
    };
    let mut line = SsidSetting {
        expr,
        suspend: false,
        dns_server: None,
        encrypted_dns_server: None,
        tfo_behaviour: None,
        span: entry.span.clone(),
    };
    match parameters {
        Some(parameters) => read_parameters(parameters, &mut line, diags),
        None => diags.push(
            Diagnostic::warning(
                codes::W_PARAM_NOT_EFFECTIVE,
                "a [SSID Setting] line without parameters has no effect",
            )
            .at(entry.span.clone()),
        ),
    }
    Some(line)
}

/// Where the parameters of `line` start: at the first place that has white
/// space in front of it and reads like `starts_a_parameter`. `None` when
/// there is no such place, and the whole line is the expression.
fn parameters_start(line: &str) -> Option<usize> {
    let mut blank_before = false;
    for (at, c) in line.char_indices() {
        if blank_before && !c.is_whitespace() && starts_a_parameter(&line[at..]) {
            return Some(at);
        }
        blank_before = c.is_whitespace();
    }
    None
}

/// Whether `text` starts with a known key and then, blanks if any, an `=`.
fn starts_a_parameter(text: &str) -> bool {
    let name = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
        .unwrap_or(text.len());
    Key::parse(&text[..name]).is_some() && text[name..].trim_start().starts_with('=')
}

/// Whether `key` is written the way the name of a parameter is: letters,
/// digits, `-` and `_`, 32 at most. No DNS server and no URL is written
/// `<that>=<value>`, so an item that is, after a list, is a parameter nobody
/// knows and not one more item of the list.
fn is_parameter_name(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 32
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// The list the items that follow a parameter go on with.
#[derive(Clone, Copy)]
enum Open {
    Nothing,
    DnsServer,
    EncryptedDnsServer,
}

fn read_parameters(text: &str, line: &mut SsidSetting, diags: &mut Diagnostics) {
    let span = line.span.clone();
    let mut open = Open::Nothing;
    for field in split_fields(text) {
        if let Some((key, value)) = &field.param
            && let Some(key) = Key::parse(key)
        {
            open = Open::Nothing;
            match key {
                Key::Suspend => match parse_bool(value) {
                    Some(suspend) => line.suspend = suspend,
                    None => diags.push(
                        Diagnostic::warning(
                            codes::W_INVALID_VALUE,
                            "invalid value for `suspend` (expected `true` or `false`); ignored",
                        )
                        .at(span.clone()),
                    ),
                },
                Key::DnsServer => {
                    open = Open::DnsServer;
                    add_dns_server(line, value, diags);
                }
                Key::EncryptedDnsServer => {
                    open = Open::EncryptedDnsServer;
                    add_encrypted_dns_server(line, value, diags);
                }
                Key::TfoBehaviour => match TfoBehaviour::parse(value) {
                    Some(behaviour) => {
                        line.tfo_behaviour = Some(behaviour);
                        diags.push(
                            Diagnostic::warning(
                                codes::W_PARAM_NOT_EFFECTIVE,
                                "tfo-behaviour has no effect: TCP Fast Open is not implemented",
                            )
                            .at(span.clone()),
                        );
                    }
                    None => diags.push(
                        Diagnostic::warning(
                            codes::W_PARAM_NOT_APPLICABLE,
                            "invalid value for `tfo-behaviour` (expected `auto`, `force-enabled` or `force-disabled`); ignored",
                        )
                        .at(span.clone()),
                    ),
                },
                Key::CellularFallback => diags.push(ios_only("cellular-fallback", &span)),
                Key::CellularMode => diags.push(
                    Diagnostic::warning(
                        codes::W_PARAM_NOT_EFFECTIVE,
                        "cellular-mode has no effect: the metered network mode is not implemented",
                    )
                    .at(span.clone()),
                ),
            }
            continue;
        }
        // not the start of a parameter: the next item of the list before it,
        // unless it is written as a parameter of its own
        let a_parameter = field
            .param
            .as_ref()
            .is_some_and(|(key, _)| is_parameter_name(key));
        match open {
            Open::DnsServer if !a_parameter => add_dns_server(line, &field.text, diags),
            Open::EncryptedDnsServer if !a_parameter => {
                add_encrypted_dns_server(line, &field.text, diags)
            }
            // nothing is told of it but that it is there
            _ => {
                open = Open::Nothing;
                diags.push(
                    Diagnostic::warning(
                        codes::W_PARAM_NOT_APPLICABLE,
                        "unknown parameter in [SSID Setting]",
                    )
                    .at(span.clone()),
                );
            }
        }
    }
}

/// An item of `dns-server`; an encrypted URL written there is one of the
/// line's encrypted servers. A list is the line's only once an item has been
/// read into it: a typing mistake leaves the profile's servers in place
/// rather than take them away for nothing.
fn add_dns_server(line: &mut SsidSetting, item: &str, diags: &mut Diagnostics) {
    if item.trim().is_empty() {
        return;
    }
    match read_dns_server_item(item, "dns-server", &line.span, diags) {
        Some(DnsItem::Plain(server)) => line.dns_server.get_or_insert_with(Vec::new).push(server),
        Some(DnsItem::Encrypted(dns)) => line
            .encrypted_dns_server
            .get_or_insert_with(Vec::new)
            .push(dns),
        None => {}
    }
}

/// An item of `encrypted-dns-server`; `off` takes away what the line listed
/// before it, and is the one way to a list with nothing in it.
fn add_encrypted_dns_server(line: &mut SsidSetting, item: &str, diags: &mut Diagnostics) {
    if item.trim().is_empty() {
        return;
    }
    match read_encrypted_dns_item(item, "encrypted-dns-server", &line.span, diags) {
        Some(EncryptedItem::Off) => line
            .encrypted_dns_server
            .get_or_insert_with(Vec::new)
            .clear(),
        Some(EncryptedItem::Url(dns)) => line
            .encrypted_dns_server
            .get_or_insert_with(Vec::new)
            .push(dns),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;
    use crate::general::EncryptedDnsScheme;
    use crate::text::{Origin, Section, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    /// The `[SSID Setting]` lines of `lines`, as the loader reads them, and
    /// what it said about them (the text layer's own diagnostics included).
    fn read(lines: &str) -> (Vec<SsidSetting>, Vec<Diagnostic>) {
        let text = format!("[SSID Setting]\n{lines}\n");
        let (profile, mut diags) = parse_str(&text, Arc::from(Path::new("s.conf")), Origin::Main);
        let found = parse(
            profile
                .section("SSID Setting")
                .into_iter()
                .flat_map(Section::active_entries),
            &mut diags,
        );
        (found, diags.into_vec())
    }

    fn said(diags: &[Diagnostic]) -> Vec<(&'static str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn expr(text: &str) -> SubnetExpr {
        SubnetExpr::parse(text).unwrap()
    }

    fn udp(addr: &str) -> DnsServer {
        DnsServer::Udp(addr.parse().unwrap())
    }

    fn urls(line: &SsidSetting) -> Vec<&str> {
        line.encrypted_dns_server
            .iter()
            .flatten()
            .map(|dns| dns.url.as_str())
            .collect()
    }

    /// One line of each shape the design lists (phase 3 M1 design 7.1).
    #[test]
    fn ssid_setting_lines_are_read() {
        // the expression is everything in front of the first known key, and
        // an SSID may have blanks in it
        let (lines, diags) = read("SSID:My Home suspend=true");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].expr, expr("SSID:My Home"));
        assert!(lines[0].suspend);
        assert_eq!(lines[0].dns_server, None);
        assert_eq!(lines[0].encrypted_dns_server, None);
        assert_eq!(lines[0].tfo_behaviour, None);
        // the line of the profile it was read from
        assert_eq!(lines[0].span.line, 2);

        // a list ends where the next known key starts
        let (lines, diags) = read("SSID:Home dns-server=1.1.1.1,8.8.8.8,suspend=true");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            lines[0].dns_server,
            Some(vec![udp("1.1.1.1:53"), udp("8.8.8.8:53")])
        );
        assert!(lines[0].suspend);
        assert_eq!(lines[0].encrypted_dns_server, None);

        // a URL with `=` and `,` in it is an item of the list, never a
        // parameter
        let (lines, diags) = read(
            "SSID:Home encrypted-dns-server=https://a.example/dns-query?x=1,https://b.example/",
        );
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            urls(&lines[0]),
            ["https://a.example/dns-query?x=1", "https://b.example/"]
        );
        assert!(
            lines[0]
                .encrypted_dns_server
                .iter()
                .flatten()
                .all(|dns| dns.scheme == EncryptedDnsScheme::Https)
        );
        assert_eq!(lines[0].dns_server, None);
        assert!(!lines[0].suspend);

        // `off` is an override that lists nothing
        let (lines, diags) = read("SSID:Office dns-server=192.168.1.1,encrypted-dns-server=off");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(lines[0].dns_server, Some(vec![udp("192.168.1.1:53")]));
        assert_eq!(lines[0].encrypted_dns_server, Some(Vec::new()));

        // an unknown key is said once, without being quoted, and the rest of
        // the line is still read
        let (lines, diags) = read("SSID:Home suspend=true,mystery=1");
        assert_eq!(
            said(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "unknown parameter in [SSID Setting]"
            )]
        );
        assert_eq!(diags[0].severity, Severity::Warning);
        assert_eq!(diags[0].span.as_ref().map(|span| span.line), Some(2));
        assert!(lines[0].suspend);

        // what is parsed and does nothing is said so
        let (lines, diags) = read("SSID:Home tfo-behaviour=force-enabled");
        assert_eq!(
            said(&diags),
            [(
                codes::W_PARAM_NOT_EFFECTIVE,
                "tfo-behaviour has no effect: TCP Fast Open is not implemented"
            )]
        );
        assert_eq!(lines[0].tfo_behaviour, Some(TfoBehaviour::ForceEnabled));
        let (lines, diags) = read("SSID:Home cellular-mode=true");
        assert_eq!(
            said(&diags),
            [(
                codes::W_PARAM_NOT_EFFECTIVE,
                "cellular-mode has no effect: the metered network mode is not implemented"
            )]
        );
        assert_eq!(lines.len(), 1);
        let (lines, diags) = read("SSID:Home cellular-fallback=wifi-assist");
        assert_eq!(
            said(&diags),
            [(
                codes::W_PLATFORM_IGNORED,
                "`cellular-fallback` is iOS-only and has no effect on desktop platforms"
            )]
        );
        assert_eq!(lines.len(), 1);

        // an expression that is wrong is an error, and the line is not used
        let (lines, diags) = read("TYPE:SATELLITE suspend=true");
        assert_eq!(
            said(&diags),
            [(
                codes::E_INVALID_RULE_VALUE,
                "invalid subnet expression `TYPE:SATELLITE`: expected WIFI, WIRED or CELLULAR"
            )]
        );
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(diags[0].hint, codes::hint(codes::E_INVALID_RULE_VALUE));
        assert!(lines.is_empty());

        // a line without parameters is kept and does nothing
        let (lines, diags) = read("SSID:Home");
        assert_eq!(
            said(&diags),
            [(
                codes::W_PARAM_NOT_EFFECTIVE,
                "a [SSID Setting] line without parameters has no effect"
            )]
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].expr, expr("SSID:Home"));
        assert!(!lines[0].suspend);
        assert_eq!(lines[0].dns_server, None);
    }

    /// Lines are kept in the order they are written, each with its own line
    /// of the profile; one that is wrong does not spoil the next.
    #[test]
    fn lines_keep_their_order_and_their_places() {
        let (lines, diags) = read("SSID:A suspend=true\nTYPE:SATELLITE suspend=true\nTYPE:WIFI");
        assert_eq!(
            said(&diags)
                .iter()
                .map(|(code, _)| *code)
                .collect::<Vec<_>>(),
            [codes::E_INVALID_RULE_VALUE, codes::W_PARAM_NOT_EFFECTIVE]
        );
        let found: Vec<(SubnetExpr, u32)> = lines
            .iter()
            .map(|line| (line.expr.clone(), line.span.line))
            .collect();
        assert_eq!(found, [(expr("SSID:A"), 2), (expr("TYPE:WIFI"), 4)]);
    }

    /// The expression ends in front of the first `<known key> =`, which has
    /// white space in front of it; the key is not case sensitive and may have
    /// blanks around its `=`.
    #[test]
    fn the_expression_ends_in_front_of_the_first_known_key() {
        for (line, expression, suspend) in [
            ("SSID:Home suspend=true", "SSID:Home", true),
            ("SSID:Home\tsuspend=true", "SSID:Home", true),
            ("SSID:Home   SUSPEND = true", "SSID:Home", true),
            ("SSID:Home Suspend= false", "SSID:Home", false),
            // a word that is a key's name but has no `=` after it is the
            // SSID's
            ("SSID:suspend now suspend=true", "SSID:suspend now", true),
            ("SSID:Home, Sweet suspend=true", "SSID:Home, Sweet", true),
            ("SSID:a=b suspend=true", "SSID:a=b", true),
            // a key that is not one of the six is part of the expression
            (
                "SSID:Home token=abc suspend=true",
                "SSID:Home token=abc",
                true,
            ),
            (
                "ROUTER:192.168.1.1 suspend=true",
                "ROUTER:192.168.1.1",
                true,
            ),
            ("TYPE:WIRED suspend=true", "TYPE:WIRED", true),
            ("My Home suspend=true", "My Home", true),
        ] {
            let (lines, diags) = read(line);
            assert!(diags.is_empty(), "{line}: {diags:?}");
            assert_eq!(lines.len(), 1, "{line}");
            assert_eq!(lines[0].expr, expr(expression), "{line}");
            assert_eq!(lines[0].suspend, suspend, "{line}");
        }
        // no white space in front of the key: it is the expression's, and
        // there is nothing else on the line
        let (lines, diags) = read("SSID:Home,suspend=true");
        assert_eq!(
            said(&diags),
            [(
                codes::W_PARAM_NOT_EFFECTIVE,
                "a [SSID Setting] line without parameters has no effect"
            )]
        );
        assert_eq!(lines[0].expr, expr("SSID:Home,suspend=true"));
        assert!(!lines[0].suspend);
        // a value in quotes is read as one anywhere
        let (lines, _) = read("SSID:Home dns-server=1.1.1.1,suspend=\"true\"");
        assert!(lines[0].suspend);
    }

    #[test]
    fn the_dns_servers_of_a_line_are_read_as_those_of_general_are() {
        let (lines, diags) = read("SSID:Home dns-server=system,[2001:db8::53]:5353,::1");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            lines[0].dns_server,
            Some(vec![
                DnsServer::System,
                udp("[2001:db8::53]:5353"),
                udp("[::1]:53")
            ])
        );
        // a hostname is no server: said as `[General]` says it, and the rest
        // of the list stands
        let (lines, diags) = read("SSID:Home dns-server=localhost,1.1.1.1");
        assert_eq!(
            said(&diags),
            [(
                codes::W_INVALID_VALUE,
                "invalid value `localhost` for `dns-server`, using default"
            )]
        );
        assert_eq!(lines[0].dns_server, Some(vec![udp("1.1.1.1:53")]));
        // an encrypted URL under `dns-server` is an encrypted server of the
        // line
        let (lines, diags) = read("SSID:Home dns-server=1.1.1.1,https://doh.example/dns-query");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(lines[0].dns_server, Some(vec![udp("1.1.1.1:53")]));
        assert_eq!(urls(&lines[0]), ["https://doh.example/dns-query"]);
        // a line that lists no plain server leaves the profile's alone
        let (lines, diags) = read("SSID:Home dns-server=https://doh.example/dns-query");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(lines[0].dns_server, None);
        assert_eq!(urls(&lines[0]), ["https://doh.example/dns-query"]);
        // `off` clears what was listed before it, a URL moved over from
        // `dns-server` included, and not what comes after
        let (lines, diags) = read(
            "SSID:Home dns-server=https://one.example/dns-query,encrypted-dns-server=tls://two.example,off,tls://three.example",
        );
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(urls(&lines[0]), ["tls://three.example"]);
        let (lines, _) =
            read("SSID:Home dns-server=https://one.example/dns-query,encrypted-dns-server=off");
        assert_eq!(lines[0].encrypted_dns_server, Some(Vec::new()));
        // a key written twice adds to its list, `suspend` is the last one
        let (lines, _) =
            read("SSID:Home dns-server=1.1.1.1,suspend=true,dns-server=8.8.8.8,suspend=false");
        assert_eq!(
            lines[0].dns_server,
            Some(vec![udp("1.1.1.1:53"), udp("8.8.8.8:53")])
        );
        assert!(!lines[0].suspend);
    }

    /// A list is the line's once something was read into it: a key with
    /// nothing after it, or with items that are none, does not take the
    /// profile's servers away for nothing. `off` is the way to a list with
    /// nothing in it.
    #[test]
    fn a_list_nothing_was_read_into_is_not_the_lines() {
        for line in [
            "SSID:Home dns-server=",
            "SSID:Home dns-server=localhost",
            "SSID:Home dns-server=localhost,suspend=true",
            "SSID:Home encrypted-dns-server=",
            "SSID:Home encrypted-dns-server=htps://bad.example/dns-query,suspend=true",
        ] {
            let (lines, _) = read(line);
            assert_eq!(lines.len(), 1, "{line}");
            assert_eq!(lines[0].dns_server, None, "{line}");
            assert_eq!(lines[0].encrypted_dns_server, None, "{line}");
        }
        let (lines, diags) = read("SSID:Home encrypted-dns-server=OFF");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(lines[0].encrypted_dns_server, Some(Vec::new()));
        assert_eq!(lines[0].dns_server, None);
    }

    /// An unknown key is said, never quoted; where it stands after a list it
    /// is still a key, for no server and no URL is written `<name>=<value>`.
    #[test]
    fn an_unknown_parameter_is_said_and_never_quoted() {
        for line in [
            "SSID:Home suspend=true,mystery=1",
            "SSID:Home dns-server=1.1.1.1,mystery=1",
            "SSID:Home encrypted-dns-server=off,mystery=1",
            "SSID:Home tfo-behaviour=auto,Mystery-Key_2 = 1",
            // a value nothing is waiting for
            "SSID:Home suspend=true,mystery",
            "SSID:Home suspend=true,192.168.1.1",
        ] {
            let (lines, diags) = read(line);
            let unknown: Vec<&Diagnostic> = diags
                .iter()
                .filter(|d| d.code == codes::W_PARAM_NOT_APPLICABLE)
                .collect();
            assert_eq!(unknown.len(), 1, "{line}: {diags:?}");
            assert_eq!(unknown[0].message, "unknown parameter in [SSID Setting]");
            for d in &diags {
                assert!(!d.message.to_ascii_lowercase().contains("mystery"), "{d}");
            }
            assert_eq!(lines.len(), 1, "{line}");
        }
        // the list before an unknown key is the line's
        let (lines, _) = read("SSID:Home dns-server=1.1.1.1,mystery=1");
        assert_eq!(lines[0].dns_server, Some(vec![udp("1.1.1.1:53")]));
    }

    #[test]
    fn tfo_behaviour_takes_three_values() {
        for (value, parsed) in [
            ("auto", TfoBehaviour::Auto),
            ("force-enabled", TfoBehaviour::ForceEnabled),
            ("FORCE-DISABLED", TfoBehaviour::ForceDisabled),
        ] {
            let (lines, diags) = read(&format!("SSID:Home tfo-behaviour={value}"));
            assert_eq!(lines[0].tfo_behaviour, Some(parsed), "{value}");
            assert_eq!(
                said(&diags),
                [(
                    codes::W_PARAM_NOT_EFFECTIVE,
                    "tfo-behaviour has no effect: TCP Fast Open is not implemented"
                )],
                "{value}"
            );
        }
        // a value that is none is said without being quoted, and it is not
        // taken for the other kind of problem
        let (lines, diags) = read("SSID:Home tfo-behaviour=always");
        assert_eq!(
            said(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "invalid value for `tfo-behaviour` (expected `auto`, `force-enabled` or `force-disabled`); ignored"
            )]
        );
        assert_eq!(lines[0].tfo_behaviour, None);
    }

    #[test]
    fn suspend_takes_the_booleans_of_general() {
        for (value, suspend) in [
            ("true", true),
            ("TRUE", true),
            ("1", true),
            ("yes", true),
            ("false", false),
            ("0", false),
            ("no", false),
        ] {
            let (lines, diags) = read(&format!("SSID:Home suspend={value}"));
            assert!(diags.is_empty(), "{value}: {diags:?}");
            assert_eq!(lines[0].suspend, suspend, "{value}");
        }
        let (lines, diags) = read("SSID:Home suspend=maybe");
        assert_eq!(
            said(&diags),
            [(
                codes::W_INVALID_VALUE,
                "invalid value for `suspend` (expected `true` or `false`); ignored"
            )]
        );
        assert!(!lines[0].suspend);
    }

    const MARKER: &str = "S3CR3T";

    /// A user name and a password in a URL are kept in what is read, for that
    /// is what the resolver connects with, and nowhere else: not in the
    /// diagnostic about a URL that is none, nor in the `Debug` of the line.
    #[test]
    fn a_password_in_a_dns_url_is_kept_and_shown_nowhere() {
        let url = format!("https://al1ce:{MARKER}@dns.example/dns-query");
        let (lines, diags) = read(&format!(
            "SSID:Home encrypted-dns-server={url},suspend=true"
        ));
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(urls(&lines[0]), [url.as_str()]);
        let shown = format!("{:?}\n{:#?}", lines[0], lines);
        assert!(!shown.contains(MARKER), "{shown}");
        assert!(
            shown.contains("https://***@dns.example/dns-query"),
            "{shown}"
        );

        for line in [
            format!("SSID:Home encrypted-dns-server=htps://al1ce:{MARKER}@dns.example/dns-query"),
            format!("SSID:Home dns-server=htps://al1ce:{MARKER}@dns.example/dns-query"),
            // the comma between two parameters is missing: the second one is
            // in the value of the first
            format!(
                "SSID:Home dns-server=1.1.1.1 encrypted-dns-server=https://al1ce:{MARKER}@dns.example/dns-query"
            ),
            format!("SSID:Home dns-server=1.1.1.1,{MARKER}-flag=1,suspend=true"),
            format!("SSID:Home suspend={MARKER},dns-server=1.1.1.1"),
            format!("SSID:Home tfo-behaviour={MARKER}"),
            format!("SSID:Home cellular-fallback={MARKER},cellular-mode={MARKER}"),
            format!("SSID:Home suspend=true,{MARKER}"),
        ] {
            let (lines, diags) = read(&line);
            for d in &diags {
                let shown = format!("{d} {d:?}").to_ascii_uppercase();
                assert!(!shown.contains(MARKER), "{line}: {d}");
            }
            let shown = format!("{lines:?}").to_ascii_uppercase();
            assert!(!shown.contains(MARKER), "{line}: {shown}");
        }
        // the URL is told as the other places tell one that is none
        let (_, diags) = read(&format!(
            "SSID:Home encrypted-dns-server=htps://al1ce:{MARKER}@dns.example/dns-query"
        ));
        assert_eq!(
            said(&diags),
            [(
                codes::W_INVALID_VALUE,
                "invalid value `htps://***@dns.example/dns-query` for `encrypted-dns-server`, using default"
            )]
        );
    }

    /// The expression is everything in front of the first known key, so a
    /// first parameter whose key is misspelt, or that follows a comma where a
    /// blank should be, is in it whole — a DNS URL with its password among
    /// it. A wrong expression is told without a word of it unless it can be
    /// quoted (`spec::reader::quotable`).
    #[test]
    fn a_wrong_expression_is_told_without_the_line_unless_it_is_quotable() {
        for (line, told) in [
            (
                format!("TYPE:WIFI dns-sever=https://al1ce:{MARKER}@dns.example/dns-query"),
                "invalid subnet expression: expected WIFI, WIRED or CELLULAR",
            ),
            (
                format!(
                    "ROUTER:192.168.1.1,dns-server=https://al1ce:{MARKER}@dns.example/dns-query"
                ),
                "invalid subnet expression: expected an IP address",
            ),
            (
                "MCCMNC:310 260 suspend=true".to_string(),
                "invalid subnet expression: expected MCC+MNC digits",
            ),
            // short, and with no blank or `=` in it: quoted, as it always was
            (
                "TYPE:SATELLITE suspend=true".to_string(),
                "invalid subnet expression `TYPE:SATELLITE`: expected WIFI, WIRED or CELLULAR",
            ),
            (
                "ROUTER:nope dns-server=1.1.1.1".to_string(),
                "invalid subnet expression `ROUTER:nope`: expected an IP address",
            ),
        ] {
            let (lines, diags) = read(&line);
            assert_eq!(
                said(&diags),
                [(codes::E_INVALID_RULE_VALUE, told)],
                "{line}"
            );
            assert!(lines.is_empty(), "{line}");
            for d in &diags {
                let shown = format!("{d} {d:?}").to_ascii_uppercase();
                assert!(!shown.contains(MARKER), "{line}: {d}");
            }
        }
    }

    /// The `Debug` of a line shows its expression on the same terms. An SSID
    /// with a blank in it is not shown either: nothing tells it from a
    /// parameter that slid into the expression.
    #[test]
    fn the_debug_of_a_line_shows_its_expression_only_when_it_is_quotable() {
        // the key of the first parameter is misspelt: the SSID is the rest of
        // the line, a URL and its password with it
        let (lines, diags) = read(&format!(
            "SSID:Home dns-sever=https://al1ce:{MARKER}@dns.example/dns-query"
        ));
        assert_eq!(
            said(&diags),
            [(
                codes::W_PARAM_NOT_EFFECTIVE,
                "a [SSID Setting] line without parameters has no effect"
            )]
        );
        let shown = format!("{:?}\n{:#?}", lines[0], lines);
        assert!(!shown.contains(MARKER), "{shown}");
        assert!(shown.contains("expr: \"***\""), "{shown}");

        for (line, text) in [
            ("SSID:Home suspend=true", "Home"),
            ("BSSID:aa:bb:* suspend=true", "aa:bb:*"),
            ("ROUTER:192.168.1.1 suspend=true", "192.168.1.1"),
            ("TYPE:WIFI suspend=true", "Wifi"),
            ("MCCMNC:310260 suspend=true", "310260"),
            ("Home suspend=true", "Home"),
        ] {
            let (lines, _) = read(line);
            let shown = format!("{:?}", lines[0]);
            assert!(shown.contains(text), "{line}: {shown}");
            assert!(!shown.contains("***"), "{line}: {shown}");
        }
        let (lines, _) = read("SSID:My Home suspend=true");
        let shown = format!("{:?}", lines[0]);
        assert!(!shown.contains("Home"), "{shown}");
        assert!(shown.contains("expr: \"***\""), "{shown}");
    }
}
