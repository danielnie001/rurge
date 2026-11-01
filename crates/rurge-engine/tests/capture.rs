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
use hyper::body::Bytes;
use rurge_engine::capture::{DEFAULT_SESSION, Outcome};
use rurge_proto::RejectKind;

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

/// One request over a fresh connection to rurge's HTTP listener, with the
/// extra header lines of `extra` before the blank line and everything written
/// in one go (`common`'s `plain_get`, which carries no header of its own).
async fn get_with(proxy: SocketAddr, url: &str, host: &str, extra: &str) -> String {
    send_with(
        proxy,
        &format!(
            "GET {url} HTTP/1.1\r\nHost: {host}\r\nProxy-Connection: keep-alive\r\nConnection: close\r\n{extra}\r\n"
        ),
    )
    .await
}

/// One `POST` of `body` over a fresh connection to rurge's HTTP listener, its
/// head and its body written together (the rule for the hand-written clients
/// of `docs/testing.md`), with the extra header lines of `extra`.
async fn post_with(proxy: SocketAddr, url: &str, host: &str, extra: &str, body: &str) -> String {
    send_with(
        proxy,
        &format!(
            "POST {url} HTTP/1.1\r\nHost: {host}\r\nProxy-Connection: keep-alive\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n{body}",
            body.len()
        ),
    )
    .await
}

/// `request` as it stands over a fresh connection to `proxy`, and everything
/// the answer had, bounded as every read of these cases is.
async fn send_with(proxy: SocketAddr, request: &str) -> String {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).into_owned()
}

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

// ---- the decrypted side ----
//
// The fixtures from here to `decrypted` are copies of `tests/rewrite.rs`'s
// (which copied them from `tests/mitm.rs`): this file is a crate of its own,
// and they stay where they are used rather than moving into `common`, whose
// other files go on using their own copies (`the phase 4 / M4a plan's P14`).

/// The `[MITM]` section of a profile: the hosts to decrypt, and the CA rurge
/// signs with as `ca-p12`, whose passphrase is `pw` (from `tests/rewrite.rs`'s
/// `mitm_section`; not moved into `common`, because the files using it keep
/// their own).
fn mitm_section(hostname: &str, ca_p12: &str) -> String {
    format!("[MITM]\nhostname = {hostname}\nca-p12 = {ca_p12}\nca-passphrase = pw")
}

/// A CA for rurge to sign with, and its trust anchors (from `tests/rewrite.rs`'s
/// `mitm_ca`; not moved into `common`, because the files using it keep their
/// own). Its subject is not the origin fixture's, so a client trusting only one
/// of the two tells them apart.
fn mitm_ca() -> Arc<TlsFixture> {
    TlsFixture::with_ca_name("rurge test MITM CA", &[])
}

/// A rustls client trusting `roots` and nothing else; `sni` is whether it sends
/// the server name extension (from `tests/rewrite.rs`'s `client_config`; not
/// moved into `common`, because the files using it keep their own).
fn client_config(
    roots: Arc<rustls::RootCertStore>,
    alpn: &[&[u8]],
    sni: bool,
) -> Arc<rustls::ClientConfig> {
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("the protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    config.enable_sni = sni;
    Arc::new(config)
}

/// The client side of a TLS handshake to `name` over any stream, bounded as
/// every TLS step of these cases is (from `tests/rewrite.rs`'s `handshake`; not
/// moved into `common`, because the files using it keep their own).
async fn handshake<S>(
    stream: S,
    config: Arc<rustls::ClientConfig>,
    name: &str,
) -> std::io::Result<tokio_rustls::client::TlsStream<S>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let name = rustls::pki_types::ServerName::try_from(name.to_string()).expect("a server name");
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio_rustls::TlsConnector::from(config).connect(name, stream),
    )
    .await
    .expect("the handshake ends within the bound")
}

/// The response head that starts at `s`, up to and including the blank line
/// (from `tests/rewrite.rs`'s `read_head`; not moved into `common`, because the
/// files using it keep their own).
async fn read_head<S>(s: &mut S) -> String
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut byte))
            .await
            .expect("the response head comes within the bound")
            .unwrap();
        assert!(
            n > 0,
            "closed before the response head: {:?}",
            String::from_utf8_lossy(&head)
        );
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// The whole response at `s`, its body read by its `Content-Length` (from
/// `tests/rewrite.rs`'s `read_response`; not moved into `common`, because the
/// files using it keep their own).
async fn read_response<S>(s: &mut S) -> String
where
    S: tokio::io::AsyncRead + Unpin,
{
    let head = read_head(s).await;
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut body))
        .await
        .expect("the response body comes within the bound")
        .unwrap();
    format!("{head}{}", String::from_utf8_lossy(&body))
}

