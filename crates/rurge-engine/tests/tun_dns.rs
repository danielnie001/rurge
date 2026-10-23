//! The engine behind the virtual adapter's DNS responder, its pre-matching and
//! the fake addresses that come in at a proxy port (phase 3 M3 design 11 to 13,
//! 15.2): `Engine::start_tun` on one end of a memory device, an app's TCP/IP
//! stack on the other, and between them the real rules, the real DIRECT, the
//! pool of fake addresses and the request log. The app asks the responder at
//! `198.18.0.2:53` by UDP, and hears what the TUN writes for it.
//!
//! Nothing here goes beyond the loopback, whatever fails (plan P24):
//!
//! * The rules of what comes through the adapter end with the two guards
//!   (`guarded`): a session of the app that no rule above them took is refused,
//!   and never reaches the `FINAL,DIRECT` the harness adds.
//! * A name the app asks the responder for is pinned to 127.0.0.1 by the
//!   harness's DNS and given to `DIRECT` by a rule of its own; the app reaches
//!   it by the fake address the responder gave it. A connection to a real
//!   address is one to a documentation address, which a rule under test
//!   refuses before it is dialed.
//! * What comes in at a proxy port (`listener_rules`) lets the name through by a
//!   rule of its own, and refuses all of rurge's space and the upstream's port
//!   after it: an address that is not turned back into the name is refused, and
//!   never reaches the `FINAL,DIRECT` either.
//!
//! No clock is paused: the lookups of processes and the pool's file use
//! blocking threads, which a paused clock does not wait for.

mod common;
use common::tun::*;
use common::*;
use rurge_config::session::ProcessInfo;
use rurge_dns::message::{Answer, Rcode, parse_response};
use rurge_dns::testing::query_wire;
use rurge_engine::engine::ESCALATE_COUNT;
use rurge_engine::{FAKE_ADDRESS_NOT_KNOWN, Mode, OwnerLookup, OwnerQuery};
use rurge_proto::testing::echo_server;
use rurge_tun::Ending;
use rurge_tun::testing::packets;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::error::TryRecvError;

/// The type of the queries the app asks.
const A: u16 = 1;

/// How long a wait for something that must not happen lasts. What is waited
/// for is a datagram for the app: the responder decides a query as it reads it,
/// in microseconds, and the datagram goes out at once, so that 300 ms is far
/// more than there is need of.
const NOTHING: Duration = Duration::from_millis(300);

/// The rule that the pre-matching tests share.
const ADS: &str = "DOMAIN-SUFFIX,ads.test,REJECT,pre-matching";

/// Where the app sends its queries: the responder's IPv4 address.
fn responder() -> SocketAddr {
    "198.18.0.2:53".parse().unwrap()
}

/// The app asks the responder for the A record of `name`, with the ID `id`.
fn ask(rig: &Rig, id: u16, name: &str) {
    let query = query_wire(id, name, A, None);
    rig.app.inject(packets::udp(app_udp(), responder(), &query));
}

/// The next datagram for the app, read as the responder's answer to it.
async fn heard(udp: &mut UnboundedReceiver<Vec<u8>>) -> Answer {
    let seen =
        packets::read_udp(&next(udp).await).expect("a whole datagram with the right checksums");
    assert_eq!(
        (seen.src, seen.dst),
        (responder(), app_udp()),
        "from the responder, to the app"
    );
    parse_response(&seen.payload).expect("an answer that reads")
}

/// `answer` says no address of any family, and no error: what a rule that
/// refuses the name is answered with.
fn assert_empty(answer: &Answer, id: u16) {
    assert_eq!(answer.id, id, "the ID of the query");
    assert_eq!(answer.rcode, Rcode::NoError);
    assert!(!answer.truncated, "{answer:?}");
    assert!(answer.v4.is_empty() && answer.v6.is_empty(), "{answer:?}");
}

/// The app asks for `name` and is given a fake IPv4 address.
async fn fake_of(rig: &Rig, udp: &mut UnboundedReceiver<Vec<u8>>, id: u16, name: &str) -> Ipv4Addr {
    ask(rig, id, name);
    let answer = heard(udp).await;
    assert_eq!(
        (answer.id, answer.rcode),
        (id, Rcode::NoError),
        "{answer:?}"
    );
    match answer.v4.as_slice() {
        [(address, 30)] => *address,
        other => panic!("one fake address with the TTL of 30 seconds, not {other:?}"),
    }
}

