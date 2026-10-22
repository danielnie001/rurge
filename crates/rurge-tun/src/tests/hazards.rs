//! The hazards of smoltcp 0.12 that the shards stand in front of (phase 3 M3
//! design 5.3): SACK (H3) and the edge of the window (H2). The stack that
//! panics all the same, behind both, is in `rebuild.rs`. First what is a
//! function of its input alone: the SACK-permitted option, the edge. Then the
//! whole TUN: SACK, a segment past the edge, the window scale the edge rests
//! on, a connection stalled at a window of 0. Then one shard, its turns taken
//! by hand, at the edge of the window.

use super::support::{
    Lone, Rig, WITHIN, addr, data_segment_v4, echo_packet, eventually, hello, is_syn, lone_shard,
    refresh, rig, segment_v4, sock, syn_v4, tcp_header, tcp_packet, tcp_payload_len, tcp_repr,
    udp_packet, with_ip_options, within, write_pattern,
};
use crate::packet::{Segment, strip_sack_permitted};
use crate::space::ADAPTER_V4;
use crate::tcp::guard::{Edges, past};
use crate::tcp::shard::Opened;
use crate::tcp::{ConnId, Shard, TunStream, Tuple};
use crate::testing::{AppStack, AppTcp, Direction, Script, Upstream, pattern};
use crate::{Counters, TunOptions};
use smoltcp::phy::ChecksumCapabilities;
use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, TcpControl, TcpPacket, TcpRepr, TcpSeqNumber, TcpTimestampRepr};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SYN: u8 = 0x02;
const FIN: u8 = 0x01;
const RST: u8 = 0x04;
pub(super) const ACK: u8 = 0x10;
pub(super) const PSH_ACK: u8 = 0x18;

/// The kinds of the TCP options the tests look for.
const WINDOW_SCALE: u8 = 3;
const SACK_PERMITTED: u8 = 4;
const SACK_BLOCKS: u8 = 5;

// ---- reading what a test sees

/// The numbers and flags of a TCP segment, read off its packet.
#[derive(Clone, Copy, Debug)]
pub(super) struct Fields {
    pub(super) seq: u32,
    ack: u32,
    window: u16,
    flags: u8,
    payload_len: usize,
}

pub(super) fn fields(packet: &[u8]) -> Option<Fields> {
    let (header, _) = tcp_header(packet)?;
    let word = |at: usize| {
        u32::from_be_bytes(
            packet[header + at..header + at + 4]
                .try_into()
                .expect("four bytes"),
        )
    };
    Some(Fields {
        seq: word(4),
        ack: word(8),
        window: u16::from_be_bytes([packet[header + 14], packet[header + 15]]),
        flags: packet[header + 13],
        payload_len: tcp_payload_len(packet),
    })
}

/// The option table of the TCP segment `packet`.
fn options(packet: &[u8]) -> &[u8] {
    let (header, _) = tcp_header(packet).expect("a TCP segment");
    &packet[header + 20..header + usize::from(packet[header + 12] >> 4) * 4]
}

/// Where in a sound table of options the option of this kind is, if one is.
fn find_option(options: &[u8], kind: u8) -> Option<usize> {
    let mut at = 0;
    while at < options.len() {
        match options[at] {
            0 => return None,
            1 => at += 1,
            this if this == kind => return Some(at),
            _ => at += usize::from(*options.get(at + 1)?).max(2),
        }
    }
    None
}

fn has_option(options: &[u8], kind: u8) -> bool {
    find_option(options, kind).is_some()
}

/// What the TUN has told the app of its connection, read off the packets
/// that go to the app.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Told {
    /// The left shift of the windows after the SYN-ACK: its window scale.
    shift: u8,
    /// The TUN's next sequence number, what it has acknowledged, and the room
    /// it gave for more, in bytes, after the shift.
    pub(super) seq: u32,
    pub(super) ack: u32,
    window: u32,
    /// How many packets there have been.
    packets: usize,
    /// A window of 0 has been told, once or more.
    zero_window: bool,
}

impl Told {
    fn take(&mut self, packet: &[u8]) {
        let Some(read) = fields(packet) else {
            return;
        };
        if read.flags & RST != 0 {
            return;
        }
        let syn = read.flags & SYN != 0;
        if syn {
            self.shift =
                find_option(options(packet), WINDOW_SCALE).map_or(0, |at| options(packet)[at + 2]);
        }
        self.seq = read
            .seq
            .wrapping_add(read.payload_len as u32)
            .wrapping_add(u32::from(syn))
            .wrapping_add(u32::from(read.flags & FIN != 0));
        self.ack = read.ack;
        // the window of a SYN is not scaled
        self.window = u32::from(read.window) << if syn { 0 } else { self.shift };
        self.zero_window |= !syn && read.window == 0;
        self.packets += 1;
    }

    /// Where the window the TUN told of ends, in the app's sequence numbers.
    fn edge(&self) -> u32 {
        self.ack.wrapping_add(self.window)
    }
}

