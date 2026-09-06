//! Hot reload (M3 design §7.4): atomically swap in a new config generation.
//! Building the new `Runtime` (validating config, rebuilding the stack) is the
//! caller's job; the engine only swaps and reports whether listeners must be
//! rebound.

use crate::engine::Engine;
use crate::runtime::Runtime;
use rurge_config::general::General;
use rurge_config::session::ListenerKind;
use std::collections::BTreeSet;
use std::net::SocketAddr;

/// The set of (kind, address) a config asks listeners to bind, in the same
/// derivation as `Engine::listener_specs`.
pub(crate) fn listen_addrs(general: &General) -> BTreeSet<(u8, SocketAddr)> {
    Engine::listener_specs(general)
        .into_iter()
        .map(|s| {
            let tag = match s.kind {
                ListenerKind::Http => 0u8,
                ListenerKind::Socks5 => 1,
                ListenerKind::Tun => 2,
                ListenerKind::Forward => 3,
                ListenerKind::Internal => 4,
            };
            (tag, s.addr)
        })
        .collect()
}

impl Engine {
    /// Atomically swaps in the next config generation. In-flight sessions keep
    /// their snapshot; new sessions use the new one. Returns whether the set of
    /// listen addresses changed, so the caller can rebind listeners.
    pub fn swap_runtime(self: &std::sync::Arc<Self>, next: Runtime) -> bool {
        let before = listen_addrs(&self.runtime().config.general);
        let after = listen_addrs(&next.config.general);
        // The new generation has its own DNS pipeline connector; bind it before
        // the swap so the first DNS query of the new generation already sees it.
        if let Some(pc) = next.dns_pipeline() {
            pc.attach(std::sync::Arc::downgrade(self));
        }
        self.store_runtime(next);
        before != after
    }
}
