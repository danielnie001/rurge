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
use rurge_engine::{HttpCounts, Mode, SniffedCounts};
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

/// How long the sniffs of these tests wait for the first bytes: short, so
/// that a session that says nothing ends in a moment rather than after the
/// 15 seconds of `SNIFF_WAIT`.
fn sniffing() -> EngineShared {
    EngineShared {
        sniff_wait: Duration::from_millis(300),
        ..EngineShared::default()
    }
}

/// The client side of a TLS handshake to the fixture's `sni.test`, over any
/// stream — a SOCKS5 or CONNECT tunnel, the app's side of the adapter —,
/// bounded as every TLS step of these tests is.
async fn handshake<S>(stream: S) -> std::io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let config = rurge_net::http::tls_client_config(true).expect("a client config");
    let name = rustls::pki_types::ServerName::try_from("sni.test".to_string()).expect("a name");
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio_rustls::TlsConnector::from(config).connect(name, stream),
    )
    .await
    .expect("the handshake ends within the bound")
}

/// One TLS round trip over an established tunnel, then the tunnel goes; the
/// three steps are bounded as in `extended_matching_rules_a_tls_session_by_its_sni`.
async fn ping_over<S>(stream: S)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut tls = handshake(stream)
        .await
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
}

/// The reply of the SOCKS5 listener to a CONNECT to `host:port` (the host as
/// a name), and the connection it came on: `tests/pipeline.rs`'s helper of the
/// same name, for the tests that read a code other than success.
async fn socks5_connect(proxy: SocketAddr, host: &str, port: u16) -> ([u8; 10], TcpStream) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut method))
        .await
        .expect("the proxy chooses a method")
        .unwrap();
    assert_eq!(method, [5, 0]);
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&port.to_be_bytes());
    s.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut reply))
        .await
        .expect("the proxy answers")
        .unwrap();
    (reply, s)
}

/// Two policies over loopback SOCKS5 servers that both reach `to`: `A` and
/// `B`, and the `[Proxy]` text that names them.
async fn two_policies(to: SocketAddr) -> (FakeSocks5, FakeSocks5, String) {
    let script = || Socks5Script {
        connect_to: Some(to),
        ..Socks5Script::default()
    };
    let a = FakeSocks5::spawn(script()).await;
    let b = FakeSocks5::spawn(script()).await;
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}\nB = socks5, 127.0.0.1, {}",
        a.addr().port(),
        b.addr().port()
    );
    (a, b, proxies)
}

// ---- the whole passage, from the answer to the record (plan task 7)

/// `extended-matching` on a line of the rules lets the ClientHello's SNI take
/// the session at every entry that answers before it reads (design 4.1, 7.2):
/// through the HTTP listener's CONNECT, and through the adapter.
#[tokio::test]
async fn extended_matching_rules_a_tls_session_by_its_sni_on_every_entry() {
    let fixture = TlsFixture::new(&["sni.test"]);
    let (a, b, proxies) = two_policies(fixture.spawn_echo(false).await).await;
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: &guarded("DOMAIN,sni.test,B,extended-matching\nDOMAIN,target.test,A"),
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    // through the HTTP listener's CONNECT
    let tunnel = connect_via_http(h.http(), "target.test:443").await;
    ping_over(tunnel).await;
    // its record is written before the adapter's session opens, so that the
    // two records come in this order
    the_records(&h, 1).await;

    // and through the adapter, whose app opens the fake address of the name
    let rig = Rig::on(h).await;
    let (fake, _) = rig.fakes("target.test");
    let tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the handshake completes");
    ping_over(tcp).await;

    let records = the_records(&rig.h, 2).await;
    for record in &records {
        assert_eq!(
            record.rule.as_deref(),
            Some("DOMAIN,sni.test,B,extended-matching"),
            "{record:?}"
        );
        assert_eq!(record.policy, ["B"], "{record:?}");
        assert_eq!(record.sni.as_deref(), Some("sni.test"), "{record:?}");
        assert_eq!(record.protocol, Some(ProtocolKind::Https), "{record:?}");
    }
    assert_eq!(records[0].listener, Http);
    assert_eq!(records[0].dst, "target.test:443");
    assert_eq!(records[1].listener, Tun);
    assert_eq!(records[1].dst, "target.test:443", "the name, not the fake");

    let asked = b.requests();
    assert_eq!(asked.len(), 2, "one for each entry");
    assert!(
        a.requests().is_empty(),
        "the destination's policy is not asked"
    );
}

