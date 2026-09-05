//! One immutable config generation (M3 design §7.1).

use crate::stack::{Stack, StackOptions, build_stack};
use rurge_config::{Config, Diagnostics};
use rurge_policy::{GroupSelections, PolicyRegistry};
use rurge_proto::{Direct, OutboundRef};
use rurge_rules::{OutboundMode, RuleEngine};
use std::sync::Arc;

pub struct RuntimeOptions {
    pub stack: StackOptions,
    pub outbound_mode: OutboundMode,
    pub selections: GroupSelections,
}

pub struct Runtime {
    pub config: Arc<Config>,
    pub stack: Stack,
    pub rules: RuleEngine,
    pub policies: PolicyRegistry,
    pub outbound_mode: OutboundMode,
}

impl Runtime {
    /// Resources → sets → GeoIP → resolver → rule engine → policy registry.
    /// Must run inside a tokio runtime (background tasks are spawned).
    pub async fn build(config: Config, opts: RuntimeOptions) -> anyhow::Result<Runtime> {
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
        })
    }

    /// Diagnostics produced while building the stack (sets, GeoIP, resolver).
    pub fn diagnostics(&self) -> &Diagnostics {
        &self.stack.diagnostics
    }
}