/// Nothing is on its way to the app (see `NOTHING`).
async fn quiet(udp: &mut UnboundedReceiver<Vec<u8>>) {
    tokio::time::sleep(NOTHING).await;
    assert!(
        matches!(udp.try_recv(), Err(TryRecvError::Empty)),
        "a datagram for the app, which nothing was to answer"
    );
}

/// The rules for what comes in at a proxy port: the name `target.test` goes to
/// `DIRECT`, and after that all of rurge's space and the port of the upstream
/// are refused (plan P24).
fn listener_rules(upstream_port: u16) -> String {
    format!(
        "DOMAIN,target.test,DIRECT\nIP-CIDR,198.18.0.0/15,REJECT,no-resolve\nDEST-PORT,{upstream_port},REJECT"
    )
}

/// `CONNECT authority` to the proxy at `proxy`, and the start of the answer,
/// `HTTP/1.1 502` for one; an error where the proxy closes without one.
async fn connect_answer(proxy: SocketAddr, authority: &str) -> io::Result<String> {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut status = [0u8; 12];
    tokio::time::timeout(WITHIN, s.read_exact(&mut status))
        .await
        .expect("the proxy answers or closes")?;
    Ok(String::from_utf8_lossy(&status).into_owned())
}

/// `CONNECT to:port` through rurge's SOCKS5 listener with the address as an
/// address (type 1, not as a name): the stream, and the reply code of the proxy.
async fn socks5_connect(proxy: SocketAddr, to: Ipv4Addr, port: u16) -> (TcpStream, u8) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    tokio::time::timeout(WITHIN, s.read_exact(&mut method))
        .await
        .expect("the proxy chooses a method")
        .unwrap();
    assert_eq!(method, [5, 0]);
    let mut request = vec![5, 1, 0, 1];
    request.extend_from_slice(&to.octets());
    request.extend_from_slice(&port.to_be_bytes());
    s.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(WITHIN, s.read_exact(&mut reply))
        .await
        .expect("the proxy answers")
        .unwrap();
    (s, reply[1])
}

/// How many addresses of the pool hold a lease (`FakeIpPool::leased`): each
/// session that connected to a fake address holds one until it ends.
fn leased(rig: &Rig) -> usize {
    rig.tun.pool().expect("a pool").leased()
}

/// Above 65535, so that no port number a log line carries can contain it.
const APP_PID: u32 = 4_000_321;

/// The platform's lookup, as far as the engine can tell: it finds the app for
/// every socket and notes what it is asked.
#[derive(Default)]
struct Asked(Mutex<Vec<OwnerQuery>>);

impl Asked {
    fn queries(&self) -> Vec<OwnerQuery> {
        self.0.lock().unwrap().clone()
    }
}

impl OwnerLookup for Asked {
    fn is_local(&self, _ip: IpAddr) -> Option<bool> {
        Some(true)
    }

    fn owner(&self, query: &OwnerQuery) -> Option<ProcessInfo> {
        self.0.lock().unwrap().push(query.clone());
        Some(ProcessInfo {
            pid: Some(APP_PID),
            name: "app.exe".to_string(),
            path: None,
        })
    }
}

// ---- the responder

/// The app asks the responder for a name, is given a fake address, and connects
/// to it: the session is for the name, which the engine dials, and the record
/// says so.
#[tokio::test]
async fn a_name_asked_through_the_responder_connects_to_its_upstream() {
    let echo = echo_server().await;
    let rules = guarded("DOMAIN,target.test,DIRECT");
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    let fake = fake_of(&rig, &mut udp, 7, "target.test").await;
    let mut tcp = rig
        .app
        .connect(SocketAddr::new(IpAddr::V4(fake), echo.port()))
        .await
        .unwrap();
    echoes(&mut tcp, b"through the adapter, by the name it was given").await;
    close(tcp).await;
    let records = tun_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "the query is no session: {records:?}");
    let record = &records[0];
    assert_eq!(record.dst, format!("target.test:{}", echo.port()));
    assert_eq!(record.listener, ListenerKind::Tun);
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert_eq!(record.rule.as_deref(), Some("DOMAIN,target.test,DIRECT"));
    assert!(record.notes.is_empty(), "{:?}", record.notes);
}

