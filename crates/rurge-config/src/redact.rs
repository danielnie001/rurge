//! Profile text redaction for `GET /v1/profiles/current?sensitive=0` (M4
//! design §4.3): secrets become `***`, everything else (including line count)
//! is preserved so line numbers in diagnostics still line up.
//!
//! A line is read the way the section parser (`text::parse_str`) reads it —
//! with its own functions, in its order — and what it hands to the line
//! parsers is what gets redacted; what it takes off stays as written. That
//! includes a line it reads as a comment: a commented-out line is where old
//! secrets are left, and it is redacted as if it were not commented out.

use crate::policy::PolicyKind;
use crate::requirement::split_line_slice;
use crate::text::{directive_name, is_comment, is_requirement_prefix, strip_inline_comment};
use crate::value::{parse_key_value, split_list};

/// Keys whose whole value is a secret when they stand alone on a line
/// (`[WireGuard]` `private-key`, `[Snell Server]` `psk`, `[Tailscale]`
/// `auth-key`, …). `[General]`'s two test URLs are among them as a policy's
/// `test-url` is among the inline parameters: a profile that came from a
/// provider can carry a token in them. So is `geoip-maxmind-url`: MaxMind's
/// own download link carries the account's licence key in its query. A key is
/// compared without regard to its case, its `-` and its `_`: wg-quick writes
/// `PrivateKey` and `PresharedKey`, the manual `preshared-key`. (The two
/// spellings of a peer's key are one under that comparison; the list names
/// both so that it reads as the manuals do.)
const SECRET_KEYS: [&str; 12] = [
    "password",
    "ca-passphrase",
    "ca-p12",
    "private-key",
    "psk",
    "pre-shared-key",
    "preshared-key",
    "token",
    "auth-key",
    "proxy-test-url",
    "internet-test-url",
    "geoip-maxmind-url",
];
/// Inline `name=value` parameters redacted wherever they appear in a value.
/// `username` also covers harmless SSH user names; `headers` and `ws-headers`
/// are blanked whole (header names included), and so is `ws-path`, which
/// nodes behind a CDN routinely use as a shared secret. `shadow-tls-password`
/// and Hysteria 2's `salamander-password` / `gecko-password` need entries of
/// their own: `password` only matches at a token boundary. A group's
/// `policy-path` usually carries a subscription token, and its
/// `external-policy-modifier` can set any parameter, a password included. A
/// policy's `test-url` can come from a subscription line and carry its
/// token, and so can the `url` that older profiles write on a group line
/// instead. A `[WireGuard]` peer's `preshared-key` is the manual's spelling;
/// `pre-shared-key` stays for profiles written the other way. An `external`
/// policy's `args` often carry a password (`sshpass -p …`, M4-D12). The
/// camouflage host of `obfs-host` can identify the user (phase 2 M6 design 6),
/// and behind a front that routes by path `obfs-uri` is a shared secret, as
/// `ws-path` is. A `[Script]` line's `argument` is where a script's token or
/// cookie is put. Over-redacting is the safe side for an endpoint whose
/// purpose is safe output.
const SECRET_PARAMS: [&str; 23] = [
    "password",
    "psk",
    "private-key",
    "pre-shared-key",
    "preshared-key",
    "base64",
    "token",
    "uuid",
    "username",
    "headers",
    "ws-headers",
    "ws-path",
    "shadow-tls-password",
    "salamander-password",
    "gecko-password",
    "policy-path",
    "external-policy-modifier",
    "test-url",
    "url",
    "args",
    "argument",
    "obfs-host",
    "obfs-uri",
];
/// Keys whose value is `secret@address`, the whole of it: the parser cuts it
/// at its last `@` and never at a comma, so a key may hold one.
const CONTROLLER_KEYS: [&str; 2] = ["http-api", "external-controller-access"];
/// Keys whose value is a list of `[secret@]address`: what stands before an
/// item's last `@` goes.
const LISTENER_KEYS: [&str; 2] = ["http-listen", "socks5-listen"];
/// Keys that list DNS servers, whose URLs may carry a user name and a
/// password (`https://user:pass@host/…`).
const DNS_KEYS: [&str; 3] = ["dns-server", "encrypted-dns-server", "doh-server"];

pub fn redact_profile(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&redact_line(line));
    }
    out
}

/// Redacts one line, keeping a CRLF file's `\r` where it was: the line is split
/// on `\n` by the caller, so `\r` would otherwise be eaten by the branches that
/// rebuild the line from its parts.
pub(crate) fn redact_line(line: &str) -> String {
    match line.strip_suffix('\r') {
        Some(rest) => format!("{}\r", redact_body(rest)),
        None => redact_body(line),
    }
}

/// What may stand in front of a line without being part of it: white space,
/// a byte-order mark, a zero-width character (a copy from a web page or a
/// chat program brings them along).
fn is_blank(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{feff}' | '\u{200b}'..='\u{200f}' | '\u{2060}')
}

/// The comment marker `line` starts with, as `text::is_comment` reads it.
fn comment_marker(line: &str) -> Option<&str> {
    is_comment(line).then(|| &line[..if line.starts_with("//") { 2 } else { 1 }])
}

/// Where `part`, a slice of `whole`, starts in it.
fn offset_in(whole: &str, part: &str) -> Option<usize> {
    let start = (part.as_ptr() as usize).checked_sub(whole.as_ptr() as usize)?;
    (start + part.len() <= whole.len()).then_some(start)
}

/// How much of the front of `line` is a requirement directive (`#!MACOS-ONLY`,
/// `#!REQUIREMENT <expression>`, white space after it included): what
/// `requirement::split_line` takes off as a prefix, the expression with it —
/// it may hold an `=`, as in `CORE_VERSION>=20`.
fn requirement_prefix(line: &str) -> Option<usize> {
    if !is_requirement_prefix(line) {
        return None;
    }
    let (Some(_), content) = split_line_slice(line).ok()? else {
        return None;
    };
    offset_in(line, content).filter(|len| *len > 0)
}

fn redact_body(line: &str) -> String {
    let mut shown = String::with_capacity(line.len());
    let mut rest = line;
    let mut commented = false;
    // what the section parser takes off the front of a line, in its order:
    // white space, comment markers, a requirement directive. All of it stays
    // as written, and what is left is redacted as if the line were not
    // commented out: a marker may be followed by another, and by a directive.
    loop {
        let body = rest.trim_start_matches(is_blank);
        shown.push_str(&rest[..rest.len() - body.len()]);
        rest = body;
        let kept = if let Some(marker) = comment_marker(rest) {
            commented = true;
            marker.len()
        } else if let Some(prefix) = requirement_prefix(rest) {
            prefix
        } else {
            break;
        };
        shown.push_str(&rest[..kept]);
        rest = &rest[kept..];
    }
    let part = if let Some(name) = commented.then(|| managed_look_alike(rest)).flatten() {
        // `#MANAGED-CONFIG <link>`: the directive with its `!` dropped
        let (name, words) = rest.split_at(name);
        format!("{name}{}", each_word(words, managed_word))
    } else if is_directive_line(rest) {
        redact_directive(rest)
    } else {
        redact_content(rest)
    };
    // a comment is free text: a secret named in it goes too
    shown.push_str(&if commented {
        redact_by_name(&part)
    } else {
        part
    });
    hide_url_credentials(&shown)
}

/// Whether the section parser takes `line` for a directive of its own, whose
/// text it keeps whole: it starts with `#!` and is neither an include nor a
/// requirement directive.
fn is_directive_line(line: &str) -> bool {
    line.starts_with("#!") && !line.starts_with("#!include") && !is_requirement_prefix(line)
}

/// An entry's line: the inline comment and a requirement directive at its end
/// are taken off as the parser takes them off, kept as written but for a
/// secret named in them; what is left is the content the line parsers get.
fn redact_content(line: &str) -> String {
    let content = strip_inline_comment(line);
    let (start, end) = match split_line_slice(content) {
        Ok((_, body)) => match offset_in(content, body) {
            Some(start) => (start, start + body.len()),
            None => (0, content.len()),
        },
        Err(_) => (0, content.len()),
    };
    format!(
        "{}{}{}",
        &line[..start],
        redact_core(&line[start..end]),
        redact_by_name(&line[end..])
    )
}

/// What the line parsers get: a `#!` line by its shape, any other by its key.
fn redact_core(core: &str) -> String {
    if core.starts_with("#!") {
        redact_directive(core)
    } else {
        redact_entry(core)
    }
}

/// A `#!` line is read by its shape and not by what a parser makes of it: a
/// directive spelt a little differently, or one with a link glued to it, is a
/// `#!` line all the same.
fn redact_directive(line: &str) -> String {
    let name = directive_name(line);
    if name == MANAGED_CONFIG {
        return format!("{name}{}", each_word(&line[name.len()..], managed_word));
    }
    // a directive that is none of the known ones may still stand in front of
    // a line of the profile, which is redacted as any other; a link is
    // hidden wherever it is left
    let shown = redact_entry(line);
    match directive_name(&shown) {
        "" => shown,
        name => {
            let words = &shown[name.len()..];
            let words = if name == "#!include" {
                hide_include_links(words)
            } else {
                each_word(words, link_hidden)
            };
            format!("{name}{words}")
        }
    }
}

/// A key without its case, its `-` and its `_`.
fn plain_key(key: &str) -> impl Iterator<Item = u8> + '_ {
    key.bytes()
        .filter(|b| !matches!(b, b'-' | b'_'))
        .map(|b| b.to_ascii_lowercase())
}

/// Whether `key` is one of `list`, as the parsers compare a key and a little
/// more loosely: the case, the `-` and the `_` do not count, nor does a blank
/// around it.
fn is_key(list: &[&str], key: &str) -> bool {
    let key: Vec<u8> = plain_key(key.trim_matches(is_blank)).collect();
    list.iter()
        .any(|name| plain_key(name).eq(key.iter().copied()))
}

/// `key = value` lines: the keys that are secrets, the lists of addresses
/// that may hold a user name and a password, and the definitions of policies.
/// The key ends at the first `=` — or `＝`, which an input method types; a
/// text in front of it that holds a comma is no key, and neither is anything
/// when there is no `=`: the line is then a definition. A key that is a
/// secret has `:` — or `：` — too for a separator.
fn redact_entry(line: &str) -> String {
    // what stands before a `:` that follows an `=` holds that `=`, and is no key
    if let Some(colon) = line.find([':', '：'])
        && is_key(&SECRET_KEYS, &line[..colon])
    {
        let (key, rest) = line.split_at(colon);
        let sep = rest.chars().next().unwrap_or(':');
        return format!("{key}{sep} ***");
    }
    let Some(at) = line.find(['=', '＝']) else {
        return redact_definition(line);
    };
    let key = &line[..at];
    // a comma — or a `，`, which an input method types for one — in front of
    // the `=`: the text is no key
    if next_comma(key, true).is_some() {
        return redact_both_ways(line);
    }
    let width = line[at..].chars().next().map_or(1, char::len_utf8);
    let (sep, value) = line[at..].split_at(width);
    if is_key(&SECRET_KEYS, key) {
        return format!("{key}{sep} ***");
    }
    if is_key(&["wifi-access-http-auth"], key) {
        let v = value.trim_start();
        let lead = &value[..value.len() - v.len()];
        return match v.split_once(':') {
            Some((user, _)) => format!("{key}{sep}{lead}{user}:***"),
            None => format!("{key}{sep}{lead}***"),
        };
    }
    if is_key(&CONTROLLER_KEYS, key) {
        return format!("{key}{sep}{}", hide_controller(value));
    }
    if is_key(&LISTENER_KEYS, key) {
        return format!("{key}{sep}{}", hide_listed(value, without_credentials));
    }
    if is_key(&DNS_KEYS, key) {
        return format!("{key}{sep}{}", hide_listed(value, url_without_credentials));
    }
    // policy lines: `name = type, host, port, password=..., psk=...`, or
    // `name = http/https/socks5/socks5-tls, host, port, username, password`
    // (those types carry credentials positionally; on every other type that
    // takes a server and a port a positional value is blanked all the same).
    format!("{key}{sep}{}", redact_definition(value))
}

/// A line with a comma in front of its first `=`. It is a definition that lost
/// the `=` after its name (`P http, host, 80, user, password, tfo=true`), or
/// the name holds a comma and the definition follows its `=`: it is redacted
/// as the one and as the other.
fn redact_both_ways(line: &str) -> String {
    let whole = redact_definition(line);
    let Some(at) = whole.find(['=', '＝']) else {
        return whole;
    };
    let width = whole[at..].chars().next().map_or(1, char::len_utf8);
    let (head, tail) = whole.split_at(at + width);
    format!("{head}{}", redact_definition(tail))
}

/// The value of an API key: `key@address`, cut at its last `@` and nowhere
/// else — a comma is part of the key. Without an `@` there is no telling the
/// key from the address, and the whole of it goes.
fn hide_controller(value: &str) -> String {
    let body = value.trim_start();
    let lead = &value[..value.len() - body.len()];
    let text = body.trim_end();
    let trail = &body[text.len()..];
    let shown = if last_at(text).is_some() {
        without_credentials(text)
    } else {
        "***".to_string()
    };
    format!("{lead}{shown}{trail}")
}

