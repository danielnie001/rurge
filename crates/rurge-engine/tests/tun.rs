//! The engine behind a virtual adapter (phase 3 M3b design 9, 15.2):
//! `Engine::start_tun` on one end of a memory device, an app's TCP/IP stack on
//! the other, and between them the real rules, the real DIRECT, the pool of
//! fake addresses and the request log.
//!
//! Nothing here goes beyond the loopback, whatever fails (plan P17):
//!
//! * The rules end with the two guards (`guarded`): a session of the app that
//!   no rule above them took is refused, and never reaches the `FINAL,DIRECT`
//!   the harness adds.
//! * A name is pinned to 127.0.0.1 by the harness's DNS and given to `DIRECT`
//!   by a rule of its own; the app reaches it by the fake address the pool
//!   gives it.
//! * A connection to a real address is one to a documentation address, which a
//!   rule hands to a fake SOCKS5 upstream that connects to a loopback echo
//!   whatever it is asked for. No datagram goes to a real address.
//! * An echo request goes to the fake pinger, which sends it nowhere.
//!
//! No clock is paused: the lookups of processes and the pool's file use
//! blocking threads, which a paused clock does not wait for.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::{ProcessInfo, SessionInfo, Transport};
use rurge_dns::fakeip::FILE_NAME;
use rurge_engine::{OwnerLookup, OwnerQuery, RequestRecord};
use rurge_inbound::{Dialer, UdpClient};
use rurge_net::BoxFuture;
use rurge_net::connector::Target;
use rurge_proto::RejectKind;
use rurge_proto::testing::{echo_server, udp_echo_server};
use rurge_tun::device::MemoryDevice;
use rurge_tun::testing::packets::{self, IcmpSeen, UdpSeen};
use rurge_tun::testing::{AppStack, AppTcp, FakePinger, PingScript};
use rurge_tun::{Ending, TunHandle, TunOptions};
use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::error::TryRecvError;
use tokio_util::sync::CancellationToken;

/// What every wait of these tests is bounded by.
const WITHIN: Duration = Duration::from_secs(5);

/// The address the app has on the adapter, and a UDP endpoint of it.
const APP: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1));
const APP_UDP: &str = "198.18.0.1:5000";

fn app_udp() -> SocketAddr {
    APP_UDP.parse().unwrap()
}

/// `rules`, then the two guards. A session from the app's address that no rule
/// above them took is refused, so it never reaches the `FINAL,DIRECT` of the
/// harness, which would send it wherever it is going (plan P17).
fn guarded(rules: &str) -> String {
    format!("{rules}\nSRC-IP,198.18.0.1/32,REJECT\nSRC-IP,fd00:6152::1/128,REJECT")
}

fn profile(rules: &str) -> Profile<'_> {
    Profile {
        rules,
        ..Profile::default()
    }
}

/// An engine with a TUN on a memory device, the app's stack on the other end
/// of it, and the pinger the TUN was started with.
struct Rig {
    h: Harness,
    tun: TunHandle,
    app: AppStack,
    pinger: Arc<FakePinger>,
}

impl Rig {
    async fn new(profile: Profile<'_>) -> Rig {
        Rig::on(harness(profile).await).await
    }

    /// A TUN started by the engine of `h` on a new memory device.
    async fn on(h: Harness) -> Rig {
        let (device, end) = MemoryDevice::pair(1500, 4096);
        let pinger = FakePinger::new();
        let tun = h
            .engine
            .start_tun(Arc::new(device), pinger.clone(), TunOptions::default())
            .await;
        Rig {
            h,
            tun,
            app: AppStack::start(end),
            pinger,
        }
    }

    /// The fake addresses of `name`, the IPv4 one and the IPv6 one: the pool
    /// gives the name its address when asked, as the DNS responder will.
    fn fakes(&self, name: &str) -> (IpAddr, IpAddr) {
        let pool = self.tun.pool().expect("the engine gave the TUN a pool");
        pool.assign(name, std::time::Instant::now())
            .expect("an address for the name");
        (
            pool.address_of(name, false).expect("an IPv4 address"),
            pool.address_of(name, true).expect("an IPv6 address"),
        )
    }

    fn data_dir(&self) -> &Path {
        self.h.dir.path()
    }
}

/// `payload` through `tcp` and back, from the echo at the other end.
async fn echoes(tcp: &mut AppTcp, payload: &[u8]) {
    tcp.write_all(payload).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    tokio::time::timeout(WITHIN, tcp.read_exact(&mut back))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(back, payload);
}

