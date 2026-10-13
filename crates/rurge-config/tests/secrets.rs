//! No credential leaves the configuration by a side door: not in the
//! `Debug` of anything loaded, not in a diagnostic, not in the redacted
//! profile. Every secret below carries the marker `S3CR3T`, or is the one
//! UUID, so one search finds whatever got out.

use rurge_config::config::{LoadOptions, Loaded, from_text, load as load_file};
use rurge_config::redact::redact_profile;
use rurge_config::{Remote, RemoteSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use url::Url;

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
DOMAIN-SET:https://al1ce:{MARKER}-hostdomainset@sets.example/hosts.txt = 10.1.1.1
RULE-SET:https://al1ce:{MARKER}-hostruleset@sets.example/hosts.list = server:8.8.8.8

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
RULE-SET,https://al1ce:{MARKER}-ruleset@rules.example/list.txt,DIRECT
DOMAIN-SET,https://al1ce:{MARKER}-domainset@rules.example/domains.txt,REJECT
AND,((PROTOCOL,UDP),(RULE-SET,https://al1ce:{MARKER}-nestedset@rules.example/nested.list)),DIRECT
FINAL,DIRECT

[Ruleset Inline]
RULE-SET,https://al1ce:{MARKER}-inlineset@rules.example/inline.list
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
    use rurge_config::{DnsUpstream, HostKey, HostValue, ResourceRef, RuleKind};
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
    // the URL of a set keeps its user name and its password in what is read,
    // for that is what is fetched: the views are what hide them
    let user_info = |what: &str| format!("al1ce:{MARKER}-{what}@");
    let set_url = |kind: &RuleKind| match kind {
        RuleKind::RuleSet(ResourceRef::Url(url)) | RuleKind::DomainSet(ResourceRef::Url(url)) => {
            url.clone()
        }
        other => panic!("not a set with a URL: {other:?}"),
    };
    assert!(set_url(&config.rules[0].kind).contains(&user_info("ruleset")));
    assert!(set_url(&config.rules[1].kind).contains(&user_info("domainset")));
    let RuleKind::And(parts) = &config.rules[2].kind else {
        panic!("{:?}", config.rules[2].kind)
    };
    assert!(set_url(&parts[1].kind).contains(&user_info("nestedset")));
    assert!(set_url(&config.rulesets[0].rules[0].kind).contains(&user_info("inlineset")));
    assert!(matches!(
        &config.hosts[1].key,
        HostKey::DomainSet(ResourceRef::Url(url)) if url.contains(&user_info("hostdomainset"))
    ));
    assert!(matches!(
        &config.hosts[2].key,
        HostKey::RuleSet(ResourceRef::Url(url)) if url.contains(&user_info("hostruleset"))
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

/// What a loaded profile writes out as text. A rule as `Display` gives it is
/// what the request record, the API's rule list, `rule match --explain` and
/// the summary show; the key of a `[Host]` entry goes to the summary and to
/// the resolver's messages.
#[test]
fn the_texts_a_loaded_profile_gives_out_hold_no_secret() {
    let loaded = load(&profile());
    let config = &loaded.config;
    let summary = config.summary();
    // the summary derives `Debug` and `Serialize` and has a field for where a
    // managed profile comes from. That is the link, a credential as a whole:
    // the summary shows where it goes and nothing more
    assert_eq!(
        summary.managed.as_deref(),
        Some("https://sub.example/\u{2026}")
    );
    let whole = format!("{summary:?} {}", serde_json::to_string(&summary).unwrap());
    assert_eq!(leaks(&whole), Vec::<String>::new());
    let mut texts: Vec<String> = config.rules.iter().map(ToString::to_string).collect();
    texts.extend(
        config
            .rulesets
            .iter()
            .flat_map(|set| set.rules.iter().map(|rule| rule.raw.clone())),
    );
    texts.extend(config.hosts.iter().map(|host| host.raw_key.clone()));
    texts.extend(summary.rules);
    texts.extend(summary.hosts);
    texts.extend(summary.managed);
    assert_eq!(leaks(&texts.join("\n")), Vec::<String>::new());
    // and they still say which line it was
    for told in [
        "RULE-SET,https://***@rules.example/list.txt,DIRECT",
        "DOMAIN-SET,https://***@rules.example/domains.txt,REJECT",
        "AND,((PROTOCOL,UDP),(RULE-SET,https://***@rules.example/nested.list)),DIRECT",
        "RULE-SET,https://***@rules.example/inline.list",
        "DOMAIN-SET:https://***@sets.example/hosts.txt",
        "RULE-SET:https://***@sets.example/hosts.list",
    ] {
        assert!(texts.iter().any(|t| t == told), "{told}: {texts:#?}");
    }
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
        "remote include not read: no data directory",
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
/// The `[WireGuard]` section is one that does not load, for its diagnostics;
/// a section that loads is the next test's.
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
    // the loader said what the lines are there to provoke
    let said: Vec<&str> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    for told in [
        "policy: unknown type",
        "`corp.example`: invalid DNS server `htps://***@dns.example/dns-query`",
        "unknown [Keystore] field `passphrase` ignored",
        "keystore item `cert2`: unknown type",
        "`dns-server` `https://***@dns.example/dns-query`: an encrypted-DNS URL is not accepted here",
        "peer 1: invalid `endpoint` (expected host:port)",
    ] {
        assert!(said.iter().any(|m| m.contains(told)), "{told}: {said:#?}");
    }
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

/// A `[WireGuard]` section that loads, a peer's pre-shared key among its
/// lines: the keys are read, and the `Debug` of what the loader returns
/// shows neither.
#[test]
fn a_wireguard_section_that_loads_shows_no_key_in_debug() {
    use base64::Engine as _;
    let text = format!(
        "[Proxy]
Wg = wireguard, section-name=Home

[WireGuard Home]
private-key = {KEY}
self-ip = 10.0.0.2
dns-server = 1.1.1.1
peer = (public-key = {PUBLIC}, preshared-key = {KEY}, allowed-ips = 0.0.0.0/0, endpoint = wg.example:51820)

[Rule]
FINAL,DIRECT
"
    );
    let loaded = load(&text);
    assert!(
        !loaded.diagnostics.has_errors() && loaded.diagnostics.is_empty(),
        "{:#?}",
        loaded.diagnostics
    );
    // the section loads, and both keys are read
    let key: [u8; 32] = base64::engine::general_purpose::STANDARD
        .decode(KEY)
        .unwrap()
        .try_into()
        .unwrap();
    let section = &loaded.config.wireguard[0];
    assert_eq!(*section.private_key.expose(), key);
    assert_eq!(
        section.peers[0].preshared_key.as_ref().map(|k| *k.expose()),
        Some(key)
    );
    // the Debug has the section, and not what is secret in it
    let shown = format!("{:#?}", loaded.config);
    assert!(shown.contains("WireGuardSection"), "{shown}");
    assert_eq!(leaks(&shown), Vec::<String>::new());
}

/// A secret parameter behind what an input method or a copy from a web page
/// puts in front of it, or written with a full-width `=`: spellings a person
/// means as the parameter, whatever a parser makes of them, and the redacted
/// profile shows no value after any of them.
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

/// The redacted form of each line that still holds a secret.
fn shown_with_secret(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| redact_profile(line))
        .filter(|shown| !leaks(shown).is_empty())
        .collect()
}

/// A key the manual has and neither list of secrets named.
#[test]
fn the_secret_of_the_mtproto_server_is_hidden() {
    let text = format!(
        "[MTProto]\ninterface = 0.0.0.0\nport = 8443\nsecret = dd0123456789abcdef{MARKER}0123456789\n\n[Rule]\nFINAL,DIRECT\n"
    );
    let loaded = load(&text);
    // the parser keeps the section, its lines with it
    assert!(
        loaded
            .config
            .summary()
            .deferred
            .iter()
            .any(|s| s.eq_ignore_ascii_case("MTProto")),
        "{:#?}",
        loaded.diagnostics
    );
    let mut found = leaks(&redact_profile(&text));
    found.extend(leaks(&format!("{:#?}", loaded.config)));
    assert_eq!(found, Vec::<String>::new());
}

/// Something stands in front of a whole-line key.
#[test]
fn a_key_with_something_in_front_of_it_is_hidden() {
    let lines = [
        format!("\u{ff1b}ca-passphrase = {MARKER}-fwsemi"),
        format!("\u{ff1b} http-api = {MARKER}-fwsemiapi@127.0.0.1:6171"),
        format!("\u{ff1b}private-key = {KEY}"),
        format!("# was http-api = {MARKER}-wasapi@127.0.0.1:6171"),
        format!("# old: ca-passphrase = {MARKER}-oldcolon"),
    ];
    assert_eq!(shown_with_secret(&lines), Vec::<String>::new());
}

/// A secret named with a colon: in a comment, in a line for another client.
#[test]
fn a_secret_named_with_a_colon_is_hidden_too() {
    let lines = [
        format!("# an old password: {MARKER}-colonprose"),
        format!("P = trojan, t.example, 443, password=x // old password: {MARKER}-colontail"),
        format!(
            "# - {{name: HK, type: ss, server: s.example, port: 8388, cipher: aes-256-gcm, password: {MARKER}-clashflow}}"
        ),
        format!("    uuid: {UUID}"),
        format!("    \"password\": \"{MARKER}-json\","),
    ];
    assert_eq!(shown_with_secret(&lines), Vec::<String>::new());
}

/// The other keys that stand on a line of their own, written with a colon.
#[test]
fn a_whole_line_key_written_with_a_colon_is_hidden_whatever_the_key() {
    let lines = [
        format!("http-api: {MARKER}-colonapi@127.0.0.1:6171"),
        format!("external-controller-access: {MARKER}-coloneca@0.0.0.0:6170"),
        format!("http-listen: {MARKER}-colonlisten@0.0.0.0:6152"),
        format!("wifi-access-http-auth: user:{MARKER}-colonwifi"),
    ];
    assert_eq!(shown_with_secret(&lines), Vec::<String>::new());
}

/// A share link that carries its secret anywhere but in a user-info.
#[test]
fn a_share_link_shows_nothing_of_what_follows_its_scheme() {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let vmess = b64.encode(format!(
        "{{\"v\":\"2\",\"ps\":\"HK\",\"add\":\"v.example\",\"port\":\"443\",\"id\":\"{UUID}\",\"aid\":\"0\",\"net\":\"ws\",\"tls\":\"tls\"}}"
    ));
    let legacy = b64.encode(format!("aes-256-gcm:{MARKER}-legacyss@ss.example:8388"));
    let auth = format!("{MARKER}-hyauth");
    let obfs = format!("{MARKER}-hy2obfs");
    let cases = [
        (format!("vmess://{vmess}"), vmess.clone()),
        (format!("# vmess://{vmess}"), vmess.clone()),
        (format!("ss://{legacy}#HK"), legacy.clone()),
        (
            format!("hysteria://hy.example:443?protocol=udp&auth={auth}&peer=hy.example#HK"),
            auth.clone(),
        ),
        (
            format!(
                "hysteria2://{MARKER}-hy2@hy.example:443/?obfs=salamander&obfs-password={obfs}#HK"
            ),
            obfs.clone(),
        ),
    ];
    let shown: Vec<String> = cases
        .iter()
        .map(|(line, secret)| (redact_profile(line), secret))
        .filter(|(shown, secret)| shown.contains(secret.as_str()))
        .map(|(shown, _)| shown)
        .collect();
    assert_eq!(shown, Vec::<String>::new());
}

/// The comma after the type is the one that is missing.
#[test]
fn a_policy_line_that_lost_the_comma_after_its_type_shows_no_credential() {
    let lines = [
        format!("P = http h.example, 8080, alice, {MARKER}-typecomma"),
        format!("S = socks5 1.2.3.4, 1080, user, {MARKER}-typecomma2"),
        format!("# Old = https h.example, 443, bob, {MARKER}-typecomma3"),
    ];
    assert_eq!(shown_with_secret(&lines), Vec::<String>::new());
}

/// A link in a comment.
#[test]
fn a_link_in_a_comment_shows_no_token() {
    let lines = [
        format!(
            "# \u{8ba2}\u{9605}: https://sub.example/api/v1/client/subscribe?token={MARKER}-commentlink"
        ),
        format!("# updated from https://sub.example/link/{MARKER}-commentpath?surge=4"),
        format!("// https://sub.example/link/{MARKER}-barelink"),
    ];
    assert_eq!(shown_with_secret(&lines), Vec::<String>::new());
}

/// What an inline comment holds besides a name with a blank in front of it.
#[test]
fn an_inline_comment_is_redacted_as_a_commented_out_line_is() {
    let lines = [
        format!("P = trojan, t.example, 443, password=x #password={MARKER}-gluedtail"),
        format!(
            "P = http, h.example, 8080, alice, x # was: http, old.example, 8080, alice, {MARKER}-oldtail"
        ),
    ];
    assert_eq!(shown_with_secret(&lines), Vec::<String>::new());
}

/// A line that lands in another section than its author meant.
#[test]
fn a_line_in_the_wrong_section_is_neither_quoted_nor_kept() {
    let texts = [
        // a share link pasted at the end of a profile that ends with [Rule]
        format!(
            "[Rule]\nFINAL,DIRECT\ntrojan://{MARKER}-rulelink@t.example:443?sni=t.example#HK\n"
        ),
        // ... or under [Host], where it loads: an alias
        format!(
            "[Host]\ntrojan://{MARKER}-hostlink@t.example:443?sni=t.example#HK\n\n[Rule]\nFINAL,DIRECT\n"
        ),
        // a header the parser does not take for one: the keys below it are
        // lines of the section above
        format!("[Rule]\nFINAL,DIRECT\n[MITM] # my certificate\nca-passphrase = {MARKER}CA\n"),
        format!(
            "[Proxy]\nHttp = http, h.example, 8080\n[Snell Server] # mine\ninterface = 0.0.0.0\nport = 6160\npsk = {MARKER}psk\n\n[Rule]\nFINAL,DIRECT\n"
        ),
    ];
    let mut found = Vec::new();
    for text in &texts {
        let loaded = load(text);
        // a diagnostic may change the case of what it quotes
        let messages: Vec<String> = loaded
            .diagnostics
            .iter()
            .map(|d| d.message.to_ascii_uppercase())
            .collect();
        found.extend(leaks(&messages.join("\n")));
        found.extend(leaks(&format!("{:#?}", loaded.config)));
    }
    assert_eq!(found, Vec::<String>::new());
}

/// What stands where the parameters of a rule stand and is none — a field of
/// another kind of line, a `name=value` nobody knows — is not quoted by the
/// warning about it: only a name that is written like a parameter is. The
/// line itself stays what the views of the rules show: a rule is shown as it
/// is written, with only the user name and password of a link left out.
#[test]
fn what_stands_among_the_parameters_of_a_rule_is_not_quoted() {
    let text = format!(
        "[Rule]\nDOMAIN,a.example,DIRECT,{MARKER}-flag\nDOMAIN,b.example,DIRECT,token={MARKER}-value\nAND,((DOMAIN,c.example,{MARKER}-sub),(DEST-PORT,443)),DIRECT\nRULE-SET,Mine,DIRECT\nFINAL,DIRECT\n[Ruleset Mine]\nDOMAIN,d.example,{MARKER}-policy\n"
    );
    let loaded = load(&text);
    assert!(!loaded.diagnostics.has_errors());
    let said: Vec<&str> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == rurge_config::codes::W_UNKNOWN_RULE_PARAM)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        said,
        [
            "unknown [Ruleset Mine] parameter ignored",
            "unknown rule parameter ignored",
            "unknown rule parameter `token` ignored",
            "unknown sub-rule parameter ignored",
        ]
    );
    let messages: Vec<String> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.to_ascii_uppercase())
        .collect();
    assert_eq!(leaks(&messages.join("\n")), Vec::<String>::new());
}

/// A header the section parser does not take for one hands every line below
/// it to the section above. Under `[General]` the "key" of such a line is a
/// policy's name, or, when the `=` after the name is missing or the line is a
/// share link, text with a password in it: only a key that can be one is
/// quoted.
#[test]
fn a_line_under_general_that_is_no_setting_is_not_quoted() {
    let text = format!(
        "[General]\nloglevel = notify\nmy-key = 1\nHttp = http, h.example, 8080, alice, {MARKER}-named\ntrojan://{MARKER}-link@t.example:443?sni=t.example#HK\nP http h.example, 8080, alice, {MARKER}-nocomma, tfo=true\n\n[Rule]\nFINAL,DIRECT\n"
    );
    let loaded = load(&text);
    let said: Vec<&str> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == rurge_config::codes::W_UNKNOWN_KEY)
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        said,
        [
            "unknown [General] key `my-key` ignored",
            "unknown [General] key `http` ignored",
            "unknown [General] key ignored",
            "unknown [General] key ignored",
        ]
    );
    let messages: Vec<String> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.to_ascii_uppercase())
        .collect();
    let mut found = leaks(&messages.join("\n"));
    found.extend(leaks(&format!("{:#?}", loaded.config)));
    assert_eq!(found, Vec::<String>::new());
}

