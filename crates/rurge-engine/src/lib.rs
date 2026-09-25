//! The session pipeline (M3 design §7): one immutable `Runtime` per config
//! generation, the `Engine` that dials and relays sessions for the inbound
//! listeners, and the session log.

mod auto;
pub mod control;
pub mod dns_pipeline;
pub mod engine;
pub mod observe;
pub mod outbounds;
pub mod relay;
mod reload;
pub mod runtime;
pub mod shared;
pub mod sniff;
pub mod stack;
pub mod state;
mod subscriptions;
pub mod views;

pub use control::{Control, LogLevel, Mode, ReloadReport};
pub use engine::{Engine, ListenerSpec, PoliciesView, RuleView, UnknownPolicy};
pub use observe::{RecordStatus, RequestLog, RequestRecord, TrafficStats, TrafficTotals};
pub use outbounds::{EngineFactory, dry_build, load_checked};
pub use runtime::{Runtime, RuntimeOptions};
pub use rurge_inbound::Running;
pub use rurge_policy::EmptyGroup;
pub use shared::{EngineShared, ResolverCell};
pub use subscriptions::check_profile;
pub use views::{GroupView, MemberView, SelectError};
