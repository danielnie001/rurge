//! Tests of the fixtures everything else is tested with: the memory device,
//! the app-side stack, the fake `Dialer` and the loopback servers.

use crate::device::{AppEnd, Device, MemoryDevice};
use crate::testing::{AppStack, AppTcp, Direction, FakeDialer, Script, Upstream, pattern};
use rurge_config::{HostName, ListenerKind, SessionInfo};
use rurge_inbound::{DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
use rurge_proto::RejectKind;
use smoltcp::wire::IpCidr;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// What every wait in these tests is bounded by.
const WITHIN: Duration = Duration::from_secs(10);

async fn within<T>(future: impl Future<Output = T>) -> T {
    timeout(WITHIN, future).await.expect("timed out")
}

/// Polls `condition` every millisecond until it holds, for at most `limit`.
async fn eventually(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + limit;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

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

type Accepting<'a> = Pin<Box<dyn Future<Output = io::Result<AppTcp>> + Send + 'a>>;

/// `accept`, started: this returns once the listening socket exists, so a
/// SYN sent after it finds the listener.
async fn listening(stack: &AppStack, local: SocketAddr) -> Accepting<'_> {
    let mut accept: Accepting<'_> = Box::pin(stack.accept(local));
    std::future::poll_fn(|cx| {
        assert!(
            accept.as_mut().poll(cx).is_pending(),
            "nobody has connected yet"
        );
        Poll::Ready(())
    })
    .await;
    accept
}

/// A connection from `client` to `server`'s address `to`: both ends of it.
async fn connected(client: &AppStack, server: &AppStack, to: SocketAddr) -> (AppTcp, AppTcp) {
    let accepting = listening(server, to).await;
    let client = within(client.connect(to)).await.expect("connect");
    let server = within(accepting).await.expect("accept");
    (client, server)
}

const CHUNK: usize = 16 * 1024;

async fn write_pattern(to: &mut (impl AsyncWrite + Unpin), bytes: usize) -> io::Result<()> {
    let mut done = 0;
    while done < bytes {
        let n = (bytes - done).min(CHUNK);
        let chunk: Vec<u8> = (done..done + n).map(|i| pattern(i as u64)).collect();
        to.write_all(&chunk).await?;
        done += n;
    }
    Ok(())
}

async fn read_pattern(from: &mut (impl AsyncRead + Unpin), bytes: usize) -> io::Result<()> {
    let mut done = 0;
    let mut chunk = vec![0; CHUNK];
    while done < bytes {
        let n = (bytes - done).min(CHUNK);
        from.read_exact(&mut chunk[..n]).await?;
        for (k, byte) in chunk[..n].iter().enumerate() {
            assert_eq!(
                *byte,
                pattern((done + k) as u64),
                "byte {} is not the pattern's",
                done + k
            );
        }
        done += n;
    }
    Ok(())
}

/// The Internet checksum (RFC 1071) of `data`.
fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn v4(a: SocketAddr) -> (Ipv4Addr, u16) {
    match a {
        SocketAddr::V4(a) => (*a.ip(), a.port()),
        SocketAddr::V6(_) => panic!("an IPv4 address"),
    }
}

/// An IPv4 packet around `payload`, the header checksum right.
fn ipv4_packet(protocol: u8, src: Ipv4Addr, dst: Ipv4Addr, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x45, 0];
    packet.extend((20 + payload.len() as u16).to_be_bytes());
    packet.extend([0, 0, 0x40, 0, 64, protocol, 0, 0]);
    packet.extend(src.octets());
    packet.extend(dst.octets());
    let sum = checksum(&packet);
    packet[10..12].copy_from_slice(&sum.to_be_bytes());
    packet.extend(payload);
    packet
}

/// A UDP datagram over IPv4, without a checksum (which UDP over IPv4 allows).
fn udp_v4(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let ((src_ip, src_port), (dst_ip, dst_port)) = (v4(src), v4(dst));
    let mut udp = Vec::new();
    udp.extend(src_port.to_be_bytes());
    udp.extend(dst_port.to_be_bytes());
    udp.extend((8 + payload.len() as u16).to_be_bytes());
    udp.extend([0, 0]);
    udp.extend(payload);
    ipv4_packet(17, src_ip, dst_ip, &udp)
}

/// A TCP SYN over IPv4, the checksums right.
fn syn_v4(src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let ((src_ip, src_port), (dst_ip, dst_port)) = (v4(src), v4(dst));
    let mut tcp = Vec::new();
    tcp.extend(src_port.to_be_bytes());
    tcp.extend(dst_port.to_be_bytes());
    tcp.extend(1u32.to_be_bytes());
    tcp.extend(0u32.to_be_bytes());
    // a header of 20 bytes, SYN, a window of 65535, no checksum yet, no urgent pointer
    tcp.extend([0x50, 0x02, 0xff, 0xff, 0, 0, 0, 0]);
    let mut pseudo = Vec::new();
    pseudo.extend(src_ip.octets());
    pseudo.extend(dst_ip.octets());
    pseudo.extend([0, 6]);
    pseudo.extend((tcp.len() as u16).to_be_bytes());
    pseudo.extend(&tcp);
    let sum = checksum(&pseudo);
    tcp[16..18].copy_from_slice(&sum.to_be_bytes());
    ipv4_packet(6, src_ip, dst_ip, &tcp)
}

