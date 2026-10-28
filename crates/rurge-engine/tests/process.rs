//! The process behind a session, through the engine (phase 3 M2 design 4): a
//! fake stands for the platform's lookup, behind the real HTTP and SOCKS5
//! listeners, the real rules and the real request log. Nothing here reads the
//! machine's sockets, and no clock is paused: the lookups run on blocking
//! threads, which a paused clock does not wait for.

mod common;
use common::*;
use rurge_config::HostName;
use rurge_config::session::{ProcessInfo, SessionInfo, Transport};
use rurge_engine::owner::{LOOKUPS, NOT_IN_TIME, SLOW};
use rurge_engine::{OwnerLookup, OwnerQuery, RequestRecord};
use rurge_net::connector::{Connector, DirectConnector};
use rurge_proto::testing::echo_server;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

/// Above 65535, so that no port number a log line carries can contain it.
const APP_PID: u32 = 4_000_321;
const APP_PATH: &str = "C:\\Apps\\app.exe";

/// The process every lookup that finds one finds.
fn app() -> ProcessInfo {
    ProcessInfo {
        pid: Some(APP_PID),
        name: "app.exe".to_string(),
        path: Some(APP_PATH.to_string()),
    }
}

/// Where a fake lookup waits until the test lets it by.
struct Doorway {
    open: Mutex<bool>,
    changed: Condvar,
}

impl Doorway {
    /// On the blocking thread: waits for the gate to be opened — at most
    /// `LONGEST`, so that a test that never opens it ends all the same.
    fn pass(&self) {
        const LONGEST: Duration = Duration::from_secs(30);
        let open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .changed
            .wait_timeout_while(open, LONGEST, |open| !*open)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// The test's own hold on a gate. The gate opens when the test says so, or
/// when this is dropped — as it is when the test fails: the engine holds the
/// fake that waits at the gate for as long as the runtime lives, and the
/// runtime waits for its blocking threads to end before it ends. A fake that
/// opened the gate when *it* is dropped would never get there.
struct Gate(Arc<Doorway>);

impl Gate {
    fn shut() -> Gate {
        Gate(Arc::new(Doorway {
            open: Mutex::new(false),
            changed: Condvar::new(),
        }))
    }

    fn open(&self) {
        *self.0.open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.changed.notify_all();
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.open();
    }
}

/// Counted in `in_flight` for as long as a lookup is under way, a panic
/// included.
struct InFlight<'a>(&'a AtomicUsize);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The platform's lookup, as far as the engine can tell. It answers what it
/// is told to, when it is let to, and counts what it is asked.
struct Fake {
    /// What `is_local` answers.
    local: Mutex<Option<bool>>,
    /// What `owner` finds.
    process: Mutex<Option<ProcessInfo>>,
    /// How long `owner` takes to answer, once past the gate.
    delay: Duration,
    doorway: Option<Arc<Doorway>>,
    /// `owner` panics (the platform failing in a way nobody expected).
    panics: bool,
    is_local_calls: AtomicUsize,
    owner_calls: AtomicUsize,
    in_flight: AtomicUsize,
    most_in_flight: AtomicUsize,
    queries: Mutex<Vec<OwnerQuery>>,
}

impl Fake {
    /// A lookup that finds `process` for every connection, at once, and says
    /// it cannot tell whether a source is this machine's.
    fn new(process: Option<ProcessInfo>) -> Fake {
        Fake {
            local: Mutex::new(None),
            process: Mutex::new(process),
            delay: Duration::ZERO,
            doorway: None,
            panics: false,
            is_local_calls: AtomicUsize::new(0),
            owner_calls: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            most_in_flight: AtomicUsize::new(0),
            queries: Mutex::new(Vec::new()),
        }
    }

    fn finding(process: ProcessInfo) -> Fake {
        Fake::new(Some(process))
    }

    /// Takes `delay` to answer, as a slow system does.
    fn after(self, delay: Duration) -> Fake {
        Fake { delay, ..self }
    }

