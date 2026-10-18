//! What the tests of this crate share: the TUN they start, waits that are
//! bounded, streams of the pattern, and the builders and readers of the
//! packets they make by hand.

use crate::device::{AppEnd, MemoryDevice};
use crate::testing::{AppStack, FakeDialer, pattern};
use crate::{Tun, TunHandle, TunOptions};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::wire::{
    Icmpv4Packet, Icmpv4Repr, Icmpv6Packet, Icmpv6Repr, TcpControl, TcpPacket, TcpRepr,
    TcpSeqNumber, UdpPacket, UdpRepr,
};
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A TUN on one end of a memory device, the app's stack on the other, and
/// the fake `Dialer` the TUN hands its sessions to.
pub(crate) struct Rig {
    pub(crate) tun: TunHandle,
    pub(crate) app: AppStack,
    pub(crate) dialer: Arc<FakeDialer>,
}

/// A rig on a memory device with an MTU of 1500 and room for 4096 packets
/// each way.
pub(crate) fn rig(options: TunOptions) -> Rig {
    rig_on(MemoryDevice::pair(1500, 4096), options)
}

/// Every test that starts a TUN starts it here, so that a change of what
/// `Tun::start` takes is a change of this function only.
pub(crate) fn rig_on((device, end): (MemoryDevice, AppEnd), options: TunOptions) -> Rig {
    let (tun, dialer) = tun_on(device, options);
    Rig {
        tun,
        app: AppStack::start(end),
        dialer,
    }
}

/// The TUN half of `rig_on`, on the current runtime: for a test that runs
/// the app's stack on another one. The only call of `Tun::start` is here.
pub(crate) fn tun_on(device: MemoryDevice, options: TunOptions) -> (TunHandle, Arc<FakeDialer>) {
    let dialer = FakeDialer::new();
    let tun = Tun::start(Arc::new(device), dialer.clone(), options);
    (tun, dialer)
}

/// What a wait in these tests is bounded by, unless it says otherwise.
pub(crate) const WITHIN: Duration = Duration::from_secs(10);

/// `future`, which has `WITHIN` to finish; `what` is what it waits for.
pub(crate) async fn within<T>(what: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(WITHIN, future)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
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
    let caps = ChecksumCapabilities::default();
    match (src, dst) {
        (IpAddr::V4(_), IpAddr::V4(_)) => {
            let repr = Icmpv4Repr::EchoRequest {
                ident: 7,
                seq_no: 1,
                data: &[1, 2, 3, 4],
            };
            let mut bytes = vec![0; repr.buffer_len()];
            repr.emit(&mut Icmpv4Packet::new_unchecked(&mut bytes[..]), &caps);
            bytes
        }
        (IpAddr::V6(src), IpAddr::V6(dst)) => {
            let repr = Icmpv6Repr::EchoRequest {
                ident: 7,
                seq_no: 1,
                data: &[1, 2, 3, 4],
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
/// that changed the header and means the checksum to be right.
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
