//! The session pipeline (M3 design §7): one immutable `Runtime` per config
//! generation, the `Engine` that dials and relays sessions for the inbound
//! listeners, and the session log.

mod auto;
pub mod control;
pub mod dns_pipeline;
pub mod engine;
mod external_resources;
mod fake_address;
mod http;
pub mod mitm;
pub mod observe;
pub mod outbounds;
pub mod owner;
mod prematch;
pub mod profile;
mod quic_adaptive;
pub mod relay;
mod reload;
pub mod runtime;
mod serve;
pub mod shared;
mod smart;
pub mod sniff;
pub mod stack;
pub mod state;
mod subscriptions;
mod tun;
mod udp;
pub mod views;

pub use control::{Control, EnhancedStatus, LogLevel, Mode, ReloadReport};
pub use engine::{
    Engine, FAKE_ADDRESS_NOT_KNOWN, ListenerSpec, PoliciesView, REFUSED_STRICT, RuleView,
    UnknownPolicy,
};
pub use external_resources::{ExternalResource, UnusedLines};
pub use mitm::{MitmCounts, MitmShared};
pub use observe::{RecordStatus, RequestLog, RequestRecord, TrafficStats, TrafficTotals};
pub use outbounds::{EngineFactory, dry_build, load_checked};
pub use owner::{OwnerLookup, OwnerQuery};
pub use runtime::{Runtime, RuntimeOptions};
/// The capture types and the store the API reads (`phase 4 M4 design 4.1`):
/// handed over the way `ApiTls` is, so that `rurge-api` needs only this crate.
pub use rurge_http::capture;
pub use rurge_http::mitm::ApiTls;
pub use rurge_inbound::{ProcessSlot, Running};
pub use rurge_policy::EmptyGroup;
pub use rurge_policy::testbook::TestResult;
pub use rurge_tun::Counters as TunCounters;
pub use serve::{HttpCounts, SniffedCounts};
pub use shared::{EngineShared, ExternalPrograms, ResolverCell};
pub use subscriptions::check_profile;
pub use views::{GeoIpView, GroupView, MemberView, SelectError};
