//! What becomes of a TUN as its sessions are rejected, as it is told to stop and
//! as its device fails (phase 3 M3 design 5.6). First the rejections that are
//! left to the session: REJECT-DROP holds the connection and then closes it (and
//! lets it go when the connection ends first), the others reset it. Then the
//! stop: every connection is reset and the app is told, a relay stuck on its
//! upstream is cancelled, the resets reach the device even when the device is
//! slow, and a device that takes nothing cannot hold the stop up for longer than
//! a grace. Then a device that fails. The last tests run one shard's task by
//! hand, with channels of their own in place of the reader and the writer.

mod device_fails;
mod rejects;
mod shard_task;
mod stop;

use crate::TunHandle;
use crate::tcp::shard::Records;

/// Nothing is left of the TUN's connections: no socket, no count, no buffer,
/// no entry in any table of any shard.
fn assert_nothing_left(tun: &TunHandle) {
    assert_eq!(tun.sockets_in_use(), 0, "sockets");
    assert_eq!(tun.sockets_counted(), 0, "sockets counted against the cap");
    assert_eq!(tun.half_open(), 0, "half-open connections counted");
    assert_eq!(tun.budget_in_use(), 0, "bytes of buffers");
    assert_eq!(tun.records(), Records::default(), "the shards' tables");
}