/// One request over a decrypted connection, and the whole response (from
/// `tests/rewrite.rs`'s `decrypted_get`; not moved into `common`, because the
/// files using it keep their own).
async fn decrypted_get<S>(s: &mut S, host: &str, path: &str) -> String
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    read_response(s).await
}

/// An engine that trusts `roots` for the origin side and sniffs for 300 ms, as
/// the cases of `tests/mitm.rs` do (from `tests/rewrite.rs`'s `harness_mitm`;
/// not moved into `common`, because the files using it keep their own).
async fn harness_mitm(p: Profile<'_>, roots: Arc<rustls::RootCertStore>) -> Harness {
    harness_with(
        p,
        EngineShared {
            roots: Some(roots),
            sniff_wait: Duration::from_millis(300),
            ..EngineShared::default()
        },
    )
    .await
}

/// A decrypted connection to `name:port` through the SOCKS5 listener, and the
/// protocol the handshake settled on (from `tests/rewrite.rs`'s `decrypted`,
/// with the name and the port the caller gives; not moved into `common`,
/// because the files using it keep their own).
async fn decrypted(
    h: &Harness,
    roots: Arc<rustls::RootCertStore>,
    name: &str,
    port: u16,
    alpn: &[&[u8]],
) -> (Option<String>, tokio_rustls::client::TlsStream<TcpStream>) {
    let tunnel = connect_via_socks5(h.socks(), name, port).await;
    let tls = handshake(tunnel, client_config(roots, alpn, true), name)
        .await
        .expect("the client trusts the MITM CA");
    let settled = tls
        .get_ref()
        .1
        .alpn_protocol()
        .map(|p| String::from_utf8_lossy(p).into_owned());
    (settled, tls)
}

/// A hand-written upstream HTTP proxy: it records the whole head of every
/// request, answers a plain one with an empty 200, and closes on anything else
/// — a `CONNECT` gets no answer, so the TLS handshake that follows fails at
/// once rather than waiting out its bound. This is `tests/rewrite.rs`'s
/// `RecordingProxy` (not moved into `common`, because the files using it keep
/// their own), which keeps the request line only; this copy keeps the whole
/// head, because the case here is about a header the proxy is sent.
struct RecordingProxy {
    addr: SocketAddr,
    heads: Arc<std::sync::Mutex<Vec<String>>>,
}

impl RecordingProxy {
    async fn spawn() -> RecordingProxy {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let heads: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let kept = heads.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let kept = kept.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut byte))
                            .await
                        {
                            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return,
                            Ok(Ok(_)) => head.push(byte[0]),
                        }
                    }
                    let text = String::from_utf8_lossy(&head).into_owned();
                    let line = text.lines().next().unwrap_or_default().to_string();
                    kept.lock().unwrap().push(text);
                    if line.starts_with("GET ") {
                        let _ = stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            )
                            .await;
                    }
                });
            }
        });
        RecordingProxy { addr, heads }
    }

    /// The head of every request it was asked, in order.
    fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }

    /// The value `head` was sent with for `name`, if it carries one.
    fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}

/// A request a `REJECT` rule refuses is captured as well: nothing was dialled,
/// and the head the client was answered with is the entry's response (`phase 4
/// M4 design 5.3`: the capture rides on `Go` and the refusal comes at the
/// dial, `M3-D27`; the phase 4 / M4a plan's task 8).
#[tokio::test]
async fn a_request_a_reject_rule_refuses_is_captured() {
    let origin = TestServer::spawn().await;
    origin.set("/blocked", "never asked");
    let port = origin_addr(&origin).port();
    let h = harness(Profile {
        // with no page a refusal answers nothing, and there is no response
        // head to hold against the record: this case takes the generation's
        // page (`show-error-page-for-reject`, whose default is off)
        general: "show-error-page-for-reject = true",
        hosts: HOSTS,
        rules: "DOMAIN,example.test,REJECT",
        ..Profile::default()
    })
    .await;
    h.engine.set_capture(true).await;

    let url = format!("http://example.test:{port}/blocked");
    let answer = plain_get(h.http(), &url, &format!("example.test:{port}")).await;
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session(DEFAULT_SESSION).is_empty()
    })
    .await;

    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].status,
        RecordStatus::Rejected("REJECT".into()),
        "{records:?}"
    );
    assert_eq!(records[0].status_code, Some(403), "{records:?}");
    assert!(
        origin.requests().is_empty(),
        "a refusal dials nothing: {:?}",
        origin.requests()
    );

    let entries = h.engine.capture().session(DEFAULT_SESSION);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.id, records[0].id, "the entry and the record are one");
    assert_eq!(entry.outcome, Outcome::Rejected(RejectKind::Reject));
    assert!(entry.remote.is_none(), "no dial, so no remote address");
    assert_eq!(entry.request.method.as_str(), "GET");
    assert_eq!(entry.request.url, url);
    let response = entry
        .response
        .as_ref()
        .expect("the page a refusal is answered with is the response");
    assert_eq!(
        Some(u16::from(response.status)),
        records[0].status_code,
        "the entry's status is the record's"
    );
}

