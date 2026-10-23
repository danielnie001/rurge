//! The tests of `ScriptedTcp`, the client B2 opens its ten thousand
//! connections with.

use crate::device::AppEnd;
use crate::testing::{AppStack, Script, ScriptedTcp};
use crate::tests::support::{
    WITHIN, addr, checksum, data_segment_v4, eventually, is_syn, rig, segment_v4, syn_v4,
    tcp_header, tcp_payload_len, within,
};
use crate::{Counters, TunOptions};
use rurge_config::{HostName, ListenerKind};
use std::io;

/// A hundred connections of a `ScriptedTcp` through a TUN to far ends that
/// send 8 KiB each: every one is dialed, every hello is read, and `take`
/// returns once every connection has had its 8 KiB.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_tcp_opens_and_takes() {
    let rig = rig(TunOptions::default());
    let dst = addr("192.0.2.10:80");
    rig.dialer.script(
        dst,
        Script::Memory {
            buffer: 1024,
            send: 8192,
        },
    );
    let ports = 10_000..=10_099;
    let mut scripted = ScriptedTcp::new(&rig.app, ports.clone());
    within(
        "a hundred connections to open",
        scripted.open(ports.clone(), dst, 1024),
    )
    .await
    .expect("open");
    within("every connection to take its 8 KiB", scripted.take(8192))
        .await
        .expect("take");
    let dialed = rig.dialer.dialed();
    assert_eq!(dialed.len(), 100, "a session for each connection");
    let mut sources: Vec<u16> = dialed.iter().map(|session| session.src.port()).collect();
    sources.sort_unstable();
    assert_eq!(sources, ports.collect::<Vec<_>>(), "each source port once");
    for session in &dialed {
        assert_eq!(session.src.ip(), addr("198.18.0.1:0").ip());
        assert_eq!(session.dst_host, HostName::Ip(dst.ip()));
        assert_eq!(session.dst_port, 80);
        assert_eq!(session.listener, ListenerKind::Tun);
    }
    // the hellos arrived: the far ends have read them
    eventually("the far ends to read every hello", WITHIN, || {
        rig.dialer.received(dst) == 100 * 1024
    })
    .await;
    assert_eq!(rig.tun.counters(), Counters::default());
}

fn seq_number(packet: &[u8]) -> u32 {
    let (header, _) = tcp_header(packet).expect("a TCP segment");
    u32::from_be_bytes(
        packet[header + 4..header + 8]
            .try_into()
            .expect("four bytes"),
    )
}

fn ack_number(packet: &[u8]) -> u32 {
    let (header, _) = tcp_header(packet).expect("a TCP segment");
    u32::from_be_bytes(
        packet[header + 8..header + 12]
            .try_into()
            .expect("four bytes"),
    )
}

/// The bytes a `ScriptedTcp` has taken are how far the stream got, not the sum
/// of the payloads that came: a segment sent twice and a keep-alive probe (one
/// byte, one below the next expected) carry bytes that are not new. A TUN
/// driven by hand sends them, and what comes back to each is an ACK of where
/// the stream is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_tcp_counts_bytes_by_sequence_number() {
    const PEER: u32 = 5_000;
    let (end, mut tun) = AppEnd::crossed(1500, 64);
    let stack = AppStack::start(end);
    let mut scripted = ScriptedTcp::new(&stack, 10_000..=10_000);
    let (app, dst) = (addr("198.18.0.1:10000"), addr("192.0.2.10:80"));
    // the handshake: a SYN with the MSS and no other option, answered; the
    // ACK of the answer carries the hello; `open` returns once that is answered
    let mut opening = Box::pin(scripted.open(10_000..=10_000, dst, 5));
    let hello_in = async {
        let syn = within("the SYN", tun.recv()).await.expect("a packet");
        assert!(is_syn(&syn));
        assert_eq!(checksum(&syn[..20]), 0, "the IPv4 header checksum is right");
        let (header, _) = tcp_header(&syn).expect("a TCP segment");
        assert_eq!(syn[header + 12] >> 4, 6, "a header of 24 bytes");
        assert_eq!(
            syn[header + 20..header + 24],
            [2, 4, 0x05, 0xb4],
            "the MSS, 1460, and no other option"
        );
        let isn = seq_number(&syn);
        tun.send(segment_v4(dst, app, PEER, isn.wrapping_add(1), 0x12))
            .await
            .expect("send the SYN-ACK");
        let hello = within("the hello", tun.recv()).await.expect("a packet");
        assert_eq!(tcp_payload_len(&hello), 5);
        assert_eq!(seq_number(&hello), isn.wrapping_add(1));
        assert_eq!(ack_number(&hello), PEER + 1);
        isn
    };
    let isn = tokio::select! {
        biased;
        opened = &mut opening => panic!("open returned before its hello was acknowledged: {opened:?}"),
        isn = hello_in => isn,
    };
    let (opened, ()) = tokio::join!(within("the connection to open", &mut opening), async {
        tun.send(segment_v4(dst, app, PEER + 1, isn.wrapping_add(6), 0x10))
            .await
            .expect("send the ACK of the hello");
    });
    opened.expect("open");
    drop(opening);

    let thousand = vec![7u8; 1000];
    let first = PEER + 1;
    let data = |seq: u32| data_segment_v4(dst, app, seq, isn.wrapping_add(6), 0x18, &thousand);
    // one thousand bytes, the same again, a keep-alive probe: 2001 bytes of
    // payload in all, 1000 of them new
    let mut taking = Box::pin(scripted.take(2000));
    let script = async {
        let probe = data_segment_v4(dst, app, first + 999, isn.wrapping_add(6), 0x10, &[0]);
        for segment in [data(first), data(first), probe] {
            tun.send(segment).await.expect("send");
            let ack = within("an ACK", tun.recv()).await.expect("a packet");
            assert_eq!(tcp_payload_len(&ack), 0, "a plain ACK");
            assert_eq!(ack_number(&ack), first + 1000, "of where the stream is");
        }
    };
    tokio::select! {
        biased;
        taken = &mut taking => panic!("2000 bytes taken after 1000 and two repeats: {taken:?}"),
        () = script => {}
    }
    // the next thousand completes it
    let (taken, ack) = tokio::join!(within("2000 bytes to be taken", &mut taking), async {
        tun.send(data(first + 1000)).await.expect("send");
        within("an ACK", tun.recv()).await.expect("a packet")
    });
    taken.expect("take");
    assert_eq!(ack_number(&ack), first + 2000);
}

