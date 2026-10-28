//! What the tests of this crate share: the TUN they start, waits that are
//! bounded, streams of the pattern, and the builders and readers of the
//! packets they make by hand.

use crate::device::{AppEnd, Device, MemoryDevice};
use crate::dispatch::Ingress;
use crate::stats::Stats;
use crate::tcp::{Admission, Budget, Shard};
use crate::testing::{
    AppStack, AppTcp, FakeDialer, FakeDns, FakeHooks, FakePinger, FakeResolve, pattern,
};
use crate::{Life, Tun, TunHandle, TunOptions, TunSettings};
use rurge_config::HostList;
use rurge_dns::fakeip::FakeIpPool;
use rurge_inbound::Dialer;
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    Icmpv4Packet, Icmpv4Repr, Icmpv6Packet, Icmpv6Repr, TcpControl, TcpPacket, TcpRepr,
    TcpSeqNumber, UdpPacket, UdpRepr,
};
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

/// A TUN on one end of a memory device, the app's stack on the other, and
/// the fake `Dialer` the TUN hands its sessions to.
pub(crate) struct Rig {
    pub(crate) tun: TunHandle,
    pub(crate) app: AppStack,
    pub(crate) dialer: Arc<FakeDialer>,
}

/// What a TUN is started with besides its device, its dialer and its options,
/// and what a test may want to reach once it is started: the settings of the
/// generation in use, the pinger and the pool of fake addresses.
pub(crate) struct Extras {
    pub(crate) hooks: Arc<FakeHooks>,
    pub(crate) pinger: Arc<FakePinger>,
    pub(crate) pool: Option<Arc<FakeIpPool>>,
    /// The `FakeDns` the settings in `hooks` started with, for a test to teach
    /// names and script replies. Settings a test sets later carry the DNS it
    /// gives them.
    pub(crate) dns: Arc<FakeDns>,
}

impl Default for Extras {
    /// Settings with a resolver that knows no name and `icmp-forwarding` on, a
    /// `FakeDns` that maps no name and forwards everything as it came, no
    /// `always-real-ip`, `allow-dns-svcb` off and no `hijack-dns`; a pinger
    /// that has been scripted nothing (every echo request fails), and a pool
    /// with no name in it.
    fn default() -> Extras {
        let dns = FakeDns::new();
        Extras {
            hooks: FakeHooks::new(TunSettings {
                dns: dns.clone(),
                ..settings(FakeResolve::new(), true)
            }),
            pinger: FakePinger::new(),
            pool: Some(Arc::new(FakeIpPool::new())),
            dns,
        }
    }
}

/// Settings with this `resolver` and `icmp-forwarding`, and the DNS settings
/// of `Extras::default()` (a `FakeDns` of their own): for a test that is about
/// the echo requests and writes nothing else of the settings.
pub(crate) fn settings(resolver: Arc<dyn Resolve>, icmp_forwarding: bool) -> TunSettings {
    TunSettings {
        resolver,
        icmp_forwarding,
        dns: FakeDns::new(),
        always_real_ip: HostList::empty(),
        allow_dns_svcb: false,
        hijack_dns: Vec::new(),
        ip_rewrite: Arc::default(),
        udp_priority: true,
    }
}

/// A rig on a memory device with an MTU of 1500 and room for 4096 packets
/// each way.
pub(crate) fn rig(options: TunOptions) -> Rig {
    rig_on(MemoryDevice::pair(1500, 4096), options)
}

/// The same with `extras` in place of the default ones, which are given back
/// for the test to reach what the TUN was started with.
pub(crate) fn rig_extras(options: TunOptions, extras: Extras) -> (Rig, Extras) {
    let rig = rig_on_extras(MemoryDevice::pair(1500, 4096), options, &extras);
    (rig, extras)
}

/// A rig on `pair`, a memory device and the app's end of it. The TUN is
/// started by `tun_dialing`, the one place that calls `Tun::start`: a change of
/// what `Tun::start` takes is a change of that function only.
pub(crate) fn rig_on(pair: (MemoryDevice, AppEnd), options: TunOptions) -> Rig {
    rig_on_extras(pair, options, &Extras::default())
}

/// `rig_on` with `extras` in place of the default ones: the wiring of
/// `rig_on` and of `rig_extras`.
fn rig_on_extras(
    (device, end): (MemoryDevice, AppEnd),
    options: TunOptions,
    extras: &Extras,
) -> Rig {
    let dialer = FakeDialer::new();
    let tun = tun_dialing(Arc::new(device), dialer.clone(), extras, options);
    Rig {
        tun,
        app: AppStack::start(end),
        dialer,
    }
}

