//! Which sessions are answered "connected" before anything is read, and what
//! happens to one that is (phase 4 M1 design 4, 7): the judgement the engine
//! makes for all three entries, the probe of pre-matching, the fake addresses
//! that come in at a proxy port, and a TLS session whose rule matches the SNI
//! its ClientHello carried.
//!
//! Nothing here goes beyond the loopback: the TLS echo server and the fake
//! SOCKS5 upstreams are loopback servers, and the table of the first test reads
//! no setting that would send anything anywhere.

mod common;
use common::tun::*;
use common::*;
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{SessionInfo, Transport};
use rurge_engine::Mode;
use rurge_inbound::Dialer;
use std::net::IpAddr;

use ListenerKind::{Forward, Http, Internal, Socks5, Tun};
use Transport::{Tcp, Udp};

/// Every condition of design 4.1 at a time, at the three entries.
#[tokio::test]
async fn which_sessions_are_sniffable() {
    let h = harness(Profile {
        general: "force-http-engine-hosts = forced.test, *.anyport.test:0\n\
always-raw-tcp-hosts = raw.test\n\
always-raw-tcp-keywords = RawKw",
        ..Profile::default()
    })
    .await;
    let cases: &[(ListenerKind, Transport, &str, u16, bool)] = &[
        // the ports of the design, at a proxy port
        (Socks5, Tcp, "target.test", 443, true),
        (Socks5, Tcp, "target.test", 80, true),
        (Socks5, Tcp, "target.test", 8443, false),
        // `force-http-engine-hosts` has 80 for a default port
        (Http, Tcp, "forced.test", 8080, false),
        (Socks5, Tcp, "a.anyport.test", 8080, true),
        // `always-raw-tcp-hosts` comes before everything
        (Tun, Tcp, "raw.test", 443, false),
        // and so do the keywords, in any case
        (Socks5, Tcp, "xrawkwx.test", 443, false),
        // an address is not a hostname: no keyword is looked for in it
        (Socks5, Tcp, "10.1.2.3", 443, true),
        // only TCP, and only the clients' entries
        (Socks5, Udp, "target.test", 443, false),
        (Internal, Tcp, "target.test", 443, false),
        (Forward, Tcp, "target.test", 443, false),
    ];
    for &(listener, transport, host, port, expected) in cases {
        let session = SessionInfo {
            listener,
            transport,
            ..SessionInfo::tcp(HostName::parse(host), port)
        };
        assert_eq!(
            h.engine.sniffable(&session),
            expected,
            "{listener:?} {transport:?} {host}:{port}"
        );
    }
}

/// A paused engine refuses its sessions before it answers them, as it does
/// today (design 4.1, 2).
#[tokio::test]
async fn a_paused_engine_sniffs_nothing() {
    let h = harness(Profile::default()).await;
    let session = SessionInfo {
        listener: ListenerKind::Socks5,
        ..SessionInfo::tcp(HostName::parse("target.test"), 443)
    };
    assert!(h.engine.sniffable(&session));
    h.engine.suspend(Some("strict".into()));
    assert!(
        !h.engine.sniffable(&session),
        "a refused session keeps today's answer"
    );
    h.engine.suspend(None);
    assert!(h.engine.sniffable(&session));
}

/// A session that pre-matching refuses is not sniffed at the proxy ports, so
/// the refusal still comes before the answer (design 4.3).
#[tokio::test]
async fn a_session_that_pre_matching_refuses_is_not_sniffed() {
    let h = harness(Profile {
        rules: "DOMAIN,ads.test,REJECT,pre-matching",
        ..Profile::default()
    })
    .await;
    let at = |listener| SessionInfo {
        listener,
        ..SessionInfo::tcp(HostName::parse("ads.test"), 443)
    };
    assert!(
        !h.engine.sniffable(&at(ListenerKind::Socks5)),
        "the probe refuses it before the answer"
    );
    assert!(
        h.engine.sniffable(&at(ListenerKind::Tun)),
        "the adapter's sessions asked pre-matching at the SYN"
    );
    h.engine.set_mode(Mode::Direct).await;
    assert!(
        h.engine.sniffable(&at(ListenerKind::Socks5)),
        "direct mode never reaches the rules"
    );
}

