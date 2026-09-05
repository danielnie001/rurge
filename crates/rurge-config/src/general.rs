//! Typed view of the `[General]` section.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::hostlist::HostList;
use crate::span::Span;
use crate::text::Section;
use crate::value::{parse_bool, split_definition, split_list};
use ipnet::IpNet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogLevel {
    Verbose,
    Info,
    #[default]
    Notify,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsServer {
    System,
    Udp(SocketAddr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptedDnsScheme {
    Https,
    H3,
    Quic,
    Tls,
    Tcp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedDns {
    pub scheme: EncryptedDnsScheme,
    pub url: String,
}

impl EncryptedDns {
    /// Recognise `https://`, `h3://`, `quic://`, `tls://`, `tcp://`.
    pub fn parse(s: &str) -> Option<EncryptedDns> {
        let lower = s.to_ascii_lowercase();
        let scheme = if lower.starts_with("https://") {
            EncryptedDnsScheme::Https
        } else if lower.starts_with("h3://") {
            EncryptedDnsScheme::H3
        } else if lower.starts_with("quic://") {
            EncryptedDnsScheme::Quic
        } else if lower.starts_with("tls://") {
            EncryptedDnsScheme::Tls
        } else if lower.starts_with("tcp://") {
            EncryptedDnsScheme::Tcp
        } else {
            return None;
        };
        Some(EncryptedDns {
            scheme,
            url: s.to_string(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HijackTarget {
    /// `None` means `*` (any destination address).
    pub addr: Option<Ipv4Addr>,
    pub port: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ipv6Vif {
    #[default]
    Disabled,
    Auto,
    Always,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerAccess {
    pub key: String,
    pub addr: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpTest {
    pub hostname: String,
    pub server: Ipv4Addr,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UdpFallback {
    #[default]
    Reject,
    Direct,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlockQuicGlobal {
    #[default]
    PerPolicy,
    AllProxy,
    All,
    AlwaysAllow,
}

#[derive(Clone, PartialEq, Eq)]
pub struct Listener {
    pub password: Option<String>,
    pub addr: SocketAddr,
}

impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listener")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("addr", &self.addr)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownKey {
    pub key: String,
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct General {
    pub loglevel: LogLevel,
    pub debug_cpu_usage: bool,
    pub debug_memory_usage: bool,
    pub dns_server: Vec<DnsServer>,
    pub encrypted_dns_server: Vec<EncryptedDns>,
    pub encrypted_dns_follow_outbound_mode: bool,
    pub encrypted_dns_skip_cert_verification: bool,
    pub allow_dns_svcb: bool,
    pub use_local_host_item_for_proxy: bool,
    pub hijack_dns: Vec<HijackTarget>,
    pub always_real_ip: HostList,
    pub geoip_maxmind_url: Option<String>,
    pub disable_geoip_db_auto_update: bool,
    pub ipv6: bool,
    pub ipv6_vif: Ipv6Vif,
    pub tun_excluded_routes: Vec<IpNet>,
    pub tun_included_routes: Vec<IpNet>,
    pub icmp_forwarding: bool,
    pub skip_proxy: HostList,
    pub exclude_simple_hostnames: bool,
    pub proxy_restricted_to_lan: bool,
    pub gateway_restricted_to_lan: bool,
    pub external_controller_access: Option<ControllerAccess>,
    pub http_api: Option<ControllerAccess>,
    pub http_api_tls: bool,
    pub http_api_web_dashboard: bool,
    pub internet_test_url: String,
    pub proxy_test_url: String,
    pub test_timeout: Duration,
    pub proxy_test_udp: Option<UdpTest>,
    pub force_http_engine_hosts: HostList,
    pub always_raw_tcp_hosts: HostList,
    pub always_raw_tcp_keywords: Vec<String>,
    pub udp_policy_not_supported_behaviour: UdpFallback,
    pub udp_priority: bool,
    pub block_quic: BlockQuicGlobal,
    pub show_error_page: bool,
    pub show_error_page_for_reject: bool,
    // iOS only (parsed, ignored at runtime on desktop)
    pub compatibility_mode: u8,
    pub auto_suspend: bool,
    pub allow_wifi_access: bool,
    pub allow_hotspot_access: bool,
    pub wifi_access_http_port: u16,
    pub wifi_access_socks5_port: u16,
    pub wifi_access_http_auth: Option<(String, String)>,
    pub wifi_assist: bool,
    pub all_hybrid: bool,
    pub hide_vpn_icon: bool,
    pub include_all_networks: bool,
    pub include_local_networks: bool,
    pub include_apns: bool,
    pub include_cellular_services: bool,
    // macOS only in Surge; supported everywhere in rurge
    pub http_listen: Vec<Listener>,
    pub socks5_listen: Vec<Listener>,
    pub set_system_socks_proxy: bool,
    pub read_etc_hosts: bool,
    pub subnet_exp_wifi_always_match: bool,
    pub unknown: Vec<UnknownKey>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            loglevel: LogLevel::Notify,
            debug_cpu_usage: false,
            debug_memory_usage: false,
            dns_server: Vec::new(),
            encrypted_dns_server: Vec::new(),
            encrypted_dns_follow_outbound_mode: false,
            encrypted_dns_skip_cert_verification: false,
            allow_dns_svcb: false,
            use_local_host_item_for_proxy: false,
            hijack_dns: Vec::new(),
            always_real_ip: HostList::empty(),
            geoip_maxmind_url: None,
            disable_geoip_db_auto_update: false,
            ipv6: false,
            ipv6_vif: Ipv6Vif::Disabled,
            tun_excluded_routes: Vec::new(),
            tun_included_routes: Vec::new(),
            icmp_forwarding: true,
            skip_proxy: HostList::empty(),
            exclude_simple_hostnames: false,
            proxy_restricted_to_lan: true,
            gateway_restricted_to_lan: true,
            external_controller_access: None,
            http_api: None,
            http_api_tls: false,
            http_api_web_dashboard: false,
            internet_test_url: "http://bing.com/".into(),
            proxy_test_url: "http://bing.com/".into(),
            test_timeout: Duration::from_secs(5),
            proxy_test_udp: None,
            force_http_engine_hosts: HostList {
                default_port: Some(80),
                ..HostList::empty()
            },
            always_raw_tcp_hosts: HostList::empty(),
            always_raw_tcp_keywords: Vec::new(),
            udp_policy_not_supported_behaviour: UdpFallback::Reject,
            udp_priority: true,
            block_quic: BlockQuicGlobal::PerPolicy,
            show_error_page: true,
            show_error_page_for_reject: false,
            compatibility_mode: 0,
            auto_suspend: true,
            allow_wifi_access: false,
            allow_hotspot_access: false,
            wifi_access_http_port: 6152,
            wifi_access_socks5_port: 6153,
            wifi_access_http_auth: None,
            wifi_assist: false,
            all_hybrid: false,
            hide_vpn_icon: false,
            include_all_networks: false,
            include_local_networks: false,
            include_apns: false,
            include_cellular_services: false,
            http_listen: Vec::new(),
            socks5_listen: Vec::new(),
            set_system_socks_proxy: true,
            read_etc_hosts: true,
            subnet_exp_wifi_always_match: true,
            unknown: Vec::new(),
        }
    }
}

const IOS_ONLY_KEYS: &[&str] = &[
    "compatibility-mode",
    "auto-suspend",
    "allow-wifi-access",
    "allow-hotspot-access",
    "wifi-access-http-port",
    "wifi-access-socks5-port",
    "wifi-access-http-auth",
    "wifi-assist",
    "all-hybrid",
    "hide-vpn-icon",
    "include-all-networks",
    "include-local-networks",
    "include-apns",
    "include-cellular-services",
];

const VANISHED_KEYS: &[&str] = &[
    "vif-mode",
    "tls-provider",
    "network-framework",
    "bypass-system",
    "bypass-tun",
    "enhanced-mode-by-rule",
    "allow-udp-proxy",
];

fn invalid(diags: &mut Diagnostics, span: &Span, key: &str, value: &str) {
    diags.push(
        Diagnostic::warning(
            codes::W_INVALID_VALUE,
            format!("invalid value `{value}` for `{key}`, using default"),
        )
        .at(span.clone()),
    );
}

fn bool_or(diags: &mut Diagnostics, span: &Span, key: &str, value: &str, current: bool) -> bool {
    match parse_bool(value) {
        Some(b) => b,
        None => {
            invalid(diags, span, key, value);
            current
        }
    }
}

fn u16_or(diags: &mut Diagnostics, span: &Span, key: &str, value: &str, current: u16) -> u16 {
    match value.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            invalid(diags, span, key, value);
            current
        }
    }
}

fn u8_or(diags: &mut Diagnostics, span: &Span, key: &str, value: &str, current: u8) -> u8 {
    match value.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            invalid(diags, span, key, value);
            current
        }
    }
}

/// Parse `[password@]address[:port]`; the address must be an IP literal.
fn parse_listener(s: &str, default_port: u16) -> Result<Listener, &'static str> {
    let (password, rest) = match s.rsplit_once('@') {
        Some((pw, rest)) => (Some(pw.to_string()), rest),
        None => (None, s),
    };
    let addr = parse_socket_addr(rest, default_port)
        .ok_or("address must be an IP literal like 0.0.0.0:6152")?;
    Ok(Listener { password, addr })
}

/// `ip`, `ip:port`, `[v6]`, `[v6]:port`.
fn parse_socket_addr(s: &str, default_port: u16) -> Option<SocketAddr> {
    let s = s.trim();
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(sa);
    }
    let bare = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(s);
    let ip: IpAddr = bare.parse().ok()?;
    Some(SocketAddr::new(ip, default_port))
}

fn parse_controller(s: &str) -> Option<ControllerAccess> {
    let (key, rest) = s.rsplit_once('@')?;
    if key.is_empty() {
        return None;
    }
    let addr = rest.parse::<SocketAddr>().ok()?;
    Some(ControllerAccess {
        key: key.to_string(),
        addr,
    })
}

fn parse_listeners(
    diags: &mut Diagnostics,
    span: &Span,
    key: &str,
    value: &str,
    default_port: u16,
    allow_password: bool,
) -> Vec<Listener> {
    let mut out = Vec::new();
    for item in split_list(value) {
        match parse_listener(&item, default_port) {
            Ok(mut l) => {
                if l.password.is_some() && !allow_password {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_INVALID_VALUE,
                            format!("`{key}` does not support a password; ignoring it in `{item}`"),
                        )
                        .at(span.clone()),
                    );
                    l.password = None;
                }
                out.push(l);
            }
            Err(msg) => diags.push(
                Diagnostic::error(
                    codes::E_LISTENER_NOT_IP,
                    format!("`{key}`: `{item}`: {msg}"),
                )
                .at(span.clone()),
            ),
        }
    }
    out
}

fn parse_nets(diags: &mut Diagnostics, span: &Span, key: &str, value: &str) -> Vec<IpNet> {
    let mut out = Vec::new();
    for item in split_list(value) {
        match item.parse::<IpNet>() {
            Ok(n) => out.push(n),
            Err(_) => invalid(diags, span, key, &item),
        }
    }
    out
}

fn parse_host_list(
    diags: &mut Diagnostics,
    span: &Span,
    key: &str,
    value: &str,
    default_port: Option<u16>,
) -> HostList {
    let list = HostList::parse(value, default_port);
    for bad in &list.invalid {
        diags.push(
            Diagnostic::warning(
                codes::W_INVALID_HOST_LIST_ENTRY,
                format!("`{key}`: invalid entry `{bad}` ignored"),
            )
            .at(span.clone()),
        );
    }
    list
}

fn migrated(diags: &mut Diagnostics, span: &Span, old: &str, new: &str) {
    diags.push(
        Diagnostic::info(
            codes::I_LEGACY_MIGRATED,
            format!("legacy key `{old}` migrated to `{new}`"),
        )
        .at(span.clone()),
    );
}

pub fn parse_general(section: Option<&Section>, diags: &mut Diagnostics) -> General {
    let mut g = General::default();
    let Some(section) = section else { return g };
    let mut legacy_http: (Option<String>, Option<u16>, Option<Span>) = (None, None, None);
    let mut legacy_socks: (Option<String>, Option<u16>, Option<Span>) = (None, None, None);

    for e in section.active_entries() {
        let span = &e.span;
        let Some((key, value)) = split_definition(&e.raw) else {
            diags.push(
                Diagnostic::error(
                    codes::E_INVALID_DEFINITION,
                    format!("expected `key = value`, found `{}`", e.raw),
                )
                .at(span.clone()),
            );
            continue;
        };
        let key = key.to_ascii_lowercase();
        let key = key.as_str();
        macro_rules! set_bool {
            ($field:ident) => {
                g.$field = bool_or(diags, span, key, value, g.$field)
            };
        }
        match key {
            "loglevel" => {
                g.loglevel = match value.to_ascii_lowercase().as_str() {
                    "verbose" => LogLevel::Verbose,
                    "info" => LogLevel::Info,
                    "notify" => LogLevel::Notify,
                    "warning" => LogLevel::Warning,
                    _ => {
                        invalid(diags, span, key, value);
                        g.loglevel
                    }
                }
            }
            "debug-cpu-usage" => set_bool!(debug_cpu_usage),
            "debug-memory-usage" => set_bool!(debug_memory_usage),
            "dns-server" => {
                for item in split_list(value) {
                    if item.eq_ignore_ascii_case("system") {
                        g.dns_server.push(DnsServer::System);
                    } else if let Some(enc) = EncryptedDns::parse(&item) {
                        g.encrypted_dns_server.push(enc);
                    } else if let Some(sa) = parse_socket_addr(&item, 53) {
                        g.dns_server.push(DnsServer::Udp(sa));
                    } else {
                        invalid(diags, span, key, &item);
                    }
                }
            }
            "encrypted-dns-server" | "doh-server" => {
                if key == "doh-server" {
                    migrated(diags, span, key, "encrypted-dns-server");
                }
                for item in split_list(value) {
                    match EncryptedDns::parse(&item) {
                        Some(enc) => g.encrypted_dns_server.push(enc),
                        None => invalid(diags, span, key, &item),
                    }
                }
            }
            "encrypted-dns-follow-outbound-mode" | "doh-follow-outbound-mode" => {
                if key == "doh-follow-outbound-mode" {
                    migrated(diags, span, key, "encrypted-dns-follow-outbound-mode");
                }
                set_bool!(encrypted_dns_follow_outbound_mode)
            }
            "encrypted-dns-skip-cert-verification" | "doh-skip-cert-verification" => {
                if key == "doh-skip-cert-verification" {
                    migrated(diags, span, key, "encrypted-dns-skip-cert-verification");
                }
                set_bool!(encrypted_dns_skip_cert_verification)
            }
            "allow-dns-svcb" => set_bool!(allow_dns_svcb),
            "use-local-host-item-for-proxy" => set_bool!(use_local_host_item_for_proxy),
            "hijack-dns" => {
                for item in split_list(value) {
                    let (host, port) = match item.rsplit_once(':') {
                        Some((h, p)) => (h, p.parse::<u16>().ok()),
                        None => (item.as_str(), Some(53)),
                    };
                    let addr = if host == "*" {
                        Ok(None)
                    } else {
                        host.parse::<Ipv4Addr>().map(Some)
                    };
                    match (addr, port) {
                        (Ok(addr), Some(port)) => g.hijack_dns.push(HijackTarget { addr, port }),
                        _ => invalid(diags, span, key, &item),
                    }
                }
            }
            "always-real-ip" => g.always_real_ip = parse_host_list(diags, span, key, value, None),
            "geoip-maxmind-url" => g.geoip_maxmind_url = Some(value.to_string()),
            "disable-geoip-db-auto-update" => set_bool!(disable_geoip_db_auto_update),
            "ipv6" => set_bool!(ipv6),
            "ipv6-vif" => {
                g.ipv6_vif = match value.to_ascii_lowercase().as_str() {
                    "disabled" => Ipv6Vif::Disabled,
                    "auto" => Ipv6Vif::Auto,
                    "always" => Ipv6Vif::Always,
                    "off" => {
                        migrated(diags, span, "ipv6-vif = off", "ipv6-vif = disabled");
                        Ipv6Vif::Disabled
                    }
                    _ => {
                        invalid(diags, span, key, value);
                        g.ipv6_vif
                    }
                }
            }
            "tun-excluded-routes" => g.tun_excluded_routes = parse_nets(diags, span, key, value),
            "tun-included-routes" => g.tun_included_routes = parse_nets(diags, span, key, value),
            "icmp-forwarding" => set_bool!(icmp_forwarding),
            "skip-proxy" => g.skip_proxy = parse_host_list(diags, span, key, value, None),
            "exclude-simple-hostnames" => set_bool!(exclude_simple_hostnames),
            "proxy-restricted-to-lan" => set_bool!(proxy_restricted_to_lan),
            "gateway-restricted-to-lan" => set_bool!(gateway_restricted_to_lan),
            "external-controller-access" => {
                g.external_controller_access = parse_controller(value);
                if g.external_controller_access.is_none() {
                    invalid(diags, span, key, value);
                }
            }
            "http-api" => {
                g.http_api = parse_controller(value);
                if g.http_api.is_none() {
                    invalid(diags, span, key, value);
                }
            }
            "http-api-tls" => set_bool!(http_api_tls),
            "http-api-web-dashboard" => set_bool!(http_api_web_dashboard),
            "internet-test-url" => g.internet_test_url = value.to_string(),
            "proxy-test-url" => g.proxy_test_url = value.to_string(),
            "test-timeout" => match value.trim().parse::<u64>() {
                Ok(s) => g.test_timeout = Duration::from_secs(s),
                Err(_) => invalid(diags, span, key, value),
            },
            "proxy-test-udp" => match value.split_once('@').and_then(|(h, ip)| {
                ip.trim()
                    .parse::<Ipv4Addr>()
                    .ok()
                    .map(|ip| (h.trim().to_string(), ip))
            }) {
                Some((hostname, server)) => g.proxy_test_udp = Some(UdpTest { hostname, server }),
                None => invalid(diags, span, key, value),
            },
            "force-http-engine-hosts" => {
                g.force_http_engine_hosts = parse_host_list(diags, span, key, value, Some(80))
            }
            "always-raw-tcp-hosts" => {
                g.always_raw_tcp_hosts = parse_host_list(diags, span, key, value, None)
            }
            "always-raw-tcp-keywords" => g.always_raw_tcp_keywords = split_list(value),
            "udp-policy-not-supported-behaviour" => {
                g.udp_policy_not_supported_behaviour = match value.to_ascii_uppercase().as_str() {
                    "REJECT" => UdpFallback::Reject,
                    "DIRECT" => UdpFallback::Direct,
                    _ => {
                        invalid(diags, span, key, value);
                        g.udp_policy_not_supported_behaviour
                    }
                }
            }
            "udp-priority" => set_bool!(udp_priority),
            "block-quic" => {
                g.block_quic = match value.to_ascii_lowercase().as_str() {
                    "per-policy" => BlockQuicGlobal::PerPolicy,
                    "all-proxy" => BlockQuicGlobal::AllProxy,
                    "all" => BlockQuicGlobal::All,
                    "always-allow" => BlockQuicGlobal::AlwaysAllow,
                    _ => {
                        invalid(diags, span, key, value);
                        g.block_quic
                    }
                }
            }
            "show-error-page" => set_bool!(show_error_page),
            "show-error-page-for-reject" => set_bool!(show_error_page_for_reject),
            "compatibility-mode" => {
                g.compatibility_mode = u8_or(diags, span, key, value, g.compatibility_mode)
            }
            "auto-suspend" => set_bool!(auto_suspend),
            "allow-wifi-access" => set_bool!(allow_wifi_access),
            "allow-hotspot-access" => set_bool!(allow_hotspot_access),
            "wifi-access-http-port" => {
                g.wifi_access_http_port = u16_or(diags, span, key, value, g.wifi_access_http_port)
            }
            "wifi-access-socks5-port" => {
                g.wifi_access_socks5_port =
                    u16_or(diags, span, key, value, g.wifi_access_socks5_port)
            }
            "wifi-access-http-auth" => match value.split_once(':') {
                Some((u, p)) => g.wifi_access_http_auth = Some((u.to_string(), p.to_string())),
                None => invalid(diags, span, key, value),
            },
            "wifi-assist" => set_bool!(wifi_assist),
            "all-hybrid" => set_bool!(all_hybrid),
            "hide-vpn-icon" => set_bool!(hide_vpn_icon),
            "include-all-networks" => set_bool!(include_all_networks),
            "include-local-networks" => set_bool!(include_local_networks),
            "include-apns" => set_bool!(include_apns),
            "include-cellular-services" => set_bool!(include_cellular_services),
            "http-listen" => g.http_listen = parse_listeners(diags, span, key, value, 6152, true),
            "socks5-listen" => {
                g.socks5_listen = parse_listeners(diags, span, key, value, 6153, false)
            }
            "set-system-socks-proxy" => set_bool!(set_system_socks_proxy),
            "read-etc-hosts" => set_bool!(read_etc_hosts),
            "subnet-exp-wifi-always-match" => set_bool!(subnet_exp_wifi_always_match),
            "use-default-policy-if-wifi-not-primary" => {
                migrated(diags, span, key, "subnet-exp-wifi-always-match (inverted)");
                let v = bool_or(diags, span, key, value, !g.subnet_exp_wifi_always_match);
                g.subnet_exp_wifi_always_match = !v;
            }
            "interface" => {
                legacy_http.0 = Some(value.to_string());
                legacy_http.2 = Some(span.clone());
            }
            "port" => {
                legacy_http.1 = Some(u16_or(diags, span, key, value, 6152));
                legacy_http.2 = Some(span.clone());
            }
            "socks-interface" => {
                legacy_socks.0 = Some(value.to_string());
                legacy_socks.2 = Some(span.clone());
            }
            "socks-port" => {
                legacy_socks.1 = Some(u16_or(diags, span, key, value, 6153));
                legacy_socks.2 = Some(span.clone());
            }
            k if VANISHED_KEYS.contains(&k) => {
                diags.push(
                    Diagnostic::warning(
                        codes::W_VANISHED_KEY,
                        format!("`{k}` is no longer supported by Surge and is ignored"),
                    )
                    .at(span.clone()),
                );
            }
            _ => {
                diags.push(
                    Diagnostic::warning(
                        codes::W_UNKNOWN_KEY,
                        format!("unknown [General] key `{key}` ignored"),
                    )
                    .at(span.clone()),
                );
                g.unknown.push(UnknownKey {
                    key: key.to_string(),
                    value: value.to_string(),
                    span: span.clone(),
                });
            }
        }
        if IOS_ONLY_KEYS.contains(&key) {
            diags.push(
                Diagnostic::warning(
                    codes::W_PLATFORM_IGNORED,
                    format!("`{key}` is iOS-only and has no effect on desktop platforms"),
                )
                .at(span.clone()),
            );
        }
    }

    if let (Some(span), true) = (&legacy_http.2, g.http_listen.is_empty()) {
        let addr = format!(
            "{}:{}",
            legacy_http.0.as_deref().unwrap_or("127.0.0.1"),
            legacy_http.1.unwrap_or(6152)
        );
        migrated(diags, span, "interface/port", "http-listen");
        g.http_listen = parse_listeners(diags, span, "http-listen", &addr, 6152, true);
    }
    if let (Some(span), true) = (&legacy_socks.2, g.socks5_listen.is_empty()) {
        let addr = format!(
            "{}:{}",
            legacy_socks.0.as_deref().unwrap_or("127.0.0.1"),
            legacy_socks.1.unwrap_or(6153)
        );
        migrated(diags, span, "socks-interface/socks-port", "socks5-listen");
        g.socks5_listen = parse_listeners(diags, span, "socks5-listen", &addr, 6153, false);
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    fn parse(text: &str) -> (General, Diagnostics) {
        let (p, mut d) = parse_str(text, Arc::from(Path::new("g.conf")), Origin::Main);
        let g = parse_general(p.section("General"), &mut d);
        (g, d)
    }

    fn codes_of(d: &Diagnostics) -> Vec<&'static str> {
        d.iter().map(|x| x.code).collect()
    }

    #[test]
    fn typical_section() {
        let (g, d) = parse(
            "[General]\nloglevel = warning\ndns-server = system, 8.8.8.8, 192.0.2.53:5353, https://doh.example/dns-query\nencrypted-dns-server = tls://1.1.1.1, tcp://dns.example.com\nhijack-dns = 8.8.8.8:53, *\nskip-proxy = 127.0.0.1, 192.168.0.0/16, localhost, *.local\nexclude-simple-hostnames = true\nhttp-api = key@0.0.0.0:6171\nhttp-listen = 0.0.0.0:6152, pw@127.0.0.1:7000, [::1]:6152\nsocks5-listen = 127.0.0.1:6153\ntest-timeout = 8\nproxy-test-udp = apple.com@8.8.8.8\nblock-quic = all-proxy\nudp-policy-not-supported-behaviour = DIRECT\nipv6-vif = auto\ntun-excluded-routes = 192.168.0.0/16, 10.0.0.0/8\nforce-http-engine-hosts = *.example.com, api.test:8080\nalways-raw-tcp-keywords = kw1, kw2\n",
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert_eq!(g.loglevel, LogLevel::Warning);
        assert_eq!(
            g.dns_server,
            vec![
                DnsServer::System,
                DnsServer::Udp("8.8.8.8:53".parse().unwrap()),
                DnsServer::Udp("192.0.2.53:5353".parse().unwrap())
            ]
        );
        assert_eq!(g.encrypted_dns_server.len(), 3);
        assert_eq!(g.encrypted_dns_server[0].scheme, EncryptedDnsScheme::Https);
        assert_eq!(g.encrypted_dns_server[1].scheme, EncryptedDnsScheme::Tls);
        assert_eq!(g.encrypted_dns_server[2].scheme, EncryptedDnsScheme::Tcp);
        assert_eq!(
            g.hijack_dns,
            vec![
                HijackTarget {
                    addr: Some("8.8.8.8".parse().unwrap()),
                    port: 53
                },
                HijackTarget {
                    addr: None,
                    port: 53
                }
            ]
        );
        assert_eq!(g.skip_proxy.entries.len(), 4);
        assert!(g.exclude_simple_hostnames);
        assert_eq!(g.http_api.as_ref().unwrap().key, "key");
        assert_eq!(
            g.http_api.as_ref().unwrap().addr,
            "0.0.0.0:6171".parse().unwrap()
        );
        assert_eq!(g.http_listen.len(), 3);
        assert_eq!(g.http_listen[1].password.as_deref(), Some("pw"));
        assert_eq!(g.http_listen[2].addr, "[::1]:6152".parse().unwrap());
        assert_eq!(g.test_timeout, Duration::from_secs(8));
        assert_eq!(g.proxy_test_udp.as_ref().unwrap().hostname, "apple.com");
        assert_eq!(g.block_quic, BlockQuicGlobal::AllProxy);
        assert_eq!(g.udp_policy_not_supported_behaviour, UdpFallback::Direct);
        assert_eq!(g.ipv6_vif, Ipv6Vif::Auto);
        assert_eq!(g.tun_excluded_routes.len(), 2);
        assert_eq!(g.force_http_engine_hosts.default_port, Some(80));
        assert_eq!(g.always_raw_tcp_keywords, ["kw1", "kw2"]);
    }

    #[test]
    fn defaults_when_section_missing() {
        let (g, d) = parse("[Rule]\nFINAL,DIRECT\n");
        assert!(d.is_empty());
        assert_eq!(g.loglevel, LogLevel::Notify);
        assert_eq!(g.test_timeout, Duration::from_secs(5));
        assert_eq!(g.internet_test_url, "http://bing.com/");
        assert!(g.icmp_forwarding);
        assert!(g.proxy_restricted_to_lan);
        assert_eq!(g.udp_policy_not_supported_behaviour, UdpFallback::Reject);
        assert!(g.http_listen.is_empty());
    }

    #[test]
    fn legacy_keys_migrate() {
        let (g, d) = parse(
            "[General]\ndoh-server = https://a/dns-query\ndoh-follow-outbound-mode = true\ndoh-skip-cert-verification = true\ninterface = 0.0.0.0\nport = 6152\nsocks-interface = 127.0.0.1\nsocks-port = 6153\nuse-default-policy-if-wifi-not-primary = true\nipv6-vif = off\nvif-mode = v2\n",
        );
        assert!(!d.has_errors());
        assert_eq!(g.encrypted_dns_server[0].url, "https://a/dns-query");
        assert!(g.encrypted_dns_follow_outbound_mode);
        assert!(g.encrypted_dns_skip_cert_verification);
        assert_eq!(g.http_listen[0].addr, "0.0.0.0:6152".parse().unwrap());
        assert_eq!(g.socks5_listen[0].addr, "127.0.0.1:6153".parse().unwrap());
        assert!(!g.subnet_exp_wifi_always_match);
        assert_eq!(g.ipv6_vif, Ipv6Vif::Disabled);
        let c = codes_of(&d);
        assert_eq!(
            c.iter().filter(|x| **x == codes::I_LEGACY_MIGRATED).count(),
            7
        );
        assert!(c.contains(&codes::W_VANISHED_KEY));
    }

    #[test]
    fn invalid_values_unknown_keys_and_platform_keys() {
        let (g, d) = parse(
            "[General]\nipv6 = maybe\nloglevel = loud\nhttp-listen = example.com:6152\nsocks5-listen = pw@127.0.0.1:6153\nhttp-api = 0.0.0.0:6171\nmystery-key = 1\ncompatibility-mode = 3\nallow-wifi-access = true\nwifi-access-http-auth = user:pass\n",
        );
        assert!(!g.ipv6);
        assert_eq!(g.loglevel, LogLevel::Notify);
        assert!(g.http_listen.is_empty());
        assert_eq!(g.socks5_listen.len(), 1);
        assert_eq!(g.socks5_listen[0].password, None);
        assert!(g.http_api.is_none());
        assert_eq!(g.unknown.len(), 1);
        assert_eq!(g.unknown[0].key, "mystery-key");
        assert_eq!(g.compatibility_mode, 3);
        assert!(g.allow_wifi_access);
        assert_eq!(
            g.wifi_access_http_auth,
            Some(("user".into(), "pass".into()))
        );
        let c = codes_of(&d);
        assert!(c.contains(&codes::W_INVALID_VALUE));
        assert!(c.contains(&codes::E_LISTENER_NOT_IP));
        assert!(c.contains(&codes::W_UNKNOWN_KEY));
        assert_eq!(
            c.iter()
                .filter(|x| **x == codes::W_PLATFORM_IGNORED)
                .count(),
            3
        );
    }

    #[test]
    fn compatibility_mode_out_of_range_warns_and_keeps_default() {
        let (g, d) = parse("[General]\ncompatibility-mode = 300\n");
        assert_eq!(g.compatibility_mode, 0);
        let c = codes_of(&d);
        assert!(c.contains(&codes::W_INVALID_VALUE));
        assert!(c.contains(&codes::W_PLATFORM_IGNORED));
    }

    #[test]
    fn listener_debug_redacts_the_password() {
        let l = Listener {
            password: Some("s3cret".to_string()),
            addr: "127.0.0.1:6152".parse().unwrap(),
        };
        let shown = format!("{l:?}");
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(
            shown.contains("<redacted>") && shown.contains("6152"),
            "{shown}"
        );
    }
}