/// Makes `app` keep what the TUN tells it of its connections in the
/// returned `Told`; it lets every packet through.
pub(super) fn watch(app: &AppStack) -> Arc<Mutex<Told>> {
    let told = Arc::new(Mutex::new(Told::default()));
    let watching = told.clone();
    app.set_filter(move |direction, packet| {
        if direction == Direction::FromTun {
            watching.lock().expect("what the TUN told").take(packet);
        }
        true
    });
    told
}

// ---- the SACK-permitted option (H3)

/// A TCP SYN over IPv4 with `options` for its option table, the lengths and
/// the IPv4 header checksum right (the TCP checksum is not: the TUN does not
/// check it).
fn syn_with_options(options: &[u8]) -> Vec<u8> {
    assert_eq!(options.len() % 4, 0, "a table of whole words");
    let syn = syn_v4(addr("198.18.0.1:40000"), addr("192.0.2.1:80"));
    let mut longer = syn[..40].to_vec();
    longer.extend(options);
    // the data offset, in words
    longer[20 + 12] = (((20 + options.len()) / 4) as u8) << 4;
    let total = longer.len() as u16;
    longer[2..4].copy_from_slice(&total.to_be_bytes());
    refresh(longer)
}

/// `packet` as smoltcp reads its TCP segment.
fn read_by_smoltcp(packet: &[u8]) -> TcpRepr<'_> {
    let (header, total) = tcp_header(packet).expect("a TCP segment");
    let segment = TcpPacket::new_checked(&packet[header..total]).expect("a TCP header");
    let (src, dst): (IpAddress, IpAddress) = (
        addr("198.18.0.1:0").ip().into(),
        addr("192.0.2.1:0").ip().into(),
    );
    TcpRepr::parse(&segment, &src, &dst, &ChecksumCapabilities::ignored())
        .expect("smoltcp reads the segment")
}

#[test]
fn strip_sack_permitted_leaves_other_options() {
    let (app, dst) = (addr("198.18.0.1:40000"), addr("192.0.2.1:80"));
    let repr = TcpRepr {
        control: TcpControl::Syn,
        seq_number: TcpSeqNumber(1),
        max_seg_size: Some(1460),
        window_scale: Some(7),
        sack_permitted: true,
        timestamp: Some(TcpTimestampRepr::new(1, 0)),
        ..tcp_repr(app, dst, &[])
    };
    let syn = tcp_packet(app, dst, &repr);
    assert!(has_option(options(&syn), SACK_PERMITTED), "the premise");
    let at = find_option(options(&syn), SACK_PERMITTED).expect("SACK-permitted");
    let (header, _) = tcp_header(&syn).expect("a TCP segment");
    let mut stripped = syn.clone();
    strip_sack_permitted(&mut stripped);
    // two bytes of the packet changed: the option's kind and length, to two no-operations
    let mut expected = syn.clone();
    expected[header + 20 + at..header + 20 + at + 2].copy_from_slice(&[1, 1]);
    assert_eq!(stripped, expected);
    // smoltcp reads the rest as it read it
    let read = read_by_smoltcp(&stripped);
    assert!(!read.sack_permitted);
    assert_eq!(read.max_seg_size, Some(1460));
    assert_eq!(read.window_scale, Some(7));
    assert_eq!(read.timestamp, Some(TcpTimestampRepr::new(1, 0)));
    assert_eq!(read.control, TcpControl::Syn);
    // an IPv4 header with options: the TCP header begins where the length of
    // the IP header says
    let with_options = with_ip_options(&syn, &[1, 1, 1, 0]);
    let (long_header, _) = tcp_header(&with_options).expect("a TCP segment");
    assert_eq!(long_header, 24);
    let mut stripped = with_options.clone();
    strip_sack_permitted(&mut stripped);
    let mut expected = with_options.clone();
    expected[long_header + 20 + at..long_header + 20 + at + 2].copy_from_slice(&[1, 1]);
    assert_eq!(stripped, expected);
    // the same over IPv6
    let (app, dst) = (addr("[fd00:6152::1]:40000"), addr("[2001:db8::1]:80"));
    let syn = tcp_packet(app, dst, &repr);
    let mut stripped = syn.clone();
    strip_sack_permitted(&mut stripped);
    let (header, _) = tcp_header(&syn).expect("a TCP segment");
    assert_eq!(header, 40, "an IPv6 header and no more");
    let mut expected = syn.clone();
    expected[header + 20 + at..header + 20 + at + 2].copy_from_slice(&[1, 1]);
    assert_eq!(stripped, expected);
}

/// Kind 4 with another length than two is no offer of SACK (and smoltcp
/// would drop the SYN).
#[test]
fn strip_sack_permitted_takes_only_an_option_of_two_bytes() {
    let table = [4, 3, 9, 1];
    let mut syn = syn_with_options(&table);
    strip_sack_permitted(&mut syn);
    assert_eq!(options(&syn), table);
}

#[test]
fn strip_sack_permitted_takes_every_one_and_stops_at_the_end_of_the_list() {
    // each SACK-permitted option in the table, however many
    let table = [4, 2, 4, 2, 1, 4, 2, 1];
    let mut syn = syn_with_options(&table);
    strip_sack_permitted(&mut syn);
    assert_eq!(options(&syn), [1, 1, 1, 1, 1, 1, 1, 1]);
    // what follows the end of the list is not a table any more
    let table = [4, 2, 0, 0, 4, 2, 1, 1];
    let mut syn = syn_with_options(&table);
    strip_sack_permitted(&mut syn);
    assert_eq!(options(&syn), [1, 1, 0, 0, 4, 2, 1, 1]);
    let table = [1, 0, 4, 2];
    let mut syn = syn_with_options(&table);
    strip_sack_permitted(&mut syn);
    assert_eq!(
        options(&syn),
        table,
        "after the end of the list there is none"
    );
}

