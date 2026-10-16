//! Platform-specific helpers. Only this crate (and `rurge-tun`) may contain
//! platform-specific code (AR-02).
pub mod command;
pub mod dirs;
pub mod dns;
pub mod netenv;
pub mod process;
pub mod service;
pub mod socket;
pub mod sysinfo;
pub mod sysproxy;
