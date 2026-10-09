//! Profile text redaction for `GET /v1/profiles/current?sensitive=0` (M4
//! design §4.3): secrets become `***`, everything else (including line count)
//! is preserved so line numbers in diagnostics still line up.

/// Keys whose whole value is a secret when they stand alone on a line
/// (`[WireGuard]` `private-key`, `[Snell Server]` `psk`, `[Tailscale]`
/// `auth-key`, …). `[General]`'s two test URLs are among them as a policy's
/// `test-url` is among the inline parameters: a profile that came from a
/// provider can carry a token in them. So is `geoip-maxmind-url`: MaxMind's
/// own download link carries the account's licence key in its query.
const SECRET_KEYS: [&str; 11] = [
    "password",
    "ca-passphrase",
    "ca-p12",
    "private-key",
    "psk",
    "pre-shared-key",
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
/// `ws-path` is. Over-redacting is the safe side for an endpoint whose
/// purpose is safe output.
const SECRET_PARAMS: [&str; 22] = [
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
    "obfs-host",
    "obfs-uri",
];
/// Keys whose items are `[secret@]address`: what stands before an item's
/// last `@` goes.
const KEY_AT_KEYS: [&str; 4] = [
    "http-api",
    "external-controller-access",
    "http-listen",
    "socks5-listen",
];
/// Keys that list DNS servers, whose URLs may carry a user name and a
/// password (`https://user:pass@host/…`).
const DNS_KEYS: [&str; 3] = ["dns-server", "encrypted-dns-server", "doh-server"];
/// Policy types written `type, server, port, ...`. Surge documents positional
/// credentials (`username, password`) for the four classic ones, and rurge
/// reads them for `h2-connect` too (phase 2 M6 design 16); for the rest a
/// positional value from index 3 on is never meaningful — it is a stray
/// token (`W0001`), often a password written the way another type takes it —
/// and over-redacting is this module's stated bias.
const SERVER_PROXY_TYPES: [&str; 16] = [
    "http",
    "https",
    "h2-connect",
    "socks5",
    "socks5-tls",
    "ss",
    "snell",
    "vmess",
    "trojan",
    "tuic",
    "tuic-v5",
    "hysteria2",
    "masque",
    "anytls",
    "trust-tunnel",
    "ssh",
];

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

fn redact_body(line: &str) -> String {
    if let Some(managed) = redact_managed(line) {
        return managed;
    }
    let Some((key, value)) = line.split_once('=') else {
        return line.to_string();
    };
    let k = key.trim().to_ascii_lowercase();
    if SECRET_KEYS.contains(&k.as_str()) {
        return format!("{key}= ***");
    }
    if k == "wifi-access-http-auth" {
        let v = value.trim_start();
        let lead = &value[..value.len() - v.len()];
        return match v.split_once(':') {
            Some((user, _)) => format!("{key}={lead}{user}:***"),
            None => format!("{key}={lead}***"),
        };
    }
    let shown: Option<fn(&str) -> String> = if KEY_AT_KEYS.contains(&k.as_str()) {
        Some(without_credentials)
    } else if DNS_KEYS.contains(&k.as_str()) {
        Some(url_without_credentials)
    } else {
        None
    };
    if let Some(shown) = shown {
        let items: Vec<String> = value
            .split(',')
            .map(|item| {
                let t = item.trim_start();
                let lead = &item[..item.len() - t.len()];
                format!("{lead}{}", shown(t))
            })
            .collect();
        return format!("{key}={}", items.join(","));
    }
    // policy lines: `name = type, host, port, password=..., psk=...`, or
    // `name = http/https/socks5/socks5-tls, host, port, username, password`
    // (those types carry credentials positionally; on every other type that
    // takes a server and a port a positional value is blanked all the same).
    format!("{key}={}", redact_definition(value))
}

/// `#!MANAGED-CONFIG <link> …` with the link — the first word after the
/// directive, as `parse_managed` reads it — replaced by `***`: the one a
/// provider issues carries the subscription token. The directive is a word of
/// its own; a longer one that only starts like it is another directive and,
/// like every other `#!` line, left alone.
fn redact_managed(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("#!MANAGED-CONFIG")?;
    let link = rest.trim_start();
    if link.len() == rest.len() || link.is_empty() {
        return None;
    }
    let end = link.find(char::is_whitespace).unwrap_or(link.len());
    Some(format!(
        "{}***{}",
        &line[..line.len() - link.len()],
        &link[end..]
    ))
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

/// A policy definition (the text right of `name =`) with its secrets blanked:
/// the positional values of every type that takes a server and a port, and
/// every secret `name=value` parameter.
pub fn redact_definition(definition: &str) -> String {
    let mut redacted = redact_positional_credentials(definition);
    for param in SECRET_PARAMS {
        redacted = redact_param(&redacted, param);
    }
    redacted
}

/// For a `SERVER_PROXY_TYPES` policy line, blanks every positional token from
/// index 3 onward (0 = type, 1 = host, 2 = port) — that is where Surge puts
/// `username, password`. A token counts as a named parameter (and is kept)
/// only when what follows its first `=` is non-empty and not made only of `=`,
/// so base64 padding (`aHVudGVyMg==`) is redacted while `tfo=true` survives;
/// `sni=` with an empty value is over-redacted. Any other line is returned
/// unchanged.
fn redact_positional_credentials(value: &str) -> String {
    let tokens = split_top_level(value);
    let is_server_type = tokens
        .first()
        .is_some_and(|t| SERVER_PROXY_TYPES.contains(&t.trim().to_ascii_lowercase().as_str()));
    if !is_server_type {
        return value.to_string();
    }
    tokens
        .iter()
        .enumerate()
        .map(|(i, tok)| {
            let t = tok.trim_start();
            let lead = &tok[..tok.len() - t.len()];
            if i >= 3 && !is_named_param(t.trim_end()) {
                format!("{lead}***")
            } else {
                (*tok).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
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
                b',' if depth == 0 => return Some(i),
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

/// `name=value` with a value that is neither empty nor pure `=` padding. A
/// token that opens with a quote is one quoted value, `=` and all.
fn is_named_param(token: &str) -> bool {
    if token.starts_with('"') || token.starts_with('\'') {
        return false;
    }
    token
        .split_once('=')
        .is_some_and(|(_, after)| !after.is_empty() && !after.chars().all(|c| c == '='))
}

/// How long a parameter's value is: up to the first comma the parser would
/// split on — quotes may open anywhere inside the value and parentheses group,
/// so `ab"c,d"` and `a(b,c)d` are each one value. When the scan starts inside
/// a group (a secret nested in a `peer = (…)`), depth starts at 0 there, so the
/// value ends at the group's own next comma.
fn param_value_len(value: &str) -> usize {
    next_top_level_comma(value).unwrap_or(value.len())
}

/// Replaces the value of every `<param> = <value>` occurrence with `***`
/// (`param_value_len` says where the value ends), keeping the surrounding
/// spacing. Substring-based on purpose: a secret also has to be found nested
/// inside a parenthesised group (a `[WireGuard]` `peer = (…, pre-shared-key =
/// …)`), where no top-level split would reach it.
fn redact_param(value: &str, param: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let mut out = String::with_capacity(value.len());
    let mut rest = 0usize;
    let mut search = 0usize;
    while let Some(pos) = lower[search..].find(param) {
        let start = search + pos;
        // must be at a token boundary followed by optional spaces and '=';
        // `(` is one too: a group's first item has nothing but it in front
        let before_ok =
            start == 0 || matches!(lower.as_bytes()[start - 1], b',' | b' ' | b'\t' | b'(');
        let after = &value[start + param.len()..];
        let eq = after.trim_start().strip_prefix('=');
        match (before_ok, eq) {
            (true, Some(tail)) => {
                let tail_start = value.len() - tail.len();
                let tail_lead = &tail[..tail.len() - tail.trim_start().len()];
                let val_len = param_value_len(tail.trim_start());
                out.push_str(&value[rest..tail_start]);
                out.push_str(tail_lead);
                out.push_str("***");
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
    /// and the link carries the subscription token: the directive and its
    /// parameters stay, the link — the first word after the directive, as
    /// `parse_managed` reads it — goes.
    #[test]
    fn a_managed_config_line_loses_its_link() {
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
            // no link: nothing to hide
            ("#!MANAGED-CONFIG", "#!MANAGED-CONFIG"),
            ("#!MANAGED-CONFIG  \r", "#!MANAGED-CONFIG  \r"),
            // other `#!` lines are left alone, a longer word that only starts
            // like the directive too: the word after it is no link in any
            // reading of the format (`parse_managed`, which takes the
            // directive by its prefix, reads the `X` of this one as the link)
            (
                "#!FORBIDDEN-AUTO-UPGRADE smart-group",
                "#!FORBIDDEN-AUTO-UPGRADE smart-group",
            ),
            (
                "#!MANAGED-CONFIGX https://provider.test/t0k3n interval=60",
                "#!MANAGED-CONFIGX https://provider.test/t0k3n interval=60",
            ),
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
}