// ---- pre-matching at the DNS layer

/// A rule that pre-matches a name is answered with an empty answer, the second
/// time as the first, and the request log has the refusal once: the name with
/// no port, the rule, the policy and the note that says where it came from.
/// The session log line says it once, at INFO, and no other line above DEBUG
/// has the name.
#[tokio::test]
async fn pre_matching_at_the_dns_layer_answers_empty_and_records_once() {
    // held across the awaits: what the engine logs for this test is logged on
    // this thread
    let log = LogCapture::start();
    let rules = guarded(ADS);
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    for id in [1, 2] {
        ask(&rig, id, "x.ads.test");
        assert_empty(&heard(&mut udp).await, id);
    }
    let records = tun_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "two queries, one record: {records:?}");
    let record = &records[0];
    assert_eq!(record.dst, "x.ads.test", "the name, without a port");
    assert_eq!(record.listener, ListenerKind::Tun);
    assert_eq!(record.transport, rurge_config::session::Transport::Udp);
    assert_eq!(record.src, app_udp());
    assert_eq!(record.listen_port, 0);
    assert_eq!(record.notes, ["pre-matching at DNS"]);
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".to_string()));
    assert_eq!(record.policy, ["REJECT"]);
    let rule = record.rule.as_deref().expect("the rule is named");
    assert!(rule.starts_with("DOMAIN-SUFFIX,ads.test,REJECT"), "{rule}");
    assert!(record.process.found().is_none());

    let lines = log.lines();
    let said: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("session rejected"))
        .collect();
    assert_eq!(said.len(), 1, "{lines:?}");
    assert!(said[0].starts_with("INFO "), "{said:?}");
    assert!(said[0].contains("dst=x.ads.test "), "no port: {said:?}");
    for line in &lines {
        let above_debug = line.starts_with("INFO ") || line.starts_with("WARN ");
        if above_debug && !line.contains("session rejected") {
            assert!(!line.contains("ads.test"), "a name above DEBUG: {line}");
        }
    }
}

/// A rule that refers to a set, and matches by a line of it, says which line, in
/// a note of its own after the one that says where the refusal came from.
#[tokio::test]
async fn the_line_of_a_set_that_made_a_rule_match_is_noted_after_the_layer() {
    let sets = tempfile::tempdir().unwrap();
    let list = sets.path().join("ads.list");
    std::fs::write(&list, "DOMAIN-SUFFIX,ads.test\n").unwrap();
    let rules = guarded(&format!("RULE-SET,{},REJECT,pre-matching", list.display()));
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    ask(&rig, 1, "x.ads.test");
    assert_empty(&heard(&mut udp).await, 1);
    let record = tun_records(&rig.h, 1).await.remove(0);
    assert_eq!(
        record.notes,
        [
            "pre-matching at DNS".to_string(),
            format!(
                "Sub-rule matched: DOMAIN-SUFFIX,ads.test (in {})",
                list.display()
            )
        ]
    );
    assert_eq!(record.error, None, "a note explains no failure");
}

/// `REJECT-DROP` is answered with nothing at all, and the refusal is recorded
/// with its own name. (What is waited for is described at `NOTHING`; the second
/// query, a name that is answered, is behind the first and so shows that the
/// first has been decided.)
#[tokio::test]
async fn pre_matching_with_reject_drop_answers_nothing() {
    let rules = guarded("DOMAIN-SUFFIX,ads.test,REJECT-DROP,pre-matching");
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    ask(&rig, 1, "x.ads.test");
    fake_of(&rig, &mut udp, 2, "target.test").await;
    quiet(&mut udp).await;
    let record = tun_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.dst, "x.ads.test");
    assert_eq!(
        record.status,
        RecordStatus::Rejected("REJECT-DROP".to_string())
    );
    assert_eq!(record.policy, ["REJECT-DROP"]);
    assert_eq!(record.notes, ["pre-matching at DNS"]);
}