/// The password a client gives rurge's own HTTP listener is that hop's alone
/// (`phase 4 M1 design 9`), so it is not captured either: the request reaches
/// the origin, and the entry's request headers carry no `proxy-authorization`
/// (`phase 4 M4 design 6.6`, `M4-D15`; the phase 4 / M4a plan's task 8).
#[tokio::test]
async fn a_listener_password_is_not_captured() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi");
    let port = origin_addr(&origin).port();
    let h = harness(Profile {
        // the shared profile writes `http-listen = 127.0.0.1:0` first and the
        // last line wins: this one asks for rurge's own password
        general: "http-listen = old@127.0.0.1:0",
        hosts: HOSTS,
        rules: "DOMAIN,example.test,DIRECT",
        ..Profile::default()
    })
    .await;
    h.engine.set_capture(true).await;

    let url = format!("http://example.test:{port}/hello");
    let answer = get_with(
        h.http(),
        &url,
        &format!("example.test:{port}"),
        // `rurge:old`, as `tests/pipeline.rs`'s `CRED_OLD` has it: the
        // listener compares the password half only
        "Proxy-Authorization: Basic cnVyZ2U6b2xk\r\n",
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("hi"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session(DEFAULT_SESSION).is_empty()
    })
    .await;

    let entries = h.engine.capture().session(DEFAULT_SESSION);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert!(entry.response.is_some(), "the request was answered");
    assert!(
        entry.request.headers.get("proxy-authorization").is_none(),
        "the listener's password is not captured"
    );
    assert_eq!(entry.request.url, url);
}

/// What rurge sends an upstream HTTP proxy as its own credential is rurge's and
/// not the client's, so it is not captured either (`phase 4 M4 design 6.1`,
/// `6.6`, `M4-D15`): the proxy is sent the header, the entry's request headers
/// are not.
#[tokio::test]
async fn an_upstream_proxy_credential_is_not_captured() {
    let proxy = RecordingProxy::spawn().await;
    let proxy_port = proxy.addr.port();
    let origin = TestServer::spawn().await;
    origin.set("/hello", "at the origin, not here");
    let port = origin_addr(&origin).port();
    let h = harness(Profile {
        proxies: &format!("P = http, 127.0.0.1, {proxy_port}, user, pass"),
        hosts: HOSTS,
        rules: "DOMAIN,example.test,P",
        ..Profile::default()
    })
    .await;
    h.engine.set_capture(true).await;

    let url = format!("http://example.test:{port}/hello");
    let answer = plain_get(h.http(), &url, &format!("example.test:{port}")).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session(DEFAULT_SESSION).is_empty()
    })
    .await;

    let heads = proxy.heads();
    assert_eq!(heads.len(), 1, "the proxy was asked once");
    assert!(
        RecordingProxy::header(&heads[0], "proxy-authorization").is_some(),
        "rurge's own credential goes to the proxy: {}",
        heads[0].lines().next().unwrap_or_default()
    );
    assert!(
        origin.requests().is_empty(),
        "the proxy answered, so the origin was not reached"
    );

    let entries = h.engine.capture().session(DEFAULT_SESSION);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert!(
        entry.request.headers.get("proxy-authorization").is_none(),
        "the upstream proxy's credential is not captured"
    );
    assert_eq!(entry.request.url, url);
}