/// The app closes its side and waits for the other to close: the session ends
/// as completed, not as one the app reset.
async fn close(mut tcp: AppTcp) {
    tcp.shutdown().await.unwrap();
    let mut rest = Vec::new();
    tokio::time::timeout(WITHIN, tcp.read_to_end(&mut rest))
        .await
        .expect("the other end closes")
        .unwrap();
    assert!(rest.is_empty(), "{rest:?}");
}

/// The next packet on a receiver that `AppStack::divert_udp` or `divert_icmp`
/// gave: what the TUN wrote for the app, of that kind.
async fn next(packets: &mut UnboundedReceiver<Vec<u8>>) -> Vec<u8> {
    tokio::time::timeout(WITHIN, packets.recv())
        .await
        .expect("a packet for the app in time")
        .expect("the app's stack is there")
}

/// Waits for the TUN to end, and says why it did.
async fn ends(tun: &TunHandle) -> Ending {
    tokio::time::timeout(WITHIN, tun.join())
        .await
        .expect("the TUN ends")
}

/// The finished records of the sessions that came through the adapter, newest
/// first.
fn finished_through_the_tun(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(1000)
        .into_iter()
        .filter(|record| record.listener == ListenerKind::Tun)
        .collect()
}

/// The records of the finished sessions of the adapter, oldest first, once
/// there are `n`.
async fn tun_records(h: &Harness, n: usize) -> Vec<RequestRecord> {
    wait_until("the sessions of the TUN to finish", || {
        finished_through_the_tun(h).len() >= n
    })
    .await;
    let mut records = finished_through_the_tun(h);
    records.reverse();
    records
}

/// The pool's file in `dir`, if it can be read.
fn pool_file(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(FILE_NAME)).ok()
}

/// Whether the second line of the pool's file says that the run ended cleanly.
fn ended_cleanly(dir: &Path) -> bool {
    pool_file(dir).is_some_and(|text| {
        text.lines()
            .nth(1)
            .is_some_and(|line| line.ends_with(" clean 1"))
    })
}

/// Waits for the save task to write the pool's last state.
async fn saved_cleanly(dir: &Path) {
    wait_until("the pool to be written as having ended cleanly", || {
        ended_cleanly(dir)
    })
    .await;
}

// ---- TCP

/// A connection to the fake address of a name is a session for the name, over
/// IPv4 and over IPv6: the rules see the name, DIRECT dials it, and the
/// record says it came through the adapter.
#[tokio::test]
async fn tcp_to_a_fake_address_reaches_its_name() {
    let echo = echo_server().await;
    let rules = guarded("DOMAIN,target.test,DIRECT");
    let rig = Rig::new(profile(&rules)).await;
    let (v4, v6) = rig.fakes("target.test");
    let mut sources = Vec::new();
    for fake in [v4, v6] {
        let mut tcp = rig
            .app
            .connect(SocketAddr::new(fake, echo.port()))
            .await
            .unwrap();
        echoes(&mut tcp, b"hello through the adapter").await;
        sources.push(tcp.local());
        close(tcp).await;
    }
    let records = tun_records(&rig.h, 2).await;
    for (record, source) in records.iter().zip(sources) {
        assert_eq!(record.dst, format!("target.test:{}", echo.port()));
        assert_eq!(record.listener, ListenerKind::Tun);
        assert_eq!(record.listen_port, 0);
        assert_eq!(record.transport, Transport::Tcp);
        assert_eq!(record.src, source);
        assert_eq!(record.rule.as_deref(), Some("DOMAIN,target.test,DIRECT"));
        assert_eq!(record.policy, ["DIRECT"]);
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    }
}

/// A connection to a real address, which is a documentation address here, is
/// decided by the rules as any other is: this one goes through a SOCKS5
/// upstream, which is told where the app was going.
#[tokio::test]
async fn tcp_to_a_documentation_address_follows_the_rules() {
    let echo = echo_server().await;
    let socks = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("Proxy = socks5, 127.0.0.1, {}", socks.addr().port());
    let rules = guarded("IP-CIDR,192.0.2.0/24,Proxy");
    let rig = Rig::new(Profile {
        proxies: &proxies,
        ..profile(&rules)
    })
    .await;
    let mut tcp = rig
        .app
        .connect("192.0.2.10:80".parse().unwrap())
        .await
        .unwrap();
    echoes(&mut tcp, b"hello far away").await;
    close(tcp).await;
    let requests = socks.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(
        (
            requests[0].command,
            requests[0].host.as_str(),
            requests[0].port
        ),
        (1, "192.0.2.10", 80)
    );
    let record = tun_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.dst, "192.0.2.10:80");
    assert_eq!(record.rule.as_deref(), Some("IP-CIDR,192.0.2.0/24,Proxy"));
    assert_eq!(record.policy, ["Proxy"]);
}