/// `REJECT-NO-DROP` is answered with the address of rurge's own that refuses
/// what comes to it, `198.18.0.244`, with the TTL of a fake address.
#[tokio::test]
async fn pre_matching_with_reject_no_drop_answers_244() {
    let rules = guarded("DOMAIN-SUFFIX,ads.test,REJECT-NO-DROP,pre-matching");
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    ask(&rig, 1, "x.ads.test");
    let answer = heard(&mut udp).await;
    assert_eq!((answer.id, answer.rcode), (1, Rcode::NoError));
    assert_eq!(answer.v4, [(Ipv4Addr::new(198, 18, 0, 244), 30)]);
    assert!(answer.v6.is_empty());
    let record = tun_records(&rig.h, 1).await.remove(0);
    assert_eq!(
        record.status,
        RecordStatus::Rejected("REJECT-NO-DROP".to_string())
    );
}

/// The app asks one more time than the fiftieth for a name that a rule with
/// `policy` pre-matches, the queries one straight after another, and then for a
/// name that is answered, which is the end of the answers. The rig, and what the
/// app heard of the queries for the first name, in the order it came.
async fn asked_over_and_over(policy: &str) -> (Rig, Vec<Answer>) {
    let rules = guarded(&format!("DOMAIN-SUFFIX,ads.test,{policy},pre-matching"));
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    for id in 1..=ASKED {
        ask(&rig, id, "x.ads.test");
    }
    let end = 1000;
    ask(&rig, end, "target.test");
    let mut answers = Vec::new();
    loop {
        let answer = heard(&mut udp).await;
        if answer.id == end {
            break;
        }
        answers.push(answer);
    }
    quiet(&mut udp).await;
    (rig, answers)
}

/// How many times the name is asked: one more than the fiftieth.
const ASKED: u16 = ESCALATE_COUNT as u16 + 1;

/// The first forty-nine queries are answered with nothing in the answer, and the
/// fifty-first is not answered. The fiftieth, the one that crosses the line, is
/// left out: whether it is answered is the order in which the engine counts and
/// decides, and is no business of these tests.
fn assert_silent_from_the_line(answers: &[Answer]) {
    let ids: Vec<u16> = answers.iter().map(|answer| answer.id).collect();
    for answer in answers {
        assert_empty(answer, answer.id);
    }
    for id in 1..ASKED - 1 {
        assert!(ids.contains(&id), "query {id} is answered: {ids:?}");
    }
    assert!(
        !ids.contains(&ASKED),
        "query {ASKED} is answered in silence: {ids:?}"
    );
}

/// A name that a pre-matching rule keeps refusing is refused in silence from
/// the fiftieth time in thirty seconds, as a session to it is
/// (`REJECT-DROP` is what `REJECT` is upgraded to). The refusals that are not
/// recorded count all the same.
#[tokio::test]
async fn pre_matching_escalates_to_silence() {
    let (rig, answers) = asked_over_and_over("REJECT").await;
    assert_silent_from_the_line(&answers);
    // however many were refused, the log has one record of the rule
    let records = tun_records(&rig.h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].status,
        RecordStatus::Rejected("REJECT".to_string()),
        "the first of them was a plain REJECT"
    );
}

/// `REJECT-TINYGIF` is counted as `REJECT` is.
#[tokio::test]
async fn pre_matching_with_reject_tinygif_escalates_to_silence() {
    let (_, answers) = asked_over_and_over("REJECT-TINYGIF").await;
    assert_silent_from_the_line(&answers);
}

/// `REJECT-NO-DROP` is never upgraded, however often it refuses: every query
/// is answered with the address of rurge's own.
#[tokio::test]
async fn pre_matching_with_reject_no_drop_is_never_upgraded() {
    let (_, answers) = asked_over_and_over("REJECT-NO-DROP").await;
    // every query is answered; the order they come in is no business of this
    let mut ids: Vec<u16> = answers.iter().map(|answer| answer.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, (1..=ASKED).collect::<Vec<_>>());
    for answer in &answers {
        assert_eq!(answer.v4, [(Ipv4Addr::new(198, 18, 0, 244), 30)]);
    }
}

