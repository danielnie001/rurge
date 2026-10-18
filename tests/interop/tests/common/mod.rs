//! What the test files of this directory share: the harness, the helpers and
//! the imports (re-exported, so a test file needs `use common::*` and nothing
//! else). Each test file is a crate of its own and uses a part of all this
//! only: hence the two `allow`s.

#![allow(dead_code, unused_imports)]

pub use rurge_config::config::{LoadOptions, from_text};
pub use rurge_engine::EngineFactory;
pub use rurge_interop::{
    Inbound, InboundKind, SingBox, TlsFiles, WireGuardEndpoint, sing_box_or_skip,
};
pub use rurge_net::connector::{ConnectOpts, SystemResolve, Target};
pub use rurge_net::socket::NoopSocketHook;
pub use rurge_net::testing::TestServer;
pub use rurge_policy::OutboundFactory;
pub use rurge_proto::testing::{TlsFixture, echo_server, udp_echo_server};
pub use rurge_proto::{OutboundError, OutboundRef};
pub use rurge_proto_wireguard::testing::keypair;
pub use std::net::SocketAddr;
pub use std::path::Path;
pub use std::sync::Arc;
pub use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The outbound of policy `name` in `profile`, built the way the engine
/// builds it; `fixture`'s CA is trusted when given.
pub fn outbound(profile: &str, name: &str, fixture: Option<&Arc<TlsFixture>>) -> OutboundRef {
    let loaded = from_text(
        profile,
        Path::new("interop.conf"),
        &LoadOptions::for_tests(),
    );
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    let cfg = loaded.config;
    let factory = match fixture {
        Some(f) => EngineFactory::with_roots(
            &cfg,
            Arc::new(SystemResolve),
            Arc::new(NoopSocketHook),
            f.roots(),
        ),
        None => EngineFactory::new(&cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook)),
    };
    let spec = cfg.spec(name).expect("the policy has a spec");
    factory
        .build(spec, factory.direct_connector(&spec.common))
        .expect("the policy builds")
}

/// What a case writes on rurge's side, checked where no reference server
/// is: `profile` loads with nothing said about it but that a stream cipher
/// is one (`W0031`), and each of `names` builds its outbound. A misspelt
/// parameter is a warning only, and its line would dial something else than
/// the case means.
pub fn loads_and_builds(profile: &str, names: &[&str]) {
    let loaded = from_text(
        profile,
        Path::new("interop.conf"),
        &LoadOptions::for_tests(),
    );
    let said: Vec<String> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code != rurge_config::codes::W_STREAM_CIPHER)
        .map(|d| d.to_string())
        .collect();
    assert!(said.is_empty(), "{said:#?}\n{profile}");
    for name in names {
        assert_eq!(outbound(profile, name, None).name(), *name);
    }
}

pub fn target(addr: SocketAddr) -> Target {
    Target::new(rurge_config::HostName::Ip(addr.ip()), addr.port())
}

/// `t` with an IPv4-mapped IPv6 host read as the IPv4 address, as
/// `HostName::canonical` does: a server may name an IPv4 source either way.
pub fn canonical(t: Target) -> Target {
    Target::new(t.host.canonical(), t.port)
}

/// Every step is bounded: nobody runs these locally, and on CI an unbounded
/// read turns a regression into a hung job instead of a failed one.
pub async fn roundtrip(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(10);
    let mut stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel is established within the bound")
    .expect("the tunnel is established");
    tokio::time::timeout(bound, stream.write_all(b"interop"))
        .await
        .expect("the write reaches sing-box within the bound")
        .unwrap();
    let mut buf = [0u8; 7];
    tokio::time::timeout(bound, stream.read_exact(&mut buf))
        .await
        .expect("the echo comes back within the bound")
        .unwrap();
    assert_eq!(&buf, b"interop");
}