/// Text that is no name — a share link, what an input method types for an `=`
/// or a `,` — is neither quoted by the diagnostics nor shown by the `Debug`
/// and the summary, wherever a policy, a group, one of its members, a rule's
/// policy or a keystore item is named.
#[test]
fn a_name_that_is_none_is_neither_quoted_nor_shown() {
    let text = format!(
        "[Proxy]
A = http, a.example, 8080
P snell\u{ff0c} h.example\u{ff0c} 443\u{ff0c} alice\u{ff0c} {MARKER}-fwcomma\u{ff0c} tfo=true

[Proxy Group]
G = select, A, policy-path:https://sub.example/link/{MARKER}-member, policy-path\u{ff1d}https://sub.example/link/{MARKER}-fwmember

[Keystore]
k\u{ff0c}{MARKER}-keystore = type=jks, base64=AAAA

[Rule]
FINAL,DIRECT
"
    );
    let loaded = load(&text);
    let said: Vec<&str> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    for told in [
        "policy: unknown type `true`",
        "policy group `G` references an unknown policy",
        "keystore item: unknown type `jks`",
    ] {
        assert!(said.contains(&told), "{told}: {said:#?}");
    }
    let messages: Vec<String> = said.iter().map(|m| m.to_ascii_uppercase()).collect();
    let summary = loaded.config.summary();
    let mut found = leaks(&messages.join("\n"));
    found.extend(leaks(&format!("{:#?}", loaded.config)));
    found.extend(leaks(&summary.groups.join("\n")));
    assert_eq!(found, Vec::<String>::new());
    // a member that is a name is shown as it was
    assert_eq!(summary.groups, ["G (select) -> [A, ***, ***]"]);
    // and a rule whose policy is none is not quoted either
    let loaded = load(&format!(
        "[Rule]\nDOMAIN,a.com,policy-path\u{ff1d}https://sub.example/link/{MARKER}-rulepolicy\nFINAL,DIRECT\n"
    ));
    let said: Vec<&str> = loaded
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(said, ["rule references an unknown policy"]);
}

