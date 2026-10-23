//! A TUN, its app and its upstream: whole connections on a memory device,
//! closed, reset, left unread and opened one after another.

use crate::tcp::shard::Records;
use crate::testing::{Direction, Script, Upstream, pattern};
use crate::tests::support::{
    WITHIN, addr, eventually, hello, is_fin, read_pattern, read_pattern_from, rig, within,
};
use crate::{Counters, TunOptions};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

/// Both sides close their write sides, in both orders, and nothing is left:
/// when the app closes first the TUN's socket is the passive closer (LAST-ACK)
/// and when the far end does it is the active one (TIME-WAIT).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_graceful_close_leaves_no_socket() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let source = Upstream::source(10_000).await;
    let (app_first, far_first) = (addr("192.0.2.1:80"), addr("192.0.2.2:80"));
    // a socket in TIME-WAIT goes within a few milliseconds of the TUN seeing
    // it there; smoltcp would let it go after ten seconds
    let five_seconds = Duration::from_secs(5);
    rig.dialer.script(app_first, Script::Upstream(echo.addr()));
    rig.dialer
        .script(far_first, Script::Upstream(source.addr()));

    // the app closes first; the echo server closes its own side when it has
    // read to the end
    let mut tcp = within("the connection to be made", rig.app.connect(app_first))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    tcp.shutdown().await.expect("shutdown");
    let mut more = [0; 1];
    let end = within("the end of the stream", tcp.read(&mut more))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0);
    drop(tcp);
    eventually("the first connection to go", five_seconds, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;

    // the far end closes first, the app closes when it has read to the end
    let mut tcp = within("the connection to be made", rig.app.connect(far_first))
        .await
        .expect("connect");
    within("the bytes of the source", read_pattern(&mut tcp, 10_000))
        .await
        .expect("read");
    let end = within("the end of the stream", tcp.read(&mut more))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0);
    tcp.shutdown().await.expect("shutdown");
    drop(tcp);
    eventually("the second connection to go", five_seconds, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;

    eventually("both relays to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 2
    })
    .await;
    assert_eq!(rig.dialer.relay_ends(), vec![Ok(()), Ok(())]);
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.records(), Records::default());
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// What `value` reports once it has stopped changing: it has not changed for
/// `quiet`, or the test fails after `WITHIN`. This is the one fixed wait of the
/// test that uses it, for what is not to happen (the source writing on): 200 ms
/// of it. Once the TUN's send buffer is full the source's count of written
/// bytes went on changing for 43 ms at the most in thirty runs alone on the
/// development machine; a quiet that came too early would only start the
/// reading earlier, and nothing the test asserts would be different.
async fn settled(what: &str, quiet: Duration, value: impl Fn() -> u64) -> u64 {
    let started = Instant::now();
    let mut last = value();
    let mut since = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let now = value();
        if now != last {
            last = now;
            since = Instant::now();
        } else if since.elapsed() >= quiet {
            return last;
        }
        assert!(started.elapsed() < WITHIN, "timed out waiting for {what}");
    }
}

