//! Sessions that leave through the `wireguard` outbound: profile text →
//! Runtime → Engine → loopback listeners → `FakeWgPeer`, whose own stack
//! echoes on port 7 of every address in the tunnel (phase 2 M4 design §6).

mod common;

use common::*;
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_inbound::{DialError, Dialer};

/// A peer, and the section of policy `WG = wireguard, section-name=w` that
/// reaches it.
async fn peer() -> (FakeWgPeer, String) {
    let (private, public) = keypair();
    let peer = FakeWgPeer::start(public, PeerOpts::default()).await;
    let section = section_text("w", &private, &peer);
    (peer, section)
}

const WG: &str = "WG = wireguard, section-name=w";
const TO_WG: &str = "IP-CIDR,10.0.0.0/8,WG";

fn echo_addr() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 1], ECHO_PORT))
}

#[tokio::test]
async fn a_connect_leaves_through_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut tunnel, b"through the tunnel").await;
    assert_eq!(peer.core().connected_to, [echo_addr()]);
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["WG"]);
}

/// Without a `dns-server`, a destination name is resolved on this machine,
/// `[Host]` included (M4-D9); the tunnel carries the address.
#[tokio::test]
async fn a_name_is_resolved_on_this_machine_with_host_items() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        hosts: "echo.test = 10.0.0.1",
        rules: "DOMAIN,echo.test,WG",
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "echo.test:7").await;
    echo_through(&mut tunnel, b"by name").await;
    assert_eq!(peer.core().connected_to, [echo_addr()]);
    assert!(h.dns.queries().is_empty(), "[Host] answered");
}

/// The peers are reached through the `underlying-proxy`: here a SOCKS5
/// proxy's UDP ASSOCIATE (phase 2 M5 design 8.1).
#[tokio::test]
async fn a_tunnel_goes_over_an_underlying_socks5_proxy() {
    let (peer, section) = peer().await;
    let up = FakeSocks5::spawn(Socks5Script::default()).await;
    let proxies = format!(
        "WG = wireguard, section-name=w, underlying-proxy=Up\nUp = socks5, 127.0.0.1, {}, udp-relay=true",
        up.addr().port()
    );
    let h = harness(Profile {
        proxies: &proxies,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut tunnel, b"over the chain").await;
    assert_eq!(peer.core().connected_to, [echo_addr()]);
    let peer_at =
        rurge_net::connector::Target::new(HostName::parse("127.0.0.1"), peer.addr().port());
    assert!(
        up.datagrams().iter().all(|to| *to == peer_at),
        "every datagram went to the peer through the proxy"
    );
    assert!(!up.datagrams().is_empty());
}

/// An `underlying-proxy` that carries no UDP fails the dial, saying so, and
/// never goes around the chain (M4-D7).
#[tokio::test]
async fn a_tunnel_over_an_underlying_proxy_without_udp_fails_saying_so() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, underlying-proxy=Up\nUp = http, 127.0.0.1, 9",
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let session = SessionInfo::tcp(HostName::parse("10.0.0.1"), ECHO_PORT);
    match h.engine.dial(session).await {
        Err(DialError::Failed { message, .. }) => {
            assert_eq!(message, "via Up: the underlying policy cannot carry UDP");
        }
        Err(DialError::Reject { .. }) => panic!("expected a failure, got a reject"),
        Ok(_) => panic!("expected a failure, got a stream"),
    }
    assert!(peer.clients().is_empty(), "nothing went to the peer");
}

/// Two policies naming one section share its tunnel only when their
/// carriers are alike: the one over an `underlying-proxy` without UDP
/// fails, and the tunnel the other one started goes on (M4-D7).
#[tokio::test]
async fn a_policy_over_underlying_proxy_never_shares_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w\n\
                  WG-Up = wireguard, section-name=w, underlying-proxy=Up\n\
                  Up = socks5, 127.0.0.1, 9",
        rules: "IP-CIDR,10.0.0.2/32,WG-Up\nIP-CIDR,10.0.0.0/8,WG",
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut tunnel, b"direct").await;
    let session = SessionInfo::tcp(HostName::parse("10.0.0.2"), ECHO_PORT);
    match h.engine.dial(session).await {
        Err(DialError::Failed { message, .. }) => {
            assert_eq!(message, "via Up: the underlying policy cannot carry UDP")
        }
        Err(DialError::Reject { .. }) => panic!("expected a failure, got a reject"),
        Ok(_) => panic!("expected a failure, got a stream"),
    }
    echo_through(&mut tunnel, b"still").await;
    assert_eq!(peer.core().handshakes, 1);
}

