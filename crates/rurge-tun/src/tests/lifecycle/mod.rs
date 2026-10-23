//! The life of a TUN's stream: what its reads, writes and shutdown answer as
//! its connection ends, what dropping it does, and what is left of a
//! connection once it is over. A test at the shard's level drives one shard by
//! hand: it hands the shard the app's segments itself and takes the shard's
//! turns at times of its own, so what it checks does not hang on how fast
//! anything runs. The others run a whole TUN on a memory device.

mod drops;
mod end_to_end;
mod stream_ends;
mod unread;

use crate::tcp::{ConnId, ConnShared, Shard, TunStream};
use crate::tests::hazards::{app4, open_by_hand};
use crate::tests::support::{addr, data_segment_v4, tcp_payload_len, within};
use smoltcp::socket::tcp;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FIN_ACK: u8 = 0x11;
const RST_ACK: u8 = 0x14;

/// The TCP payload of every packet in `out`, in order.
fn payloads(out: &[Vec<u8>]) -> Vec<u8> {
    out.iter()
        .flat_map(|packet| {
            packet[packet.len() - tcp_payload_len(packet)..]
                .iter()
                .copied()
        })
        .collect()
}

/// One turn of `shard` at `at`, handed `packets`: what the stack sent in it.
fn turn(shard: &Shard, at: Instant, packets: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    shard.state().turn(at, &mut { packets }).out
}

/// The state of the socket of connection `id`, if the shard still has it.
fn state_of(shard: &Shard, id: ConnId) -> Option<tcp::State> {
    let state = shard.state();
    let conn = state.conns.get(&id)?;
    Some(state.sockets.get::<tcp::Socket>(conn.handle).state())
}

/// The app's end of a connection made by hand on a shard: where it is and the
/// numbers its segments need.
struct ByHand {
    id: ConnId,
    shared: Arc<ConnShared>,
    app: SocketAddr,
    dst: SocketAddr,
    /// The sequence number of the TUN's SYN-ACK.
    isn: u32,
}

impl ByHand {
    /// The connection from port `port` of the app, made at `ms` and a
    /// millisecond later, and the stream a session would have of it.
    async fn make(shard: &Arc<Shard>, epoch: Instant, port: u16, ms: u64) -> (ByHand, TunStream) {
        let (app, dst) = (app4(port), addr("192.0.2.1:80"));
        let (opened, isn) = open_by_hand(shard, epoch, app, dst, ms);
        let by_hand = ByHand {
            id: opened.id,
            shared: opened.shared.clone(),
            app,
            dst,
            isn,
        };
        let stream = TunStream::new(shard.clone(), opened.id, opened.shared);
        within("the handshake", stream.established())
            .await
            .expect("the connection is established");
        (by_hand, stream)
    }

    /// A segment of the app's. The app's SYN was numbered 1, so its first byte
    /// is 2 and `seq` counts from there. `acked` is how many of the TUN's
    /// sequence numbers, bytes and FIN, it acknowledges, counted from the
    /// TUN's SYN-ACK.
    fn segment(&self, seq: u32, acked: u32, flags: u8, payload: &[u8]) -> Vec<u8> {
        data_segment_v4(
            self.app,
            self.dst,
            seq,
            self.isn.wrapping_add(1 + acked),
            flags,
            payload,
        )
    }
}

/// Every read, write and shutdown of `stream` is a reset.
async fn answers_reset(stream: &mut TunStream) {
    let read = within("a read", stream.read(&mut [0; 1]))
        .await
        .expect_err("a read of a reset connection is an error");
    let write = within("a write", stream.write_all(b"x"))
        .await
        .expect_err("a write to a reset connection is an error");
    let shutdown = within("a shutdown", stream.shutdown())
        .await
        .expect_err("a shutdown of a reset connection is an error");
    for error in [read, write, shutdown] {
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    }
}
