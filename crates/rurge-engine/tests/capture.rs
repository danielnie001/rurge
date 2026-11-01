//! Capture end to end (phase 4 M4 design 12): the engine's store, the switch
//! and what a rule's `always-capture` sends an entry to.
//!
//! A file of its own rather than more of `tests/http.rs` or `tests/mitm.rs`:
//! those are long already (the phase 4 / M4a plan's P14).
//!
//! Nothing here goes beyond the loopback: the origins are plain HTTP test
//! servers on loopback ports, `example.test` is pinned to `127.0.0.1` by the
//! profile, and the rules of these tests send every request to the origin's
//! port.

mod common;
use common::*;
use rurge_engine::capture::DEFAULT_SESSION;

/// A request through rurge's HTTP listener to `origin`'s `/hello`; the
/// response is returned whole.
async fn hello(h: &Harness, origin: &TestServer) -> String {
    let port = origin_addr(origin).port();
    plain_get(
        h.http(),
        &format!("http://example.test:{port}/hello"),
        &format!("example.test:{port}"),
    )
    .await
}

/// A profile whose `[Host]` pins the name these tests ask to loopback.
fn loopback(hosts: &'static str) -> Profile<'static> {
    Profile {
        hosts,
        ..Profile::default()
    }
}

const HOSTS: &str = "example.test = 127.0.0.1";

/// An entry is stored once its record has ended and its answer's body has
/// been handed over, which over the loopback takes microseconds: this is the
/// margin of an assertion that nothing was stored at all (the phase 4 / M4a
/// plan's P14).
const QUIET: Duration = Duration::from_millis(200);

#[tokio::test]
async fn nothing_is_captured_while_the_switch_is_off() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let h = harness(loopback(HOSTS)).await;

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let records = the_records(&h, 1).await;
    tokio::time::sleep(QUIET).await;

    assert!(!h.engine.capture_on(), "the switch is off unless set");
    let status = h.engine.capture().status();
    assert_eq!(status.stored, 0, "{status:?}");
    assert_eq!(records.len(), 1, "{records:?}");
}

#[tokio::test]
async fn the_switch_captures_into_default() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let h = harness(loopback(HOSTS)).await;
    h.engine.set_capture(true).await;

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session(DEFAULT_SESSION).is_empty()
    })
    .await;

    let sessions = h.engine.capture().sessions();
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(&*sessions[0].name, DEFAULT_SESSION, "{sessions:?}");
    assert_eq!(sessions[0].entries, 1, "{sessions:?}");

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    let entries = h.engine.capture().session(DEFAULT_SESSION);
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].id, records[0].id,
        "the entry and the record are the same request"
    );
}

#[tokio::test]
async fn always_capture_captures_with_the_switch_off() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let h = harness(Profile {
        hosts: HOSTS,
        rules: "DOMAIN,example.test,DIRECT,always-capture=debug-1",
        ..Profile::default()
    })
    .await;

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session("debug-1").is_empty()
    })
    .await;

    assert!(!h.engine.capture_on(), "the switch is off unless set");
    assert_eq!(h.engine.capture().session("debug-1").len(), 1);
    assert!(
        h.engine.capture().session(DEFAULT_SESSION).is_empty(),
        "a named session takes the entry instead of the default one"
    );
}

/// The profile the harness builds always appends `FINAL,DIRECT` after the
/// rules a test writes, and the last `FINAL` is the one in effect
/// (`RuleEngine::build`): so this test writes no rule of its own and sets
/// `always-capture` on that appended line instead.
#[tokio::test]
async fn always_capture_on_final_captures_too() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let h = harness_edited(loopback(HOSTS), EngineShared::default(), |cfg| {
        let last = cfg.rules.last_mut().expect("the profile ends with FINAL");
        last.params.always_capture = Some("last".into());
    })
    .await;

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session("last").is_empty()
    })
    .await;

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].rule.as_deref(), Some("FINAL,DIRECT"));
    assert_eq!(h.engine.capture().session("last").len(), 1);
}

