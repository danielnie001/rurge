//! `external` policies through the engine (phase 2 M4 design 7.4, 7.5,
//! 8.2): the program starts on the first dial, never on a check or a build;
//! a reload keeps an unchanged program; the exit flow stops them all.

mod common;
use common::*;
use rurge_config::config::{LoadOptions, from_text};
use rurge_config::rule::PolicyRef;
use rurge_config::session::ProcessInfo;
use rurge_dns::system::StaticSystemDns;
use rurge_engine::owner::{EXTERNAL, NOT_IN_TIME};
use rurge_engine::stack::StackOptions;
use rurge_engine::{
    Engine, EngineShared, OwnerLookup, OwnerQuery, RequestRecord, Runtime, RuntimeOptions,
};
use rurge_rules::{GeoUrls, OutboundMode};
use std::sync::Mutex;

/// `[General]` for every test: loopback listeners, nothing tested online.
const GENERAL: &str = "[General]\nhttp-listen = 127.0.0.1:0\nipv6 = false\n\
proxy-test-url = http://127.0.0.1:9/\ninternet-test-url = http://127.0.0.1:9/\n";

/// A profile sending everything to `Ext`, the helper with `extra` arguments
/// on `port`.
fn profile(port: u16, extra: &str) -> String {
    format!(
        "{GENERAL}[Proxy]\nExt = external, exec = \"{}\", args = --port, args = {port}{extra}, local-port = {port}\n\
[Rule]\nFINAL,Ext\n",
        helper().replace('\\', "\\\\")
    )
}

fn shared() -> EngineShared {
    EngineShared {
        processes: Arc::new(Platform),
        ..EngineShared::default()
    }
}

async fn runtime(dir: &Path, text: &str, shared: EngineShared) -> Runtime {
    runtime_in(dir, text, shared, OutboundMode::Rule).await
}

/// `runtime`, in the outbound mode the engine starts in.
async fn runtime_in(dir: &Path, text: &str, shared: EngineShared, mode: OutboundMode) -> Runtime {
    let path = dir.join("t.conf");
    std::fs::write(&path, text).unwrap();
    let loaded = from_text(text, &path, &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: StackOptions {
                data_dir: dir.to_path_buf(),
                no_network: true,
                geo_urls: GeoUrls::default(),
                dns_cache_size: 100,
                system: Arc::new(StaticSystemDns::default()),
                wait: Duration::ZERO,
                dns_connector: None,
                socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),
                dns_override: Default::default(),
                resource_resolve: None,
            },
            outbound_mode: mode,
            idle_timeout: Duration::from_secs(600),
            shared,
            request_log_size: 100,
        },
    )
    .await
    .unwrap()
}

struct Harness {
    dir: tempfile::TempDir,
    engine: Arc<Engine>,
    http: SocketAddr,
    _listeners: Vec<(rurge_engine::ListenerSpec, rurge_engine::Running)>,
}

async fn harness(text: &str) -> Harness {
    harness_in(text, shared(), OutboundMode::Rule).await
}

async fn harness_in(text: &str, shared: EngineShared, mode: OutboundMode) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(runtime_in(dir.path(), text, shared, mode).await);
    let listeners = engine.bind_listeners().await.unwrap();
    let http = listeners[0].1.local_addr;
    Harness {
        dir,
        engine,
        http,
        _listeners: listeners,
    }
}

/// `CONNECT` through rurge's HTTP listener to `echo`, one round trip.
async fn echo_via(proxy: SocketAddr, echo: SocketAddr) {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut byte))
            .await
            .expect("the proxy answers")
            .unwrap();
        assert!(n > 0, "closed: {:?}", String::from_utf8_lossy(&head));
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut buf))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(&buf, b"ping");
}