/// What the links of the profile below were downloaded as: texts that hold
/// credentials of their own.
struct Downloaded(HashMap<String, Remote>);

impl RemoteSource for Downloaded {
    fn get(&self, url: &Url) -> Remote {
        self.0
            .get(url.as_str())
            .cloned()
            .unwrap_or(Remote::Missing {
                why: Some("no answer".to_string()),
            })
    }
}

fn downloaded(text: String, n: u32) -> Remote {
    copy_at(text, n, SystemTime::UNIX_EPOCH)
}

/// The link of a remote include is a credential as a whole: its user name,
/// password, path and query. Nothing of it, and nothing of the text that was
/// downloaded from it, comes out of a load: not in the `Debug` of what loaded
/// or of the options, not in a diagnostic, not in the redacted profile.
#[test]
fn a_remote_include_shows_nothing_of_its_link_or_its_text() {
    let rules = format!(
        "https://{MARKER}-name:{MARKER}-user@sub.example/rules/{MARKER}-path.conf?token={MARKER}-query"
    );
    let proxies = format!(
        "https://sub.example/proxies/{MARKER}-proxies?token={MARKER}-proxiesquery#{MARKER}-fragment"
    );
    let gone = format!("https://sub.example/gone/{MARKER}-gone?token={MARKER}-gonequery");
    let page = format!("https://sub.example/page/{MARKER}-page");
    let profile = format!(
        "[Proxy]\n#!include {proxies}\n[Rule]\n#!include {rules}\n#!include {gone}\n#!include {page}\nFINAL,DIRECT\n"
    );
    // the loader asks for a link without its fragment, which no server sees
    let key = |link: &str| {
        let mut url = Url::parse(link).unwrap();
        url.set_fragment(None);
        url.to_string()
    };
    let answers = Downloaded(HashMap::from([
        (
            key(&proxies),
            downloaded(
                format!(
                    "[Proxy]\nT = trojan, t.example, 443, password={MARKER}-remotepw\nE = external, exec=/bin/true, local-port=1090, args={MARKER}-remoteargs\n#!include {MARKER}-local.dconf\n"
                ),
                1,
            ),
        ),
        (
            key(&rules),
            downloaded(
                format!(
                    "#!MANAGED-CONFIG https://other.example/{MARKER}-managed interval=7200 strict=true\n[Rule]\nRULE-SET,https://al1ce:{MARKER}-remoteset@rules.example/r.list,DIRECT\nDOMAIN,a.example,T\n"
                ),
                2,
            ),
        ),
        (
            key(&gone),
            Remote::Missing {
                why: Some("connection refused".to_string()),
            },
        ),
        (
            key(&page),
            downloaded(format!("<html>{MARKER}-html</html>\n"), 3),
        ),
    ]));
    let mut opts = LoadOptions::for_tests();
    opts.remote = Some(Arc::new(answers));
    let loaded = from_text(&profile, Path::new("t.conf"), &opts);

    // the links were read, or the checks below would find nothing to find
    let remote = &loaded.config.source.remote;
    assert_eq!(remote.len(), 4);
    let found: Vec<bool> = remote.iter().map(|r| r.found()).collect();
    assert_eq!(found, [true, true, false, true]);
    assert!(remote[3].lacks_section);
    assert_eq!(remote[1].interval.as_secs(), 7200);
    assert!(loaded.config.policies.iter().any(|p| p.name == "T"));
    assert!(loaded.config.spec("T").is_some());
    assert!(loaded.config.spec("E").is_none());
    let codes: Vec<&str> = loaded.diagnostics.iter().map(|d| d.code).collect();
    for code in ["W0015", "W0017", "W0022", "W0033"] {
        assert!(codes.contains(&code), "{code}: {codes:?}");
    }

    let mut found = leaks(&format!("{loaded:#?}"));
    for d in loaded.diagnostics.iter() {
        found.extend(leaks(&d.to_string()));
        found.extend(leaks(&format!("{d:?}")));
    }
    for r in remote {
        found.extend(leaks(&format!("{r:?}")));
    }
    found.extend(leaks(&format!("{opts:?}")));
    found.extend(leaks(&redact_profile(&profile)));
    assert_eq!(found, Vec::<String>::new());

    // what is shown of a link is where it goes
    let shown = format!("{:?}", remote[0]);
    assert!(shown.contains("https://sub.example/\u{2026}"), "{shown}");
    // and of the options only that there is a source
    assert!(format!("{opts:?}").contains("remote: Some("));
    assert!(format!("{:?}", LoadOptions::for_tests()).contains("remote: None"));
}