/// A table that is not a table is not touched, though a SACK-permitted option
/// comes before the fault: smoltcp drops such a segment itself.
#[test]
fn strip_sack_permitted_leaves_a_table_that_is_not_sound() {
    let faulty: [[u8; 4]; 6] = [
        // the length is past the end of the table
        [4, 2, 5, 200],
        // a length of 0 or 1 is no length
        [4, 2, 2, 1],
        [4, 2, 2, 0],
        // a kind without its length at the end
        [4, 2, 1, 3],
        [1, 1, 1, 4],
        // an option whose length is more than what is left
        [4, 2, 8, 4],
    ];
    for table in faulty {
        let syn = syn_with_options(&table);
        let mut stripped = syn.clone();
        strip_sack_permitted(&mut stripped);
        assert_eq!(stripped, syn, "{table:?}");
    }
}

#[test]
fn strip_sack_permitted_leaves_what_is_no_syn_or_no_tcp() {
    // a segment of a connection that is on: SACK-permitted has no meaning there
    let mut ack = syn_with_options(&[4, 2, 1, 1]);
    let (header, _) = tcp_header(&ack).expect("a TCP segment");
    ack[header + 13] = ACK;
    let mut stripped = ack.clone();
    strip_sack_permitted(&mut stripped);
    assert_eq!(stripped, ack);
    // nothing that is no TCP segment, nothing cut short
    let syn = syn_with_options(&[4, 2, 1, 1]);
    let (app, dst) = (addr("198.18.0.1:40000"), addr("192.0.2.1:53"));
    let mut others = vec![
        udp_packet(app, dst, &[4, 2, 4, 2]),
        echo_packet(app.ip(), dst.ip()),
        Vec::new(),
        vec![0x45],
        vec![0x60; 3],
        syn[..20].to_vec(),
        syn[..35].to_vec(),
        syn[..syn.len() - 1].to_vec(),
    ];
    // a header whose length is less than 20, or more than the packet
    let mut short_header = syn.clone();
    short_header[0] = 0x44;
    others.push(short_header);
    let mut long_header = syn.clone();
    long_header[0] = 0x4f;
    others.push(long_header);
    // a TCP header with an offset past the end of the packet
    let mut long_offset = syn.clone();
    long_offset[20 + 12] = 0xf0;
    others.push(long_offset);
    for other in others {
        let mut stripped = other.clone();
        strip_sack_permitted(&mut stripped);
        assert_eq!(stripped, other);
    }
}