/// A comma-separated `value` with each of its items shown as `shown` makes
/// of it. The items are the parsers' own: quotes and parentheses group.
fn hide_listed(value: &str, shown: fn(&str) -> String) -> String {
    split_top_level(value)
        .into_iter()
        .map(|item| {
            let t = item.trim_start();
            let lead = &item[..item.len() - t.len()];
            format!("{lead}{}", shown(t))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// The directive of a profile a provider issues, whose line carries the
/// subscription's link, and a token with it.
const MANAGED_CONFIG: &str = "#!MANAGED-CONFIG";

/// How long the name is, at the start of a commented-out line, when it reads
/// as `MANAGED-CONFIG` spelt however a comment left it: the `!` dropped
/// (`#MANAGED-CONFIG <link>`), the case, the `-` or the `_` not as the
/// directive has them.
fn managed_look_alike(line: &str) -> Option<usize> {
    const NAME: &[u8] = b"managedconfig";
    let bytes = line.as_bytes();
    let mut at = usize::from(bytes.first() == Some(&b'!'));
    let mut matched = 0;
    while matched < NAME.len() {
        match *bytes.get(at)? {
            b'-' | b'_' => {}
            b if b.to_ascii_lowercase() == NAME[matched] => matched += 1,
            _ => return None,
        }
        at += 1;
    }
    Some(at)
}

/// `text` with each of its white-space-separated words replaced by what
/// `shown` makes of it. The white space between them stays as it was: it can
/// be a tab, a no-break space or a `\r`.
fn each_word(text: &str, shown: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let space = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..space]);
        rest = &rest[space..];
        let word = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if word > 0 {
            out.push_str(&shown(&rest[..word]));
        }
        rest = &rest[word..];
    }
    out
}

/// Of the words that follow `#!MANAGED-CONFIG` only the two parameters the
/// format has, written plainly, are shown: the link stands wherever the
/// profile has it, and a word that is none of those can be a token.
fn managed_word(word: &str) -> String {
    let plain = match word.split_once('=') {
        Some(("interval", value)) => !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
        Some(("strict", value)) => {
            !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphabetic())
        }
        _ => false,
    };
    if plain { word } else { "***" }.to_string()
}

/// On any other `#!` line a word with `://` in it is a link.
fn link_hidden(word: &str) -> String {
    if word.contains("://") { "***" } else { word }.to_string()
}

