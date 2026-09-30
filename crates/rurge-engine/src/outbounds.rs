//! Real outbounds for the policy registry, and the dry build that turns a
//! policy which cannot be built into a load error (M1 design 6.1, 6.4).

use crate::shared::ExternalPrograms;
use rurge_config::config::{LoadError, LoadOptions, Loaded, load};
use rurge_config::diagnostic::codes;
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec, ProtoSpec};
use rurge_config::{Config, Diagnostic, Diagnostics, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{Connector, DirectConnector, Resolve, ResolveVia, SystemResolve, Via};
use rurge_net::socket::{NoopSocketHook, SocketHook, SocketOpts};
use rurge_policy::{BuildError, OutboundFactory};
use rurge_proto::anytls::AnyTlsOutbound;
use rurge_proto::build::server_of;
use rurge_proto::external::{ExternalOutbound, NoProcessGroups, ProcessHook};
use rurge_proto::http::HttpOutbound;
use rurge_proto::shadowsocks::ShadowsocksOutbound;
use rurge_proto::socks5::Socks5Outbound;
use rurge_proto::trojan::TrojanOutbound;
use rurge_proto::vmess::VmessOutbound;
use rurge_proto::{Direct, OutboundRef};
use rurge_proto_ssh::SshOutbound;
use rurge_proto_wireguard::WireGuardOutbound;
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct EngineFactory {
    resolver: Arc<dyn Resolve>,
    hook: Arc<dyn SocketHook>,
    keystore: Vec<KeystoreItem>,
    roots: Arc<RootCertStore>,
    /// `[General] ipv6`: which family leads when a policy says `dual`.
    v6_first: bool,
    /// A dry build only wants the errors: no warnings. It still loads the
    /// real roots — an empty store cannot even set up standard verification.
    dry: bool,
    /// How `external` programs are started, where they log, and the list
    /// the exit flow stops them from (`with_externals`).
    processes: Arc<dyn ProcessHook>,
    external_logs: PathBuf,
    externals: Arc<ExternalPrograms>,
}

impl EngineFactory {
    /// Trusts the operating system's root certificates.
    pub fn new(
        cfg: &Config,
        resolver: Arc<dyn Resolve>,
        hook: Arc<dyn SocketHook>,
    ) -> EngineFactory {
        EngineFactory::with_roots(cfg, resolver, hook, rurge_net::tls::root_store())
    }

    /// Trusts `roots` instead (tests bring their own CA). `roots` must stay
    /// the same for the engine's lifetime: outbounds are reused across
    /// reloads by fingerprint (M2 design 7.1), and the roots are not part of
    /// it.
    pub fn with_roots(
        cfg: &Config,
        resolver: Arc<dyn Resolve>,
        hook: Arc<dyn SocketHook>,
        roots: Arc<RootCertStore>,
    ) -> EngineFactory {
        EngineFactory {
            resolver,
            hook,
            keystore: cfg.keystore.clone(),
            roots,
            v6_first: cfg.general.ipv6,
            dry: false,
            processes: Arc::new(NoProcessGroups),
            external_logs: PathBuf::from("external"),
            externals: Arc::new(ExternalPrograms::default()),
        }
    }

    /// `external` programs start through `processes`, log into `logs` (the
    /// data directory's `external`) and are listed in `externals`.
    pub fn with_externals(
        mut self,
        processes: Arc<dyn ProcessHook>,
        logs: PathBuf,
        externals: Arc<ExternalPrograms>,
    ) -> EngineFactory {
        self.processes = processes;
        self.external_logs = logs;
        self.externals = externals;
        self
    }

    fn dry(cfg: &Config) -> EngineFactory {
        EngineFactory {
            resolver: Arc::new(NeverResolve),
            hook: Arc::new(NoopSocketHook),
            keystore: cfg.keystore.clone(),
            // Real roots, not empty: loading them is local and free (no
            // network, no dial), and an empty store makes
            // `WebPkiServerVerifier::builder` fail to set up standard
            // verification even for a policy that would build fine for real.
            roots: rurge_net::tls::root_store(),
            v6_first: cfg.general.ipv6,
            dry: true,
            // a build starts nothing (M4 design 7.5)
            processes: Arc::new(NoProcessGroups),
            external_logs: PathBuf::from("external"),
            externals: Arc::new(ExternalPrograms::default()),
        }
    }
}

/// A dry build never dials, so this is never asked.
struct NeverResolve;

impl Resolve for NeverResolve {
    fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(std::future::ready(Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a dry build does not resolve names",
        ))))
    }
}