/// A refusal that is put in the request log looks up the process behind it;
/// one that is not, because its rule was recorded a moment ago, looks up
/// nothing.
#[tokio::test]
async fn only_a_recorded_refusal_looks_up_its_process() {
    let asked = Arc::new(Asked::default());
    let rules = guarded(ADS);
    let h = harness_with(
        profile(&rules),
        EngineShared {
            owners: Some(asked.clone()),
            ..EngineShared::default()
        },
    )
    .await;
    let rig = Rig::on(h).await;
    let mut udp = rig.app.divert_udp();
    for id in 1..=3 {
        ask(&rig, id, "x.ads.test");
        assert_empty(&heard(&mut udp).await, id);
    }
    wait_until("the lookup of the recorded refusal", || {
        !asked.queries().is_empty()
    })
    .await;
    // what is waited for is a lookup of the refusals that were not recorded;
    // they are begun as the refusal is decided, so 300 ms is far more than
    // there is need of
    tokio::time::sleep(NOTHING).await;
    let queries = asked.queries();
    assert_eq!(queries.len(), 1, "{queries:?}");
    assert_eq!(queries[0].local, app_udp());
    assert_eq!(queries[0].remote, None, "a UDP socket has no other end");
    let record = tun_records(&rig.h, 1).await.remove(0);
    wait_until("the process on the record", || {
        rig.h
            .engine
            .request_log()
            .recent(10)
            .iter()
            .any(|r| r.process.found().and_then(|process| process.pid) == Some(APP_PID))
    })
    .await;
    assert_eq!(record.dst, "x.ads.test");
}

/// The outbound mode is the one now, not the one the generation was built with:
/// in direct mode nothing is pre-matched and the name is given an address, in
/// rule mode again it is refused.
#[tokio::test]
async fn pre_matching_follows_the_live_outbound_mode() {
    let rules = guarded(ADS);
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    ask(&rig, 1, "x.ads.test");
    assert_empty(&heard(&mut udp).await, 1);
    rig.h.engine.set_mode(Mode::Direct).await;
    fake_of(&rig, &mut udp, 2, "x.ads.test").await;
    rig.h.engine.set_mode(Mode::Rule).await;
    ask(&rig, 3, "x.ads.test");
    assert_empty(&heard(&mut udp).await, 3);
}

/// The log of refusals is per generation: the same rule is recorded once in
/// five minutes of a generation, and again in the next, however soon it comes.
#[tokio::test]
async fn a_new_generation_records_again() {
    let rules = guarded(ADS);
    let rig = Rig::new(profile(&rules)).await;
    let mut udp = rig.app.divert_udp();
    ask(&rig, 1, "x.ads.test");
    assert_empty(&heard(&mut udp).await, 1);
    assert_eq!(tun_records(&rig.h, 1).await.len(), 1);

    let text = profile(&rules).text(rig.h.dns.addr());
    rig.h
        .engine
        .swap_runtime(runtime(rig.data_dir(), &text, rig.h.engine.shared()).await);
    ask(&rig, 2, "x.ads.test");
    assert_empty(&heard(&mut udp).await, 2);
    let records = tun_records(&rig.h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_eq!(record.notes, ["pre-matching at DNS"]);
        assert_eq!(record.dst, "x.ads.test");
    }
    // and in the generation that is, a third is no record
    ask(&rig, 3, "x.ads.test");
    assert_empty(&heard(&mut udp).await, 3);
    assert_eq!(finished_through_the_tun(&rig.h).len(), 2);
}

// ---- pre-matching at the SYN layer

/// A rule that needs the port is no refusal of the query, which has none: the
/// name is given an address. The connection to the address is refused at its
/// first packet for the port the rule names, with a reset, and recorded; to
/// another port it is let through to the upstream.
#[tokio::test]
async fn pre_matching_at_the_syn_layer_resets_only_the_port_it_names() {
    let echo = echo_server().await;
    let named = closed_tcp_port();
    let rules = guarded(&format!(
        "AND,((DOMAIN,x.test),(DEST-PORT,{named})),REJECT,pre-matching\nDOMAIN,x.test,DIRECT"
    ));
    let rig = Rig::new(profile(&rules)).await;
    rig.h.dns.set("x.test", &["127.0.0.1"], &[], 60);
    let mut udp = rig.app.divert_udp();
    let fake = IpAddr::V4(fake_of(&rig, &mut udp, 1, "x.test").await);

    let refused = rig
        .app
        .connect(SocketAddr::new(fake, named))
        .await
        .expect_err("a reset for the SYN");
    assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
    let record = tun_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.dst, format!("x.test:{named}"));
    assert_eq!(record.notes, ["pre-matching at SYN"]);
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".to_string()));
    assert_eq!(record.policy, ["REJECT"]);
    assert_eq!(record.transport, rurge_config::session::Transport::Tcp);
    let rule = record.rule.as_deref().expect("the rule is named");
    assert!(rule.starts_with("AND,"), "{rule}");

    let mut tcp = rig
        .app
        .connect(SocketAddr::new(fake, echo.port()))
        .await
        .expect("another port is let through");
    echoes(&mut tcp, b"a port the rule does not name").await;
    close(tcp).await;
    let records = tun_records(&rig.h, 2).await;
    assert_eq!(records[1].dst, format!("x.test:{}", echo.port()));
    assert_eq!(records[1].status, RecordStatus::Completed, "{records:?}");
    assert!(records[1].notes.is_empty());
}

