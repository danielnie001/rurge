//! What this build of rurge actually implements: the built-in alias
//! policies, the HTTP / SOCKS5 proxy family (phase 2 M1), `trojan`
//! (phase 2 M2a), `vmess` with the AEAD handshake and `anytls` (phase 2
//! M2b), `ssh` (phase 2 M4a), `wireguard` (phase 2 M4b), `external`
//! (phase 2 M4c), `ss` with the AEAD, `none` and 2022 methods (phase 2
//! M6a) and the stream ciphers of the manual (phase 2 M8b), `snell`
//! versions 4 and 5 (phase 2 M6b) and 1 to 3 (phase 2 M8b), `h2-connect`
//! and `trust-tunnel` over HTTP/2 (phase 2 M6c), `hysteria2` (phase 2 M7a),
//! `tuic` and `tuic-v5` (phase 2 M7b), `masque` and `trust-tunnel` over
//! HTTP/3 (phase 2 M7c), `select` groups, `url-test` / `fallback` /
//! `load-balance` groups (phase 2 M3b), `smart` groups (phase 2 M3c), and
//! `subnet` groups and `SUBNET` rules, which look at the network the engine
//! is told it is on (phase 3 M1), and `PROCESS-NAME` rules, which are matched
//! against the process a session comes from (phase 3 M2).

use rurge_config::config::Capabilities;
use rurge_config::policy::{GroupKind, PolicyKind};
use std::collections::HashSet;

/// Reported as `CORE_VERSION` to requirement expressions (see FR-CFG-08).
/// The manual ties 22 to the `smart` group, which rurge has. It ties 20 to
/// Body Rewrite and inline Map Local, which rurge parses without effect
/// (`W0016`): a profile that asks for 20 gets the same here either way.
pub const CORE_VERSION: u64 = 22;

const INACTIVE_RULE_TYPES: [&str; 5] = [
    "SCRIPT",
    "DEVICE-NAME",
    "MAC-ADDRESS",
    "CELLULAR-RADIO",
    "CELLULAR-CARRIER",
];

pub fn current() -> Capabilities {
    Capabilities {
        policy_kinds: HashSet::from([
            PolicyKind::Direct,
            PolicyKind::Reject,
            PolicyKind::RejectDrop,
            PolicyKind::RejectNoDrop,
            PolicyKind::RejectTinyGif,
            PolicyKind::Http,
            PolicyKind::Https,
            PolicyKind::Socks5,
            PolicyKind::Socks5Tls,
            PolicyKind::Trojan,
            PolicyKind::Vmess,
            PolicyKind::AnyTls,
            PolicyKind::Ssh,
            PolicyKind::WireGuard,
            PolicyKind::External,
            PolicyKind::Shadowsocks,
            PolicyKind::Snell,
            PolicyKind::H2Connect,
            PolicyKind::TrustTunnel,
            PolicyKind::Hysteria2,
            PolicyKind::Tuic,
            PolicyKind::TuicV5,
            PolicyKind::Masque,
        ]),
        group_kinds: HashSet::from([
            GroupKind::Select,
            GroupKind::UrlTest,
            GroupKind::Fallback,
            GroupKind::LoadBalance,
            GroupKind::Smart,
            GroupKind::Subnet,
        ]),
        rule_types: Capabilities::ALL_RULE_TYPES
            .iter()
            .copied()
            .filter(|t| !INACTIVE_RULE_TYPES.contains(t))
            .collect(),
    }
}
