//! The life of a TUN's stream: what its reads, writes and shutdown answer as
//! its connection ends. A test at the shard's level drives one shard by
//! hand: it hands the shard the app's segments itself and takes the shard's
//! turns at times of its own, so what it checks does not hang on how fast
//! anything runs.

use super::support::{addr, is_reset, is_syn, lone_shard, segment_v4, syn_v4, tcp_header, within};
use crate::TunOptions;
use crate::tcp::TunStream;
use std::io;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// smoltcp closes a socket that has had nothing from the app for its
/// timeout, with no segment to say so; the shard sees it when it next looks
/// at every connection, up to a `SWEEP` later. A shutdown in between answers
/// the reset the read answers, which is the outcome the shard records when
/// it lets the connection go.
#[tokio::test]
async fn a_shutdown_after_the_stacks_own_timeout_is_a_reset() {
    let options = TunOptions {
        timeout: Duration::from_millis(100),
        ..TunOptions::default()
    };
    let epoch = Instant::now();
    let at = |ms| epoch + Duration::from_millis(ms);
    let shard = lone_shard(&options, epoch).shard;
    let (app, dst) = (addr("198.18.0.1:40000"), addr("192.0.2.1:80"));
    let mut turn = shard.state().turn(at(0), &mut vec![syn_v4(app, dst)]);
    let opened = turn.opened.pop().expect("the SYN opens a connection");
    let syn_ack = turn
        .out
        .iter()
        .find(|packet| is_syn(packet))
        .expect("the SYN-ACK");
    let (header, _) = tcp_header(syn_ack).expect("a TCP segment");
    let isn = u32::from_be_bytes(
        syn_ack[header + 4..header + 8]
            .try_into()
            .expect("four bytes"),
    );
    // the app's ACK ends the handshake
    let ack = segment_v4(app, dst, 2, isn.wrapping_add(1), 0x10);
    shard.state().turn(at(1), &mut vec![ack]);
    let mut stream = TunStream::new(shard.clone(), opened.id, opened.shared);
    within("the handshake", stream.established())
        .await
        .expect("the connection is established");
    // past the timeout, before the shard's next look at every connection
    let turn = shard.state().turn(at(200), &mut Vec::new());
    assert!(
        turn.out.iter().any(|packet| is_reset(packet)),
        "smoltcp's timeout resets the connection"
    );
    assert_eq!(
        shard.state().socket_count(),
        1,
        "the shard has not let the connection go yet"
    );
    let error = within("the shutdown", stream.shutdown())
        .await
        .expect_err("a reset, as the read answers");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    let error = within("the read", stream.read(&mut [0; 1]))
        .await
        .expect_err("a reset");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    // the next look at every connection lets it go, a reset
    shard.state().turn(at(1_100), &mut Vec::new());
    assert_eq!(
        shard.state().socket_count(),
        0,
        "the shard has let the connection go"
    );
    let error = within("the shutdown", stream.shutdown())
        .await
        .expect_err("the outcome, a reset");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
}