/// The handshake is over before the rules are asked, so a refusal reaches the
/// app as a reset on its first read.
#[tokio::test]
async fn a_rejected_tcp_connection_is_reset() {
    let rules = guarded("DOMAIN,rej.test,REJECT");
    let rig = Rig::new(profile(&rules)).await;
    let (fake, _) = rig.fakes("rej.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, 80))
        .await
        .expect("the handshake completes");
    let mut byte = [0u8; 1];
    let error = tokio::time::timeout(WITHIN, tcp.read(&mut byte))
        .await
        .expect("the reset comes")
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    let record = tun_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".to_string()));
    assert_eq!(record.rule.as_deref(), Some("DOMAIN,rej.test,REJECT"));
}

// ---- UDP

/// A datagram to the fake address of a name goes to the name, and what the
/// name answers comes back as from the endpoint the app sent to.
#[tokio::test]
async fn udp_to_a_fake_address_reaches_its_name_and_answers_from_it() {
    let echo = udp_echo_server().await;
    let rules = guarded("DOMAIN,target.test,DIRECT");
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    let (fake, _) = rig.fakes("target.test");
    let to = SocketAddr::new(fake, echo.port());
    rig.app.inject(packets::udp(app_udp(), to, b"ping"));
    let seen = packets::read_udp(&next(&mut udp).await)
        .expect("a whole datagram with the right checksums");
    assert_eq!(
        seen,
        UdpSeen {
            src: to,
            dst: app_udp(),
            payload: b"ping".to_vec(),
        }
    );
}

/// What the app is told of a datagram the rules refused: the ICMP message
/// "administratively prohibited", from the endpoint it was sent to.
async fn prohibited_from(icmp: &mut UnboundedReceiver<Vec<u8>>, fake: IpAddr) {
    match packets::read_icmp(&next(icmp).await) {
        Some(IcmpSeen::Prohibited { src, dst, .. }) => {
            assert_eq!((src, dst), (fake, APP));
        }
        other => panic!("an administratively prohibited message, not {other:?}"),
    }
}

/// A datagram to a name the rules refuse is answered with ICMP, from the
/// fake address of the name.
#[tokio::test]
async fn a_rejected_udp_flow_answers_with_icmp() {
    let rules = guarded("DOMAIN,rej.test,REJECT");
    let rig = Rig::new(profile(&rules)).await;
    let mut icmp = rig.app.divert_icmp();
    let (fake, _) = rig.fakes("rej.test");
    rig.app.inject(packets::udp(
        app_udp(),
        SocketAddr::new(fake, 9000),
        b"knock",
    ));
    prohibited_from(&mut icmp, fake).await;
    wait_until("the record of the flow", || !udp_records(&rig.h).is_empty()).await;
    let record = udp_records(&rig.h).remove(0);
    assert_eq!(record.listener, ListenerKind::Tun);
    assert_eq!(record.dst, "rej.test:9000");
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".to_string()));
}

/// `block-quic` drops a QUIC flow that the rules let through, and the app is
/// told so, as for any other flow that was refused: its browser falls back to
/// TCP at once.
#[tokio::test]
async fn a_blocked_quic_flow_answers_with_icmp() {
    let rules = guarded("DOMAIN,target.test,DIRECT");
    let rig = Rig::new(Profile {
        general: "block-quic = all",
        ..profile(&rules)
    })
    .await;
    let mut icmp = rig.app.divert_icmp();
    let (fake, _) = rig.fakes("target.test");
    rig.app.inject(packets::udp(
        app_udp(),
        SocketAddr::new(fake, 443),
        &quic_initial(),
    ));
    prohibited_from(&mut icmp, fake).await;
    wait_until("the record of the flow", || !udp_records(&rig.h).is_empty()).await;
    let record = udp_records(&rig.h).remove(0);
    assert_eq!(record.error.as_deref(), Some("QUIC blocked"));
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".to_string()));
    assert_eq!(
        record.protocol,
        Some(rurge_config::rule::ProtocolKind::Quic)
    );
}

// ---- ICMP

/// A ping to the fake address of a name goes to the address the name has, and
/// the reply comes back as from the fake address. With `icmp-forwarding` off in
/// the next generation of the profile, the next ping is not answered.
#[tokio::test]
async fn a_ping_to_a_fake_address_goes_to_the_names_address() {
    let rules = guarded("DOMAIN,target.test,DIRECT");
    let rig = Rig::new(profile(&rules)).await;
    let target = IpAddr::V4(Ipv4Addr::LOCALHOST);
    rig.pinger.script(target, PingScript::Echo);
    let mut icmp = rig.app.divert_icmp();
    let (fake, _) = rig.fakes("target.test");
    rig.app
        .inject(packets::echo_request(APP, fake, 0x1234, 7, b"ping!"));
    match packets::read_icmp(&next(&mut icmp).await) {
        Some(IcmpSeen::EchoReply {
            src,
            dst,
            ident,
            seq,
            payload,
        }) => {
            assert_eq!((src, dst), (fake, APP));
            assert_eq!((ident, seq), (0x1234, 7));
            assert_eq!(payload, b"ping!");
        }
        other => panic!("an echo reply, not {other:?}"),
    }
    assert_eq!(rig.pinger.pinged(), [(target, b"ping!".to_vec())]);

    // the next generation of the profile forwards no pings
    let text = Profile {
        general: "icmp-forwarding = false",
        ..profile(&rules)
    }
    .text(rig.h.dns.addr());
    rig.h
        .engine
        .swap_runtime(runtime(rig.data_dir(), &text, rig.h.engine.shared()).await);
    assert_eq!(rig.tun.counters().unsupported, 0);
    rig.app
        .inject(packets::echo_request(APP, fake, 0x1234, 8, b"ping?"));
    // the request is dropped where the TUN reads the setting, which counts it:
    // after that nothing is on its way to the app
    wait_until("the second ping to be dropped", || {
        rig.tun.counters().unsupported == 1
    })
    .await;
    assert!(
        matches!(icmp.try_recv(), Err(TryRecvError::Empty)),
        "a reply to a ping nobody forwards"
    );
    assert_eq!(rig.pinger.pinged().len(), 1, "the pinger was asked once");
}

// ---- the process behind a session

/// Above 65535, so that no port number a log line carries can contain it.
const APP_PID: u32 = 4_000_321;

fn the_app() -> ProcessInfo {
    ProcessInfo {
        pid: Some(APP_PID),
        name: "app.exe".to_string(),
        path: Some("C:\\Apps\\app.exe".to_string()),
    }
}

/// The platform's lookup, as far as the engine can tell: it finds the app for
/// every connection and notes what it is asked. `is_local` says yes, as it will
/// for the adapter's own address; "surely not" would mean no lookup at all.
#[derive(Default)]
struct Lookup {
    asked: Mutex<Vec<OwnerQuery>>,
}

impl Lookup {
    fn asked(&self) -> Vec<OwnerQuery> {
        self.asked.lock().unwrap().clone()
    }
}

impl OwnerLookup for Lookup {
    fn is_local(&self, _ip: IpAddr) -> Option<bool> {
        Some(true)
    }

    fn owner(&self, query: &OwnerQuery) -> Option<ProcessInfo> {
        self.asked.lock().unwrap().push(query.clone());
        Some(the_app())
    }
}

/// The sessions of the adapter are looked up by the connection or the socket
/// the app has: a TCP connection by its own endpoint and the endpoint it
/// connected to, a UDP socket by its own endpoint alone.
#[tokio::test]
async fn tun_sessions_are_looked_up_by_their_own_shape() {
    let lookup = Arc::new(Lookup::default());
    let tcp_echo = echo_server().await;
    let udp_echo = udp_echo_server().await;
    let rules = guarded("DOMAIN,target.test,DIRECT");
    let h = harness_with(
        profile(&rules),
        EngineShared {
            owners: Some(lookup.clone()),
            ..EngineShared::default()
        },
    )
    .await;
    let rig = Rig::on(h).await;
    let mut udp = rig.app.divert_udp();
    let (fake, _) = rig.fakes("target.test");

    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, tcp_echo.port()))
        .await
        .unwrap();
    echoes(&mut tcp, b"tcp").await;
    let to = SocketAddr::new(fake, udp_echo.port());
    rig.app.inject(packets::udp(app_udp(), to, b"udp"));
    next(&mut udp).await;

    wait_until("both lookups", || lookup.asked().len() >= 2).await;
    let asked = lookup.asked();
    assert_eq!(asked.len(), 2, "{asked:?}");
    let query = |transport| {
        asked
            .iter()
            .find(|query| query.transport == transport)
            .unwrap_or_else(|| panic!("no {transport:?} query in {asked:?}"))
    };
    let (tcp_query, udp_query) = (query(Transport::Tcp), query(Transport::Udp));
    assert_eq!(tcp_query.local, tcp.local());
    assert_eq!(
        tcp_query.remote,
        Some(SocketAddr::new(fake, tcp_echo.port())),
        "the endpoint the app connected to"
    );
    assert_eq!(udp_query.local, app_udp());
    assert_eq!(udp_query.remote, None, "a UDP socket has no other end");

    // both sessions are still running: their records name the process
    wait_until("the process on both records", || {
        let active = rig.h.engine.request_log().active();
        [Transport::Tcp, Transport::Udp].iter().all(|&transport| {
            active.iter().any(|record| {
                record.listener == ListenerKind::Tun
                    && record.transport == transport
                    && record.process.found().and_then(|p| p.pid) == Some(APP_PID)
            })
        })
    })
    .await;
    close(tcp).await;
}

