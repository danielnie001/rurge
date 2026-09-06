//! The session pipeline (M3 design §7): one immutable `Runtime` per config
//! generation, the `Engine` that dials and relays sessions for the inbound
//! listeners, and the session log.

pub mod engine;
pub mod relay;
pub mod runtime;
pub mod stack;
pub mod state;

pub use engine::{Engine, ListenerSpec};
pub use runtime::{Runtime, RuntimeOptions};
