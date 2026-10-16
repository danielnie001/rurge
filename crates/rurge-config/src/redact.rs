//! Profile text redaction for `GET /v1/profiles/current?sensitive=0` (M4
//! design §4.3): secrets become `***`, everything else (including line count)
//! is preserved so line numbers in diagnostics still line up.
//!
//! A line is read the way the section parser (`text::parse_str`) reads it —
//! with its own functions, in its order — and what it hands to the line
//! parsers is what gets redacted; what it takes off stays as written. That
//! includes a line it reads as a comment: a commented-out line is where old
//! secrets are left, and it is redacted as if it were not commented out. So
//! is the inline comment at the end of a line. A link noted in a comment is
//! the subscription that `#!MANAGED-CONFIG` and `policy-path` hide, and shows
//! its scheme and its authority only.

use crate::policy::PolicyKind;
use crate::requirement::{is_requirement_prefix, split_line_slice};
use crate::text::{directive_name, is_comment, strip_inline_comment};
use crate::value::{item_spans, parse_key_value, split_list};

/// Keys whose whole value is a secret when they stand alone on a line
/// (`[WireGuard]` `private-key`, `[Snell Server]` `psk`, `[Tailscale]`
/// `auth-key`, …). `[General]`'s two test URLs are among them as a policy's
/// `test-url` is among the inline parameters: a profile that came from a
/// provider can carry a token in them. So is `geoip-maxmind-url`: MaxMind's
/// own download link carries the account's licence key in its query. A key is
/// compared without regard to its case, its `-` and its `_`: wg-quick writes
/// `PrivateKey` and `PresharedKey`, the manual `preshared-key`. (The two
/// spellings of a peer's key are one under that comparison; the list names
/// both so that it reads as the manuals do.) `secret` is `[MTProto]`'s key: a
/// section rurge keeps as written, whose lines reach the redacted profile and
/// the `Debug` of `Config`. `passphrase` is what `[MITM]`'s `ca-passphrase` is
/// called by anyone who writes it short.
const SECRET_KEYS: [&str; 14] = [
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
    "secret",
    "passphrase",
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
/// cookie is put. `passphrase` is what people write on a `[Keystore]` line
/// where the field is `password` (`[MITM]`'s key is `ca-passphrase`).
/// Over-redacting is the safe side for an endpoint whose purpose is safe
/// output.
const SECRET_PARAMS: [&str; 24] = [
    "password",
    "passphrase",
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

/// How many comments inside comments `redact_text` follows (`a # b # c …`);
/// deeper than this the search by name and the links still run, and a line of
/// a hundred thousand ` # x` costs a few levels, not a stack.
const MAX_COMMENT_DEPTH: usize = 4;

fn redact_body(line: &str) -> String {
    redact_text(line, 0)
}

/// `line` redacted as a line of the profile; `depth` is how many comments
/// inside comments it is in.
fn redact_text(line: &str, depth: usize) -> String {
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
        redact_content(rest, depth)
    };
    // a comment is free text: a secret named in it goes too
    shown.push_str(&if commented {
        redact_by_name(&part)
    } else {
        part
    });
    let shown = hide_url_credentials(&shown);
    if commented {
        hide_comment_links(&shown)
    } else {
        shown
    }
}

/// Whether the section parser takes `line` for a directive of its own, whose
/// text it keeps whole: it starts with `#!` and is neither an include nor a
/// requirement directive.
fn is_directive_line(line: &str) -> bool {
    line.starts_with("#!") && !line.starts_with("#!include") && !is_requirement_prefix(line)
}

/// An entry's line: the inline comment and a requirement directive at its end
/// are taken off as the parser takes them off; what is left is the content the
/// line parsers get. The directive is kept as written but for a secret named
/// in it, the comment — its marker as written — is redacted as a
/// commented-out line is. A key whose value is empty is the exception: the
/// section parser takes the secret that starts with `#`, `;` or `//` for a
/// comment, and the comment is where it is.
fn redact_content(line: &str, depth: usize) -> String {
    let content = strip_inline_comment(line);
    let (start, end) = match split_line_slice(content) {
        Ok((_, body)) => match offset_in(content, body) {
            Some(start) => (start, start + body.len()),
            None => (0, content.len()),
        },
        Err(_) => (0, content.len()),
    };
    let core = &line[start..end];
    let comment = &line[content.len()..];
    let comment = if comment.is_empty() {
        String::new()
    } else if secret_key_without_value(core) {
        hide_comment_text(comment)
    } else if depth < MAX_COMMENT_DEPTH {
        redact_text(comment, depth + 1)
    } else {
        hide_comment_links(&redact_by_name(comment))
    };
    format!(
        "{}{}{}{comment}",
        &line[..start],
        redact_core(core),
        redact_by_name(&line[end..content.len()])
    )
}

/// Whether `core`, the content of a line, is a key whose value holds a secret
/// with nothing written after its separator.
fn secret_key_without_value(core: &str) -> bool {
    key_line(core)
        .is_some_and(|k| is_secret_key(k.key) && k.value.trim_matches(is_blank).is_empty())
}

/// `comment`, an inline comment — blanks, its marker, its text — with the text
/// replaced by `***`: the marker and the blanks stay as written.
fn hide_comment_text(comment: &str) -> String {
    let (lead, body) = comment.split_at(comment.len() - comment.trim_start_matches(is_blank).len());
    let marker = if body.starts_with("//") {
        2
    } else {
        body.chars().next().map_or(0, char::len_utf8)
    };
    let (marker, text) = body.split_at(marker);
    let (gap, text) = text.split_at(text.len() - text.trim_start_matches(is_blank).len());
    let words = text.trim_end_matches(is_blank);
    if words.is_empty() {
        return comment.to_string();
    }
    format!("{lead}{marker}{gap}***{}", &text[words.len()..])
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

/// The lists of keys that stand on a line of their own, by what is done to the
/// value that follows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    /// `SECRET_KEYS`: the whole value is a secret.
    Secret,
    /// `wifi-access-http-auth`: `user:password`.
    Wifi,
    /// `CONTROLLER_KEYS`: `secret@address`.
    Controller,
    /// `LISTENER_KEYS`: a list of `[secret@]address`.
    Listener,
    /// `DNS_KEYS`: a list of servers, whose URLs may carry credentials.
    Dns,
}

/// What may stand in front of a name without making it part of another word,
/// in the search by name and for a key alike: white space, a comma, a `(`, a
/// quote, and any character that is not ASCII — a full-width comma, a
/// zero-width space or a byte-order mark brought along from a web page, a
/// typographic quote. A letter, a digit or the punctuation of a name or a URL
/// in front of it makes it part of another word (`my-args=`, `exec=/bin/args=x`,
/// `?a=1&url=…`).
fn is_name_boundary(c: char) -> bool {
    c.is_ascii_whitespace() || matches!(c, ',' | '(' | '"' | '\'') || !c.is_ascii()
}

/// The characters a name is made of.
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
}

/// Which list the key of a line is in, and whether nothing but blanks stands
/// around its name. The key is the text in front of a separator, and it ends
/// with a listed one: the run of name characters at its end — blanks and one
/// closing quote after it not counted — is compared as `is_key` compares, and
/// that run starts the text or stands behind a boundary of the name search.
/// What is in front of the key is whatever a comment, an input method or
/// another client leaves there (`# was http-api`, `；ca-passphrase`,
/// `"password"`).
fn listed_key(text: &str) -> Option<(KeyKind, bool)> {
    let key = text.trim_end_matches(is_blank);
    let key = key.strip_suffix(['"', '\'']).unwrap_or(key);
    let key = key.trim_end_matches(is_blank);
    let front = key.trim_end_matches(is_name_char);
    let name = &key[front.len()..];
    if name.is_empty() || !front.chars().next_back().is_none_or(is_name_boundary) {
        return None;
    }
    let kind = if is_key(&SECRET_KEYS, name) {
        KeyKind::Secret
    } else if is_key(&["wifi-access-http-auth"], name) {
        KeyKind::Wifi
    } else if is_key(&CONTROLLER_KEYS, name) {
        KeyKind::Controller
    } else if is_key(&LISTENER_KEYS, name) {
        KeyKind::Listener
    } else if is_key(&DNS_KEYS, name) {
        KeyKind::Dns
    } else {
        return None;
    };
    Some((kind, text.trim_matches(is_blank) == name))
}

/// Whether `key` — the text in front of a separator — is one whose value
/// holds a secret: a key of `SECRET_KEYS`, `CONTROLLER_KEYS` or
/// `LISTENER_KEYS`, or `wifi-access-http-auth`, read as the redaction reads it
/// (something may stand in front of the key's name). The value of a DNS key is
/// a list of servers whose credentials go one by one, and is not one of them.
pub(crate) fn is_secret_key(key: &str) -> bool {
    listed_key(key).is_some_and(|(kind, _)| kind != KeyKind::Dns)
}

/// `listed_key` for the key of a line. A DNS key counts only when nothing but
/// blanks stands around it: with text in front of it the line is another kind
/// of line — an `[SSID Setting]` line is `<subnet expression> key=value,
/// key=value`, and `encrypted-dns-server=https://…` is a parameter there, not
/// a server — and is redacted as a definition, whose URLs lose their user-info
/// wherever they stand.
fn line_key(text: &str) -> Option<(KeyKind, bool)> {
    listed_key(text).filter(|&(kind, alone)| alone || kind != KeyKind::Dns)
}