/// The targets of `#!include` are a list separated by commas, and a path with
/// a space in it can be quoted: a target with `://` in it is a link, a local
/// path stays.
fn hide_include_links(targets: &str) -> String {
    split_top_level(targets)
        .into_iter()
        .map(|target| {
            if target.contains("://") {
                let rest = target.trim_start();
                let lead = &target[..target.len() - rest.len()];
                let trail = &rest[rest.trim_end().len()..];
                format!("{lead}***{trail}")
            } else {
                target.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Where the credential of `item` ends: its last `@`, as the parsers split
/// there — a key may hold one itself. The fullwidth `＠` counts too: what
/// resolves a name may read it as `@`.
fn last_at(item: &str) -> Option<(usize, usize)> {
    item.char_indices()
        .rfind(|(_, c)| matches!(c, '@' | '＠'))
        .map(|(at, sign)| (at, at + sign.len_utf8()))
}

/// `item` — `[secret@]address` — without what stands before its last `@`,
/// an API key or a listener's password: all a diagnostic or the redacted
/// profile may show of it.
pub fn without_credentials(item: &str) -> String {
    match last_at(item) {
        Some((_, rest)) => format!("***@{}", &item[rest..]),
        None => item.to_string(),
    }
}

/// A URL without the user name and the password it may carry: its scheme
/// — what can be one, a mistyped one too — stays in front of what follows
/// the last `@`.
pub fn url_without_credentials(url: &str) -> String {
    let Some((at, rest)) = last_at(url) else {
        return url.to_string();
    };
    let scheme = url[..at]
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .filter(|scheme| {
            scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        });
    match scheme {
        Some(scheme) => format!("{scheme}://***@{}", &url[rest..]),
        None => format!("***@{}", &url[rest..]),
    }
}

/// `text` with the user name and the password of every URL in it gone: what
/// stands before the last `@` of the authority that follows a `://` — which
/// ends at a `/`, a `?`, a `#`, a comma, a quote, a `)`, white space or the
/// end. An `@` further on is part of a path or a query and stays. A `[Host]`
/// server, a rule set or a script fetched with credentials, a share link
/// pasted as a line: wherever the URL stands.
fn hide_url_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, after) = rest.split_at(at + 3);
        out.push_str(head);
        let end = after
            .find(|c: char| {
                matches!(c, '/' | '?' | '#' | ',' | '"' | '\'' | ')') || c.is_whitespace()
            })
            .unwrap_or(after.len());
        let (authority, tail) = after.split_at(end);
        match last_at(authority) {
            Some((_, host)) => {
                out.push_str("***@");
                out.push_str(&authority[host..]);
            }
            None => out.push_str(authority),
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// A policy definition (the text right of `name =`) with its secrets blanked:
/// the positional values of every type that takes a server and a port, and
/// every secret `name=value` parameter, however the line spells it.
pub fn redact_definition(definition: &str) -> String {
    hide_url_credentials(&redact_items(&redact_positional_credentials(definition), 0))
}

/// How many parenthesised groups deep `redact_items` follows. A `[WireGuard]`
/// peer is one and no parser reads a group inside a group as anything but a
/// value; deeper than this the search by name still runs, and a line of
/// ten thousand parentheses costs a few levels, not a stack.
const MAX_GROUP_DEPTH: usize = 4;

/// `text`, a list of items, with every item redacted. The items are the
/// parsers' own (`split_top_level` is `split_list`'s state machine) and the
/// commas between them stay.
fn redact_items(text: &str, depth: usize) -> String {
    split_top_level(text)
        .into_iter()
        .map(|item| redact_item(item, depth))
        .collect::<Vec<_>>()
        .join(",")
}

/// One item, the way the parsers read it: `split_list` takes the quotes off
/// an item that is wholly in them, `parse_key_value` splits what is left at
/// its first `=`, and both trim whatever `char::is_whitespace` knows.
///
/// - A parameter whose name is a secret one loses its value. Written plainly
///   it keeps its own spacing and name; from inside quotes only the name
///   is left, `name=***`.
/// - A parenthesised group, which is how a `[WireGuard]` peer is written
///   (`wireguard.rs` takes the parentheses off and splits what is inside),
///   has its items redacted the same way.
/// - Anything else goes through the search by name, which finds a secret
///   wherever one stands in the text: what the parsers read as one value (an
///   item with a quote left open, say) can hold a secret parameter of its own.
fn redact_item(item: &str, depth: usize) -> String {
    let rest = item.trim_start();
    let lead = &item[..item.len() - rest.len()];
    let body = rest.trim_end();
    let trail = &rest[body.len()..];
    let Some(field) = split_list(item).pop() else {
        // nothing in it: an empty item, or one made of nothing but quotes
        return item.to_string();
    };
    if depth < MAX_GROUP_DEPTH {
        let inside = group_inside(body).or_else(|| group_inside(&field));
        if let Some(inside) = inside {
            let redacted = redact_items(inside, depth + 1);
            // a group that is wholly in quotes loses them only for a secret
            return if redacted == inside {
                item.to_string()
            } else {
                format!("{lead}({redacted}){trail}")
            };
        }
    }
    let secret = parse_key_value(&field).map(|(key, _)| key).filter(|key| {
        SECRET_PARAMS
            .iter()
            .any(|param| key.eq_ignore_ascii_case(param))
    });
    match (secret, body.split_once('=')) {
        (Some(key), Some((name, value))) if name.trim() == key => {
            let blank = &value[..value.len() - value.trim_start().len()];
            format!("{lead}{name}={blank}***{trail}")
        }
        (Some(key), _) => format!("{lead}{key}=***{trail}"),
        (None, _) => redact_by_name(item),
    }
}

/// What stands between the parentheses of a group, as `wireguard.rs` takes
/// a peer apart.
fn group_inside(item: &str) -> Option<&str> {
    item.trim().strip_prefix('(')?.strip_suffix(')')
}

/// Whether `name` can be the name of a parameter: ASCII letters, digits, `-`
/// and `_`, and not empty.
fn is_param_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Blanks the positional tokens of a policy line: every field from the fourth
/// on (the first three are the type, the server and the port) — that is where
/// Surge puts `username, password`. It is done for the types that take a
/// server and a port, and for any line whose third field is a port number: a
/// line written for another client (Loon's `Shadowsocks, host, 443, method,
/// "password"`, Surge's old `custom, host, 443, method, password, module`) is
/// an unknown type to rurge, and its password stands in the fifth place all
/// the same. For a type that takes a server, what stands in the port's place
/// and is no number is a user name or a password that lost its comma (`8080
/// alice, hunter2`), and goes too. The fields are the parser's: `split_list`
/// reads the type and the port with the quotes off, and an empty item is
/// none. A token counts as
/// a named parameter (and is kept) only when what stands before its first `=`
/// is a name and what follows it is non-empty and not made only of `=`, so
/// base64 padding (`aHVudGVyMg==`) is redacted while `tfo=true` survives, and
/// so is a password with a missing comma in front of a parameter (`hunter2
/// tfo=true`); `sni=` with an empty value is over-redacted. Any other line is
/// returned unchanged.
fn redact_positional_credentials(value: &str) -> String {
    // a line typed with an input method may have `，` for some of its commas:
    // it is read here as the author meant it, and the separators stay
    let mut tokens: Vec<(&str, &str)> = Vec::new();
    let mut rest = value;
    while let Some((at, width)) = next_comma(rest, true) {
        tokens.push((&rest[..at], &rest[at..at + width]));
        rest = &rest[at + width..];
    }
    tokens.push((rest, ""));
    // the fields the parser reads, an empty item being none; only as many as
    // the decision needs are looked at
    let mut read = tokens
        .iter()
        .filter_map(|(token, _)| split_list(token).pop());
    let takes_server = read
        .next()
        .and_then(|keyword| PolicyKind::parse(keyword.trim_matches(is_blank)))
        .is_some_and(|kind| kind.takes_server());
    let port_third = read
        .nth(1)
        .is_some_and(|field| field.trim_matches(is_blank).parse::<u16>().is_ok());
    if !takes_server && !port_third {
        return value.to_string();
    }
    let mut index = 0usize;
    let mut out = String::with_capacity(value.len());
    for (tok, sep) in tokens {
        if let Some(field) = split_list(tok).pop() {
            index += 1;
            let t = tok.trim_start();
            let stray_port =
                index == 3 && takes_server && field.trim_matches(is_blank).parse::<u16>().is_err();
            if (index > 3 || stray_port) && !is_named_param(t.trim_end()) {
                out.push_str(&tok[..tok.len() - t.len()]);
                out.push_str("***");
            } else {
                out.push_str(tok);
            }
        } else {
            out.push_str(tok);
        }
        out.push_str(sep);
    }
    out
}

/// The first comma that is inside no quote and no parenthesised group, by the
/// parser's own state machine (`value::split_list`): a `"` or a `'` opens a
/// quote **wherever** it appears, inside `"` a backslash escapes the next
/// character, inside `'` nothing escapes, and `(` / `)` nest outside quotes.
/// `None` when there is none — an unterminated quote or group therefore runs
/// to the end of the line, which is the safe side for redaction.
///
/// Everything that has to agree with the parser about where a value ends is
/// built on this one function, so the two cannot drift apart.
fn next_top_level_comma(value: &str) -> Option<usize> {
    next_comma(value, false).map(|(at, _)| at)
}

/// `next_top_level_comma`, with where the comma ends too and, when `wide`, the
/// full-width comma `，` an input method types counted as one.
fn next_comma(value: &str, wide: bool) -> Option<(usize, usize)> {
    let bytes = value.as_bytes();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == b'\\' && q == b'"' {
                    i += 1; // whatever follows is escaped, quote included
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'"' | b'\'' => quote = Some(c),
                b'(' => depth += 1,
                b')' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => return Some((i, 1)),
                // `，` is the bytes EF BC 8C, and 0xEF starts a character
                0xEF if wide && depth == 0 && value[i..].starts_with('，') => {
                    return Some((i, '，'.len_utf8()));
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// Splits at top-level commas only, returning the raw slices so that quoting
/// and spacing survive. A top-level comma leaves the scanner with no open
/// quote and depth 0, so each item is scanned afresh.
pub(crate) fn split_top_level(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(i) = next_top_level_comma(rest) {
        out.push(&rest[..i]);
        rest = &rest[i + 1..];
    }
    out.push(rest);
    out
}

/// `name=value` with a name before the `=` and a value that is neither empty
/// nor pure `=` padding. A token that opens with a quote is one quoted value,
/// `=` and all.
fn is_named_param(token: &str) -> bool {
    if token.starts_with('"') || token.starts_with('\'') {
        return false;
    }
    token.split_once('=').is_some_and(|(name, after)| {
        is_param_name(name.trim()) && !after.is_empty() && !after.chars().all(|c| c == '=')
    })
}

/// How long a parameter's value is: up to the first comma the parser would
/// split on — quotes may open anywhere inside the value and parentheses group,
/// so `ab"c,d"` and `a(b,c)d` are each one value. When the scan starts inside
/// a group (a secret nested in a `peer = (…)`), depth starts at 0 there, so the
/// value ends at the group's own next comma.
fn param_value_len(value: &str) -> usize {
    next_top_level_comma(value).unwrap_or(value.len())
}

#[cfg(test)]
thread_local! {
    // how many times `redact_param` searched, for a test to tell that the
    // look before the searches saves them
    static SEARCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `text` with the value of every secret parameter it names blanked, wherever
/// the name stands: a secret has to be found in what a comment says, in what
/// the parsers read as one value, in a group nested too deep to follow.
fn redact_by_name(text: &str) -> String {
    // one look first: a name can only be a secret one's when it stands right
    // before an `=` (white space between the two does not count), and most
    // items, comments and rules have none that does
    let lower = text.to_ascii_lowercase();
    // `＝` starts with the byte 0xEF, which only starts a character
    let names_one = lower.bytes().enumerate().any(|(at, b)| {
        (b == b'=' || (b == 0xEF && lower[at..].starts_with('＝')))
            && SECRET_PARAMS
                .iter()
                .any(|param| lower[..at].trim_end().ends_with(param))
    });
    if !names_one {
        return text.to_string();
    }
    SECRET_PARAMS
        .iter()
        .fold(text.to_string(), |text, param| redact_param(&text, param))
}

/// Replaces the value of every `<param> = <value>` occurrence with `***`
/// (`param_value_len` says where the value ends), keeping the surrounding
/// spacing. Substring-based on purpose: a secret also has to be found nested
/// inside a parenthesised group (a `[WireGuard]` `peer = (…, pre-shared-key =
/// …)`), where no top-level split would reach it.
fn redact_param(value: &str, param: &str) -> String {
    #[cfg(test)]
    SEARCHES.with(|searches| searches.set(searches.get() + 1));
    let lower = value.to_ascii_lowercase();
    let mut out = String::with_capacity(value.len());
    let mut rest = 0usize;
    let mut search = 0usize;
    while let Some(pos) = lower[search..].find(param) {
        let start = search + pos;
        // must be at a token boundary followed by optional spaces and '=' (or
        // `＝`, which an input method types). A name starts after white space,
        // a comma, a `(`, a quote, or any character that is not ASCII — a
        // full-width comma, a zero-width space or a byte-order mark brought
        // along from a web page, a typographic quote. A letter, a digit or the
        // punctuation of a name or a URL in front makes it part of another
        // word (`my-args=`, `exec=/bin/args=x`, `?a=1&url=…`).
        let before = lower[..start].chars().next_back();
        let before_ok = before.is_none_or(|c| {
            c.is_ascii_whitespace() || matches!(c, ',' | '(' | '"' | '\'') || !c.is_ascii()
        });
        let after = &value[start + param.len()..];
        let eq = after.trim_start().strip_prefix(['=', '＝']);
        match (before_ok, eq) {
            (true, Some(tail)) => {
                let tail_start = value.len() - tail.len();
                let tail_lead = &tail[..tail.len() - tail.trim_start().len()];
                let tail_value = tail.trim_start();
                // behind a quote the value runs to the end of the text: it is
                // a quote the item pass could not read as a whole, one left
                // open takes the rest of the line with it
                let val_len = if matches!(before, Some('"' | '\'')) {
                    tail_value.len()
                } else {
                    param_value_len(tail_value)
                };
                let blank = &tail_value[..val_len];
                out.push_str(&value[rest..tail_start]);
                out.push_str(tail_lead);
                out.push_str("***");
                // the white space that ends the value is not part of the secret
                out.push_str(&blank[blank.trim_end().len()..]);
                rest = tail_start + tail_lead.len() + val_len;
                search = rest;
            }
            _ => search = start + param.len(),
        }
    }
    out.push_str(&value[rest..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets_but_keeps_structure() {
        let text = "[General]\nhttp-api = s3cret@127.0.0.1:6171\nhttp-listen = pw@127.0.0.1:6152, 127.0.0.1:6153\nwifi-access-http-auth = alice:hunter2\n[Proxy]\nHK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x1, udp-relay=true\nTJ = trojan, h, 443, password = y2\nProxyA = https, 1.2.3.4, 443, carol, trustno1\nS = socks5, 1.2.3.4, 1080\n[Keystore]\nkey1 = type=openssh-private-key, base64=BBBB\n[MITM]\nca-passphrase = abc\nca-p12 = MIIK...\n[Rule]\nFINAL,DIRECT\n";
        let out = redact_profile(text);
        assert!(out.contains("http-api = ***@127.0.0.1:6171"), "{out}");
        assert!(
            out.contains("http-listen = ***@127.0.0.1:6152, 127.0.0.1:6153"),
            "{out}"
        );
        assert!(out.contains("wifi-access-http-auth = alice:***"), "{out}");
        assert!(out.contains("password=***, udp-relay=true"), "{out}");
        assert!(out.contains("password = ***"), "{out}");
        assert!(
            out.contains("ca-passphrase = ***") && out.contains("ca-p12 = ***"),
            "{out}"
        );
        // positional credentials (http/https/socks5/socks5-tls) are blanked too
        assert!(
            out.contains("ProxyA = https, 1.2.3.4, 443, ***, ***"),
            "{out}"
        );
        // a socks5 line with no trailing credentials is untouched
        assert!(out.contains("S = socks5, 1.2.3.4, 1080"), "{out}");
        // [Keystore] payloads (`base64=...`) are redacted like other secrets
        assert!(out.contains("base64=***"), "{out}");
        assert!(
            !out.contains("s3cret")
                && !out.contains("hunter2")
                && !out.contains("x1")
                && !out.contains("y2")
                && !out.contains("carol")
                && !out.contains("trustno1")
                && !out.contains("BBBB")
                && !out.contains("MIIK")
        );
        assert_eq!(out.lines().count(), text.lines().count());
        assert!(out.contains("FINAL,DIRECT"));
    }

    /// What stands before the last `@` goes, a key that holds an `@` of its
    /// own with it, and a URL keeps its scheme; a list of DNS servers is
    /// redacted item by item.
    #[test]
    fn what_stands_before_the_last_at_sign_goes() {
        for (item, shown) in [
            ("k3y@127.0.0.1:6171", "***@127.0.0.1:6171"),
            ("pa@ss@127.0.0.1:6171", "***@127.0.0.1:6171"),
            // a key is no URL, whatever it holds
            ("pa://ss@127.0.0.1:6171", "***@127.0.0.1:6171"),
            ("k3y＠127.0.0.1:6171", "***@127.0.0.1:6171"),
            ("127.0.0.1:6152", "127.0.0.1:6152"),
        ] {
            assert_eq!(without_credentials(item), shown, "{item}");
        }
        for (url, shown) in [
            (
                "https://al1ce:pw@dns.example/dns-query",
                "https://***@dns.example/dns-query",
            ),
            ("htps://al1ce:p://w@dns.example", "htps://***@dns.example"),
            ("quic://al1ce:pw＠dns.example", "quic://***@dns.example"),
            // no scheme in front: none of what stands there is one
            ("al1ce:p://w@1.2.3.4", "***@1.2.3.4"),
            (
                "https://dns.example/dns-query",
                "https://dns.example/dns-query",
            ),
            ("8.8.8.8", "8.8.8.8"),
        ] {
            assert_eq!(url_without_credentials(url), shown, "{url}");
        }
        let out = redact_profile(
            "[General]
http-api = pa@ss@127.0.0.1:6171
encrypted-dns-server = https://al1ce:pw@dns.example/dns-query, quic://dns.example
dns-server = 8.8.8.8, h3://b0b:pw@dns.example
[Tailscale home]
auth-key = tskey-auth-abc
",
        );
        assert!(out.contains("http-api = ***@127.0.0.1:6171"), "{out}");
        assert!(
            out.contains(
                "encrypted-dns-server = https://***@dns.example/dns-query, quic://dns.example"
            ),
            "{out}"
        );
        assert!(
            out.contains("dns-server = 8.8.8.8, h3://***@dns.example"),
            "{out}"
        );
        assert!(out.contains("auth-key = ***"), "{out}");
    }

    #[test]
    fn redacts_wireguard_snell_vmess_tuic_and_padded_credentials() {
        let text = "[WireGuard wg1]\nprivate-key = WGKEY\nself-ip = 10.0.0.2\n\
peer = (public-key = PUB, pre-shared-key = PSK1, endpoint = 1.2.3.4:51820)\n\
[Snell Server]\npsk = SNELLPSK\n\
[Proxy]\nV = vmess, h, 443, username=11111111-2222-3333-4444-555555555555, tls=true\n\
T = tuic, h, 443, token=TUICTOK, uuid=abcd-ef\n\
P = https, h, 443, bob, aHVudGVyMg==, tfo=true\n";
        let out = redact_profile(text);
        for secret in [
            "WGKEY",
            "PSK1",
            "SNELLPSK",
            "11111111-2222-3333-4444-555555555555",
            "TUICTOK",
            "abcd-ef",
            "bob",
            "aHVudGVyMg==",
        ] {
            assert!(!out.contains(secret), "{secret} leaked:\n{out}");
        }
        // non-secrets survive
        assert!(out.contains("PUB"), "{out}");
        assert!(out.contains("1.2.3.4:51820"), "{out}");
        assert!(
            out.contains("tls=true") && out.contains("tfo=true"),
            "{out}"
        );
        assert!(out.contains("self-ip = 10.0.0.2"), "{out}");
        assert_eq!(out.lines().count(), text.lines().count());
    }

    #[test]
    fn crlf_line_endings_survive_redaction() {
        let out = redact_profile("a = b\r\npassword = x\r\n");
        assert_eq!(out, "a = b\r\npassword = ***\r\n");
    }

    #[test]
    fn a_definition_is_redacted_like_its_profile_line() {
        for def in [
            "http, proxy.test, 8080, alice, s3cret, skip-cert-verify=true",
            "trojan, t.test, 443, password=\"pw0rd,x\", ws-path=\"/s3cretpath\"",
            "trojan, h, 443, password=ab\"c,d\", ws=true",
            "socks5, proxy.test, 1080, username=bob, password=hunter2",
            "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x",
            "direct, interface=eth0",
            "http, h.test, 80, headers=X-Auth:tok3n;X-B:1",
            "trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=Host:edge.test|X-Key:k3y",
        ] {
            let alone = redact_definition(def);
            // one rule, two entry points: the profile endpoint must agree
            assert_eq!(
                format!("X = {alone}"),
                redact_profile(&format!("X = {def}")),
                "{def}"
            );
            for secret in [
                "s3cret",
                "hunter2",
                "alice",
                "bob",
                "tok3n",
                "pw0rd",
                "s3cretpath",
                "k3y",
                "edge.test",
            ] {
                assert!(!alone.contains(secret), "{def} -> {alone}");
            }
        }
        // `headers=` carries credentials: the whole value goes, names included
        assert_eq!(
            redact_definition("http, h.test, 80, headers=X-Auth:tok3n;X-B:1"),
            "http, h.test, 80, headers=***"
        );
        assert!(redact_definition("http, h, 1, u, p").contains("***"));
        assert_eq!(
            redact_definition("direct, interface=eth0"),
            "direct, interface=eth0"
        );
        assert_eq!(
            redact_definition(
                "trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=Host:edge.test|X-Key:k3y"
            ),
            "trojan, t.test, 443, password=***, ws=true, ws-path=***, ws-headers=***"
        );
        // not covered by `password`: that one only matches at a token boundary
        assert_eq!(
            redact_definition(
                "snell, 1.2.3.4, 443, psk=pwd1, shadow-tls-password=pwd2, shadow-tls-sni=example.com"
            ),
            "snell, 1.2.3.4, 443, psk=***, shadow-tls-password=***, shadow-tls-sni=example.com"
        );
    }

    /// A value that contains a comma has to be quoted (`value::split_list`
    /// honours quotes), so redaction must end a value where the parser does —
    /// otherwise the tail of a password is served in the clear.
    #[test]
    fn a_quoted_value_is_blanked_whole_however_it_is_quoted() {
        for (def, expected) in [
            (
                "trojan, t.test, 443, password=\"p,w\", ws=true",
                "trojan, t.test, 443, password=***, ws=true",
            ),
            (
                "trojan, t.test, 443, password='p,w', ws=true",
                "trojan, t.test, 443, password=***, ws=true",
            ),
            // a `\"` inside a double-quoted value does not end it
            (
                "trojan, t.test, 443, password=\"a\\\",b\", ws=true",
                "trojan, t.test, 443, password=***, ws=true",
            ),
            // an unterminated quote takes the rest of the line: the safe side
            (
                "trojan, t.test, 443, password=\"p,w",
                "trojan, t.test, 443, password=***",
            ),
            (
                "trojan, t.test, 443, password=pw, ws-path=\"/a,b\", ws-headers=\"X-K:v,1|X-B:2\"",
                "trojan, t.test, 443, password=***, ws-path=***, ws-headers=***",
            ),
        ] {
            assert_eq!(redact_definition(def), expected, "{def}");
        }
    }

    /// Positional values belong to no type beyond the four Surge documents,
    /// but a stray one (`W0001`) on any other type that takes `server, port`
    /// is a credential often enough that this module's bias applies.
    #[test]
    fn a_positional_value_is_blanked_on_every_type_that_takes_a_server_and_a_port() {
        for (def, expected) in [
            // one quoted value, its comma and its `=` included
            ("http, h, 80, user, \"a,b=c\"", "http, h, 80, ***, ***"),
            (
                "trojan, h, 443, hunter2, password=real",
                "trojan, h, 443, ***, password=***",
            ),
            (
                "vmess, h, 443, hunter2, username=u",
                "vmess, h, 443, ***, username=***",
            ),
            (
                "anytls, h, 443, hunter2, password=real",
                "anytls, h, 443, ***, password=***",
            ),
            // not a policy line with a server and a port: untouched
            ("select, A, B, C, D", "select, A, B, C, D"),
            ("direct, interface=eth0", "direct, interface=eth0"),
        ] {
            assert_eq!(redact_definition(def), expected, "{def}");
        }
        // nor is a [General] list of values
        let dns = "dns-server = 1.1.1.1, 8.8.8.8, 9.9.9.9, 4.4.4.4";
        assert_eq!(redact_profile(dns), dns);
    }

    /// The parser opens a quote wherever one appears in a value and groups by
    /// parentheses, so the value's first byte says nothing about where it
    /// ends: only a scan by the parser's own rules does.
    #[test]
    fn a_value_ends_at_the_first_top_level_comma_wherever_its_quotes_open() {
        for (def, expected) in [
            (
                "trojan, h, 443, password=ab\"c,d\", ws=true",
                "trojan, h, 443, password=***, ws=true",
            ),
            (
                "trojan, h, 443, password=ab'c,d', ws=true",
                "trojan, h, 443, password=***, ws=true",
            ),
            (
                "trojan, h, 443, password=a(b,c)d, ws=true",
                "trojan, h, 443, password=***, ws=true",
            ),
        ] {
            assert_eq!(redact_definition(def), expected, "{def}");
        }
    }

    /// A secret nested in a parenthesised group is found whether or not a
    /// space follows the `(`.
    #[test]
    fn a_secret_right_after_an_opening_parenthesis_is_found() {
        assert_eq!(
            redact_profile("peer = (pre-shared-key = PSK1, endpoint = 1.2.3.4:51820)"),
            "peer = (pre-shared-key = ***, endpoint = 1.2.3.4:51820)"
        );
    }

    /// The manual spells a peer's key `preshared-key`.
    #[test]
    fn a_wireguard_peers_preshared_key_is_redacted() {
        assert_eq!(
            redact_profile(
                "peer = (public-key = PUB, preshared-key = PSK1, endpoint = 1.2.3.4:51820)"
            ),
            "peer = (public-key = PUB, preshared-key = ***, endpoint = 1.2.3.4:51820)"
        );
    }

    /// A subscription URL usually carries a token, and a modifier can set any
    /// parameter of the imported lines, a password included (M3-D7).
    #[test]
    fn a_group_line_loses_its_subscription_and_its_modifier() {
        assert_eq!(
            redact_definition(
                "select, A, policy-path=https://sub.test/nodes?token=t0k3n, update-interval=3600, external-policy-modifier=\"password=hunter2,tfo=true\", policy-regex-filter=^HK"
            ),
            "select, A, policy-path=***, update-interval=3600, external-policy-modifier=***, policy-regex-filter=^HK"
        );
        // a local file goes all the same: the safe side
        assert_eq!(
            redact_profile("G = select, policy-path=nodes.txt"),
            "G = select, policy-path=***"
        );
    }

    /// A policy's `test-url` can come from a subscription line and carry its
    /// token; so can `[General]`'s `proxy-test-url` / `internet-test-url`,
    /// in a profile that came from a provider. MaxMind's download link,
    /// `geoip-maxmind-url`, carries the account's licence key.
    #[test]
    fn a_policy_line_and_the_profile_lose_their_test_urls() {
        assert_eq!(
            redact_definition("http, 127.0.0.1, 80, test-url=http://t.test/?token=t0k3n, tfo=true"),
            "http, 127.0.0.1, 80, test-url=***, tfo=true"
        );
        assert_eq!(
            redact_profile(
                "[General]\nproxy-test-url = http://p.test/?token=t0k3n\n\
                 Internet-Test-URL=http://i.test/\ntest-timeout = 5\n\
                 geoip-maxmind-url = https://download.maxmind.test/geoip?license_key=t0k3n&suffix=tar.gz\n"
            ),
            "[General]\nproxy-test-url = ***\nInternet-Test-URL= ***\ntest-timeout = 5\n\
             geoip-maxmind-url = ***\n"
        );
    }

    /// Older profiles write a group's test URL as `url=`: the same kind of
    /// value as `test-url`. The name counts at a token boundary only, so
    /// whatever else holds `url` stays as it was.
    #[test]
    fn a_groups_obsolete_url_is_hidden_and_what_only_holds_url_is_not() {
        assert_eq!(
            redact_profile("G = url-test, A, B, url=http://t.test/?token=t0k3n, interval=600"),
            "G = url-test, A, B, url=***, interval=600"
        );
        assert_eq!(
            redact_profile("G = fallback, A, url = http://t.test/?token=t0k3n"),
            "G = fallback, A, url = ***"
        );
        // `test-url` has an entry of its own, and `[General]`'s test URL a key
        assert_eq!(
            redact_profile("G = select, A, test-url=http://t.test/?token=t0k3n"),
            "G = select, A, test-url=***"
        );
        assert_eq!(
            redact_profile("proxy-test-url = http://t.test/"),
            "proxy-test-url = ***"
        );
        // neither a name that ends in `url` nor a `url=` inside a value is the
        // parameter
        for line in [
            "G = select, A, icon-url=https://example.com/icon.png",
            "G = select, A, icon-url=https://example.com/i.png?a=1&url=https://example.com/",
            "cron = type=cron, cronexp=0 0 * * *, script-path=https://example.com/s.js?url=https://example.com/",
        ] {
            assert_eq!(redact_profile(line), line);
        }
    }

    /// A profile issued by a provider starts with `#!MANAGED-CONFIG <link> …`,
    /// and the link carries the subscription token: the directive stays, and
    /// of the words after it only `interval=` with digits and `strict=` with
    /// letters; the link wherever it stands, a second one, a stray `token=`
    /// go, with whatever else is there.
    #[test]
    fn a_managed_config_line_shows_nothing_but_its_interval_and_strictness() {
        for (line, shown) in [
            (
                "#!MANAGED-CONFIG https://provider.test/link/t0k3n?surge=4 interval=43200 strict=true",
                "#!MANAGED-CONFIG *** interval=43200 strict=true",
            ),
            (
                "#!MANAGED-CONFIG https://provider.test/t0k3n",
                "#!MANAGED-CONFIG ***",
            ),
            // the spacing as written, and a CRLF file's `\r`
            (
                "  #!MANAGED-CONFIG\thttps://provider.test/t0k3n  strict=true\r",
                "  #!MANAGED-CONFIG\t***  strict=true\r",
            ),
            (
                "#!MANAGED-CONFIG\u{a0}https://provider.test/t0k3n\u{a0}strict=true\r",
                "#!MANAGED-CONFIG\u{a0}***\u{a0}strict=true\r",
            ),
            (
                "#!MANAGED-CONFIG https://provider.test/t0k3n\r\r",
                "#!MANAGED-CONFIG ***\r\r",
            ),
            // a file saved with a byte-order mark, as Notepad does
            (
                "\u{feff}#!MANAGED-CONFIG https://provider.test/t0k3n?surge=4 interval=60",
                "\u{feff}#!MANAGED-CONFIG *** interval=60",
            ),
            (
                "\u{feff}  #!MANAGED-CONFIG https://provider.test/t0k3n",
                "\u{feff}  #!MANAGED-CONFIG ***",
            ),
            // as many marks and spaces as there are, in either order
            (
                "\u{feff}\u{feff}#!MANAGED-CONFIG https://provider.test/t0k3n",
                "\u{feff}\u{feff}#!MANAGED-CONFIG ***",
            ),
            (
                "\u{3000}\u{feff}\u{a0}#!MANAGED-CONFIG https://provider.test/t0k3n",
                "\u{3000}\u{feff}\u{a0}#!MANAGED-CONFIG ***",
            ),
            // the link is not the first word, or there are two, or a word
            // that is no parameter
            (
                "#!MANAGED-CONFIG interval=60 https://provider.test/t0k3n strict=true",
                "#!MANAGED-CONFIG interval=60 *** strict=true",
            ),
            (
                "#!MANAGED-CONFIG https://a.test/t0k3n https://b.test/t0k3n2",
                "#!MANAGED-CONFIG *** ***",
            ),
            (
                "#!MANAGED-CONFIG https://provider.test/x token=t0k3n interval=60",
                "#!MANAGED-CONFIG *** *** interval=60",
            ),
            (
                "#!MANAGED-CONFIG interval=60 sub.test/t0k3n \"https://q.test/t0k3n\"",
                "#!MANAGED-CONFIG interval=60 *** ***",
            ),
            // a parameter is shown in its plain form only
            (
                "#!MANAGED-CONFIG https://p.test/x interval=abc123 strict=t0k3n interval=60 strict=false Interval=60 interval= interval=6 0",
                "#!MANAGED-CONFIG *** *** *** interval=60 strict=false *** *** interval=6 ***",
            ),
            // no link: nothing to hide
            ("#!MANAGED-CONFIG", "#!MANAGED-CONFIG"),
            ("#!MANAGED-CONFIG  \r", "#!MANAGED-CONFIG  \r"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
        assert_eq!(
            redact_profile(
                "#!MANAGED-CONFIG https://provider.test/t0k3n?surge=4 interval=60\r\n[General]\r\nloglevel = notify\r\n"
            ),
            "#!MANAGED-CONFIG *** interval=60\r\n[General]\r\nloglevel = notify\r\n"
        );
    }

    /// Every `#!` line is read by its shape: the name, and a word with `://`
    /// in it is a link whoever the directive is. A name that is spelled a
    /// little differently is another directive, and so is one with something
    /// glued to it; neither is `#!MANAGED-CONFIG`, and both lose their links.
    #[test]
    fn a_look_alike_of_the_managed_config_line_loses_its_link_too() {
        for (line, shown) in [
            (
                "#!MANAGED_CONFIG https://provider.test/t0k3n?surge=4",
                "#!MANAGED_CONFIG ***",
            ),
            (
                "#!managed-config https://provider.test/t0k3n",
                "#!managed-config ***",
            ),
            (
                "#! MANAGED-CONFIG https://provider.test/t0k3n",
                "#! MANAGED-CONFIG ***",
            ),
            (
                "#!MANAGED-CONFIGX https://provider.test/t0k3n interval=60",
                "#!MANAGED-CONFIGX *** interval=60",
            ),
            (
                "#!MANAGED_CONFIG=https://provider.test/t0k3n",
                "#!MANAGED_CONFIG***",
            ),
            (
                "#!MANAGED-CONFIGhttps://provider.test/t0k3n",
                "#!MANAGED-CONFIGhttps***",
            ),
            // glued to the exact name, `parse_managed` takes it for the link
            (
                "#!MANAGED-CONFIG=https://provider.test/t0k3n interval=60",
                "#!MANAGED-CONFIG*** interval=60",
            ),
            (
                "#!MANAGED-CONFIG.provider.test/t0k3n",
                "#!MANAGED-CONFIG***",
            ),
            (
                "#!Managed-Config https://p.test/t0k3n",
                "#!Managed-Config ***",
            ),
            (
                "#!MANAGED-CONFIG\u{200b} https://p.test/t0k3n",
                "#!MANAGED-CONFIG*** ***",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A remote include is a link on a `#!include` line, one target of a list
    /// separated by commas; a local path stays.
    #[test]
    fn a_remote_include_loses_its_link_and_a_local_one_stays() {
        for (line, shown) in [
            (
                "#!include a.conf, https://h.example/p?token=t0k3n",
                "#!include a.conf, ***",
            ),
            ("#!include https://h.example/p?token=t0k3n", "#!include ***"),
            (
                "#!include \"my list.conf\", \"https://h.example/a b?token=t0k3n\" , b.conf",
                "#!include \"my list.conf\", *** , b.conf",
            ),
            (
                "#!include conf=1/a.conf, b.conf",
                "#!include conf=1/a.conf, b.conf",
            ),
            ("#!include", "#!include"),
            // another spelling of the directive is no include: its words
            ("#!INCLUDE a.conf,https://h.example/t0k3n", "#!INCLUDE ***"),
            ("#!includehttps://h.example/t0k3n", "#!includehttps***"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// Lines with nothing to hide stay as written: a directive without a link,
    /// and a line that only has `#!` in its middle, which is no directive line.
    /// A requirement directive in front of a line of the profile does not take
    /// the line's own redaction away.
    #[test]
    fn what_has_no_link_stays_as_written() {
        for line in [
            "#!",
            "#!FORBIDDEN-AUTO-UPGRADE smart-group",
            "#!SOMETHING-ELSE",
            "#!REQUIREMENT CORE_VERSION>=20",
            "#!MACOS-ONLY DOMAIN,a.com,REJECT",
            "loglevel = notify #!MANAGED-CONFIG https://provider.test/t0k3n",
            "DOMAIN,a.com,REJECT #!MACOS-ONLY",
        ] {
            assert_eq!(redact_profile(line), line, "{line:?}");
        }
        for (line, shown) in [
            (
                "#!MACOS-ONLY Hk = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x1",
                "#!MACOS-ONLY Hk = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=***",
            ),
            (
                "#!IOS-ONLY G = select, A, policy-path=https://sub.test/n?token=t0k3n",
                "#!IOS-ONLY G = select, A, policy-path=***",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// Every `args` of an `external` line, quoted or not (M4-D12).
    #[test]
    fn an_external_line_loses_its_args() {
        assert_eq!(
            redact_definition(
                "external, exec = \"/usr/bin/sshpass\", args = \"-p\", args = \"hunter2, really\", args=ssh, local-port = 1080"
            ),
            "external, exec = \"/usr/bin/sshpass\", args = ***, args = ***, args=***, local-port = 1080"
        );
    }

    /// The camouflage host and path of an `ss` / `snell` line (phase 2 M6
    /// design 6): behind a front that routes by path the path is a shared
    /// secret, as `ws-path` is; the method stays.
    #[test]
    fn an_obfs_line_loses_its_host_and_path() {
        assert_eq!(
            redact_definition(
                "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=pw, obfs=http, obfs-host=my.cdn.test, obfs-uri=/s3cretPath"
            ),
            "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=***, obfs=http, obfs-host=***, obfs-uri=***"
        );
        assert_eq!(
            redact_definition(
                "snell, 1.2.3.4, 8000, psk=s3cretPsk, version=5, obfs=http, obfs-host=my.cdn.test"
            ),
            "snell, 1.2.3.4, 8000, psk=***, version=5, obfs=http, obfs-host=***"
        );
    }

    /// The HTTP/2 family (phase 2 M6 design 6): named and positional
    /// credentials and the whole `headers` list; `max-streams` and `h3` stay.
    #[test]
    fn an_h2_line_loses_its_credentials_and_headers() {
        assert_eq!(
            redact_definition(
                "h2-connect, example.com, 443, user, pass, headers=X-Token:abc, max-streams=5, udp-relay=true"
            ),
            "h2-connect, example.com, 443, ***, ***, headers=***, max-streams=5, udp-relay=true"
        );
        assert_eq!(
            redact_definition(
                "trust-tunnel, 192.168.20.62, 443, username=test, password=s3cret, headers=X-Padding:<random-string(16-32)>, h3=true"
            ),
            "trust-tunnel, 192.168.20.62, 443, username=***, password=***, headers=***, h3=true"
        );
    }

    /// Hysteria 2 (phase 2 M7 design 8): the password and the obfuscation
    /// password, Salamander or Gecko; `download-bandwidth` and the hopping
    /// ports stay.
    #[test]
    fn a_hysteria2_line_loses_its_passwords() {
        assert_eq!(
            redact_definition(
                "hysteria2, h.test, 443, password=s3cret, download-bandwidth=100, salamander-password=0bfs, port-hopping=1234;5000-6000"
            ),
            "hysteria2, h.test, 443, password=***, download-bandwidth=100, salamander-password=***, port-hopping=1234;5000-6000"
        );
        assert_eq!(
            redact_profile("H = hysteria2, h.test, 443, password=s3cret, gecko-password=0bfs"),
            "H = hysteria2, h.test, 443, password=***, gecko-password=***"
        );
    }

    /// TUIC (phase 2 M7 design 8): v4's token, v5's UUID and password, and a
    /// stray positional value; the ALPN and the hopping ports stay.
    #[test]
    fn a_tuic_line_loses_its_credentials() {
        assert_eq!(
            redact_definition(
                "tuic, 192.168.20.6, 443, token=s3cret, alpn=h3, port-hopping=1234;5000-6000"
            ),
            "tuic, 192.168.20.6, 443, token=***, alpn=h3, port-hopping=1234;5000-6000"
        );
        assert_eq!(
            redact_profile(
                "T = tuic-v5, h.test, 443, hunter2, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=s3cret, alpn=h3"
            ),
            "T = tuic-v5, h.test, 443, ***, uuid=***, password=***, alpn=h3"
        );
    }

    /// MASQUE (phase 2 M7 design 7.1): the named credentials and a stray
    /// positional value; the ALPN and the hopping ports stay.
    #[test]
    fn a_masque_line_loses_its_credentials() {
        assert_eq!(
            redact_definition(
                "masque, example.com, 443, username=user, password=s3cret, alpn=h3, port-hopping=1234;5000-6000"
            ),
            "masque, example.com, 443, username=***, password=***, alpn=h3, port-hopping=1234;5000-6000"
        );
        assert_eq!(
            redact_profile("M = masque, example.com, 443, hunter2, username=user, password=s3cret"),
            "M = masque, example.com, 443, ***, username=***, password=***"
        );
    }

    // What the parsers read as a secret parameter is hidden, however the
    // item is written.

    /// Values that are nothing but distinctive: whatever of them is in an
    /// output is a leak.
    const V: &str = "Zq9kV7xM";
    const W: &str = "Wp4nT2yR";
    /// A WireGuard peer's keys are Base64 of 32 bytes.
    const PSK: &str = "Zq9kV7xMZq9kV7xMZq9kV7xMZq9kV7xMZq9kV7xMZq8=";
    const PUBLIC: &str = "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=";
    const PRIVATE: &str = "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=";
    /// Harmless items to stand beside the secret one: a plain one and a
    /// quoted one with a comma in it.
    const NEIGHBOURS: [&str; 2] = ["tfo=true", "\"sni=a.test,b.test\""];
    /// Where the secret item (`S`) stands among the neighbours (`0`, `1`).
    const ARRANGEMENTS: [&[&str]; 5] = [
        &["S"],
        &["0", "S", "1"],
        &["1", "S", "0"],
        &["0", "1", "S"],
        &["S", "0"],
    ];

    fn span() -> crate::span::Span {
        crate::span::Span::new(std::sync::Arc::from(std::path::Path::new("r.conf")), 1)
    }

    /// One way to write a parameter between two commas, the value the parser
    /// reads from it, and whether it leaves a quote open (the rest of the
    /// line is its value then, so nothing can follow).
    struct Spelling {
        text: String,
        value: String,
        open: bool,
    }

    fn spellings(name: &str) -> Vec<Spelling> {
        let upper = name.to_ascii_uppercase();
        let both = format!("{V},{W}");
        let spelling = |text: String, value: &str| Spelling {
            text,
            value: value.to_string(),
            open: false,
        };
        vec![
            spelling(format!("{name}={V}"), V),
            spelling(format!("{name} = {V} "), V),
            spelling(format!("{upper}={V}"), V),
            spelling(format!("\"{name}={V}\""), V),
            spelling(format!("'{name}={V}'"), V),
            spelling(format!("\"{name}={both}\""), &both),
            spelling(format!("{name}=\"{both}\""), &both),
            spelling(format!("\u{3000}{name}={V}"), V),
            spelling(format!("\u{a0}{name}={V}"), V),
            spelling(format!("{name}={V}\u{3000}"), V),
            // more of what the parser takes
            spelling(format!("'{name}={both}'"), &both),
            spelling(format!("{name}='{both}'"), &both),
            spelling(format!("\"{upper} = {V}\""), V),
            spelling(format!("{name}\t=\t{V}"), V),
            spelling(format!("{name}\u{3000}=\u{3000}{V}"), V),
            spelling(format!("\u{2003}{name} = \"{both}\"\u{a0}"), &both),
            spelling(format!("{name}=\"{V}\\\"{W}\""), &format!("{V}\"{W}")),
            spelling(format!("{name}=a\"{both}\"b"), &format!("a\"{both}\"b")),
            spelling(format!("{name}=a({both})b"), &format!("a({both})b")),
            Spelling {
                text: format!("{name}=\"{both}"),
                value: format!("\"{both}"),
                open: true,
            },
        ]
    }

    /// A spelling with the name and the values taken out: what its leak is
    /// called in a failure message.
    fn shape(text: &str, name: &str) -> String {
        let upper = name.to_ascii_uppercase();
        let shape = text
            .replace(name, "name")
            .replace(&upper, "NAME")
            .replace(V, "v")
            .replace(W, "w");
        format!("{shape:?}")
    }

    /// How many lines of each head and shape went wrong, and the first few.
    fn summary(kinds: &[String], lines: &[String]) -> String {
        let mut counts: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for kind in kinds {
            *counts.entry(kind).or_default() += 1;
        }
        format!(
            "{} lines go wrong, by kind: {counts:#?}, the first of them: {:#?}",
            kinds.len(),
            &lines[..lines.len().min(8)]
        )
    }

    /// `slots` of `ARRANGEMENTS` as the items between the commas.
    fn arranged(slots: &[&str], secret: &str, neighbours: &[&str; 2]) -> Vec<String> {
        slots
            .iter()
            .map(|slot| match *slot {
                "S" => secret.to_string(),
                n => neighbours[n.parse::<usize>().unwrap()].to_string(),
            })
            .collect()
    }

    /// A positional credential after the port of a head.
    const POS: &str = "Qk3mRt8z";

    /// The forms a line of the profile takes when the section parser takes
    /// something off it: `{line}` is the definition. The second is whether the
    /// parser reads the line (a commented-out one it reads as nothing).
    const FORMS: [(&str, bool); 6] = [
        ("X = {line}", true),
        ("X = {line} // note", true),
        ("X = {line} #!MACOS-ONLY", true),
        ("#!MACOS-ONLY X = {line}", true),
        ("#!REQUIREMENT CORE_VERSION>=20 X = {line}", true),
        ("# X = {line}", false),
    ];

    /// What the section parser makes of `text`, a line of `[Proxy]` or of
    /// `[Proxy Group]`: the values it reads for the parameter `name`, and the
    /// `Debug` of what it built (when asked for: it is the same in every form
    /// of one line). Nothing when it reads no line, or one that does not parse.
    fn read_through_the_parser(
        text: &str,
        group: bool,
        name: &str,
        debug: bool,
    ) -> (Vec<String>, String) {
        let section = if group { "Proxy Group" } else { "Proxy" };
        let (profile, _) = crate::text::parse_str(
            &format!("[{section}]\n{text}\n"),
            std::sync::Arc::from(std::path::Path::new("r.conf")),
            crate::text::Origin::Main,
        );
        let Some((key, definition)) = profile
            .sections
            .first()
            .and_then(|s| s.entries.first())
            .and_then(|e| crate::value::split_definition(&e.raw))
        else {
            return (Vec::new(), String::new());
        };
        let values = |params: &crate::value::ParamMap| {
            params
                .get_all(name)
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
        };
        if group {
            match crate::policy::parse_group(key, definition, &span()) {
                Ok(g) => (
                    values(&g.params),
                    if debug {
                        format!("{g:?}")
                    } else {
                        String::new()
                    },
                ),
                Err(_) => (Vec::new(), String::new()),
            }
        } else {
            match crate::policy::parse_policy(key, definition, &span()) {
                Ok(p) => (
                    values(&p.params),
                    if debug {
                        format!("{p:?}")
                    } else {
                        String::new()
                    },
                ),
                Err(_) => (Vec::new(), String::new()),
            }
        }
    }

    /// For every head, every secret name and every spelling of its item,
    /// alone and among harmless ones, in every form the section parser takes
    /// something off (an inline comment, a requirement directive in front or
    /// at the end, a comment marker): the parser reads the name with that
    /// value, and the redacted text then has none of the value. The parser
    /// goes first, so that the spellings are no wishful thinking.
    #[test]
    fn whatever_the_parser_reads_as_a_secret_parameter_is_hidden_however_it_is_written() {
        // each with whether it gets every arrangement of the items
        let heads: Vec<(String, bool, bool)> = [
            ("trojan, t.test, 443", false),
            ("http, h.test, 80", false),
            ("external", false),
            ("external, exec=/bin/x, local-port=1080", false),
            ("direct", false),
            ("select, A, B", true),
            ("url-test, A, B", true),
        ]
        .into_iter()
        .map(|(head, group)| (head.to_string(), group, true))
        // the type in quotes, in the blanks an input method types
        .chain([
            ("\"trojan\", t.test, 443".to_string(), false, false),
            (
                "\u{3000}trojan\u{3000}, t.test, 443".to_string(),
                false,
                false,
            ),
            ("'HTTPS' , h.test, 443".to_string(), false, false),
        ])
        // and with positional credentials after the port
        .chain([
            (format!("http, h.test, 80, alice, {POS}"), false, false),
            (
                format!("\"socks5\", s.test, 1080,\u{3000}bob\u{3000}, {POS}"),
                false,
                false,
            ),
            (
                format!("\u{3000}trojan\u{3000}, t.test, 443, \"alice\", {POS}"),
                false,
                false,
            ),
        ])
        .collect();
        let mut checked = 0usize;
        let mut unread: Vec<String> = Vec::new();
        let mut kinds: Vec<String> = Vec::new();
        let mut leaked: Vec<String> = Vec::new();
        for (head, group, every_arrangement) in &heads {
            let server = crate::value::split_list(head)
                .first()
                .and_then(|keyword| PolicyKind::parse(keyword))
                .is_some_and(|kind| kind.takes_server());
            for name in SECRET_PARAMS {
                for spelling in spellings(name) {
                    for slots in ARRANGEMENTS {
                        if spelling.open && slots.last() != Some(&"S") {
                            continue;
                        }
                        for pad in ["", " "] {
                            // the work is kept in bounds: every arrangement for
                            // the plain form of the first heads, one for the
                            // other forms and heads; how the line is wrapped
                            // and how the items stand around the secret one
                            // do not depend on each other
                            let one = slots == ["0", "S", "1"] && pad == " ";
                            if !every_arrangement && !one {
                                continue;
                            }
                            let forms = if one { &FORMS[..] } else { &FORMS[..1] };
                            let items = arranged(slots, &spelling.text, &NEIGHBOURS);
                            let line = format!(
                                "{head}{}",
                                items
                                    .iter()
                                    .map(|i| format!(",{pad}{i}"))
                                    .collect::<String>()
                            );
                            let out = redact_definition(&line);
                            for (form, read_by_parser) in forms.iter().copied() {
                                // an open quote takes what follows it for its own
                                if spelling.open && !form.ends_with("{line}") {
                                    continue;
                                }
                                let text = form.replace("{line}", &line);
                                // the parser first
                                let debug = if read_by_parser {
                                    let (read, debug) = read_through_the_parser(
                                        &text,
                                        *group,
                                        name,
                                        form == FORMS[0].0,
                                    );
                                    if !read.contains(&spelling.value) {
                                        unread.push(text);
                                        continue;
                                    }
                                    debug
                                } else {
                                    String::new()
                                };
                                checked += 1;
                                let shown = redact_profile(&text);
                                let mut bad = Vec::new();
                                for secret in [V, W, POS, "alice", "bob"] {
                                    if shown.contains(secret) {
                                        bad.push(format!("shows {secret}"));
                                    }
                                }
                                for secret in [V, W] {
                                    if debug.contains(secret) {
                                        bad.push(format!("Debug shows {secret}"));
                                    }
                                }
                                // the neighbours stay as written, but for a quoted
                                // one on a line of a type that takes a server: it is
                                // a positional value there, and those go whole
                                for slot in slots.iter().filter(|s| **s != "S") {
                                    let n = NEIGHBOURS[slot.parse::<usize>().unwrap()];
                                    let expected = if server && n.starts_with('"') {
                                        format!(",{pad}***")
                                    } else {
                                        format!(",{pad}{n}")
                                    };
                                    if !shown.contains(&expected) {
                                        bad.push(format!("lost {n}"));
                                    }
                                }
                                // one rule, two entry points: the line is the
                                // definition's, with what the parser took off kept
                                if shown != text.replace(&line, &out) {
                                    bad.push("the profile endpoint differs".to_string());
                                }
                                // and what is redacted is redacted
                                if form == FORMS[0].0 && redact_profile(&shown) != shown {
                                    bad.push("redacting it again changes it".to_string());
                                }
                                if !bad.is_empty() {
                                    kinds.push(format!(
                                        "{head} | {form} | {}",
                                        shape(&spelling.text, name)
                                    ));
                                    leaked
                                        .push(format!("{text:?} -> {shown:?}: {}", bad.join(", ")));
                                }
                            }
                        }
                    }
                }
            }
        }
        println!("{checked} lines read by the parser and redacted");
        assert!(
            unread.is_empty(),
            "{} lines the parser does not read as generated, the first of them: {:#?}",
            unread.len(),
            &unread[..unread.len().min(8)]
        );
        assert!(checked > 30_000, "{checked}");
        assert!(
            kinds.is_empty(),
            "of {checked} lines, {}",
            summary(&kinds, &leaked)
        );
    }

    /// The same for a `[Keystore]` line: its password and its payload.
    #[test]
    fn a_keystore_line_loses_what_the_parser_reads_as_its_password_and_payload() {
        let mut checked = 0usize;
        let mut kinds: Vec<String> = Vec::new();
        let mut leaked: Vec<String> = Vec::new();
        for name in ["password", "base64"] {
            for spelling in spellings(name) {
                for slots in ARRANGEMENTS {
                    // an open quote takes the payload that follows with it
                    if spelling.open && (slots.last() != Some(&"S") || name == "password") {
                        continue;
                    }
                    let items = arranged(slots, &spelling.text, &NEIGHBOURS);
                    let mut line = format!("type=p12, {}", items.join(", "));
                    if name == "password" {
                        line.push_str(", base64=B64B64");
                    }
                    let item = crate::keystore::parse_keystore_item("k", &line, &span()).unwrap();
                    let read = if name == "password" {
                        item.password.clone().unwrap_or_default()
                    } else {
                        item.base64.clone()
                    };
                    assert_eq!(read, spelling.value, "{line}");
                    checked += 1;
                    let out = redact_profile(&format!("k = {line}"));
                    let mut bad = Vec::new();
                    for secret in [V, W, "B64B64"] {
                        if out.contains(secret) {
                            bad.push(format!("shows {secret}"));
                        }
                    }
                    for slot in slots.iter().filter(|s| **s != "S") {
                        let n = NEIGHBOURS[slot.parse::<usize>().unwrap()];
                        if !out.contains(&format!(", {n}")) {
                            bad.push(format!("lost {n}"));
                        }
                    }
                    if !bad.is_empty() {
                        kinds.push(format!("{name} | {}", shape(&spelling.text, name)));
                        leaked.push(format!("{line:?} -> {out:?}: {}", bad.join(", ")));
                    }
                }
            }
        }
        println!("{checked} keystore lines read by the parser and redacted");
        assert!(checked > 100, "{checked}");
        assert!(
            kinds.is_empty(),
            "of {checked} lines, {}",
            summary(&kinds, &leaked)
        );
    }

    /// The same for a `[WireGuard]` peer, alone on its line and with another
    /// beside it: the key the section parser reads as the peer's pre-shared
    /// key is hidden, and the group keeps its shape.
    #[test]
    fn a_wireguard_peer_loses_what_the_parser_reads_as_its_preshared_key() {
        use base64::Engine;
        let psk: [u8; 32] = base64::engine::general_purpose::STANDARD
            .decode(PSK)
            .unwrap()
            .try_into()
            .unwrap();
        let neighbours = ["keepalive = 25", "client-id = 83/12/235"];
        let base = format!(
            "public-key = {PUBLIC}, \"allowed-ips = 10.0.0.0/8, 192.168.0.0/16\", endpoint = a.test:1"
        );
        let mut checked = 0usize;
        let mut kinds: Vec<String> = Vec::new();
        let mut leaked: Vec<String> = Vec::new();
        for spelling in spellings("preshared-key") {
            // one 32-byte key is all a peer takes
            if spelling.value != V {
                continue;
            }
            let secret = spelling.text.replace(V, PSK);
            for slots in ARRANGEMENTS {
                if spelling.open && slots.last() != Some(&"S") {
                    continue;
                }
                let items = arranged(slots, &secret, &[neighbours[0], neighbours[1]]);
                let peer = format!("({base}, {})", items.join(", "));
                for line in [
                    format!("peer = {peer}"),
                    format!(
                        "peer = (public-key = {PUBLIC}, allowed-ips = 10.1.0.0/16, endpoint = b.test:2), {peer}"
                    ),
                ] {
                    let text = format!(
                        "[WireGuard w]\nprivate-key = {PRIVATE}\nself-ip = 10.0.0.2\n{line}\n"
                    );
                    let (profile, d) = crate::text::parse_str(
                        &text,
                        std::sync::Arc::from(std::path::Path::new("w.conf")),
                        crate::text::Origin::Main,
                    );
                    assert!(d.is_empty(), "{:?}", d.into_vec());
                    let mut diags = crate::diagnostic::Diagnostics::default();
                    let section = crate::wireguard::parse_section(&profile.sections[0], &mut diags)
                        .unwrap_or_else(|| panic!("{line}: {:?}", diags.into_vec()));
                    let last = section.peers.last().unwrap();
                    assert_eq!(
                        last.preshared_key.as_ref().map(|k| *k.expose()),
                        Some(psk),
                        "{line}"
                    );
                    checked += 1;
                    let out = redact_profile(&text);
                    let shown = out.lines().nth(3).unwrap();
                    let mut bad = Vec::new();
                    if out.contains(PSK) || out.contains("Zq9kV7xM") {
                        bad.push("shows the key".to_string());
                    }
                    if !shown.contains(&base) || !shown.ends_with(')') {
                        bad.push("lost the shape".to_string());
                    }
                    for slot in slots.iter().filter(|s| **s != "S") {
                        let n = neighbours[slot.parse::<usize>().unwrap()];
                        if !shown.contains(&format!(", {n}")) {
                            bad.push(format!("lost {n}"));
                        }
                    }
                    if !bad.is_empty() {
                        kinds.push(shape(&spelling.text, "preshared-key"));
                        leaked.push(format!("{shown:?}: {}", bad.join(", ")));
                    }
                }
            }
        }
        println!("{checked} peer lines read by the parser and redacted");
        assert!(checked > 50, "{checked}");
        assert!(
            kinds.is_empty(),
            "of {checked} lines, {}",
            summary(&kinds, &leaked)
        );
    }

    /// How such lines come out: the item as the parser reads it, the
    /// spacing, the name and the case of one written plainly kept; from
    /// inside quotes only `name=***` is left, and every other item as it was
    /// written.
    #[test]
    fn an_item_is_hidden_the_way_the_parser_reads_it() {
        for (line, shown) in [
            // the full-width space an East Asian input method types
            (
                "HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm,\u{3000}password=x1",
                "HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm,\u{3000}password=***",
            ),
            (
                "Ext = external,\u{a0}args = hunter2 ,exec=/bin/x",
                "Ext = external,\u{a0}args = *** ,exec=/bin/x",
            ),
            // the whole item in quotes: read as the parameter of that name
            (
                "G = select, \"policy-path=https://sub.example/list?token=t0k3n\"",
                "G = select, policy-path=***",
            ),
            (
                "Ext = external, exec=/bin/x, local-port=1080, \"args=-p hunter2\"",
                "Ext = external, exec=/bin/x, local-port=1080, args=***",
            ),
            (
                "k = type=p12, \"password=pw\", base64=AAAA",
                "k = type=p12, password=***, base64=***",
            ),
            // single quotes, a comma inside, a space around `=`
            (
                "Ext = external, 'args = a,b' , local-port=1080",
                "Ext = external, args=*** , local-port=1080",
            ),
            (
                "Ext = external, \"ARGS=a,b\", args=\"c,d\", Args = 'e' ",
                "Ext = external, ARGS=***, args=***, Args = *** ",
            ),
            // a quote that opens inside the value takes the rest of the line,
            // as it does for the parser; so does one left open
            (
                "Ext = external, args=a\"b,c\"d, local-port=1080",
                "Ext = external, args=***, local-port=1080",
            ),
            (
                "Ext = external, local-port=1080, args=\"p,w",
                "Ext = external, local-port=1080, args=***",
            ),
            // a name that only ends like a secret one, a secret one in the
            // value of another, and a quoted item that is nobody's secret
            (
                "Ext = external, my-args=a, exec=/bin/args=x, \"sni=a.test,b.test\"",
                "Ext = external, my-args=a, exec=/bin/args=x, \"sni=a.test,b.test\"",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A `[WireGuard]` peer is a group whose items are read as a line's are,
    /// a group in quotes too; the parentheses stay.
    #[test]
    fn the_items_of_a_group_are_hidden_the_way_the_parser_reads_them() {
        for (line, shown) in [
            (
                "peer = (public-key = P, \"preshared-key = V\", endpoint = a.test:1)",
                "peer = (public-key = P, preshared-key=***, endpoint = a.test:1)",
            ),
            (
                "peer = (public-key = P,\u{3000}preshared-key = V)",
                "peer = (public-key = P,\u{3000}preshared-key = ***)",
            ),
            (
                "peer = (public-key = P, endpoint = a.test:1, 'PRESHARED-KEY=V' )",
                "peer = (public-key = P, endpoint = a.test:1, PRESHARED-KEY=*** )",
            ),
            // several peers on a line; a group in quotes is read as one
            (
                "peer = (public-key = P, preshared-key = V), (public-key = Q, endpoint = b.test:2)",
                "peer = (public-key = P, preshared-key = ***), (public-key = Q, endpoint = b.test:2)",
            ),
            (
                "peer = \"(public-key = P, preshared-key = V)\", (public-key = Q, preshared-key=W)",
                "peer = (public-key = P, preshared-key = ***), (public-key = Q, preshared-key=***)",
            ),
            (
                "peer = \"(public-key = P, endpoint = a.test:1)\", (public-key = Q, endpoint = b.test:2)",
                "peer = \"(public-key = P, endpoint = a.test:1)\", (public-key = Q, endpoint = b.test:2)",
            ),
            // a group on a line of another kind
            (
                "T = external, x=1, (password=V), tfo=true",
                "T = external, x=1, (password=***), tfo=true",
            ),
            // and one in a place of a positional value: a group is no name
            (
                "T = trojan, h, 443, (password=V), tfo=true",
                "T = trojan, h, 443, ***, tfo=true",
            ),
            // a group left open is no group: the search by name finds the secret
            (
                "peer = (public-key = P, preshared-key = V",
                "peer = (public-key = P, preshared-key = ***",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// What the parsers read as one value can hold a secret parameter of its
    /// own, wherever the item pass cannot tell: the search by name stays, and
    /// it too takes any Unicode whitespace for the start of a name.
    #[test]
    fn the_search_by_name_finds_a_secret_in_what_is_one_value() {
        for (def, shown) in [
            (
                "external, x=\"a,\u{3000}password=V",
                "external, x=\"a,\u{3000}password=***",
            ),
            (
                "external, x=\"a,\u{a0}token = V, b\"",
                "external, x=\"a,\u{a0}token = ***, b\"",
            ),
            (
                "external, x=1\u{2003}uuid=V",
                "external, x=1\u{2003}uuid=***",
            ),
            (
                "external, x=(1,\u{3000}psk=V)",
                "external, x=(1,\u{3000}psk=***",
            ),
        ] {
            assert_eq!(redact_definition(def), shown, "{def:?}");
        }
    }

    /// Text the parsers make nothing of, and text that is long or deep:
    /// nothing panics, nothing overflows, and what is a secret still goes.
    #[test]
    fn odd_text_neither_panics_nor_keeps_a_secret() {
        for text in [
            "",
            ",",
            ",,,",
            "(",
            ")",
            "()",
            "(,)",
            "\"",
            "'",
            "\\",
            "\"\\",
            "=",
            "==",
            " = ",
            "password",
            "password=",
            "\u{3000}",
            "\u{feff}",
            "é=x, ééé=é, \u{3000}é",
            "\"é",
            "(é",
            "é)",
        ] {
            let shown = redact_definition(text);
            assert_eq!(redact_definition(&shown), shown, "{text:?}");
        }
        for (text, shown) in [
            ("password=", "password=***"),
            (
                "\u{3000}password\u{3000}=\u{3000}é",
                "\u{3000}password\u{3000}=\u{3000}***",
            ),
            ("é,\u{a0}password=é,é", "é,\u{a0}password=***,é"),
            ("\"é\",\"password=é\"", "\"é\",password=***"),
            ("(é, password=é)", "(é, password=***)"),
            ("((é), \"password=é\")", "((é), password=***)"),
        ] {
            assert_eq!(redact_definition(text), shown, "{text:?}");
        }
        let depth = 10_000;
        let nested = format!("{}password={V}{}", "(".repeat(depth), ")".repeat(depth));
        assert!(!redact_definition(&nested).contains(V));
        let nested = format!("trojan, h, 443, {}password={V}", "(".repeat(depth));
        assert!(!redact_definition(&nested).contains(V));
        let nested = format!("x=\"{}\", password={V}", "(\"".repeat(depth));
        assert!(!redact_definition(&nested).contains(V));
        let long = format!("external, {}, args={V}", "a=b,".repeat(100_000));
        assert!(!redact_definition(&long).contains(V));
        let wide = format!("external, args={}", V.repeat(100_000));
        assert!(!redact_definition(&wide).contains(V));
    }

    // The line is read as the section parser reads it: what it takes off the
    // line stays as written, what is left is redacted.

    /// A commented-out line is where old secrets are left: the marker (any
    /// run of them) stays, and what follows is redacted as if the line were
    /// not commented out — a managed config line included, which starts with
    /// a `#` of its own.
    #[test]
    fn a_commented_out_line_is_redacted_as_if_it_were_not() {
        for (line, shown) in [
            ("# private-key = K3Y", "# private-key = ***"),
            (";ca-passphrase = hunter2", ";ca-passphrase = ***"),
            (
                "// http-api = k3y@127.0.0.1:6171",
                "// http-api = ***@127.0.0.1:6171",
            ),
            // any run of markers, with white space between them or without
            ("## private-key = K3Y", "## private-key = ***"),
            ("# # private-key = K3Y", "# # private-key = ***"),
            ("//# ;private-key = K3Y", "//# ;private-key = ***"),
            (";;;\tprivate-key = K3Y", ";;;\tprivate-key = ***"),
            // in front of the marker: white space and byte-order marks
            (
                "\u{feff}  # private-key = K3Y",
                "\u{feff}  # private-key = ***",
            ),
            (
                "\u{3000}\u{200b}// private-key = K3Y",
                "\u{3000}\u{200b}// private-key = ***",
            ),
            // a commented-out policy: positional credentials and all
            (
                "# P = http, h.test, 80, alice, hunter2",
                "# P = http, h.test, 80, ***, ***",
            ),
            (
                "// Hk = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x1",
                "// Hk = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=***",
            ),
            // a secret mentioned in a comment, as free text
            ("# old password=hunter2", "# old password=***"),
            ("// the token = t0k3n was", "// the token = ***"),
            // the managed config line, commented out in each way
            (
                "# #!MANAGED-CONFIG https://provider.test/t0k3n?surge=4 interval=43200 strict=true",
                "# #!MANAGED-CONFIG *** interval=43200 strict=true",
            ),
            (
                "  # #!MANAGED-CONFIG https://provider.test/t0k3n",
                "  # #!MANAGED-CONFIG ***",
            ),
            (
                "##!MANAGED-CONFIG https://provider.test/t0k3n",
                "##!MANAGED-CONFIG ***",
            ),
            (
                ";#!MANAGED-CONFIG https://provider.test/t0k3n",
                ";#!MANAGED-CONFIG ***",
            ),
            (
                "// #!MANAGED-CONFIG interval=60 https://provider.test/t0k3n",
                "// #!MANAGED-CONFIG interval=60 ***",
            ),
            // and with the `!` dropped, whatever the spelling of the name
            (
                "#MANAGED-CONFIG https://provider.test/t0k3n",
                "#MANAGED-CONFIG ***",
            ),
            (
                "# MANAGED-CONFIG https://provider.test/t0k3n interval=60",
                "# MANAGED-CONFIG *** interval=60",
            ),
            (
                "//managed_config https://provider.test/t0k3n",
                "//managed_config ***",
            ),
            (
                "# !Managed-Config https://provider.test/t0k3n",
                "# !Managed-Config ***",
            ),
            // a comment with nothing to hide stays as written
            ("# the managed config", "# the managed config"),
            (
                "// see https://example.com/docs for more",
                "// see https://example.com/docs for more",
            ),
            ("; password hints", "; password hints"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A requirement directive in front of a line is kept as written — the
    /// expression with it, an `=` in it included — and the rest of the line is
    /// redacted as any other line: it must not be taken for a directive of
    /// the other kind, whose links go.
    #[test]
    fn a_requirement_directive_in_front_is_kept_and_the_line_after_it_is_redacted() {
        for (line, shown) in [
            (
                "#!MACOS-ONLY RULE-SET,https://example.com/a.list,Proxy",
                "#!MACOS-ONLY RULE-SET,https://example.com/a.list,Proxy",
            ),
            (
                "#!IOS-ONLY P = http, h.test, 80, alice, hunter2",
                "#!IOS-ONLY P = http, h.test, 80, ***, ***",
            ),
            (
                "#!TVOS-ONLY\tca-passphrase = hunter2",
                "#!TVOS-ONLY\tca-passphrase = ***",
            ),
            // the first `=` of the line is the one after the name
            (
                "#!REQUIREMENT CORE_VERSION>=20 P = http, h.test, 80, alice, hunter2",
                "#!REQUIREMENT CORE_VERSION>=20 P = http, h.test, 80, ***, ***",
            ),
            (
                "#!REQUIREMENT \"CORE_VERSION >= 20 && SYSTEM = 'macOS'\" http-api = k3y@127.0.0.1:6171",
                "#!REQUIREMENT \"CORE_VERSION >= 20 && SYSTEM = 'macOS'\" http-api = ***@127.0.0.1:6171",
            ),
            (
                "#!REQUIREMENT CORE_VERSION>=20 private-key = K3Y",
                "#!REQUIREMENT CORE_VERSION>=20 private-key = ***",
            ),
            // one directive after another, and a comment after one
            (
                "#!MACOS-ONLY #!IOS-ONLY P = http, h.test, 80, alice, hunter2",
                "#!MACOS-ONLY #!IOS-ONLY P = http, h.test, 80, ***, ***",
            ),
            (
                "#!MACOS-ONLY # P = trojan, h.test, 443, hunter2, password=x1",
                "#!MACOS-ONLY # P = trojan, h.test, 443, ***, password=***",
            ),
            // a directive the parser cannot read (the quote stays open) is
            // redacted all the same
            (
                "#!REQUIREMENT \"CORE_VERSION>=20 P = http, h.test, 80, alice, hunter2",
                "#!REQUIREMENT \"CORE_VERSION>=20 P = http, h.test, 80, ***, ***",
            ),
            // a directive of the other kind in the place of the line
            (
                "#!MACOS-ONLY #!MANAGED-CONFIG https://provider.test/t0k3n interval=60",
                "#!MACOS-ONLY #!MANAGED-CONFIG *** interval=60",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// An inline comment and a requirement directive at the end of a line are
    /// taken off as the parser takes them off: the line is redacted without
    /// them, and they stay as written but for a secret named in them.
    #[test]
    fn an_inline_comment_and_a_directive_at_the_end_are_kept_and_the_line_is_redacted() {
        for (line, shown) in [
            // the quote is whole again without the comment
            (
                "G = select, A, \"policy-path=https://sub.test/n?token=t0k3n\" // main",
                "G = select, A, policy-path=*** // main",
            ),
            // a positional password is not mistaken for a named parameter
            (
                "PosC = socks5, s.test, 1080, alice, hunter2 # expires=2027-01",
                "PosC = socks5, s.test, 1080, ***, *** # expires=2027-01",
            ),
            (
                "PosC = socks5, s.test, 1080, alice, hunter2 ; expires=2027-01",
                "PosC = socks5, s.test, 1080, ***, *** ; expires=2027-01",
            ),
            (
                "P = http, h.test, 80, alice, hunter2 #!MACOS-ONLY",
                "P = http, h.test, 80, ***, *** #!MACOS-ONLY",
            ),
            (
                "P = http, h.test, 80, alice, hunter2 //!IOS-ONLY",
                "P = http, h.test, 80, ***, *** //!IOS-ONLY",
            ),
            (
                "P = http, h.test, 80, alice, hunter2 #!REQUIREMENT CORE_VERSION>=20",
                "P = http, h.test, 80, ***, *** #!REQUIREMENT CORE_VERSION>=20",
            ),
            (
                "P = http, h.test, 80, alice, hunter2 #!MACOS-ONLY // old",
                "P = http, h.test, 80, ***, *** #!MACOS-ONLY // old",
            ),
            // a secret named in what is kept goes
            (
                "P = http, h.test, 80, alice, hunter2 // old password=hunter3",
                "P = http, h.test, 80, ***, *** // old password=***",
            ),
            (
                "P = direct, interface=eth0 # was args = -p hunter2",
                "P = direct, interface=eth0 # was args = ***",
            ),
            // a `#` or a `;` without white space in front is no comment
            (
                "Ext = external, args=a;b, local-port=1080",
                "Ext = external, args=***, local-port=1080",
            ),
            // a comment inside a quote is no comment
            (
                "Ext = external, \"args=a // b\", local-port=1080",
                "Ext = external, args=***, local-port=1080",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// The keys that stand on a line of their own, as their parsers read
    /// them: the separator, the spelling of the name, a text that is no key.
    #[test]
    fn a_whole_line_key_is_read_as_its_parser_reads_it() {
        for (line, shown) in [
            // `＝`, which an input method types
            ("ca-passphrase ＝ hunter2", "ca-passphrase ＝ ***"),
            ("token＝t0k3n", "token＝ ***"),
            // `:` for a key that is a secret, when no `=` stands before it
            ("ca-passphrase: hunter2", "ca-passphrase: ***"),
            ("ca-passphrase\u{ff1a} hunter2", "ca-passphrase\u{ff1a} ***"),
            ("private-key \u{ff1a}B64==", "private-key \u{ff1a} ***"),
            ("private-key : B64KEY==", "private-key : ***"),
            ("psk:a=b", "psk: ***"),
            // the case, the `-` and the `_` of the name do not matter
            ("PrivateKey = K3Y", "PrivateKey = ***"),
            ("PresharedKey = K3Y", "PresharedKey = ***"),
            ("preshared-key = K3Y", "preshared-key = ***"),
            ("Pre_Shared_Key=K3Y", "Pre_Shared_Key= ***"),
            ("CA_P12 = MIIK", "CA_P12 = ***"),
            ("\u{200b}password = x", "\u{200b}password = ***"),
            ("password\u{200b} = x", "password\u{200b} = ***"),
            (
                "\u{feff}ca-p12\u{3000}=\u{3000}MIIK",
                "\u{feff}ca-p12\u{3000}= ***",
            ),
            // a `:` is a separator for no other key
            ("loglevel: notify", "loglevel: notify"),
            ("dns-server: 8.8.8.8", "dns-server: 8.8.8.8"),
            ("password-hint: none", "password-hint: none"),
            // a text that holds a comma in front of the first `=` is no key:
            // the `=` after the name is missing, and the line is a definition
            (
                "P http, h.test, 8080, alice, hunter2, tfo=true",
                "P http, h.test, 8080, ***, ***, tfo=true",
            ),
            (
                "http, h.test, 8080, alice, hunter2",
                "http, h.test, 8080, ***, ***",
            ),
            (
                "ss, 1.2.3.4, 8388, hunter2, encrypt-method=aes-128-gcm",
                "ss, 1.2.3.4, 8388, ***, encrypt-method=aes-128-gcm",
            ),
            // ... or the name holds a comma, and the definition follows its `=`
            (
                "US, New York = http, h.test, 80, alice, hunter2",
                "US, New York = http, h.test, 80, ***, ***",
            ),
            (
                "A, B = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=hunter2",
                "A, B = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=***",
            ),
            ("Ext, password=hunter2", "Ext, password=***"),
            (
                "US, New York \u{ff1d} http, h.test, 80, alice, hunter2",
                "US, New York \u{ff1d} http, h.test, 80, ***, ***",
            ),
            (
                "\u{9999}\u{6e2f}\u{ff0c}\u{8282}\u{70b9} = ss, 1.2.3.4, 8388, alice, hunter2",
                "\u{9999}\u{6e2f}\u{ff0c}\u{8282}\u{70b9} = ss, 1.2.3.4, 8388, ***, ***",
            ),
            // a line of the other sections is not touched
            ("FINAL,DIRECT", "FINAL,DIRECT"),
            ("DEST-PORT,443,DIRECT", "DEST-PORT,443,DIRECT"),
            (
                "IP-CIDR,10.0.0.0/8,DIRECT,no-resolve",
                "IP-CIDR,10.0.0.0/8,DIRECT,no-resolve",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// `http-api` and `external-controller-access`: the parser takes the
    /// whole value and cuts it at its last `@`, so a key may hold a comma; a
    /// value without an `@` is a key and nothing else.
    #[test]
    fn an_api_key_is_cut_at_the_last_at_sign_and_never_at_a_comma() {
        for (line, shown) in [
            (
                "http-api = pa,ss@127.0.0.1:6171",
                "http-api = ***@127.0.0.1:6171",
            ),
            (
                "http-api=pa@ss,word@127.0.0.1:6171  ",
                "http-api=***@127.0.0.1:6171  ",
            ),
            (
                "external-controller-access = \"pa,ss\"@127.0.0.1:6170",
                "external-controller-access = ***@127.0.0.1:6170",
            ),
            (
                "http-api = pa,ss＠127.0.0.1:6171",
                "http-api = ***@127.0.0.1:6171",
            ),
            // the address forgotten: the key is all there is
            ("http-api = hunter2", "http-api = ***"),
            ("Http-Api =  hunter2, tail ", "Http-Api =  *** "),
            (
                "external-controller-access = hunter2",
                "external-controller-access = ***",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// The listeners and the DNS servers are lists the way the parsers split
    /// them: a quote or a parenthesis groups, a comma inside one is not a
    /// separator.
    #[test]
    fn a_list_of_listeners_or_servers_is_split_as_its_parser_splits_it() {
        for (line, shown) in [
            (
                "http-listen = \"pa,ss\"@127.0.0.1:6152, 127.0.0.1:6153",
                "http-listen = ***@127.0.0.1:6152, 127.0.0.1:6153",
            ),
            (
                "http-listen = pw@127.0.0.1:6152,pw2@127.0.0.1:6154",
                "http-listen = ***@127.0.0.1:6152,***@127.0.0.1:6154",
            ),
            (
                "socks5-listen = (a,b)pw@127.0.0.1:6153",
                "socks5-listen = ***@127.0.0.1:6153",
            ),
            (
                "dns-server = 8.8.8.8, 'h3://b0b:p,w@dns.example', 1.1.1.1",
                "dns-server = 8.8.8.8, ***@dns.example', 1.1.1.1",
            ),
            (
                "encrypted-dns-server = https://al1ce:pw@dns.example/dns-query, quic://dns.example",
                "encrypted-dns-server = https://***@dns.example/dns-query, quic://dns.example",
            ),
            (
                "doh-server = (https://al1ce:p,w@dns.example/dns-query)",
                "doh-server = ***@dns.example/dns-query)",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// The type is read as the parser reads it — the first field it does not
    /// drop, the quotes off, as `PolicyKind::parse` compares — and a line for
    /// another client is an unknown type whose password stands in the same
    /// place.
    #[test]
    fn the_positional_values_are_blanked_after_the_type_the_parser_reads() {
        for (line, shown) in [
            // the type in quotes, in blanks, behind an empty item
            (
                "P = \"http\", h.test, 80, alice, hunter2",
                "P = \"http\", h.test, 80, ***, ***",
            ),
            (
                "P = 'HTTPS' , h.test, 80, alice, hunter2",
                "P = 'HTTPS' , h.test, 80, ***, ***",
            ),
            (
                "P = \u{3000}trojan\u{3000}, h.test, 443, hunter2",
                "P = \u{3000}trojan\u{3000}, h.test, 443, ***",
            ),
            (
                "P = , http, h.test, 80, alice, hunter2",
                "P = , http, h.test, 80, ***, ***",
            ),
            (
                "P = \"\", ,\"http\", h.test, 80, alice, hunter2",
                "P = \"\", ,\"http\", h.test, 80, ***, ***",
            ),
            // `，`, which an input method types, is read for a comma: in a line
            // typed so, in a line with one of them, and the separators stay
            (
                "P \u{ff1d} ss\u{ff0c} h.test\u{ff0c} 8388\u{ff0c} alice\u{ff0c} hunter2",
                "P \u{ff1d} ss\u{ff0c} h.test\u{ff0c} 8388\u{ff0c} ***\u{ff0c} ***",
            ),
            (
                "P = ss\u{ff0c} h.test, 8388, alice, hunter2",
                "P = ss\u{ff0c} h.test, 8388, ***, ***",
            ),
            (
                "P = ss, h.test, 8388, alice\u{ff0c} hunter2",
                "P = ss, h.test, 8388, ***\u{ff0c} ***",
            ),
            (
                "ss\u{ff0c} h.test\u{ff0c} 8388\u{ff0c} hunter2",
                "ss\u{ff0c} h.test\u{ff0c} 8388\u{ff0c} ***",
            ),
            // the `=` after the name lost as well
            (
                "P ss\u{ff0c} h.test\u{ff0c} 8388\u{ff0c} alice\u{ff0c} hunter2\u{ff0c} tfo=true",
                "P ss\u{ff0c} h.test\u{ff0c} 8388\u{ff0c} ***\u{ff0c} ***\u{ff0c} tfo=true",
            ),
            // the invisible characters a copy brings along are no part of the
            // type or the port
            (
                "P = custom, h.test,\u{200b}443, alice, hunter2",
                "P = custom, h.test,\u{200b}443, ***, ***",
            ),
            (
                "P = \u{feff}trojan, h.test, \u{feff}443, hunter2",
                "P = \u{feff}trojan, h.test, \u{feff}443, ***",
            ),
            (
                "P = \u{feff}trojan, h.test, alice, hunter2",
                "P = \u{feff}trojan, h.test, ***, ***",
            ),
            // the type in quotes is read with them off, the port being
            // what it may
            (
                "P = \"http\", h.test, alice, hunter2",
                "P = \"http\", h.test, ***, ***",
            ),
            // one in a name or a password is part of it
            (
                "\u{9999}\u{6e2f}\u{ff0c}\u{8282}\u{70b9} = http, h.test, 80, alice, hunter2",
                "\u{9999}\u{6e2f}\u{ff0c}\u{8282}\u{70b9} = http, h.test, 80, ***, ***",
            ),
            (
                "P = external, args=-p a\u{ff0c}b, local-port=1080",
                "P = external, args=***, local-port=1080",
            ),
            (
                "P = trojan, h.test, 443, password=a\u{ff0c}b",
                "P = trojan, h.test, 443, password=***",
            ),
            // what stands in the port's place and is no number goes: a user
            // name or a password that lost its comma
            (
                "P = http, h.test, 8080 alice, hunter2",
                "P = http, h.test, ***, ***",
            ),
            (
                "P = http, h.test, alice, hunter2",
                "P = http, h.test, ***, ***",
            ),
            (
                "P = http, h.test, tfo=true, alice",
                "P = http, h.test, tfo=true, ***",
            ),
            (
                "P = http, h.test, \"8080\", alice",
                "P = http, h.test, \"8080\", ***",
            ),
            // whatever the type word, a port in the third place
            (
                "P = Shadowsocks, host.test, 443, aes-128-gcm, \"hunter2\"",
                "P = Shadowsocks, host.test, 443, ***, ***",
            ),
            (
                "Old = custom, 1.2.3.4, 443, chacha20-ietf-poly1305, hunter2, https://example.com/SSEncrypt.module",
                "Old = custom, 1.2.3.4, 443, ***, ***, ***",
            ),
            ("select, A, B, 8080, C", "select, A, B, 8080, C"),
            // no port in the third place and no type that takes a server
            ("select, A, B, C, D", "select, A, B, C, D"),
            (
                "wireguard, section-name=Home",
                "wireguard, section-name=Home",
            ),
            ("direct, interface=eth0", "direct, interface=eth0"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A token is a named parameter when what stands before its `=` is a
    /// name; anything else is a value, the missing comma of `hunter2
    /// tfo=true` included.
    #[test]
    fn a_token_is_a_named_parameter_only_when_a_name_stands_before_its_equals_sign() {
        for (line, shown) in [
            (
                "P = http, h.test, 80, =hunter2",
                "P = http, h.test, 80, ***",
            ),
            (
                "P = http, h.test, 80, hunter2 tfo=true",
                "P = http, h.test, 80, ***",
            ),
            (
                "P = http, h.test, 80, aHVudGVyMg==",
                "P = http, h.test, 80, ***",
            ),
            (
                "P = http, h.test, 80, ab+c/d==, tfo=true",
                "P = http, h.test, 80, ***, tfo=true",
            ),
            (
                "P = http, h.test, 80, some.key=1, a:b=2",
                "P = http, h.test, 80, ***, ***",
            ),
            (
                "P = http, h.test, 80, (user=bob, hunter2)",
                "P = http, h.test, 80, ***",
            ),
            // a name has no blank inside, a name may be spaced from its `=`
            (
                "P = http, h.test, 80, tfo = true, TFO_2=1, ip-version=v4-only",
                "P = http, h.test, 80, tfo = true, TFO_2=1, ip-version=v4-only",
            ),
            ("P = http, h.test, 80, tfo=", "P = http, h.test, 80, ***"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A name starts a secret parameter after white space, a comma, a `(`, a
    /// quote — and after anything that is not ASCII: what an input method or a
    /// copy from a web page puts in front of it. A letter, a digit or the
    /// punctuation of a name or a URL makes it part of another word. After
    /// the name, `＝` is an `=`.
    #[test]
    fn a_name_is_found_behind_what_an_input_method_or_a_web_page_leaves_in_front() {
        for (line, shown) in [
            // a full-width comma, a zero-width space, a byte-order mark, a
            // typographic quote, a full-width parenthesis
            (
                "G = select, A\u{ff0c}policy-path=https://sub.test/n?token=t0k3n",
                "G = select, A\u{ff0c}policy-path=***",
            ),
            (
                "HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm\u{ff0c}password=x1",
                "HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm\u{ff0c}password=***",
            ),
            (
                "Ext = external, exec=/bin/x, local-port=1080, \u{200b}args=-p x1",
                "Ext = external, exec=/bin/x, local-port=1080, \u{200b}args=***",
            ),
            (
                "T = external, exec=/bin/x, \u{feff}password=x1",
                "T = external, exec=/bin/x, \u{feff}password=***",
            ),
            // on a type that takes a server it is a positional value, and goes
            (
                "T = trojan, t.test, 443, \u{feff}password=x1",
                "T = trojan, t.test, 443, ***",
            ),
            (
                "G = select, A, \u{201c}policy-path=https://sub.test/n\u{201d}",
                "G = select, A, \u{201c}policy-path=***",
            ),
            (
                "G = select, A\u{ff08}policy-path=https://sub.test/n\u{ff09}",
                "G = select, A\u{ff08}policy-path=***",
            ),
            // the full-width equals sign after the name
            (
                "Ext = external, exec=/bin/x, local-port=1080, args\u{ff1d}-p x1",
                "Ext = external, exec=/bin/x, local-port=1080, args\u{ff1d}***",
            ),
            (
                "Ext = external, exec=/bin/x, args \u{ff1d} -p x1, local-port=1080",
                "Ext = external, exec=/bin/x, args \u{ff1d} ***, local-port=1080",
            ),
            // still part of another word
            (
                "Ext = external, my-args=a, exec=/bin/args=x, icon-url=https://x.test/i.png?a=1&url=https://y.test/",
                "Ext = external, my-args=a, exec=/bin/args=x, icon-url=https://x.test/i.png?a=1&url=https://y.test/",
            ),
            (
                "Ext = external, x_args=a, 1args=b, .args=c, ?args=d, a/args=e",
                "Ext = external, x_args=a, 1args=b, .args=c, ?args=d, a/args=e",
            ),
            // a script's `argument` is where its token goes
            (
                "S = script, argument=cookie=abc, script-path=s.js",
                "S = script, argument=***, script-path=s.js",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// When the name stands right behind a quote, the value is the rest of the
    /// item: the quote is none the item pass could read as a whole. A quoted
    /// item that is whole is that pass's, and never gets here.
    #[test]
    fn behind_a_quote_the_value_runs_to_the_end_of_the_item() {
        // closed: the item pass reads it
        let shown = redact_definition(&format!("external, \"password={V},{W}\""));
        assert_eq!(shown, "external, password=***");
        // left open: the rest of the line is the item
        let shown = redact_definition(&format!("external, \"password={V},{W}"));
        assert_eq!(shown, "external, \"password=***");
        // a quote that does not start the item
        let shown = redact_definition(&format!("external, x\"password={V},{W}\", tfo=true"));
        assert_eq!(shown, "external, x\"password=***, tfo=true");
        let shown = redact_definition(&format!("external, 'x'password={V}, tfo=true"));
        assert_eq!(shown, "external, 'x'password=***, tfo=true");
    }

    /// The search by name runs only where a secret parameter can be named:
    /// before an `=`. What is left alone is returned as it is, and the result
    /// of the search is the same as without the look before it.
    #[test]
    fn the_search_by_name_is_not_run_where_no_secret_parameter_is_named() {
        let searches = |text: &str| {
            SEARCHES.with(|searches| searches.set(0));
            let shown = redact_by_name(text);
            (shown, SEARCHES.with(|searches| searches.get()))
        };
        for text in [
            "",
            "tfo=true",
            "a.test",
            "sni=a.test, tls=true",
            "https://h.test/?a=1&uid=x",
            "password is hidden",
            "a password, an args list = 1",
            "ws-path-x",
        ] {
            assert_eq!(searches(text), (text.to_string(), 0), "{text:?}");
        }
        // a name before an equals sign: the search runs, the case and the
        // white space between the two do not matter
        for (text, shown) in [
            ("password=x", "password=***"),
            ("PASSWORD \u{3000}= x", "PASSWORD \u{3000}= ***"),
            ("args\u{ff1d}x", "args\u{ff1d}***"),
            // it runs, and finds a name that is part of another word
            ("icon-url=x", "icon-url=x"),
        ] {
            assert_eq!(
                searches(text),
                (shown.to_string(), SECRET_PARAMS.len()),
                "{text:?}"
            );
        }
    }

    /// The user name and the password of a URL go wherever the URL stands: in
    /// a `[Host]` server the resolver uses, in a rule set or a script fetched
    /// with credentials, in a share link pasted as a line.
    #[test]
    fn the_user_info_of_a_url_goes_wherever_the_url_stands() {
        for (line, shown) in [
            (
                "corp.example = server:https://al1ce:hunter2@dns.example/dns-query",
                "corp.example = server:https://***@dns.example/dns-query",
            ),
            (
                "RULE-SET,https://al1ce:hunter2@rules.example/a.list,Proxy",
                "RULE-SET,https://***@rules.example/a.list,Proxy",
            ),
            (
                "S = script, script-path=https://al1ce:hunter2@h.test/s.js, type=cron",
                "S = script, script-path=https://***@h.test/s.js, type=cron",
            ),
            (
                "trojan://hunter2@t.example:443?sni=t.example#HK",
                "trojan://***@t.example:443?sni=t.example#HK",
            ),
            (
                "ss://YWVzLTEyOC1nY206cGFzcw@1.2.3.4:8388#name",
                "ss://***@1.2.3.4:8388#name",
            ),
            // the last `@` of the authority, and a `＠` that reads as one
            ("https://a@b@c.test/x", "https://***@c.test/x"),
            ("https://al1ce:pw＠c.test/x", "https://***@c.test/x"),
            // the authority ends at a `)`, a quote, a comma, white space
            ("(https://u:p@h.test)", "(https://***@h.test)"),
            ("\"https://u:p@h.test\" x", "\"https://***@h.test\" x"),
            ("a https://u:p@h.test b@c", "a https://***@h.test b@c"),
            // several in a line
            (
                "https://u:p@a.test/x https://v:q@b.test/y",
                "https://***@a.test/x https://***@b.test/y",
            ),
            // the authority ends at a `?`, a `#`, a comma, a quote or a `)` too
            ("https://h.test?x=a@b", "https://h.test?x=a@b"),
            ("https://h.test#a@b", "https://h.test#a@b"),
            ("https://u:p@h.test,x@y", "https://***@h.test,x@y"),
            ("\"https://u:p@h.test\"@x", "\"https://***@h.test\"@x"),
            ("'https://u:p@h.test'@x", "'https://***@h.test'@x"),
            ("(https://u:p@h.test)@x", "(https://***@h.test)@x"),
            // an `@` in a path, a query or a fragment is no user-info
            (
                "RULE-SET,https://h.test/a@b.list?mail=x@y.z#c@d,Proxy",
                "RULE-SET,https://h.test/a@b.list?mail=x@y.z#c@d,Proxy",
            ),
            (
                "script-path=https://h.test/s.js?u=a@b",
                "script-path=https://h.test/s.js?u=a@b",
            ),
            // in a comment too
            ("# was https://u:p@h.test/x", "# was https://***@h.test/x"),
            // nothing to hide
            ("https://h.test/dns-query", "https://h.test/dns-query"),
            ("://", "://"),
            ("a://", "a://"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
        // and in a definition alone, where a policy's view and its hash get it
        assert_eq!(
            redact_definition("external, exec=https://al1ce:hunter2@h.test/x, local-port=1080"),
            "external, exec=https://***@h.test/x, local-port=1080"
        );
    }
}