    /// Waits at `gate` before it answers.
    fn behind(self, gate: &Gate) -> Fake {
        Fake {
            doorway: Some(gate.0.clone()),
            ..self
        }
    }

    fn failing(self) -> Fake {
        Fake {
            panics: true,
            ..self
        }
    }

    fn is_local_says(&self, answer: Option<bool>) {
        *self.local.lock().unwrap() = answer;
    }

    fn finds(&self, process: Option<ProcessInfo>) {
        *self.process.lock().unwrap() = process;
    }

    fn owner_calls(&self) -> usize {
        self.owner_calls.load(Ordering::SeqCst)
    }

    fn is_local_calls(&self) -> usize {
        self.is_local_calls.load(Ordering::SeqCst)
    }

    fn most_in_flight(&self) -> usize {
        self.most_in_flight.load(Ordering::SeqCst)
    }

    fn queries(&self) -> Vec<OwnerQuery> {
        self.queries.lock().unwrap().clone()
    }
}

impl OwnerLookup for Fake {
    fn is_local(&self, _ip: IpAddr) -> Option<bool> {
        self.is_local_calls.fetch_add(1, Ordering::SeqCst);
        *self.local.lock().unwrap()
    }

    fn owner(&self, query: &OwnerQuery) -> Option<ProcessInfo> {
        self.owner_calls.fetch_add(1, Ordering::SeqCst);
        self.queries.lock().unwrap().push(query.clone());
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_in_flight.fetch_max(now, Ordering::SeqCst);
        let _under_way = InFlight(&self.in_flight);
        if let Some(doorway) = &self.doorway {
            doorway.pass();
        }
        // a slow system: this runs on a blocking thread, which is what a
        // lookup that takes its time does to one
        std::thread::sleep(self.delay);
        if self.panics {
            // Unwinds as a panic does — the place is dropped on the way, and
            // tokio reports the blocking task as panicked — but without the
            // panic hook: with `RUST_BACKTRACE` set the hook captures and
            // symbolises a backtrace here, which alone takes longer than the
            // 200 ms the rules wait.
            std::panic::resume_unwind(Box::new("the platform's lookup failed"));
        }
        self.process.lock().unwrap().clone()
    }
}

fn shared_with(fake: &Arc<Fake>) -> EngineShared {
    EngineShared {
        owners: Some(fake.clone()),
        ..EngineShared::default()
    }
}

/// An engine that looks processes up with `fake`, with `rules` before the
/// `FINAL,DIRECT` of the profile.
async fn engine(rules: &str, fake: &Arc<Fake>) -> Harness {
    harness_with(
        Profile {
            rules,
            ..Profile::default()
        },
        shared_with(fake),
    )
    .await
}

/// A session through the HTTP listener that the rules let through: a round
/// trip to the echo, then the client goes.
async fn let_through(h: &Harness, echo: SocketAddr) {
    let mut tunnel = connect_via_http(h.http(), &echo.to_string()).await;
    echo_through(&mut tunnel, b"hello").await;
}

/// A `CONNECT` through the HTTP listener that the rules refuse: the listener
/// closes the connection, with no answer to it.
async fn refused_by_http(h: &Harness, echo: SocketAddr) {
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut answer = Vec::new();
    // a reset says as much as the end of the stream does
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut answer))
        .await
        .expect("the refusal comes within the bound");
    assert!(
        !answer.starts_with(b"HTTP/1.1 200"),
        "{}",
        String::from_utf8_lossy(&answer)
    );
}

/// A SOCKS5 `CONNECT` to the echo through the SOCKS5 listener; the reply code.
async fn socks5_reply(h: &Harness, echo: SocketAddr) -> u8 {
    let mut s = TcpStream::connect(h.socks()).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut method = [0u8; 2];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut method))
        .await
        .expect("the proxy chooses a method")
        .unwrap();
    let SocketAddr::V4(echo) = echo else {
        panic!("the echo is on IPv4 loopback")
    };
    let mut request = vec![5, 1, 0, 1];
    request.extend_from_slice(&echo.ip().octets());
    request.extend_from_slice(&echo.port().to_be_bytes());
    s.write_all(&request).await.unwrap();
    let mut reply = [0u8; 10];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut reply))
        .await
        .expect("the proxy answers")
        .unwrap();
    reply[1]
}

