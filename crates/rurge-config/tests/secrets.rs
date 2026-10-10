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
/// 32 bytes of it, behind the quotes of a whole item.
const KEY_QUOTED: &str = "S3CR3TquotedquotedquotedquotedquotedquotedQ=";
/// A peer's public key is no secret.
const PUBLIC: &str = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVowMTIzNDU=";

fn profile() -> String {
    format!(
        "\u{feff}#!MANAGED-CONFIG https://sub.example/managed/{MARKER}-managedpath?token={MARKER}-managed interval=43200 strict=true
#!MANAGED_CONFIG https://sub.example/link/{MARKER}-lookalike?surge=4
#!MANAGED-CONFIG interval=43200 https://sub.example/managed/{MARKER}-notfirst?token={MARKER}-notfirsttoken
# #!MANAGED-CONFIG https://sub.example/commented/{MARKER}-commentedmanaged?token={MARKER}-commentedtoken interval=43200
[General]
http-api = {MARKER}-a@{MARKER}-pi@127.0.0.1:6171
external-controller-access = {MARKER}-eca,{MARKER}-ecacomma@127.0.0.1:6170
http-listen = {MARKER}-listen@127.0.0.1:6152, \"{MARKER}-quoted,{MARKER}-quotedlisten\"@127.0.0.1:6154
# private-key = {MARKER}-oldkey
;ca-passphrase = {MARKER}-oldphrase
// http-api = {MARKER}-oldapi@127.0.0.1:6171
## an old password={MARKER}-oldfree
wifi-access-http-auth = user:{MARKER}-wifi
encrypted-dns-server = https://al1ce:{MARKER}-doh@dns.example/dns-query
proxy-test-url = http://t.example/?token={MARKER}-proxytest
internet-test-url = http://t.example/?token={MARKER}-internettest
geoip-maxmind-url = https://download.example/geoip?edition_id=GeoLite2-Country&license_key={MARKER}-geoip&suffix=tar.gz

[Proxy]
Http = http, h.example, 8080, user, {MARKER}-http
Https = https, h.example, 443, username=user, password={MARKER}-https, headers=X-Auth:{MARKER}-header
Socks = socks5, s.example, 1080, user, {MARKER}-socks
Trojan = trojan, t.example, 443, password={MARKER}-trojan, ws=true, ws-path=/{MARKER}-path, ws-headers=X-Key:{MARKER}-wsheader
Vmess = vmess, v.example, 443, username={UUID}, vmess-aead=true
AnyTls = anytls, a.example, 443, password={MARKER}-anytls
Ss = ss, ss.example, 8388, encrypt-method=aes-128-gcm, password={MARKER}-ss, obfs=http, obfs-host={MARKER}-obfs.example, obfs-uri=/{MARKER}-uri
Ss2022 = ss, ss.example, 8389, encrypt-method=2022-blake3-aes-128-gcm, password={KEY_16}
SsStream = ss, ss.example, 8390, encrypt-method=aes-256-cfb, password={MARKER}-ssstream
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
Ideo = trojan, i.example, 443,\u{3000}password={MARKER}-ideographic
ExtQ = external, exec=/bin/true, local-port=1081, \"args=-p {MARKER}-quotedargs\"
#!MACOS-ONLY Req = http, req.example, 8080, user, {MARKER}-reqpos
#!REQUIREMENT CORE_VERSION>=20 ReqN = trojan, rn.example, 443, password={MARKER}-reqnamed
Inline = socks5, in.example, 1080, user, {MARKER}-inlinepos # expires=2027-01
Suffix = http, su.example, 8080, user, {MARKER}-suffixpos #!MACOS-ONLY
QType = \"http\", q.example, 8080, user, {MARKER}-quotedtype
# Old = trojan, o.example, 443, {MARKER}-commentedpolicy, password={MARKER}-commentedpassword

[Proxy Group]
G = select, Http, policy-path=https://sub.example/list?token={MARKER}-sub, external-policy-modifier=\"password={MARKER}-modifier\"
Old = url-test, Http, url=http://t.example/?token={MARKER}-groupurl
GQ = select, Http, \"policy-path=https://sub.example/list?token={MARKER}-quotedpath\"

[Host]
corp.example = server:https://al1ce:{MARKER}-hostdoh@dns.example/dns-query

[Keystore]
cert = type=p12, password={MARKER}-p12, base64={MARKER}{MARKER}
certq = type=p12, \"password={MARKER}-quotedp12\", base64={MARKER}Quoted

[WireGuard Home]
private-key = {KEY}
self-ip = 10.0.0.2
peer = (public-key = {PUBLIC}, preshared-key = {KEY}, allowed-ips = 0.0.0.0/0, endpoint = wg.example:51820)
peer = (public-key = {PUBLIC}, \"preshared-key={KEY_QUOTED}\", allowed-ips = 10.1.0.0/16, endpoint = wg2.example:51820)

[Tailscale ts]
auth-key = tskey-{MARKER}-tailscale

[MITM]
ca-passphrase = {MARKER}-ca
ca-p12 = {MARKER}{MARKER}{MARKER}

[Script]
Cookie = type=http-response, pattern=^https?://x.example, script-path=s.js, argument=cookie={MARKER}-argument

[Rule]
#!include https://sub.example/rules/{MARKER}-include.conf?token={MARKER}-includetoken
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
    use base64::Engine as _;
    use rurge_config::{DnsUpstream, HostValue};
    let loaded = load(&profile());
    assert!(
        !loaded.diagnostics.has_errors(),
        "the marker profile must be a valid one: {:#?}",
        loaded.diagnostics
    );
    // every line that takes a spec has one: the secrets are really read
    for name in [
        "Http", "Https", "Socks", "Trojan", "Vmess", "AnyTls", "Ss", "Ss2022", "SsStream", "Snell",
        "Hy2", "TuicV5", "Tuic", "Masque", "Tt", "H2", "Ssh", "Wg", "Ext", "Shadow", "Tested",
        "Ideo", "ExtQ", "Req", "ReqN", "Inline", "Suffix", "QType",
    ] {
        assert!(loaded.config.spec(name).is_some(), "{name}");
    }
    // and the spellings that only the parser's reading makes a secret of —
    // quotes around a whole item, a requirement directive in front, a comment
    // behind, a comma in a key — are read as the parameter they are written
    // for: a profile whose quotes the parser stopped taking off would still
    // load, and the redaction would have nothing to hide
    let config = &loaded.config;
    let policy = |name: &str| {
        config
            .policies
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("policy {name}"))
    };
    // a whole item in quotes
    assert_eq!(
        policy("ExtQ").params.get("args"),
        Some(format!("-p {MARKER}-quotedargs").as_str())
    );
    let group = config.groups.iter().find(|g| g.name == "GQ").unwrap();
    assert_eq!(
        group.params.get("policy-path"),
        Some(format!("https://sub.example/list?token={MARKER}-quotedpath").as_str())
    );
    let certq = config.keystore.iter().find(|k| k.name == "certq").unwrap();
    assert_eq!(certq.password, Some(format!("{MARKER}-quotedp12")));
    let peers = &config.wireguard[0].peers;
    let quoted_key: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(KEY_QUOTED)
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(
        peers[1].preshared_key.as_ref().map(|key| *key.expose()),
        Some(quoted_key)
    );
    // a requirement directive in front, an inline comment or a directive
    // behind, a type in quotes
    let user_and = |name: &str, password: &str| {
        assert_eq!(
            policy(name).positional,
            ["user".to_string(), format!("{MARKER}-{password}")],
            "{name}"
        );
    };
    user_and("Req", "reqpos");
    user_and("Inline", "inlinepos");
    user_and("Suffix", "suffixpos");
    user_and("QType", "quotedtype");
    assert_eq!(
        policy("ReqN").params.get("password"),
        Some(format!("{MARKER}-reqnamed").as_str())
    );
    // a key that holds a comma, a listener whose password is quoted with one
    let general = &config.general;
    assert_eq!(
        general
            .external_controller_access
            .as_ref()
            .map(|access| access.key.clone()),
        Some(format!("{MARKER}-eca,{MARKER}-ecacomma"))
    );
    assert_eq!(
        general.http_listen[1].password,
        Some(format!("\"{MARKER}-quoted,{MARKER}-quotedlisten\""))
    );
    // a server of `[Host]` that carries a user name and a password
    assert!(matches!(
        &config.hosts[0].value,
        HostValue::Servers(servers)
            if matches!(&servers[0], DnsUpstream::Encrypted(dns) if dns.url.contains("hostdoh"))
    ));
    // and what is commented out is not read at all
    assert!(
        general
            .http_api
            .as_ref()
            .is_some_and(|access| !access.key.contains("oldapi"))
    );
    assert!(config.policies.iter().all(|p| p.name != "Old"));
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
        "#!MANAGED-CONFIG https://sub.example/managed/{MARKER}-managedurl interval=43200 token={MARKER}-managedparam {MARKER}-bare https://other.example/{MARKER}-pasted?a=b {MARKER}Zm9vYmFy=
#!MANAGED_CONFIG https://sub.example/link/{MARKER}-directive?surge=4
#!MANAGED_CONFIG=https://sub.example/link/{MARKER}-glued?surge=4
[General]
#!MANAGED_CONFIG https://sub.example/link/{MARKER}-inside?surge=4
dns-server = htps://al1ce:{MARKER}-one@dns.example/dns-query
encrypted-dns-server = doq://b0b:{MARKER}-two@dns.example
doh-server = http://carol:{MARKER}-three@dns.example/dns-query
http-api = {MARKER}-four@api.example:6171
external-controller-access = {MARKER}-five@api.example:6170
http-listen = {MARKER}-six@listen.example:6152
socks5-listen = {MARKER}-seven@127.0.0.1:6153
wifi-access-http-auth = {MARKER}-eight
[Proxy]
Slid = snell, sn.example, 443, psk=pw, version=4 psk={MARKER}-slid
Slid2 = trojan, t.example, 443, password=pw, tfo=true password={MARKER}-slid2
Slid3 = trojan, t.example, 443, password=pw, underlying-proxy=Gone password={MARKER}-slid3
Slid4 = ssh, ssh.example, 22, username=root, private-key=nokey password={MARKER}-slid4
Slid5 = trojan, t.example, 443, password=pw, client-cert=nocert password={MARKER}-slid5
Unk = trojan, u.example, 443, password=pw, {MARKER}-aHVudGVyMg==
UnkQ = trojan, uq.example, 443, password=pw, \"{MARKER}-pa=ss\"
trojan://{MARKER}-link2@t2.example:443?sni=t2.example#HK
[Proxy Group]
UnkG = select, Slid3, {MARKER}-Zm9vYmFy==
SlidG = select, Slid3, underlying-proxy=Gone password={MARKER}-slid6
SlidH = select, Slid3, include-other-group=gone password={MARKER}-slid7
[Rule]
#!include https://sub.example/rules/{MARKER}-remoteinclude.conf
FINAL,DIRECT
"
    );
    let loaded = load(&text);
    let messages: Vec<&str> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    // a diagnostic may lower-case what it quotes
    assert_eq!(
        leaks(&messages.join("\n").to_ascii_uppercase()),
        Vec::<String>::new()
    );
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
        "unknown MANAGED-CONFIG parameter `token`",
        "unknown MANAGED-CONFIG parameter (not written as a lower-case `name=value`)",
        "unknown directive ignored: #!MANAGED_CONFIG",
        "remote include not supported yet",
        "invalid value for `version` (expected an integer from 1 to 6)",
        "invalid value for `tfo` (expected true or false)",
        "`underlying-proxy` references unknown policy",
        "`private-key` references unknown keystore item",
        "`client-cert` references unknown keystore item",
        "`include-other-group` references unknown policy group",
        "policy `Unk`: unknown parameter ignored",
        "policy `UnkQ`: unknown parameter ignored",
        "policy group `UnkG`: unknown parameter ignored",
        "policy: unknown type",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(told)),
            "{told}: {messages:#?}"
        );
    }
}

