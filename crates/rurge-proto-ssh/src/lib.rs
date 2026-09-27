//! The `ssh` outbound (phase 2 M4 design §5): SSH dynamic forwarding, one
//! session per policy with a `direct-tcpip` channel per connection.

pub mod keys;
pub mod outbound;
pub mod pins;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use keys::decode_private_key;
pub use outbound::SshOutbound;
pub use pins::host_key_allowed;
