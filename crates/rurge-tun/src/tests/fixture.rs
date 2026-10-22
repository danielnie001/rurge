//! Tests of the fixtures everything else is tested with: the memory device,
//! the app-side stack, the fake `Dialer` and the loopback servers.

use super::support::{
    WITHIN, addr, eventually, is_reset, is_syn, read_pattern, segment_v4, syn_v4, tcp_header,
    tcp_payload_len, udp_v4, within, write_pattern,
};
use crate::device::{AppEnd, Device, MemoryDevice};
use crate::testing::{
    AppStack, AppTcp, Direction, FakeDialer, FakeHooks, FakePinger, FakeResolve, PingScript,
    Script, Upstream,
};
use crate::{Hooks, Pinger, TunSettings};
use rurge_config::{HostName, ListenerKind, SessionInfo};
use rurge_inbound::{DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
use rurge_net::connector::Resolve;
use rurge_proto::RejectKind;
use smoltcp::wire::IpCidr;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout;

fn cidr(ip: &str, prefix: u8) -> IpCidr {
    IpCidr::new(ip.parse::<IpAddr>().unwrap().into(), prefix)
}

/// Two stacks on the two ends of a crossed pair: the first with the
/// adapter's own addresses (what the operating system has), the second with
/// 192.0.2.1 and 2001:db8::1 (somewhere to connect to).
fn stacks() -> (AppStack, AppStack) {
    let (a, b) = AppEnd::crossed(1500, 4096);
    let documentation = [cidr("192.0.2.1", 32), cidr("2001:db8::1", 128)];
    (AppStack::start(a), AppStack::start_with(b, &documentation))
}

/// A connection from `client` to `server`'s address `to`: both ends of it.
async fn connected(client: &AppStack, server: &AppStack, to: SocketAddr) -> (AppTcp, AppTcp) {
    let accepting = server.accept(to);
    let client = within("the connection to be made", client.connect(to))
        .await
        .expect("connect");
    let server = within("the connection to be accepted", accepting)
        .await
        .expect("accept");
    (client, server)
}

/// The session of an app connecting to `dst` through the adapter.
fn session(dst: SocketAddr) -> SessionInfo {
    SessionInfo {
        src: addr("198.18.0.1:49152"),
        listener: ListenerKind::Tun,
        ..SessionInfo::tcp(HostName::Ip(dst.ip()), dst.port())
    }
}

fn expect_dialed(result: Result<Dialed, DialError>) -> Dialed {
    match result {
        Ok(dialed) => dialed,
        Err(DialError::Reject { kind, .. }) => panic!("rejected: {kind:?}"),
        Err(DialError::Failed { message, .. }) => panic!("failed: {message}"),
    }
}

/// A session dialed and relayed in a task of its own, the client side of the
/// relay being one end of a pipe: the other end is returned.
async fn relayed(
    dialer: &Arc<FakeDialer>,
    to: SocketAddr,
) -> (
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<()>,
    Arc<SessionHandle>,
) {
    let dialed = expect_dialed(within("the session to be dialed", dialer.dial(session(to))).await);
    let (app, tun_side) = tokio::io::duplex(64 * 1024);
    let handle = dialed.handle.clone();
    let relay = tokio::spawn({
        let dialer = dialer.clone();
        async move {
            dialer
                .relay(Box::new(tun_side), dialed.stream, dialed.handle)
                .await
        }
    });
    (app, relay, handle)
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn divert_and_inject_go_around_the_stack() {
    let (a, b) = stacks();
    let mut diverted = b.divert(9000..=9009);
    let from = addr("198.18.0.1:5000");
    let datagram = udp_v4(from, addr("192.0.2.1:9004"), b"datagram");
    let syn = syn_v4(from, addr("192.0.2.1:9009"));
    a.inject(datagram.clone());
    a.inject(syn.clone());
    assert_eq!(
        within("the datagram to be diverted", diverted.recv()).await,
        Some(datagram)
    );
    assert_eq!(
        within("the SYN to be diverted", diverted.recv()).await,
        Some(syn)
    );
    // neither reached the stack of b, which would have answered the SYN with
    // a reset; and a's own stack was not what sent them
    assert_eq!(b.packets(), (0, 0));
    assert_eq!(a.packets(), (0, 0));

    // outside the range the same SYN is the stack's: it takes it in and
    // answers it
    a.inject(syn_v4(from, addr("192.0.2.1:9010")));
    eventually("b's stack to answer a SYN to a closed port", WITHIN, || {
        b.packets() == (1, 1)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filter_sees_both_ways_and_can_drop() {
    let (a, b) = stacks();
    let seen = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let dropped = Arc::new(AtomicUsize::new(0));
    {
        let (seen, dropped) = (seen.clone(), dropped.clone());
        a.set_filter(move |direction, packet| {
            seen[usize::from(direction == Direction::FromTun)].fetch_add(1, Ordering::SeqCst);
            // the first segment going out that carries data is lost
            let first_data = direction == Direction::ToTun
                && tcp_payload_len(packet) > 0
                && dropped
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok();
            !first_data
        });
    }
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8080")).await;
    client.write_all(b"sent twice").await.unwrap();
    let mut data = [0u8; 10];
    within(
        "the data to arrive after the loss",
        server.read_exact(&mut data),
    )
    .await
    .unwrap();
    assert_eq!(
        &data, b"sent twice",
        "it arrived all the same: it was sent again"
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    assert!(seen[0].load(Ordering::SeqCst) > 0, "packets to the TUN");
    assert!(seen[1].load(Ordering::SeqCst) > 0, "packets from the TUN");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_filter_can_change_a_packet() {
    let (a, b) = stacks();
    let changed = Arc::new(AtomicUsize::new(0));
    {
        let changed = changed.clone();
        a.set_filter(move |direction, packet| {
            if direction == Direction::ToTun
                && tcp_payload_len(packet) > 0
                && changed
                    .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                // the last byte of the payload: the checksum is no longer right
                *packet.last_mut().unwrap() ^= 0xff;
            }
            true
        });
    }
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8080")).await;
    client.write_all(b"intact").await.unwrap();
    let mut data = [0u8; 6];
    within(
        "the data to arrive after the damage",
        server.read_exact(&mut data),
    )
    .await
    .unwrap();
    assert_eq!(&data, b"intact", "the damaged segment was not taken");
    assert_eq!(changed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_memory_device_fails_once_its_app_end_is_gone() {
    let (device, end) = MemoryDevice::pair(1500, 4);
    assert_eq!(device.mtu(), 1500);
    // what the app end had sent is still there to be received, then the end
    within("the packet to be sent", end.send(vec![9u8; 20]))
        .await
        .unwrap();
    drop(end);
    let mut got = Vec::new();
    within("the packet", device.recv(&mut got, 8))
        .await
        .unwrap();
    assert_eq!(got, [vec![9u8; 20]]);
    got.clear();
    let error = within("the end", device.recv(&mut got, 8))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(got.is_empty());
    let error = within("the send to fail", device.send(&mut vec![vec![1u8; 20]]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn an_app_end_sees_its_device_gone() {
    let (device, mut end) = MemoryDevice::pair(1500, 4);
    assert_eq!(end.mtu(), 1500);
    // what the device had written is still there to be received, then the end
    within(
        "the packets to be sent",
        device.send(&mut vec![vec![7u8; 20], vec![8u8; 20]]),
    )
    .await
    .unwrap();
    drop(device);
    assert_eq!(
        within("the first packet", end.recv()).await,
        Some(vec![7u8; 20])
    );
    assert_eq!(end.try_recv(), Some(vec![8u8; 20]));
    assert_eq!(within("the end", end.recv()).await, None);
    assert_eq!(end.try_recv(), None);
    let error = within("the send to fail", end.send(vec![1u8; 20]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn a_memory_device_keeps_order_and_waits_for_room() {
    // at most `max` at a time, in the order they came
    let (device, end) = MemoryDevice::pair(1500, 8);
    for i in 0..5u8 {
        within("the packet to be sent", end.send(vec![i; 20]))
            .await
            .unwrap();
    }
    let first = |packets: &[Vec<u8>]| packets.iter().map(|p| p[0]).collect::<Vec<_>>();
    let mut got = Vec::new();
    within("the first three", device.recv(&mut got, 3))
        .await
        .unwrap();
    assert_eq!(first(&got), [0, 1, 2]);
    got.clear();
    within("the rest", device.recv(&mut got, 3)).await.unwrap();
    assert_eq!(first(&got), [3, 4]);

    // with room for one packet, the second waits for the app end to take the first
    let (device, mut end) = MemoryDevice::pair(1500, 1);
    let mut packets = vec![vec![1u8; 20], vec![2u8; 20]];
    let mut sending = device.send(&mut packets);
    // waits for: the second packet to be written although there is no room;
    // margin: a send that finds room completes in microseconds
    assert!(
        timeout(Duration::from_millis(50), &mut sending)
            .await
            .is_err(),
        "the second packet must wait"
    );
    assert_eq!(
        within("the first packet", end.recv()).await,
        Some(vec![1u8; 20])
    );
    within("the send to finish", sending).await.unwrap();
    assert_eq!(
        within("the second packet", end.recv()).await,
        Some(vec![2u8; 20])
    );
    assert!(packets.is_empty(), "a send leaves its packets empty");
}

#[tokio::test]
async fn the_fake_dialer_answers_what_it_was_told() {
    let echo = Upstream::echo().await;
    let dialer = FakeDialer::new();
    let to_echo = addr("192.0.2.10:80");
    let to_reject = addr("192.0.2.11:80");
    let to_fail = addr("192.0.2.12:80");
    let unscripted = addr("192.0.2.13:80");
    dialer.script(to_echo, Script::Upstream(echo.addr()));
    dialer.script(to_reject, Script::Reject(RejectKind::TinyGif));
    dialer.script(to_fail, Script::Fail);

    // dialed, and relayed to the end
    let (mut app, relay, handle) = relayed(&dialer, to_echo).await;
    assert_eq!(handle.id(), 1);
    app.write_all(b"ping").await.unwrap();
    let mut pong = [0u8; 4];
    within("the echo", app.read_exact(&mut pong)).await.unwrap();
    assert_eq!(&pong, b"ping");
    app.shutdown().await.unwrap();
    within("the relay to end", relay).await.unwrap();
    assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
    assert_eq!(dialer.relay_ends(), [Ok(())]);

    // rejected: the handle is finished before the answer
    let Some(DialError::Reject { kind, handle, .. }) =
        within("the rejection", dialer.dial(session(to_reject)))
            .await
            .err()
    else {
        panic!("not a rejection");
    };
    assert_eq!(kind, RejectKind::TinyGif);
    assert_eq!(handle.id(), 2);
    assert_eq!(
        handle.outcome(),
        Some(SessionOutcome::Rejected(RejectKind::TinyGif))
    );

    // a scripted failure and a destination nobody wrote anything for
    for (to, text, id) in [
        (to_fail, "scripted failure", 3),
        (unscripted, "not scripted", 4),
    ] {
        let Some(DialError::Failed {
            kind,
            message,
            handle,
            ..
        }) = within("the failure", dialer.dial(session(to))).await.err()
        else {
            panic!("not a failure");
        };
        assert_eq!(kind, FailKind::Connect);
        assert_eq!(message, text);
        assert_eq!(handle.id(), id);
        assert_eq!(handle.outcome(), Some(SessionOutcome::Failed(text.into())));
    }

    // every session, in order
    let dialed: Vec<(IpAddr, u16)> = dialer
        .dialed()
        .into_iter()
        .map(|s| match s.dst_host {
            HostName::Ip(ip) => (ip, s.dst_port),
            HostName::Domain(name) => panic!("a name: {name}"),
        })
        .collect();
    let expected: Vec<_> = [to_echo, to_reject, to_fail, unscripted]
        .iter()
        .map(|a| (a.ip(), a.port()))
        .collect();
    assert_eq!(dialed, expected);
}

/// A stream whose reads fail.
struct Broken;

impl AsyncRead for Broken {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "the app went away",
        )))
    }
}

impl AsyncWrite for Broken {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_failed_relay_is_recorded_with_its_text() {
    let dialer = FakeDialer::new();
    let handle = SessionHandle::new(77, session(addr("192.0.2.14:80")));
    // what `relay_ends` holds when the handle is told it is over
    let ends_then = Arc::new(AtomicUsize::new(usize::MAX));
    handle.on_finish({
        let (dialer, ends_then) = (dialer.clone(), ends_then.clone());
        move |_, _| ends_then.store(dialer.relay_ends().len(), Ordering::SeqCst)
    });
    let (_far, near) = tokio::io::duplex(64);
    within(
        "the relay to end",
        dialer.relay(Box::new(Broken), Box::new(near), handle.clone()),
    )
    .await;
    assert_eq!(dialer.relay_ends(), [Err("the app went away".to_string())]);
    assert_eq!(
        handle.outcome(),
        Some(SessionOutcome::Failed("the app went away".into()))
    );
    assert_eq!(
        ends_then.load(Ordering::SeqCst),
        1,
        "the end is recorded before the handle finishes"
    );
}

/// The fake dialer's relay ends when the session's token is cancelled, as the
/// engine's does, with an app that says nothing and an upstream that is held:
/// it is recorded as cancelled, and the handle finishes completed.
#[tokio::test]
async fn a_cancelled_relay_ends_and_is_recorded_as_cancelled() {
    let dialer = FakeDialer::new();
    let to = addr("192.0.2.17:443");
    dialer.script(to, Script::Hold { buffer: 1024 });
    let (_app, relay, handle) = relayed(&dialer, to).await;
    handle.token().cancel();
    within("the relay to end", relay).await.unwrap();
    assert_eq!(dialer.relay_ends(), [Err("cancelled".to_string())]);
    assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
}

#[tokio::test]
async fn a_memory_upstream_sends_what_it_was_told_and_counts_what_it_gets() {
    let dialer = FakeDialer::new();
    let to = addr("192.0.2.15:80");
    dialer.script(
        to,
        Script::Memory {
            buffer: 1024,
            send: 5000,
        },
    );
    let (mut app, relay, _handle) = relayed(&dialer, to).await;
    within("what the far end sends", read_pattern(&mut app, 5000))
        .await
        .unwrap();
    write_pattern(&mut app, 3000).await.unwrap();
    eventually("the far end to read it", WITHIN, || {
        dialer.received(to) == 3000
    })
    .await;
    app.shutdown().await.unwrap();
    within("the relay to end", relay).await.unwrap();
    assert_eq!(dialer.relay_ends(), [Ok(())]);
    assert_eq!(dialer.received(to), 3000);
}

/// A wait for what the far ends have read is woken by the reading, and ends
/// when its time is up.
#[tokio::test]
async fn waiting_for_what_a_far_end_has_read_is_woken_and_bounded() {
    let dialer = FakeDialer::new();
    let to = addr("192.0.2.16:80");
    dialer.script(
        to,
        Script::Memory {
            buffer: 1024,
            send: 0,
        },
    );
    let (mut app, relay, _handle) = relayed(&dialer, to).await;
    // begun before the far end reads the bytes, so only the reading can end it
    let waiting = tokio::spawn({
        let dialer = dialer.clone();
        async move { dialer.wait_for(to, 3000, WITHIN).await }
    });
    write_pattern(&mut app, 3000).await.unwrap();
    assert!(
        within("the wait to end", waiting).await.unwrap(),
        "woken by the bytes"
    );
    // waits for: the time to run out, as a 3001st byte never comes; margin:
    // none needed, whenever it ends the answer is false
    assert!(!dialer.wait_for(to, 3001, Duration::from_millis(50)).await);
    // and an answer for what is already there does not wait
    assert!(dialer.wait_for(to, 3000, Duration::ZERO).await);
    app.shutdown().await.unwrap();
    within("the relay to end", relay).await.unwrap();
}

#[tokio::test]
async fn a_held_upstream_reads_only_once_released() {
    let dialer = FakeDialer::new();
    let to = addr("192.0.2.20:443");
    dialer.script(to, Script::Hold { buffer: 1024 });
    let (mut app, relay, _handle) = relayed(&dialer, to).await;
    app.write_all(&[0xaa; 4096]).await.unwrap();
    // waits for: the far end to read something before it was released;
    // margin: a far end that reads takes the first 1024 bytes within microseconds
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(dialer.received(to), 0);
    dialer.release(to);
    eventually(
        "the far end to read all of it",
        Duration::from_secs(5),
        || dialer.received(to) == 4096,
    )
    .await;
    app.shutdown().await.unwrap();
    within("the relay to end", relay).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_an_app_stack_fails_its_device() {
    // the device: it has lost its other end
    let (device, end) = MemoryDevice::pair(1500, 4);
    let alone = AppStack::start(end);
    alone.close();
    let error = within("the device to fail", device.recv(&mut Vec::new(), 8))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    let error = within("the device to fail", device.send(&mut vec![vec![0u8; 20]]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

    // the connections: a read that waits is woken, the others fail at once, and none can start
    let (a, b) = stacks();
    let (mut client, _server) = connected(&a, &b, addr("192.0.2.1:8080")).await;
    let mut buf = [0u8; 16];
    let mut read = Box::pin(client.read(&mut buf));
    std::future::poll_fn(|cx| {
        assert!(read.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    a.close();
    let error = within("the waiting read to fail", read).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    let error = client.write_all(b"x").await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    let error = within("the connect to fail", a.connect(addr("192.0.2.1:8080")))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    let error = within("the accept to fail", a.accept(addr("198.18.0.1:8080")))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
}

#[tokio::test]
#[should_panic(expected = "loopback")]
async fn the_fake_dialer_dials_only_the_loopback() {
    FakeDialer::new().script(addr("192.0.2.9:80"), Script::Upstream(addr("192.0.2.1:80")));
}

#[tokio::test]
async fn the_upstream_servers_do_what_they_say() {
    // the echo sends back what it gets, and closes when the client has
    let echo = Upstream::echo().await;
    let mut stream = TcpStream::connect(echo.addr()).await.unwrap();
    stream.write_all(b"echo").await.unwrap();
    let mut back = [0u8; 4];
    within("the echo", stream.read_exact(&mut back))
        .await
        .unwrap();
    assert_eq!(&back, b"echo");
    stream.shutdown().await.unwrap();
    assert_eq!(
        within("the echo's end", stream.read(&mut back))
            .await
            .unwrap(),
        0
    );

    // the sink counts
    let sink = Upstream::sink().await;
    let mut stream = TcpStream::connect(sink.addr()).await.unwrap();
    within(
        "the pattern to be written",
        write_pattern(&mut stream, 100_000),
    )
    .await
    .unwrap();
    assert!(sink.wait_for(100_000, WITHIN).await);
    assert_eq!(sink.received(), 100_000);
    // waits for: `wait_for` to give up after 1 ms; margin: nothing can come
    // to a sink nobody is connected to, so there is no race to lose
    assert!(
        !Upstream::sink()
            .await
            .wait_for(1, Duration::from_millis(1))
            .await,
        "nothing came, and the wait is over"
    );

    // the source writes the pattern and then closes
    let source = Upstream::source(300_000).await;
    let mut stream = TcpStream::connect(source.addr()).await.unwrap();
    within("the pattern", read_pattern(&mut stream, 300_000))
        .await
        .unwrap();
    let mut rest = Vec::new();
    within("the end", stream.read_to_end(&mut rest))
        .await
        .unwrap();
    assert!(rest.is_empty());
    assert_eq!(source.sent(), 300_000);

    // dropping a server ends its connections and its listener
    let kept = Upstream::echo().await;
    let to = kept.addr();
    let mut stream = TcpStream::connect(to).await.unwrap();
    drop(kept);
    let end = within("the connection to end", stream.read(&mut back)).await;
    assert!(matches!(end, Ok(0) | Err(_)), "{end:?}");
    within("the listener to go", async {
        while TcpStream::connect(to).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_filter_replaces_the_old_one() {
    let (a, b) = stacks();
    let (first, second) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    for counter in [first.clone(), second.clone()] {
        a.set_filter(move |_, _| {
            counter.fetch_add(1, Ordering::SeqCst);
            true
        });
    }
    let _connection = connected(&a, &b, addr("192.0.2.1:8080")).await;
    assert_eq!(
        first.load(Ordering::SeqCst),
        0,
        "the first filter was replaced"
    );
    assert!(second.load(Ordering::SeqCst) > 0);
}

/// A session is dialed to where the app connected to: its `local` end when it
/// has one, which for a connection to a fake address is that address and not
/// the name the session is for; the address it names when it has none.
#[tokio::test]
async fn the_fake_dialer_finds_a_session_by_where_the_app_connected_to() {
    let echo = Upstream::echo().await;
    let dialer = FakeDialer::new();
    let (fake, elsewhere) = (addr("198.18.1.1:80"), addr("192.0.2.9:80"));
    dialer.script(fake, Script::Upstream(echo.addr()));
    dialer.script(elsewhere, Script::Fail);
    // the session of a name behind a fake address
    let mut named = session(fake);
    named.dst_host = HostName::Domain("a.test".to_owned());
    named.local = Some(fake);
    expect_dialed(within("the dial", dialer.dial(named)).await);
    // `local` comes first: the address `dst_host` names is another one
    let mut moved = session(elsewhere);
    moved.local = Some(fake);
    expect_dialed(within("the dial", dialer.dial(moved)).await);
    // with none the address it names is the one
    let result = within("the dial", dialer.dial(session(elsewhere))).await;
    assert!(matches!(result, Err(DialError::Failed { .. })));
    // and a name with no `local` has nothing to be found by
    let mut nameless = session(fake);
    nameless.dst_host = HostName::Domain("a.test".to_owned());
    let result = within("the dial", dialer.dial(nameless)).await;
    let Err(DialError::Failed { message, .. }) = result else {
        panic!("a session with nothing to find it by is not dialed");
    };
    assert_eq!(message, "not scripted");
}

/// The settings a TUN asks for are those set last; one handed out before keeps
/// what it was.
#[test]
fn fake_hooks_give_the_settings_that_were_set_last() {
    let hooks = FakeHooks::new(TunSettings {
        resolver: FakeResolve::new(),
        icmp_forwarding: true,
    });
    let before = hooks.settings();
    assert!(before.icmp_forwarding);
    hooks.set(TunSettings {
        resolver: FakeResolve::new(),
        icmp_forwarding: false,
    });
    assert!(!hooks.settings().icmp_forwarding);
    assert!(before.icmp_forwarding, "the settings handed out before");
}

#[tokio::test]
async fn the_fake_resolver_knows_what_it_was_told() {
    let resolver = FakeResolve::new();
    let (v4, v6) = (
        IpAddr::from(Ipv4Addr::new(127, 0, 0, 1)),
        IpAddr::from(Ipv6Addr::LOCALHOST),
    );
    resolver.insert("a.test", vec![v6, v4]);
    // in the order it was given
    let found = within("the answer", resolver.resolve("a.test")).await;
    assert_eq!(found.expect("a name that was inserted"), [v6, v4]);
    // a name it was not told is not found
    let error = within("the answer", resolver.resolve("b.test"))
        .await
        .expect_err("a name nobody inserted");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);
    // a name inserted again has what it was given last
    resolver.insert("a.test", vec![v4]);
    let found = within("the answer", resolver.resolve("a.test")).await;
    assert_eq!(found.expect("a name that was inserted"), [v4]);
}

#[tokio::test]
async fn the_fake_pinger_does_what_it_was_scripted_and_writes_every_request_down() {
    let pinger = FakePinger::new();
    let [echoed, failed, silent, unscripted] =
        [1u8, 2, 3, 4].map(|last| IpAddr::from(Ipv4Addr::new(192, 0, 2, last)));
    pinger.script(echoed, PingScript::Echo);
    pinger.script(failed, PingScript::Fail);
    pinger.script(silent, PingScript::Silent);
    let reply = within("the reply", pinger.echo(echoed, b"hello")).await;
    assert_eq!(reply.expect("an echo"), b"hello");
    assert!(
        within("the failure", pinger.echo(failed, b"x"))
            .await
            .is_err()
    );
    assert!(
        within("the failure", pinger.echo(unscripted, b"y"))
            .await
            .is_err(),
        "a destination nothing was scripted for fails"
    );
    // the one fixed wait of this test, for what must not happen: an answer
    // from a pinger that is scripted to say nothing. It would come at once if
    // it came at all, so 100 ms is a wide margin
    assert!(
        timeout(Duration::from_millis(100), pinger.echo(silent, b"z"))
            .await
            .is_err()
    );
    assert_eq!(
        pinger.pinged(),
        [
            (echoed, b"hello".to_vec()),
            (failed, b"x".to_vec()),
            (unscripted, b"y".to_vec()),
            (silent, b"z".to_vec()),
        ]
    );
}