/// Lines the section parser reads only after it took something off them (a
/// comment marker, a requirement directive, an inline comment), a key that
/// holds a comma, and a `[Host]` server: the profile loads, the parser reads
/// every secret (checked first), and the redacted profile shows none.
#[test]
fn what_the_section_parser_reads_is_hidden_in_the_redacted_profile() {
    use rurge_config::{DnsUpstream, HostValue};
    let text = format!(
        "# #!MANAGED-CONFIG https://sub.example/s/{MARKER}-commented?surge=4 interval=43200 strict=true
[General]
http-api = {MARKER}-comma,tail@127.0.0.1:6171
#!MACOS-ONLY external-controller-access = {MARKER}-reqeca@127.0.0.1:6170
#!REQUIREMENT CORE_VERSION>=20 http-listen = {MARKER}-reqlisten@127.0.0.1:6152

[Proxy]
Http = http, h.example, 8080
#!REQUIREMENT CORE_VERSION>=20 Pos = http, p.example, 8080, user, {MARKER}-reqpos
PosC = socks5, s.example, 1080, user, {MARKER}-poscomment # expires=2027-01
Ext = external, exec=/bin/true, local-port=1080, \"args=-p {MARKER}-quotedargs\" // tunnel

[Proxy Group]
GQ = select, Http, \"policy-path=https://sub.example/list?token={MARKER}-quotedpath\" #!MACOS-ONLY

[Keystore]
certq = type=p12, base64=AAAA, \"password={MARKER}-quotedp12\" ; work

[Host]
corp.example = server:https://al1ce:{MARKER}-hostdoh@dns.example/dns-query

[MITM]
#!MACOS-ONLY ca-passphrase = {MARKER}-reqca

[Rule]
FINAL,DIRECT
"
    );
    let loaded = load(&text);
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:#?}",
        loaded.diagnostics
    );
    // the parser reads every one of them
    let general = &loaded.config.general;
    assert_eq!(
        general.http_api.as_ref().map(|c| c.key.clone()),
        Some(format!("{MARKER}-comma,tail"))
    );
    assert_eq!(
        general
            .external_controller_access
            .as_ref()
            .map(|c| c.key.clone()),
        Some(format!("{MARKER}-reqeca"))
    );
    assert_eq!(
        general.http_listen.first().and_then(|l| l.password.clone()),
        Some(format!("{MARKER}-reqlisten"))
    );
    let positional = |name: &str| -> Vec<String> {
        loaded
            .config
            .policies
            .iter()
            .find(|p| p.name == name)
            .map(|p| p.positional.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        positional("Pos"),
        ["user".to_string(), format!("{MARKER}-reqpos")]
    );
    assert_eq!(
        positional("PosC"),
        ["user".to_string(), format!("{MARKER}-poscomment")]
    );
    let args = loaded
        .config
        .policies
        .iter()
        .find(|p| p.name == "Ext")
        .and_then(|p| p.params.get("args").map(str::to_string));
    assert_eq!(args, Some(format!("-p {MARKER}-quotedargs")));
    assert_eq!(
        loaded.config.groups[0]
            .params
            .get("policy-path")
            .map(str::to_string),
        Some(format!(
            "https://sub.example/list?token={MARKER}-quotedpath"
        ))
    );
    assert_eq!(
        loaded.config.keystore[0].password,
        Some(format!("{MARKER}-quotedp12"))
    );
    assert!(matches!(
        &loaded.config.hosts[0].value,
        HostValue::Servers(servers)
            if matches!(&servers[0], DnsUpstream::Encrypted(dns) if dns.url.contains("hostdoh"))
    ));
    // and the redacted profile shows none
    assert_eq!(leaks(&redact_profile(&text)), Vec::<String>::new());
}