/// `len` bytes each way at once through one tunnel. Bounded like
/// `roundtrip`, by a minute.
pub async fn transfer(out: &OutboundRef, echo: SocketAddr, len: usize) {
    let bound = std::time::Duration::from_secs(60);
    let payload: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
    let stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel is established within the bound")
    .expect("the tunnel is established");
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; len];
    let both = async {
        tokio::join!(async { writer.write_all(&payload).await.unwrap() }, async {
            reader.read_exact(&mut back).await.unwrap()
        })
    };
    tokio::time::timeout(bound, both)
        .await
        .expect("the payload makes it there and back within the bound");
    assert!(back == payload, "the echo differs");
}

/// Two carriers of one policy at once, each a UDP session of its own on
/// the policy's connection, each with payloads of its own (an answer handed
/// to the other carrier would show): a few bytes, then 3000 — more than a
/// QUIC datagram holds, so a protocol that splits packets sends them in
/// fragments both ways (macOS sends no datagram past 9216 bytes from a
/// socket with the default send buffer). The source of an answer is
/// compared in canonical form. Bounded like `roundtrip`.
pub async fn udp_side_by_side(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(10);
    let mut carriers = Vec::new();
    for _ in 0..2 {
        let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
            .await
            .expect("the carrier opens within the bound")
            .expect("the carrier opens");
        carriers.push(carrier);
    }
    let small = |n: u8| vec![n; 7];
    let large = |n: u8| -> Vec<u8> { (0..3000u32).map(|i| (i as u8) ^ n).collect() };
    for payloads in [[small(1), small(2)], [large(1), large(2)]] {
        for (carrier, payload) in carriers.iter().zip(&payloads) {
            carrier.send_to(payload, &target(echo)).await.unwrap();
            let mut buf = vec![0u8; 65536];
            let (n, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
                .await
                .expect("the echo comes back within the bound")
                .unwrap();
            assert_eq!(canonical(from), canonical(target(echo)));
            assert!(buf[..n] == payload[..], "the echo differs ({n} bytes)");
        }
    }
}

/// One datagram to `echo` through a fresh UDP carrier of `out`, and its
/// answer, whose source is compared in canonical form. Bounded like
/// `roundtrip`.
pub async fn udp_roundtrip(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(10);
    let carrier = tokio::time::timeout(bound, out.open_udp(&ConnectOpts::default()))
        .await
        .expect("the carrier opens within the bound")
        .expect("the carrier opens");
    for payload in [&b"interop"[..], b"again"] {
        carrier.send_to(payload, &target(echo)).await.unwrap();
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(bound, carrier.recv_from(&mut buf))
            .await
            .expect("the echo comes back within the bound")
            .unwrap();
        assert_eq!(
            (&buf[..n], canonical(from)),
            (payload, canonical(target(echo)))
        );
    }
}

/// `count` requests one after another, each ended cleanly both ways, so
/// that an outbound that reuses connections hands each one's to the next.
/// Bounded like `roundtrip`.
pub async fn requests_one_after_another(out: &OutboundRef, echo: SocketAddr, count: usize) {
    let bound = std::time::Duration::from_secs(10);
    for i in 0..count {
        let mut stream = tokio::time::timeout(
            bound,
            out.connect_tcp(&target(echo), &ConnectOpts::default()),
        )
        .await
        .expect("the tunnel is established within the bound")
        .expect("the tunnel is established");
        let payload = format!("request {i}");
        let mut back = vec![0u8; payload.len()];
        let exchange = async {
            stream.write_all(payload.as_bytes()).await.unwrap();
            stream.read_exact(&mut back).await.unwrap();
            stream.shutdown().await.unwrap();
            // the target's end comes back as the server's end of the request
            let mut rest = Vec::new();
            stream.read_to_end(&mut rest).await.unwrap();
            rest
        };
        let rest = tokio::time::timeout(bound, exchange)
            .await
            .expect("the request completes within the bound");
        assert_eq!((back.as_slice(), rest.len()), (payload.as_bytes(), 0));
    }
}

/// What failed of the policies a test tried, each under its policy's name.
/// A test that stops at the first failure hides what its other policies
/// would have met, and on CI every run costs a push: so a test tries them
/// all and fails once, at its end. A `Failures` that holds failures and is
/// dropped without `finish` fails its test there and then, so that a test
/// cannot pass by leaving the call out.
#[must_use = "a `Failures` is there to be recorded into and `finish`ed; one dropped as soon as it is built checks nothing"]
#[derive(Default)]
pub struct Failures {
    tried: usize,
    failed: Vec<(String, String)>,
}

impl Failures {
    /// `what` went as `outcome` says.
    pub fn record(&mut self, what: &str, outcome: Result<(), String>) {
        self.tried += 1;
        if let Err(why) = outcome {
            self.failed.push((what.to_string(), why));
        }
    }

    /// How many failed, then each failure on a line of its own.
    fn summary(&self) -> String {
        let mut text = format!("{} of {} failed\n", self.failed.len(), self.tried);
        for (what, why) in &self.failed {
            // one line, whatever the layout of the panic's own text
            let why: Vec<&str> = why.split_whitespace().collect();
            text += &format!("{what}: {}\n", why.join(" "));
        }
        text
    }

    /// What `finish` panics with: the summary and `log` — what the reference
    /// server logged — last and once. The digest CI makes of a failed test
    /// shows the head of its output, 60 lines or 3 000 characters of it: the
    /// log is what may go.
    pub fn report(&self, log: &str) -> String {
        self.summary() + "--- the server's log ---\n" + log
    }

    /// Fails the test when anything failed. `log` is read only then: on CI
    /// nobody can ask the server afterwards.
    #[track_caller]
    pub fn finish(mut self, log: impl FnOnce() -> String) {
        if !self.failed.is_empty() {
            let report = self.report(&log());
            // the panic below tells them: dropping `self` on the way out has
            // nothing left to guard
            self.failed.clear();
            panic!("{report}");
        }
    }
}

impl Drop for Failures {
    fn drop(&mut self) {
        // A test that is failing for another reason says that: a panic here,
        // in the middle of its unwind, would abort the process.
        if !self.failed.is_empty() && !std::thread::panicking() {
            panic!(
                "failures were recorded and never finished, so the test would have passed:\n{}",
                self.summary()
            );
        }
    }
}

/// What a panic said, when it said it in words.
pub fn panic_text(panic: Box<dyn std::any::Any + Send>) -> String {
    match panic.downcast::<String>() {
        Ok(text) => *text,
        Err(panic) => match panic.downcast::<&'static str>() {
            Ok(text) => text.to_string(),
            Err(_) => "a panic without a message".to_string(),
        },
    }
}