#[tokio::test]
async fn a_logical_rule_names_the_session_of_its_line() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let port = origin_addr(&origin).port();
    let rules =
        format!("AND,((DOMAIN,example.test),(DEST-PORT,{port})),DIRECT,always-capture=logic");
    let h = harness(Profile {
        hosts: HOSTS,
        rules: &rules,
        ..Profile::default()
    })
    .await;

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session("logic").is_empty()
    })
    .await;

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0].rule.as_deref(), Some(rules.as_str()));
    assert_eq!(h.engine.capture().session("logic").len(), 1);
}

#[tokio::test]
async fn the_session_a_rule_names_comes_before_default() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let h = harness(Profile {
        hosts: HOSTS,
        rules: "DOMAIN,example.test,DIRECT,always-capture=named",
        ..Profile::default()
    })
    .await;
    h.engine.set_capture(true).await;

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session("named").is_empty()
    })
    .await;

    assert_eq!(h.engine.capture().session("named").len(), 1);
    assert!(
        h.engine.capture().session(DEFAULT_SESSION).is_empty(),
        "the name the rule gives wins over the default session"
    );
}

#[tokio::test]
async fn the_direct_mode_reads_no_rule_and_so_no_always_capture() {
    use rurge_engine::control::Mode;

    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let h = harness(Profile {
        hosts: HOSTS,
        rules: "DOMAIN,example.test,DIRECT,always-capture=named",
        ..Profile::default()
    })
    .await;
    h.engine.set_mode(Mode::Direct).await;

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    tokio::time::sleep(QUIET).await;
    assert_eq!(
        h.engine.capture().status().stored,
        0,
        "no rule was read, so no `always-capture` was either"
    );

    h.engine.set_capture(true).await;
    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session(DEFAULT_SESSION).is_empty()
    })
    .await;

    assert_eq!(h.engine.capture().session(DEFAULT_SESSION).len(), 1);
    assert!(
        h.engine.capture().session("named").is_empty(),
        "the rule was not read, so its session has nothing"
    );
}

#[tokio::test]
async fn turning_the_switch_off_keeps_what_was_captured() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let h = harness(loopback(HOSTS)).await;
    h.engine.set_capture(true).await;

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session(DEFAULT_SESSION).is_empty()
    })
    .await;

    h.engine.set_capture(false).await;
    assert_eq!(h.engine.capture().status().entries, 1, "it stays");

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    tokio::time::sleep(QUIET).await;

    let status = h.engine.capture().status();
    assert_eq!(status.stored, 1, "the second request is not captured");
    assert_eq!(status.entries, 1, "{status:?}");
}

/// The switch is kept in `state.json` and a start that reads the file puts it
/// back, without writing the file itself (the shape of `tests/mitm.rs`'s
/// `the_switch_is_restored_from_state_json`; phase 4 M4 design 5.1).
#[tokio::test]
async fn the_capture_switch_is_kept_in_state_json_and_restored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(STATE_FILE);

    let (store, _) = StateStore::open(path.clone()).await;
    let h = harness(Profile::default()).await;
    h.engine.attach_state(store);
    h.engine.set_capture(true).await;

    let text = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["version"], serde_json::json!(2), "{text}");
    assert_eq!(
        json["features"]["capture"],
        serde_json::json!(true),
        "{text}"
    );

    let (restored, state) = StateStore::open(path.clone()).await;
    assert_eq!(state.features.capture, Some(true));
    let other = harness(Profile::default()).await;
    other.engine.attach_state(restored);
    assert!(!other.engine.capture_on(), "the switch is off unless set");
    let before = std::fs::read(&path).unwrap();
    other.engine.restore_switches(&state.features);
    assert!(other.engine.capture_on(), "the file's word is taken");
    let after = std::fs::read(&path).unwrap();
    assert_eq!(before, after, "a start does not write state.json");

    h.engine.set_capture(false).await;
    let text = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        json["features"]["capture"],
        serde_json::json!(false),
        "{text}"
    );

    // the third engine switches it on with no file attached, so the memory
    // says on and the file says off; the file's word is what a start takes
    let (restored, state) = StateStore::open(path.clone()).await;
    assert_eq!(state.features.capture, Some(false));
    let third = harness(Profile::default()).await;
    third.engine.set_capture(true).await;
    assert!(third.engine.capture_on());
    third.engine.attach_state(restored);
    third.engine.restore_switches(&state.features);
    assert!(!third.engine.capture_on(), "the file's word is taken");
}