/// More values another field slid into, a field nobody reads, a link that
/// is no policy line: no diagnostic quotes them and no `Debug` shows them.
#[test]
fn more_diagnostics_and_debug_hold_no_secret() {
    let text = format!(
        "[Proxy]
trojan://{MARKER}-link@t.example:443?sni=t.example#HK

[Keystore]
cert = type=p12, passphrase={MARKER}-misspelt, base64=AAAA
cert2 = type=p12 password={MARKER}-slidtype, base64=AAAA

[Host]
corp.example = server:htps://al1ce:{MARKER}-hostdiag@dns.example/dns-query

[WireGuard Home]
private-key = {KEY}
self-ip = 10.0.0.2
dns-server = https://al1ce:{MARKER}-wgdns@dns.example/dns-query
peer = (public-key = {PUBLIC}, allowed-ips = 0.0.0.0/0, endpoint = wg.example:51820 preshared-key = {KEY})

[Rule]
FINAL,DIRECT
"
    );
    let loaded = load(&text);
    // `unknown type` lower-cases what it quotes
    let messages: Vec<String> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.to_ascii_uppercase())
        .collect();
    let mut found = leaks(&messages.join("\n"));
    found.extend(leaks(&format!("{:#?}", loaded.config)));
    assert_eq!(found, Vec::<String>::new());
}