/// The records of the sessions that have finished, oldest first, once `n`
/// have.
async fn finished(h: &Harness, n: usize) -> Vec<RequestRecord> {
    wait_until("the sessions to finish", || {
        h.engine.request_log().recent(1000).len() >= n
    })
    .await;
    let mut records = h.engine.request_log().recent(1000);
    records.reverse();
    records
}

fn pid_of(record: &RequestRecord) -> Option<u32> {
    record.process.found().and_then(|p| p.pid)
}

fn gave_up_waiting(record: &RequestRecord) -> bool {
    record.notes.iter().any(|note| note == NOT_IN_TIME)
}

/// `PROCESS-NAME` rules are decided by the process the lookup finds, whether
/// the client came in through the HTTP listener or the SOCKS5 one: each
/// session is looked up by the client's own address and the address it
/// connected to.
#[tokio::test]
async fn a_process_rule_matches_through_both_listeners() {
    let fake = Arc::new(Fake::finding(app()));
    let h = engine("PROCESS-NAME,app.exe,REJECT", &fake).await;
    let echo = echo_server().await;
    refused_by_http(&h, echo).await;
    assert_eq!(
        socks5_reply(&h, echo).await,
        0x02,
        "SOCKS5 says: not allowed by the ruleset"
    );
    let records = finished(&h, 2).await;
    for (record, listener) in records
        .iter()
        .zip([ListenerKind::Http, ListenerKind::Socks5])
    {
        assert_eq!(record.listener, listener);
        assert_eq!(
            record.status,
            RecordStatus::Rejected("REJECT".to_string()),
            "{record:?}"
        );
        assert_eq!(record.rule.as_deref(), Some("PROCESS-NAME,app.exe,REJECT"));
        assert_eq!(record.process.found(), Some(&app()));
        assert!(!gave_up_waiting(record), "{record:?}");
    }
    assert_eq!(fake.owner_calls(), 2);
    let queries = fake.queries();
    for ((query, record), remote) in queries.iter().zip(&records).zip([h.http(), h.socks()]) {
        assert_eq!(query.transport, Transport::Tcp);
        assert_eq!(query.local, record.src, "the client's own endpoint");
        assert_eq!(query.remote, Some(remote), "the address it connected to");
    }
}

/// The lookup starts with the session, whatever the rules. A session that has
/// no use for the process does not wait for it (the lookup is stuck at a gate
/// here), and its record has the process whenever it comes.
#[tokio::test]
async fn a_session_that_needs_no_process_does_not_wait() {
    let gate = Gate::shut();
    let fake = Arc::new(Fake::finding(app()).behind(&gate));
    let h = engine("DOMAIN,never.test,REJECT", &fake).await;
    let echo = echo_server().await;
    let_through(&h, echo).await;
    // the lookup was started all the same, in the background
    wait_until("the lookup to start", || fake.owner_calls() == 1).await;
    let record = finished(&h, 1).await.remove(0);
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert_eq!(record.process.get(), None, "the lookup is at the gate");
    assert!(
        record.notes.is_empty(),
        "no rule asked for the process, so none gave up waiting: {record:?}"
    );
    gate.open();
    wait_until("the process on the record", || {
        pid_of(&h.engine.request_log().recent(1)[0]) == Some(APP_PID)
    })
    .await;
    let record = h.engine.request_log().recent(1).remove(0);
    assert_eq!(record.process.found(), Some(&app()));
}

