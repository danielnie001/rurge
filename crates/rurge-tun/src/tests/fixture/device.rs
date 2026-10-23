//! The memory device, and the ways around the app stack: what is diverted or
//! injected, what a filter sees and changes, and what a device does when the
//! other end of it goes away.

use super::{connected, stacks};
use crate::device::{Device, MemoryDevice};
use crate::testing::{AppStack, Direction};
use crate::tests::support::{WITHIN, addr, eventually, syn_v4, tcp_payload_len, udp_v4, within};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

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
