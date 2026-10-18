//! What the tests of this crate share: the TUN they start, waits that are
//! bounded, streams of the pattern, and the builders and readers of the
//! packets they make by hand.

use crate::device::{AppEnd, MemoryDevice};
use crate::testing::{AppStack, FakeDialer, pattern};
use crate::{Tun, TunHandle, TunOptions};
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
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
    let dialer = FakeDialer::new();
    let tun = Tun::start(Arc::new(device), dialer.clone(), options);
    Rig {
        tun,
        app: AppStack::start(end),
        dialer,
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
    let ((src_ip, src_port), (dst_ip, dst_port)) = (v4(src), v4(dst));
    let mut tcp = Vec::new();
    tcp.extend(src_port.to_be_bytes());
    tcp.extend(dst_port.to_be_bytes());
    tcp.extend(seq.to_be_bytes());
    tcp.extend(ack.to_be_bytes());
    // a header of 20 bytes, the flags, a window of 65535, no checksum yet, no urgent pointer
    tcp.extend([0x50, flags, 0xff, 0xff, 0, 0, 0, 0]);
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