/// The program starts on the first dial; its log is in the data directory.
/// The connection the session's own dial made was to the program's port, and
/// the session's record says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_dial_starts_the_program() {
    let port = free_port();
    let h = harness(&profile(port, "")).await;
    echo_via(h.http, echo_server().await).await;
    let log = std::fs::read_to_string(h.dir.path().join("external").join("Ext.log")).unwrap();
    assert!(
        log.contains(&format!("socks-helper listening on 127.0.0.1:{port}")),
        "{log}"
    );
    // the record is written when the relay ends, after the client went
    let record = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(record) = h.engine.request_log().recent(1).pop() {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the session is recorded");
    assert_eq!(
        record.remote_addr,
        Some(SocketAddr::from(([127, 0, 0, 1], port))),
        "{record:?}"
    );
    h.engine.stop_external_programs().await;
    wait_closed(port).await;
}

/// A check and a build only check: nothing starts (M4 design 7.5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_check_or_a_build_starts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let record = dir.path().join("record");
    let text = profile(
        port,
        &format!(
            ", args = --record, args = \"{}\"",
            record.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    let path = dir.path().join("t.conf");
    std::fs::write(&path, &text).unwrap();
    let loaded = rurge_engine::load_checked(&path, &LoadOptions::for_tests()).unwrap();
    assert!(!loaded.diagnostics.has_errors());
    let engine = Engine::new(runtime(dir.path(), &text, shared()).await);
    assert_eq!(engine.registry().names().len(), 1);
    // give a start that should not happen the time to show
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!record.exists(), "a program was started");
}

/// A reload that leaves the line alone keeps the program; one that
/// changes it starts the new program and stops the old one once the old
/// generation is gone (M4 design 7.4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_keeps_an_unchanged_program_and_stops_a_replaced_one() {
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record");
    let extra = format!(
        ", args = --record, args = \"{}\"",
        record.to_string_lossy().replace('\\', "\\\\")
    );
    let h = harness(&profile(port, &extra)).await;
    let echo = echo_server().await;
    echo_via(h.http, echo).await;

    let unrelated = format!("{}\n[Host]\nx.test = 127.0.0.1\n", profile(port, &extra));
    h.engine
        .swap_runtime(runtime(h.dir.path(), &unrelated, h.engine.shared()).await);
    echo_via(h.http, echo).await;
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert_eq!(recorded.lines().count(), 1, "the same program: {recorded}");

    let other = free_port();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &profile(other, &extra), h.engine.shared()).await);
    echo_via(h.http, echo).await;
    wait_closed(port).await;
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert_eq!(recorded.lines().count(), 2, "{recorded}");
    h.engine.stop_external_programs().await;
    wait_closed(other).await;
}

/// A reload that changes the line but keeps its `local-port`: while the old
/// outbound is still held (an in-flight dial, a test, a session's hook), the
/// new program's first start stops the old program and its tree, so the new
/// one gets the port and serves; the old outbound never starts a program
/// again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reload_that_keeps_the_port_hands_it_to_the_new_program() {
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| {
        dir.path()
            .join(name)
            .to_string_lossy()
            .replace('\\', "\\\\")
    };
    let old_extra = format!(
        ", args = --record, args = \"{}\", args = --child, args = \"{}\"",
        path("record"),
        path("child-port")
    );
    let h = harness(&profile(port, &old_extra)).await;
    let echo = echo_server().await;
    echo_via(h.http, echo).await;
    let grandchild: u16 = wait_for_file(&dir.path().join("child-port"))
        .await
        .trim()
        .parse()
        .unwrap();
    // held past the reload, as a session's hook would hold it
    let old = h.engine.registry().resolve_member("Ext").outbound;

    let new_extra = format!(
        ", args = --record, args = \"{}\", args = --served, args = \"{}\"",
        path("record"),
        path("served")
    );
    h.engine
        .swap_runtime(runtime(h.dir.path(), &profile(port, &new_extra), h.engine.shared()).await);
    echo_via(h.http, echo).await;
    // the old program's tree is gone
    wait_closed(grandchild).await;
    let recorded = std::fs::read_to_string(dir.path().join("record")).unwrap();
    let lines: Vec<&str> = recorded.lines().collect();
    assert_eq!(lines.len(), 2, "{recorded}");
    assert!(lines[1].contains("--served"), "{recorded}");
    let new_pid = lines[1]
        .split(' ')
        .next()
        .unwrap()
        .trim_start_matches("pid=");
    // the session went through the new program
    let served = wait_for_file(&dir.path().join("served")).await;
    assert_eq!(served.lines().collect::<Vec<_>>(), [new_pid], "{served}");

    let err = old
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .err()
        .unwrap();
    assert_eq!(
        err.to_string(),
        "external: a newer configuration of this policy is in use"
    );
    drop(old);
    h.engine.stop_external_programs().await;
    wait_closed(port).await;
}