/// A secret parameter behind what an input method or a copy from a web page
/// puts in front of it, or written with a full-width `=`: the parsers read
/// none of these as the parameter, and the redacted profile shows no value.
#[test]
fn a_secret_behind_full_width_or_invisible_characters_is_hidden() {
    let lines = [
        // a comma typed with a Chinese input method
        format!("G = select, A\u{ff0c}policy-path=https://sub.example/list?token={MARKER}-fwcomma"),
        format!(
            "HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm\u{ff0c}password={MARKER}-fwcomma2"
        ),
        // a zero-width space, a byte-order mark
        format!("Ext = external, exec=/bin/true, local-port=1080, \u{200b}args=-p {MARKER}-zwsp"),
        format!("T = trojan, t.example, 443, \u{feff}password={MARKER}-bom"),
        // typographic quotes around the item
        format!(
            "G = select, A, \u{201c}policy-path=https://sub.example/list?token={MARKER}-curly\u{201d}"
        ),
        // a full-width parenthesis
        format!("G = select, A\u{ff08}policy-path=https://sub.example/list?token={MARKER}-fwparen"),
        // a full-width equals sign
        format!("Ext = external, exec=/bin/true, local-port=1080, args\u{ff1d}-p {MARKER}-fweq"),
        format!("ca-passphrase \u{ff1d} {MARKER}-fweq2"),
    ];
    let shown: Vec<String> = lines
        .iter()
        .map(|line| redact_profile(line))
        .filter(|shown| shown.contains(MARKER))
        .collect();
    assert_eq!(shown, Vec::<String>::new());
}