/// A text that was downloaded at `at`, kept in a file of its own.
fn copy_at(text: String, n: u32, at: SystemTime) -> Remote {
    Remote::Text {
        text,
        path: PathBuf::from(format!("/cache/{n}.conf")),
        fetched_at: at,
    }
}

/// The link of a managed profile is a credential as a whole, in the local
/// file and in every copy on the way: its user name, password, path, query and
/// fragment. Nothing of it, and nothing of a copy's text, comes out of a load
/// of any of the ways it can go: not in the `Debug` of what loaded, not in a
/// diagnostic or in the summary, not in the redacted profile.
#[test]
fn a_managed_profile_shows_nothing_of_its_link_or_its_copy() {
    let local_link = format!(
        "https://{MARKER}-name:{MARKER}-user@sub.example/managed/{MARKER}-path?token={MARKER}-query#{MARKER}-fragment"
    );
    let moved_link =
        format!("https://sub.example/moved/{MARKER}-moved?token={MARKER}-movedquery#{MARKER}-frag");
    let local =
        format!("#!MANAGED-CONFIG {local_link} interval=43200 strict=true\n[Rule]\nFINAL,DIRECT\n");
    let body = format!(
        "[Proxy]\nT = trojan, t.example, 443, password={MARKER}-copypw\n[Rule]\nDOMAIN,a.example,T\nFINAL,DIRECT\n"
    );
    let naming = |to: &str| format!("#!MANAGED-CONFIG {to} interval=7200 strict=true\n{body}");
    // the loader asks for a link without its fragment, which no server sees
    let key = |link: &str| {
        let mut url = Url::parse(link).unwrap();
        url.set_fragment(None);
        url.to_string()
    };
    let at = |seconds: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
    let (before, downloaded_at, after) = (at(1_000_000_000), at(1_100_000_000), at(1_200_000_000));
    let moved_never_downloaded = || {
        HashMap::from([
            (
                key(&local_link),
                copy_at(naming(&moved_link), 1, downloaded_at),
            ),
            (
                key(&moved_link),
                Remote::Missing {
                    why: Some("connection refused".to_string()),
                },
            ),
        ])
    };
    // the scene, what the remote files are, when the local file was changed,
    // whether a copy is in effect, and a code the load has to say
    let scenes = [
        (
            "an address that moved and was never downloaded",
            moved_never_downloaded(),
            before,
            true,
            "W0022",
        ),
        (
            "the same, with the local file changed after the download",
            moved_never_downloaded(),
            after,
            false,
            "I0004",
        ),
        (
            "a copy that names no address",
            HashMap::from([(key(&local_link), copy_at(body.clone(), 1, downloaded_at))]),
            before,
            true,
            "W0034",
        ),
        (
            "addresses that lead back to each other",
            HashMap::from([
                (
                    key(&local_link),
                    copy_at(naming(&moved_link), 1, downloaded_at),
                ),
                (
                    key(&moved_link),
                    copy_at(naming(&local_link), 2, downloaded_at),
                ),
            ]),
            before,
            true,
            "W0034",
        ),
    ];
    for (what, answers, modified, copy_in_effect, code) in scenes {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.conf");
        std::fs::write(&main, &local).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&main)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let mut opts = LoadOptions::for_tests();
        opts.remote = Some(Arc::new(Downloaded(answers)));
        let loaded = load_file(&main, &opts).unwrap();

        // the links were read, and the text in effect is the one meant, or the
        // checks below would find nothing to find
        let said: Vec<&str> = loaded.diagnostics.iter().map(|d| d.code).collect();
        assert!(said.contains(&code), "{what}: {said:?}");
        assert!(!loaded.config.source.remote.is_empty(), "{what}");
        assert_eq!(
            loaded.config.spec("T").is_some(),
            copy_in_effect,
            "{what}: {said:?}"
        );

        let mut found = leaks(&format!("{loaded:#?}"));
        for d in loaded.diagnostics.iter() {
            found.extend(leaks(&d.to_string()));
            found.extend(leaks(&format!("{d:?}")));
        }
        for r in &loaded.config.source.remote {
            found.extend(leaks(&format!("{r:?}")));
        }
        let summary = loaded.config.summary();
        found.extend(leaks(&format!(
            "{summary:?} {}",
            serde_json::to_string(&summary).unwrap()
        )));
        found.extend(leaks(&format!("{:?}", loaded.config.managed)));
        found.extend(leaks(&format!("{opts:?}")));
        found.extend(leaks(&redact_profile(&local)));
        found.extend(leaks(&redact_profile(&naming(&moved_link))));
        assert_eq!(found, Vec::<String>::new(), "{what}");
    }
}