/// A line that is `<text><key><separator><value>` with a listed key.
struct KeyLine<'a> {
    kind: KeyKind,
    /// Whether nothing but blanks stands around the key's name.
    alone: bool,
    /// The text in front of the separator, as written.
    key: &'a str,
    sep: &'a str,
    value: &'a str,
}

/// The line as `<key><separator><value>`, when its key is a listed one. The
/// separator is the first `=` — or `＝`, which an input method types — behind a
/// key that has no comma in front of it: a text with a comma in front of the
/// `=` is no key. A `:` — or `：` — is a separator too, in front of the first
/// `=`: a text that holds an `=` is no key either, so a colon in a value is not
/// one. A DNS key with text in front of it is no key (`line_key`): the search
/// goes on to the next colon, and behind the `=` there is none.
fn key_line(line: &str) -> Option<KeyLine<'_>> {
    let equals = line.find(['=', '＝']);
    let colons = line[..equals.unwrap_or(line.len())]
        .char_indices()
        .filter(|(_, c)| matches!(c, ':' | '：'));
    for (at, colon) in colons {
        if let Some((kind, alone)) = line_key(&line[..at]) {
            let (key, rest) = line.split_at(at);
            let (sep, value) = rest.split_at(colon.len_utf8());
            return Some(KeyLine {
                kind,
                alone,
                key,
                sep,
                value,
            });
        }
    }
    let at = equals?;
    let key = &line[..at];
    // a comma — or a `，`, which an input method types for one — in front of
    // the `=`: the text is no key. It keeps today's reading only (line-splitting
    // design 6.1): with the parsers' reading as well, `a'b, http-listen = …`
    // would no longer be a key line, and its password would stay where it is.
    if next_comma(key, true).is_some() {
        return None;
    }
    let (kind, alone) = line_key(key)?;
    let width = line[at..].chars().next().map_or(1, char::len_utf8);
    let (sep, value) = line[at..].split_at(width);
    Some(KeyLine {
        kind,
        alone,
        key,
        sep,
        value,
    })
}

/// The line `key_line` found, with its value hidden as its kind says.
fn redact_key_line(entry: &KeyLine<'_>) -> String {
    let KeyLine {
        kind,
        alone,
        key,
        sep,
        value,
    } = *entry;
    match kind {
        KeyKind::Secret if alone => format!("{key}{sep} ***"),
        KeyKind::Secret => {
            // something stands in front of the key — a comment's words, a
            // quote, a stray mark: its value is hidden where it stands
            let body = value.trim_start();
            let lead = &value[..value.len() - body.len()];
            let trail = &body[body.trim_end().len()..];
            format!("{key}{sep}{lead}***{trail}")
        }
        KeyKind::Wifi => {
            let v = value.trim_start();
            let lead = &value[..value.len() - v.len()];
            match v.split_once(':') {
                Some((user, _)) => format!("{key}{sep}{lead}{user}:***"),
                None => format!("{key}{sep}{lead}***"),
            }
        }
        KeyKind::Controller => format!("{key}{sep}{}", hide_controller(value)),
        KeyKind::Listener => format!("{key}{sep}{}", hide_listed(value, without_credentials)),
        KeyKind::Dns => format!("{key}{sep}{}", hide_listed(value, url_without_credentials)),
    }
}

/// `key = value` lines: the keys that are secrets, the lists of addresses
/// that may hold a user name and a password, and the definitions of policies.
/// The key ends at the first `=` — or `＝`, which an input method types; a
/// text in front of it that holds a comma is no key, and neither is anything
/// when there is no `=`: the line is then a definition. A listed key has `:` —
/// or `：` — for a separator too, as `key_line` reads it.
fn redact_entry(line: &str) -> String {
    if let Some(entry) = key_line(line) {
        return redact_key_line(&entry);
    }
    let Some(at) = line.find(['=', '＝']) else {
        return redact_definition(line);
    };
    let key = &line[..at];
    // a comma in front of the `=`, by the parsers' reading or by today's
    // (`next_comma`, which also takes a quote in the middle of a value for an
    // opening one): redacting the line both ways only hides more
    if next_comma(key, true).is_some() || item_spans(key, true).len() > 1 {
        return redact_both_ways(line);
    }
    let width = line[at..].chars().next().map_or(1, char::len_utf8);
    let (sep, value) = line[at..].split_at(width);
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
/// of it. The items are the parsers' own, taken in regions (design LS-D9): a
/// region of several items is shown as one, which hides at least as much.
fn hide_listed(value: &str, shown: fn(&str) -> String) -> String {
    let mut out = String::with_capacity(value.len());
    for r in regions(value) {
        let item = &value[r.start..r.end];
        let t = item.trim_start();
        out.push_str(&item[..item.len() - t.len()]);
        out.push_str(&shown(t));
        out.push_str(&value[r.end..r.sep_end]);
    }
    out
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
/// path stays. The targets are taken in regions (design LS-D9), and a region
/// with a link in it goes whole.
fn hide_include_links(targets: &str) -> String {
    let mut out = String::with_capacity(targets.len());
    for r in regions(targets) {
        let target = &targets[r.start..r.end];
        if target.contains("://") {
            let rest = target.trim_start();
            let lead = &target[..target.len() - rest.len()];
            let trail = &rest[rest.trim_end().len()..];
            out.push_str(lead);
            out.push_str("***");
            out.push_str(trail);
        } else {
            out.push_str(target);
        }
        out.push_str(&targets[r.end..r.sep_end]);
    }
    out
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

/// The schemes a parser of this crate reads: a rule set, a script or an
/// include (`http`, `https`), a DNS server (`https`, `h3`, `quic`, `tls`,
/// `tcp`).
const READ_SCHEMES: [&str; 6] = ["http", "https", "h3", "quic", "tls", "tcp"];

/// The scheme of the URL whose `://` ends `head`: the run of `[A-Za-z0-9+.-]`
/// right in front of it, less what leads it and is no ASCII letter — a scheme
/// begins with one (RFC 3986, section 3.1), and the `+` of `[a-z]+://` or the
/// `.+` of `^.+://` is a regular expression's. Empty for a regular expression
/// (`^https?://`) and when nothing is left (`^.+://`).
fn scheme_before(head: &str) -> &str {
    let front =
        head.trim_end_matches(|c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    head[front.len()..].trim_start_matches(|c: char| !c.is_ascii_alphabetic())
}

/// Where the authority of a URL ends: a `/`, a `?`, a `#`, a comma, a quote, a
/// `)`, white space.
fn is_authority_end(c: char) -> bool {
    matches!(c, '/' | '?' | '#' | ',' | '"' | '\'' | ')') || c.is_whitespace()
}

/// `text` with the user name and the password of every URL in it gone: what
/// stands before the last `@` of the authority that follows a `://` — which
/// ends at a `/`, a `?`, a `#`, a comma, a quote, a `)`, white space or the
/// end. An `@` further on is part of a path or a query and stays. A `[Host]`
/// server, a rule set or a script fetched with credentials, a share link
/// pasted as a line: wherever the URL stands.
///
/// A URL whose scheme no parser of this crate reads (`READ_SCHEMES`; a
/// regular expression, whose `://` has no scheme in front, is none) shows
/// nothing of what follows its `://` up to the next white space: a share
/// link carries its secret where this function does not look
/// (`vmess://<base64>`, `hysteria://host?auth=…`). The cost is known: a
/// rewrite target with an app's scheme shows as `myapp://***`.
fn hide_url_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, after) = rest.split_at(at + 3);
        out.push_str(head);
        let scheme = scheme_before(&head[..at]);
        if !scheme.is_empty() && !READ_SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(scheme)) {
            let end = after.find(char::is_whitespace).unwrap_or(after.len());
            if end > 0 {
                out.push_str("***");
            }
            rest = &after[end..];
            continue;
        }
        let end = after.find(is_authority_end).unwrap_or(after.len());
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

/// `text`, a comment, with every URL of a scheme a parser reads cut after its
/// authority: what follows up to the next white space, when anything does,
/// becomes `/***`. A link noted in a comment — a subscription's, a rule set's
/// — is the secret that `#!MANAGED-CONFIG` and `policy-path` hide. It hides
/// more than it has to, on purpose (`# https://github.com/owner/repo` shows
/// as `# https://github.com/***`); the same URL on a line that is no comment
/// stays whole, a rule set being named by its URL. The user-info of the
/// authority is `hide_url_credentials`', which runs first, and so is every
/// other scheme.
fn hide_comment_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, after) = rest.split_at(at + 3);
        out.push_str(head);
        let scheme = scheme_before(&head[..at]);
        if !READ_SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(scheme)) {
            rest = after;
            continue;
        }
        let end = after.find(is_authority_end).unwrap_or(after.len());
        let (authority, tail) = after.split_at(end);
        out.push_str(authority);
        let link = tail.find(char::is_whitespace).unwrap_or(tail.len());
        if link > 0 {
            out.push_str("/***");
        }
        rest = &tail[link..];
    }
    out.push_str(rest);
    out
}

