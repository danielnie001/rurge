//! One immutable config generation (M3 design §7.1).

use crate::outbounds::EngineFactory;
use crate::prematch::Recorded;
use crate::shared::{EngineShared, OwnResolve};
use crate::stack::{DnsOverride, Stack, StackOptions, build_stack};
use crate::subscriptions::Subscriptions;
use rurge_config::{Config, Diagnostics};
use rurge_http::Rewrite;
use rurge_http::mitm::{Authority, Mitm};
use rurge_net::connector::SystemResolve;
use rurge_policy::PolicyRegistry;
use rurge_rules::{OutboundMode, RuleEngine};
use rurge_tun::TunSettings;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio_util::task::AbortOnDropHandle;

pub struct RuntimeOptions {
    pub stack: StackOptions,
    pub outbound_mode: OutboundMode,
    pub idle_timeout: Duration,
    /// The engine's generation-independent objects: `EngineShared::new` for
    /// the first build, `Engine::shared()` for every later one.
    pub shared: EngineShared,
    pub request_log_size: usize,
}

pub struct Runtime {
    pub config: Arc<Config>,
    pub stack: Stack,
    pub rules: RuleEngine,
    pub outbound_mode: OutboundMode,
    pub idle_timeout: Duration,
    pub request_log_size: usize,
    pub(crate) shared: EngineShared,
    /// Present only when `encrypted-dns-follow-outbound-mode` is on; the engine
    /// attaches itself to it so DNS upstream connections take the dial pipeline.
    pub(crate) dns_pipeline: Option<Arc<crate::dns_pipeline::PipelineConnector>>,
    /// The registry built with this generation, until the engine publishes
    /// it: from then on the one in use is `EngineShared.cell`'s (M3 design
    /// 5.8).
    pub(crate) registry: Option<Arc<PolicyRegistry>>,
    /// What rebuilds the registry when a subscription changes (5.7).
    pub(crate) factory: Arc<EngineFactory>,
    pub(crate) subscriptions: Subscriptions,
    /// The task watching `subscriptions`; it goes with the generation.
    pub(crate) watcher: OnceLock<AbortOnDropHandle<()>>,
    /// What the virtual adapter asks of this generation (phase 3 M3 design
    /// 9.1): handed out by `Hooks::settings` while the generation is in use.
    pub(crate) tun: Arc<TunSettings>,
    /// When each rule of this generation last had a pre-matching refusal put
    /// in the request log (phase 3 M3 design 12.4): a generation records them
    /// anew.
    pub(crate) pre_matched: Recorded,
    /// The DNS override of the network the machine was on when this
    /// generation was built; the resolver was made with it.
    dns_override: DnsOverride,
    /// What this generation decrypts with (phase 4 M2 design 5.4): `None`
    /// when `[MITM]` names no CA, when it cannot be used, or when TLS cannot
    /// be set up. Only the entries and the hand-over read it.
    pub(crate) mitm: Option<Arc<Mitm>>,
    /// What this generation rewrites with (`phase 4 M3 design 8.4`): `None`
    /// when neither section has a usable line, when the roots of the origin
    /// side cannot be read, or when the TLS configurations cannot be built.
    /// The switch of the engine and the policy a request took are asked
    /// elsewhere (`M3-D27`).
    pub(crate) rewrite: Option<Arc<Rewrite>>,
}

