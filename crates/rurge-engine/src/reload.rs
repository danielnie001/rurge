//! Hot reload (M3 design §7.4): atomically swap in a new config generation.
//! Building the new `Runtime` (validating config, rebuilding the stack) is the
//! caller's job; the engine only swaps and reports whether listeners must be
//! rebound.

use crate::engine::{Engine, ListenerSpec};
use crate::runtime::Runtime;
use rurge_config::general::General;

/// Everything a listener bakes in when it is bound: what to listen on (kind
/// and address), how to authenticate, and the `[General]` switches that go
/// into its `ListenerOpts`. Nothing refreshes a bound listener in place, so
/// any difference here means the listeners have to be rebuilt — comparing
/// addresses alone would let a rotated proxy password or a newly enabled
/// `proxy-restricted-to-lan` be reported as reloaded and silently ignored.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ListenerSurface {
    /// In `Engine::listener_specs` order.
    specs: Vec<ListenerSpec>,
    restrict_to_lan: bool,
    show_error_page: bool,
    show_error_page_for_reject: bool,
}

/// The listener configuration surface `general` asks for.
pub(crate) fn listener_surface(general: &General) -> ListenerSurface {
    ListenerSurface {
        specs: Engine::listener_specs(general),
        restrict_to_lan: general.proxy_restricted_to_lan,
        show_error_page: general.show_error_page,
        show_error_page_for_reject: general.show_error_page_for_reject,
    }
}

impl Engine {
    /// Atomically swaps in the next config generation. In-flight sessions keep
    /// their snapshot; new sessions use the new one. Returns whether the
    /// listener configuration surface (addresses, authentication, source
    /// restriction, error-page switches) changed, so the caller can rebind
    /// listeners and pick up the new `ListenerOpts` with them. Must be
    /// called inside a tokio runtime: a generation with subscriptions starts
    /// a watcher task.
    pub fn swap_runtime(self: &std::sync::Arc<Self>, mut next: Runtime) -> bool {
        let before = listener_surface(&self.runtime().config.general);
        let after = listener_surface(&next.config.general);
        // The new generation has its own DNS pipeline connector; bind it before
        // the swap so the first DNS query of the new generation already sees it.
        if let Some(pc) = next.dns_pipeline() {
            pc.attach(std::sync::Arc::downgrade(self));
        }
        let receivers = next.subscriptions.take_receivers();
        // `rt` is read inside the same lock that published it: another swap
        // can never land in between and make this attach `receivers` (this
        // generation's own) to a later generation's watcher.
        let rt = {
            let _generation = self.generation_lock();
            self.publish_generation(&mut next);
            self.store_runtime(next);
            self.runtime()
        };
        self.watch_subscriptions(&rt, receivers);
        before != after
    }
}