/// A FIN takes a sequence number but is not a byte of the stream: a segment of
/// a thousand bytes and a FIN has the connection take a thousand, and is
/// acknowledged at a thousand and one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripted_tcp_does_not_count_a_fin_as_a_byte() {
    const PEER: u32 = 5_000;
    let (end, mut tun) = AppEnd::crossed(1500, 64);
    let stack = AppStack::start(end);
    let mut scripted = ScriptedTcp::new(&stack, 10_000..=10_000);
    let (app, dst) = (addr("198.18.0.1:10000"), addr("192.0.2.10:80"));
    // an established connection, with no hello to acknowledge
    let answering = async {
        let syn = within("the SYN", tun.recv()).await.expect("a packet");
        let isn = seq_number(&syn);
        tun.send(segment_v4(dst, app, PEER, isn.wrapping_add(1), 0x12))
            .await
            .expect("send the SYN-ACK");
        // the ACK that ends the handshake, with nothing in it
        let ack = within("the ACK of the SYN-ACK", tun.recv())
            .await
            .expect("a packet");
        assert_eq!(tcp_payload_len(&ack), 0);
        assert_eq!(ack_number(&ack), PEER + 1);
        isn
    };
    let (opened, isn) = tokio::join!(
        within(
            "the connection to open",
            scripted.open(10_000..=10_000, dst, 0)
        ),
        answering
    );
    opened.expect("open");

    let first = PEER + 1;
    // FIN, PSH and ACK
    let last = data_segment_v4(dst, app, first, isn.wrapping_add(1), 0x19, &[7u8; 1000]);
    let mut taking = Box::pin(scripted.take(1001));
    let script = async {
        tun.send(last).await.expect("send");
        let ack = within("an ACK", tun.recv()).await.expect("a packet");
        assert_eq!(ack_number(&ack), first + 1001, "the ACK covers the FIN");
    };
    tokio::select! {
        biased;
        taken = &mut taking => panic!("1001 bytes taken after a thousand and a FIN: {taken:?}"),
        () = script => {}
    }
    drop(taking);
    within("a thousand bytes to be taken", scripted.take(1000))
        .await
        .expect("take");
}

/// A range with a port in it that cannot be opened is refused whole: nothing is
/// registered, and no SYN is sent for the ports before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scripted_tcp_checks_every_port_before_it_sends_one() {
    let (end, mut tun) = AppEnd::crossed(1500, 64);
    let stack = AppStack::start(end);
    let mut scripted = ScriptedTcp::new(&stack, 10_000..=10_001);
    let dst = addr("192.0.2.10:80");
    // 10002 is not one of the ports it was given
    let error = scripted
        .open(10_000..=10_002, dst, 0)
        .await
        .expect_err("a port it was not given");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    // a table that has taken no memory: `HashMap::new` allocates at the first
    // insert (std does not promise it, so this can only fail red)
    assert_eq!(scripted.table_bytes(), 0, "nothing is registered");
    // what is injected goes out in order: the other end gets this packet
    // first, so no SYN went before it
    let marker = syn_v4(addr("198.18.0.1:50000"), dst);
    stack.inject(marker.clone());
    assert_eq!(within("a packet", tun.recv()).await, Some(marker));
}