/// Whatever the bytes are, it does not panic, and doing it again changes
/// nothing more.
#[test]
fn strip_sack_permitted_takes_any_bytes() {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // MSS, SACK-permitted, a no-operation, window scale, a no-operation, the end
    let base = syn_with_options(&[2, 4, 5, 180, 4, 2, 1, 3, 3, 7, 1, 0]);
    for _ in 0..20_000 {
        let mut packet = base.clone();
        let length = (next() % 80) as usize;
        packet.resize(length, 0);
        for _ in 0..(next() % 6) {
            let at = (next() % packet.len().max(1) as u64) as usize;
            if let Some(byte) = packet.get_mut(at) {
                *byte = next() as u8;
            }
        }
        let mut once = packet.clone();
        strip_sack_permitted(&mut once);
        let mut twice = once.clone();
        strip_sack_permitted(&mut twice);
        assert_eq!(once, twice, "stripping again changes nothing: {packet:?}");
        assert_eq!(once.len(), packet.len());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sack_permitted_is_not_offered_back() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    // each SYN either side sent, and whether it offers SACK
    let syns: Arc<Mutex<Vec<(Direction, bool)>>> = Arc::default();
    let seen = syns.clone();
    rig.app.set_filter(move |direction, packet| {
        if is_syn(packet) {
            seen.lock()
                .expect("the SYNs")
                .push((direction, has_option(options(packet), SACK_PERMITTED)));
        }
        true
    });
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    let syns = syns.lock().expect("the SYNs").clone();
    assert!(
        syns.contains(&(Direction::ToTun, true)),
        "the app's stack always offers SACK in its SYN: {syns:?}"
    );
    let answers: Vec<bool> = syns
        .iter()
        .filter(|(direction, _)| *direction == Direction::FromTun)
        .map(|&(_, offers)| offers)
        .collect();
    assert!(!answers.is_empty(), "the TUN answered the SYN");
    assert!(
        answers.iter().all(|&offers| !offers),
        "the TUN offers SACK back: {syns:?}"
    );
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// What the filter of the next test does to the app's data segments and the
/// TUN's acknowledgements.
#[derive(Default)]
struct Reordering {
    /// The first data segment of the upload, held back until `release`:
    /// where it begins in the app's sequence numbers.
    held: Option<u32>,
    release: bool,
    /// Segments after it that went through meanwhile.
    passed_after: usize,
    /// The TUN's acknowledgements of the sequence number held back.
    asked_again: usize,
    /// The TUN's packets with SACK blocks.
    with_blocks: usize,
}

/// The first data segment of an upload reaches the TUN last, after the ones
/// that follow it. For each of those the TUN acknowledges the byte it is still
/// waiting for, again (a duplicate acknowledgement); with SACK on, it would say
/// in blocks which data it holds. No acknowledgement has a block, and nothing
/// panics (smoltcp 0.12 adds up the ends of the blocks in a way that overflows
/// in a debug build).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn out_of_order_data_gets_no_sack_blocks() {
    const BYTES: usize = 4000;
    let rig = rig(TunOptions::default());
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(
        dst,
        Script::Memory {
            buffer: 64 * 1024,
            send: 0,
        },
    );
    let state = Arc::new(Mutex::new(Reordering::default()));
    let watching = state.clone();
    rig.app.set_filter(move |direction, packet| {
        let Some(read) = fields(packet) else {
            return true;
        };
        let mut state = watching.lock().expect("the reordering");
        match direction {
            Direction::ToTun if read.payload_len > 0 => match state.held {
                // the first data segment, and its repeats until it is let go
                None => {
                    state.held = Some(read.seq);
                    return false;
                }
                Some(held) if read.seq == held && !state.release => return false,
                Some(held) if read.seq != held => state.passed_after += 1,
                Some(_) => {}
            },
            Direction::FromTun => {
                state.with_blocks += usize::from(has_option(options(packet), SACK_BLOCKS));
                if read.flags & SYN == 0 && Some(read.ack) == state.held {
                    state.asked_again += 1;
                }
            }
            Direction::ToTun => {}
        }
        true
    });
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let data: Vec<u8> = (0..BYTES).map(|i| pattern(i as u64)).collect();
    tcp.write_all(&data).await.expect("write");
    eventually(
        "the TUN to ask again for the segment held back",
        WITHIN,
        || {
            let state = state.lock().expect("the reordering");
            state.passed_after >= 1 && state.asked_again >= 1
        },
    )
    .await;
    state.lock().expect("the reordering").release = true;
    assert!(
        rig.dialer
            .wait_for(dst, BYTES as u64, Duration::from_secs(10))
            .await,
        "the data arrives whole"
    );
    assert_eq!(
        state.lock().expect("the reordering").with_blocks,
        0,
        "an acknowledgement of the TUN's has SACK blocks"
    );
    assert_eq!(rig.tun.counters(), Counters::default());
}

// ---- the edge of the window (H2)

#[test]
fn past_wraps_around() {
    // after, equal, before
    assert!(past(11, 10));
    assert!(!past(10, 10));
    assert!(!past(9, 10));
    // across the end of the space: 2 comes after 0xffff_fffe
    assert!(past(2, u32::MAX - 1));
    assert!(!past(u32::MAX - 1, 2));
    assert!(past(0, u32::MAX));
    assert!(!past(u32::MAX, 0));
    // as far as half the space is after; past that is before
    assert!(past(0x7fff_ffff, 0));
    assert!(!past(0x8000_0000, 0));
    assert!(past(0x8000_0000, 1));
    assert!(past(5 + 0x7fff_ffff, 5));
    assert!(!past(5 + 0x8000_0000, 5));
}

/// A segment the stack sends from `dst` to `app`, with these numbers.
fn from_the_stack(
    app: SocketAddr,
    dst: SocketAddr,
    control: TcpControl,
    ack: Option<u32>,
    window: u16,
    scale: Option<u8>,
) -> Vec<u8> {
    let repr = TcpRepr {
        control,
        seq_number: TcpSeqNumber(7),
        ack_number: ack.map(|number| TcpSeqNumber(number as i32)),
        window_len: window,
        window_scale: scale,
        ..tcp_repr(dst, app, &[])
    };
    tcp_packet(dst, app, &repr)
}

/// A data segment of the app's with this sequence number and payload.
fn from_the_app(tuple: Tuple, seq: u32, payload_len: usize) -> Segment {
    Segment {
        tuple,
        syn: false,
        ack: true,
        fin: false,
        rst: false,
        seq,
        ack_number: 0,
        header_len: 20,
        payload_len,
    }
}

#[test]
fn the_edge_is_where_the_last_window_the_stack_told_of_ends() {
    for (app, dst) in [
        (addr("198.18.0.1:40000"), addr("192.0.2.1:80")),
        (addr("[fd00:6152::1]:40000"), addr("[2001:db8::1]:80")),
    ] {
        let tuple = Tuple { src: app, dst };
        let stack =
            |control, ack, window, scale| from_the_stack(app, dst, control, ack, window, scale);
        let mut edges = Edges::default();
        // nothing is known of a connection until its SYN-ACK
        edges.sent(&stack(TcpControl::None, Some(100), 1000, None));
        assert_eq!(edges.len(), 0, "an acknowledgement of no connection");
        assert!(!edges.beyond(&from_the_app(tuple, 5000, 10)));
        // the SYN-ACK: the window of a SYN is not scaled, the scale is for what follows
        edges.sent(&stack(TcpControl::Syn, Some(100), 1000, Some(2)));
        assert_eq!(edges.len(), 1);
        assert!(
            !edges.beyond(&from_the_app(tuple, 1080, 20)),
            "up to the edge, 1100"
        );
        assert!(
            edges.beyond(&from_the_app(tuple, 1081, 20)),
            "a byte past it"
        );
        assert!(edges.beyond(&from_the_app(tuple, 2000, 1)));
        // a segment with no data is let by, past the edge or not
        assert!(!edges.beyond(&from_the_app(tuple, 5000, 0)));
        // the latest wins, though it is nearer: 500 and 100 scaled by 4 is 900
        edges.sent(&stack(TcpControl::None, Some(500), 100, None));
        assert!(!edges.beyond(&from_the_app(tuple, 880, 20)), "up to 900");
        assert!(edges.beyond(&from_the_app(tuple, 881, 20)));
        // a reset says nothing of the window
        edges.sent(&stack(TcpControl::Rst, Some(2000), 0, None));
        assert!(edges.beyond(&from_the_app(tuple, 881, 20)));
        assert!(!edges.beyond(&from_the_app(tuple, 880, 20)));
        // an acknowledgement is the edge too, with a window of 0
        edges.sent(&stack(TcpControl::None, Some(777), 0, None));
        assert!(
            edges.beyond(&from_the_app(tuple, 777, 1)),
            "the probe of a zero window"
        );
        assert!(!edges.beyond(&from_the_app(tuple, 770, 7)));
        // another connection has none
        let other = Tuple {
            src: SocketAddr::new(app.ip(), app.port() + 1),
            dst,
        };
        assert!(!edges.beyond(&from_the_app(other, 9999, 10)));
        // the connection goes and its edge with it
        edges.forget(&tuple);
        assert_eq!(edges.len(), 0);
        assert!(!edges.beyond(&from_the_app(tuple, 9999, 10)));
        // the edge goes round the end of the space with the numbers
        edges.sent(&stack(TcpControl::Syn, Some(u32::MAX - 10), 100, None));
        assert!(
            !edges.beyond(&from_the_app(tuple, u32::MAX - 5, 10)),
            "to 89"
        );
        assert!(!edges.beyond(&from_the_app(tuple, 70, 19)));
        assert!(edges.beyond(&from_the_app(tuple, 70, 20)));
        // a scale of more than 14 is taken as 14, as smoltcp and the RFC have it
        edges.sent(&stack(TcpControl::Syn, Some(0), 1, Some(20)));
        edges.sent(&stack(TcpControl::None, Some(0), 1, None));
        assert!(!edges.beyond(&from_the_app(tuple, 0, 1 << 14)));
        assert!(edges.beyond(&from_the_app(tuple, 1, 1 << 14)));
    }
}

#[test]
fn the_edge_takes_any_bytes_without_a_panic() {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let (app, dst) = (addr("198.18.0.1:40000"), addr("192.0.2.1:80"));
    let base = from_the_stack(app, dst, TcpControl::Syn, Some(1), 10, Some(2));
    let mut edges = Edges::default();
    for _ in 0..20_000 {
        let mut packet = base.clone();
        packet.resize((next() % 80) as usize, 0);
        for _ in 0..(next() % 6) {
            let at = (next() % packet.len().max(1) as u64) as usize;
            if let Some(byte) = packet.get_mut(at) {
                *byte = next() as u8;
            }
        }
        edges.sent(&packet);
    }
    // an ICMP message and a UDP datagram
    edges.sent(&echo_packet(app.ip(), dst.ip()));
    edges.sent(&udp_packet(app, dst, b"x"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_segment_past_the_edge_is_dropped() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    let heard = watch(&rig.app);
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    let told = *heard.lock().expect("what the TUN told");
    assert!(told.window > 0, "the TUN has room: {told:?}");
    // a segment of the connection that begins a thousand bytes past the edge
    // of what the TUN last said it has room for
    rig.app.inject(data_segment_v4(
        tcp.local(),
        dst,
        told.edge().wrapping_add(1000),
        told.seq,
        PSH_ACK,
        &[0xee; 10],
    ));
    eventually("the segment to be dropped", WITHIN, || {
        rig.tun.counters().beyond_window == 1
    })
    .await;
    // the connection goes on
    hello(&mut tcp).await;
    assert_eq!(
        rig.tun.counters(),
        Counters {
            beyond_window: 1,
            ..Counters::default()
        }
    );
    assert_eq!(rig.tun.sockets_counted(), 1);
}

/// The edge the guard keeps only moves forward while the TUN's window scale is
/// 0 (see `RX_MAX`). The app's SYN offers window scaling, and the SYN-ACK of the
/// TUN answers with a scale of 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_tun_never_scales_its_window() {
    let rig = rig(TunOptions::default());
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Upstream(echo.addr()));
    // the window scale of each SYN either side sent, if it had one
    type Scales = Vec<(Direction, Option<u8>)>;
    let scales: Arc<Mutex<Scales>> = Arc::default();
    let seen = scales.clone();
    rig.app.set_filter(move |direction, packet| {
        if is_syn(packet) {
            let table = options(packet);
            let scale = find_option(table, WINDOW_SCALE).map(|at| table[at + 2]);
            seen.lock().expect("the SYNs").push((direction, scale));
        }
        true
    });
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    let scales = scales.lock().expect("the SYNs").clone();
    assert!(
        scales
            .iter()
            .any(|&(direction, scale)| direction == Direction::ToTun && scale.is_some()),
        "the app's SYN offers window scaling: {scales:?}"
    );
    let answers: Vec<Option<u8>> = scales
        .iter()
        .filter(|(direction, _)| *direction == Direction::FromTun)
        .map(|&(_, scale)| scale)
        .collect();
    assert!(!answers.is_empty(), "the TUN answered the SYN");
    assert!(
        answers.iter().all(|&scale| scale == Some(0)),
        "the SYN-ACK of the TUN carries a window scale of 0: {scales:?}"
    );
}

/// An upload into an upstream that reads nothing until `release`: a MiB the
/// app writes in a task of its own, and what the TUN tells of the window.
struct Stalled {
    rig: Rig,
    dst: SocketAddr,
    heard: Arc<Mutex<Told>>,
    writing: tokio::task::JoinHandle<io::Result<AppTcp>>,
}

const MIB: usize = 1 << 20;

async fn stalled_upload() -> Stalled {
    let rig = rig(TunOptions::default());
    let dst = addr("192.0.2.1:80");
    rig.dialer.script(dst, Script::Hold { buffer: 1024 });
    let heard = watch(&rig.app);
    let mut tcp = within("the connection to be made", rig.app.connect(dst))
        .await
        .expect("connect");
    let writing = tokio::spawn(async move {
        write_pattern(&mut tcp, MIB).await?;
        Ok(tcp)
    });
    eventually(
        "the TUN to tell the app it has no room",
        Duration::from_secs(5),
        || heard.lock().expect("what the TUN told").zero_window,
    )
    .await;
    Stalled {
        rig,
        dst,
        heard,
        writing,
    }
}

impl Stalled {
    /// Waits until the TUN has told of a window of 0 and said nothing more for
    /// 100 ms, and returns what it told. What it waits for is the end of the
    /// fill-up: the relay stops reading when the pipe and its own buffer are
    /// full, and the window stays 0, which a window of 0 told a moment ago does
    /// not say (the relay may read again and open it). The margin, on the
    /// development machine: from its first window of 0 to its last packet the
    /// TUN took at most 1.4 ms (100 runs alone) or 4.2 ms (30 runs with the
    /// whole test binary running), and the longest silence between two of its
    /// packets in that time was 1.3 ms (3.8 ms), so the 100 ms of quiet is more
    /// than 20 times either.
    async fn settled(&self) -> Told {
        let mut packets = None;
        let mut quiet = 0;
        for _ in 0..250 {
            let told = *self.heard.lock().expect("what the TUN told");
            if told.window == 0 && packets == Some(told.packets) {
                quiet += 1;
                if quiet == 5 {
                    return told;
                }
            } else {
                quiet = 0;
            }
            packets = Some(told.packets);
            // a poll, the wait for something not to happen being what settles
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the TUN did not stay at a window of 0");
    }

    /// Lets the upstream read, waits for the MiB to get there and the writer
    /// to end, and finds the TUN's counts as they should be.
    async fn finish(self, counted: Counters) {
        self.rig.dialer.release(self.dst);
        assert!(
            self.rig
                .dialer
                .wait_for(self.dst, MIB as u64, Duration::from_secs(30))
                .await,
            "the MiB did not get there"
        );
        within("the writer", self.writing)
            .await
            .expect("the writer's task")
            .expect("the writes");
        assert_eq!(self.rig.tun.counters(), counted);
    }
}

/// An upstream that reads nothing fills the TUN's buffer, the TUN tells the
/// app it has no room, and when the upstream reads again the window opens and
/// the upload goes on: the guard is not in its way, and nothing panics.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upload_into_a_stalled_upstream_resumes() {
    stalled_upload().await.finish(Counters::default()).await;
}

/// The zero-window probe of Windows and BSD stacks: one byte of new data at the
/// receiver's acknowledgment number, into a window of 0 (Linux sends an empty
/// segment one byte back, which the guard never touches; smoltcp's own sender
/// sends neither, so the test makes the probe). The buffer is full: the stack
/// takes nothing of the byte (tcp.rs 1636-1641) and answers it with an ACK of
/// where it waits, window 0 (1433-1446). That answer is what keeps a sender
/// probing a closed window (RFC 9293 3.8.6.1), and the guard lets the probe go
/// in for it. Nothing is counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_window_probe_into_a_full_buffer_is_answered() {
    let stalled = stalled_upload().await;
    let told = stalled.settled().await;
    let app = stalled.rig.dialer.dialed()[0].src;
    // the TUN has been quiet: what it says after this was said for the probe
    let said = stalled.heard.lock().expect("what the TUN told").packets;
    stalled.rig.app.inject(data_segment_v4(
        app,
        stalled.dst,
        told.ack,
        told.seq,
        PSH_ACK,
        &[0x5a],
    ));
    eventually("the TUN to answer the probe", WITHIN, || {
        let now = stalled.heard.lock().expect("what the TUN told");
        now.packets > said && now.ack == told.ack && now.window == 0
    })
    .await;
    // the upload takes up where it was, and the probe's byte is not in it
    stalled.finish(Counters::default()).await;
}