// ---- traffic

/// The bytes of a session of the adapter are counted under the adapter and
/// under no other listener.
#[tokio::test]
async fn tun_traffic_is_counted_under_its_listener() {
    let echo = echo_server().await;
    let rules = guarded("DOMAIN,target.test,DIRECT");
    let rig = Rig::new(profile(&rules)).await;
    // a session of the HTTP listener first, so that its bucket is not empty
    let mut tunnel = connect_via_http(rig.h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"through the proxy port").await;
    drop(tunnel);
    wait_until("the HTTP session to finish", || {
        rig.h.engine.request_log().recent(10).len() == 1
    })
    .await;
    let before = rig.h.engine.traffic().by_listener();
    let kinds: Vec<_> = before.iter().map(|(kind, _, _)| *kind).collect();
    assert_eq!(
        kinds,
        [ListenerKind::Http, ListenerKind::Socks5, ListenerKind::Tun]
    );
    assert!(before[0].1 > 0 && before[0].2 > 0, "{before:?}");
    assert_eq!((before[2].1, before[2].2), (0, 0), "{before:?}");

    let (fake, _) = rig.fakes("target.test");
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, echo.port()))
        .await
        .unwrap();
    echoes(&mut tcp, b"through the adapter").await;
    close(tcp).await;
    tun_records(&rig.h, 1).await;

    let after = rig.h.engine.traffic().by_listener();
    assert_eq!(after[0], before[0], "the HTTP bucket moved");
    assert_eq!(after[1], before[1], "the SOCKS5 bucket moved");
    let (up, down) = (after[2].1, after[2].2);
    assert!(up > 0 && down > 0, "{after:?}");
}

