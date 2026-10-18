//! The TCP path through the TUN: a connection the app's stack opens through
//! the memory device is handed to the `Dialer` once its handshake is over,
//! and relayed to what the `Dialer` dials.

use super::support::{
    Rig, WITHIN, addr, eventually, hello, is_syn, read_pattern, rig, rig_on, within, write_pattern,
};
use crate::device::MemoryDevice;
use crate::testing::{Direction, Script, Upstream};
use crate::{Counters, Ending, TunOptions};
use rurge_config::{HostName, ListenerKind, Transport};
use rurge_proto::RejectKind;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// A connection from the app to `to` is relayed to an echo server: `hello`
/// comes back, and the one session the `Dialer` saw says where it came from
/// and where it went.
async fn a_connection_is_relayed(to: SocketAddr, app_address: &str) {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    rig.dialer.script(to, Script::Upstream(echo.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(to))
        .await
        .expect("connect");
    tcp.write_all(b"hello").await.expect("write");
    let mut back = [0; 5];
    within("hello to come back", tcp.read_exact(&mut back))
        .await
        .expect("read");
    assert_eq!(&back, b"hello");
    let dialed = rig.dialer.dialed();
    assert_eq!(dialed.len(), 1, "one session: {dialed:?}");
    let session = &dialed[0];
    assert_eq!(session.src, tcp.local());
    assert_eq!(session.src.ip(), app_address.parse::<IpAddr>().unwrap());
    assert_eq!(session.in_port, 0);
    assert_eq!(session.listener, ListenerKind::Tun);
    assert_eq!(session.dst_host, HostName::Ip(to.ip()));
    assert_eq!(session.dst_port, to.port());
    assert_eq!(session.transport, Transport::Tcp);
    assert_eq!(rig.tun.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_is_relayed_v4() {
    a_connection_is_relayed(addr("192.0.2.1:80"), "198.18.0.1").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_is_relayed_v6() {
    a_connection_is_relayed(addr("[2001:db8::1]:443"), "fd00:6152::1").await;
}

/// A server on the loopback for one connection: it reads `bytes` bytes, each
/// checked against the pattern, then waits for the end of the stream.
async fn pattern_reader(bytes: usize) -> (SocketAddr, JoinHandle<io::Result<()>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let address = listener.local_addr().expect("the listener's address");
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        read_pattern(&mut stream, bytes).await?;
        let mut more = [0; 1];
        match stream.read(&mut more).await? {
            0 => Ok(()),
            n => Err(io::Error::other(format!("{n} bytes past the pattern"))),
        }
    });
    (address, task)
}

/// 1 MiB each way through `rig`, both connections to the end: the upload goes
/// to a server that checks every byte, the download comes from one that sends
/// the pattern.
async fn a_mebibyte_crosses_each_way_through(rig: &Rig) {
    const MIB: usize = 1 << 20;
    // the upload goes to a server that checks every byte (the sink only counts them)
    let (reader, reading) = pattern_reader(MIB).await;
    let source = Upstream::source(MIB as u64).await;
    let (up, down) = (addr("192.0.2.1:80"), addr("192.0.2.2:80"));
    rig.dialer.script(up, Script::Upstream(reader));
    rig.dialer.script(down, Script::Upstream(source.addr()));
    let upload = async {
        let mut tcp = rig.app.connect(up).await?;
        write_pattern(&mut tcp, MIB).await?;
        tcp.shutdown().await?;
        // the TUN closes its end once the reader has closed its own
        let mut more = [0; 1];
        assert_eq!(tcp.read(&mut more).await?, 0, "nothing comes back");
        io::Result::Ok(())
    };
    let download = async {
        let mut tcp = rig.app.connect(down).await?;
        read_pattern(&mut tcp, MIB).await?;
        let mut more = [0; 1];
        assert_eq!(tcp.read(&mut more).await?, 0, "nothing past the pattern");
        io::Result::Ok(())
    };
    let (uploaded, downloaded) = timeout(Duration::from_secs(30), async {
        tokio::join!(upload, download)
    })
    .await
    .expect("a mebibyte each way within 30 s");
    uploaded.expect("the upload");
    downloaded.expect("the download");
    within("the reader to finish", reading)
        .await
        .expect("the reader's task")
        .expect("the reader");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mebibyte_crosses_each_way() {
    let rig = rig(TunOptions::default());
    a_mebibyte_crosses_each_way_through(&rig).await;
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// With the budget spent every socket has `buffer_min` and no more, which is
/// what every socket made after the first few hundred has in a TUN that is
/// busy. A mebibyte each way crosses on those buffers at an MTU of `mtu`.
async fn a_mebibyte_each_way_on_the_smallest_buffers(mtu: usize) {
    let options = TunOptions {
        budget: 0,
        ..TunOptions::default()
    };
    let smallest = options.buffer_min;
    let rig = rig_on(MemoryDevice::pair(mtu, 4096), options);
    // a socket made now has the smallest buffers both ways: the transfers
    // below are run on nothing larger
    let echo = Upstream::echo().await;
    let probe_to = addr("192.0.2.3:80");
    rig.dialer.script(probe_to, Script::Upstream(echo.addr()));
    let probe = within("the connection to be made", rig.app.connect(probe_to))
        .await
        .expect("connect");
    assert_eq!(rig.tun.budget_in_use(), 2 * smallest);
    drop(probe);
    eventually("the TUN to let the connection go", WITHIN, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;
    a_mebibyte_crosses_each_way_through(&rig).await;
    assert_eq!(rig.tun.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mebibyte_crosses_each_way_on_the_smallest_buffers() {
    a_mebibyte_each_way_on_the_smallest_buffers(1500).await;
}

/// The MSS here, 8960, is larger than the window of the smallest buffers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mebibyte_crosses_each_way_on_the_smallest_buffers_at_mtu_9000() {
    a_mebibyte_each_way_on_the_smallest_buffers(9000).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn half_close_keeps_the_other_direction() {
    const BYTES: usize = 64 * 1024;
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    within("the bytes to be written", write_pattern(&mut tcp, BYTES))
        .await
        .expect("write");
    tcp.shutdown().await.expect("shutdown");
    within("the bytes to come back", read_pattern(&mut tcp, BYTES))
        .await
        .expect("read back");
    let mut more = [0; 1];
    let end = within("the end of the stream", tcp.read(&mut more))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0);
    assert!(echo.wait_for(BYTES as u64, WITHIN).await);
    // both ends are closed: the TUN lets the socket and its buffers go
    eventually("the TUN to let the connection go", WITHIN, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;
    assert_eq!(rig.dialer.relay_ends(), vec![Ok(())]);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// The handshake is over before the session is dialed, so `connect`
/// succeeds whatever the `Dialer` answers; an answer that is not a stream
/// resets the connection.
async fn the_connection_is_reset(script: Option<Script>) {
    let rig = rig(TunOptions::default());
    let dst = addr("192.0.2.1:80");
    if let Some(script) = script {
        rig.dialer.script(dst, script);
    }
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let mut buf = [0; 1];
    let error = within("the reset", tcp.read(&mut buf))
        .await
        .expect_err("a reset, not data or the end of the stream");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    let dialed = rig.dialer.dialed();
    assert_eq!(dialed.len(), 1, "one session: {dialed:?}");
    assert_eq!(dialed[0].dst_host, HostName::Ip(dst.ip()));
    assert_eq!(dialed[0].dst_port, dst.port());
    eventually("the TUN to let the connection go", WITHIN, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;
    assert_eq!(rig.tun.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_dial_resets_the_connection() {
    // nothing is scripted for the destination: the fake `Dialer` fails it
    the_connection_is_reset(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rejected_session_resets_the_connection() {
    the_connection_is_reset(Some(Script::Reject(RejectKind::Reject))).await;
}

/// The session starts once the TUN's side of the handshake is over, that is
/// once the app's ACK of the SYN-ACK is in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_is_dialed_before_the_handshake_completes() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    // what the app sends after its SYN is held: the ACK that ends the handshake
    let held = Arc::new(Mutex::new(Some(Vec::new())));
    let holding = held.clone();
    rig.app.set_filter(move |direction, packet| {
        if direction == Direction::FromTun || is_syn(packet) {
            return true;
        }
        match holding.lock().expect("the held packets").as_mut() {
            Some(held) => {
                held.push(packet.clone());
                false
            }
            None => true,
        }
    });
    let mut tcp = within("the app's side of the handshake", rig.app.connect(dst))
        .await
        .expect("connect");
    // the one fixed wait, for what must not happen: a session dialed before
    // the handshake is over, which would start at the SYN; 200 ms leaves its
    // dial, a connect on the loopback, a wide margin
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        rig.dialer.dialed().is_empty(),
        "nothing is dialed before the handshake is over"
    );
    let acks = held
        .lock()
        .expect("the held packets")
        .take()
        .expect("held until now");
    assert!(!acks.is_empty(), "the app acknowledged the TUN's SYN-ACK");
    for packet in acks {
        rig.app.inject(packet);
    }
    eventually("the session to be dialed", WITHIN, || {
        rig.dialer.dialed().len() == 1
    })
    .await;
    hello(&mut tcp).await;
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// The timers of the two tests of the timeout, short enough for a test. The
/// app's stack here is smoltcp, which answers a keep-alive at most once a
/// second however often they come (it rate-limits its challenge ACKs): a
/// timeout of 3 s leaves the app's side about 2 s to be late by. With 1 s
/// the TUN would give up on it just before its next answer.
fn short_timers() -> TunOptions {
    TunOptions {
        keep_alive: Duration::from_millis(100),
        timeout: Duration::from_secs(3),
        ..TunOptions::default()
    }
}

/// smoltcp resets a connection that has had nothing from the app for its
/// timeout; the keep-alives have a live app answer, so a connection idle
/// both ways stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_connection_outlives_the_timeout() {
    let options = short_timers();
    let idle = options.timeout * 2;
    let rig = rig(options);
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    // the one fixed wait, for what must not happen: the idle connection
    // reset; twice the timeout: without the keep-alives the TUN resets it a
    // timeout after the first hello, a whole timeout before the second
    tokio::time::sleep(idle).await;
    hello(&mut tcp).await;
    assert!(rig.dialer.relay_ends().is_empty(), "the relay goes on");
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// An app that has gone silent: nothing the TUN sends reaches it, so it
/// answers no keep-alive, and the connection goes after the timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_app_is_reset_after_the_timeout() {
    let rig = rig(short_timers());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let _tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    // the session is dialed once the handshake is over on the TUN's side too
    eventually("the session to be dialed", WITHIN, || {
        rig.dialer.dialed().len() == 1
    })
    .await;
    rig.app
        .set_filter(|direction, _| direction != Direction::FromTun);
    eventually("the TUN to let the connection go", WITHIN, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;
    eventually("the relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    assert!(
        rig.dialer.relay_ends()[0].is_err(),
        "the relay ends with the reset, not the end of the stream: {:?}",
        rig.dialer.relay_ends()
    );
    assert_eq!(rig.tun.counters(), Counters::default());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_ends_the_tun() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    // a session in the middle of its relay, which the stop has to end too
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    tcp.write_all(b"hello").await.expect("write");
    let mut back = [0; 5];
    within("hello to come back", tcp.read_exact(&mut back))
        .await
        .expect("read");
    rig.tun.stop();
    let ending = timeout(Duration::from_secs(5), rig.tun.join())
        .await
        .expect("every task of the TUN to end within 5 s");
    assert_eq!(ending, Ending::Stopped);
    let again = timeout(Duration::from_secs(5), rig.tun.join())
        .await
        .expect("the same answer again");
    assert_eq!(again, Ending::Stopped);
    assert_eq!(rig.dialer.relay_ends().len(), 1, "the relay ended");
    assert_eq!(rig.tun.counters(), Counters::default());
}

#[tokio::test]
#[should_panic(expected = "at least 1280")]
async fn a_device_mtu_below_1280_is_refused() {
    let _rig = rig_on(MemoryDevice::pair(1279, 16), TunOptions::default());
}