// ---- one shard, its turns taken by hand

pub(super) fn app4(port: u16) -> SocketAddr {
    sock(ADAPTER_V4, port)
}

/// A connection from `app` to `dst` made on `shard` by hand, at `ms` and a
/// millisecond later: the SYN, the SYN-ACK it is answered with, the app's ACK.
/// The connection comes back with the sequence number of the SYN-ACK.
pub(super) fn open_by_hand(
    shard: &Shard,
    epoch: Instant,
    app: SocketAddr,
    dst: SocketAddr,
    ms: u64,
) -> (Opened, u32) {
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let mut turn = shard.state().turn(at(ms), &mut vec![syn_v4(app, dst)]);
    let opened = turn.opened.pop().expect("the SYN opens a connection");
    let syn_ack = turn
        .out
        .iter()
        .find(|packet| is_syn(packet))
        .expect("the SYN-ACK");
    let isn = fields(syn_ack).expect("a TCP segment").seq;
    let ack = segment_v4(app, dst, 2, isn.wrapping_add(1), ACK);
    shard.state().turn(at(ms + 1), &mut vec![ack]);
    (opened, isn)
}

/// The app sends `bytes` bytes (0xaa) on a connection `open_by_hand` made: a
/// thousand to a segment, a turn of the shard for each, 20 ms apart (past the
/// delay of an acknowledgement), and a turn more for the acknowledgement of the
/// last. Returns what the TUN told on the way, and the time in ms of the last
/// turn.
fn send_data(
    lone: &Lone,
    epoch: Instant,
    app: SocketAddr,
    dst: SocketAddr,
    isn: u32,
    bytes: usize,
) -> (Told, u64) {
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let mut told = Told::default();
    let mut now = 2;
    let mut sent = 0;
    while sent < bytes {
        let piece = (bytes - sent).min(1000);
        let data = data_segment_v4(
            app,
            dst,
            2 + sent as u32,
            isn.wrapping_add(1),
            PSH_ACK,
            &vec![0xaa; piece],
        );
        now += 20;
        let turn = lone.shard.state().turn(at(now), &mut vec![data]);
        turn.out.iter().for_each(|packet| told.take(packet));
        sent += piece;
    }
    now += 20;
    let turn = lone.shard.state().turn(at(now), &mut Vec::new());
    turn.out.iter().for_each(|packet| told.take(packet));
    (told, now)
}