/// `steps` as a task of its own, so that its failure is not the test's last
/// word: the panic says what happened, and `failures` keeps it under `what`,
/// the policy it happened to, for the test to go on to the next one.
pub async fn attempt<F>(failures: &mut Failures, what: &str, steps: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let outcome = tokio::spawn(steps)
        .await
        .map_err(|failed| match failed.try_into_panic() {
            Ok(panic) => panic_text(panic),
            Err(cancelled) => cancelled.to_string(),
        });
    failures.record(what, outcome);
}

/// What a `snell` session says when the server ended the connection without
/// a byte of its own: where that arrives as an end, and where it arrives as
/// a reset.
pub const UNANSWERED: [&str; 2] = [
    "snell: the server closed the connection without answering (wrong psk or version?)",
    "snell: the server reset the connection without answering (wrong psk or version?)",
];

/// How long a request no server can read may take to fail.
pub const UNANSWERED_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

/// What reading says after a request the server cannot read: `Ok` with the
/// error's text. The tunnel comes up, since nothing is asked before the
/// first payload; the payload leaves; and then the failure must come within
/// `bound`. Whatever goes otherwise is the `Err`, in words.
pub async fn unanswered(
    out: &OutboundRef,
    echo: SocketAddr,
    bound: std::time::Duration,
) -> Result<String, String> {
    // bounded like `roundtrip`: neither step waits for the server
    let step = std::time::Duration::from_secs(10);
    let (to, opts) = (target(echo), ConnectOpts::default());
    let mut stream = match tokio::time::timeout(step, out.connect_tcp(&to, &opts)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => return Err(format!("the tunnel did not come up: {e}")),
        Err(_) => return Err(format!("no tunnel within {step:?}")),
    };
    match tokio::time::timeout(step, stream.write_all(b"interop")).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(format!("the first payload did not leave: {e}")),
        Err(_) => return Err(format!("the first payload did not leave within {step:?}")),
    }
    let mut answer = Vec::new();
    match tokio::time::timeout(bound, stream.read_to_end(&mut answer)).await {
        Ok(Err(e)) => Ok(e.to_string()),
        Ok(Ok(_)) => Err(format!(
            "the request ended without an error, {} bytes in",
            answer.len()
        )),
        Err(_) => Err(format!(
            "no failure within {bound:?}, {} bytes in: the request hangs",
            answer.len()
        )),
    }
}