/// A reload that changes what the carriers go through (here `ip-version`)
/// builds the policy anew, and its tunnel takes over from the old one.
#[tokio::test]
async fn a_reload_that_changes_the_carriers_takes_the_tunnel_over() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut first = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut first, b"one").await;
    let changed = Profile {
        proxies: "WG = wireguard, section-name=w, ip-version=v4-only",
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &changed, h.engine.shared()).await);
    let mut second = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut second, b"two").await;
    assert_eq!(
        peer.core().handshakes,
        2,
        "a tunnel of the new configuration"
    );
}

/// Without a `dns-server` or a `test-url`, a test of the policy is a
/// handshake with its peers, which the session log shows going to the
/// first peer (phase 2 M4 design 6.7).
#[tokio::test]
async fn a_policy_test_is_a_handshake_with_the_peers() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let results = h
        .engine
        .test_policies(&["WG".to_string()], None)
        .await
        .unwrap();
    let result = results[0].1.as_ref().expect("a wireguard policy is tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    assert!(peer.core().handshakes >= 1);
    let log = h.engine.request_log();
    let test = || {
        log.recent(10)
            .into_iter()
            .find(|r| r.rule.as_deref() == Some("policy test"))
    };
    wait_until("the test session", || test().is_some()).await;
    assert_eq!(test().unwrap().dst, peer.addr().to_string());
}

/// With a `test-url`, the test fetches it through the tunnel.
#[tokio::test]
async fn a_test_url_is_fetched_through_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, test-url=http://10.0.0.1/",
        sections: &section,
        ..Profile::default()
    })
    .await;
    let results = h
        .engine
        .test_policies(&["WG".to_string()], None)
        .await
        .unwrap();
    let result = results[0].1.as_ref().expect("a wireguard policy is tested");
    assert!(result.outcome.is_ok(), "{:?}", result.outcome);
    assert_eq!(peer.core().http_requests, 2, "two HEADs, one connection");
}

/// A reload that leaves the line and its section alone keeps the tunnel,
/// handshake and all; an edited section builds the policy anew.
#[tokio::test]
async fn a_reload_keeps_the_tunnel_unless_its_section_changes() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let mut first = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut first, b"one").await;
    let before = outbound_now(&h, "WG");

    let proxies = format!("{WG}\nOther = http, other.example, 8080");
    let unrelated = Profile {
        proxies: &proxies,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &unrelated, h.engine.shared()).await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "WG")),
        "WG was rebuilt"
    );
    let mut second = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut second, b"two").await;
    assert_eq!(peer.core().handshakes, 1, "the same tunnel");

    let edited = format!("{section}mtu = 1400\n");
    let next = Profile {
        proxies: &proxies,
        rules: TO_WG,
        sections: &edited,
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(
        !Arc::ptr_eq(&before, &outbound_now(&h, "WG")),
        "WG was kept"
    );
    let mut third = connect_via_http(h.http(), "10.0.0.1:7").await;
    echo_through(&mut third, b"three").await;
    assert_eq!(peer.core().handshakes, 2, "a tunnel of its own");
}

/// UDP from a SOCKS5 association through the tunnel to the peer's echo, and
/// back; a host behind the peer that was never written to reaches the
/// client too (full cone, phase 2 M5 design §7).
#[tokio::test]
async fn udp_leaves_through_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: WG,
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("10.0.0.1", ECHO_PORT, b"through").await;
    assert_eq!(association.recv().await, (echo_addr(), b"through".to_vec()));
    let SocketAddr::V4(ours) = peer.core().udp_echoed[0].0 else {
        panic!("the tunnel is IPv4")
    };
    peer.send_udp("10.0.0.9:5000".parse().unwrap(), ours, b"unasked")
        .await;
    assert_eq!(
        association.recv().await,
        (SocketAddr::from(([10, 0, 0, 9], 5000)), b"unasked".to_vec())
    );
    drop(association);
    let log = h.engine.request_log();
    wait_until("the flow to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["WG"]);
}

/// The UDP test (`test-udp`) asks its question through the tunnel; a policy
/// without UDP, or without a UDP test, has none (phase 2 M5 design 8.4).
#[tokio::test]
async fn a_udp_test_asks_through_the_tunnel() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, test-udp=echo.test@10.0.0.53
H = http, 127.0.0.1, 9",
        sections: &section,
        ..Profile::default()
    })
    .await;
    let names = ["WG", "H", "DIRECT"].map(String::from);
    let results = h.engine.test_udp(&names).await;
    let outcome = |name: &str| results.iter().find(|(n, _)| n == name).unwrap().1.clone();
    assert!(matches!(outcome("WG"), Some(Ok(_))), "{:?}", outcome("WG"));
    assert_eq!(outcome("H"), None, "an http proxy carries no UDP");
    assert_eq!(outcome("DIRECT"), None, "no proxy-test-udp");
    assert_eq!(peer.core().dns_questions, ["echo.test A"]);
}