/// A probe at the edge of a window of 0, the window having opened since by a
/// read: the TUN's window update goes out before the probe is looked at, so
/// the probe is inside the new window, is taken in, and is acknowledged. Were
/// it looked at first, it would be past the edge the guard has (and, with no
/// guard, smoltcp would take the byte in and then find the window it last told
/// of ended before the data it holds, and panic).
#[tokio::test]
async fn a_probe_after_a_read_is_inside_the_new_window() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (app, dst) = (app4(40000), addr("192.0.2.1:80"));
    let (opened, isn) = open_by_hand(&lone.shard, epoch, app, dst, 0);
    let mut stream = TunStream::new(lone.shard.clone(), opened.id, opened.shared);
    within("the handshake", stream.established())
        .await
        .expect("the connection is established");
    // the app sends until the TUN has no room: the first socket of a TUN has
    // the largest receive buffer, RX_MAX
    let (told, mut now) = send_data(&lone, epoch, app, dst, isn, crate::RX_MAX);
    assert_eq!(told.window, 0, "the TUN has no room: {told:?}");
    assert_eq!(told.ack, 2 + crate::RX_MAX as u32, "it has all of it");
    // a read of a KiB: room, which the window update tells of
    within("the read", stream.read_exact(&mut [0; 1024]))
        .await
        .expect("a KiB to read");
    // and the probe is the first packet of the turn after it
    let probe = data_segment_v4(app, dst, told.ack, told.seq, PSH_ACK, &[0xab]);
    now += 20;
    let mut out = lone.shard.state().turn(at(now), &mut vec![probe]).out;
    now += 20;
    out.extend(lone.shard.state().turn(at(now), &mut Vec::new()).out);
    let read: Vec<Fields> = out.iter().filter_map(|packet| fields(packet)).collect();
    assert!(
        read.first()
            .is_some_and(|first| first.window == 1024 && first.ack == told.ack),
        "the window update goes first: {read:?}"
    );
    assert!(
        read.iter().any(|segment| segment.ack == told.ack + 1),
        "the probe's byte is acknowledged: {read:?}"
    );
    assert_eq!(lone.stats.counters(), Counters::default());
}

