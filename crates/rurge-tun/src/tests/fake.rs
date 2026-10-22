//! TCP to the addresses that stand for names (phase 3 M3 design 5.2, 10.2 and
//! 10.4): a connection to a fake address is dialed as the name behind it, and
//! keeps the address for its name for as long as it lives; one to an address
//! the pool does not know is reset.

use super::support::{Extras, WITHIN, eventually, hello, rig, rig_extras, sock, within};
use crate::testing::{Script, Upstream};
use crate::{Counters, TunOptions};
use rurge_config::{HostName, ListenerKind, Transport};
use rurge_dns::fakeip::{FakeIpPool, v4_of, v6_of};
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

/// A connection to either address of a name is dialed as the name, and
/// remembers the address it was made to: the name is what the engine's rules
/// and policies work with, the address is the endpoint the app connected to.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_to_a_fake_address_dials_its_name() {
    let rig = rig(TunOptions::default());
    let pool = rig.tun.pool().expect("the rig's TUN has a pool");
    let index = pool
        .assign("a.test", Instant::now())
        .expect("an address for the name");
    let echo = Upstream::echo().await;
    let fakes = [sock(v4_of(index), 80), sock(v6_of(index), 80)];
    for fake in fakes {
        // the dialer is scripted by the endpoint the app connected to
        rig.dialer.script(fake, Script::Upstream(echo.addr()));
        let mut tcp = within("the connection to be made", rig.app.connect(fake))
            .await
            .expect("connect");
        hello(&mut tcp).await;
    }
    let dialed = rig.dialer.dialed();
    assert_eq!(
        dialed.len(),
        2,
        "one session for each connection: {dialed:?}"
    );
    for (session, fake) in dialed.iter().zip(fakes) {
        assert_eq!(session.dst_host, HostName::Domain("a.test".to_owned()));
        assert_eq!(session.dst_port, 80);
        assert_eq!(
            session.local,
            Some(fake),
            "the endpoint the app connected to"
        );
        assert_eq!(session.listener, ListenerKind::Tun);
        assert_eq!(session.transport, Transport::Tcp);
    }
    assert_eq!(rig.tun.counters(), Counters::default());
}

/// A connection that stays open keeps its address: names that come and go
/// meanwhile, enough of them to take every address that can be taken, are not
/// given it, whatever else they are given. The address is the pool's to hand
/// out again once the connection is over.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connection_holds_its_name_for_its_life() {
    // two addresses in all: the connection's, and the one every other name
    // takes in turn
    let pool = Arc::new(FakeIpPool::with_count(2));
    let (rig, _) = rig_extras(
        TunOptions::default(),
        Extras {
            pool: Some(pool.clone()),
            ..Extras::default()
        },
    );
    let echo = Upstream::echo().await;
    let now = Instant::now();
    let a = pool.assign("a.test", now).expect("an address for a.test");
    let a_address = IpAddr::V4(v4_of(a));
    let fake = sock(v4_of(a), 80);
    rig.dialer.script(fake, Script::Upstream(echo.addr()));
    let mut tcp = within("the connection to be made", rig.app.connect(fake))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    for i in 0..20 {
        let other = pool
            .assign(&format!("n{i}.test"), now)
            .expect("an address for another name");
        assert_ne!(
            other, a,
            "n{i}.test took the address of the open connection"
        );
        // not `name_of`, which uses the address and so keeps it from being the
        // one used longest ago: this must not be what holds it back
        assert_eq!(
            pool.address_of("a.test", false),
            Some(a_address),
            "after n{i}.test"
        );
    }
    assert_eq!(pool.name_of(a_address).as_deref(), Some("a.test"));
    // the connection goes on as it did
    hello(&mut tcp).await;
    assert_eq!(rig.dialer.dialed().len(), 1);
    drop(tcp);
    eventually("the session to end and its socket to go", WITHIN, || {
        rig.tun.session_tasks() == 0 && rig.tun.sockets_in_use() == 0
    })
    .await;
    // the address is free again, and the one used longest ago goes first: the
    // other address, then this
    let taken = [
        pool.assign("x.test", now).expect("an address for x.test"),
        pool.assign("y.test", now).expect("an address for y.test"),
    ];
    assert!(taken.contains(&a), "the address was given back: {taken:?}");
    assert_eq!(pool.address_of("a.test", false), None);
}

/// An address of the pool's space that no name has is refused as a SYN to
/// rurge's own addresses is, and counted (`unknown_fake`); nothing is dialed.
/// So is every fake address when the TUN has no pool.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_address_the_pool_does_not_know_is_reset() {
    for pool in [Some(Arc::new(FakeIpPool::new())), None] {
        let had_pool = pool.is_some();
        let (rig, _) = rig_extras(
            TunOptions::default(),
            Extras {
                pool,
                ..Extras::default()
            },
        );
        for (refused, fake) in [sock(v4_of(7), 80), sock(v6_of(7), 80)]
            .into_iter()
            .enumerate()
        {
            let error = within("the connection to be refused", rig.app.connect(fake))
                .await
                .expect_err("a connection to a name nobody knows is refused");
            assert_eq!(
                error.kind(),
                io::ErrorKind::ConnectionRefused,
                "{fake} (a pool: {had_pool})"
            );
            assert_eq!(rig.tun.counters().unknown_fake, refused as u64 + 1);
        }
        assert!(rig.dialer.dialed().is_empty(), "nothing is dialed");
        assert_eq!(
            rig.tun.counters(),
            Counters {
                unknown_fake: 2,
                ..Counters::default()
            }
        );
        assert_eq!(rig.tun.sockets_in_use(), 0);
        assert_eq!(rig.tun.sockets_counted(), 0);
        assert_eq!(rig.tun.half_open(), 0);
    }
}