impl Runtime {
    /// Resources → sets → GeoIP → resolver → rule engine → policy registry.
    /// Must run inside a tokio runtime (background tasks are spawned).
    pub async fn build(config: Config, mut opts: RuntimeOptions) -> anyhow::Result<Runtime> {
        // the DNS override of the network as the engine's cell has it now
        // (phase 3 M1 design 7.3): the resolver is made with it, and the
        // generation remembers it
        let dns_override = DnsOverride::for_network(&config, &opts.shared.network.load());
        opts.stack.dns_override = dns_override.clone();
        // the names rurge dials itself go to the system's library, or to this
        // engine's generation while the enhanced mode is on (M4 design 7.6);
        // a caller that gives its own keeps it
        if opts.stack.resource_resolve.is_none() {
            opts.stack.resource_resolve = Some(Arc::new(OwnResolve::new(
                opts.stack.system.clone(),
                opts.shared.resolver.clone(),
                Arc::new(SystemResolve),
            )));
        }
        let dns_pipeline = if config.general.encrypted_dns_follow_outbound_mode {
            // the sockets of the fallback are rurge's own: the platform's hook
            // binds them outside the virtual adapter (M4 design 7.2)
            let fallback: Arc<dyn rurge_net::connector::Connector> =
                Arc::new(rurge_net::connector::DirectConnector::with_opts(
                    Arc::new(SystemResolve),
                    rurge_net::socket::SocketOpts::default(),
                    opts.stack.socket_hook.clone(),
                ));
            let pc = crate::dns_pipeline::PipelineConnector::new(fallback);
            opts.stack.dns_connector = Some(pc.connectors());
            Some(pc)
        } else {
            None
        };
        let mut stack = build_stack(&config, &opts.stack).await?;
        // the adapter's echo requests to a fake address look the name up with
        // this generation's resolver, and are forwarded as its profile says;
        // its DNS responder asks the same resolver for the names `[Host]`
        // maps and for the queries it forwards, and answers as the profile
        // says of `always-real-ip`, `allow-dns-svcb` and `hijack-dns`; and its
        // `[IP Rewrite]` is this generation's table
        let tun = Arc::new(TunSettings {
            resolver: stack.resolver.clone(),
            icmp_forwarding: config.general.icmp_forwarding,
            dns: stack.resolver.clone(),
            always_real_ip: config.general.always_real_ip.clone(),
            allow_dns_svcb: config.general.allow_dns_svcb,
            hijack_dns: config.general.hijack_dns.clone(),
            ip_rewrite: Arc::new(config.ip_rewrite.clone()),
            udp_priority: config.general.udp_priority,
        });
        // the engine's cell, not one of this generation's: the network is
        // the machine's, and outlives the profile it is read against
        let rules =
            RuleEngine::build_with_registry(&config, stack.registry.clone(), stack.geo.clone())?
                .with_network(opts.shared.network.clone());
        // The cell, not this generation's resolver: an outbound may outlive
        // the generation it was built in (M2 design 7.2).
        let factory = match &opts.shared.roots {
            Some(roots) => EngineFactory::with_roots(
                &config,
                opts.shared.resolver.clone(),
                opts.stack.socket_hook.clone(),
                roots.clone(),
            ),
            None => EngineFactory::new(
                &config,
                opts.shared.resolver.clone(),
                opts.stack.socket_hook.clone(),
            ),
        };
        let factory = Arc::new(factory.with_externals(
            opts.shared.processes.clone(),
            opts.stack.data_dir.join("external"),
            opts.shared.externals.clone(),
        ));
        // What earlier runs cached is in the first assembly already: no group
        // starts empty for want of a download (M3-D5).
        let subscriptions = Subscriptions::register(&config, &stack.resources);
        let assembly = rurge_policy::assemble(&config, &subscriptions.snapshots());
        stack.diagnostics.extend(assembly.diagnostics.clone());
        // The generation being replaced (none on the first build): whatever
        // it built from the same fingerprint is kept, connection pools and all.
        let previous = opts.shared.cell.load();
        // The dry build has already turned every build failure into a load
        // error (`load_checked`), so this only fails for a caller that skipped it.
        // the groups that pick by the network read the engine's cell, as the
        // rule engine does
        let registry = Arc::new(
            PolicyRegistry::build(
                &config,
                &assembly,
                factory.as_ref(),
                &opts.shared.cell,
                opts.shared.selections.clone(),
                previous.as_deref(),
                opts.shared.empty_group,
                &opts.shared.auto,
            )
            .map_err(|e| anyhow::anyhow!("cannot build the policies: {e}"))?
            .with_network(opts.shared.network.clone()),
        );
        // last: it reads the authority the previous generation left, and the
        // profile is not needed after it
        let mitm = build_mitm(&config, &opts.shared).await;
        let rewrite = build_rewrite(&config, &opts.shared).await;
        Ok(Runtime {
            config: Arc::new(config),
            stack,
            rules,
            outbound_mode: opts.outbound_mode,
            idle_timeout: opts.idle_timeout,
            request_log_size: opts.request_log_size.max(1),
            shared: opts.shared,
            dns_pipeline,
            registry: Some(registry),
            factory,
            subscriptions,
            watcher: OnceLock::new(),
            tun,
            pre_matched: Recorded::default(),
            dns_override,
            mitm,
            rewrite,
        })
    }

    /// The DNS override of the network the machine was on when this generation
    /// was built (phase 3 M1 design 7.3); the resolver was made with it. It
    /// stays what it was. To find out whether the network that is now would
    /// override something else, compare it with `DnsOverride::for_network` of
    /// this generation's config and the network of the moment.
    pub fn dns_override(&self) -> &DnsOverride {
        &self.dns_override
    }