/// A connection to an address is refused at its first packet as well, by a
/// rule that names the address: the record has the address and no name.
#[tokio::test]
async fn pre_matching_at_the_syn_layer_refuses_an_address_too() {
    let rules = guarded("IP-CIDR,192.0.2.0/24,REJECT,pre-matching");
    let rig = Rig::new(profile(&rules)).await;
    let refused = rig
        .app
        .connect("192.0.2.10:80".parse().unwrap())
        .await
        .expect_err("a reset for the SYN");
    assert_eq!(refused.kind(), io::ErrorKind::ConnectionRefused);
    let record = tun_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.dst, "192.0.2.10:80");
    assert_eq!(record.notes, ["pre-matching at SYN"]);
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".to_string()));
}

// ---- fake addresses that come in at a proxy port

/// A `CONNECT` to the fake address of a name, over IPv4 and over IPv6, is a
/// session for the name: the rules see the name, DIRECT dials it, and the
/// record is the name's. The address is held for the name as long as the
/// session lasts.
#[tokio::test]
async fn a_listener_session_to_a_fake_address_goes_to_its_name() {
    let echo = echo_server().await;
    let rules = listener_rules(echo.port());
    let rig = Rig::new(profile(&rules)).await;
    let (v4, v6) = rig.fakes("target.test");
    assert_eq!(leased(&rig), 0);
    for (n, fake) in [v4, v6].into_iter().enumerate() {
        let authority = SocketAddr::new(fake, echo.port()).to_string();
        let mut tunnel = connect_via_http(rig.h.http(), &authority).await;
        echo_through(&mut tunnel, b"through the proxy port, to a fake address").await;
        assert_eq!(leased(&rig), 1, "the address is held while the session is");
        drop(tunnel);
        let records = the_records(&rig.h, n + 1).await;
        let record = &records[n];
        assert_eq!(record.dst, format!("target.test:{}", echo.port()));
        assert_eq!(record.listener, ListenerKind::Http);
        assert_eq!(record.rule.as_deref(), Some("DOMAIN,target.test,DIRECT"));
        assert_eq!(record.policy, ["DIRECT"]);
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        wait_until("the address to be let go", || leased(&rig) == 0).await;
    }
}