/// A policy definition (the text right of `name =`) with its secrets blanked:
/// every secret `name=value` parameter, however the line spells it, and the
/// positional values of every type that takes a server and a port. The items
/// go first, on the text as written, and the positional values then on what
/// that made of it (design LS-D9).
pub fn redact_definition(definition: &str) -> String {
    hide_url_credentials(&redact_positional_credentials(&redact_items(definition, 0)))
}

/// One region of a list: where it starts, where its first item ends, where
/// its last item ends and where the separator after that one ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Region {
    start: usize,
    first_end: usize,
    end: usize,
    sep_end: usize,
}

/// Where the regions of `text` are (design LS-D9): a region is an item of the
/// parsers' (`value::item_spans`) together with the items that follow it as
/// long as today's reading, from the item's start (`next_comma`), runs past
/// their end — a quote or a parenthesis in the middle of a value guarded a
/// comma, and its writer may have meant it to. A region ends where the later
/// of the two readings ends, moved on to the parsers' next cut; the next
/// region starts with the item after it. Every scan stops at its own region,
/// so a text is read twice at most.
fn regions(text: &str) -> Vec<Region> {
    let spans = item_spans(text, false);
    let mut out = Vec::new();
    let mut i = 0;
    while i < spans.len() {
        let start = spans[i].start;
        let reach = next_comma(&text[start..], false).map_or(text.len(), |(at, _)| start + at);
        let mut j = i;
        while j + 1 < spans.len() && spans[j].end < reach {
            j += 1;
        }
        out.push(Region {
            start,
            first_end: spans[i].end,
            end: spans[j].end,
            sep_end: spans[j].sep_end,
        });
        i = j + 1;
    }
    out
}

/// How many parenthesised groups deep `redact_items` follows. A `[WireGuard]`
/// peer is one and no parser reads a group inside a group as anything but a
/// value; deeper than this the search by name still runs, and a line of
/// ten thousand parentheses costs a few levels, not a stack.
const MAX_GROUP_DEPTH: usize = 4;

/// `text`, a list of items, with every item redacted. The items are the
/// parsers' own (`value::item_spans`); an item whose value today's reading
/// (`next_comma`) carries past the item's end is redacted together with the
/// items it carries along — a region (design LS-D9). The separators between
/// the regions stay.
fn redact_items(text: &str, depth: usize) -> String {
    let mut out = String::with_capacity(text.len());
    for r in regions(text) {
        let body = &text[r.start..r.end];
        if r.first_end == r.end {
            out.push_str(&redact_item(body, depth));
        } else {
            out.push_str(&redact_joined(body, r.first_end - r.start));
        }
        out.push_str(&text[r.end..r.sep_end]);
    }
    out
}

/// A region of several items (design LS-D9): when the first is a secret
/// parameter, its value is hidden together with the items the region takes
/// along; otherwise every secret it names is searched for by name across the
/// whole region.
fn redact_joined(region: &str, first_len: usize) -> String {
    let (lead, body, trail) = blanks_around(region);
    let first = split_list(&region[..first_len]).pop();
    hide_value(
        lead,
        body,
        trail,
        first.as_deref().and_then(secret_key),
        || redact_by_name(region),
    )
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
    let (lead, body, trail) = blanks_around(item);
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
    hide_value(lead, body, trail, secret_key(&field), || {
        redact_by_name(item)
    })
}

/// `text` as the blanks in front of it, what it holds, and the blanks behind
/// it.
fn blanks_around(text: &str) -> (&str, &str, &str) {
    let rest = text.trim_start();
    let lead = &text[..text.len() - rest.len()];
    let body = rest.trim_end();
    let trail = &rest[body.len()..];
    (lead, body, trail)
}

/// The key of `field`, an item as `split_list` gives it, when it is a secret
/// parameter's.
fn secret_key(field: &str) -> Option<&str> {
    parse_key_value(field).map(|(key, _)| key).filter(|key| {
        SECRET_PARAMS
            .iter()
            .any(|param| key.eq_ignore_ascii_case(param))
    })
}