/// Data past the edge the TUN last told of, into a buffer with room past it
/// (which a read since has made): smoltcp takes in the part of a segment that
/// fits in the room, not what the window it told allowed (tcp.rs 1598-1599,
/// 1660-1672), and then finds the window it last told of ended before the data
/// it holds (693-701): the subtraction underflows and panics. The guard drops
/// the segment and counts it, for any room at all: a segment longer than the
/// room is dropped as one that fits is, for the part of it that fits would do
/// the same harm. A read of less than the window told does not make the stack
/// tell a new window, which it does only when the window at least doubles
/// (2232-2246), so the edge stays where it was.
#[tokio::test]
async fn a_segment_past_the_edge_into_room_is_dropped() {
    // the window the TUN last told when the app has sent all it can, what the
    // stream reads of it (so the room is that and more), a segment 500 bytes
    // past the edge, which is 500 bytes inside the room, and one 500 bytes
    // longer than the room
    const TOLD: usize = 4_000;
    const READ: usize = 1_000;
    const SEGMENT: usize = TOLD + 500;
    const LONGER: usize = TOLD + READ + 500;
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (app, dst) = (app4(40000), addr("192.0.2.1:80"));
    let (opened, isn) = open_by_hand(&lone.shard, epoch, app, dst, 0);
    let id = opened.id;
    let mut stream = TunStream::new(lone.shard.clone(), opened.id, opened.shared);
    within("the handshake", stream.established())
        .await
        .expect("the connection is established");
    let sent = crate::RX_MAX - TOLD;
    let (mut told, mut now) = send_data(&lone, epoch, app, dst, isn, sent);
    assert_eq!(
        told.window, TOLD as u32,
        "the TUN told of its room: {told:?}"
    );
    assert_eq!(told.ack, 2 + sent as u32, "it has all the app sent");
    within("the read", stream.read_exact(&mut [0; READ]))
        .await
        .expect("bytes to read");
    now += 20;
    let edge = told.edge();
    let quiet = lone.shard.state().turn(at(now), &mut Vec::new());
    quiet.out.iter().for_each(|packet| told.take(packet));
    assert_eq!(told.edge(), edge, "no segment tells of a new edge");
    let segment = data_segment_v4(app, dst, told.ack, told.seq, PSH_ACK, &[0xee; SEGMENT]);
    now += 20;
    lone.shard.state().turn(at(now), &mut vec![segment]);
    assert_eq!(
        lone.stats.counters(),
        Counters {
            beyond_window: 1,
            ..Counters::default()
        }
    );
    // the buffer holds what the app sent, and nothing of the segment
    let queued = |lone: &Lone| {
        let state = lone.shard.state();
        let socket = state.sockets.get::<tcp::Socket>(state.conns[&id].handle);
        socket.recv_queue()
    };
    assert_eq!(queued(&lone), sent - READ);
    // a segment longer than the room, in a turn of its own: the stack would
    // take in the room's worth of it, and the end of that is past the edge
    let longer = data_segment_v4(app, dst, told.ack, told.seq, PSH_ACK, &[0xdd; LONGER]);
    now += 20;
    lone.shard.state().turn(at(now), &mut vec![longer]);
    assert_eq!(
        lone.stats.counters(),
        Counters {
            beyond_window: 2,
            ..Counters::default()
        }
    );
    assert_eq!(queued(&lone), sent - READ);
    let mut rest = vec![0; sent - READ];
    within("the rest", stream.read_exact(&mut rest))
        .await
        .expect("the rest to read");
    assert!(
        rest.iter().all(|&byte| byte == 0xaa),
        "only what the app sent"
    );
    assert_eq!(queued(&lone), 0);
}