/// The platform's lookup as far as the engine can tell: it answers the
/// process it is told to, at once, or after a delay as a slow system does.
struct Owners {
    process: Mutex<Option<ProcessInfo>>,
    delay: Duration,
}

impl Owners {
    fn new() -> Arc<Owners> {
        Arc::new(Owners {
            process: Mutex::new(None),
            delay: Duration::ZERO,
        })
    }

    /// A lookup that takes longer than the rules wait for it.
    fn slow() -> Arc<Owners> {
        Arc::new(Owners {
            process: Mutex::new(None),
            delay: Duration::from_millis(600),
        })
    }

    /// From now on the process behind a session is `pid`, at once.
    fn finds(&self, pid: u32) {
        *self.process.lock().unwrap() = Some(ProcessInfo {
            pid: Some(pid),
            name: "socks-helper".to_string(),
            path: None,
        });
    }
}

impl OwnerLookup for Owners {
    fn is_local(&self, _ip: std::net::IpAddr) -> Option<bool> {
        None
    }

    fn owner(&self, _query: &OwnerQuery) -> Option<ProcessInfo> {
        std::thread::sleep(self.delay);
        self.process.lock().unwrap().clone()
    }
}

fn shared_with(owners: Arc<Owners>) -> EngineShared {
    EngineShared {
        processes: Arc::new(Platform),
        owners: Some(owners),
        ..EngineShared::default()
    }
}

/// The process id `--record` wrote first.
fn pid_of(record: &str) -> u32 {
    record
        .lines()
        .next()
        .expect("a recorded start")
        .split(' ')
        .next()
        .expect("pid=")
        .trim_start_matches("pid=")
        .parse()
        .expect("a process id")
}

/// The id of the newest written record, or 0 when none is written yet.
fn newest(h: &Harness) -> u64 {
    h.engine.request_log().recent(1).pop().map_or(0, |r| r.id)
}

