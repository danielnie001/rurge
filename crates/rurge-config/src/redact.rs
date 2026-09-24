//! Profile text redaction for `GET /v1/profiles/current?sensitive=0` (M4
//! design §4.3): secrets become `***`, everything else (including line count)
//! is preserved so line numbers in diagnostics still line up.

/// Keys whose whole value is a secret when they stand alone on a line
/// (`[WireGuard]` `private-key`, `[Snell Server]` `psk`, …).
const SECRET_KEYS: [&str; 7] = [
    "password",
    "ca-passphrase",
    "ca-p12",
    "private-key",
    "psk",
    "pre-shared-key",
    "token",
];
/// Inline `name=value` parameters redacted wherever they appear in a value.
/// `username` also covers harmless SSH user names; `headers` and `ws-headers`
/// are blanked whole (header names included), and so is `ws-path`, which
/// nodes behind a CDN routinely use as a shared secret. `shadow-tls-password`
/// needs its own entry: `password` only matches at a token boundary. A
/// group's `policy-path` usually carries a subscription token, and its
/// `external-policy-modifier` can set any parameter, a password included.
/// Over-redacting is the safe side for an endpoint whose purpose is safe output.
const SECRET_PARAMS: [&str; 14] = [
    "password",
    "psk",
    "private-key",
    "pre-shared-key",
    "base64",
    "token",
    "uuid",
    "username",
    "headers",
    "ws-headers",
    "ws-path",
    "shadow-tls-password",
    "policy-path",
    "external-policy-modifier",
];
const KEY_AT_KEYS: [&str; 4] = [
    "http-api",
    "external-controller-access",
    "http-listen",
    "socks5-listen",
];
/// Policy types written `type, server, port, ...`. Surge documents positional
/// credentials (`username, password`) for the four classic ones; for the rest
/// a positional value from index 3 on is never meaningful — it is a stray
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
fn redact_line(line: &str) -> String {
    match line.strip_suffix('\r') {
        Some(rest) => format!("{}\r", redact_body(rest)),
        None => redact_body(line),
    }
}

fn redact_body(line: &str) -> String {
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
    if KEY_AT_KEYS.contains(&k.as_str()) {
        let items: Vec<String> = value
            .split(',')
            .map(|item| {
                let t = item.trim_start();
                let lead = &item[..item.len() - t.len()];
                match t.split_once('@') {
                    Some((_, rest)) => format!("{lead}***@{rest}"),
                    None => item.to_string(),
                }
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
}
