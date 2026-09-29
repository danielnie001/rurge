//! `ExternalOutbound` with a real program (phase 2 M4 design 7.1–7.3): the
//! test helper, started and stopped through `rurge-platform::process`.

mod common;
use common::*;

/// The first use starts the program with its arguments in order and no
/// proxy settings; its output goes to the log, after a separator.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_use_starts_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let record = dir.path().join("record");
    let o = outbound(
        "Ext",
        &args(
            port,
            &["--record", &record.to_string_lossy(), "--delay-ms", "300"],
        ),
        port,
        dir.path(),
    );
    assert!(!record.exists(), "nothing starts before the first use");
    let echo = echo_server().await;
    round_trip(&o, echo).await.unwrap();
    // one program serves every later connection
    round_trip(&o, echo).await.unwrap();
    let recorded = std::fs::read_to_string(&record).unwrap();
    assert_eq!(recorded.lines().count(), 1, "{recorded}");
    assert!(
        recorded.contains(&format!(
            "args=--port {port} --record {} --delay-ms 300 ",
            record.to_string_lossy()
        )),
        "{recorded}"
    );
    assert!(
        recorded.contains(" HTTP_PROXY= ALL_PROXY= NO_PROXY=* no_proxy=*"),
        "{recorded}"
    );
    assert_eq!(o.log_path(), dir.path().join("Ext.log"));
    let log = std::fs::read_to_string(o.log_path()).unwrap();
    assert!(log.starts_with("--- rurge: starting the program"), "{log}");
    assert!(
        log.contains(&format!("socks-helper listening on 127.0.0.1:{port}")),
        "{log}"
    );
    o.stop().await;
    wait_closed(port).await;
}

/// A program that exited is started again on the next use — not sooner
/// than two seconds after the last start, which the request waits out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_program_that_exited_is_started_again() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let record = dir.path().join("record");
    let o = outbound(
        "Ext",
        &args(
            port,
            &["--record", &record.to_string_lossy(), "--serve", "1"],
        ),
        port,
        dir.path(),
    );
    let echo = echo_server().await;
    let first = Instant::now();
    round_trip(&o, echo).await.unwrap();
    wait_closed(port).await;
    round_trip(&o, echo).await.unwrap();
    assert!(first.elapsed() >= external_gap(), "{:?}", first.elapsed());
    let recorded = std::fs::read_to_string(&record).unwrap();
    let pids: Vec<&str> = recorded
        .lines()
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    assert_eq!(pids.len(), 2, "{recorded}");
    assert_ne!(pids[0], pids[1]);
    let log = std::fs::read_to_string(o.log_path()).unwrap();
    assert_eq!(
        log.matches("--- rurge: starting the program").count(),
        2,
        "{log}"
    );
    o.stop().await;
}

fn external_gap() -> Duration {
    rurge_proto::external::START_GAP
}

/// A program that never listens on its port: six attempts, half a second
/// apart, then the fixed text.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_port_that_never_opens_fails_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let elsewhere = free_port();
    let o = outbound("Ext", &args(elsewhere, &[]), port, dir.path());
    let started = Instant::now();
    let err = round_trip(&o, echo_server().await).await.unwrap_err();
    assert_eq!(
        err,
        "external: the local SOCKS5 port refused the connection"
    );
    let took = started.elapsed();
    assert!(
        took >= Duration::from_millis(2500) && took < Duration::from_secs(6),
        "{took:?}"
    );
    o.stop().await;
    wait_closed(elsewhere).await;
}

/// `udp-relay=true` (phase 2 M5): UDP goes through the program's own
/// SOCKS5 server, started for it when it does not run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn udp_goes_through_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let o = outbound("Ext", &args(port, &[]), port, dir.path());
    let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 256];
        while let Ok((n, from)) = echo.recv_from(&mut buf).await {
            let _ = echo.send_to(&buf[..n], from).await;
        }
    });
    let carrier = o.open_udp(&ConnectOpts::default()).await.unwrap();
    carrier.send_to(b"ping", &target(echo_addr)).await.unwrap();
    let mut buf = [0u8; 256];
    let (n, from) = tokio::time::timeout(Duration::from_secs(5), carrier.recv_from(&mut buf))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!((&buf[..n], from), (&b"ping"[..], target(echo_addr)));
    drop(carrier);
    o.stop().await;
    wait_closed(port).await;
}

/// Stopping ends the program and what it started (M4-D6): the helper's
/// own child keeps a port open until it is ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_ends_the_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let child_port = dir.path().join("child-port");
    let o = outbound(
        "Ext",
        &args(port, &["--child", &child_port.to_string_lossy()]),
        port,
        dir.path(),
    );
    round_trip(&o, echo_server().await).await.unwrap();
    let grandchild: u16 = wait_for_file(&child_port).await.trim().parse().unwrap();
    TcpStream::connect(("127.0.0.1", grandchild))
        .await
        .expect("the program's child runs");
    o.stop().await;
    wait_closed(port).await;
    wait_closed(grandchild).await;
    // stopping twice is fine
    o.stop().await;
}

/// A program that exits by itself takes what it started with it: a child
/// left behind would hold the port the next start needs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_program_that_exits_ends_its_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let child_port = dir.path().join("child-port");
    let o = outbound(
        "Ext",
        &args(
            port,
            &["--serve", "1", "--child", &child_port.to_string_lossy()],
        ),
        port,
        dir.path(),
    );
    round_trip(&o, echo_server().await).await.unwrap();
    let grandchild: u16 = wait_for_file(&child_port).await.trim().parse().unwrap();
    // one session served: the program exits, and its child goes with it
    wait_closed(port).await;
    wait_closed(grandchild).await;
    o.stop().await;
}

/// A released outbound stops its program (M4 design 7.4: a reload that
/// replaces the policy).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_the_outbound_stops_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let child_port = dir.path().join("child-port");
    let o = outbound(
        "Ext",
        &args(port, &["--child", &child_port.to_string_lossy()]),
        port,
        dir.path(),
    );
    round_trip(&o, echo_server().await).await.unwrap();
    let grandchild: u16 = wait_for_file(&child_port).await.trim().parse().unwrap();
    drop(o);
    wait_closed(port).await;
    wait_closed(grandchild).await;
}