/// The edge goes with the connection that has the tuple. Were a connection let
/// go of under another that has taken its tuple, the old one's going would take
/// the new one's edge with it, and the guard would know nothing of the new
/// connection.
#[test]
fn a_connection_that_does_not_have_its_tuple_leaves_the_edge_to_the_one_that_does() {
    let epoch = Instant::now();
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (app, dst) = (app4(40000), addr("192.0.2.1:80"));
    open_by_hand(&lone.shard, epoch, app, dst, 0);
    let tuple = Tuple { src: app, dst };
    lone.shard.state().by_tuple.insert(tuple, ConnId::next());
    lone.shard.state().end_all();
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.shard.state().edge_count(), 1, "the edge stays");
    assert!(lone.shard.state().by_tuple.contains_key(&tuple));
}

#[test]
fn a_connection_that_ends_leaves_no_edge() {
    let epoch = Instant::now();
    let at = |ms: u64| epoch + Duration::from_millis(ms);
    let lone = lone_shard(&TunOptions::default(), epoch);
    let (app, dst) = (app4(40000), addr("192.0.2.1:80"));
    let (_, isn) = open_by_hand(&lone.shard, epoch, app, dst, 0);
    assert_eq!(lone.shard.state().edge_count(), 1);
    // the app resets it: the shard lets it go in the same turn
    lone.shard.state().turn(
        at(5),
        &mut vec![segment_v4(app, dst, 2, isn.wrapping_add(1), RST)],
    );
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.shard.state().edge_count(), 0);
    // a connection a reset in SYN-RECEIVED took leaves none either
    lone.shard.state().turn(at(10), &mut vec![syn_v4(app, dst)]);
    assert_eq!(lone.shard.state().edge_count(), 1);
    lone.shard
        .state()
        .turn(at(11), &mut vec![segment_v4(app, dst, 2, 0, RST)]);
    assert_eq!(lone.shard.state().socket_count(), 0);
    assert_eq!(lone.shard.state().edge_count(), 0);
}