/// Where the TCP header of `packet` (IPv4 or IPv6, no extension headers)
/// starts, and where the packet ends; `None` for any other packet.
fn tcp_header(packet: &[u8]) -> Option<(usize, usize)> {
    let (protocol, header, total) = match packet.first().map(|b| b >> 4) {
        Some(4) => (
            packet[9],
            usize::from(packet[0] & 15) * 4,
            usize::from(u16::from_be_bytes([packet[2], packet[3]])),
        ),
        Some(6) => (
            packet[6],
            40,
            40 + usize::from(u16::from_be_bytes([packet[4], packet[5]])),
        ),
        _ => return None,
    };
    (protocol == 6).then_some((header, total))
}

/// The bytes of TCP payload `packet` carries; 0 for anything else.
fn tcp_payload_len(packet: &[u8]) -> usize {
    tcp_header(packet).map_or(0, |(header, total)| {
        total - header - usize::from(packet[header + 12] >> 4) * 4
    })
}

/// Whether `packet` is a TCP segment with the RST flag.
fn is_reset(packet: &[u8]) -> bool {
    tcp_header(packet).is_some_and(|(header, _)| packet[header + 13] & 0x04 != 0)
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
    let dialed = expect_dialed(within(dialer.dial(session(to))).await);
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
        within(server.read_exact(&mut word)).await.unwrap();
        assert_eq!(&word, b"hello");
        server.write_all(b"world").await.unwrap();
        within(client.read_exact(&mut word)).await.unwrap();
        assert_eq!(&word, b"world");

        // a megabyte each way at the same time, every byte the pattern's
        const MIB: usize = 1 << 20;
        let (mut client_read, mut client_write) = tokio::io::split(client);
        let (mut server_read, mut server_write) = tokio::io::split(server);
        within(async {
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
    within(client.read_to_end(&mut said)).await.unwrap();
    assert_eq!(said, b"last words");
    assert_eq!(
        within(client.read(&mut [0u8; 16])).await.unwrap(),
        0,
        "the end stays the end"
    );
    client.write_all(b"still heard").await.unwrap();
    let mut heard = [0u8; 11];
    within(server.read_exact(&mut heard)).await.unwrap();
    assert_eq!(&heard, b"still heard");
    let error = server.write_all(b"after a shutdown").await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);

    // a reset: both ways fail at once
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8081")).await;
    server.abort();
    let error = within(client.read(&mut [0u8; 16])).await.unwrap_err();
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
    let error = within(client.read(&mut [0u8; 16])).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);

    // one that was shut down ends cleanly instead
    let (mut client, mut server) = connected(&a, &b, addr("192.0.2.1:8081")).await;
    server.shutdown().await.unwrap();
    drop(server);
    assert_eq!(within(client.read(&mut [0u8; 16])).await.unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_syn_nobody_listens_for_is_refused() {
    let (a, _b) = stacks();
    for to in ["192.0.2.1:8080", "[2001:db8::1]:8080"] {
        let error = within(a.connect(addr(to))).await.unwrap_err();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ports_are_not_taken_twice() {
    let (a, b) = stacks();
    let to = addr("192.0.2.1:8080");
    let (first, _first_server) = connected(&a, &b, to).await;
    let accepting = listening(&b, to).await;
    let second = within(a.connect(to)).await.unwrap();
    assert_ne!(first.local().port(), second.local().port());
    drop(within(accepting).await.unwrap());

    // a port that is taken is taken, whoever it goes to
    let accepting = listening(&b, to).await;
    let held = within(a.connect_from(50000, to)).await.unwrap();
    assert_eq!(held.local().port(), 50000);
    drop(within(accepting).await.unwrap());
    let error = within(a.connect_from(50000, addr("192.0.2.1:8081")))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AddrInUse);

    // and free again once the connection is forgotten (nobody listens at the
    // new destination, which answers the attempt with a reset)
    drop(held);
    within(async {
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
        within(server.read_exact(&mut data)).await.unwrap();
        match round % 3 {
            // both ends reset
            0 => {
                client.abort();
                server.abort();
            }
            // both ends close, one after the other
            1 => {
                client.shutdown().await.unwrap();
                assert_eq!(within(server.read(&mut data)).await.unwrap(), 0);
                server.shutdown().await.unwrap();
                assert_eq!(within(client.read(&mut data)).await.unwrap(), 0);
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

    // listening sockets nobody connected to go as well
    let waiting = listening(&b, addr("192.0.2.1:9000")).await;
    assert_eq!(b.socket_count(), 1);
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
    assert_eq!(within(diverted.recv()).await, Some(datagram));
    assert_eq!(within(diverted.recv()).await, Some(syn));
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
    within(server.read_exact(&mut data)).await.unwrap();
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
    within(server.read_exact(&mut data)).await.unwrap();
    assert_eq!(&data, b"intact", "the damaged segment was not taken");
    assert_eq!(changed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_memory_device_fails_once_its_app_end_is_gone() {
    let (device, end) = MemoryDevice::pair(1500, 4);
    assert_eq!(device.mtu(), 1500);
    // what the app end had sent is still there to be received, then the end
    within(end.send(vec![9u8; 20])).await.unwrap();
    drop(end);
    let mut got = Vec::new();
    within(device.recv(&mut got, 8)).await.unwrap();
    assert_eq!(got, [vec![9u8; 20]]);
    got.clear();
    let error = within(device.recv(&mut got, 8)).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(got.is_empty());
    let error = within(device.send(&mut vec![vec![1u8; 20]]))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn an_app_end_sees_its_device_gone() {
    let (device, mut end) = MemoryDevice::pair(1500, 4);
    assert_eq!(end.mtu(), 1500);
    // what the device had written is still there to be received, then the end
    within(device.send(&mut vec![vec![7u8; 20], vec![8u8; 20]]))
        .await
        .unwrap();
    drop(device);
    assert_eq!(within(end.recv()).await, Some(vec![7u8; 20]));
    assert_eq!(end.try_recv(), Some(vec![8u8; 20]));
    assert_eq!(within(end.recv()).await, None);
    assert_eq!(end.try_recv(), None);
    let error = within(end.send(vec![1u8; 20])).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn a_memory_device_keeps_order_and_waits_for_room() {
    // at most `max` at a time, in the order they came
    let (device, end) = MemoryDevice::pair(1500, 8);
    for i in 0..5u8 {
        within(end.send(vec![i; 20])).await.unwrap();
    }
    let first = |packets: &[Vec<u8>]| packets.iter().map(|p| p[0]).collect::<Vec<_>>();
    let mut got = Vec::new();
    within(device.recv(&mut got, 3)).await.unwrap();
    assert_eq!(first(&got), [0, 1, 2]);
    got.clear();
    within(device.recv(&mut got, 3)).await.unwrap();
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
    assert_eq!(within(end.recv()).await, Some(vec![1u8; 20]));
    within(sending).await.unwrap();
    assert_eq!(within(end.recv()).await, Some(vec![2u8; 20]));
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
    within(app.read_exact(&mut pong)).await.unwrap();
    assert_eq!(&pong, b"ping");
    app.shutdown().await.unwrap();
    within(relay).await.unwrap();
    assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
    assert_eq!(dialer.relay_ends(), [Ok(())]);

    // rejected: the handle is finished before the answer
    let Some(DialError::Reject { kind, handle, .. }) =
        within(dialer.dial(session(to_reject))).await.err()
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
        }) = within(dialer.dial(session(to))).await.err()
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
    let (_far, near) = tokio::io::duplex(64);
    within(dialer.relay(Box::new(Broken), Box::new(near), handle.clone())).await;
    assert_eq!(dialer.relay_ends(), [Err("the app went away".to_string())]);
    assert_eq!(
        handle.outcome(),
        Some(SessionOutcome::Failed("the app went away".into()))
    );
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
    within(read_pattern(&mut app, 5000)).await.unwrap();
    write_pattern(&mut app, 3000).await.unwrap();
    eventually("the far end to read it", WITHIN, || {
        dialer.received(to) == 3000
    })
    .await;
    app.shutdown().await.unwrap();
    within(relay).await.unwrap();
    assert_eq!(dialer.relay_ends(), [Ok(())]);
    assert_eq!(dialer.received(to), 3000);
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
    within(relay).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_an_app_stack_fails_its_device() {
    // the device: it has lost its other end
    let (device, end) = MemoryDevice::pair(1500, 4);
    let alone = AppStack::start(end);
    alone.close();
    let error = within(device.recv(&mut Vec::new(), 8)).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    let error = within(device.send(&mut vec![vec![0u8; 20]]))
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
    let error = within(read).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    let error = client.write_all(b"x").await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    let error = within(a.connect(addr("192.0.2.1:8080"))).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    let error = within(a.accept(addr("198.18.0.1:8080"))).await.unwrap_err();
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
    within(stream.read_exact(&mut back)).await.unwrap();
    assert_eq!(&back, b"echo");
    stream.shutdown().await.unwrap();
    assert_eq!(within(stream.read(&mut back)).await.unwrap(), 0);

    // the sink counts
    let sink = Upstream::sink().await;
    let mut stream = TcpStream::connect(sink.addr()).await.unwrap();
    write_pattern(&mut stream, 100_000).await.unwrap();
    assert!(sink.wait_for(100_000, WITHIN).await);
    assert_eq!(sink.received(), 100_000);
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
    within(read_pattern(&mut stream, 300_000)).await.unwrap();
    let mut rest = Vec::new();
    within(stream.read_to_end(&mut rest)).await.unwrap();
    assert!(rest.is_empty());
    assert_eq!(source.sent(), 300_000);

    // dropping a server ends its connections and its listener
    let kept = Upstream::echo().await;
    let to = kept.addr();
    let mut stream = TcpStream::connect(to).await.unwrap();
    drop(kept);
    let end = within(stream.read(&mut back)).await;
    assert!(matches!(end, Ok(0) | Err(_)), "{end:?}");
    within(async {
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