/// A downloaded text that names local files — to include, as a set, as a
/// `policy-path`, and a program to start — is not obeyed, and says so without
/// the paths: what a text names on this machine is the text's, and a path
/// that comes back in a diagnostic or a `Debug` tells whoever reads them what
/// the text's author wanted to find out. The paths below carry the marker.
#[test]
fn a_downloaded_text_that_names_local_files_shows_none_of_the_paths() {
    // the lines of such a text, whichever section it is read for
    let lines = format!(
        "[Proxy]
Ext = external, exec=/bin/true, local-port=1090
#!include /{MARKER}-included.dconf
#!include ../{MARKER}-up.dconf
#!include C:\\{MARKER}-drive.dconf
#!include sets/{MARKER}-plain.dconf
[Proxy Group]
G = select, Ext, policy-path={MARKER}-nodes.txt
H = select, Ext, policy-path=/{MARKER}-absolute-nodes.txt
[Host]
DOMAIN-SET:{MARKER}-hosts.list = 10.0.0.1
RULE-SET:/{MARKER}-absolute-hosts.list = 10.0.0.2
[Ruleset Inline]
RULE-SET,{MARKER}-nested.list
[Rule]
RULE-SET,sets/{MARKER}-a.list,DIRECT
DOMAIN-SET,../{MARKER}-b.list,DIRECT
AND,((RULE-SET,{MARKER}-c.list),(DEST-PORT,443)),DIRECT
NOT,((DOMAIN-SET,/{MARKER}-d.list)),DIRECT
"
    );
    // a profile that takes `link` in under every section the text has
    let sections = ["Proxy", "Proxy Group", "Host", "Ruleset Inline", "Rule"];
    let including = |link: &str| {
        let mut profile = String::new();
        for section in sections {
            profile += &format!("[{section}]\n#!include {link}\n");
        }
        profile + "FINAL,DIRECT\n"
    };
    // the loader asks for a link without its fragment, which no server sees
    let key = |link: &str| {
        let mut url = Url::parse(link).unwrap();
        url.set_fragment(None);
        url.to_string()
    };
    let mut loads = Vec::new();

    // the text of a remote include
    let link = format!("https://sub.example/names-files/{MARKER}-link?token={MARKER}-linktoken");
    let profile = including(&link);
    let mut opts = LoadOptions::for_tests();
    opts.remote = Some(Arc::new(Downloaded(HashMap::from([(
        key(&link),
        downloaded(lines.clone(), 1),
    )]))));
    loads.push((
        "a remote include",
        from_text(&profile, Path::new("t.conf"), &opts),
        profile,
        // the four includes of the text, once for each of the five sections
        // it is taken in under, and nine more lines
        29,
    ));

    // the copy of a managed profile in effect, which carries the lines itself
    let dir = tempfile::tempdir().unwrap();
    let managed =
        format!("https://sub.example/managed/{MARKER}-managed?token={MARKER}-managedtoken");
    let copy = format!("#!MANAGED-CONFIG {managed} interval=3600\n{lines}FINAL,DIRECT\n");
    let main = dir.path().join("main.conf");
    let local = format!("#!MANAGED-CONFIG {managed} interval=3600\n[Rule]\nFINAL,DIRECT\n");
    std::fs::write(&main, &local).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&main)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH)
        .unwrap();
    let mut opts = LoadOptions::for_tests();
    opts.remote = Some(Arc::new(Downloaded(HashMap::from([(
        key(&managed),
        copy_at(copy, 1, SystemTime::UNIX_EPOCH + Duration::from_secs(1_000)),
    )]))));
    loads.push((
        "a managed copy",
        load_file(&main, &opts).unwrap(),
        local,
        // the four includes of the text and the nine more lines, each once:
        // the lines stand in the copy, which is taken in under no section
        13,
    ));

    for (what, loaded, profile, refusals) in loads {
        // the text was read, and refused what it asked for, or the checks
        // below would find nothing to find
        let refused = loaded.diagnostics.iter().filter(|d| d.code == "W0033");
        assert_eq!(
            refused.count(),
            refusals,
            "{what}: {:#?}",
            loaded.diagnostics
        );
        assert!(loaded.config.spec("Ext").is_none(), "{what}");
        let mut found = leaks(&format!("{loaded:#?}"));
        for d in loaded.diagnostics.iter() {
            found.extend(leaks(&d.to_string()));
            found.extend(leaks(&format!("{d:?}")));
        }
        found.extend(leaks(&format!("{:?}", loaded.config.summary())));
        found.extend(leaks(&redact_profile(&profile)));
        assert_eq!(found, Vec::<String>::new(), "{what}");
    }
}
