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

/// No UDP through a chain before M5: the policy rejects and says why, and
/// never goes around the chain (M4-D7).
#[tokio::test]
async fn a_tunnel_over_underlying_proxy_rejects_with_a_note() {
    let (peer, section) = peer().await;
    let h = harness(Profile {
        proxies: "WG = wireguard, section-name=w, underlying-proxy=Up\nUp = socks5, 127.0.0.1, 9",
        rules: TO_WG,
        sections: &section,
        ..Profile::default()
    })
    .await;
    let session = SessionInfo::tcp(HostName::parse("10.0.0.1"), ECHO_PORT);
    match h.engine.dial(session).await {
        Err(DialError::Reject { kind, handle, .. }) => {
            assert_eq!(kind, rurge_proto::RejectKind::Reject);
            assert_eq!(
                handle.error().as_deref(),
                Some("policy protocol not implemented: wireguard over underlying-proxy")
            );
        }
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
    }
    assert!(peer.clients().is_empty(), "nothing went to the peer");
}

/// Two policies naming one section share its tunnel only when their
/// carriers are alike: the one over `underlying-proxy` still rejects, and
/// the tunnel the other one started goes on (M4-D7).
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
        Err(DialError::Reject { handle, .. }) => assert_eq!(
            handle.error().as_deref(),
            Some("policy protocol not implemented: wireguard over underlying-proxy")
        ),
        Err(DialError::Failed { message, .. }) => panic!("expected a reject, failed: {message}"),
        Ok(_) => panic!("expected a reject, got a stream"),
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