/// The record of the session that began after the one whose id is `after`:
/// waits for it at most 10 seconds, as a session is recorded once it ends.
async fn record_after(h: &Harness, after: u64) -> RequestRecord {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(record) = h.engine.request_log().recent(1).pop()
            && record.id > after
        {
            return record;
        }
        assert!(Instant::now() < deadline, "no record after {after}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A profile with the helper as `Ext` on `port`, `extra` arguments, and the
/// rule `FINAL,DIRECT`: `Ext` is there but nothing sends a session to it.
fn unused_program(port: u16, extra: &str) -> String {
    format!(
        "{GENERAL}[Proxy]\nExt = external, exec = \"{}\", args = --port, args = {port}{extra}, local-port = {port}\n\
[Rule]\nFINAL,DIRECT\n",
        helper().replace('\\', "\\\\")
    )
}

/// A session of an `external` program's process is DIRECT whatever the rules
/// say, and the record says why; a session of any other process follows them
/// (M4 design 11, M4-D21).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sessions_of_an_external_program_go_direct_whatever_the_rules() {
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let record_file = dir.path().join("record");
    let extra = format!(
        ", args = --record, args = \"{}\"",
        record_file.to_string_lossy().replace('\\', "\\\\")
    );
    let owners = Owners::new();
    let h = harness_in(
        &profile(port, &extra),
        shared_with(owners.clone()),
        OutboundMode::Rule,
    )
    .await;
    let echo = echo_server().await;

    // the program starts because of a session that `FINAL` sends to it
    echo_via(h.http, echo).await;
    let pid = pid_of(&wait_for_file(&record_file).await);
    // the first session's record, so the next one is told apart from it
    let before = record_after(&h, 0).await.id;

    // its own sessions are DIRECT, and the record says why
    owners.finds(pid);
    echo_via(h.http, echo).await;
    let record = record_after(&h, before).await;
    assert_eq!(record.policy, ["DIRECT"], "{record:?}");
    assert!(record.notes.contains(&EXTERNAL.to_string()), "{record:?}");

    // another process's follow the rules, which send them to `Ext`
    owners.finds(std::process::id());
    let before = newest(&h);
    echo_via(h.http, echo).await;
    let record = record_after(&h, before).await;
    assert_eq!(record.policy, ["Ext"], "{record:?}");
    assert!(!record.notes.contains(&EXTERNAL.to_string()), "{record:?}");

    h.engine.stop_external_programs().await;
    wait_closed(port).await;
}

/// The global mode sends every session to the `external` policy; a session of
/// the program's process is still DIRECT.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_the_global_mode_too() {
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let record_file = dir.path().join("record");
    let extra = format!(
        ", args = --record, args = \"{}\"",
        record_file.to_string_lossy().replace('\\', "\\\\")
    );
    let owners = Owners::new();
    let h = harness_in(
        &profile(port, &extra),
        shared_with(owners.clone()),
        OutboundMode::Proxy(PolicyRef::parse("Ext")),
    )
    .await;
    let echo = echo_server().await;

    echo_via(h.http, echo).await;
    let pid = pid_of(&wait_for_file(&record_file).await);
    // the first session's record, so the next one is told apart from it
    let before = record_after(&h, 0).await.id;
    owners.finds(pid);
    echo_via(h.http, echo).await;
    let record = record_after(&h, before).await;
    assert_eq!(record.policy, ["DIRECT"], "{record:?}");
    assert!(record.notes.contains(&EXTERNAL.to_string()), "{record:?}");

    h.engine.stop_external_programs().await;
    wait_closed(port).await;
}

/// What the program started is its own too: the session of a child process is
/// DIRECT (M4 design 11). On every platform: the child is recognised through
/// its process tree (a process group on Unix, a Job Object on Windows).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn what_the_program_started_goes_direct_too() {
    let port = free_port();
    let dir = tempfile::tempdir().unwrap();
    let path = |name: &str| {
        dir.path()
            .join(name)
            .to_string_lossy()
            .replace('\\', "\\\\")
    };
    let extra = format!(
        ", args = --child, args = \"{}\", args = --child-pid, args = \"{}\"",
        path("child-port"),
        path("child-pid")
    );
    let owners = Owners::new();
    let h = harness_in(
        &profile(port, &extra),
        shared_with(owners.clone()),
        OutboundMode::Rule,
    )
    .await;
    let echo = echo_server().await;

    echo_via(h.http, echo).await;
    let child: u32 = wait_for_file(&dir.path().join("child-pid"))
        .await
        .trim()
        .parse()
        .unwrap();
    // the first session's record, so the next one is told apart from it
    let before = record_after(&h, 0).await.id;
    owners.finds(child);
    echo_via(h.http, echo).await;
    let record = record_after(&h, before).await;
    assert_eq!(record.policy, ["DIRECT"], "{record:?}");
    assert!(record.notes.contains(&EXTERNAL.to_string()), "{record:?}");

    h.engine.stop_external_programs().await;
    wait_closed(port).await;
}

/// A program that is configured but not running is not waited for: the
/// session's process never arrives in time, no rule asks for it, and the
/// record is not marked down for a wait nobody needed (M4b plan P16).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_wait_without_a_running_program() {
    let port = free_port();
    let h = harness_in(
        &unused_program(port, ""),
        shared_with(Owners::slow()),
        OutboundMode::Rule,
    )
    .await;

    echo_via(h.http, echo_server().await).await;
    let record = record_after(&h, 0).await;
    assert_eq!(record.policy, ["DIRECT"], "{record:?}");
    assert!(
        !record.notes.contains(&NOT_IN_TIME.to_string()),
        "the session waited for a process nobody asked for: {record:?}"
    );
}