/// A lookup that takes a while, inside the 200 ms the rules give it, is
/// waited for: the rule that names the process matches.
#[tokio::test]
async fn a_rule_waits_for_the_lookup() {
    let fake = Arc::new(Fake::finding(app()).after(Duration::from_millis(50)));
    let h = engine("PROCESS-NAME,app.exe,REJECT", &fake).await;
    let echo = echo_server().await;
    refused_by_http(&h, echo).await;
    let record = finished(&h, 1).await.remove(0);
    assert_eq!(record.rule.as_deref(), Some("PROCESS-NAME,app.exe,REJECT"));
    assert_eq!(record.process.found(), Some(&app()));
    assert!(!gave_up_waiting(&record), "{record:?}");
    // it did wait for the lookup, which took 50 ms
    assert!(record.elapsed_ms >= 40, "{record:?}");
}

/// A lookup that does not answer in time is decided without: the rule that
/// names the process does not match, the session goes on to the next rule,
/// and its record says why. The lookup goes on all the same, and its answer
/// is on the record when it comes.
#[tokio::test]
async fn a_slow_lookup_is_decided_without_the_process() {
    let gate = Gate::shut();
    let fake = Arc::new(Fake::finding(app()).behind(&gate));
    let h = engine("PROCESS-NAME,app.exe,REJECT", &fake).await;
    let echo = echo_server().await;
    // not refused: the lookup is at the gate, and the rules do not wait for ever
    let_through(&h, echo).await;
    let record = finished(&h, 1).await.remove(0);
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    assert_eq!(record.rule.as_deref(), Some("FINAL,DIRECT"));
    assert!(gave_up_waiting(&record), "{record:?}");
    assert_eq!(record.process.get(), None);
    // it waited, for the 200 ms, and not for much longer
    let connect_ms = record.connect_ms.expect("the session was dialled");
    assert!((150..3000).contains(&connect_ms), "{record:?}");
    gate.open();
    wait_until("the process on the record", || {
        pid_of(&h.engine.request_log().recent(1)[0]) == Some(APP_PID)
    })
    .await;
    let record = h.engine.request_log().recent(1).remove(0);
    assert_eq!(record.process.found(), Some(&app()));
    assert!(gave_up_waiting(&record), "the note stays: {record:?}");
}

/// The first time the rules give up on a lookup the engine says so, once, in
/// words that name no session, no address and no process; the sessions say
/// it in their own records and in DEBUG lines from then on.
#[tokio::test]
async fn the_first_slow_lookup_is_warned_once() {
    // held across the awaits: the engine's tasks run on this thread
    let log = LogCapture::start();
    let gate = Gate::shut();
    let fake = Arc::new(Fake::finding(app()).behind(&gate));
    let h = engine("PROCESS-NAME,app.exe,REJECT", &fake).await;
    let echo = echo_server().await;
    let_through(&h, echo).await;
    let_through(&h, echo).await;
    let records = finished(&h, 2).await;
    assert!(records.iter().all(gave_up_waiting), "{records:?}");
    let lines = log.lines();
    let warned: Vec<&String> = lines.iter().filter(|l| l.starts_with("WARN")).collect();
    assert_eq!(warned, [&format!("WARN {SLOW}")], "{lines:?}");
    let gave_up: Vec<&String> = lines.iter().filter(|l| l.contains(NOT_IN_TIME)).collect();
    assert_eq!(gave_up.len(), 2, "{gave_up:?}");
    for (line, record) in gave_up.iter().zip(&records) {
        assert!(
            line.starts_with(&format!("DEBUG {NOT_IN_TIME}")),
            "the note is a DEBUG line: {line}"
        );
        assert!(
            line.ends_with(&format!(" session={}", record.id)),
            "the session's own: {line}"
        );
    }
}