/// Each policy of `names` dials a server that cannot read its request — one
/// of another PSK, or of another version's records. Such a server has no
/// word to answer with: the session must fail within `bound`, with one of
/// the two sentences that say what may be wrong and nothing of either PSK.
/// A third sentence has the same hint — the server's data failed to decrypt
/// — and would mean that the server had answered: it is not taken. Whatever
/// a policy meets instead is its failure, told as what it was.
pub async fn none_is_answered(
    failures: &mut Failures,
    profile: &str,
    names: &[&str],
    bound: std::time::Duration,
) {
    let echo = echo_server().await;
    for name in names {
        let out = outbound(profile, name, None);
        let outcome = match unanswered(&out, echo, bound).await {
            Ok(text) if UNANSWERED.contains(&text.as_str()) => Ok(()),
            Ok(text) => Err(format!("failed with another sentence: {text}")),
            Err(otherwise) => Err(otherwise),
        };
        failures.record(name, outcome);
    }
}

pub fn plain(kind: InboundKind, users: &[(&str, &str)]) -> Inbound {
    Inbound {
        kind,
        users: users
            .iter()
            .map(|(u, p)| (u.to_string(), p.to_string()))
            .collect(),
        tls: None,
        ws_path: None,
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The fixture's leaf certificate and key as PEM files in `dir`.
pub fn leaf_files(fixture: &TlsFixture, dir: &Path) -> TlsFiles {
    let write = |name: &str, text: String| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    TlsFiles {
        certificate: write("leaf.pem", fixture.leaf_pem()),
        key: write("leaf.key", fixture.leaf_key_pem()),
        client_ca: None,
    }
}

pub fn trojan_inbound(tls: TlsFiles, ws_path: Option<&str>) -> Inbound {
    Inbound {
        kind: InboundKind::Trojan,
        users: vec![("u".into(), "s3same".into())],
        tls: Some(tls),
        ws_path: ws_path.map(str::to_string),
    }
}

pub const VMESS_ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

pub fn vmess_inbound(tls: Option<TlsFiles>, ws_path: Option<&str>) -> Inbound {
    Inbound {
        kind: InboundKind::Vmess,
        users: vec![("u".into(), VMESS_ID.into())],
        tls,
        ws_path: ws_path.map(str::to_string),
    }
}

/// More than one chunk each way: the length masks and the nonce counter have
/// to stay in step. Bounded like `roundtrip`.
pub async fn roundtrip_big(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(20);
    let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel is established within the bound")
    .expect("the tunnel is established");
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; payload.len()];
    let both = async {
        tokio::join!(async { writer.write_all(&payload).await.unwrap() }, async {
            reader.read_exact(&mut back).await.unwrap()
        })
    };
    tokio::time::timeout(bound, both)
        .await
        .expect("100 000 bytes make it there and back within the bound");
    assert!(back == payload, "the echo differs");
}