/// `body`, with `lead` and `trail` around it as written, with the value of
/// the secret parameter `key` hidden: written plainly the name keeps its
/// spacing, otherwise only `key=***` is left. Without a secret key, what
/// `by_name` makes of it.
fn hide_value(
    lead: &str,
    body: &str,
    trail: &str,
    key: Option<&str>,
    by_name: impl FnOnce() -> String,
) -> String {
    match (key, body.split_once('=')) {
        (Some(key), Some((name, value))) if name.trim() == key => {
            let blank = &value[..value.len() - value.trim_start().len()];
            format!("{lead}{name}={blank}***{trail}")
        }
        (Some(key), _) => format!("{lead}{key}=***{trail}"),
        (None, _) => by_name(),
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

/// One field of a policy line as `redact_positional_credentials` counts it:
/// where it starts, ends and where the separator after it ends, and whether
/// it is one of the words a first field was split into
/// (`words_of_type_field`).
#[derive(Clone, Copy)]
struct FieldSpan {
    start: usize,
    end: usize,
    sep_end: usize,
    word: bool,
}

/// Blanks the positional tokens of a policy line: every field from the fourth
/// on (the first three are the type, the server and the port) — that is where
/// Surge puts `username, password`. It is done for the types that take a
/// server and a port, and for any line whose third field is a port number: a
/// line written for another client (Loon's `Shadowsocks, host, 443, method,
/// "password"`) is an unknown type to rurge, and its password stands in the
/// fifth place all the same; Surge's old `custom, host, 443, method,
/// password, module` is an `ss` line, whose password stands there too. For a
/// type that takes a server, what stands in the port's place and is no
/// number is a user name or a password that lost its comma (`8080 alice,
/// hunter2`), and goes too. The fields are the parser's
/// (`value::item_spans`, with `，` cutting too — design LS-D10): `split_list`
/// reads the type and the port with the quotes off, and an empty item is
/// none; a field that is blanked goes together with the items today's
/// reading takes along with it (`next_comma`, design LS-D9). A token counts as
/// a named parameter (and is kept) only when what stands before its first `=`
/// is a name and what follows it is non-empty and not made only of `=`, so
/// base64 padding (`aHVudGVyMg==`) is redacted while `tfo=true` survives, and
/// so is a password with a missing comma in front of a parameter (`hunter2
/// tfo=true`); `sni=` with an empty value is over-redacted. A first field of
/// several words whose first word is a type that takes a server is that type
/// with the commas after it missing (`http h.example, 8080, alice, hunter2`):
/// its words are fields, the server second and the port third — when a port
/// number stands there, `words_of_type_field` says. Any other line is
/// returned unchanged.
fn redact_positional_credentials(value: &str) -> String {
    // the fields as the parsers read them. A line typed with an input method
    // may have `，` for some of its commas: it is read here as the author
    // meant it (design LS-D10), and the separators stay
    let spans: Vec<FieldSpan> = item_spans(value, true)
        .into_iter()
        .map(|span| FieldSpan {
            start: span.start,
            end: span.end,
            sep_end: span.sep_end,
            word: false,
        })
        .collect();
    let spans = words_of_type_field(value, spans);
    // the fields the parser reads, an empty item being none; only as many as
    // the decision needs are looked at
    let mut read = spans
        .iter()
        .filter_map(|span| split_list(&value[span.start..span.end]).pop());
    let takes_server = read
        .next()
        .is_some_and(|keyword| takes_server_type(&keyword));
    let port_third = read
        .nth(1)
        .is_some_and(|field| field.trim_matches(is_blank).parse::<u16>().is_ok());
    if !takes_server && !port_third {
        return value.to_string();
    }
    let mut index = 0usize;
    let mut out = String::with_capacity(value.len());
    let mut k = 0usize;
    while k < spans.len() {
        let span = spans[k];
        let tok = &value[span.start..span.end];
        let blanked = match split_list(tok).pop() {
            Some(field) => {
                index += 1;
                let t = tok.trim_start();
                let stray_port = index == 3
                    && takes_server
                    && field.trim_matches(is_blank).parse::<u16>().is_err();
                (index > 3 || stray_port) && !is_named_param(t.trim_end())
            }
            None => false,
        };
        if !blanked {
            out.push_str(&value[span.start..span.sep_end]);
            k += 1;
            continue;
        }
        let t = tok.trim_start();
        out.push_str(&tok[..tok.len() - t.len()]);
        out.push_str("***");
        // the items today's reading takes along with the field (design
        // LS-D9) go with it; a word of the first field takes none (the blanks
        // between the words are no commas). They need no counting: the field
        // is past the port, and so is every field after it
        let mut last = k;
        if !span.word {
            let reach = next_comma(&value[span.start..], true)
                .map_or(value.len(), |(at, _)| span.start + at);
            while last + 1 < spans.len() && spans[last].end < reach {
                last += 1;
            }
        }
        out.push_str(&value[spans[last].end..spans[last].sep_end]);
        k = last + 1;
    }
    out
}

/// Whether `keyword`, a field read as a policy's type, is one that takes a
/// server and a port.
fn takes_server_type(keyword: &str) -> bool {
    PolicyKind::parse(keyword.trim_matches(is_blank)).is_some_and(|kind| kind.takes_server())
}

/// `spans`, the fields of a policy definition, with the first of them — when
/// it is several words, the first a type that takes a server — split into its
/// words: the white space between them is their separator, and the last word
/// keeps the separator the field had.
///
/// It is split only when a port number stands where the third field is after
/// the split (the third word, or the next field when the first has two, read
/// as `redact_positional_credentials` reads the port): a comment that opens
/// with a protocol word is prose and not a policy line that lost its commas
/// (`# HTTP DNS blocking list`, `// ss node via Hong Kong`). The cost is
/// known and accepted: a line that lost the comma after its type and has no
/// port number in the third place (`P = http h.example, 8080 alice, hunter2`)
/// is not read as a policy line by this rule.
fn words_of_type_field(value: &str, spans: Vec<FieldSpan>) -> Vec<FieldSpan> {
    let Some(first) = spans
        .iter()
        .position(|span| split_list(&value[span.start..span.end]).pop().is_some())
    else {
        return spans;
    };
    let FieldSpan {
        start,
        end,
        sep_end,
        ..
    } = spans[first];
    let field = split_list(&value[start..end]).pop().unwrap_or_default();
    let mut words = field.split(char::is_whitespace).filter(|w| !w.is_empty());
    if !(words.next().is_some_and(takes_server_type) && words.next().is_some()) {
        return spans;
    }
    let mut split = spans[..first].to_vec();
    let mut at = start;
    loop {
        let rest = &value[at..end];
        let lead = rest.len() - rest.trim_start().len();
        let word = lead
            + rest[lead..]
                .find(char::is_whitespace)
                .unwrap_or(rest.len() - lead);
        let gap = rest[word..].len() - rest[word..].trim_start().len();
        if word + gap >= rest.len() {
            split.push(FieldSpan {
                start: at,
                end: at + word,
                sep_end,
                word: true,
            });
            break;
        }
        split.push(FieldSpan {
            start: at,
            end: at + word,
            sep_end: at + word + gap,
            word: true,
        });
        at += word + gap;
    }
    split.extend_from_slice(&spans[first + 1..]);
    let port_third = split
        .iter()
        .filter_map(|span| split_list(&value[span.start..span.end]).pop())
        .nth(2)
        .is_some_and(|field| field.trim_matches(is_blank).parse::<u16>().is_ok());
    if port_third { split } else { spans }
}

/// The first comma that is inside no quote and no parenthesised group, by the
/// reading of lists before the line-splitting design: a `"` or a `'` opens a
/// quote **wherever** it appears, inside `"` a backslash escapes the next
/// character, inside `'` nothing escapes, and `(` / `)` nest outside quotes.
/// `None` when there is none — an unterminated quote or group therefore runs
/// to the end of the line, which is the safe side for redaction. The parsers
/// no longer read this way (`value::item_spans`); redaction keeps it to
/// measure how far a value it hides reaches (design LS-D9).
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

/// How long a parameter's value is: up to the first comma today's reading
/// would split on (`next_top_level_comma`) — quotes may open anywhere inside
/// the value and parentheses group, so `ab"c,d"` and `a(b,c)d` are each one
/// value, as their writer may have meant them. When the scan starts inside a
/// group (a secret nested in a `peer = (…)`), depth starts at 0 there, so the
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
    // before a separator — one closing quote and white space between the two
    // do not count — and most items, comments and rules have none that does.
    // The look must hold wherever `redact_param` finds a name: its conditions
    // stand below, side by side with these
    let lower = text.to_ascii_lowercase();
    // `＝` and `：` start with the byte 0xEF, which only starts a character
    let names_one = lower.bytes().enumerate().any(|(at, b)| {
        (matches!(b, b'=' | b':') || (b == 0xEF && lower[at..].starts_with(['＝', '：']))) && {
            let head = lower[..at].trim_end();
            let head = head.strip_suffix(['"', '\'']).unwrap_or(head);
            SECRET_PARAMS.iter().any(|param| head.ends_with(param))
        }
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
/// …)`), where no top-level split would reach it. The separator is `=` or `:`
/// — and `＝` or `：`, which an input method types: a person notes a secret as
/// `password: x`, and a Clash or sing-box line has `password: x` and
/// `"password": "x"`.
fn redact_param(value: &str, param: &str) -> String {
    #[cfg(test)]
    SEARCHES.with(|searches| searches.set(searches.get() + 1));
    let lower = value.to_ascii_lowercase();
    let mut out = String::with_capacity(value.len());
    let mut rest = 0usize;
    let mut search = 0usize;
    while let Some(pos) = lower[search..].find(param) {
        let start = search + pos;
        // must be at a token boundary (`is_name_boundary`) followed by one
        // closing quote, if any, optional white space and a separator
        let before = lower[..start].chars().next_back();
        let before_ok = before.is_none_or(is_name_boundary);
        let after = &value[start + param.len()..];
        let after = after.strip_prefix(['"', '\'']).unwrap_or(after);
        let eq = after.trim_start().strip_prefix(['=', '＝', ':', '：']);
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

    /// A quote or a parenthesis in the middle of a value opens nothing for
    /// the parser (design LS-D1, LS-D3), but its writer may have meant it to
    /// guard the comma: the value is hidden as far as today's reading takes
    /// it, the items it cut off included — a region (design LS-D9).
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
            // a quote in the middle of a value opens nothing (design LS-D1):
            // the parser reads `a"{V}` and the next item is `{W}"b`
            spelling(format!("{name}=a\"{both}\"b"), &format!("a\"{V}")),
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
                                // what the parser read into the parameter: a
                                // part it cut off is an item of its own — on a
                                // group line a member, which the group's
                                // `Debug` lists (design 6.3)
                                for secret in [V, W] {
                                    if spelling.value.contains(secret) && debug.contains(secret) {
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
            // a quote in the middle of the value opens nothing for the parser
            // (design LS-D1), but the value is hidden as far as today's
            // reading takes it; a quote left open where a value starts takes
            // the rest of the line, for the parser too
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
                "// see https://example.com/*** for more",
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

    /// `#!WINDOWS-ONLY` and `#!LINUX-ONLY` are requirement directives like the
    /// other three, in front of a line and at its end: kept as written, and the
    /// line they stand on is redacted as any other.
    #[test]
    fn the_windows_and_linux_tags_are_requirement_directives_too() {
        for (line, shown) in [
            (
                "#!WINDOWS-ONLY http-api = key@127.0.0.1:6170",
                "#!WINDOWS-ONLY http-api = ***@127.0.0.1:6170",
            ),
            (
                "#!LINUX-ONLY http-api = key@127.0.0.1:6170",
                "#!LINUX-ONLY http-api = ***@127.0.0.1:6170",
            ),
            (
                "#!WINDOWS-ONLY P = http, h.test, 80, alice, hunter2",
                "#!WINDOWS-ONLY P = http, h.test, 80, ***, ***",
            ),
            (
                "http-api = key@127.0.0.1:6170 #!WINDOWS-ONLY",
                "http-api = ***@127.0.0.1:6170 #!WINDOWS-ONLY",
            ),
            (
                "P = http, h.test, 80, alice, hunter2 //!LINUX-ONLY",
                "P = http, h.test, 80, ***, *** //!LINUX-ONLY",
            ),
            // the tag alone, and one in front of a line with nothing to hide
            ("#!LINUX-ONLY", "#!LINUX-ONLY"),
            (
                "#!WINDOWS-ONLY RULE-SET,https://example.com/a.list,Proxy",
                "#!WINDOWS-ONLY RULE-SET,https://example.com/a.list,Proxy",
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
            // a `:` is a separator for every listed key, and for no other
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
                "P = Shadowsocks, h.test,\u{200b}443, alice, hunter2",
                "P = Shadowsocks, h.test,\u{200b}443, ***, ***",
            ),
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
                "Old = Shadowsocks, 1.2.3.4, 443, chacha20-ietf-poly1305, hunter2, https://example.com/SSEncrypt.module",
                "Old = Shadowsocks, 1.2.3.4, 443, ***, ***, ***",
            ),
            // Surge's old Shadowsocks line is an `ss` line to rurge: it takes a
            // server, so what stands in its port's place and is no number
            // goes too
            (
                "Old = custom, 1.2.3.4, 443, chacha20-ietf-poly1305, hunter2, https://example.com/SSEncrypt.module",
                "Old = custom, 1.2.3.4, 443, ***, ***, ***",
            ),
            (
                "P = custom, h.test, 443 aes-128-gcm, hunter2",
                "P = custom, h.test, ***, ***",
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

    /// The positional values are counted the way the parser counts its
    /// fields (line-splitting design 6.3): an apostrophe or a parenthesis
    /// that pairs with nothing in a server's name no longer hides the
    /// password behind it, and a group's members stay.
    #[test]
    fn positional_credentials_are_counted_as_the_parser_counts_its_fields() {
        for (line, shown) in [
            (
                "X = http, Bob's-host, 80, alice, pw",
                "X = http, Bob's-host, 80, ***, ***",
            ),
            (
                "X = http, Proxy (US-host, 80, alice, pw",
                "X = http, Proxy (US-host, 80, ***, ***",
            ),
            (
                "G = select, Tom's HK, policy-path=https://sub.example/list?token=t0k3n",
                "G = select, Tom's HK, policy-path=***",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// Design 6.2: a region starts with an item of the parser's, ends where
    /// the later of the two readings ends — moved on to the parser's next
    /// cut — and the regions cover the text. Every string of up to six
    /// characters the reading looks at.
    #[test]
    fn a_region_ends_where_the_later_of_the_two_readings_ends() {
        let mut checked = 0usize;
        for text in crate::value::short_strings() {
            let spans = item_spans(&text, false);
            let mut at = 0;
            for r in regions(&text) {
                assert_eq!(r.start, at, "{text:?}");
                let first = spans
                    .iter()
                    .position(|s| s.start == r.start)
                    .unwrap_or_else(|| panic!("{text:?}"));
                assert_eq!(spans[first].end, r.first_end, "{text:?}");
                let last = spans
                    .iter()
                    .position(|s| (s.end, s.sep_end) == (r.end, r.sep_end))
                    .unwrap_or_else(|| panic!("{text:?}"));
                let old =
                    next_comma(&text[r.start..], false).map_or(text.len(), |(c, _)| r.start + c);
                assert!(r.end >= r.first_end && r.end >= old, "{text:?}");
                assert!(spans[first..last].iter().all(|s| s.end < old), "{text:?}");
                at = r.sep_end;
            }
            assert_eq!(at, text.len(), "{text:?}");
            checked += 1;
        }
        assert!(checked > 500_000, "{checked}");
    }

    /// A line whose first `=` has a comma in front of it by the parsers'
    /// reading is redacted both ways, though today's reading sees no comma
    /// there: an apostrophe in front opened a quote for it (line-splitting
    /// design 6.1).
    #[test]
    fn a_comma_in_front_of_the_first_equals_sign_by_the_parsers_reading_counts() {
        assert_eq!(
            redact_profile("Tom's P http, h, 80, tfo=true, alice"),
            "Tom's P http, h, 80, tfo=true, ***"
        );
    }

    /// A secret parameter whose value today's reading carries past a comma
    /// keeps its own spacing around the `=` (line-splitting design LS-D9).
    #[test]
    fn a_value_carried_past_a_comma_keeps_the_spacing_around_its_name() {
        assert_eq!(
            redact_definition("trojan, h, 443, password  =  ab\"c,d\", ws=true"),
            "trojan, h, 443, password  =  ***, ws=true"
        );
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
    /// item — of the region (design LS-D9) when today's reading carries the
    /// item past a comma: the quote is none the item pass could read as a
    /// whole. A quoted item that is whole is that pass's, and never gets here.
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
            // a scheme no parser reads: nothing after the `://` is shown
            (
                "trojan://hunter2@t.example:443?sni=t.example#HK",
                "trojan://***",
            ),
            ("ss://YWVzLTEyOC1nY206cGFzcw@1.2.3.4:8388#name", "ss://***"),
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
            // in a comment too, where a link shows its authority only
            ("# was https://u:p@h.test/x", "# was https://***@h.test/***"),
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

    /// `[MTProto]`'s key is `secret` — a section rurge keeps as written, whose
    /// lines reach the redacted profile — and a `[Keystore]` line says
    /// `passphrase` where the field is `password`.
    #[test]
    fn the_secret_of_a_deferred_section_and_a_short_passphrase_are_hidden() {
        for (line, shown) in [
            (
                "secret = dd0123456789abcdef0123456789abcdef",
                "secret = ***",
            ),
            ("Secret=dd0123456789abcdef", "Secret= ***"),
            ("passphrase = hunter2", "passphrase = ***"),
            (
                "cert = type=p12, passphrase=hunter2, base64=AAAA",
                "cert = type=p12, passphrase=***, base64=***",
            ),
            (
                "cert = type=p12, \"passphrase=pw\"",
                "cert = type=p12, passphrase=***",
            ),
            // a word that only holds one is another name
            ("my-secret = 1", "my-secret = 1"),
            ("secrets = 1", "secrets = 1"),
            ("ca-passphrase = hunter2", "ca-passphrase = ***"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// The key of a line is the run of name characters at the end of the text
    /// in front of its separator, blanks and one closing quote after it not
    /// counted, when that run starts the text or stands behind a boundary of
    /// the name search. Whatever stands in front of it stays as written, and
    /// the value is hidden where it stands.
    #[test]
    fn a_key_with_something_in_front_of_it_is_still_the_key() {
        for (line, shown) in [
            // the `；` an input method types for `;`: the section parser does
            // not take it for a comment marker, and neither does the redaction
            (
                "\u{ff1b}ca-passphrase = hunter2",
                "\u{ff1b}ca-passphrase = ***",
            ),
            (
                "\u{ff1b} http-api = KEY@127.0.0.1:6171",
                "\u{ff1b} http-api = ***@127.0.0.1:6171",
            ),
            ("\u{ff1b}private-key = K3Y", "\u{ff1b}private-key = ***"),
            // a word of a comment's own, or a mark of a quoting program
            (
                "# was http-api = KEY@127.0.0.1:6171",
                "# was http-api = ***@127.0.0.1:6171",
            ),
            (
                "# old: ca-passphrase = hunter2",
                "# old: ca-passphrase = ***",
            ),
            ("> ca-passphrase = hunter2", "> ca-passphrase = ***"),
            // every kind of key, and the other separators
            (
                "# old http-listen = PW@0.0.0.0:6152",
                "# old http-listen = ***@0.0.0.0:6152",
            ),
            (
                "# old wifi-access-http-auth = user:PW",
                "# old wifi-access-http-auth = user:***",
            ),
            (
                "# old dns-server = https://al1ce:pw@dns.example/dns-query",
                "# old dns-server = https://***@dns.example/***",
            ),
            (
                "old dns-server = https://al1ce:pw@dns.example/dns-query",
                "old dns-server = https://***@dns.example/dns-query",
            ),
            (
                "# the old\tpassword\u{ff1d}hunter2",
                "# the old\tpassword\u{ff1d}***",
            ),
            (
                "# 12:30 http-api: KEY@127.0.0.1:6171",
                "# 12:30 http-api: ***@127.0.0.1:6171",
            ),
            // a quote around the key: one closing quote, blanks around it
            ("\"password\" = hunter2", "\"password\" = ***"),
            ("'ca-p12' : MIIK", "'ca-p12' : ***"),
            // hidden where it stands: the spacing as written, the rest of the
            // line gone
            ("# old password=hunter2", "# old password=***"),
            ("// the token = t0k3n was", "// the token = ***"),
            ("# old password=hunter2  ", "# old password=***  "),
            // nothing written after the separator: `***` stands there, as the
            // search by name has it for `password=`
            ("# set the password:", "# set the password:***"),
            ("# set the ca-passphrase:", "# set the ca-passphrase:***"),
            // stays as written
            (
                "^https://example\\.com/login\\?token=(.*) https://example.com/$1 302",
                "^https://example\\.com/login\\?token=(.*) https://example.com/$1 302",
            ),
            ("password-hint: none", "password-hint: none"),
            ("loglevel: notify", "loglevel: notify"),
            ("my.token = 1.2.3.4", "my.token = 1.2.3.4"),
            ("a/token = 1", "a/token = 1"),
            // a comma in front of the first `=`: still no key
            (
                "P http, h.example, 8080, alice, hunter2, tfo=true",
                "P http, h.example, 8080, ***, ***, tfo=true",
            ),
            ("Ext, password=hunter2", "Ext, password=***"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A secret named with a colon — in a comment, in a line for another
    /// client — goes as one named with an `=` does; one closing quote may
    /// stand between the name and the separator. The value ends where it
    /// ends for an `=`.
    #[test]
    fn a_secret_named_with_a_colon_goes_too() {
        for (line, shown) in [
            ("# an old password: hunter2", "# an old password: ***"),
            (
                "P = trojan, t.example, 443, password=x // old password: hunter1",
                "P = trojan, t.example, 443, password=*** // old password: ***",
            ),
            (
                "# - {name: HK, type: ss, server: s.example, port: 8388, cipher: aes-256-gcm, password: hunter2}",
                "# - {name: HK, type: ss, server: s.example, port: 8388, cipher: aes-256-gcm, password: ***",
            ),
            (
                "    uuid: 5ec4e75e-c4e7-45ec-8e75-ec4e75ec4e75",
                "    uuid: ***",
            ),
            ("    \"password\": \"hunter2\",", "    \"password\": ***"),
            // a name that is no whole-line key: the search by name
            ("# see url: https://x.example/y", "# see url: ***"),
            ("# 'args': -p hunter2", "# 'args': ***"),
            ("# token\u{ff1a}t0k3n", "# token\u{ff1a} ***"),
            ("# Password : hunter2", "# Password : ***"),
            (
                "X = external, a=b, ws-path: /s3cret, tfo=true",
                "X = external, a=b, ws-path: ***, tfo=true",
            ),
            (
                "X = external, a=b, \"headers\" : X-K:v, tfo=true",
                "X = external, a=b, \"headers\" : ***, tfo=true",
            ),
            // stays as written
            (
                "DOMAIN-SUFFIX,example.com,Proxy",
                "DOMAIN-SUFFIX,example.com,Proxy",
            ),
            (
                "example.com = server:8.8.8.8",
                "example.com = server:8.8.8.8",
            ),
            ("P = http, 2001:db8::1, 8080", "P = http, 2001:db8::1, 8080"),
            (
                "URL-REGEX,^https://a\\.com/\\?url:x,REJECT",
                "URL-REGEX,^https://a\\.com/\\?url:x,REJECT",
            ),
            ("data:image/png;base64,AAAA", "data:image/png;base64,AAAA"),
            ("urn:uuid:5ec4e75e", "urn:uuid:5ec4e75e"),
            ("# tokens: 5", "# tokens: 5"),
            ("# curl: (7) failed", "# curl: (7) failed"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// The other keys that stand on a line of their own take a colon for a
    /// separator as the secret ones do; a text in front of the first colon
    /// that holds an `=` is no key.
    #[test]
    fn a_colon_follows_every_whole_line_key() {
        for (line, shown) in [
            (
                "http-api: KEY@127.0.0.1:6171",
                "http-api: ***@127.0.0.1:6171",
            ),
            (
                "external-controller-access: KEY@0.0.0.0:6170",
                "external-controller-access: ***@0.0.0.0:6170",
            ),
            (
                "http-listen: PW@0.0.0.0:6152",
                "http-listen: ***@0.0.0.0:6152",
            ),
            (
                "socks5-listen\u{ff1a} PW@127.0.0.1:6153, 127.0.0.1:6154",
                "socks5-listen\u{ff1a} ***@127.0.0.1:6153, 127.0.0.1:6154",
            ),
            (
                "wifi-access-http-auth: user:PW",
                "wifi-access-http-auth: user:***",
            ),
            (
                "dns-server: https://al1ce:pw@dns.example/dns-query",
                "dns-server: https://***@dns.example/dns-query",
            ),
            (
                "Encrypted-DNS-Server : quic://b0b:pw@dns.example",
                "Encrypted-DNS-Server : quic://***@dns.example",
            ),
            // read by the `=`
            (
                "http-listen = pw@0.0.0.0:6152",
                "http-listen = ***@0.0.0.0:6152",
            ),
            (
                "http-api = a:b@127.0.0.1:6171",
                "http-api = ***@127.0.0.1:6171",
            ),
            // the colon of a value is no separator
            (
                "wifi-access-http-auth = user:PW",
                "wifi-access-http-auth = user:***",
            ),
            // no secret in it, no change
            ("loglevel: notify", "loglevel: notify"),
            ("dns-server: 8.8.8.8", "dns-server: 8.8.8.8"),
            ("password-hint: none", "password-hint: none"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A DNS key with text in front of it is no key line: a line of
    /// `[SSID Setting]` is `<subnet expression> key=value, key=value`, and its
    /// items are not URLs. It is read as a definition, whose URLs lose their
    /// user-info and nothing else.
    #[test]
    fn an_ssid_line_keeps_the_names_of_its_dns_parameters() {
        assert_eq!(
            redact_profile(
                "SSID:Office dns-server=192.168.1.1,encrypted-dns-server=https://u:p@dns.example/dns-query"
            ),
            "SSID:Office dns-server=192.168.1.1,encrypted-dns-server=https://***@dns.example/dns-query"
        );
    }

    /// Passing over a DNS key that has text in front of it must not pass over
    /// what stands behind it: the search for a key goes on to the next
    /// separator, and the rest of the line is a definition.
    #[test]
    fn a_secret_behind_a_dns_key_with_text_in_front_is_found() {
        for (line, shown) in [
            // the colon
            (
                "x dns-server: 1.1.1.1 password: hunter2",
                "x dns-server: 1.1.1.1 password: ***",
            ),
            (
                "x dns-server: 1.1.1.1 http-api: KEY@127.0.0.1:6171",
                "x dns-server: 1.1.1.1 http-api: ***@127.0.0.1:6171",
            ),
            // the `=`
            (
                "SSID:Office dns-server=1.1.1.1 password=hunter2",
                "SSID:Office dns-server=1.1.1.1 password=***",
            ),
            // nothing to hide, nothing changes
            (
                "SSID:Office dns-server=192.168.1.1,encrypted-dns-server=off",
                "SSID:Office dns-server=192.168.1.1,encrypted-dns-server=off",
            ),
            // a DNS key that stands alone is read as before
            (
                "encrypted-dns-server: https://u:p@dns.example/dns-query",
                "encrypted-dns-server: https://***@dns.example/dns-query",
            ),
            (
                "dns-server = 8.8.8.8, https://u:p@dns.example/dns-query",
                "dns-server = 8.8.8.8, https://***@dns.example/dns-query",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A URL whose scheme no parser here reads shows nothing of what follows
    /// its `://` up to the next white space: a share link carries its secret
    /// where the user-info is not.
    #[test]
    fn a_url_with_a_scheme_no_parser_reads_shows_nothing_after_it() {
        for (line, shown) in [
            ("vmess://eyJ2IjoiMiIsInBzIjoiSEsifQ==", "vmess://***"),
            (
                "ss://YWVzLTI1Ni1nY206aHVudGVyMkBzcy5leGFtcGxlOjgzODg=#HK",
                "ss://***",
            ),
            (
                "hysteria://hy.example:443?protocol=udp&auth=PW&peer=hy.example#HK",
                "hysteria://***",
            ),
            (
                "hysteria2://PW@hy.example:443/?obfs=salamander&obfs-password=PW2#HK",
                "hysteria2://***",
            ),
            (
                "trojan://hunter2@t.example:443?sni=t.example#HK",
                "trojan://***",
            ),
            // wherever it stands, in a comment, among words, more than once
            ("# vmess://eyJ2IjoiMiJ9", "# vmess://***"),
            (
                "see trojan://a@b:1 and ss://c ok",
                "see trojan://*** and ss://*** ok",
            ),
            ("Vmess://AAAA", "Vmess://***"),
            ("x.y+z-1://secret", "x.y+z-1://***"),
            // in a definition alone, too
            (
                "S = http, h.test, 80, tfo=true, x=ssh://git@h.test/r.git",
                "S = http, h.test, 80, tfo=true, x=ssh://***",
            ),
            // the schemes a parser of this crate reads: the user-info goes
            ("https://u:p@h.test/x", "https://***@h.test/x"),
            ("HTTP://u:p@h.test", "HTTP://***@h.test"),
            (
                "h3://u:p@dns.example/dns-query",
                "h3://***@dns.example/dns-query",
            ),
            ("quic://u:p@dns.example", "quic://***@dns.example"),
            ("tls://u:p@dns.example", "tls://***@dns.example"),
            ("tcp://u:p@dns.example", "tcp://***@dns.example"),
            // no scheme in front: a regular expression
            (
                "^https?://(www\\.)?example\\.com https://example.com 302",
                "^https?://(www\\.)?example\\.com https://example.com 302",
            ),
            ("(http|https)://u:p@h.test", "(http|https)://***@h.test"),
            ("://x@y", "://***@y"),
            // nothing follows the `://`: nothing is hidden
            ("a://", "a://"),
            ("ssh://", "ssh://"),
            ("ssh:// x", "ssh:// x"),
            // stays as written
            (
                "RULE-SET,https://example.com/list.txt,Proxy",
                "RULE-SET,https://example.com/list.txt,Proxy",
            ),
            (
                "encrypted-dns-server = quic://dns.example, h3://dns.example/dns-query",
                "encrypted-dns-server = quic://dns.example, h3://dns.example/dns-query",
            ),
            // the cost, known and accepted: a rewrite target with an app's scheme
            (
                "^https://example\\.com/item/(\\d+) myapp://item/$1 302",
                "^https://example\\.com/item/(\\d+) myapp://*** 302",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
        // and in a definition alone, where a policy's view gets it: the link
        // goes up to the next white space, the comma after it with it
        assert_eq!(
            redact_definition("external, exec=ssh://git@h.test/x, local-port=1080"),
            "external, exec=ssh://*** local-port=1080"
        );
    }

    /// A scheme begins with a letter (RFC 3986, section 3.1): what leads the
    /// run of scheme characters in front of a `://` and is none — the `+` of
    /// `[a-z]+://`, the `.+` of `^.+://` — belongs to a regular expression, and
    /// what is left of the run is the scheme.
    #[test]
    fn a_scheme_begins_with_a_letter() {
        for (line, shown) in [
            // a quantifier or a wildcard in front of the `://` of a pattern
            (
                "URL-REGEX,^[a-z]+://example\\.com/path,REJECT",
                "URL-REGEX,^[a-z]+://example\\.com/path,REJECT",
            ),
            (
                "^.+://example\\.com/(.*) https://example.org/$1 302",
                "^.+://example\\.com/(.*) https://example.org/$1 302",
            ),
            // what leads the run and is no letter is no part of the scheme
            ("1password://vault/item", "1password://***"),
            ("3http://u:p@h.test/x", "3http://***@h.test/x"),
            ("-ss://YWVz", "-ss://***"),
            // a run that begins with a letter is a scheme, a `\w+` included
            ("^\\w+://example\\.com/x", "^\\w+://***"),
            ("x.y+z-1://secret", "x.y+z-1://***"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A first field of several words whose first word is a type that takes a
    /// server is that type with the commas after it missing: its words are
    /// fields, the server second and the port third.
    #[test]
    fn the_words_of_a_type_field_are_fields() {
        for (line, shown) in [
            (
                "P = http h.example, 8080, alice, hunter2",
                "P = http h.example, 8080, ***, ***",
            ),
            (
                "S = socks5 1.2.3.4, 1080, user, pass",
                "S = socks5 1.2.3.4, 1080, ***, ***",
            ),
            (
                "# Old = https h.example, 443, bob, pw",
                "# Old = https h.example, 443, ***, ***",
            ),
            // without a single comma
            (
                "P = http h.example 8080 alice hunter2",
                "P = http h.example 8080 *** ***",
            ),
            (
                "P = http  h.example\t8080 alice, hunter2 tfo=true",
                "P = http  h.example\t8080 ***, ***",
            ),
            // the port as it may be: the words are fields only when a number
            // stands in the third place, and the line that has none is not
            // read as a policy line by this rule (the cost, known and accepted)
            (
                "P = trojan h.example, alice, hunter2",
                "P = trojan h.example, alice, hunter2",
            ),
            ("P = http h.example, 8080", "P = http h.example, 8080"),
            ("P = http h.example", "P = http h.example"),
            // in quotes, behind an empty item, typed with an input method
            (
                "P = \"http h.example\", 8080, alice, hunter2",
                "P = \"http h.example\", 8080, ***, ***",
            ),
            (
                "P = , http h.example, 8080, alice, hunter2",
                "P = , http h.example, 8080, ***, ***",
            ),
            (
                "P = ss h.example\u{ff0c} 8388\u{ff0c} alice\u{ff0c} hunter2",
                "P = ss h.example\u{ff0c} 8388\u{ff0c} ***\u{ff0c} ***",
            ),
            // a named parameter stays, as it does after a type with its comma
            (
                "P = http h.example, 8080, alice, hunter2, tfo=true",
                "P = http h.example, 8080, ***, ***, tfo=true",
            ),
            // stays as written
            ("G = select, A, B", "G = select, A, B"),
            ("G = select A B, C, D, E", "G = select A B, C, D, E"),
            (
                "HK 01 = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x",
                "HK 01 = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=***",
            ),
            ("DOMAIN,example.com,Proxy", "DOMAIN,example.com,Proxy"),
            ("direct x y z w", "direct x y z w"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// Words that open with a type are the fields of a policy line only when a
    /// port number follows them: a comment that starts with a protocol word is
    /// prose, and prose is shown as written.
    #[test]
    fn prose_that_opens_with_a_type_word_is_shown_as_written() {
        for line in [
            "# HTTP DNS blocking list",
            "# SSH tunnel to the home server",
            "# Trojan nodes below",
            "# ss is fine, really, ok",
        ] {
            assert_eq!(redact_profile(line), line, "{line:?}");
        }
    }

    /// The same in the inline comment of a line, which is redacted as a
    /// commented-out line is.
    #[test]
    fn an_ordinary_inline_comment_that_starts_with_a_protocol_word_is_kept() {
        let line = "DOMAIN,example.com,Proxy // ss node via Hong Kong";
        assert_eq!(redact_profile(line), line);
    }

    /// The inline comment of a line — its marker kept as written — is
    /// redacted as a commented-out line is: as if it were not commented out,
    /// then searched by name. A requirement directive between the content and
    /// the comment is handled as before.
    #[test]
    fn an_inline_comment_is_redacted_as_a_commented_out_line_is() {
        for (line, shown) in [
            (
                "P = trojan, t.example, 443, password=new #password=old",
                "P = trojan, t.example, 443, password=*** #password= ***",
            ),
            (
                "P = http, h.example, 8080, alice, new # was: http, old.example, 8080, alice, oldpw",
                "P = http, h.example, 8080, ***, *** # was: http, old.example, 8080, ***, ***",
            ),
            (
                "P = direct, interface=eth0 ;  old = http, h.test, 80, alice, hunter2",
                "P = direct, interface=eth0 ;  old = http, h.test, 80, ***, ***",
            ),
            (
                "P = direct, interface=eth0 // old = ss, h.test, 8388, hunter2, password=x1",
                "P = direct, interface=eth0 // old = ss, h.test, 8388, ***, password=***",
            ),
            (
                "P = http, h.test, 80, alice, hunter2 #!MACOS-ONLY // old password=hunter3",
                "P = http, h.test, 80, ***, *** #!MACOS-ONLY // old password=***",
            ),
            // a comment inside a comment
            (
                "DOMAIN,a.com,DIRECT # a # b # args=hunter2",
                "DOMAIN,a.com,DIRECT # a # b # args=***",
            ),
            (
                "DOMAIN,a.com,DIRECT # one # two # three # four # five # six # password=hunter2",
                "DOMAIN,a.com,DIRECT # one # two # three # four # five # six # password=***",
            ),
            // nothing to hide, nothing changes
            (
                "DOMAIN,a.com,DIRECT # keep it simple",
                "DOMAIN,a.com,DIRECT # keep it simple",
            ),
            (
                "loglevel = notify // the default",
                "loglevel = notify // the default",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A comment can hold a marker again, and a line of a hundred thousand
    /// of them costs a loop and a few levels, not a stack.
    #[test]
    fn a_line_of_nested_comments_costs_no_stack() {
        let tail = " # x".repeat(100_000);
        assert_eq!(
            redact_profile(&format!("P = http, h.test, 80, alice, hunter2{tail}")),
            format!("P = http, h.test, 80, ***, ***{tail}")
        );
        let tail = " // y".repeat(100_000);
        assert_eq!(redact_profile(&format!("; {tail}")), format!("; {tail}"));
        // a secret past the levels followed is found all the same
        let nested = format!("a{} # password={V}", " # x".repeat(100));
        let shown = redact_profile(&nested);
        assert!(!shown.contains(V), "{shown}");
        assert!(shown.ends_with(" # password=***"), "{shown}");
    }

    /// A subscription link noted in a comment is the secret that
    /// `#!MANAGED-CONFIG` and `policy-path` hide: in comment text — a
    /// commented-out line, an inline comment — a URL of a scheme a parser
    /// reads shows its scheme and its authority, and what follows up to the
    /// next white space is `/***`.
    #[test]
    fn a_link_in_a_comment_shows_its_scheme_and_authority_only() {
        for (line, shown) in [
            (
                "# \u{8ba2}\u{9605}: https://sub.example/api/v1/client/subscribe?token=TOKEN",
                "# \u{8ba2}\u{9605}: https://sub.example/***",
            ),
            (
                "# updated from https://sub.example/link/TOKEN?surge=4",
                "# updated from https://sub.example/***",
            ),
            (
                "// https://sub.example/link/TOKEN",
                "// https://sub.example/***",
            ),
            (
                "# https://github.com/owner/repo",
                "# https://github.com/***",
            ),
            (
                "# RULE-SET,https://example.com/list.txt,Proxy",
                "# RULE-SET,https://example.com/***",
            ),
            (
                "// see https://example.com/docs for more",
                "// see https://example.com/*** for more",
            ),
            // the user-info goes as on any line
            ("# was https://u:p@h.test/x", "# was https://***@h.test/***"),
            // what follows the authority up to the white space, a comma too
            (
                "# a https://example.com, b",
                "# a https://example.com/*** b",
            ),
            (
                "# two https://a.test/x https://b.test/y",
                "# two https://a.test/*** https://b.test/***",
            ),
            (
                "; quic://dns.example/dns-query?x=1",
                "; quic://dns.example/***",
            ),
            // nothing follows the authority: nothing is hidden
            ("# https://example.com", "# https://example.com"),
            (
                "# https://example.com and more",
                "# https://example.com and more",
            ),
            // any other scheme is the business of the rule for those
            ("# ssh://git@github.com/x/y.git", "# ssh://***"),
            // an inline comment is a comment
            (
                "DOMAIN,a.com,DIRECT // https://sub.example/link/TOKEN",
                "DOMAIN,a.com,DIRECT // https://sub.example/***",
            ),
            (
                "P = direct # see https://sub.example/a?token=T and https://u:p@h.test/b",
                "P = direct # see https://sub.example/*** and https://***@h.test/***",
            ),
            // the same URL on a line that is no comment stays whole
            (
                "RULE-SET,https://example.com/list.txt,Proxy",
                "RULE-SET,https://example.com/list.txt,Proxy",
            ),
            (
                "script-path=https://h.test/s.js?u=a@b",
                "script-path=https://h.test/s.js?u=a@b",
            ),
            // a `#!` line is no comment: it has its own rules
            (
                "#!include a.conf, https://h.example/p?token=t0k3n",
                "#!include a.conf, ***",
            ),
            (
                "#!MACOS-ONLY RULE-SET,https://example.com/a.list,Proxy",
                "#!MACOS-ONLY RULE-SET,https://example.com/a.list,Proxy",
            ),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// A secret that starts with `#` — or `;`, or `//` — is taken for an
    /// inline comment by the section parser, and the key's value is empty:
    /// the comment text is `***`, its marker stays.
    #[test]
    fn a_secret_that_starts_with_a_hash_is_the_comment() {
        for (line, shown) in [
            ("ca-passphrase = #Secret1", "ca-passphrase = *** #***"),
            ("http-api = #KEY@127.0.0.1:6171", "http-api =*** #***"),
            ("ca-passphrase = ;Secret1", "ca-passphrase = *** ;***"),
            ("ca-passphrase: // Secret1  ", "ca-passphrase: *** // ***  "),
            ("password = # the real one", "password = *** # ***"),
            (
                "wifi-access-http-auth = #user:PW",
                "wifi-access-http-auth =*** #***",
            ),
            (
                "#!MACOS-ONLY ca-passphrase = #Secret1",
                "#!MACOS-ONLY ca-passphrase = *** #***",
            ),
            (
                "ca-passphrase = #!MACOS-ONLY #Secret1",
                "ca-passphrase = *** #!MACOS-ONLY #***",
            ),
            // a comment marker with nothing behind it
            ("ca-passphrase = #", "ca-passphrase = *** #"),
            // the value is not empty: the comment is a comment
            (
                "ca-passphrase = hunter2 # the new one",
                "ca-passphrase = *** # the new one",
            ),
            // a key whose value is not a secret, or no key at all
            ("loglevel = # none yet", "loglevel = # none yet"),
            ("dns-server = # none yet", "dns-server = # none yet"),
            ("P = # nothing", "P = # nothing"),
        ] {
            assert_eq!(redact_profile(line), shown, "{line:?}");
        }
    }

    /// Which keys hold a secret as their whole value: the lists, read as the
    /// redaction reads a key, with whatever stands in front of it.
    #[test]
    fn a_key_whose_value_holds_a_secret_is_told_from_one_that_does_not() {
        for key in [
            "password",
            "ca-passphrase",
            "CA_P12",
            "Private-Key",
            "psk",
            "secret",
            "passphrase",
            "token",
            "http-api",
            "external-controller-access",
            "http-listen",
            "socks5-listen",
            "wifi-access-http-auth",
            "old: ca-passphrase",
            "\u{ff1b}http-api",
            "\"password\"",
            " token ",
        ] {
            assert!(is_secret_key(key), "{key:?}");
        }
        for key in [
            "dns-server",
            "encrypted-dns-server",
            "loglevel",
            "password-hint",
            "my.token",
            "tokens",
            "a/token",
            "",
            " ",
            "http",
        ] {
            assert!(!is_secret_key(key), "{key:?}");
        }
    }

    /// The look before the searches by name saves them and changes nothing:
    /// whatever the searches would do to a text, the look finds a name for
    /// them to do it to.
    #[test]
    fn the_look_before_the_search_by_name_changes_nothing() {
        let always = |text: &str| {
            SECRET_PARAMS
                .iter()
                .fold(text.to_string(), |text, param| redact_param(&text, param))
        };
        const PIECES: [&str; 13] = [
            "password", "url", "x", "=", ":", "\u{ff1d}", "\u{ff1a}", "\"", "'", " ", ",", "-",
            "\u{e9}",
        ];
        let mut changed = 0usize;
        let mut texts = 0usize;
        let mut check = |text: String| {
            texts += 1;
            let shown = redact_by_name(&text);
            if shown != text {
                changed += 1;
            }
            assert_eq!(shown, always(&text), "{text:?}");
        };
        // every text of up to four pieces
        let mut level = vec![String::new()];
        for _ in 0..4 {
            let mut longer = Vec::with_capacity(level.len() * PIECES.len());
            for text in &level {
                check(text.clone());
                for piece in PIECES {
                    longer.push(format!("{text}{piece}"));
                }
            }
            level = longer;
        }
        for text in level {
            check(text);
        }
        // and a good many longer ones, from a fixed seed
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let len = 5 + (next() % 8) as usize;
            let text: String = (0..len)
                .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
                .collect();
            check(text);
        }
        assert!(
            texts > 20_000 && changed > 500,
            "{texts} texts, {changed} changed"
        );
    }

    /// Every secret parameter, named with a colon the ways a person writes
    /// one or another client does, in a comment, in a definition, in a line
    /// of a flow mapping. (The section parser reads none of these as the
    /// parameter: they are a person's secrets, not a parser's.)
    #[test]
    fn every_secret_parameter_is_hidden_behind_a_colon_too() {
        let mut checked = 0usize;
        for name in SECRET_PARAMS {
            let upper = name.to_ascii_uppercase();
            for spelling in [
                format!("{name}: {V}"),
                format!("{name} : {V}"),
                format!("{upper}:{V}"),
                format!("{name}\u{ff1a}{V}"),
                format!("{name}\u{ff1a} {V}"),
                format!("\"{name}\": \"{V}\""),
                format!("'{name}' : '{V}'"),
                format!("\"{name}\":{V}"),
            ] {
                for form in [
                    "# an old {s} and more",
                    "// {s}",
                    "{s}",
                    "    {s}",
                    "X = external, a=b, {s}",
                    "X = external, {s}, tfo=true",
                    "X = http, h.test, 80, alice, hunter2 // old {s}",
                    "# - flow: {s}",
                ] {
                    let line = form.replace("{s}", &spelling);
                    let shown = redact_profile(&line);
                    checked += 1;
                    assert!(!shown.contains(V), "{line:?} -> {shown:?}");
                }
            }
        }
        assert!(checked > 1_500, "{checked}");
    }

    /// A line of one kind of character — the ones the rules look for — is
    /// redacted without a panic and in time that grows with its length, not
    /// with its square: a hundred thousand characters take well under the
    /// time a square would.
    #[test]
    fn a_long_line_of_one_kind_is_redacted_in_time_that_grows_with_its_length() {
        let n = 100_000usize;
        let lines: Vec<(&str, String)> = vec![
            ("colons", ":".repeat(n)),
            ("equals signs", "=".repeat(n)),
            ("hashes", "#".repeat(n)),
            ("slashes", "/".repeat(n)),
            ("urls", "://".repeat(n / 3)),
            ("double quotes", "\"".repeat(n)),
            ("single quotes", "'".repeat(n)),
            ("full-width semicolons", "\u{ff1b}".repeat(n / 3)),
            ("full-width colons", "\u{ff1a}".repeat(n / 3)),
            ("full-width equals signs", "\u{ff1d}".repeat(n / 3)),
            ("commas", ",".repeat(n)),
            ("spaces", " ".repeat(n)),
            ("comments", " # x".repeat(n / 4)),
            ("colon names", "password: ".repeat(n / 10)),
            ("quoted names", "\"password\": ".repeat(n / 12)),
            ("equals names", "password=".repeat(n / 9)),
            ("keys", "http-api: ".repeat(n / 10)),
            ("keys behind blanks", "a http-api:".repeat(n / 11)),
            (
                "keys with colons in front",
                "12:30 ".repeat(n / 6) + "http-api: x",
            ),
            ("type words", format!("P = http {}", "a ".repeat(n / 2))),
            (
                "type words, quoted",
                format!("P = \"http {}\"", "a ".repeat(n / 2)),
            ),
            ("share links", "ssh://a ".repeat(n / 8)),
            (
                "links in a comment",
                format!("# {}", "https://a.test/x ".repeat(n / 17)),
            ),
            (
                "links without a break",
                format!("# {}", "https://a.test/x,".repeat(n / 17)),
            ),
            (
                "empty comments",
                "ca-passphrase =".to_string() + &" #".repeat(n / 2),
            ),
            ("open parentheses", "(".repeat(n)),
            ("close parentheses", ")".repeat(n)),
            ("open parentheses and commas", "(,".repeat(n / 2)),
            ("apostrophes and commas", "'a,".repeat(n / 3)),
            ("first equals signs", "a=".repeat(n / 2)),
            ("quotes before parentheses", "a\"(".repeat(n / 3)),
            ("quotes before equals signs", "a\"=".repeat(n / 3)),
            ("names with apostrophes", "Tom's, ".repeat(n / 7)),
        ];
        for (kind, line) in &lines {
            let ten_thousand: String = line.chars().take(10_000).collect();
            for text in [ten_thousand.as_str(), line.as_str()] {
                let started = std::time::Instant::now();
                let _ = redact_profile(text);
                let _ = redact_definition(text);
                let took = started.elapsed();
                assert!(
                    took < std::time::Duration::from_secs(10),
                    "{kind} ({} bytes) took {took:?}",
                    text.len()
                );
            }
        }
    }

    /// Whatever the text, nothing panics: the slices the rules cut are on
    /// character boundaries, whatever stands near a separator or a quote.
    #[test]
    fn text_of_any_shape_never_panics() {
        const PIECES: [&str; 40] = [
            "password",
            "url",
            "http-api",
            "ca-passphrase",
            "http",
            "ss",
            "://",
            ":",
            "\u{ff1a}",
            "=",
            "\u{ff1d}",
            ",",
            "\u{ff0c}",
            "#",
            ";",
            "//",
            "\"",
            "'",
            "(",
            ")",
            " ",
            "\t",
            "\u{3000}",
            "\u{200b}",
            "\u{feff}",
            "\u{ff1b}",
            "\u{e9}",
            "x",
            "@",
            "\u{ff20}",
            "#!",
            "#!MACOS-ONLY ",
            "#!MANAGED-CONFIG ",
            "-",
            "_",
            "\\",
            "/",
            "?",
            "\r",
            "443",
        ];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..30_000 {
            let len = 1 + (next() % 24) as usize;
            let text: String = (0..len)
                .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
                .collect();
            let _ = redact_profile(&text);
            let _ = redact_definition(&text);
        }
    }
}