/// A source that is surely not this machine's has no process to find: the
/// engine does not start a lookup for it, and the session's slot is settled
/// at once with none. A source that cannot be told is looked up as usual.
#[tokio::test]
async fn a_source_that_is_not_local_is_not_looked_up() {
    let fake = Arc::new(Fake::finding(app()));
    fake.is_local_says(Some(false));
    let h = engine("PROCESS-NAME,app.exe,REJECT", &fake).await;
    let echo = echo_server().await;
    // the rule cannot match: no process, and no waiting for one
    let_through(&h, echo).await;
    let record = finished(&h, 1).await.remove(0);
    assert_eq!(record.rule.as_deref(), Some("FINAL,DIRECT"));
    assert_eq!(
        record.process.get(),
        Some(&None),
        "settled, with no process"
    );
    assert!(record.notes.is_empty(), "{record:?}");
    assert_eq!(fake.is_local_calls(), 1);
    assert_eq!(fake.owner_calls(), 0);
    // "cannot tell" is a reason to look
    fake.is_local_says(None);
    refused_by_http(&h, echo).await;
    let records = finished(&h, 2).await;
    assert_eq!(
        records[1].rule.as_deref(),
        Some("PROCESS-NAME,app.exe,REJECT")
    );
    assert_eq!(pid_of(&records[1]), Some(APP_PID));
    assert_eq!(fake.owner_calls(), 1);
}

/// rurge's own sessions are not looked up (they have no process but rurge),
/// and a rule that names a process does not match them.
#[tokio::test]
async fn an_internal_session_is_not_looked_up() {
    let fake = Arc::new(Fake::finding(app()));
    let h = engine("PROCESS-NAME,app.exe,REJECT", &fake).await;
    let echo = echo_server().await;
    let mut session = SessionInfo::tcp(HostName::Ip(echo.ip()), echo.port());
    session.listener = ListenerKind::Internal;
    let fallback: Arc<dyn Connector> = Arc::new(DirectConnector::new(h.engine.resolver()));
    let mut stream = h
        .engine
        .dial_internal(session, None, &fallback)
        .await
        .expect("an internal session is not refused for a process it has not got");
    stream.write_all(b"internal").await.unwrap();
    let mut back = [0u8; 8];
    tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut back))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(&back, b"internal");
    assert_eq!(fake.owner_calls(), 0);
    assert_eq!(fake.is_local_calls(), 0);
}

/// With no lookup at all — the engine as every test but these has it — no
/// session has a process, no rule waits for one, and the record says nothing
/// of it.
#[tokio::test]
async fn without_a_lookup_every_session_is_unknown() {
    let h = harness(Profile {
        rules: "PROCESS-NAME,app.exe,REJECT",
        ..Profile::default()
    })
    .await;
    let echo = echo_server().await;
    let_through(&h, echo).await;
    let record = finished(&h, 1).await.remove(0);
    assert_eq!(record.rule.as_deref(), Some("FINAL,DIRECT"));
    assert_eq!(record.process.get(), Some(&None));
    assert!(record.notes.is_empty(), "{record:?}");
}

/// A UDP association is looked up once, by its control connection, and its
/// flows carry the answer: a rule that names the process matches the flow to
/// either target.
#[tokio::test]
async fn a_udp_association_is_looked_up_once() {
    let fake = Arc::new(Fake::finding(app()));
    // a flow goes out only if the process matched: anything else to the
    // loopback is refused
    let h = engine(
        "PROCESS-NAME,app.exe,DIRECT\nIP-CIDR,127.0.0.0/8,REJECT,no-resolve",
        &fake,
    )
    .await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", one.port(), b"one").await;
    assert_eq!(association.recv().await, (one, b"one".to_vec()));
    association.send("127.0.0.1", two.port(), b"two").await;
    assert_eq!(association.recv().await, (two, b"two".to_vec()));
    let control = association.control.local_addr().unwrap();
    drop(association);
    wait_until("the flows to finish", || udp_records(&h).len() >= 2).await;
    let records = udp_records(&h);
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_eq!(pid_of(record), Some(APP_PID), "{record:?}");
        assert_eq!(record.rule.as_deref(), Some("PROCESS-NAME,app.exe,DIRECT"));
        assert!(!gave_up_waiting(record), "{record:?}");
    }
    assert_eq!(fake.owner_calls(), 1);
    // the control connection is the client's own socket, and a TCP one
    let queries = fake.queries();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].transport, Transport::Tcp);
    assert_eq!(queries[0].local, control);
    assert_eq!(queries[0].remote, Some(h.socks()));
}

