//! The guard at the edge of the window (phase 3 M3 design 5.3, H2). smoltcp
//! 0.12 takes data into a socket up to what its receive buffer has room for,
//! not up to the window it last told of (tcp.rs 1598-1599), and then
//! subtracts sequence numbers on the assumption that the data it holds ends
//! inside that window (693-701): with data taken in past the edge the
//! subtraction underflows and panics. An app that keeps to the window never
//! sends such data. One that does not can, by a defect or on purpose, and so
//! could a window that has gone back, as it can with a window scale (the size
//! of the receive buffer keeps the scale at 0: `RX_MAX`).
//!
//! So the right edge of the window is kept for each connection, read off each
//! segment the stack sends, as it is written (the device's send token calls
//! [`Edges::sent`]). That is the edge smoltcp itself works from: it records
//! the acknowledgement number and the window of every segment it sends. A
//! segment of the app's whose data ends past it ([`Edges::beyond`]) is dropped
//! before the stack sees it if the receive buffer has any room at all, however
//! long the segment is; the shard looks at that. Not room for the segment: the
//! stack takes in the part of a segment that fits in its window (1660-1672),
//! so a segment longer than the room would still take the end of the data it
//! holds (`next_ack`) past the edge it told of. With the buffer full there is
//! nothing to guard against: the stack takes in nothing of a segment with data
//! (1636-1641) and answers it with an ACK, at most one a second (1433-1446). So
//! that segment goes in, to be answered: it may be the probe of a sender with
//! a closed window, one byte of new data at the receiver's acknowledgment
//! number, and a sender must keep the connection open as long as its probes
//! are acknowledged (RFC 9293 3.8.6.1).

use super::Tuple;
use crate::packet::Segment;
use smoltcp::wire::{IpProtocol, Ipv4Packet, Ipv6Packet, TcpOption, TcpPacket};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

/// A window scale over this is taken as this (RFC 7323, 2.3), as smoltcp does.
const MAX_SHIFT: u8 = 14;

/// Whether `end` is after `edge` in the space of sequence numbers, which goes
/// round at 2^32: it is when it is less than half the space ahead.
pub(crate) fn past(end: u32, edge: u32) -> bool {
    end.wrapping_sub(edge) as i32 > 0
}

/// Where the window the stack last told of a connection ends.
#[derive(Clone, Copy)]
struct Edge {
    /// The acknowledgement number of the segment, plus its window in bytes.
    end: u32,
    /// What the window field is shifted left by: the window scale of the
    /// stack's SYN-ACK, which every segment after it is scaled by.
    shift: u8,
}

/// The edges of a shard's connections, by the connection's 4-tuple as the
/// app has it (its own end first).
#[derive(Default)]
pub(crate) struct Edges {
    edges: HashMap<Tuple, Edge>,
}

impl Edges {
    /// Takes in a packet the stack sent. The SYN-ACK of a connection starts
    /// its edge, with the scale it offers; every other segment of the
    /// connection moves it, to where that segment's window ends (nearer, if
    /// it is: the last is the one the app keeps to). The window of a SYN is
    /// not scaled. A reset says nothing of the window, and a segment of a
    /// connection that has no edge, or no segment at all, is not noted.
    pub(crate) fn sent(&mut self, packet: &[u8]) {
        let Some(sent) = Sent::read(packet) else {
            return;
        };
        if !sent.ack || sent.rst {
            return;
        }
        let window = u32::from(sent.window);
        if sent.syn {
            self.edges.insert(
                sent.tuple,
                Edge {
                    end: sent.ack_number.wrapping_add(window),
                    shift: sent.window_scale.unwrap_or(0).min(MAX_SHIFT),
                },
            );
        } else if let Some(edge) = self.edges.get_mut(&sent.tuple) {
            edge.end = sent.ack_number.wrapping_add(window << edge.shift);
        }
    }

    /// Whether `segment`, one of the app's, has data that ends past the edge
    /// of its connection. A segment without data does not, nor does one of a
    /// connection no edge is known of. It is the shard that drops the segment
    /// that does, and only when the receive buffer has any room at all, however
    /// long the segment is (see the module).
    pub(crate) fn beyond(&self, segment: &Segment) -> bool {
        segment.payload_len > 0
            && self.edges.get(&segment.tuple).is_some_and(|edge| {
                past(
                    segment.seq.wrapping_add(segment.payload_len as u32),
                    edge.end,
                )
            })
    }

    /// The connection is gone, and its edge with it.
    pub(crate) fn forget(&mut self, tuple: &Tuple) {
        self.edges.remove(tuple);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.edges.len()
    }
}

/// What the guard reads of a TCP segment the stack sent.
struct Sent {
    /// The connection, as the app has it: the segment goes to the app, so the
    /// packet's destination is the first end.
    tuple: Tuple,
    syn: bool,
    ack: bool,
    rst: bool,
    ack_number: u32,
    window: u16,
    /// The window scale option, of a SYN that has one.
    window_scale: Option<u8>,
}

impl Sent {
    /// `None` for what is no TCP segment of an IPv4 or IPv6 packet (without
    /// extension headers) that the stack made.
    fn read(packet: &[u8]) -> Option<Sent> {
        let (from, to, tcp): (IpAddr, IpAddr, &[u8]) = match packet.first()? >> 4 {
            4 => {
                let ip = Ipv4Packet::new_checked(packet).ok()?;
                if ip.header_len() < 20 || ip.next_header() != IpProtocol::Tcp {
                    return None;
                }
                (ip.src_addr().into(), ip.dst_addr().into(), ip.payload())
            }
            6 => {
                let ip = Ipv6Packet::new_checked(packet).ok()?;
                if ip.next_header() != IpProtocol::Tcp {
                    return None;
                }
                (ip.src_addr().into(), ip.dst_addr().into(), ip.payload())
            }
            _ => return None,
        };
        let tcp = TcpPacket::new_checked(tcp).ok()?;
        Some(Sent {
            tuple: Tuple {
                src: SocketAddr::new(to, tcp.dst_port()),
                dst: SocketAddr::new(from, tcp.src_port()),
            },
            syn: tcp.syn(),
            ack: tcp.ack(),
            rst: tcp.rst(),
            ack_number: tcp.ack_number().0 as u32,
            window: tcp.window_len(),
            window_scale: tcp.syn().then(|| window_scale(tcp.options())).flatten(),
        })
    }
}

/// The shift of the window scale option, if the table of options has one.
fn window_scale(mut options: &[u8]) -> Option<u8> {
    while !options.is_empty() {
        let (rest, option) = TcpOption::parse(options).ok()?;
        match option {
            TcpOption::EndOfList => return None,
            TcpOption::WindowScale(shift) => return Some(shift),
            _ => options = rest,
        }
    }
    None
}