// ---- stopping

/// The TUN stops a relay that is stuck on its upstream in both directions: the
/// upstream takes nothing the app writes, and says nothing. The stop gives the
/// connection a reset, which a relay that touches the connection in neither
/// direction never meets; the TUN cancels the session instead, and the record
/// of it ends.
#[tokio::test]
async fn stopping_the_tun_ends_a_relay_stuck_on_its_upstream() {
    // accepts, and holds the connections without ever reading or writing them
    let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = sink.local_addr().unwrap().port();
    let (accepted, mut held) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((stream, _)) = sink.accept().await {
            let _ = accepted.send(stream);
        }
    });
    let rules = guarded("DOMAIN,sink.test,DIRECT");
    let rig = Rig::new(profile(&rules)).await;
    rig.h.dns.set("sink.test", &["127.0.0.1"], &[], 60);
    let (fake, _) = rig.fakes("sink.test");
    let mut app = rig.app.connect(SocketAddr::new(fake, port)).await.unwrap();
    // the engine has dialed: the relay is there
    let _upstream = tokio::time::timeout(WITHIN, held.recv())
        .await
        .expect("the sink is connected to")
        .unwrap();

    // The app writes until a write does not finish: the upstream takes nothing,
    // so the buffers of the relay, of the TUN and of the app's stack fill up.
    // What is waited for there is nothing happening, and 500 ms is far more
    // than the buffers take to fill.
    let chunk = vec![0x5a; 16 * 1024];
    let mut written = 0usize;
    loop {
        assert!(
            written < 64 << 20,
            "64 MiB went in and the upstream still takes more"
        );
        match tokio::time::timeout(Duration::from_millis(500), app.write_all(&chunk)).await {
            Ok(result) => {
                result.unwrap();
                written += chunk.len();
            }
            Err(_) => break,
        }
    }

    rig.tun.stop();
    assert_eq!(ends(&rig.tun).await, Ending::Stopped);
    let record = tun_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.dst, format!("sink.test:{port}"));
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert!(record.up > 0, "something went to the upstream: {record:?}");
}