/// Without the flag the SNI decides nothing: the destination's own rule takes
/// the session, while the SNI is still read and recorded (design 7.2).
#[tokio::test]
async fn without_extended_matching_the_sni_decides_nothing() {
    let fixture = TlsFixture::new(&["sni.test"]);
    let (a, b, proxies) = two_policies(fixture.spawn_echo(false).await).await;
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: "DOMAIN,sni.test,B\nDOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "target.test", 443).await;
    ping_over(tunnel).await;

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.rule.as_deref(), Some("DOMAIN,target.test,A"));
    assert_eq!(record.policy, ["A"]);
    assert_eq!(record.sni.as_deref(), Some("sni.test"));
    assert_eq!(record.protocol, Some(ProtocolKind::Https));
    assert_eq!(a.requests().len(), 1, "the destination's own policy");
    assert!(
        b.requests().is_empty(),
        "the SNI is not matched without the flag"
    );
}

/// A rule on the protocol refuses the session the sniff shows before any
/// policy is asked (design 7.1, 7.2): the tunnel is up already, so the client
/// sees its handshake fail where today it would see the refusal itself.
#[tokio::test]
async fn protocol_https_refuses_before_the_dial() {
    let fixture = TlsFixture::new(&["sni.test"]);
    let (a, b, proxies) = two_policies(fixture.spawn_echo(false).await).await;
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: "PROTOCOL,HTTPS,REJECT\nDOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    // through SOCKS5 the answer is "connected", then the refusal closes it
    let (reply, tunnel) = socks5_connect(h.socks(), "target.test", 443).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    assert!(
        handshake(tunnel).await.is_err(),
        "the refused tunnel cannot carry a handshake"
    );

    // and through CONNECT the 200 comes first, the failed handshake after it
    let tunnel = connect_via_http(h.http(), "target.test:443").await;
    assert!(
        handshake(tunnel).await.is_err(),
        "the refused tunnel cannot carry a handshake"
    );

    let records = the_records(&h, 2).await;
    for record in &records {
        assert_eq!(
            record.status,
            RecordStatus::Rejected("REJECT".into()),
            "{record:?}"
        );
        assert_eq!(record.rule.as_deref(), Some("PROTOCOL,HTTPS,REJECT"));
        assert_eq!(record.sni.as_deref(), Some("sni.test"));
        assert_eq!(record.protocol, Some(ProtocolKind::Https));
    }
    assert!(
        a.requests().is_empty(),
        "the destination's policy is not asked"
    );
    assert!(b.requests().is_empty());
}

/// A request whose opening bytes are an HTTP method is marked as HTTP, and a
/// `PROTOCOL,HTTP` rule takes it (design 6.2, 7.2).
#[tokio::test]
async fn a_plain_http_session_is_marked_http() {
    let origin = TestServer::spawn().await;
    origin.set("/", "hi there");
    let (a, b, proxies) = two_policies(origin_addr(&origin)).await;
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let page = get(&mut tunnel, "target.test", "/").await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    drop(tunnel);

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.protocol, Some(ProtocolKind::Http), "{record:?}");
    assert_eq!(record.dst, "target.test:80");
    assert_eq!(a.requests().len(), 1);

    // another engine on the same upstreams, where the protocol decides first
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: "PROTOCOL,HTTP,B\nDOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:80").await;
    let page = get(&mut tunnel, "target.test", "/").await;
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    drop(tunnel);

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.rule.as_deref(), Some("PROTOCOL,HTTP,B"));
    assert_eq!(record.policy, ["B"]);
    assert_eq!(b.requests().len(), 1, "the protocol's policy is asked");
    assert_eq!(a.requests().len(), 1, "and the destination's is not");
}