/// A download whose app stops reading: the app's window closes, the TUN's send
/// buffer fills, the relay stops reading its upstream and the source stops
/// writing. When the app reads again the rest comes, every byte of it and in
/// order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_that_stops_and_resumes_loses_nothing() {
    const TOTAL: usize = 4 << 20;
    const FIRST: usize = 64 << 10;
    let rig = rig(TunOptions::default());
    let source = Upstream::source(TOTAL as u64).await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(source.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    within("the first 64 KiB", read_pattern(&mut tcp, FIRST))
        .await
        .expect("read");
    // the app reads nothing more, and the TUN's send buffer fills
    eventually("the TUN's send buffer to fill", WITHIN, || {
        let (queued, room) = rig.tun.send_buffers();
        room > 0 && queued == room
    })
    .await;
    // the relay is stuck on a write that the buffer does not take, and nothing
    // reaches the source's end any more: 200 ms with no byte more written
    let stopped = settled("the source to stop", Duration::from_millis(200), || {
        source.sent()
    })
    .await;
    let (queued, room) = rig.tun.send_buffers();
    assert_eq!(queued, room, "the TUN's send buffer is still full");
    assert!(stopped >= FIRST as u64, "the source wrote what was read");
    // the app reads on: the rest is the pattern from where it was
    within(
        "the rest of the download",
        read_pattern_from(&mut tcp, FIRST, TOTAL - FIRST),
    )
    .await
    .expect("read");
    let mut more = [0; 1];
    let end = within("the end of the stream", tcp.read(&mut more))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0, "nothing comes past the pattern");
    assert_eq!(source.sent(), TOTAL as u64);
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// An upload the app resets half way (the user closed the page, the process was
/// killed): the session ends with the reset, the connection to the sink is
/// closed, and nothing is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_reset_mid_transfer_ends_the_session() {
    let rig = rig(TunOptions::default());
    let sink = Upstream::sink().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(sink.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    within("the upload to get under way", async {
        let chunk = vec![0x5a; 16 * 1024];
        while sink.received() < 256 * 1024 {
            tcp.write_all(&chunk).await.expect("write");
        }
    })
    .await;
    assert_eq!(sink.open(), 1, "the sink has the connection");
    tcp.abort();
    let five_seconds = Duration::from_secs(5);
    eventually("the session to end", five_seconds, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    let ends = rig.dialer.relay_ends();
    assert!(
        matches!(&ends[0], Err(text) if text.contains("reset")),
        "the session ends with the reset: {ends:?}"
    );
    eventually("the TUN to let the connection go", five_seconds, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;
    eventually("the sink's connection to end", five_seconds, || {
        sink.open() == 0
    })
    .await;
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.records(), Records::default());
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// Short connections one after another, like a client that opens one for each
/// request. With one shard the handles of smoltcp's sockets are used again and
/// again: each connection gets back the bytes it sent, and when the last is
/// over there is nothing left, in the sockets, the budget, the counts or the
/// tables. The far end is an echo in memory: a server on the loopback would
/// cost the machine a port of its own for each connection, held for minutes
/// after the test, and a few runs in a row would leave it none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_thousand_connections_in_a_row_leave_nothing_behind() {
    const CONNECTIONS: usize = 2000;
    const BYTES: usize = 1024;
    let rig = rig(TunOptions {
        shards: 1,
        ..TunOptions::default()
    });
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::MemoryEcho { buffer: 4096 });
    let mut handles = HashSet::new();
    let all = async {
        for n in 0..CONNECTIONS {
            let mut tcp = within("the connection to be made", rig.app.connect(dst))
                .await
                .expect("connect");
            // every connection has bytes of its own
            let sent: Vec<u8> = (0..BYTES)
                .map(|k| pattern((n * BYTES + k) as u64))
                .collect();
            tcp.write_all(&sent).await.expect("write");
            let mut back = vec![0; BYTES];
            within("the bytes to come back", tcp.read_exact(&mut back))
                .await
                .expect("read");
            assert_eq!(back, sent, "connection {n} gets its own bytes back");
            handles.extend(
                rig.tun.inner.shards[0]
                    .state()
                    .conns
                    .values()
                    .map(|conn| conn.handle),
            );
            tcp.shutdown().await.expect("shutdown");
            let mut more = [0; 1];
            let end = within("the end of the stream", tcp.read(&mut more))
                .await
                .expect("the end, not an error");
            assert_eq!(end, 0, "connection {n}");
        }
    };
    timeout(Duration::from_secs(120), all)
        .await
        .expect("two thousand connections within 120 s");
    eventually("the TUN to let every connection go", WITHIN, || {
        rig.tun.sockets_in_use() == 0
            && rig.tun.sockets_counted() == 0
            && rig.tun.half_open() == 0
            && rig.tun.budget_in_use() == 0
            && rig.tun.records() == Records::default()
    })
    .await;
    assert_eq!(rig.dialer.dialed().len(), CONNECTIONS);
    eventually("every relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == CONNECTIONS
    })
    .await;
    assert!(
        rig.dialer.relay_ends().iter().all(|end| end.is_ok()),
        "every relay ended well"
    );
    // the premise: the handles of the sockets were used again
    assert!(
        handles.len() < CONNECTIONS / 10,
        "{} handles for {CONNECTIONS} connections",
        handles.len()
    );
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// A connection whose stream is dropped while the app's FIN, the TUN's own and
/// the acknowledgement of it are what is left to go: from the TUN's FIN on
/// nothing it sends reaches the app, so the FIN is never acknowledged, and the
/// connection stays in LAST-ACK (`timeout` and `keep_alive` are far longer than
/// anything here) until the `linger` that follows the drop runs out. It goes
/// then, with a reset that the app does not see either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dropped_stream_lingers_then_goes() {
    let rig = rig(TunOptions {
        linger: Duration::from_millis(300),
        ..TunOptions::default()
    });
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let swallowing = Arc::new(AtomicBool::new(false));
    let seen = swallowing.clone();
    rig.app.set_filter(move |direction, packet| {
        if direction == Direction::FromTun && is_fin(packet) {
            seen.store(true, Ordering::SeqCst);
        }
        direction != Direction::FromTun || !seen.load(Ordering::SeqCst)
    });
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    // the app's FIN: the relay passes it on, the echo server closes its own
    // side, and the relay shuts the stream down and drops it
    tcp.shutdown().await.expect("shutdown");
    eventually("the relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    // the FIN goes out a moment after the stream has shut down and is gone
    eventually("the TUN's FIN to be swallowed", WITHIN, || {
        swallowing.load(Ordering::SeqCst)
    })
    .await;
    eventually(
        "the TUN to let the connection go",
        Duration::from_secs(5),
        || rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0,
    )
    .await;
    assert_eq!(rig.dialer.relay_ends(), vec![Ok(())]);
    assert_eq!(rig.tun.sockets_counted(), 0);
    assert_eq!(rig.tun.records(), Records::default());
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// The app's FIN is the end of the stream for the relay: the echo server reads
/// to the end and closes its side, and the relay ends well. The app's reset is
/// an error for it: the relay ends with it and the upstream connection is
/// closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fin_and_reset_reach_the_relay_as_eof_and_error() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));

    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    tcp.write_all(b"bye").await.expect("write");
    tcp.shutdown().await.expect("shutdown");
    let mut back = [0; 3];
    within("bye to come back", tcp.read_exact(&mut back))
        .await
        .expect("read");
    assert_eq!(&back, b"bye");
    // the echo server read to the end, or it would not have closed its side
    let end = within("the end of the stream", tcp.read(&mut back))
        .await
        .expect("the end, not an error");
    assert_eq!(end, 0);
    eventually("the relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 1
    })
    .await;
    assert_eq!(rig.dialer.relay_ends(), vec![Ok(())]);
    assert_eq!(echo.received(), 3);

    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    tcp.abort();
    eventually("the relay to end", WITHIN, || {
        rig.dialer.relay_ends().len() == 2
    })
    .await;
    let ends = rig.dialer.relay_ends();
    assert!(
        matches!(&ends[1], Err(text) if text.contains("reset")),
        "the relay ends with the reset: {ends:?}"
    );
    eventually("the echo server's connection to end", WITHIN, || {
        echo.open() == 0
    })
    .await;
    eventually("the TUN to let the connections go", WITHIN, || {
        rig.tun.sockets_in_use() == 0 && rig.tun.budget_in_use() == 0
    })
    .await;
    assert_eq!(rig.tun.counters(), Counters::default());
}
