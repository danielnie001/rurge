//! The virtual adapter on Windows (M4 design 8.4, 8.5).
//!
//! This version has the half before the adapter: finding `wintun.dll`,
//! checking who signed it, and loading it ([`wintun`]). The adapter, its
//! session and the device come in the M4d plan's task 5, and until then
//! `tun::mod`'s platform for Windows is still `unsupported` (the plan's
//! execution record row 12).

mod wintun;