/// Lines that do not load are served all the same (the redacted profile is
/// read from the file as it is on disk): what the diagnostics already treat
/// as a credential is hidden there too.
#[test]
fn a_line_that_does_not_load_shows_no_secret_either() {
    let lines = [
        // an API key written without its address
        format!("http-api = {MARKER}-keyonly"),
        // the `=` after the name is missing, or the name is
        format!("P http, h.example, 8080, alice, {MARKER}-noeq, tfo=true"),
        format!("http, h.example, 8080, alice, {MARKER}-noname"),
        // a share link pasted as a line
        format!("trojan://{MARKER}-link@t.example:443?sni=t.example#HK"),
        // Surge's old Shadowsocks line
        format!(
            "Old = custom, 1.2.3.4, 443, chacha20-ietf-poly1305, {MARKER}-custom, https://example.com/SSEncrypt.module"
        ),
        // a peer's key on a line of its own, as wg-quick writes it
        format!("preshared-key = {KEY}"),
        format!("PrivateKey = {KEY}"),
        format!("Pre_Shared_Key = {KEY}"),
        // a key written with a colon
        format!("ca-passphrase: {MARKER}-colon"),
        // a line for another client, and a type in quotes
        format!("Loon = Shadowsocks, loon.example, 443, aes-128-gcm, \"{MARKER}-loon\""),
        format!("Q = \"http\", h.example, 8080, alice, {MARKER}-quotedtype"),
        format!(", http, h.example, 8080, alice, {MARKER}-emptyitem"),
        // a password that lost its comma
        format!("P = http, h.example, 80, {MARKER}-nocomma tfo=true"),
        // what a comment says
        format!("# an old password={MARKER}-oldfree"),
        format!("// Old = http, h.example, 80, alice, {MARKER}-fakepass"),
        // a URL that carries its credentials
        format!("RULE-SET,https://al1ce:{MARKER}-ruleset@rules.example/list.txt,DIRECT"),
        format!("corp.example = server:https://al1ce:{MARKER}-hosturl@dns.example/dns-query"),
        format!("# was https://al1ce:{MARKER}-commentedurl@sub.example/list"),
    ];
    let shown: Vec<String> = lines
        .iter()
        .map(|line| redact_profile(line))
        .filter(|shown| shown.contains(MARKER))
        .collect();
    assert_eq!(shown, Vec::<String>::new());
}
