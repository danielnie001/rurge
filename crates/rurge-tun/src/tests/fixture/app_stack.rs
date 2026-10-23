//! The app-side stack: two of them carry a connection over a crossed pair, and
//! what a connection does as it ends, is reset, is dropped or is refused.

use super::{connected, stacks};
use crate::device::{Device, MemoryDevice};
use crate::testing::{AppStack, AppTcp, Direction};
use crate::tests::support::{
    WITHIN, addr, eventually, is_reset, is_syn, read_pattern, segment_v4, tcp_header,
    tcp_payload_len, within, write_pattern,
};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// What the engine takes a client side as, and what the relay of the fake `Dialer` takes.
#[test]
fn an_app_tcp_is_a_stream_for_the_engine() {
    fn is_a_stream<T: AsyncRead + AsyncWrite + Unpin + Send>() {}
    is_a_stream::<AppTcp>();
    let _boxed: fn(AppTcp) -> rurge_net::connector::BoxedStream = |tcp| Box::new(tcp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_app_stacks_carry_a_connection() {
    let (a, b) = stacks();
    for to in ["192.0.2.1:8080", "[2001:db8::1]:8080"] {
        let to = addr(to);
        let (client, server) = connected(&a, &b, to).await;
        let own = if to.is_ipv4() {
            "198.18.0.1"
        } else {
            "fd00:6152::1"
        };
        assert_eq!(client.local().ip(), own.parse::<IpAddr>().unwrap());
        assert!(client.local().port() >= 49152, "an ephemeral port");
        assert_eq!(server.local(), to);

        let (mut client, mut server) = (client, server);
        client.write_all(b"hello").await.unwrap();
        let mut word = [0u8; 5];
        within("hello to arrive", server.read_exact(&mut word))
            .await
            .unwrap();
        assert_eq!(&word, b"hello");
        server.write_all(b"world").await.unwrap();
        within("world to arrive", client.read_exact(&mut word))
            .await
            .unwrap();
        assert_eq!(&word, b"world");

        // a megabyte each way at the same time, every byte the pattern's
        const MIB: usize = 1 << 20;
        let (mut client_read, mut client_write) = tokio::io::split(client);
        let (mut server_read, mut server_write) = tokio::io::split(server);
        within("a megabyte each way", async {
            tokio::try_join!(
                write_pattern(&mut client_write, MIB),
                read_pattern(&mut server_read, MIB),
                write_pattern(&mut server_write, MIB),
                read_pattern(&mut client_read, MIB),
            )
        })
        .await
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fin_and_reset_are_told_apart() {
    let (a, b) = stacks();

    // a FIN: what was sent is read, then the end; the other direction is still open
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8080")).await;
    server.write_all(b"last words").await.unwrap();
    server.shutdown().await.unwrap();
    let mut said = Vec::new();
    within("the last words and the end", client.read_to_end(&mut said))
        .await
        .unwrap();
    assert_eq!(said, b"last words");
    assert_eq!(
        within("the end again", client.read(&mut [0u8; 16]))
            .await
            .unwrap(),
        0,
        "the end stays the end"
    );
    client.write_all(b"still heard").await.unwrap();
    let mut heard = [0u8; 11];
    within(
        "what was written after the end",
        server.read_exact(&mut heard),
    )
    .await
    .unwrap();
    assert_eq!(&heard, b"still heard");
    let error = server.write_all(b"after a shutdown").await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

    // a reset: both ways fail at once
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8081")).await;
    server.abort();
    let error = within("the reset to be read", client.read(&mut [0u8; 16]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    let error = client.write_all(b"too late").await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    // the end that aborted knows it did
    let error = server.read(&mut [0u8; 16]).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reset_discards_what_was_not_read() {
    let (a, b) = stacks();
    let data = Arc::new(AtomicBool::new(false));
    let reset = Arc::new(AtomicBool::new(false));
    {
        let (data, reset) = (data.clone(), reset.clone());
        a.set_filter(move |direction, packet| {
            if direction == Direction::FromTun {
                data.fetch_or(tcp_payload_len(packet) > 0, Ordering::SeqCst);
                reset.fetch_or(is_reset(packet), Ordering::SeqCst);
            }
            true
        });
    }
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8080")).await;
    server.write_all(b"unread").await.unwrap();
    eventually("the data to reach the client", WITHIN, || {
        data.load(Ordering::SeqCst)
    })
    .await;
    server.abort();
    eventually("the reset to reach the client", WITHIN, || {
        reset.load(Ordering::SeqCst)
    })
    .await;
    // the stack takes the reset in under its lock, which this waits for
    a.packets();
    let error = client.read(&mut [0u8; 16]).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_a_connection_that_was_not_shut_down_resets_it() {
    let (a, b) = stacks();
    let (mut client, server) = connected(&a, &b, addr("192.0.2.1:8080")).await;
    drop(server);
    let error = within("the reset to be read", client.read(&mut [0u8; 16]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);

    // one that was shut down ends cleanly instead
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8081")).await;
    server.shutdown().await.unwrap();
    drop(server);
    assert_eq!(
        within("the end to be read", client.read(&mut [0u8; 16]))
            .await
            .unwrap(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_syn_nobody_listens_for_is_refused() {
    let (a, _b) = stacks();
    for to in ["192.0.2.1:8080", "[2001:db8::1]:8080"] {
        let error = within("the refusal", a.connect(addr(to)))
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused);
    }
}

#[tokio::test(start_paused = true)]
async fn a_connection_nobody_answers_times_out_after_five_seconds() {
    // the device is kept, and nobody reads it
    let (_device, end) = MemoryDevice::pair(1500, 64);
    let stack = AppStack::start(end);
    let started = tokio::time::Instant::now();
    let error = stack.connect(addr("192.0.2.1:80")).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(5) && waited < Duration::from_secs(6),
        "{waited:?}"
    );
}

/// A SYN-ACK with a reset right behind it, the two in one send. On one
/// thread the stack's task takes both in before the task of the `connect`
/// is polled again, so the socket is closed by the time that task looks:
/// the connect has succeeded all the same, as on a real system, and the
/// reset shows on the first read.
#[tokio::test]
async fn a_reset_right_after_the_handshake_fails_the_first_read() {
    let (device, end) = MemoryDevice::pair(1500, 16);
    let stack = AppStack::start(end);
    let to = addr("192.0.2.1:80");
    let connecting = tokio::spawn({
        let stack = stack.clone();
        async move { stack.connect(to).await }
    });
    let syn = within("the app's SYN", async {
        loop {
            let mut got = Vec::new();
            device.recv(&mut got, 8).await.unwrap();
            if let Some(syn) = got.into_iter().find(|packet| is_syn(packet)) {
                break syn;
            }
        }
    })
    .await;
    let (header, _) = tcp_header(&syn).expect("a TCP segment");
    let from = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(syn[12], syn[13], syn[14], syn[15])),
        u16::from_be_bytes([syn[header], syn[header + 1]]),
    );
    let isn = u32::from_be_bytes(syn[header + 4..header + 8].try_into().unwrap());
    // SYN|ACK, then RST at the sequence number right after the SYN-ACK's
    let ours = 1000;
    let mut answer = vec![
        segment_v4(to, from, ours, isn.wrapping_add(1), 0x12),
        segment_v4(to, from, ours + 1, 0, 0x04),
    ];
    within("the answer to be taken", device.send(&mut answer))
        .await
        .unwrap();
    let mut tcp = within("the connect", connecting)
        .await
        .unwrap()
        .expect("the handshake was over before the reset came");
    let error = within("the first read", tcp.read(&mut [0u8; 16]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
}

/// The listening socket is there when `accept` returns, not when the future
/// it returns is first polled: a SYN that comes in between finds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_accept_listens_before_it_is_polled() {
    let (a, b) = stacks();
    let to = addr("192.0.2.1:8080");
    let accepting = b.accept(to);
    let mut client = within("the connection to be made", a.connect(to))
        .await
        .expect("the SYN found the listener");
    let mut server = within("the connection to be accepted", accepting)
        .await
        .expect("accept");
    client.write_all(b"hello").await.unwrap();
    let mut word = [0u8; 5];
    within("hello to arrive", server.read_exact(&mut word))
        .await
        .unwrap();
    assert_eq!(&word, b"hello");
    server.write_all(b"world").await.unwrap();
    within("world to arrive", client.read_exact(&mut word))
        .await
        .unwrap();
    assert_eq!(&word, b"world");
}

/// What later tests do: `accept` on a task of its own, and the connection
/// made right after, while that task may not have run yet.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawned_accept_is_ready_for_the_connection_made_right_after() {
    let (a, b) = stacks();
    for round in 0..50u16 {
        let to = addr(&format!("192.0.2.1:{}", 8000 + round));
        let accepting = tokio::spawn(b.accept(to));
        let client = within("the connection to be made", a.connect(to))
            .await
            .expect("the SYN found the listener");
        let server = within("the accept task", accepting)
            .await
            .unwrap()
            .expect("accept");
        assert_eq!(server.local(), to);
        drop((client, server));
    }
}

#[tokio::test]
async fn an_accept_that_cannot_listen_says_so_when_it_is_awaited() {
    let (_a, b) = stacks();
    // nothing can listen on port 0
    let accepting = b.accept(addr("192.0.2.1:0"));
    let error = within("the error", accepting).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(b.socket_count(), 0);
}

/// The side that closes first ends in TIME-WAIT, and the last segment of the
/// other side, which came within the 10 ms smoltcp holds an ACK back, is
/// still owed one when the connection is dropped: forgetting the socket at
/// once leaves the other side in LAST-ACK until its FIN is sent again and
/// answered with a reset. On a paused clock: everything before the drops
/// happens at one instant, the stack's timers fire when they are due and
/// not at the system's next tick, and the 1.5 s cost nothing.
#[tokio::test(start_paused = true)]
async fn a_dropped_connection_in_time_wait_still_sends_its_last_ack() {
    let (a, b) = stacks();
    let resets = Arc::new(AtomicUsize::new(0));
    {
        // a's filter sees everything that crosses, both ways
        let resets = resets.clone();
        a.set_filter(move |_, packet| {
            if is_reset(packet) {
                resets.fetch_add(1, Ordering::SeqCst);
            }
            true
        });
    }
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8080")).await;
    // a asks and closes its end; b reads to the end, answers at once (the
    // data, then its FIN, well inside 10 ms) and goes
    client.write_all(b"request").await.unwrap();
    client.shutdown().await.unwrap();
    let mut request = Vec::new();
    within("the request and the end", server.read_to_end(&mut request))
        .await
        .unwrap();
    assert_eq!(request, b"request");
    server.write_all(b"response").await.unwrap();
    server.shutdown().await.unwrap();
    drop(server);
    // a reads the answer and the end, and goes
    let mut response = Vec::new();
    within(
        "the response and the end",
        client.read_to_end(&mut response),
    )
    .await
    .unwrap();
    assert_eq!(response, b"response");
    drop(client);

    // b's FIN is acknowledged, so b is done, and a's socket goes after its grace
    let soon = Duration::from_millis(300);
    eventually("b to be done with the connection", soon, || {
        b.socket_count() == 0
    })
    .await;
    eventually("a to forget the connection", soon, || a.socket_count() == 0).await;
    // 1.5 s on, long enough for b's FIN to be sent again, had it not been
    // acknowledged (a retransmission timeout is 0.8 s after the first
    // samples), and answered with a reset
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(resets.load(Ordering::SeqCst), 0, "no reset crossed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ports_are_not_taken_twice() {
    let (a, b) = stacks();
    let to = addr("192.0.2.1:8080");
    let (first, _first_server) = connected(&a, &b, to).await;
    let accepting = b.accept(to);
    let second = within("the second connection", a.connect(to))
        .await
        .unwrap();
    assert_ne!(first.local().port(), second.local().port());
    drop(
        within("the second connection to be accepted", accepting)
            .await
            .unwrap(),
    );

    // a port that is taken is taken, whoever it goes to
    let accepting = b.accept(to);
    let held = within(
        "the connection from a chosen port",
        a.connect_from(50000, to),
    )
    .await
    .unwrap();
    assert_eq!(held.local().port(), 50000);
    drop(
        within("that connection to be accepted", accepting)
            .await
            .unwrap(),
    );
    let error = within(
        "the refusal of a taken port",
        a.connect_from(50000, addr("192.0.2.1:8081")),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AddrInUse);

    // and free again once the connection is forgotten (nobody listens at the
    // new destination, which answers the attempt with a reset)
    drop(held);
    within("the port to be free again", async {
        loop {
            match a.connect_from(50000, addr("192.0.2.1:8082")).await {
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(Duration::from_millis(1)).await
                }
                Err(error) => break assert_eq!(error.kind(), io::ErrorKind::ConnectionRefused),
                Ok(_) => panic!("nobody listens there"),
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finished_connections_leave_no_sockets_behind() {
    let (a, b) = stacks();
    for round in 0..18u16 {
        let to = addr(&format!("192.0.2.1:{}", 8000 + round));
        let (mut client, mut server) = connected(&a, &b, to).await;
        client.write_all(b"data").await.unwrap();
        let mut data = [0u8; 4];
        within("the data to arrive", server.read_exact(&mut data))
            .await
            .unwrap();
        match round % 3 {
            // both ends reset
            0 => {
                client.abort();
                server.abort();
            }
            // both ends close, one after the other
            1 => {
                client.shutdown().await.unwrap();
                assert_eq!(
                    within("the client's end", server.read(&mut data))
                        .await
                        .unwrap(),
                    0
                );
                server.shutdown().await.unwrap();
                assert_eq!(
                    within("the server's end", client.read(&mut data))
                        .await
                        .unwrap(),
                    0
                );
            }
            // one end goes away without a word
            _ => drop(server),
        }
        drop(client);
    }
    eventually("both stacks to forget every socket", WITHIN, || {
        a.socket_count() == 0 && b.socket_count() == 0
    })
    .await;

    // a listening socket nobody connects to is there from the call on, and
    // goes with its future
    let waiting = b.accept(addr("192.0.2.1:9000"));
    assert_eq!(b.socket_count(), 1, "listening before the future is polled");
    drop(waiting);
    eventually("the listener to be forgotten", WITHIN, || {
        b.socket_count() == 0
    })
    .await;
}
