//! One immutable config generation (M3 design §7.1).

use crate::outbounds::EngineFactory;
use crate::prematch::Recorded;
use crate::shared::{EngineShared, OwnResolve};
use crate::stack::{DnsOverride, Stack, StackOptions, build_stack};
use crate::subscriptions::Subscriptions;
use rurge_config::{Config, Diagnostics};
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
        // says of `always-real-ip`, `allow-dns-svcb` and `hijack-dns`
        let tun = Arc::new(TunSettings {
            resolver: stack.resolver.clone(),
            icmp_forwarding: config.general.icmp_forwarding,
            dns: stack.resolver.clone(),
            always_real_ip: config.general.always_real_ip.clone(),
            allow_dns_svcb: config.general.allow_dns_svcb,
            hijack_dns: config.general.hijack_dns.clone(),
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
