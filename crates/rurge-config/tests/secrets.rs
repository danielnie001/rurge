//! No credential leaves the configuration by a side door: not in the
//! `Debug` of anything loaded, not in a diagnostic, not in the redacted
//! profile. Every secret below carries the marker `S3CR3T`, or is the one
//! UUID, so one search finds whatever got out.

use rurge_config::config::{LoadOptions, Loaded, from_text};
use rurge_config::redact::redact_profile;
use std::path::Path;

const MARKER: &str = "S3CR3T";
const UUID: &str = "5ec4e75e-c4e7-45ec-8e75-ec4e75ec4e75";
/// 32 bytes of Base64 that spell the marker: a WireGuard key.
const KEY: &str = "S3CR3TS3CR3TS3CR3TS3CR3TS3CR3TS3CR3TS3CR3TQ=";
/// 16 bytes of it: a Shadowsocks 2022 key.
const KEY_16: &str = "S3CR3TS3CR3TS3CR3TS3CQ==";
/// A peer's public key is no secret.
const PUBLIC: &str = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVowMTIzNDU=";

fn profile() -> String {
    format!(
        "[General]
http-api = {MARKER}-a@{MARKER}-pi@127.0.0.1:6171
external-controller-access = {MARKER}-eca@127.0.0.1:6170
http-listen = {MARKER}-listen@127.0.0.1:6152
wifi-access-http-auth = user:{MARKER}-wifi
encrypted-dns-server = https://al1ce:{MARKER}-doh@dns.example/dns-query

[Proxy]
Http = http, h.example, 8080, user, {MARKER}-http
Https = https, h.example, 443, username=user, password={MARKER}-https, headers=X-Auth:{MARKER}-header
Socks = socks5, s.example, 1080, user, {MARKER}-socks
Trojan = trojan, t.example, 443, password={MARKER}-trojan, ws=true, ws-path=/{MARKER}-path, ws-headers=X-Key:{MARKER}-wsheader
Vmess = vmess, v.example, 443, username={UUID}, vmess-aead=true
AnyTls = anytls, a.example, 443, password={MARKER}-anytls
Ss = ss, ss.example, 8388, encrypt-method=aes-128-gcm, password={MARKER}-ss, obfs=http, obfs-host={MARKER}-obfs.example, obfs-uri=/{MARKER}-uri
Ss2022 = ss, ss.example, 8389, encrypt-method=2022-blake3-aes-128-gcm, password={KEY_16}
Snell = snell, sn.example, 443, psk={MARKER}-snell, version=4
Hy2 = hysteria2, hy.example, 443, password={MARKER}-hy2, salamander-password={MARKER}-salamander
TuicV5 = tuic-v5, tu.example, 443, uuid={UUID}, password={MARKER}-tuic
Tuic = tuic, t4.example, 443, token={MARKER}-token
Masque = masque, mq.example, 443, username=user, password={MARKER}-masque
Tt = trust-tunnel, tt.example, 443, username=user, password={MARKER}-tt
H2 = h2-connect, h2.example, 443, username=user, password={MARKER}-h2
Ssh = ssh, ssh.example, 22, username=root, password={MARKER}-ssh
Wg = wireguard, section-name=Home
Ext = external, exec=/bin/true, local-port=1080, args={MARKER}-args
Shadow = trojan, st.example, 443, password={MARKER}-st, shadow-tls-password={MARKER}-shadow, shadow-tls-version=3, shadow-tls-sni=st.example
Tested = http, tested.example, 80, test-url=http://t.example/?token={MARKER}-testurl
Ts = tailscale, section-name=ts

[Proxy Group]
G = select, Http, policy-path=https://sub.example/list?token={MARKER}-sub, external-policy-modifier=\"password={MARKER}-modifier\"

[Keystore]
cert = type=p12, password={MARKER}-p12, base64={MARKER}{MARKER}

[WireGuard Home]
private-key = {KEY}
self-ip = 10.0.0.2
peer = (public-key = {PUBLIC}, preshared-key = {KEY}, allowed-ips = 0.0.0.0/0, endpoint = wg.example:51820)

[Tailscale ts]
auth-key = tskey-{MARKER}-tailscale

[MITM]
ca-passphrase = {MARKER}-ca
ca-p12 = {MARKER}{MARKER}{MARKER}

[Rule]
FINAL,DIRECT
"
    )
}

fn load(text: &str) -> Loaded {
    from_text(text, Path::new("t.conf"), &LoadOptions::for_tests())
}

/// The lines of `text` that hold a secret, for a failure to name.
fn leaks(text: &str) -> Vec<String> {
    let key = &KEY[..12];
    text.lines()
        .filter(|l| l.contains(MARKER) || l.contains(UUID) || l.contains(key))
        .map(|l| l.chars().take(200).collect())
        .collect()
}

#[test]
fn the_profile_loads() {
    let loaded = load(&profile());
    assert!(
        !loaded.diagnostics.has_errors(),
        "the marker profile must be a valid one: {:#?}",
        loaded.diagnostics
    );
    // every line that takes a spec has one: the secrets are really read
    for name in [
        "Http", "Https", "Socks", "Trojan", "Vmess", "AnyTls", "Ss", "Ss2022", "Snell", "Hy2",
        "TuicV5", "Tuic", "Masque", "Tt", "H2", "Ssh", "Wg", "Ext", "Shadow", "Tested",
    ] {
        assert!(loaded.config.spec(name).is_some(), "{name}");
    }
}

#[test]
fn the_debug_of_a_loaded_profile_holds_no_secret() {
    let loaded = load(&profile());
    let shown = format!("{:#?}", loaded.config);
    assert_eq!(leaks(&shown), Vec::<String>::new());
}

#[test]
fn the_diagnostics_of_a_valid_profile_hold_no_secret() {
    let loaded = load(&profile());
    let shown = format!("{:#?}", loaded.diagnostics);
    assert_eq!(leaks(&shown), Vec::<String>::new());
}

#[test]
fn the_redacted_profile_holds_no_secret() {
    let shown = redact_profile(&profile());
    assert_eq!(leaks(&shown), Vec::<String>::new());
    // and nothing else of it changed: the line count is the profile's
    assert_eq!(shown.lines().count(), profile().lines().count());
}

/// Values that are wrong the way a typing mistake makes them: the
/// diagnostic may say which key and which server, never the credential.
#[test]
fn a_diagnostic_of_a_mistyped_general_value_holds_no_secret() {
    let text = format!(
        "[General]
dns-server = htps://al1ce:{MARKER}-one@dns.example/dns-query
encrypted-dns-server = doq://b0b:{MARKER}-two@dns.example
doh-server = http://carol:{MARKER}-three@dns.example/dns-query
http-api = {MARKER}-four@api.example:6171
external-controller-access = {MARKER}-five@api.example:6170
http-listen = {MARKER}-six@listen.example:6152
socks5-listen = {MARKER}-seven@127.0.0.1:6153
wifi-access-http-auth = {MARKER}-eight
[Rule]
FINAL,DIRECT
"
    );
    let loaded = load(&text);
    let messages: Vec<&str> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(leaks(&messages.join("\n")), Vec::<String>::new());
    // what is left still says which item it was
    for told in [
        "htps://***@dns.example/dns-query",
        "doq://***@dns.example",
        "http://***@dns.example/dns-query",
        "***@api.example:6171",
        "***@api.example:6170",
        "***@listen.example:6152",
        "***@127.0.0.1:6153",
        "`wifi-access-http-auth`",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(told)),
            "{told}: {messages:#?}"
        );
    }
}