/// `always-raw-tcp-hosts` keeps today's order: the dial comes first, and its
/// refusal is what the client sees (design 4.1).
#[tokio::test]
async fn always_raw_tcp_hosts_keep_the_dial_first() {
    let a = FakeSocks5::spawn(Socks5Script {
        reply: 5,
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("A = socks5, 127.0.0.1, {}", a.addr().port());

    // without the setting the tunnel is up before anything is read, and the
    // dial's refusal ends it
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let (reply, mut s) = socks5_connect(h.socks(), "target.test", 443).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    s.write_all(b"hello").await.unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut back))
        .await
        .expect("the failed dial ends the tunnel within the bound")
        .unwrap();
    assert!(back.is_empty(), "nothing was echoed: {back:?}");

    // with it the refusal is today's, before anything is read
    let h = harness_with(
        Profile {
            general: "always-raw-tcp-hosts = target.test",
            proxies: &proxies,
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let (reply, _s) = socks5_connect(h.socks(), "target.test", 443).await;
    assert_eq!(reply[1], 0x05, "the dial's refusal comes first: {reply:?}");
}

/// An `always-raw-tcp-keywords` entry matches whatever the case it was
/// written in (design 4.2).
#[tokio::test]
async fn always_raw_tcp_keywords_ignore_case() {
    let a = FakeSocks5::spawn(Socks5Script {
        reply: 5,
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("A = socks5, 127.0.0.1, {}", a.addr().port());
    let h = harness_with(
        Profile {
            general: "always-raw-tcp-keywords = TARGET",
            proxies: &proxies,
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, _s) = socks5_connect(h.socks(), "target.test", 443).await;
    assert_eq!(reply[1], 0x05, "the keyword matches in any case: {reply:?}");
}

/// `force-http-engine-hosts` adds a port to the sniffed ones, and its default
/// port is 80 (design 4.1).
#[tokio::test]
async fn force_http_engine_hosts_sniff_another_port() {
    let a = FakeSocks5::spawn(Socks5Script {
        reply: 5,
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("A = socks5, 127.0.0.1, {}", a.addr().port());
    let harness = |general: &'static str| {
        harness_with(
            Profile {
                general,
                proxies: &proxies,
                rules: "DOMAIN,target.test,A",
                ..Profile::default()
            },
            sniffing(),
        )
    };

    // the port named in the setting is sniffed
    let h = harness("force-http-engine-hosts = target.test:8080").await;
    let (reply, mut s) = socks5_connect(h.socks(), "target.test", 8080).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    // the settings keep the forced port in the engine whatever it sends: the
    // dial `A` refuses fails, so the engine answers a 502 of its own (design
    // 6.4, 8.8)
    s.write_all(b"GET / HTTP/1.1\r\nHost: target.test:8080\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut back))
        .await
        .expect("the failed dial ends the tunnel within the bound")
        .unwrap();
    let back = String::from_utf8_lossy(&back);
    assert!(back.starts_with("HTTP/1.1 502"), "{back}");

    // a name without a port is port 80, so 8080 is not sniffed
    let h = harness("force-http-engine-hosts = target.test").await;
    let (reply, _s) = socks5_connect(h.socks(), "target.test", 8080).await;
    assert_eq!(reply[1], 0x05, "{reply:?}");

    // and without the setting neither is 8080
    let h = harness("").await;
    let (reply, _s) = socks5_connect(h.socks(), "target.test", 8080).await;
    assert_eq!(reply[1], 0x05, "{reply:?}");
}

/// A session pre-matching refuses is not sniffed at a proxy port, so its
/// refusal still comes before the answer (design 4.3): the SOCKS5 "not
/// allowed by ruleset", and a CONNECT that is closed with nothing on it.
#[tokio::test]
async fn pre_matching_still_refuses_before_the_answer() {
    let h = harness_with(
        Profile {
            hosts: "ads.test = 127.0.0.1",
            rules: "DOMAIN,ads.test,REJECT,pre-matching",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, _s) = socks5_connect(h.socks(), "ads.test", 443).await;
    assert_eq!(reply[1], 0x02, "not allowed by ruleset: {reply:?}");

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT ads.test:443 HTTP/1.1\r\nHost: ads.test:443\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut answer))
        .await
        .expect("the refusal comes within the bound")
        .unwrap();
    assert!(
        answer.is_empty(),
        "a refused CONNECT says nothing: {answer:?}"
    );

    // without the flag the refusal comes after the answer instead
    let h = harness_with(
        Profile {
            hosts: "ads.test = 127.0.0.1",
            rules: "DOMAIN,ads.test,REJECT",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let (reply, mut s) = socks5_connect(h.socks(), "ads.test", 443).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    s.write_all(b"hello").await.unwrap();
    let mut back = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut back))
        .await
        .expect("the refusal ends the tunnel within the bound")
        .unwrap();
    assert!(back.is_empty(), "the refusal answers nothing: {back:?}");
}

/// A paused engine refuses its sessions before it answers them, exactly as it
/// does today (design 4.1): the SOCKS5 "connection refused", and a record that
/// says why.
#[tokio::test]
async fn a_paused_engine_still_refuses_before_the_answer() {
    let a = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    h.engine.suspend(Some("strict".into()));
    let (reply, _s) = socks5_connect(h.socks(), "target.test", 443).await;
    assert_eq!(reply[1], 0x05, "today's refusal, not a tunnel: {reply:?}");

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(
        record.error.as_deref(),
        Some(rurge_engine::REFUSED_STRICT),
        "{record:?}"
    );
    assert_eq!(record.rule, None, "the refusal comes before the rules");
    assert!(record.policy.is_empty(), "{record:?}");
    assert!(a.requests().is_empty(), "nothing is dialed");
}

/// A protocol where the server speaks first says nothing for the whole wait;
/// then the session is dialed and forwarded as it is, with no protocol on its
/// record (design 6.3, 7.1).
#[tokio::test]
async fn a_server_first_protocol_passes_after_the_wait() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                stream.write_all(b"220 hello\r\n").await.ok();
                let mut buf = [0u8; 64];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                }
            });
        }
    });
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(addr),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let started = std::time::Instant::now();
    let (reply, mut s) = socks5_connect(h.socks(), "target.test", 443).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let mut hello = [0u8; 11];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut hello))
        .await
        .expect("the greeting comes within the bound")
        .unwrap();
    assert_eq!(&hello, b"220 hello\r\n");
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(300),
        "the greeting waited for the sniff: {took:?}"
    );
    assert!(took < Duration::from_secs(5), "{took:?}");
    drop(s);

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.protocol, None, "nothing was shown: {record:?}");
}