/// A request on a connection `[MITM]` decrypted is captured as the `https://`
/// request it was, with the origin's own body (`phase 4 M4 design 6.1`): the
/// engine answers the client on one side and speaks TLS to the origin on the
/// other, and the entry is of the request between them.
#[tokio::test]
async fn a_decrypted_request_is_captured_as_https() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "at the TLS origin");
    let port = origin_addr(&server).port();
    let mitm = mitm_ca();

    // the port written down in `[MITM]`'s hostname is the one sniffed and
    // decrypted (`phase 4 M2 design 7.6`)
    let sections = mitm_section(&format!("origin.test:{port}"), &mitm.ca_p12_base64("pw"));
    let h = harness_mitm(
        Profile {
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;
    h.engine.set_capture(true).await;

    let (settled, mut tls) = decrypted(&h, mitm.roots(), "origin.test", port, &[b"http/1.1"]).await;
    assert_eq!(settled.as_deref(), Some("http/1.1"), "{settled:?}");
    let answer = decrypted_get(&mut tls, &format!("origin.test:{port}"), "/a").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("at the TLS origin"), "{answer}");

    wait_until("the entry is stored", || {
        !h.engine.capture().session(DEFAULT_SESSION).is_empty()
    })
    .await;
    let entries = h.engine.capture().session(DEFAULT_SESSION);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.request.url, format!("https://origin.test:{port}/a"));
    assert!(
        entry.request.url.starts_with("https://"),
        "the decrypted request keeps its scheme: {}",
        entry.request.url
    );
    let response = entry.response.as_ref().expect("it was answered");
    assert_eq!(u16::from(response.status), 200);
    assert_eq!(
        entry.response_body.data,
        Bytes::from_static(b"at the TLS origin"),
        "the entry holds the origin's body"
    );
}

/// The store belongs to the engine and not to a generation (`phase 4 M4 design
/// 4.1`): a reload keeps what was captured and the switch as it was, and the
/// requests of the generation after it go into the same store.
#[tokio::test]
async fn entries_outlive_a_reload() {
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
    assert_eq!(h.engine.capture().status().stored, 1);

    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &text, h.engine.shared()).await);

    assert!(
        h.engine.capture_on(),
        "a reload leaves the switch as it was"
    );
    assert_eq!(
        h.engine.capture().session(DEFAULT_SESSION).len(),
        1,
        "the reloaded generation's store is the one that was there"
    );

    let answer = hello(&h, &origin).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the second entry is stored", || {
        h.engine.capture().status().stored == 2
    })
    .await;

    let status = h.engine.capture().status();
    assert_eq!(status.entries, 2, "{status:?}");
    assert_eq!(h.engine.capture().session(DEFAULT_SESSION).len(), 2);
}

/// What is captured goes into the store and nowhere else: no log line carries a
/// captured header value or a byte of a body, at the most detailed level this
/// test can hold (`phase 4 M4 design 6.6`, `M4-D15`, the phase 4 / M4a plan's
/// task 8). The failure says which line it was, and nothing of what was in it:
/// the line is what should not have been written at all.
#[tokio::test]
async fn captured_content_never_reaches_the_log() {
    let log = LogCapture::everything();
    let origin = TestServer::spawn().await;
    origin.set("/p", "done");
    let port = origin_addr(&origin).port();
    let h = harness(loopback(HOSTS)).await;
    h.engine.set_capture(true).await;

    let url = format!("http://example.test:{port}/p");
    let answer = post_with(
        h.http(),
        &url,
        &format!("example.test:{port}"),
        "x-marker: rurge-capture-marker-1\r\n",
        "rurge-capture-marker-2",
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    wait_until("the entry is stored", || {
        !h.engine.capture().session(DEFAULT_SESSION).is_empty()
    })
    .await;

    let entries = h.engine.capture().session(DEFAULT_SESSION);
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(
        entry
            .request
            .headers
            .get("x-marker")
            .and_then(|v| v.to_str().ok()),
        Some("rurge-capture-marker-1"),
        "the header is in the entry, which is where it is to be"
    );
    assert_eq!(
        entry.request_body.data,
        Bytes::from_static(b"rurge-capture-marker-2"),
        "the body is in the entry, which is where it is to be"
    );

    let lines = log.lines();
    let caught: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.contains("rurge-capture-marker"))
        .map(|(i, _)| i + 1)
        .collect();
    assert!(
        caught.is_empty(),
        "log line(s) {caught:?} of {} carry what was captured",
        lines.len()
    );
}