/// The 200 ms run from the launch of the association's lookup: a flow that
/// is opened after that does not wait for it again, and is decided without
/// the process, with its note. It carries the same slot, so the process is on
/// its record when the lookup ends.
#[tokio::test]
async fn a_flow_opened_after_the_window_decides_without_waiting() {
    let gate = Gate::shut();
    let fake = Arc::new(Fake::finding(app()).behind(&gate));
    let h = engine("PROCESS-NAME,app.exe,REJECT", &fake).await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    // the first flow waits out the window, and goes on without the process
    association.send("127.0.0.1", one.port(), b"first").await;
    assert_eq!(association.recv().await, (one, b"first".to_vec()));
    // the second is opened when the window has long closed
    association.send("127.0.0.1", two.port(), b"second").await;
    assert_eq!(association.recv().await, (two, b"second".to_vec()));
    drop(association);
    wait_until("the flows to finish", || udp_records(&h).len() >= 2).await;
    let records = udp_records(&h);
    for record in &records {
        assert!(gave_up_waiting(record), "{record:?}");
        assert_eq!(record.rule.as_deref(), Some("FINAL,DIRECT"));
        assert_eq!(record.process.get(), None);
    }
    assert_eq!(fake.owner_calls(), 1, "one lookup for the association");
    gate.open();
    wait_until("the process on both records", || {
        udp_records(&h)
            .iter()
            .all(|record| pid_of(record) == Some(APP_PID))
    })
    .await;
}

/// A session that is over before its process is found has the process on its
/// record all the same, once it is found: the record reads the slot when it
/// is read. (A rejected session is often over in a flash.)
#[tokio::test]
async fn a_record_shows_a_process_found_after_the_session_ended() {
    let gate = Gate::shut();
    let fake = Arc::new(Fake::finding(app()).behind(&gate));
    // a rule that has no use for the process
    let h = engine("DOMAIN,ads.test,REJECT", &fake).await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT ads.test:443 HTTP/1.1\r\nHost: ads.test:443\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut answer))
        .await
        .expect("the refusal comes within the bound");
    let record = finished(&h, 1).await.remove(0);
    assert!(
        matches!(record.status, RecordStatus::Rejected(_)),
        "{record:?}"
    );
    assert_eq!(record.process.get(), None, "the lookup is at the gate");
    gate.open();
    wait_until("the process on the record", || {
        pid_of(&h.engine.request_log().recent(1)[0]) == Some(APP_PID)
    })
    .await;
    let record = h.engine.request_log().recent(1).remove(0);
    assert_eq!(record.process.found(), Some(&app()));
    assert!(record.notes.is_empty(), "{record:?}");
}

/// A burst of connections — a browser opening a page — each wants its
/// process, and the platform's lookup is slow: they share the `LOOKUPS`
/// places, none fails, each waits for its rule at most the 200 ms, and every
/// record has its process in the end.
#[tokio::test]
async fn a_burst_of_sessions_with_a_slow_lookup_all_complete() {
    const SESSIONS: usize = 32;
    let fake = Arc::new(Fake::finding(app()).after(Duration::from_millis(30)));
    let h = engine("PROCESS-NAME,app.exe,DIRECT", &fake).await;
    let echo = echo_server().await;
    let mut burst = tokio::task::JoinSet::new();
    for n in 0..SESSIONS {
        let proxy = h.http();
        burst.spawn(async move {
            let mut tunnel = connect_via_http(proxy, &echo.to_string()).await;
            echo_through(&mut tunnel, &[n as u8]).await;
        });
    }
    while let Some(done) = burst.join_next().await {
        done.expect("a session of the burst completes");
    }
    let records = finished(&h, SESSIONS).await;
    assert!(
        records.iter().all(|r| r.status == RecordStatus::Completed),
        "{records:?}"
    );
    wait_until("every record to show its process", || {
        h.engine
            .request_log()
            .recent(1000)
            .iter()
            .all(|r| pid_of(r) == Some(APP_PID))
    })
    .await;
    assert_eq!(fake.owner_calls(), SESSIONS);
    // two blocking threads at a time, the design's number: the constant is
    // what the engine goes by, and this is what the design says
    assert_eq!(LOOKUPS, 2);
    assert_eq!(
        fake.most_in_flight(),
        2,
        "the lookups share their two places"
    );
}