/// `skip-cert-verify` without a pinned fingerprint: the proxy is not
/// authenticated at all (with a fingerprint, the pin takes over — W0012).
fn skips_verification(spec: &PolicySpec) -> bool {
    spec.proto
        .tls()
        .is_some_and(|tls| tls.skip_cert_verify && tls.fingerprint_sha256.is_none())
}

impl OutboundFactory for EngineFactory {
    fn direct_connector(&self, common: &CommonOpts) -> Arc<dyn Connector> {
        // with `dns-follow-interface`, what the policy looks up — DIRECT's
        // destinations, a proxy's server — is asked through its interface
        // (phase 2 M5 design 8.5)
        let resolver = match (&common.interface, common.dns_follow_interface) {
            (Some(interface), true) => {
                let questions = DirectConnector::with_opts(
                    // the DNS servers are addresses: nothing to look up
                    Arc::new(SystemResolve),
                    SocketOpts {
                        interface: Some(interface.clone()),
                        allow_other_interface: common.allow_other_interface,
                        ip_version: IpVersion::Dual,
                        v6_first: self.v6_first,
                        tos: 0,
                    },
                    self.hook.clone(),
                );
                let via = Via {
                    key: interface.clone(),
                    connector: Arc::new(questions),
                };
                Arc::new(ResolveVia::new(self.resolver.clone(), via)) as Arc<dyn Resolve>
            }
            _ => self.resolver.clone(),
        };
        Arc::new(DirectConnector::with_opts(
            resolver,
            SocketOpts {
                interface: common.interface.clone(),
                allow_other_interface: common.allow_other_interface,
                ip_version: common.ip_version,
                v6_first: self.v6_first,
                tos: common.tos,
            },
            self.hook.clone(),
        ))
    }

    fn environment(&self) -> String {
        // the one `[General]` item captured by value: it goes into every
        // direct connector's `SocketOpts`. The roots and the socket hook are
        // per-process; the resolver is read through its cell.
        format!("ipv6={}", self.v6_first)
    }

    fn roots(&self) -> Arc<RootCertStore> {
        self.roots.clone()
    }

