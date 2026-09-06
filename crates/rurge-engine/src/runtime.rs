//! One immutable config generation (M3 design §7.1).

use crate::stack::{Stack, StackOptions, build_stack};
use rurge_config::{Config, Diagnostics};
use rurge_policy::{GroupSelections, PolicyRegistry};
use rurge_proto::{Direct, OutboundRef};
use rurge_rules::{OutboundMode, RuleEngine};
use std::sync::Arc;
use std::time::Duration;

pub struct RuntimeOptions {
    pub stack: StackOptions,
    pub outbound_mode: OutboundMode,
    pub idle_timeout: Duration,
    pub selections: GroupSelections,
    pub request_log_size: usize,
}

pub struct Runtime {
    pub config: Arc<Config>,
    pub stack: Stack,
    pub rules: RuleEngine,
    pub policies: PolicyRegistry,
    pub outbound_mode: OutboundMode,
    pub idle_timeout: Duration,
    pub request_log_size: usize,
    /// Present only when `encrypted-dns-follow-outbound-mode` is on; the engine
    /// attaches itself to it so DNS upstream connections take the dial pipeline.
    pub(crate) dns_pipeline: Option<Arc<crate::dns_pipeline::PipelineConnector>>,
}

impl Runtime {
    /// Resources → sets → GeoIP → resolver → rule engine → policy registry.
    /// Must run inside a tokio runtime (background tasks are spawned).
    pub async fn build(config: Config, mut opts: RuntimeOptions) -> anyhow::Result<Runtime> {
        let dns_pipeline = if config.general.encrypted_dns_follow_outbound_mode {
            let fallback: Arc<dyn rurge_net::connector::Connector> =
                Arc::new(rurge_net::connector::DirectConnector::new(Arc::new(
                    rurge_net::connector::SystemResolve,
                )));
            let pc = crate::dns_pipeline::PipelineConnector::new(fallback);
            opts.stack.dns_connector = Some(pc.clone());
            Some(pc)
        } else {
            None
        };
        let stack = build_stack(&config, &opts.stack).await?;
        let rules =
            RuleEngine::build_with_registry(&config, stack.registry.clone(), stack.geo.clone())?;
        let direct: OutboundRef = Arc::new(Direct::with_resolver(stack.resolver.clone()));
        let policies = PolicyRegistry::build(&config, &opts.selections, direct);
        Ok(Runtime {
            config: Arc::new(config),
            stack,
            rules,
            policies,
            outbound_mode: opts.outbound_mode,
            idle_timeout: opts.idle_timeout,
            request_log_size: opts.request_log_size.max(1),
            dns_pipeline,
        })
    }

    /// Diagnostics produced while building the stack (sets, GeoIP, resolver).
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.stack.diagnostics
    }

    /// The connector this generation's resolver dials TCP / DoT / DoH upstreams
    /// through, when `encrypted-dns-follow-outbound-mode` is on.
    pub fn dns_pipeline(&self) -> Option<&Arc<crate::dns_pipeline::PipelineConnector>> {
        self.dns_pipeline.as_ref()
    }
}