/// The bytes the client sent before the sniff was over reach the upstream
/// whole, in order, and nothing is lost or repeated (design 7.1).
#[tokio::test]
async fn the_prefix_reaches_the_upstream_whole() {
    let echo = rurge_proto::testing::echo_server().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut s) = socks5_connect(h.socks(), "target.test", 443).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    let mut sent = b"GE".to_vec();
    s.write_all(&sent).await.unwrap();
    // the 100 ms splits the opening bytes into two reads, so that the sniff is
    // over one prefix made of both: this is not a wait for nothing to happen
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rest = b"t / not http\r\n\r\n";
    sent.extend_from_slice(rest);
    sent.extend(std::iter::repeat_n(0x5a, 10 * 1024));
    s.write_all(rest).await.unwrap();
    s.write_all(&sent[sent.len() - 10 * 1024..]).await.unwrap();

    let mut back = vec![0u8; sent.len()];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(back, sent, "every byte went up and came back as it was");
}

/// A client that closes its side during the wait is dialed all the same: its
/// bytes go up, and what comes back reaches it before the end (design 6.3).
#[tokio::test]
async fn a_client_that_closes_during_the_wait_is_still_dialed() {
    let echo = rurge_proto::testing::echo_server().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let (reply, mut s) = socks5_connect(h.socks(), "target.test", 443).await;
    assert_eq!(reply[1], 0x00, "{reply:?}");
    s.write_all(b"GE").await.unwrap();
    s.shutdown().await.unwrap();
    let mut back = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(&back, b"GE");
    let mut rest = Vec::new();
    let n = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut rest))
        .await
        .expect("the end comes within the bound")
        .unwrap();
    assert_eq!((n, rest.len()), (0, 0), "the far end closed after the echo");
    drop(s);

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert_eq!(record.protocol, None, "nothing was judged: {record:?}");
}

