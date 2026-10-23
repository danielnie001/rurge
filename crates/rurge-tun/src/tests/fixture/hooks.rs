//! The fake `Hooks`, the fake resolver and the fake pinger.

use crate::testing::{FakeHooks, FakePinger, FakeResolve, PingScript};
use crate::tests::support::within;
use crate::{Hooks, Pinger, TunSettings};
use rurge_net::connector::Resolve;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tokio::time::timeout;

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
