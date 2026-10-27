//! What the L2 privileged tests and their helper program both need (M4 design
//! section 14, M4-D27; M4c plan task 9).
//!
//! Only the DNS wire format lives here so far. The helper's five uses are all
//! networking (a TCP echo server, a DNS server, a TCP client, a DNS client and
//! an HTTP client), and of those only the DNS messages are worth testing
//! without a machine to run them on. Kept in the package's library rather than
//! in the program, they are compiled and tested on the development machine as
//! well as on Linux. The helper program itself is not platform-specific either:
//! it builds on every platform, and `clippy` sees it there. Only
//! `tests/linux_netns.rs`, which starts it, is Linux's alone.
//!
//! Nothing here is platform-specific, and nothing here touches a machine.

pub mod dns;