/// A flag on one line of a rule set applies to the whole set (design 7.3, the
/// compatibility matrix's L314), so the SNI decides the session through
/// `RULE-SET` too.
#[tokio::test]
async fn a_flag_on_one_line_of_a_rule_set_matches_the_sni() {
    let fixture = TlsFixture::new(&["sni.test"]);
    let (a, b, proxies) = two_policies(fixture.spawn_echo(false).await).await;
    let h = harness_with(
        Profile {
            proxies: &proxies,
            rules: "RULE-SET,sni,B\nDOMAIN,target.test,A",
            sections: "[Ruleset sni]\nDOMAIN,sni.test\nDOMAIN,flag.test,extended-matching",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;

    let tunnel = connect_via_socks5(h.socks(), "target.test", 443).await;
    ping_over(tunnel).await;

    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.rule.as_deref(), Some("RULE-SET,sni,B"));
    assert_eq!(record.policy, ["B"]);
    assert_eq!(record.sni.as_deref(), Some("sni.test"));
    assert_eq!(b.requests().len(), 1, "the set's policy is asked");
    assert!(a.requests().is_empty());
}

/// The five ways a sniff ends are counted, one for each (design 7.5).
#[tokio::test]
async fn the_sniffs_are_counted() {
    let echo = rurge_proto::testing::echo_server().await;
    let a = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let h = harness_with(
        Profile {
            proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
            rules: "DOMAIN,target.test,A",
            ..Profile::default()
        },
        sniffing(),
    )
    .await;
    let counts = || h.engine.http_counts().sniffed;

    // a ClientHello the plain echo cannot answer: only the sniff is counted
    let tunnel = connect_via_socks5(h.socks(), "target.test", 443).await;
    assert!(
        handshake(tunnel).await.is_err(),
        "a plain echo is no TLS server"
    );

    // an HTTP request: the engine carries it, and the plain echo is no HTTP
    // server, so its answer is the engine's 502
    let mut s = connect_via_socks5(h.socks(), "target.test", 443).await;
    s.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
    let mut back = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut back))
        .await
        .expect("the engine answers within the bound")
        .unwrap();
    assert_eq!(&back, b"HTTP/1.1 502", "the echo is no HTTP server");
    drop(s);

    // neither: `hello` is not a method
    let mut s = connect_via_socks5(h.socks(), "target.test", 443).await;
    s.write_all(b"hello").await.unwrap();
    let mut back = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    drop(s);

    // nothing at all, until the wait runs out
    let s = connect_via_socks5(h.socks(), "target.test", 443).await;
    wait_until_within("the wait to run out", Duration::from_secs(5), || {
        counts().timeout == 1
    })
    .await;
    drop(s);

    // a client that says `GE` and closes its side
    let mut s = connect_via_socks5(h.socks(), "target.test", 443).await;
    s.write_all(b"GE").await.unwrap();
    s.shutdown().await.unwrap();
    let mut back = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut back))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut rest))
        .await
        .expect("the end comes within the bound")
        .unwrap();
    drop(s);

    the_records(&h, 5).await;
    assert_eq!(
        h.engine.http_counts(),
        HttpCounts {
            sniffed: SniffedCounts {
                tls: 1,
                http: 1,
                other: 1,
                timeout: 1,
                closed: 1,
            },
        }
    );
}

/// A stop ends a sniffing session of the adapter at once, and leaves nothing
/// behind: no dial, no handle, no record (design 7.1).
#[tokio::test]
async fn a_stop_during_the_wait_ends_a_tun_session_at_once() {
    // the wait is the 15 seconds of `SNIFF_WAIT`: the stop ends it, not a
    // timeout that happened to be short
    let a = FakeSocks5::spawn(Socks5Script::default()).await;
    let rig = Rig::new(Profile {
        proxies: &format!("A = socks5, 127.0.0.1, {}", a.addr().port()),
        rules: &guarded("DOMAIN,target.test,A"),
        ..Profile::default()
    })
    .await;
    let (fake, _) = rig.fakes("target.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the handshake completes");

    rig.tun.stop();
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(WITHIN, tcp.read(&mut byte))
        .await
        .expect("the stop ends the session at once");
    assert!(
        matches!(read, Ok(0) | Err(_)),
        "the app is let go, not answered: {read:?}"
    );

    // nothing is recorded: the only fixed wait here is for a record that came
    // late, and 300 ms is far more than the stop needs to end one that would
    // have come at once
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        rig.h.engine.request_log().recent(10).is_empty(),
        "a session ended by the stop has no record"
    );
    assert!(rig.h.engine.request_log().active().is_empty());
    assert!(a.requests().is_empty(), "nothing is dialed after the stop");
}

/// A session the rules drop is held after the answer already went out, and
/// the hold ends when the app closes its side (design 7.4).
#[tokio::test]
async fn a_dropped_tun_session_ends_when_the_app_closes() {
    let rig = Rig::on(
        harness_with(
            Profile {
                rules: &guarded("DOMAIN,drop.test,REJECT-DROP"),
                ..Profile::default()
            },
            sniffing(),
        )
        .await,
    )
    .await;
    let (fake, _) = rig.fakes("drop.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 443))
        .await
        .expect("the handshake completes");
    tcp.write_all(b"hello").await.unwrap();
    tcp.shutdown().await.unwrap();

    let mut rest = Vec::new();
    let n = tokio::time::timeout(WITHIN, tcp.read_to_end(&mut rest))
        .await
        .expect("the app's closing ends the hold well within the bound")
        .unwrap();
    assert_eq!(
        (n, rest.len()),
        (0, 0),
        "the hold answers nothing at all: {rest:?}"
    );
}