    /// Diagnostics produced while building the stack (sets, GeoIP, resolver),
    /// plus the subscription assembly's warnings (skipped lines, empty
    /// groups, cycles, and the rest of M3 design 5.9).
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.stack.diagnostics
    }

    /// This generation's pipeline connector, when
    /// `encrypted-dns-follow-outbound-mode` is on: the resolver reaches its
    /// TCP / DoT / DoH / DoQ / DoH3 upstreams through the connectors it gives
    /// out (`PipelineConnector::connectors`).
    pub fn dns_pipeline(&self) -> Option<&Arc<crate::dns_pipeline::PipelineConnector>> {
        self.dns_pipeline.as_ref()
    }
}

/// This generation's MITM (phase 4 M2 design 5.4, the phase 4 / M2a plan's
/// P16).
///
/// `None` when `[MITM]` names no CA at all, when the CA cannot be loaded
/// (`W0039` has said why at load time), or when the TLS configurations cannot
/// be built. An authority whose material is the one the last published
/// generation used, and which is still usable, is taken on: the leaves and the
/// TLS sessions it cached are what the connections before it were served
/// with.
///
/// Both the CA and the roots of the origin side are read on a blocking thread:
/// the PKCS#12 decoding does a PBKDF with an iteration count the file decides,
/// and the first read of the system's roots touches the system store.
async fn build_mitm(config: &Config, shared: &EngineShared) -> Option<Arc<Mitm>> {
    let cfg = &config.mitm;
    if cfg.ca_keystore_name.is_none() && cfg.ca_p12.is_none() {
        return None;
    }
    let authority = match shared.mitm.last_live() {
        Some(last) if last.same_material(cfg, &config.keystore) && last.usable_now() => last,
        _ => {
            // `load_checked` has already named what is wrong with the CA
            // (`W0039`), so this is only what the log of a failure says
            let cfg = cfg.clone();
            let keystore = config.keystore.clone();
            match tokio::task::spawn_blocking(move || Authority::load(&cfg, &keystore)).await {
                Ok(Ok(Some(authority))) => Arc::new(authority),
                Ok(Ok(None)) => return None,
                Ok(Err(problem)) => {
                    tracing::debug!(
                        problem = %problem.message,
                        "the MITM CA cannot be loaded; MITM is off for this generation"
                    );
                    return None;
                }
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "loading the MITM CA failed; MITM is off for this generation"
                    );
                    return None;
                }
            }
        }
    };
    let roots = match &shared.roots {
        Some(roots) => Arc::clone(roots),
        None => match tokio::task::spawn_blocking(rurge_net::tls::union_root_store).await {
            Ok(roots) => roots,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "the roots of the origin side cannot be read; MITM is off for this generation"
                );
                return None;
            }
        },
    };
    match Mitm::build(cfg, authority, roots) {
        Ok(mitm) => Some(Arc::new(mitm)),
        Err(e) => {
            tracing::warn!("MITM is off: cannot set up TLS: {e}");
            None
        }
    }
}

/// This generation's rewrites (`phase 4 M3 design 8.4`, the phase 4 / M3a
/// plan's P17).
///
/// Neither section with a usable line means `None`, and the roots of the
/// origin side are not read at all then. They are read from the same place
/// `build_mitm` reads them: the shared trust the caller brought when there is
/// one, the operating system's otherwise, on a blocking thread.
///
/// A failure — the roots cannot be read, or the TLS configurations cannot be
/// built — turns the rewrites of this generation off, not the engine: the
/// next generation builds its own (`phase 4 M3 design 8.4`), and what says so
/// is the log line the failure leaves.
async fn build_rewrite(config: &Config, shared: &EngineShared) -> Option<Arc<Rewrite>> {
    if config.rewrite.is_empty() {
        return None;
    }
    let roots = match &shared.roots {
        Some(roots) => Arc::clone(roots),
        None => match tokio::task::spawn_blocking(rurge_net::tls::union_root_store).await {
            Ok(roots) => roots,
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "the roots of the origin side cannot be read; rewrites are off for this generation"
                );
                return None;
            }
        },
    };
    match Rewrite::build(
        &config.rewrite,
        &config.mitm,
        roots,
        Arc::clone(&shared.rewrite),
    ) {
        Ok(rewrite) => rewrite.map(Arc::new),
        Err(e) => {
            tracing::warn!("rewrites are off: cannot set up TLS: {e}");
            None
        }
    }
}