/// An address of rurge's space at a proxy port is judged by the name behind
/// it, and one that no name is behind is not sniffed at all (design 4.1, plan
/// P5).
#[tokio::test]
async fn fake_addresses_at_a_proxy_port_are_judged_by_their_names() {
    let rig = Rig::new(Profile {
        general: "always-raw-tcp-hosts = raw.test",
        rules: &guarded(""),
        ..Profile::default()
    })
    .await;
    let (raw, _) = rig.fakes("raw.test");
    let (target, _) = rig.fakes("target.test");
    let at = |ip: IpAddr| SessionInfo {
        listener: ListenerKind::Socks5,
        ..SessionInfo::tcp(HostName::Ip(ip), 443)
    };
    assert!(
        !rig.h.engine.sniffable(&at(raw)),
        "the name behind {raw} is raw.test"
    );
    assert!(
        rig.h.engine.sniffable(&at(target)),
        "the name behind {target} is target.test"
    );
    let unknown: IpAddr = "198.19.255.254".parse().unwrap();
    assert!(
        !rig.h.engine.sniffable(&at(unknown)),
        "no name stands behind {unknown}"
    );
}

/// The ClientHello's SNI is there before the rules are asked: a rule with
/// `extended-matching` takes a session whose destination is another name
/// (design 4.1, 7.2).
#[tokio::test]
async fn extended_matching_rules_a_tls_session_by_its_sni() {
    let fixture = TlsFixture::new(&["sni.test"]);
    let echo = fixture.spawn_echo(false).await;
    let script = || Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    };
    let a = FakeSocks5::spawn(script()).await;
    let b = FakeSocks5::spawn(script()).await;
    let h = harness(Profile {
        proxies: &format!(
            "A = socks5, 127.0.0.1, {}\nB = socks5, 127.0.0.1, {}",
            a.addr().port(),
            b.addr().port()
        ),
        rules: "DOMAIN,sni.test,B,extended-matching\nDOMAIN,target.test,A",
        ..Profile::default()
    })
    .await;
    let s = connect_via_socks5(h.socks(), "target.test", 443).await;
    let config = rurge_net::http::tls_client_config(true).unwrap();
    let name = rustls::pki_types::ServerName::try_from("sni.test".to_string()).unwrap();
    let mut tls = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_rustls::TlsConnector::from(config).connect(name, s),
    )
    .await
    .expect("the handshake ends within the bound")
    .expect("the tunnel carries the handshake");
    tokio::time::timeout(Duration::from_secs(5), tls.write_all(b"ping"))
        .await
        .expect("the echo is written within the bound")
        .unwrap();
    let mut back = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), tls.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(&back, b"ping");
    drop(tls);

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(
        record.rule.as_deref(),
        Some("DOMAIN,sni.test,B,extended-matching")
    );
    assert_eq!(record.policy, ["B"]);
    assert_eq!(record.sni.as_deref(), Some("sni.test"));
    assert_eq!(record.protocol, Some(ProtocolKind::Https));
    assert_eq!(record.dst, "target.test:443");

    let asked = b.requests();
    assert_eq!(asked.len(), 1, "only the policy the SNI chose");
    assert_eq!(
        (asked[0].command, asked[0].host.as_str(), asked[0].port),
        (1, "target.test", 443)
    );
    assert!(
        a.requests().is_empty(),
        "the destination's policy is not asked"
    );

    let seen = fixture.seen_at_least(1).await;
    assert_eq!(
        seen[0].sni.as_deref(),
        Some("sni.test"),
        "the client's SNI reaches the far end as it was"
    );
}