// ---- the pool

/// The pool is kept in the data directory: a name that was given an address is
/// in the file, and the file says that the run ended cleanly once the TUN has.
#[tokio::test]
async fn the_pool_is_saved_in_the_data_directory() {
    let rig = Rig::new(Profile::default()).await;
    assert!(
        pool_file(rig.data_dir()).is_some(),
        "the pool's file is there from the start"
    );
    rig.fakes("saved.test");
    rig.tun.stop();
    ends(&rig.tun).await;
    saved_cleanly(rig.data_dir()).await;
    let text = pool_file(rig.data_dir()).unwrap();
    assert!(
        text.lines().skip(2).any(|line| line
            .split_once(' ')
            .is_some_and(|(_, name)| name == "saved.test")),
        "{text}"
    );
}

/// Another engine on the same data directory finds the names where the first
/// left them, at the addresses it gave them.
#[tokio::test]
async fn the_engine_reopens_the_pool() {
    let rig = Rig::new(Profile::default()).await;
    let (v4, v6) = rig.fakes("kept.test");
    rig.tun.stop();
    ends(&rig.tun).await;
    // the first engine's pool is done with the file before the second opens it
    saved_cleanly(rig.data_dir()).await;

    let text = std::fs::read_to_string(rig.data_dir().join("t.conf")).unwrap();
    let second = Engine::new(runtime(rig.data_dir(), &text, EngineShared::default()).await);
    // the app's end is kept: a device whose other end is gone fails
    let (device, _app_end) = MemoryDevice::pair(1500, 16);
    let tun = second
        .start_tun(Arc::new(device), FakePinger::new(), TunOptions::default())
        .await;
    let pool = tun.pool().expect("a pool");
    assert_eq!(pool.address_of("kept.test", false), Some(v4));
    assert_eq!(pool.address_of("kept.test", true), Some(v6));
    tun.stop();
    ends(&tun).await;
}

/// The engine that stops accepting stops the TUN, and the pool is saved as
/// having ended cleanly.
#[tokio::test]
async fn stopping_the_engine_stops_the_tun() {
    let rig = Rig::new(Profile::default()).await;
    rig.fakes("saved.test");
    assert!(!ended_cleanly(rig.data_dir()));
    rig.h.engine.stop_accepting();
    assert_eq!(ends(&rig.tun).await, Ending::Stopped);
    saved_cleanly(rig.data_dir()).await;
}

/// A file the pool cannot read is told of once, without a word of what was in
/// it, and left as it is until the first save.
#[tokio::test]
async fn an_unreadable_pool_file_is_left_and_warned_once() {
    // held across the awaits: what the engine logs for this test is logged on
    // this thread
    let log = LogCapture::start();
    let h = harness(Profile::default()).await;
    let secret = "nothing-of-the-file-is-to-be-logged.test";
    let content = format!("this is not a pool\n{secret}\n");
    let file = h.dir.path().join(FILE_NAME);
    std::fs::write(&file, &content).unwrap();
    let _rig = Rig::on(h).await;

    let lines = log.lines();
    let told: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("fake-ip:"))
        .collect();
    assert_eq!(told.len(), 1, "{lines:?}");
    assert!(
        told[0]
            .starts_with("WARN fake-ip: the saved addresses could not be read; starting with none"),
        "{}",
        told[0]
    );
    assert!(lines.iter().all(|line| !line.contains(secret)), "{lines:?}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), content);
}