/// A lookup that fails (the platform panics) finds nothing: the session goes
/// on without a process, the rules do not wait for an answer that will not
/// come, and the lookup's place is free for the next.
#[tokio::test]
async fn a_lookup_that_fails_finds_no_process_and_gives_its_place_back() {
    let fake = Arc::new(Fake::finding(app()).failing());
    let h = engine("PROCESS-NAME,app.exe,REJECT", &fake).await;
    let echo = echo_server().await;
    // one more than there are places: a place that a failed lookup kept
    // would leave the last one waiting for the whole window
    for _ in 0..=LOOKUPS {
        let_through(&h, echo).await;
    }
    let records = finished(&h, LOOKUPS + 1).await;
    for record in &records {
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        assert_eq!(record.process.get(), Some(&None), "{record:?}");
        assert!(!gave_up_waiting(record), "{record:?}");
    }
    assert_eq!(fake.owner_calls(), LOOKUPS + 1);
}

/// A lookup says what it found, and how long it took, in a DEBUG line. The
/// PID, the name and the path are in that line and in no line above DEBUG:
/// the path of a program often has the user's name in it.
#[tokio::test]
async fn a_finished_lookup_is_logged_at_debug_and_nowhere_higher() {
    let log = LogCapture::start();
    let fake = Arc::new(Fake::finding(app()));
    // The rule's pattern is not the program's whole name: a line above DEBUG
    // may say which rule decided a session, and must not give the name away
    // that way.
    let h = engine("PROCESS-NAME,app*,REJECT", &fake).await;
    let echo = echo_server().await;
    refused_by_http(&h, echo).await;
    let said = |what: &str| {
        log.lines()
            .into_iter()
            .filter(|l| l.starts_with(&format!("DEBUG {what}")))
            .collect::<Vec<String>>()
    };
    wait_until("the lookup to be logged", || {
        !said("process lookup found the process").is_empty()
    })
    .await;
    let found = said("process lookup found the process");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains(" src=127.0.0.1:"), "{}", found[0]);
    assert!(
        found[0].contains(&format!(" remote={}", h.http())),
        "{}",
        found[0]
    );
    assert!(found[0].contains(" elapsed_ms="), "{}", found[0]);
    assert!(
        found[0].contains(&format!(" pid={APP_PID}")),
        "{}",
        found[0]
    );
    assert!(found[0].contains("app.exe"), "{}", found[0]);
    assert!(found[0].contains("Apps"), "{}", found[0]);
    // a lookup that finds none says so
    fake.finds(None);
    let_through(&h, echo).await;
    wait_until("the second lookup to be logged", || {
        !said("process lookup found no process").is_empty()
    })
    .await;
    let none = said("process lookup found no process");
    assert!(none[0].contains(" src=127.0.0.1:"), "{}", none[0]);
    assert!(!none[0].contains("pid="), "{}", none[0]);
    // none of it above DEBUG
    let lines = log.lines();
    let above: Vec<&String> = lines
        .iter()
        .filter(|l| !l.starts_with("DEBUG") && !l.starts_with("TRACE"))
        .collect();
    assert!(
        !above.is_empty(),
        "the refused session's INFO line is there"
    );
    // the PID, the name and the path
    for private in [APP_PID.to_string().as_str(), "app.exe", "Apps"] {
        for line in &above {
            assert!(!line.contains(private), "{private} is in: {line}");
        }
    }
}