/// A rig on a device of the test's own, with the app's stack on `end`, the app
/// end of the memory device that device wraps.
pub(crate) fn rig_with(device: Arc<dyn Device>, end: AppEnd, options: TunOptions) -> Rig {
    let (tun, dialer) = tun_with(device, options);
    Rig {
        tun,
        app: AppStack::start(end),
        dialer,
    }
}

/// A TUN on `device` with the default `Extras`, started on the current
/// runtime, and the dialer it was started with: no app stack comes with it, for
/// a test that runs the app's stack on another runtime.
pub(crate) fn tun_on(device: MemoryDevice, options: TunOptions) -> (TunHandle, Arc<FakeDialer>) {
    tun_with(Arc::new(device), options)
}

/// The same on any device.
pub(crate) fn tun_with(
    device: Arc<dyn Device>,
    options: TunOptions,
) -> (TunHandle, Arc<FakeDialer>) {
    let dialer = FakeDialer::new();
    let tun = tun_dialing(device, dialer.clone(), &Extras::default(), options);
    (tun, dialer)
}

/// The same with a `Dialer` of the test's own, and `extras`. The only call of
/// `Tun::start` is here.
pub(crate) fn tun_dialing(
    device: Arc<dyn Device>,
    dialer: Arc<dyn Dialer>,
    extras: &Extras,
    options: TunOptions,
) -> TunHandle {
    Tun::start(
        device,
        dialer,
        extras.hooks.clone(),
        extras.pinger.clone(),
        extras.pool.clone(),
        options,
    )
}

/// A memory device that takes nothing to write until the test opens it: what
/// the TUN writes waits in the writer's hands, then in its queue, and the TUN
/// goes on until that is full.
pub(crate) struct GatedDevice {
    inner: MemoryDevice,
    gate: watch::Sender<bool>,
}

impl GatedDevice {
    /// A closed device with an MTU of 1500 and room for 4096 packets each way,
    /// and the app's end of it.
    pub(crate) fn pair() -> (Arc<GatedDevice>, AppEnd) {
        let (inner, end) = MemoryDevice::pair(1500, 4096);
        let device = GatedDevice {
            inner,
            gate: watch::channel(false).0,
        };
        (Arc::new(device), end)
    }

    /// From now on the device writes what it is given, and what it holds back.
    pub(crate) fn open(&self) {
        self.gate.send_replace(true);
    }
}

impl Device for GatedDevice {
    fn recv<'a>(&'a self, into: &'a mut Vec<Vec<u8>>, max: usize) -> BoxFuture<'a, io::Result<()>> {
        self.inner.recv(into, max)
    }

    fn send<'a>(&'a self, packets: &'a mut Vec<Vec<u8>>) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let mut gate = self.gate.subscribe();
            let opened = gate.wait_for(|open| *open).await.is_ok();
            if !opened {
                return Err(io::Error::other("the gate is gone"));
            }
            self.inner.send(packets).await
        })
    }

    fn mtu(&self) -> usize {
        self.inner.mtu()
    }
}

/// `packets` as the dispatcher hands them to a shard, holding none of the
/// budget of the shards' queues: for a test that hands a shard its turns by
/// hand.
pub(crate) fn ingress(packets: Vec<Vec<u8>>) -> Vec<Ingress> {
    packets.into_iter().map(Ingress::unheld).collect()
}

/// One shard on its own, for a test that takes its turns by hand, with what a
/// TUN gives each of its shards: the test holds the TUN's `Life`, `Stats`
/// and `Admission` as the TUN holds them, and the hooks the shard asks the
/// pre-matching rules through (the settings are those of a default `Extras`).
pub(crate) struct Lone {
    pub(crate) shard: Arc<Shard>,
    pub(crate) life: Arc<Life>,
    pub(crate) stats: Arc<Stats>,
    pub(crate) admission: Arc<Admission>,
    pub(crate) budget: Arc<Budget>,
    pub(crate) hooks: Arc<FakeHooks>,
}

/// A shard on a device with an MTU of 1500, whose clock starts at `epoch`, and
/// with no pool of fake addresses behind it.
pub(crate) fn lone_shard(options: &TunOptions, epoch: Instant) -> Lone {
    lone_shard_with_pool(options, epoch, None)
}

