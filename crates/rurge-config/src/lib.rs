//! Surge-compatible profile parser.
//!
//! Two layers: the *text layer* (`text::Profile`) keeps every section and line
//! with its origin; the *semantic layer* (`config::Config`) is the typed view
//! consumed by the engine. Parsing never panics; problems are reported as
//! `diagnostic::Diagnostic` values.

pub mod config;
pub mod deferred;
pub mod diagnostic;
pub mod general;
pub mod glob;
pub mod host;
pub mod hostlist;
pub mod hostname;
pub mod ip_rewrite;
pub mod keystore;
pub mod managed;
pub mod network;
pub mod policy;
pub mod reach;
pub mod redact;
pub mod remote;
pub mod requirement;
pub mod rule;
pub mod session;
pub mod span;
pub mod spec;
pub mod ssid_setting;
pub mod text;
pub mod types;
pub mod value;
pub mod wireguard;

pub use config::{
    Capabilities, Config, ConfigSummary, InlineRuleset, LoadError, LoadOptions, Loaded, Platform,
    PolicyTarget, SourceInfo, load,
};
pub use deferred::DeferredSections;
pub use diagnostic::{Diagnostic, Diagnostics, ParseError, Severity, codes};
pub use general::{
    BlockQuicGlobal, ControllerAccess, DnsServer, EncryptedDns, EncryptedDnsScheme, General,
    HijackTarget, Ipv6Vif, Listener, LogLevel, UdpFallback, UdpTest, UnknownKey,
};
pub use glob::{Glob, GlobOptions};
pub use host::{DnsUpstream, HostEntry, HostKey, HostValue, SystemMode};
pub use hostlist::{HostList, HostListEntry, HostPattern, PortSpec};
pub use ip_rewrite::{IpRewrite, IpRewriteAction};
pub use keystore::{KeystoreItem, KeystoreType};
pub use managed::ManagedConfig;
pub use network::{NetKind, NetworkCell, NetworkFacts, Primary, WifiLink};
pub use policy::{Builtin, GroupKind, NetType, PolicyGroup, PolicyKind, ProxyPolicy, SubnetExpr};
pub use reach::Reach;
pub use remote::{Remote, RemoteRef, RemoteRole, RemoteSource};
pub use requirement::Environment;
pub use rule::{
    HostnameType, InternalSet, ParseCtx, Pattern, PolicyRef, PortExpr, ProcessPattern,
    ProtocolKind, ResourceRef, Rule, RuleKind, RuleParams, SubRule,
};
pub use session::{DeviceInfo, ListenerKind, PreMatchLayer, ProcessInfo, SessionInfo, Transport};
pub use span::Span;
pub use spec::{PolicySpec, ProtoSpec};
pub use ssid_setting::{SsidSetting, TfoBehaviour};
pub use text::include::IncludeOptions;
pub use text::{Entry, Origin, Profile, Section, SectionKind};
pub use types::HostName;
pub use value::ParamMap;
pub use wireguard::{PeerEndpoint, TunnelDns, WireGuardPeer, WireGuardSection};

pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