/// An address of rurge's space that no name is behind is a failure of the
/// session, which says so and dials nothing: the client is answered 502.
#[tokio::test]
async fn a_listener_session_to_an_unknown_fake_address_fails() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = upstream.local_addr().unwrap().port();
    let dialed = Arc::new(AtomicUsize::new(0));
    tokio::spawn({
        let dialed = dialed.clone();
        async move {
            while upstream.accept().await.is_ok() {
                dialed.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let rules = listener_rules(port);
    let rig = Rig::new(profile(&rules)).await;
    let answer = connect_answer(rig.h.http(), &format!("198.18.200.1:{port}"))
        .await
        .expect("an answer");
    assert_eq!(answer, "HTTP/1.1 502");
    let record = the_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.dst, format!("198.18.200.1:{port}"));
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(record.error.as_deref(), Some(FAKE_ADDRESS_NOT_KNOWN));
    assert_eq!(FAKE_ADDRESS_NOT_KNOWN, "fake address not known");
    assert_eq!(record.listener, ListenerKind::Http);
    assert_eq!(record.rule, None, "no rule was asked");
    assert!(record.policy.is_empty(), "{:?}", record.policy);
    // the session ended before anything was dialed
    assert_eq!(dialed.load(Ordering::SeqCst), 0);
}

/// A SOCKS5 `CONNECT` to the fake address of a name is a session for the name
/// as an HTTP one is, and holds the address as long as it lasts.
#[tokio::test]
async fn a_socks5_session_to_a_fake_address_goes_to_its_name() {
    let echo = echo_server().await;
    let rules = listener_rules(echo.port());
    let rig = Rig::new(profile(&rules)).await;
    let (IpAddr::V4(fake), _) = rig.fakes("target.test") else {
        panic!("an IPv4 address");
    };
    let (mut tunnel, reply) = socks5_connect(rig.h.socks(), fake, echo.port()).await;
    assert_eq!(reply, 0, "the connection is made");
    echo_through(&mut tunnel, b"through the SOCKS5 port, to a fake address").await;
    assert_eq!(leased(&rig), 1, "the address is held while the session is");
    drop(tunnel);
    let record = the_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.dst, format!("target.test:{}", echo.port()));
    assert_eq!(record.listener, ListenerKind::Socks5);
    assert_eq!(record.rule.as_deref(), Some("DOMAIN,target.test,DIRECT"));
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    wait_until("the address to be let go", || leased(&rig) == 0).await;
}

/// A SOCKS5 `CONNECT` to an address that no name is behind is refused, and the
/// record says why.
#[tokio::test]
async fn a_socks5_session_to_an_unknown_fake_address_is_refused() {
    let echo = echo_server().await;
    let rules = listener_rules(echo.port());
    let rig = Rig::new(profile(&rules)).await;
    let (_, reply) = socks5_connect(rig.h.socks(), Ipv4Addr::new(198, 19, 255, 254), 80).await;
    assert_eq!(reply, 5, "connection refused");
    let record = the_records(&rig.h, 1).await.remove(0);
    assert_eq!(record.dst, "198.19.255.254:80");
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(record.error.as_deref(), Some(FAKE_ADDRESS_NOT_KNOWN));
}

/// While a TUN runs, a session at a proxy port to an address outside rurge's
/// space, or to a name, is as it is without one: the pool has nothing to say.
#[tokio::test]
async fn an_address_outside_rurges_space_is_no_business_of_the_pool() {
    let echo = echo_server().await;
    let rig = Rig::new(profile("DOMAIN,target.test,DIRECT")).await;
    for (n, authority) in [echo.to_string(), format!("target.test:{}", echo.port())]
        .iter()
        .enumerate()
    {
        let mut tunnel = connect_via_http(rig.h.http(), authority).await;
        echo_through(&mut tunnel, b"through the proxy port, as ever").await;
        assert_eq!(leased(&rig), 0);
        drop(tunnel);
        let record = the_records(&rig.h, n + 1).await.remove(n);
        assert_eq!(&record.dst, authority);
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    }
}

/// The pool is the engine's for as long as the TUN runs: once the TUN has ended
/// and its pool is saved, an address of rurge's space that comes in at a proxy
/// port is an address like any other again.
#[tokio::test]
async fn the_fake_addresses_are_turned_back_while_the_tun_runs_and_not_after() {
    let echo = echo_server().await;
    let rules = listener_rules(echo.port());
    let rig = Rig::new(profile(&rules)).await;
    let authority = format!("198.18.200.1:{}", echo.port());
    let answer = connect_answer(rig.h.http(), &authority).await;
    assert_eq!(answer.unwrap(), "HTTP/1.1 502", "unknown to the pool");
    rig.tun.stop();
    assert_eq!(ends(&rig.tun).await, Ending::Stopped);
    // The pool is let go once its last save is written, a moment after the TUN
    // has ended: asked again and again, the proxy turns from saying that the
    // address is unknown to closing the connection of a session the rules
    // refused.
    let deadline = std::time::Instant::now() + WITHIN;
    while connect_answer(rig.h.http(), &authority).await.is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "the engine still holds the pool of a TUN that has ended"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let record = the_records(&rig.h, 1).await.pop().expect("a record");
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".to_string()));
}

/// Without a TUN there is no pool, and an address of rurge's space is an
/// address like any other: the rules see it, and here they refuse it.
#[tokio::test]
async fn without_a_tun_a_fake_address_is_an_address() {
    let echo = echo_server().await;
    let rules = listener_rules(echo.port());
    let h = harness(profile(&rules)).await;
    let answer = connect_answer(h.http(), &format!("198.18.200.1:{}", echo.port())).await;
    assert!(
        answer.is_err(),
        "a refused connection is closed without an answer: {answer:?}"
    );
    let record = the_records(&h, 1).await.remove(0);
    assert_eq!(record.dst, format!("198.18.200.1:{}", echo.port()));
    assert_eq!(record.status, RecordStatus::Rejected("REJECT".to_string()));
    assert_ne!(record.error.as_deref(), Some(FAKE_ADDRESS_NOT_KNOWN));
    assert_eq!(
        record.rule.as_deref(),
        Some("IP-CIDR,198.18.0.0/15,REJECT,no-resolve")
    );
}

/// A datagram of a SOCKS5 association to the fake address of a name goes to the
/// name, and what the name answers comes back as from the fake address the
/// client wrote.
#[tokio::test]
async fn a_socks5_udp_flow_to_a_fake_address_goes_to_its_name_and_answers_from_it() {
    let (echo, seen) = udp_echo().await;
    let rules = listener_rules(echo.port());
    let rig = Rig::new(profile(&rules)).await;
    let (fake, _) = rig.fakes("target.test");
    let association = udp_associate(rig.h.socks()).await;
    association
        .send(&fake.to_string(), echo.port(), b"ping")
        .await;
    let (from, payload) = association.recv().await;
    assert_eq!(payload, b"ping");
    assert_eq!(from, SocketAddr::new(fake, echo.port()));
    // the one datagram reached the echo at the name's own address
    assert_eq!(seen.lock().unwrap().len(), 1);
    wait_until("the record of the flow", || {
        rig.h
            .engine
            .request_log()
            .active()
            .iter()
            .any(|record| record.dst == format!("target.test:{}", echo.port()))
    })
    .await;
    let active = rig.h.engine.request_log().active();
    let flow = active
        .iter()
        .find(|record| record.dst == format!("target.test:{}", echo.port()))
        .unwrap();
    assert_eq!(flow.listener, ListenerKind::Socks5);
    assert_eq!(flow.transport, rurge_config::session::Transport::Udp);
    assert_eq!(flow.rule.as_deref(), Some("DOMAIN,target.test,DIRECT"));
    assert_eq!(leased(&rig), 1, "the address is held while the flow is");
}

/// A datagram of a SOCKS5 association to a fake address that no name is behind
/// fails the flow as any flow fails: nothing comes back, the record says why,
/// and nothing was sent.
#[tokio::test]
async fn a_socks5_udp_flow_to_an_unknown_fake_address_fails() {
    let (echo, seen) = udp_echo().await;
    let rules = listener_rules(echo.port());
    let rig = Rig::new(profile(&rules)).await;
    let association = udp_associate(rig.h.socks()).await;
    association.send("198.18.200.1", echo.port(), b"ping").await;
    wait_until("the record of the flow", || !udp_records(&rig.h).is_empty()).await;
    let record = udp_records(&rig.h).remove(0);
    assert_eq!(record.dst, format!("198.18.200.1:{}", echo.port()));
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    assert_eq!(record.error.as_deref(), Some(FAKE_ADDRESS_NOT_KNOWN));
    assert!(
        association.quiet_for(NOTHING).await,
        "an answer to a datagram that went nowhere"
    );
    assert!(seen.lock().unwrap().is_empty(), "something was sent");
}

/// A reply is shown as from the fake address only when it is known to answer
/// the flow that was sent to it. The flow to the name and a flow to the
/// name's own address send to the same place, so that the reply to the second
/// may be for either: it keeps the address it came from.
#[tokio::test]
async fn a_reply_that_may_be_for_either_flow_keeps_its_real_source() {
    let (echo, _) = udp_echo().await;
    // the second flow is to the loopback address, which the harness's
    // `FINAL,DIRECT` takes; the fake address is the one the rules refuse unless
    // it is turned back into the name
    let rules = "DOMAIN,target.test,DIRECT\nIP-CIDR,198.18.0.0/15,REJECT,no-resolve";
    let rig = Rig::new(profile(rules)).await;
    let (fake, _) = rig.fakes("target.test");
    let association = udp_associate(rig.h.socks()).await;
    association
        .send(&fake.to_string(), echo.port(), b"by the name")
        .await;
    let (from, _) = association.recv().await;
    assert_eq!(from, SocketAddr::new(fake, echo.port()), "the one flow");

    association
        .send("127.0.0.1", echo.port(), b"by the address")
        .await;
    let (from, payload) = association.recv().await;
    assert_eq!(payload, b"by the address");
    assert_eq!(
        from,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), echo.port()),
        "the reply may be for either of the two flows"
    );
}