/// The same with `pool` behind it.
pub(crate) fn lone_shard_with_pool(
    options: &TunOptions,
    epoch: Instant,
    pool: Option<Arc<FakeIpPool>>,
) -> Lone {
    let life = Arc::new(Life::default());
    let stats = Arc::new(Stats::default());
    let admission = Arc::new(Admission::new(life.clone(), stats.clone(), options));
    let budget = Arc::new(Budget::new(
        options.budget,
        options.buffer_min,
        options.tx_max,
    ));
    let hooks = FakeHooks::new(settings(FakeResolve::new(), true));
    let shard = Arc::new(Shard::new(
        0,
        1500,
        epoch,
        1,
        budget.clone(),
        admission.clone(),
        pool,
        hooks.clone(),
        options,
    ));
    Lone {
        shard,
        life,
        stats,
        admission,
        budget,
        hooks,
    }
}

/// What a wait in these tests is bounded by, unless it says otherwise.
pub(crate) const WITHIN: Duration = Duration::from_secs(10);

/// `future`, which has `WITHIN` to finish; `what` is what it waits for.
pub(crate) async fn within<T>(what: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(WITHIN, future)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// Five seconds: what `in_five_seconds` gives a future, and the limit several
/// tests give a condition to come true.
pub(crate) const FIVE_SECONDS: Duration = Duration::from_secs(5);

/// `future`, which has five seconds to finish; `what` is what it waits for.
pub(crate) async fn in_five_seconds<T>(what: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(FIVE_SECONDS, future)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// `hello` there and back.
pub(crate) async fn hello(tcp: &mut AppTcp) {
    tcp.write_all(b"hello").await.expect("write");
    let mut back = [0; 5];
    within("hello to come back", tcp.read_exact(&mut back))
        .await
        .expect("read");
    assert_eq!(&back, b"hello");
}

/// Polls `condition` every millisecond until it holds, for at most `limit`.
pub(crate) async fn eventually(what: &str, limit: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + limit;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

pub(crate) fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

pub(crate) fn sock(ip: impl Into<IpAddr>, port: u16) -> SocketAddr {
    SocketAddr::new(ip.into(), port)
}

const CHUNK: usize = 16 * 1024;

/// Writes `bytes` bytes of the pattern, from its start.
pub(crate) async fn write_pattern(
    to: &mut (impl AsyncWrite + Unpin),
    bytes: usize,
) -> io::Result<()> {
    let mut done = 0;
    while done < bytes {
        let n = (bytes - done).min(CHUNK);
        let chunk: Vec<u8> = (done..done + n).map(|i| pattern(i as u64)).collect();
        to.write_all(&chunk).await?;
        done += n;
    }
    Ok(())
}

/// Reads `bytes` bytes and panics at the first that is not the pattern's.
pub(crate) async fn read_pattern(
    from: &mut (impl AsyncRead + Unpin),
    bytes: usize,
) -> io::Result<()> {
    read_pattern_from(from, 0, bytes).await
}

/// The same for a stream whose next byte is byte `offset` of the pattern.
pub(crate) async fn read_pattern_from(
    from: &mut (impl AsyncRead + Unpin),
    offset: usize,
    bytes: usize,
) -> io::Result<()> {
    let mut done = 0;
    let mut chunk = vec![0; CHUNK];
    while done < bytes {
        let n = (bytes - done).min(CHUNK);
        from.read_exact(&mut chunk[..n]).await?;
        for (k, byte) in chunk[..n].iter().enumerate() {
            let at = offset + done + k;
            assert_eq!(*byte, pattern(at as u64), "byte {at} is not the pattern's");
        }
        done += n;
    }
    Ok(())
}

/// The Internet checksum (RFC 1071) of `data`.
pub(crate) fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub(crate) fn v4(a: SocketAddr) -> (Ipv4Addr, u16) {
    match a {
        SocketAddr::V4(a) => (*a.ip(), a.port()),
        SocketAddr::V6(_) => panic!("an IPv4 address"),
    }
}

pub(crate) fn v6(a: SocketAddr) -> (Ipv6Addr, u16) {
    match a {
        SocketAddr::V6(a) => (*a.ip(), a.port()),
        SocketAddr::V4(_) => panic!("an IPv6 address"),
    }
}

/// An IPv4 packet around `payload`, the header checksum right.
pub(crate) fn ipv4_packet(protocol: u8, src: Ipv4Addr, dst: Ipv4Addr, payload: &[u8]) -> Vec<u8> {
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
pub(crate) fn udp_v4(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Vec<u8> {
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
pub(crate) fn syn_v4(src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    segment_v4(src, dst, 1, 0, 0x02)
}

/// A TCP segment over IPv4 with these numbers and flags and nothing else
/// (no options, no payload), the checksums right.
pub(crate) fn segment_v4(
    src: SocketAddr,
    dst: SocketAddr,
    seq: u32,
    ack: u32,
    flags: u8,
) -> Vec<u8> {
    data_segment_v4(src, dst, seq, ack, flags, &[])
}

/// The same with `payload`.
pub(crate) fn data_segment_v4(
    src: SocketAddr,
    dst: SocketAddr,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> Vec<u8> {
    let ((src_ip, src_port), (dst_ip, dst_port)) = (v4(src), v4(dst));
    let mut tcp = Vec::new();
    tcp.extend(src_port.to_be_bytes());
    tcp.extend(dst_port.to_be_bytes());
    tcp.extend(seq.to_be_bytes());
    tcp.extend(ack.to_be_bytes());
    // a header of 20 bytes, the flags, a window of 65535, no checksum yet, no urgent pointer
    tcp.extend([0x50, flags, 0xff, 0xff, 0, 0, 0, 0]);
    tcp.extend(payload);
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

/// An IPv6 packet around `payload`: a hop limit of 64 and no flow label.
pub(crate) fn ipv6_packet(
    next_header: u8,
    src: Ipv6Addr,
    dst: Ipv6Addr,
    payload: &[u8],
) -> Vec<u8> {
    let mut packet = vec![0x60, 0, 0, 0];
    packet.extend((payload.len() as u16).to_be_bytes());
    packet.extend([next_header, 64]);
    packet.extend(src.octets());
    packet.extend(dst.octets());
    packet.extend(payload);
    packet
}

/// A TCP SYN over IPv6, the checksums right.
pub(crate) fn syn_v6(src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let ((src_ip, src_port), (dst_ip, dst_port)) = (v6(src), v6(dst));
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
    pseudo.extend((tcp.len() as u32).to_be_bytes());
    pseudo.extend([0, 0, 0, 6]);
    pseudo.extend(&tcp);
    let sum = checksum(&pseudo);
    tcp[16..18].copy_from_slice(&sum.to_be_bytes());
    ipv6_packet(6, src_ip, dst_ip, &tcp)
}

/// An IP packet of the addresses' family around `payload`.
pub(crate) fn ip_around(protocol: u8, src: IpAddr, dst: IpAddr, payload: &[u8]) -> Vec<u8> {
    match (src, dst) {
        (IpAddr::V4(src), IpAddr::V4(dst)) => ipv4_packet(protocol, src, dst, payload),
        (IpAddr::V6(src), IpAddr::V6(dst)) => ipv6_packet(protocol, src, dst, payload),
        _ => panic!("the addresses of a packet are of one family"),
    }
}

/// The fields of a TCP segment of the app's: a window of 65535, no options,
/// no flags.
pub(crate) fn tcp_repr(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> TcpRepr<'_> {
    TcpRepr {
        src_port: src.port(),
        dst_port: dst.port(),
        control: TcpControl::None,
        seq_number: TcpSeqNumber(0),
        ack_number: None,
        window_len: 65535,
        window_scale: None,
        max_seg_size: None,
        sack_permitted: false,
        sack_ranges: [None; 3],
        timestamp: None,
        payload,
    }
}

/// `repr` as the bytes of a TCP header and payload, the checksum right for
/// the addresses.
pub(crate) fn tcp_bytes(repr: &TcpRepr<'_>, src: IpAddr, dst: IpAddr) -> Vec<u8> {
    let mut bytes = vec![0; repr.buffer_len()];
    repr.emit(
        &mut TcpPacket::new_unchecked(&mut bytes[..]),
        &src.into(),
        &dst.into(),
        &ChecksumCapabilities::default(),
    );
    bytes
}

pub(crate) fn tcp_packet(src: SocketAddr, dst: SocketAddr, repr: &TcpRepr<'_>) -> Vec<u8> {
    ip_around(6, src.ip(), dst.ip(), &tcp_bytes(repr, src.ip(), dst.ip()))
}

/// A UDP datagram around `payload`, the checksum right.
pub(crate) fn udp_bytes(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Vec<u8> {
    let repr = UdpRepr {
        src_port: src.port(),
        dst_port: dst.port(),
    };
    let mut bytes = vec![0; 8 + payload.len()];
    repr.emit(
        &mut UdpPacket::new_unchecked(&mut bytes[..]),
        &src.ip().into(),
        &dst.ip().into(),
        payload.len(),
        |buffer| buffer.copy_from_slice(payload),
        &ChecksumCapabilities::default(),
    );
    bytes
}

pub(crate) fn udp_packet(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Vec<u8> {
    ip_around(17, src.ip(), dst.ip(), &udp_bytes(src, dst, payload))
}

/// An ICMP echo request of the addresses' family, the checksum right.
pub(crate) fn echo_bytes(src: IpAddr, dst: IpAddr) -> Vec<u8> {
    echo_bytes_with(src, dst, &[1, 2, 3, 4])
}

/// The same with `data` after the header.
pub(crate) fn echo_bytes_with(src: IpAddr, dst: IpAddr, data: &[u8]) -> Vec<u8> {
    let caps = ChecksumCapabilities::default();
    match (src, dst) {
        (IpAddr::V4(_), IpAddr::V4(_)) => {
            let repr = Icmpv4Repr::EchoRequest {
                ident: 7,
                seq_no: 1,
                data,
            };
            let mut bytes = vec![0; repr.buffer_len()];
            repr.emit(&mut Icmpv4Packet::new_unchecked(&mut bytes[..]), &caps);
            bytes
        }
        (IpAddr::V6(src), IpAddr::V6(dst)) => {
            let repr = Icmpv6Repr::EchoRequest {
                ident: 7,
                seq_no: 1,
                data,
            };
            let mut bytes = vec![0; repr.buffer_len()];
            repr.emit(
                &src,
                &dst,
                &mut Icmpv6Packet::new_unchecked(&mut bytes[..]),
                &caps,
            );
            bytes
        }
        _ => panic!("the addresses of a packet are of one family"),
    }
}

pub(crate) fn echo_packet(src: IpAddr, dst: IpAddr) -> Vec<u8> {
    let protocol = if src.is_ipv4() { 1 } else { 58 };
    ip_around(protocol, src, dst, &echo_bytes(src, dst))
}

/// `packet` with the checksum of its IPv4 header worked out again, for a test
/// that changed the header and means the checksum to be right. The header
/// has to fit in the packet: it panics when its length (the IHL times four)
/// is more than the packet's.
pub(crate) fn refresh(mut packet: Vec<u8>) -> Vec<u8> {
    let header = usize::from(packet[0] & 15) * 4;
    packet[10..12].fill(0);
    let sum = checksum(&packet[..header]);
    packet[10..12].copy_from_slice(&sum.to_be_bytes());
    packet
}

/// An IPv4 packet with this flags and fragment offset field, the checksum
/// right.
pub(crate) fn with_fragment_field(mut packet: Vec<u8>, field: u16) -> Vec<u8> {
    packet[6..8].copy_from_slice(&field.to_be_bytes());
    refresh(packet)
}

/// An IPv4 packet with `options` (a multiple of four bytes) after its
/// header, the lengths and the checksum right.
pub(crate) fn with_ip_options(packet: &[u8], options: &[u8]) -> Vec<u8> {
    assert_eq!(options.len() % 4, 0);
    let mut longer = packet[..20].to_vec();
    longer.extend(options);
    longer.extend(&packet[20..]);
    longer[0] = 0x45 + (options.len() / 4) as u8;
    let total = longer.len() as u16;
    longer[2..4].copy_from_slice(&total.to_be_bytes());
    refresh(longer)
}

/// Where the TCP header of `packet` (IPv4 or IPv6, no extension headers)
/// starts, and where the packet ends; `None` for any other packet.
pub(crate) fn tcp_header(packet: &[u8]) -> Option<(usize, usize)> {
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
pub(crate) fn tcp_payload_len(packet: &[u8]) -> usize {
    tcp_header(packet).map_or(0, |(header, total)| {
        total - header - usize::from(packet[header + 12] >> 4) * 4
    })
}

/// Whether `packet` is a TCP segment with the RST flag.
pub(crate) fn is_reset(packet: &[u8]) -> bool {
    tcp_header(packet).is_some_and(|(header, _)| packet[header + 13] & 0x04 != 0)
}

/// Whether `packet` is a TCP segment with the SYN flag.
pub(crate) fn is_syn(packet: &[u8]) -> bool {
    tcp_header(packet).is_some_and(|(header, _)| packet[header + 13] & 0x02 != 0)
}

/// Whether `packet` is a TCP segment with the FIN flag.
pub(crate) fn is_fin(packet: &[u8]) -> bool {
    tcp_header(packet).is_some_and(|(header, _)| packet[header + 13] & 0x01 != 0)
}