/// A pool whose last run did not end cleanly is told of once, at INFO, and
/// what it holds back is said in numbers.
#[tokio::test]
async fn a_pool_left_unclean_is_told_of_once() {
    let log = LogCapture::start();
    let h = harness(Profile::default()).await;
    std::fs::write(
        h.dir.path().join(FILE_NAME),
        "rurge-fake-ip 1\ncursor 1 wrapped 0 clean 0\n0 left-behind.test\n",
    )
    .unwrap();
    let rig = Rig::on(h).await;

    let lines = log.lines();
    let told: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("fake-ip:"))
        .collect();
    assert_eq!(
        told,
        [
            "INFO fake-ip: rurge did not stop cleanly last time; 2048 fake addresses are held back for 120 seconds"
        ]
    );
    assert!(
        lines.iter().all(|line| !line.contains("left-behind")),
        "{lines:?}"
    );
    // what the run left is there to be had
    assert!(
        rig.tun
            .pool()
            .unwrap()
            .address_of("left-behind.test", false)
            .is_some()
    );
}

// ---- what the TUN's UDP path asks of the engine

/// The client of an association made by hand: it hands the engine the
/// datagrams it was given, and notes what it is sent and what it is told was
/// refused.
struct Probe {
    to_send: Mutex<VecDeque<(Vec<u8>, Target)>>,
    heard: Mutex<Vec<(Vec<u8>, Target)>>,
    refused: Mutex<Vec<(Target, RejectKind)>>,
}

impl UdpClient for Probe {
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let next = self.to_send.lock().unwrap().pop_front();
            match next {
                Some((payload, to)) => {
                    buf[..payload.len()].copy_from_slice(&payload);
                    Ok((payload.len(), to))
                }
                None => std::future::pending().await,
            }
        })
    }

    fn send<'a>(
        &'a self,
        payload: &'a [u8],
        from: &'a Target,
        _answers: Option<&'a Target>,
    ) -> BoxFuture<'a, io::Result<()>> {
        self.heard
            .lock()
            .unwrap()
            .push((payload.to_vec(), from.clone()));
        Box::pin(std::future::ready(Ok(())))
    }

    fn refused(&self, target: &Target, kind: RejectKind) {
        self.refused.lock().unwrap().push((target.clone(), kind));
    }
}

/// What the TUN counts on in an association it started: the client is told of a
/// flow that was refused (`refused`, which the flow's own task calls), the
/// association returns once the token it was given is cancelled, and the engine
/// keeps no hold of the client after that. The TUN's writer queue closes with
/// the last hold on the client; while one is left, the writer stays up for the
/// whole of the grace a stop gives it.
#[tokio::test]
async fn an_association_returns_when_closed_and_lets_go_of_its_client() {
    let echo = udp_echo_server().await;
    let rules = guarded("DOMAIN,target.test,DIRECT\nDOMAIN,rej.test,REJECT");
    let h = harness(profile(&rules)).await;
    let name = |name: &str, port| Target::new(HostName::parse(name), port);
    let client = Arc::new(Probe {
        to_send: Mutex::new(VecDeque::from([
            (b"to the echo".to_vec(), name("target.test", echo.port())),
            (b"to nowhere".to_vec(), name("rej.test", 9)),
        ])),
        heard: Mutex::default(),
        refused: Mutex::default(),
    });
    let template = SessionInfo {
        src: app_udp(),
        listener: ListenerKind::Tun,
        ..SessionInfo::udp(HostName::parse("198.18.0.253"), 0)
    };
    let closed = CancellationToken::new();
    let association = tokio::spawn({
        let (engine, client, closed) = (h.engine.clone(), client.clone(), closed.clone());
        async move { engine.associate(client, template, closed).await }
    });

    wait_until("the echo and the refusal", || {
        !client.heard.lock().unwrap().is_empty() && !client.refused.lock().unwrap().is_empty()
    })
    .await;
    assert_eq!(client.heard.lock().unwrap()[0].0, b"to the echo");
    assert_eq!(
        *client.refused.lock().unwrap(),
        [(name("rej.test", 9), RejectKind::Reject)]
    );
    assert!(!association.is_finished(), "the association runs on");

    closed.cancel();
    tokio::time::timeout(WITHIN, association)
        .await
        .expect("the association returns once it is closed")
        .unwrap();
    // the engine's tasks that hold the client end with the association
    wait_until("the engine to let go of the client", || {
        Arc::strong_count(&client) == 1
    })
    .await;
}
