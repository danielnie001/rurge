//! Layers between a connector's stream and a protocol's own handshake.

pub mod head;
pub mod lazy_head;
pub mod prefixed;
pub mod shadow_tls;
pub mod stack;
pub mod tls;
pub mod ws;

pub use stack::Stack;