    fn build(
        &self,
        spec: &PolicySpec,
        connector: Arc<dyn Connector>,
    ) -> Result<OutboundRef, BuildError> {
        let outbound: OutboundRef = match &spec.proto {
            ProtoSpec::Direct => Arc::new(Direct::new(connector)),
            ProtoSpec::Reject(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}` is a reject alias and has no outbound of its own",
                    spec.name
                )));
            }
            ProtoSpec::Http(_) => Arc::new(HttpOutbound::from_spec(
                spec,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::Socks5(_) => Arc::new(Socks5Outbound::from_spec(
                spec,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::Trojan(trojan) => Arc::new(TrojanOutbound::new(
                &spec.name,
                server_of(spec)?,
                trojan,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::Vmess(vmess) => Arc::new(VmessOutbound::new(
                &spec.name,
                server_of(spec)?,
                vmess,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::AnyTls(anytls) => Arc::new(AnyTlsOutbound::new(
                &spec.name,
                server_of(spec)?,
                anytls,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::Ssh(ssh) => Arc::new(SshOutbound::new(
                &spec.name,
                server_of(spec)?,
                ssh,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            // destination names without a `dns-server` are resolved here,
            // with `[Host]` (M4-D9); two policies naming one section share
            // its tunnel only when what their carriers go through is alike
            ProtoSpec::WireGuard(wireguard) => Arc::new(
                WireGuardOutbound::new(&spec.name, wireguard, self.resolver.clone(), connector)
                    .with_carrier(format!(
                        "{} ip-version={:?} via={:?}",
                        self.environment(),
                        spec.common.ip_version,
                        spec.common.underlying_proxy
                    )),
            ),
            ProtoSpec::Ss(ss) => Arc::new(ShadowsocksOutbound::new(
                &spec.name,
                server_of(spec)?,
                ss,
                spec.shadow_tls.as_ref(),
                self.roots.clone(),
                connector,
            )?),
            // nothing starts here: the program starts on the first dial
            ProtoSpec::External(external) => {
                let outbound = ExternalOutbound::new(
                    &spec.name,
                    external,
                    &self.external_logs,
                    self.processes.clone(),
                );
                if self.dry {
                    Arc::new(outbound)
                } else {
                    // a rebuilt policy takes its port from the older
                    // generation's outbound when its program first starts
                    let outbound =
                        Arc::new(outbound.with_local_ports(self.externals.local_ports()));
                    self.externals.add(&outbound);
                    outbound
                }
            }
        };
        if !self.dry && skips_verification(spec) {
            tracing::warn!(
                policy = %spec.name,
                "skip-cert-verify is on: the proxy server is not authenticated"
            );
        }
        Ok(outbound)
    }
}

/// Builds every policy that could fail to build and throws the result away:
/// what cannot be built is a load error at the policy's own line. Direct is
/// skipped because `Direct::new` cannot fail; only a reject alias has no
/// outbound of its own. Offline and quick — no name is resolved, no socket
/// opened.
pub fn dry_build(cfg: &Config) -> Diagnostics {
    let factory = EngineFactory::dry(cfg);
    let mut diagnostics = Diagnostics::default();
    for spec in &cfg.specs {
        if matches!(spec.proto, ProtoSpec::Direct | ProtoSpec::Reject(_)) {
            continue;
        }
        if let Err(e) = factory.build(spec, factory.direct_connector(&spec.common)) {
            diagnostics.push(
                Diagnostic::error(
                    codes::E_POLICY_BUILD,
                    format!("policy `{}` cannot be built: {}", spec.name, e.message),
                )
                .at(spec.span.clone()),
            );
        }
    }
    diagnostics
}

/// `rurge_config::config::load` plus the dry build: the one way `run`, a
/// reload and — under `check_profile` — `check` and `POST /v1/profiles/check`
/// read a profile.
pub fn load_checked(path: &Path, opts: &LoadOptions) -> Result<Loaded, LoadError> {
    let mut loaded = load(path, opts)?;
    loaded.diagnostics.extend(dry_build(&loaded.config));
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_config::diagnostic::{Severity, codes};
    use rurge_net::connector::{ConnectOpts, Target};
    use rurge_net::socket::NoopSocketHook;
    use rurge_proto::testing::{FakeSocks5, Socks5Script, echo_server};
    use rurge_proto_ssh::testing::ED25519_WITH_PASSPHRASE;
    use std::path::Path;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn config(text: &str) -> Config {
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        loaded.config
    }

    fn factory(cfg: &Config) -> EngineFactory {
        EngineFactory::new(cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook))
    }

    #[test]
    fn the_environment_follows_what_the_factory_captures_by_value() {
        let v4 = config("[General]\nipv6 = false\n[Rule]\nFINAL,DIRECT\n");
        let v6 = config("[General]\nipv6 = true\n[Rule]\nFINAL,DIRECT\n");
        assert_eq!(factory(&v4).environment(), "ipv6=false");
        assert_eq!(factory(&v6).environment(), "ipv6=true");
    }

    #[test]
    fn every_implemented_protocol_builds() {
        let cfg = config(
            "[Proxy]\nH = http, proxy.test, 8080, alice, s3cret\nHS = https, proxy.test, 443, sni=edge.test\n\
S = socks5, proxy.test, 1080\nST = socks5-tls, proxy.test, 1443, skip-cert-verify=true\n\
T = trojan, proxy.test, 443, password=pw, ws=true, ws-path=/x\n\
V = vmess, proxy.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, ws=true\n\
A = anytls, proxy.test, 443, password=pw\n\
SSH = ssh, proxy.test, 22, username=u, password=pw\n\
SS = ss, proxy.test, 8388, encrypt-method=aes-128-gcm, password=pw, obfs=http, udp-relay=true\n\
SK = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-128-gcm, password=MDEyMzQ1Njc4OWFiY2RlZg==:MDEyMzQ1Njc4OWFiY2RlZg==, shadow-tls-password=st\n\
SN = ss, proxy.test, 8388, encrypt-method=none\n\
Corp = direct, interface=eth9, allow-other-interface=true\nBlock = reject\n[Rule]\nFINAL,DIRECT\n",
        );
        let f = factory(&cfg);
        for (name, outbound_name) in [
            ("H", "H"),
            ("HS", "HS"),
            ("S", "S"),
            ("ST", "ST"),
            ("T", "T"),
            ("V", "V"),
            ("A", "A"),
            ("SSH", "SSH"),
            ("SS", "SS"),
            ("SK", "SK"),
            ("SN", "SN"),
            ("Corp", "DIRECT"),
        ] {
            let spec = cfg
                .spec(name)
                .unwrap_or_else(|| panic!("no spec for {name}"));
            let out = f
                .build(spec, f.direct_connector(&spec.common))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(out.name(), outbound_name);
        }
        let block = cfg.spec("Block").expect("reject aliases have a spec");
        let e = f
            .build(block, f.direct_connector(&block.common))
            .err()
            .expect("a reject alias has no outbound of its own");
        assert_eq!(
            e.message,
            "policy `Block` is a reject alias and has no outbound of its own"
        );
    }

    #[test]
    fn a_vmess_or_anytls_policy_that_cannot_be_built_is_a_load_error() {
        let cfg = config(
            "[Proxy]\nV = vmess, proxy.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, client-cert=cert1\n\
A = anytls, proxy.test, 443, password=s3same0pen, client-cert=cert1\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n",
        );
        let diags = dry_build(&cfg).sorted();
        let messages: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].starts_with("policy `V` cannot be built: keystore item `cert1`"));
        assert!(messages[1].starts_with("policy `A` cannot be built: keystore item `cert1`"));
        for m in &messages {
            assert!(
                !m.contains("hunter2") && !m.contains("s3same0pen") && !m.contains("0233d11c"),
                "{m}"
            );
        }
    }

    /// `rurge check` names the item and what is wrong with it, never the
    /// key (phase 2 M4 design 4.5).
    #[test]
    fn an_ssh_key_rurge_cannot_use_is_a_load_error() {
        let cfg = config(&format!(
            "[Proxy]\nS = ssh, proxy.test, 22, username=u, private-key=key1\n\
[Keystore]\nkey1 = type=openssh-private-key, base64={}\n[Rule]\nFINAL,DIRECT\n",
            rurge_proto_ssh::testing::keystore_item("key1", ED25519_WITH_PASSPHRASE).base64
        ));
        let diags = dry_build(&cfg);
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `S` cannot be built: keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase"
            ]
        );
    }

    /// A sound `ss` line passes the dry build; an SS 2022 key of the wrong
    /// length is already a load error at its line, never quoted (phase 2 M6
    /// design 3.1).
    #[test]
    fn an_ss_policy_passes_the_dry_build_and_a_bad_2022_key_fails_the_load() {
        let cfg = config(
            "[Proxy]\nS = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-256-gcm, \
password=MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=, obfs=tls, udp-relay=true, udp-port=8389\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(dry_build(&cfg).is_empty());
        let loaded = from_text(
            "[Proxy]\nS = ss, proxy.test, 8388, encrypt-method=2022-blake3-aes-256-gcm, \
password=MDEyMzQ1Njc4OWFiY2RlZg==\n[Rule]\nFINAL,DIRECT\n",
            Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        let errors: Vec<(&str, u32, &str)> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .map(|d| {
                let line = d.span.as_ref().map(|s| s.line).unwrap_or(0);
                (d.code, line, d.message.as_str())
            })
            .collect();
        assert_eq!(
            errors,
            [(
                codes::E_INVALID_POLICY_PARAM,
                2,
                "policy `S`: key #1 of `password` is not a Base64 key of 32 bytes, as `2022-blake3-aes-256-gcm` requires"
            )]
        );
        assert!(loaded.config.spec("S").is_none());
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
        // no tokio runtime here: spawning anything at build time would panic
        let cfg =
            config("[Proxy]\nA = anytls, proxy.test, 443, password=pw\n[Rule]\nFINAL,DIRECT\n");
        assert!(dry_build(&cfg).is_empty());
    }

    #[test]
    fn skip_cert_verify_is_noticed_on_the_new_protocols_too() {
        let cfg = config(
            "[Proxy]\nA = anytls, proxy.test, 443, password=pw, skip-cert-verify=true\n\
V = vmess, proxy.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, skip-cert-verify=true\n\
Plain = vmess, proxy.test, 80, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true\n[Rule]\nFINAL,DIRECT\n",
        );
        let flagged: Vec<&str> = cfg
            .specs
            .iter()
            .filter(|s| skips_verification(s))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(flagged, ["A", "V"]);
    }

    #[test]
    fn a_trojan_policy_that_cannot_be_built_is_a_load_error() {
        let cfg = config(
            "[Proxy]\nT = trojan, proxy.test, 443, password=s3same0pen, client-cert=cert1\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n",
        );
        let diags = dry_build(&cfg).sorted();
        let messages: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with("policy `T` cannot be built: keystore item `cert1`"),
            "{}",
            messages[0]
        );
        assert!(!messages[0].contains("hunter2") && !messages[0].contains("s3same0pen"));
    }

    #[tokio::test]
    async fn a_built_outbound_really_connects() {
        let echo = echo_server().await;
        let upstream = FakeSocks5::spawn(Socks5Script::default()).await;
        let cfg = config(&format!(
            "[Proxy]\nS = socks5, 127.0.0.1, {}\n[Rule]\nFINAL,DIRECT\n",
            upstream.addr().port()
        ));
        let f = factory(&cfg);
        let spec = cfg.spec("S").unwrap();
        let out = f.build(spec, f.direct_connector(&spec.common)).unwrap();
        let target = Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
        let mut stream = out
            .connect_tcp(&target, &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        assert_eq!(upstream.requests().len(), 1);
    }

    const BROKEN: &str = "[Proxy]\nGood = http, proxy.test, 8080\nUp = https, proxy.test, 443, client-cert=cert1\nUp2 = socks5-tls, proxy.test, 443, client-cert=empty\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\nempty = type=p12, base64=, password=hunter2\n[Rule]\nFINAL,DIRECT\n";

    #[test]
    fn the_dry_build_reports_what_cannot_be_built_at_the_policys_own_line() {
        let cfg = config(BROKEN);
        let diags = dry_build(&cfg).sorted();
        let found: Vec<(&str, u32)> = diags
            .iter()
            .map(|d| (d.code, d.span.as_ref().map(|s| s.line).unwrap_or(0)))
            .collect();
        assert_eq!(
            found,
            [(codes::E_POLICY_BUILD, 3), (codes::E_POLICY_BUILD, 4)]
        );
        let first = diags.iter().next().unwrap().message.clone();
        assert!(
            first.starts_with("policy `Up` cannot be built: keystore item `cert1`"),
            "{first}"
        );
        for d in diags.iter() {
            assert!(
                !d.message.contains("hunter2") && !d.message.contains("QUJD"),
                "{}",
                d.message
            );
        }
        // nothing to say about a sound profile
        assert!(
            dry_build(&config(
                "[Proxy]\nH = http, h.test, 80\n[Rule]\nFINAL,DIRECT\n"
            ))
            .is_empty()
        );
    }

    #[test]
    fn load_checked_is_load_plus_the_dry_build() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.conf");
        std::fs::write(&path, BROKEN).unwrap();
        let loaded = load_checked(&path, &LoadOptions::for_tests()).unwrap();
        assert!(loaded.diagnostics.has_errors());
        assert_eq!(
            loaded
                .diagnostics
                .iter()
                .filter(|d| d.code == codes::E_POLICY_BUILD)
                .count(),
            2
        );
        assert!(load_checked(&dir.path().join("missing.conf"), &LoadOptions::for_tests()).is_err());
    }

    #[test]
    fn only_a_really_unverified_policy_is_flagged() {
        let cfg = config(
            "[Proxy]\nA = https, h.test, 443, skip-cert-verify=true\n\
B = https, h.test, 443, skip-cert-verify=true, server-cert-fingerprint-sha256=0000000000000000000000000000000000000000000000000000000000000000\n\
C = https, h.test, 443\nD = http, h.test, 80\n[Rule]\nFINAL,DIRECT\n",
        );
        let flagged: Vec<&str> = cfg
            .specs
            .iter()
            .filter(|s| skips_verification(s))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(flagged, ["A"]);
    }

    #[test]
    fn a_trojan_policy_that_skips_verification_is_noticed() {
        let cfg = config(
            "[Proxy]\nT = trojan, proxy.test, 443, password=pw, skip-cert-verify=true\n[Rule]\nFINAL,DIRECT\n",
        );
        assert!(skips_verification(cfg.spec("T").unwrap()));
    }

    /// Regression coverage for `EngineFactory::dry`'s root store: standard
    /// verification (`T`, `S`) needs at least one trust anchor just to set
    /// itself up, so an empty `RootCertStore` fails these two even though
    /// they would build fine for real. The pinned and insecure modes never
    /// consult the root store at all, and are included here as a control.
    #[test]
    fn an_ordinary_tls_policy_passes_the_dry_build() {
        let cfg = config(
            "[Proxy]\nT = https, h.test, 443\nS = socks5-tls, h.test, 443\n\
Pinned = https, h.test, 443, server-cert-fingerprint-sha256=0000000000000000000000000000000000000000000000000000000000000000\n\
Loose = https, h.test, 443, skip-cert-verify=true\n[Rule]\nFINAL,DIRECT\n",
        );
        assert!(dry_build(&cfg).is_empty());
    }

    /// A resolver that answers the loopback and notes how it was asked.
    #[derive(Default)]
    struct Noting(std::sync::Mutex<Vec<String>>);

    impl Resolve for Noting {
        fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            self.0.lock().unwrap().push(host.to_string());
            Box::pin(std::future::ready(Ok(vec![IpAddr::from([127, 0, 0, 1])])))
        }

        fn resolve_via<'a>(
            &'a self,
            host: &'a str,
            via: &'a Via,
        ) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            self.0
                .lock()
                .unwrap()
                .push(format!("{host} via {}", via.key));
            Box::pin(std::future::ready(Ok(vec![IpAddr::from([127, 0, 0, 1])])))
        }
    }

    /// With `dns-follow-interface`, what a policy looks up is asked through
    /// its interface; without it, as usual (phase 2 M5 design 8.5).
    #[tokio::test]
    async fn a_policy_that_follows_its_interface_looks_up_through_it() {
        let echo = echo_server().await;
        let cfg = config(
            "[Proxy]\nFollow = direct, interface=eth9, dns-follow-interface=true\n\
Plain = direct, interface=eth9\n[Rule]\nFINAL,DIRECT\n",
        );
        let noting = Arc::new(Noting::default());
        let f = EngineFactory::new(&cfg, noting.clone(), Arc::new(NoopSocketHook));
        let target = Target::new(rurge_config::HostName::parse("echo.test"), echo.port());
        for name in ["Follow", "Plain"] {
            let spec = cfg.spec(name).unwrap();
            let out = f.build(spec, f.direct_connector(&spec.common)).unwrap();
            out.connect_tcp(&target, &ConnectOpts::default())
                .await
                .unwrap();
        }
        assert_eq!(
            *noting.0.lock().unwrap(),
            ["echo.test via eth9", "echo.test"]
        );
    }
}
