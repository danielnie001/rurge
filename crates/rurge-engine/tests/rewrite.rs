//! The rewrites one generation carries (`phase 4 M3 design 8.4, 11, 12`; the
//! phase 4 / M3a plan's task 6): the `rewrite` switch of one engine, a request
//! whose policy is one of REJECT's, the gateway a `302` carries, and the counts
//! that outlive a reload.
//!
//! Nothing here goes beyond the loopback: every origin and guard is a loopback
//! server, and every name a rule names is pinned to `127.0.0.1` by `[Host]`.

mod common;
use common::*;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::client::conn::http2;
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rurge_config::{NetKind, NetworkFacts, Primary};
use rurge_engine::ExternalResource;

/// The engine's trust and the network it is on. Every engine here brings its
/// own CA: a generation with any rewrite rule reads the roots of the origin
/// side even when no request of the case uses TLS (`the phase 4 / M3a plan's
/// execution record row 20`). The gateways only ever come from here (`the
/// phase 4 / M3a plan's P19`).
fn shared_on(network: NetworkFacts) -> EngineShared {
    let shared = EngineShared {
        roots: Some(TlsFixture::new(&["localhost"]).roots()),
        ..EngineShared::default()
    };
    shared.network.store(network);
    shared
}

/// A machine whose primary interface has an IPv6 router before the IPv4 one:
/// what a `302` carries is the first IPv4 router, not the first router.
fn with_a_gateway() -> NetworkFacts {
    NetworkFacts {
        primary: Some(Primary {
            interface: "test0".to_string(),
            kind: NetKind::Wired,
            routers: vec!["fe80::1".parse().unwrap(), "127.0.0.2".parse().unwrap()],
        }),
        wifi: None,
    }
}

/// The value of the first `name` line of a recorded response head; the field
/// names are compared without case, as HTTP does.
fn header_value(head: &str, name: &str) -> Option<String> {
    head.lines().skip(1).find_map(|line| {
        line.split_once(':')
            .filter(|(field, _)| field.trim().eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim().to_string())
    })
}

/// The `rewrite` switch (`phase 4 M3 design 11`): with it off neither a
/// header rule nor a URL rule of this generation acts, and neither leaves a
/// note or a count; with it on again, both do.
#[tokio::test]
async fn the_rewrite_switch_turns_every_rewrite_off() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/x", "second");
    let port = origin_addr(&origin).port();
    let h = harness_with(
        Profile {
            hosts: "target.test = 127.0.0.1\nr.test = 127.0.0.1\ns.test = 127.0.0.1",
            rules: "DOMAIN,target.test,DIRECT\nDOMAIN,r.test,DIRECT",
            sections: "[URL Rewrite]\n\
                       ^http://r\\.test:\\d+/x https://s.test/ 302\n\
                       [Header Rewrite]\n\
                       ^http://target\\.test header-add X-A 1",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let a = format!("http://target.test:{port}/a");
    let x = format!("http://r.test:{port}/x");

    // on: the header rule adds the field, the URL rule answers the redirect
    let answer = plain_get(h.http(), &a, "target.test").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let answer = plain_get(h.http(), &x, "r.test").await;
    assert!(answer.starts_with("HTTP/1.1 302"), "{answer}");
    assert_eq!(
        header_value(&answer, "location").as_deref(),
        Some("https://s.test/"),
        "{answer}"
    );
    assert_eq!(origin.hits("/x"), 0, "a redirect is not dialled");
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.url, counts.header), (1, 1), "{counts:?}");

    // off: neither acts
    h.engine.set_rewrite(false).await;
    assert!(!h.engine.rewrite_on());
    let answer = plain_get(h.http(), &a, "target.test").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let answer = plain_get(h.http(), &x, "r.test").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(origin.hits("/x"), 1, "the request went to the origin");
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (counts.url, counts.header),
        (1, 1),
        "a switch off changes no count: {counts:?}"
    );

    // on again: both act again
    h.engine.set_rewrite(true).await;
    let answer = plain_get(h.http(), &a, "target.test").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let answer = plain_get(h.http(), &x, "r.test").await;
    assert!(answer.starts_with("HTTP/1.1 302"), "{answer}");
    assert_eq!(origin.hits("/x"), 1);
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.url, counts.header), (2, 2), "{counts:?}");

    // the two requests that came with the switch off saw no rule at all
    let seen = origin.requests();
    let a_requests: Vec<_> = seen.iter().filter(|r| r.path == "/a").collect();
    assert_eq!(a_requests.len(), 3, "{seen:?}");
    assert_eq!(a_requests[0].header("x-a"), Some("1"));
    assert_eq!(a_requests[1].header("x-a"), None);
    assert_eq!(a_requests[2].header("x-a"), Some("1"));
    let records = the_records(&h, 6).await;
    assert_eq!(records.len(), 6, "{records:?}");
    assert!(
        records[0].notes.contains(
            &"Header Rewrite: request header-add X-A by ^http://target\\.test".to_string()
        ),
        "{:?}",
        records[0].notes
    );
    assert!(
        records[1]
            .notes
            .contains(&"URL Rewrite: 302 by ^http://r\\.test:\\d+/x".to_string()),
        "{:?}",
        records[1].notes
    );
    // with the switch off, neither rule left a note on its request
    assert!(records[2].notes.is_empty(), "{:?}", records[2].notes);
    assert!(records[3].notes.is_empty(), "{:?}", records[3].notes);
}

/// A request whose policy is one of REJECT's is not rewritten at all (`phase 4
/// M3 design 8.4`, `M3-D27`): no redirect answers it, no field of it changes,
/// and nothing is counted.
#[tokio::test]
async fn a_rejected_request_is_not_rewritten() {
    let h = harness_with(
        Profile {
            general: "show-error-page-for-reject = true",
            hosts: "r.test = 127.0.0.1\ns.test = 127.0.0.1",
            rules: "DOMAIN,r.test,REJECT",
            sections: "[URL Rewrite]\n\
                       ^http://r\\.test https://s.test/ 302\n\
                       [Header Rewrite]\n\
                       ^http://r\\.test header-add X-A 1",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    let answer = plain_get(h.http(), "http://r.test/x", "r.test").await;
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].status,
        RecordStatus::Rejected("REJECT".into()),
        "{records:?}"
    );
    assert!(records[0].notes.is_empty(), "{:?}", records[0].notes);
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (
            counts.url,
            counts.header,
            counts.body,
            counts.map_local,
            counts.skipped,
            counts.failed
        ),
        (0, 0, 0, 0, 0, 0),
        "{counts:?}"
    );
}

/// A `302` carries the first IPv4 gateway of the machine (`phase 4 M3 design
/// 6.4`, `M3-D10`); on a machine that has none the rule is skipped, with a
/// note, and the request goes where it was going.
#[tokio::test]
async fn a_redirect_carries_the_first_ipv4_gateway() {
    let origin = TestServer::spawn().await;
    origin.set("/x", "reached");
    let port = origin_addr(&origin).port();
    let profile = || Profile {
        hosts: "g.test = 127.0.0.1",
        rules: "DOMAIN,g.test,DIRECT",
        sections: "[URL Rewrite]\n^http://g\\.test:\\d+/ http://{{{GATEWAY_ADDRESS}}}/ 302",
        ..Profile::default()
    };
    let url = format!("http://g.test:{port}/x");

    // the IPv6 router comes first in `routers`; the redirect carries the IPv4
    let h = harness_with(profile(), shared_on(with_a_gateway())).await;
    let answer = plain_get(h.http(), &url, "g.test").await;
    assert!(answer.starts_with("HTTP/1.1 302"), "{answer}");
    assert_eq!(
        header_value(&answer, "location").as_deref(),
        Some("http://127.0.0.2/x"),
        "{answer}"
    );
    assert_eq!(origin.connections(), 0, "a redirect is not dialled");
    let records = the_records(&h, 1).await;
    assert_eq!(records[0].status_code, Some(302), "{records:?}");
    drop(h);

    // no primary interface: no gateway, so the rule is skipped and the request
    // goes to the origin it was for
    let h = harness_with(profile(), shared_on(NetworkFacts::default())).await;
    let answer = plain_get(h.http(), &url, "g.test").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(origin.connections(), 1, "the request reached the origin");
    let records = the_records(&h, 1).await;
    assert_eq!(
        records[0].notes,
        vec!["URL Rewrite skipped: ^http://g\\.test:\\d+/: there is no IPv4 gateway".to_string()],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (counts.url, counts.skipped, counts.failed),
        (0, 1, 0),
        "{counts:?}"
    );
}

/// The counters live in the engine and not in a generation (`phase 4 M3 design
/// 12`): a reload keeps them, and the new generation's rules go on counting
/// into the same ones.
#[tokio::test]
async fn counts_survive_a_reload() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    origin.set("/b", "second");
    let port = origin_addr(&origin).port();
    let h = harness_with(
        Profile {
            hosts: "target.test = 127.0.0.1",
            rules: "DOMAIN,target.test,DIRECT",
            sections: "[Header Rewrite]\n^http://target\\.test header-add X-A 1",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    let answer = plain_get(
        h.http(),
        &format!("http://target.test:{port}/a"),
        "target.test",
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(origin.requests()[0].header("x-a"), Some("1"));
    assert_eq!(h.engine.http_counts().rewrite.header, 1);

    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    h.engine
        .swap_runtime(runtime(h.dir.path(), &text, h.engine.shared()).await);
    assert_eq!(
        h.engine.http_counts().rewrite.header,
        1,
        "a reload keeps the counts"
    );

    let answer = plain_get(
        h.http(),
        &format!("http://target.test:{port}/b"),
        "target.test",
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(origin.requests()[1].header("x-a"), Some("1"));
    assert_eq!(h.engine.http_counts().rewrite.header, 2);
}

/// A listener a profile can name as an HTTP proxy and that nothing is to dial:
/// the cases here whose rewrite answers the request by itself put every rule
/// that would reach an upstream on this one, and fail when it was reached
/// (`the phase 4 / M3a plan's P19`).
struct Guard {
    listener: std::net::TcpListener,
    port: u16,
}

impl Guard {
    fn new() -> Guard {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a guard port");
        listener
            .set_nonblocking(true)
            .expect("a guard that does not block");
        let port = listener.local_addr().expect("a guard address").port();
        Guard { listener, port }
    }

    /// The policy line a profile names to send a request here.
    fn policy(&self, name: &str) -> String {
        format!("{name} = http, 127.0.0.1, {}", self.port)
    }

    /// How many connections reached the guard: none, for a request the case
    /// says never reaches an upstream.
    fn connections(&self) -> usize {
        let mut seen = 0;
        loop {
            match self.listener.accept() {
                Ok(_) => seen += 1,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return seen,
                // a connection that was reset after it was taken still reached
                // the guard
                Err(_) => return seen + 1,
            }
        }
    }
}

/// One request written whole — its head and its body in one write
/// (`CLAUDE.md`) — and the answer read to the end of the connection.
async fn exchange(proxy: SocketAddr, request: &str) -> String {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    exchange_over(&mut s, request).await
}

/// `exchange`, over a connection that is already open: the request is one
/// write, and the answer is read until the far end closes it.
async fn exchange_over<S>(s: &mut S, request: &str) -> String
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    s.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).into_owned()
}

/// A loopback origin that answers every request with one fixed body and keeps
/// each request's head: the manual's header example needs the request line
/// *with its query string*, which `TestServer` does not record (it keeps
/// `uri().path()`). `the phase 4 / M3a plan's execution record row 64`.
struct RecordingOrigin {
    addr: SocketAddr,
    heads: Arc<std::sync::Mutex<Vec<String>>>,
}

impl RecordingOrigin {
    async fn spawn(body: &'static str) -> RecordingOrigin {
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
                    kept.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&head).into_owned());
                    let answer = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(answer.as_bytes()).await;
                });
            }
        });
        RecordingOrigin { addr, heads }
    }

    /// The head of every request it was asked, in order.
    fn heads(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }
}

/// The manual's `[URL Rewrite]` page, its first example, word for word
/// (`http/url-rewrite.html`; `the phase 4 / M3a plan's P19`): the request
/// carries the test origin's port, which the manual's line leaves to the text
/// after the match, and the two names are pinned to the loopback by `[Host]`.
///
/// The origin is a hand-written one because what this case has to show is the
/// request line *with its query string*: `TestServer` is `rurge-net`'s and
/// records the path alone, so `RecordingOrigin` above is written here for it
/// (`the phase 4 / M3a plan's execution record row 64`).
#[tokio::test]
async fn the_manual_url_rewrite_header_example() {
    let origin = RecordingOrigin::spawn("rewritten").await;
    let port = origin.addr.port();
    let general = format!("force-http-engine-hosts = www.google.cn:{port}");
    let h = harness_with(
        Profile {
            general: &general,
            hosts: "www.google.cn = 127.0.0.1\nwww.google.com = 127.0.0.1",
            rules: "DOMAIN,www.google.cn,DIRECT",
            sections: "[URL Rewrite]\n^http://www\\.google\\.cn http://www.google.com header",
            ..Profile::default()
        },
        EngineShared {
            sniff_wait: Duration::from_millis(300),
            ..shared_on(NetworkFacts::default())
        },
    )
    .await;
    let url = format!("http://www.google.cn:{port}/s?q=1");
    let host = format!("www.google.cn:{port}");

    // through the HTTP listener, whose request line is absolute
    let answer = plain_get(h.http(), &url, &host).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("rewritten"), "{answer}");

    // through SOCKS5, on a sniffed connection: the request line names a path
    let mut tunnel = connect_via_socks5(h.socks(), "www.google.cn", port).await;
    let answer = get(&mut tunnel, &host, "/s?q=1").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("rewritten"), "{answer}");

    let heads = origin.heads();
    assert_eq!(heads.len(), 2, "{heads:?}");
    for head in &heads {
        assert!(
            head.starts_with("GET /s?q=1 HTTP/1.1"),
            "the path and its query reached the origin: {head}"
        );
        assert!(
            head.contains(&format!("\r\nhost: www.google.com:{port}\r\n"))
                || head.contains(&format!("\r\nHost: www.google.com:{port}\r\n")),
            "the new authority names the origin: {head}"
        );
    }
    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    let note = format!(
        "URL Rewrite: header by ^http://www\\.google\\.cn -> http://www.google.com:{port}/s"
    );
    for record in &records {
        assert_eq!(record.url.as_deref(), Some(url.as_str()), "{record:?}");
        assert_eq!(record.notes, [note.as_str()], "{record:?}");
        assert_eq!(
            record.rule.as_deref(),
            Some("DOMAIN,www.google.cn,DIRECT"),
            "{record:?}"
        );
        assert_eq!(record.policy, ["DIRECT"], "{record:?}");
    }
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.url, counts.header), (2, 0), "{counts:?}");
}

/// The manual's `302` and `307` examples (`http/url-rewrite.html`), one engine
/// each: the answer is the status, the `Location` and an empty body, and the
/// policy the rule chose — a guard here — is never dialled.
#[tokio::test]
async fn the_manual_url_rewrite_302_and_307_examples() {
    for (mode, status) in [("302", "302"), ("307", "307")] {
        let guard = Guard::new();
        let proxies = guard.policy("guard");
        let sections = format!("[URL Rewrite]\n^http://yachen\\.com https://yach.me {mode}");
        let h = harness_with(
            Profile {
                proxies: &proxies,
                hosts: "yachen.com = 127.0.0.1\nyach.me = 127.0.0.1",
                rules: "DOMAIN,yachen.com,guard",
                sections: &sections,
                ..Profile::default()
            },
            shared_on(NetworkFacts::default()),
        )
        .await;

        let answer = plain_get(h.http(), "http://yachen.com/", "yachen.com").await;
        assert!(
            answer.starts_with(&format!("HTTP/1.1 {status}")),
            "{answer}"
        );
        assert_eq!(
            header_value(&answer, "location").as_deref(),
            Some("https://yach.me/"),
            "{answer}"
        );
        assert_eq!(
            header_value(&answer, "content-length").as_deref(),
            Some("0"),
            "{answer}"
        );
        assert_eq!(
            guard.connections(),
            0,
            "the {mode} rule answers the request without an upstream"
        );
        let records = the_records(&h, 1).await;
        assert_eq!(
            records[0].status_code,
            Some(status.parse().unwrap()),
            "{records:?}"
        );
        assert_eq!(
            records[0].notes,
            [format!("URL Rewrite: {status} by ^http://yachen\\.com")],
            "{records:?}"
        );
        assert_eq!(records[0].policy, ["guard"], "{records:?}");
        assert_eq!(records[0].url.as_deref(), Some("http://yachen.com/"));
        let counts = h.engine.http_counts().rewrite;
        assert_eq!((counts.url, counts.skipped), (1, 0), "{counts:?}");
    }
}

/// The manual's `reject` example (`http/url-rewrite.html`): with the error
/// page on the client gets its 403; with it off the connection is closed and
/// nothing is read. Either way the policy the rule chose is never dialled.
#[tokio::test]
async fn the_manual_url_rewrite_reject_example() {
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    for pages in [true, false] {
        let general = if pages {
            "show-error-page-for-reject = true"
        } else {
            "show-error-page-for-reject = false"
        };
        let h = harness_with(
            Profile {
                general,
                proxies: &proxies,
                hosts: "ad.com = 127.0.0.1",
                rules: "DOMAIN,ad.com,guard",
                sections: "[URL Rewrite]\n^http://ad\\.com/ad\\.png _ reject",
                ..Profile::default()
            },
            shared_on(NetworkFacts::default()),
        )
        .await;

        let answer = plain_get(h.http(), "http://ad.com/ad.png", "ad.com").await;
        if pages {
            assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
            assert!(
                answer.contains("The request was rejected by a URL Rewrite rule."),
                "{answer}"
            );
        } else {
            assert!(
                answer.is_empty(),
                "a rejection with no page closes the connection: {answer:?}"
            );
        }
        assert_eq!(
            guard.connections(),
            0,
            "the reject answers the request without an upstream"
        );
        let records = the_records(&h, 1).await;
        assert_eq!(
            records[0].status,
            RecordStatus::Rejected("REJECT".into()),
            "{records:?}"
        );
        assert_eq!(
            records[0].status_code,
            if pages { Some(403) } else { None },
            "{records:?}"
        );
        assert_eq!(
            records[0].notes,
            ["URL Rewrite: reject by ^http://ad\\.com/ad\\.png".to_string()],
            "{records:?}"
        );
        let counts = h.engine.http_counts().rewrite;
        assert_eq!((counts.url, counts.skipped), (1, 0), "{counts:?}");
    }
}

/// The manual's "Matching details" (`http/url-rewrite.html`): the request line
/// names the guard and the `Host` field names `ad.com`, and the URL rebuilt
/// from that field is what the rule matches. The guard is where the dial would
/// have gone, so its silence is the evidence that nothing was dialled.
#[tokio::test]
async fn the_manual_matching_details_example() {
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let h = harness_with(
        Profile {
            general: "show-error-page-for-reject = true",
            proxies: &proxies,
            hosts: "ad.com = 127.0.0.1",
            rules: "IP-CIDR,127.0.0.1/32,guard",
            sections: "[URL Rewrite]\n^http://ad\\.com/ad\\.png _ reject",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    let url = format!("http://127.0.0.1:{}/ad.png", guard.port);
    let answer = plain_get(h.http(), &url, "ad.com").await;
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    assert_eq!(
        guard.connections(),
        0,
        "the URL rebuilt from Host matched, so the guard was not dialled"
    );
    let records = the_records(&h, 1).await;
    assert_eq!(records[0].url.as_deref(), Some(url.as_str()));
    assert_eq!(
        records[0].notes,
        ["URL Rewrite: reject by ^http://ad\\.com/ad\\.png".to_string()],
        "{records:?}"
    );
    assert_eq!(records[0].policy, ["guard"], "{records:?}");
}

/// An engine for the manual's header-rewrite examples: `example.com` pinned to
/// the loopback, and the `[Header Rewrite]` lines of one example.
async fn example_engine(section: &str) -> (Harness, TestServer) {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    let h = harness_with(
        Profile {
            hosts: "example.com = 127.0.0.1",
            rules: "DOMAIN,example.com,DIRECT",
            sections: section,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    (h, origin)
}

/// A `GET` in absolute form through the listener, its own fields beside `Host`,
/// and the answer as the client reads it. `example.com` carries the origin's
/// port, which the manual's `^http://example.com` still matches as a prefix.
async fn example_get(h: &Harness, port: u16, fields: &[(&str, &str)]) -> String {
    let fields: String = fields
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    exchange(
        h.http(),
        &format!(
            "GET http://example.com:{port}/a HTTP/1.1\r\nHost: example.com:{port}\r\n\
             {fields}Connection: close\r\n\r\n"
        ),
    )
    .await
}

/// The manual's `[Header Rewrite]` page, its "Example" (`header-rewrite.html`),
/// word for word: four lines on one engine — the two `Cookie` fields gone, a
/// `DNT` added, the `User-Agent` replaced, and the answer's `Date` rewritten.
#[tokio::test]
async fn the_manual_header_rewrite_examples() {
    let (h, origin) = example_engine(
        "[Header Rewrite]\n\
         http-request ^http://example.com header-add DNT 1\n\
         http-request ^http://example.com header-del Cookie\n\
         http-request ^http://example.com header-replace User-Agent Unknown\n\
         http-response ^http://example.com header-replace-regex Date 2022 2023",
    )
    .await;
    // hyper's server adds no `Date` of its own when the route names one
    origin.set_header("/a", "date", "Sat, 01 Jan 2022 00:00:00 GMT");
    let port = origin_addr(&origin).port();
    let request = format!(
        "GET http://example.com:{port}/a HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Cookie: a=1\r\nCookie: b=2\r\nUser-Agent: test/1\r\nConnection: close\r\n\r\n"
    );
    let answer = exchange(h.http(), &request).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let seen = origin.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let dnt: Vec<&str> = seen[0]
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("dnt"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(dnt, ["1"], "exactly one DNT: 1, as the page's After shows");
    assert_eq!(seen[0].header("cookie"), None, "{seen:?}");
    assert_eq!(seen[0].header("user-agent"), Some("Unknown"), "{seen:?}");
    assert_eq!(
        header_value(&answer, "date").as_deref(),
        Some("Sat, 01 Jan 2023 00:00:00 GMT"),
        "{answer}"
    );

    // the direction left out: `header-add` alone
    let (h, origin) =
        example_engine("[Header Rewrite]\n^http://example.com header-add DNT 1").await;
    let port = origin_addr(&origin).port();
    let answer = example_get(&h, port, &[]).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(
        origin.requests()[0].header("dnt"),
        Some("1"),
        "a line with no direction is a request rule"
    );

    // `header-del` before `header-add`: the client's `DNT: 0` is gone, and
    // exactly one `DNT: 1` takes its place
    let (h, origin) = example_engine(
        "[Header Rewrite]\n\
         ^http://example.com header-del DNT\n\
         ^http://example.com header-add DNT 1",
    )
    .await;
    let port = origin_addr(&origin).port();
    let answer = example_get(&h, port, &[("DNT", "0")]).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let seen = origin.requests();
    let dnt: Vec<&str> = seen[0]
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("dnt"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(dnt, ["1"], "{dnt:?}");
}

/// The manual's `header-replace-regex` example (`header-rewrite.html`): the
/// `Safari` of the page's "Before" `User-Agent` becomes the "After" one.
#[tokio::test]
async fn the_manual_header_rewrite_regex_example() {
    let (h, origin) = example_engine(
        "[Header Rewrite]\n\
         http-request ^http://example.com header-replace-regex User-Agent Safari Chrome",
    )
    .await;
    let before = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_12_4) AppleWebKit/603.1.30 (KHTML, like Gecko) Version/10.1 Safari/603.1.30";
    let after = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_12_4) AppleWebKit/603.1.30 (KHTML, like Gecko) Version/10.1 Chrome/603.1.30";
    let port = origin_addr(&origin).port();
    let answer = example_get(&h, port, &[("User-Agent", before)]).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(origin.requests()[0].header("user-agent"), Some(after));
}

/// The manual's "Limitations" (`header-rewrite.html`): a line that changes a
/// message's framing fails the request, and the origin is never asked. The
/// line is kept at load time with `W0041`, which this case does not count.
#[tokio::test]
async fn the_manual_header_rewrite_framing_limit() {
    let (h, origin) = example_engine(
        "[Header Rewrite]\nhttp-request ^http://example.com header-del Content-Length",
    )
    .await;
    let port = origin_addr(&origin).port();
    let body = "body";
    let request = format!(
        "POST http://example.com:{port}/a HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let answer = exchange(h.http(), &request).await;
    assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
    assert!(
        origin.requests().is_empty(),
        "a request whose framing changed is not sent"
    );
    let records = the_records(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(h.engine.http_counts().rewrite.failed, 1);
}

// ---- the decrypted side: what `tests/mitm.rs` keeps for its own cases, copied
// ---- here (`the phase 4 / M3a plan's P19`; the helpers stay where they are
// ---- used, not in `common`).

/// The `[MITM]` section of a profile: the hosts to decrypt, and the CA rurge
/// signs with as `ca-p12` with the password `pw` (`tests/mitm.rs`).
fn mitm_section(hostname: &str, ca_p12: &str) -> String {
    format!("[MITM]\nhostname = {hostname}\nca-p12 = {ca_p12}\nca-passphrase = pw")
}

/// A CA for rurge to sign with, and its trust anchors (`tests/mitm.rs`). Its
/// subject is not the origin fixture's, so a client trusting only one of the
/// two tells them apart.
fn mitm_ca() -> Arc<TlsFixture> {
    TlsFixture::with_ca_name("rurge test MITM CA", &[])
}

/// A rustls client trusting `roots` and nothing else; `sni` is whether it
/// sends the server name extension (`tests/mitm.rs`).
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
/// every TLS step of these cases is (`tests/mitm.rs`).
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
/// (`tests/mitm.rs`).
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

/// The whole response at `s`, its body read by its `Content-Length`
/// (`tests/mitm.rs`).
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

/// One request over a decrypted connection, and the whole response
/// (`tests/mitm.rs`'s `get`).
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
/// the cases of `tests/mitm.rs` do.
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

/// A decrypted connection to `origin.test:{port}` that offered `alpn`, and the
/// protocol the handshake settled on (`tests/mitm.rs`, with the port written
/// down by `[MITM]`'s hostname, so the port is sniffed).
async fn decrypted(
    h: &Harness,
    roots: Arc<rustls::RootCertStore>,
    port: u16,
    alpn: &[&[u8]],
) -> (Option<String>, tokio_rustls::client::TlsStream<TcpStream>) {
    let tunnel = connect_via_socks5(h.socks(), "origin.test", port).await;
    let tls = handshake(tunnel, client_config(roots, alpn, true), "origin.test")
        .await
        .expect("the client trusts the MITM CA");
    let settled = tls
        .get_ref()
        .1
        .alpn_protocol()
        .map(|p| String::from_utf8_lossy(p).into_owned());
    (settled, tls)
}

/// hyper's HTTP/2 client over a decrypted connection: the sender, and the task
/// that has to keep running for the connection to move (`tests/mitm.rs`).
async fn h2_over(
    tls: tokio_rustls::client::TlsStream<TcpStream>,
) -> (
    http2::SendRequest<Full<Bytes>>,
    tokio::task::JoinHandle<hyper::Result<()>>,
) {
    let (sender, conn) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(TokioIo::new(tls))
        .await
        .expect("an HTTP/2 handshake over the decrypted stream");
    (sender, tokio::spawn(conn))
}

/// Lets an HTTP/2 client go: its sender is dropped and the connection's task
/// ends within the bound, cleanly (`tests/mitm.rs`).
async fn close_h2(
    sender: http2::SendRequest<Full<Bytes>>,
    conn: tokio::task::JoinHandle<hyper::Result<()>>,
) {
    drop(sender);
    tokio::time::timeout(Duration::from_secs(5), conn)
        .await
        .expect("the connection ends within the bound")
        .expect("the connection's task did not panic")
        .expect("the client's side of the connection ends cleanly");
}

/// A `GET` as hyper's HTTP/2 client takes it: the absolute URI names the
/// scheme and the authority (`tests/mitm.rs`).
fn h2_get(path: &str) -> Request<Full<Bytes>> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("https://origin.test{path}"))
        .body(Full::new(Bytes::new()))
        .expect("a request")
}

/// One request on an HTTP/2 connection, bounded as every wait of these cases
/// is: the status, and the whole body (`tests/mitm.rs`).
async fn h2_send(
    sender: &mut http2::SendRequest<Full<Bytes>>,
    request: Request<Full<Bytes>>,
) -> (StatusCode, Vec<u8>) {
    let response = tokio::time::timeout(Duration::from_secs(5), sender.send_request(request))
        .await
        .expect("the answer comes within the bound")
        .expect("the request goes through");
    let status = response.status();
    let body = tokio::time::timeout(Duration::from_secs(5), response.into_body().collect())
        .await
        .expect("the body comes within the bound")
        .expect("a response body")
        .to_bytes()
        .to_vec();
    (status, body)
}

/// A decrypted `https://` request is rewritten: the header rules act on it,
/// and the `header` rule sends it to another TLS origin — a name the decrypted
/// connection was not for — before the dial happens (`phase 4 M3 design 8.1`,
/// `8.2`).
///
/// `[MITM]`'s hostname writes the origin's port down, so that port is sniffed
/// and decrypted (`phase 4 M2 design 7.6`, `tests/mitm.rs`'s
/// `a_written_port_makes_its_port_sniffable`).
#[tokio::test]
async fn rewrites_on_a_decrypted_https_request() {
    let origin = TlsFixture::new(&["origin.test", "localhost"]);
    let first = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    first.set("/a", "at the first origin");
    let second = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    second.set("/a", "at the second origin");
    let (first_port, second_port) = (origin_addr(&first).port(), origin_addr(&second).port());
    let mitm = mitm_ca();
    let sections = format!(
        "{}\n\
         [URL Rewrite]\n\
         ^https://origin\\.test:\\d+ https://localhost:{second_port} header\n\
         [Header Rewrite]\n\
         http-request ^https://origin\\.test header-add X-A 1",
        mitm_section(
            &format!("origin.test:{first_port}"),
            &mitm.ca_p12_base64("pw")
        )
    );
    let h = harness_mitm(
        Profile {
            hosts: "origin.test = 127.0.0.1\nlocalhost = 127.0.0.1",
            rules: "DOMAIN,origin.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let (settled, mut tls) = decrypted(&h, mitm.roots(), first_port, &[b"http/1.1"]).await;
    assert_eq!(settled.as_deref(), Some("http/1.1"), "{settled:?}");
    let answer = decrypted_get(&mut tls, &format!("origin.test:{first_port}"), "/a").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("at the second origin"), "{answer}");

    assert_eq!(
        first.requests().len(),
        0,
        "the rewrite changed the target before the dial"
    );
    let seen = second.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].path, "/a");
    assert_eq!(
        seen[0].header("host"),
        Some(format!("localhost:{second_port}").as_str())
    );
    assert_eq!(seen[0].header("x-a"), Some("1"));
    let records = the_records(&h, 1).await;
    assert_eq!(
        records[0].url.as_deref(),
        Some(format!("https://origin.test:{first_port}/a").as_str()),
        "the record keeps the URL the request came with"
    );
    assert!(
        records[0].notes.contains(
            &"Header Rewrite: request header-add X-A by ^https://origin\\.test".to_string()
        ),
        "{:?}",
        records[0].notes
    );
    assert!(
        records[0].notes.contains(&format!(
            "URL Rewrite: header by ^https://origin\\.test:\\d+ -> https://localhost:{second_port}/a"
        )),
        "{:?}",
        records[0].notes
    );
}

/// A decrypted HTTP/2 request rewritten to `http://` is sent plainly, and the
/// origin that speaks HTTP/1.1 is spoken to in HTTP/1.1 with the new authority
/// as its `Host` (`phase 4 M3 design 6.3`, `8.2`; `version.rs`'s `fit`).
#[tokio::test]
async fn a_decrypted_http2_request_can_be_sent_to_plain_http() {
    let tls_origin = TlsFixture::new(&["origin.test"]);
    let first = TestServer::spawn_tls_with(tls_origin.acceptor(false)).await;
    first.set("/a", "never reached");
    let plain = TestServer::spawn().await;
    plain.set("/a", "plain origin");
    let (first_port, plain_port) = (origin_addr(&first).port(), origin_addr(&plain).port());
    let mitm = mitm_ca();
    let sections = format!(
        "{}\nh2 = true\n\
         [URL Rewrite]\n\
         ^https://origin\\.test http://localhost:{plain_port} header",
        mitm_section(
            &format!("origin.test:{first_port}"),
            &mitm.ca_p12_base64("pw")
        )
    );
    let h = harness_mitm(
        Profile {
            hosts: "origin.test = 127.0.0.1\nlocalhost = 127.0.0.1",
            rules: "DOMAIN,origin.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        tls_origin.roots(),
    )
    .await;

    let (settled, tls) = decrypted(&h, mitm.roots(), first_port, &[b"h2"]).await;
    assert_eq!(settled.as_deref(), Some("h2"), "{settled:?}");
    let (mut sender, conn) = h2_over(tls).await;
    let (status, body) = h2_send(&mut sender, h2_get("/a")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"plain origin");
    close_h2(sender, conn).await;

    let seen = plain.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        seen[0].version, "HTTP/1.1",
        "a plain origin is spoken to in HTTP/1.1: {seen:?}"
    );
    assert_eq!(seen[0].path, "/a");
    assert_eq!(
        seen[0].header("host"),
        Some(format!("localhost:{plain_port}").as_str())
    );
    assert_eq!(first.requests().len(), 0);
}

/// A `Host` a decrypted HTTP/2 request did not have, added by a header rule,
/// becomes the `:authority` of the HTTP/2 request to the origin (`phase 4 M3
/// design 7.4`, `M3-D30`).
#[tokio::test]
async fn a_changed_authority_reaches_an_http2_origin() {
    let origin = TlsFixture::new(&["origin.test"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "from the origin");
    let port = origin_addr(&server).port();
    let mitm = mitm_ca();
    let sections = format!(
        "{}\nh2 = true\n\
         [Header Rewrite]\n\
         http-request ^https://origin\\.test header-add Host h.test:{port}",
        mitm_section(&format!("origin.test:{port}"), &mitm.ca_p12_base64("pw"))
    );
    let h = harness_mitm(
        Profile {
            hosts: "origin.test = 127.0.0.1\nh.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let (settled, tls) = decrypted(&h, mitm.roots(), port, &[b"h2"]).await;
    assert_eq!(settled.as_deref(), Some("h2"), "{settled:?}");
    let (mut sender, conn) = h2_over(tls).await;
    let (status, body) = h2_send(&mut sender, h2_get("/a")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"from the origin");
    close_h2(sender, conn).await;

    let seen = server.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].version, "HTTP/2.0", "{seen:?}");
    assert_eq!(
        seen[0].authority,
        Some(format!("h.test:{port}")),
        "the Host the rule added is the :authority: {seen:?}"
    );
    assert_eq!(
        seen[0].header("host"),
        None,
        "an HTTP/2 request names no Host field: {seen:?}"
    );
}

/// A plain `http://` request rewritten to `https://` reaches a TLS origin with
/// no certificate authority of `[MITM]`'s: the trust comes from
/// `EngineShared.roots` and the two configurations `Rewrite` builds itself
/// (`phase 4 M3 design 8.2`, `M3-D16`).
#[tokio::test]
async fn http_rewritten_to_https_reaches_a_tls_origin_without_a_ca() {
    let origin = TlsFixture::new(&["localhost"]);
    let server = TestServer::spawn_tls_with(origin.acceptor(false)).await;
    server.set("/a", "over TLS");
    let port = origin_addr(&server).port();
    let sections = format!("[URL Rewrite]\n^http://t\\.test:\\d+ https://localhost:{port} header");
    let h = harness_trusting(
        Profile {
            hosts: "t.test = 127.0.0.1\nlocalhost = 127.0.0.1",
            rules: "DOMAIN,t.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        origin.roots(),
    )
    .await;

    let url = format!("http://t.test:{port}/a");
    let answer = plain_get(h.http(), &url, &format!("t.test:{port}")).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("over TLS"), "{answer}");
    let seen = server.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        seen[0].header("host"),
        Some(format!("localhost:{port}").as_str())
    );
    let records = the_records(&h, 1).await;
    assert_eq!(records[0].url.as_deref(), Some(url.as_str()));
    assert!(
        records[0].notes.contains(&format!(
            "URL Rewrite: header by ^http://t\\.test:\\d+ -> https://localhost:{port}/a"
        )),
        "{:?}",
        records[0].notes
    );
    assert_eq!(h.engine.http_counts().rewrite.url, 1);
}

/// Two requests on one client connection to one target — one plain, one a
/// rewrite made TLS — do not share a pooled upstream connection: the plain
/// origin counts two, and the TLS one fails at the handshake (`phase 4 M3
/// design 8.3`, `the phase 4 / M3a plan's P14`).
#[tokio::test]
async fn plain_and_tls_requests_to_one_target_do_not_share_a_connection() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "first");
    let port = origin_addr(&origin).port();
    let sections = format!(
        "[URL Rewrite]\n^http://127\\.0\\.0\\.1:{port}/b https://127\\.0\\.0\\.1:{port}/b header"
    );
    let h = harness_with(
        Profile {
            sections: &sections,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(
        format!("GET http://127.0.0.1:{port}/a HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let first = read_response(&mut s).await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    assert!(first.ends_with("first"), "{first}");

    s.write_all(
        format!("GET http://127.0.0.1:{port}/b HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let second = read_response(&mut s).await;
    assert!(
        second.starts_with("HTTP/1.1 502"),
        "the plain origin cannot speak TLS: {second}"
    );
    drop(s);

    assert_eq!(
        origin.connections(),
        2,
        "the plain and the TLS request dialled one connection each"
    );
    assert_eq!(origin.hits("/a"), 1);
    assert_eq!(origin.hits("/b"), 0);
    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(
        h.engine.http_counts().rewrite.url,
        1,
        "only the rewritten request counts: {:?}",
        h.engine.http_counts().rewrite
    );
}

/// A hand-written upstream HTTP proxy: it records the request line of every
/// request, answers a plain one with an empty 200, and closes on anything else
/// — a `CONNECT` gets no answer, so the TLS handshake that follows fails at
/// once rather than waiting out its bound.
struct RecordingProxy {
    addr: SocketAddr,
    lines: Arc<std::sync::Mutex<Vec<String>>>,
}

impl RecordingProxy {
    async fn spawn() -> RecordingProxy {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let lines: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let kept = lines.clone();
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
                    kept.lock().unwrap().push(line.clone());
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
        RecordingProxy { addr, lines }
    }

    /// The request line of every request it was asked, in order.
    fn lines(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }
}

/// A listener request rewritten through a forwarding HTTP proxy
/// (`always-use-connect = false`): a plain new target goes to the proxy in
/// absolute form, a TLS one goes as `CONNECT` (`phase 4 M3 design 6.3`,
/// `M3-D28`).
#[tokio::test]
async fn a_listener_request_rewritten_through_a_forwarding_proxy() {
    let proxy = RecordingProxy::spawn().await;
    let proxy_port = proxy.addr.port();

    // to a plain target: the proxy gets the new URL in absolute form
    let sections =
        format!("[URL Rewrite]\n^http://r\\.test:\\d+/p http://new.test:{proxy_port}/p header");
    let h = harness_with(
        Profile {
            proxies: &format!("P = http, 127.0.0.1, {proxy_port}, always-use-connect=false"),
            hosts: "r.test = 127.0.0.1\nnew.test = 127.0.0.1",
            rules: "DOMAIN,r.test,P",
            sections: &sections,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let answer = plain_get(
        h.http(),
        &format!("http://r.test:{proxy_port}/p"),
        &format!("r.test:{proxy_port}"),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(
        proxy.lines(),
        [format!("GET http://new.test:{proxy_port}/p HTTP/1.1")],
        "a plain target is forwarded in absolute form"
    );
    drop(h);

    // to a TLS target: the proxy gets `CONNECT` and the handshake fails
    let sections =
        format!("[URL Rewrite]\n^http://r\\.test:\\d+/p https://new.test:{proxy_port}/p header");
    let h = harness_with(
        Profile {
            proxies: &format!("P = http, 127.0.0.1, {proxy_port}, always-use-connect=false"),
            hosts: "r.test = 127.0.0.1\nnew.test = 127.0.0.1",
            rules: "DOMAIN,r.test,P",
            sections: &sections,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let answer = plain_get(
        h.http(),
        &format!("http://r.test:{proxy_port}/p"),
        &format!("r.test:{proxy_port}"),
    )
    .await;
    assert!(
        answer.starts_with("HTTP/1.1 502") || answer.is_empty(),
        "the proxy answered no tunnel, so the request fails: {answer}"
    );
    assert_eq!(
        proxy.lines().last().map(String::as_str),
        Some(format!("CONNECT new.test:{proxy_port} HTTP/1.1").as_str()),
        "a TLS target is tunnelled: {:?}",
        proxy.lines()
    );
}

/// A header rule that changes `Host` reaches the origin: `header-replace`
/// sends the new name, and `header-del` gives the original back — the request
/// must name a host (`phase 4 M3 design 7.4`, `M3-D30`). The listener's request
/// and a sniffed connection's request both carry the changed `Host`.
#[tokio::test]
async fn a_changed_host_reaches_the_origin() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "a");
    origin.set("/b", "b");
    let port = origin_addr(&origin).port();
    let general = format!("force-http-engine-hosts = h.test:{port}");

    // `header-replace`: both entries send `Host: h.test`
    let h = harness_with(
        Profile {
            general: &general,
            hosts: "h.test = 127.0.0.1",
            rules: "DOMAIN,h.test,DIRECT",
            sections: "[Header Rewrite]\n^http://h\\.test header-replace Host h.test",
            ..Profile::default()
        },
        EngineShared {
            sniff_wait: Duration::from_millis(300),
            ..shared_on(NetworkFacts::default())
        },
    )
    .await;
    let host = format!("h.test:{port}");
    let answer = plain_get(h.http(), &format!("http://h.test:{port}/a"), &host).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let mut tunnel = connect_via_socks5(h.socks(), "h.test", port).await;
    let answer = get(&mut tunnel, &host, "/b").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let seen = origin.requests();
    assert_eq!(seen.len(), 2, "{seen:?}");
    for request in &seen {
        assert_eq!(
            request.header("host"),
            Some("h.test"),
            "the replaced Host reached the origin: {request:?}"
        );
    }
    let records = the_records(&h, 2).await;
    assert!(
        records[0].notes.contains(
            &"Header Rewrite: request header-replace Host by ^http://h\\.test".to_string()
        ),
        "{:?}",
        records[0].notes
    );
    drop(h);

    // `header-del`: the original `Host` comes back, and a note says why
    let h = harness_with(
        Profile {
            general: &general,
            hosts: "h.test = 127.0.0.1",
            rules: "DOMAIN,h.test,DIRECT",
            sections: "[Header Rewrite]\n^http://h\\.test header-del Host",
            ..Profile::default()
        },
        EngineShared {
            sniff_wait: Duration::from_millis(300),
            ..shared_on(NetworkFacts::default())
        },
    )
    .await;
    let answer = plain_get(h.http(), &format!("http://h.test:{port}/a"), &host).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let mut tunnel = connect_via_socks5(h.socks(), "h.test", port).await;
    let answer = get(&mut tunnel, &host, "/b").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let seen = origin.requests();
    let last_two = &seen[seen.len() - 2..];
    for request in last_two {
        assert_eq!(
            request.header("host"),
            Some(host.as_str()),
            "a removed Host is given back: {request:?}"
        );
    }
    let records = the_records(&h, 2).await;
    for record in &records {
        assert!(
            record
                .notes
                .contains(&"Header Rewrite skipped: Host cannot be removed".to_string()),
            "{:?}",
            record.notes
        );
    }
}

/// An absolute-form request line on a sniffed connection is not rewritten: the
/// client is talking to its own proxy, and rurge relays the request as it came
/// (`phase 4 M3 design 8.4`, `M3-D29`).
#[tokio::test]
async fn an_absolute_form_request_on_a_sniffed_connection_is_not_rewritten() {
    let origin = TestServer::spawn().await;
    origin.set("/a", "as it came");
    let port = origin_addr(&origin).port();
    let general = format!("force-http-engine-hosts = target.test:{port}");
    let h = harness_with(
        Profile {
            general: &general,
            hosts: "target.test = 127.0.0.1\nelse.test = 127.0.0.1",
            rules: "DOMAIN,target.test,DIRECT",
            sections: "[URL Rewrite]\n^http://target\\.test http://else.test/ 302\n\
                       [Header Rewrite]\n^http://target\\.test header-add X-A 1",
            ..Profile::default()
        },
        EngineShared {
            sniff_wait: Duration::from_millis(300),
            ..shared_on(NetworkFacts::default())
        },
    )
    .await;

    let mut tunnel = connect_via_socks5(h.socks(), "target.test", port).await;
    let answer = exchange_over(
        &mut tunnel,
        &format!(
            "GET http://target.test:{port}/a HTTP/1.1\r\nHost: target.test:{port}\r\n\
             Connection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("as it came"), "{answer}");
    let seen = origin.requests();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        seen[0].header("x-a"),
        None,
        "no header rule acts on an absolute-form request: {seen:?}"
    );
    let records = the_records(&h, 1).await;
    assert!(
        records[0].status_code == Some(200),
        "a 302 would have answered this request: {records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (
            counts.url,
            counts.header,
            counts.body,
            counts.map_local,
            counts.skipped,
            counts.failed
        ),
        (0, 0, 0, 0, 0, 0),
        "{counts:?}"
    );
}

/// A request a rewrite retargeted does not carry the client's
/// `Proxy-Authorization` to its new host, while one that kept its target still
/// does: on a sniffed connection those credentials were the client's, for the
/// proxy it named (`phase 4 M3 design 8.1`, `M3-D28`).
#[tokio::test]
async fn a_retargeted_request_carries_no_proxy_authorization() {
    let origin = TestServer::spawn().await;
    origin.set("/keep", "kept");
    origin.set("/moved", "moved");
    let port = origin_addr(&origin).port();
    let general = format!("force-http-engine-hosts = target.test:{port}");
    let sections = format!(
        "[URL Rewrite]\n^http://target\\.test:{port}/moved http://other.test:{port}/moved header"
    );
    let h = harness_with(
        Profile {
            general: &general,
            hosts: "target.test = 127.0.0.1\nother.test = 127.0.0.1",
            rules: "DOMAIN,target.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        EngineShared {
            sniff_wait: Duration::from_millis(300),
            ..shared_on(NetworkFacts::default())
        },
    )
    .await;

    let host = format!("target.test:{port}");
    let mut tunnel = connect_via_socks5(h.socks(), "target.test", port).await;
    tunnel
        .write_all(
            format!(
                "GET /keep HTTP/1.1\r\nHost: {host}\r\nProxy-Authorization: Basic eDp5\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let first = read_response(&mut tunnel).await;
    assert!(first.starts_with("HTTP/1.1 200"), "{first}");
    tunnel
        .write_all(
            format!(
                "GET /moved HTTP/1.1\r\nHost: {host}\r\nProxy-Authorization: Basic eDp5\r\n\
                 Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let second = read_response(&mut tunnel).await;
    assert!(second.starts_with("HTTP/1.1 200"), "{second}");
    drop(tunnel);

    let seen = origin.requests();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let kept = seen
        .iter()
        .find(|r| r.path == "/keep")
        .expect("the kept one");
    assert_eq!(
        kept.header("proxy-authorization"),
        Some("Basic eDp5"),
        "a request that kept its target keeps the credentials: {kept:?}"
    );
    let moved = seen
        .iter()
        .find(|r| r.path == "/moved")
        .expect("the moved one");
    assert_eq!(
        moved.header("proxy-authorization"),
        None,
        "a retargeted request carries none: {moved:?}"
    );
    assert_eq!(
        moved.header("host"),
        Some(format!("other.test:{port}").as_str()),
        "{moved:?}"
    );
}

/// An expression that runs away leaves a note and is skipped; the request goes
/// where it was going (`phase 4 M3 design 5.5`; the expression and its input
/// are the ones of `a_rule_that_hits_the_backtracking_limit_is_skipped_with_a_note`
/// in `crates/rurge-http/src/rewrite/url.rs`).
#[tokio::test]
async fn a_regex_that_backtracks_too_long_skips_the_rewrite() {
    let origin = TestServer::spawn().await;
    let path = format!("/{}", "ab".repeat(40));
    origin.set(&path, "reached");
    let port = origin_addr(&origin).port();
    let h = harness_with(
        Profile {
            hosts: "x.test = 127.0.0.1\nb.test = 127.0.0.1",
            rules: "DOMAIN,x.test,DIRECT",
            sections: "[URL Rewrite]\n(?i)(a|b|ab)*(?=c) http://b.test header",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    let url = format!("http://x.test:{port}{path}");
    let answer = plain_get(h.http(), &url, &format!("x.test:{port}")).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("reached"), "{answer}");
    assert_eq!(origin.hits(&path), 1, "the request was not held up");
    let records = the_records(&h, 1).await;
    assert_eq!(
        records[0].notes,
        ["URL Rewrite skipped: (?i)(a|b|ab)*(?=c): the regular expression hit its backtracking limit"
            .to_string()],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.url, counts.skipped, counts.failed), (0, 1, 0));
}

/// Neither a replaced header's value nor the query string of a request reaches
/// the notes or the log, which this case keeps down to TRACE (`phase 4 M3
/// design 8.5`, `14`; `M3-D18`).
#[tokio::test]
async fn no_header_value_reaches_the_notes_or_the_log() {
    let log = LogCapture::everything();
    let origin = TestServer::spawn().await;
    origin.set("/p", "done");
    let port = origin_addr(&origin).port();
    let sections = format!(
        "[URL Rewrite]\n^http://s\\.test:\\d+ http://localhost:{port} header\n\
         [Header Rewrite]\n\
         http-request ^http://s\\.test header-replace Authorization Bearer s3cr3t-z"
    );
    let h = harness_with(
        Profile {
            hosts: "s.test = 127.0.0.1\nlocalhost = 127.0.0.1",
            rules: "DOMAIN,s.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    let url = format!("http://s.test:{port}/p?tok=q1");
    let request = format!(
        "GET {url} HTTP/1.1\r\nHost: s.test:{port}\r\nAuthorization: old\r\n\
         Connection: close\r\n\r\n"
    );
    let answer = exchange(h.http(), &request).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(
        origin.requests()[0].header("authorization"),
        Some("Bearer s3cr3t-z"),
        "the rewritten value does reach the origin"
    );
    let records = the_records(&h, 1).await;
    assert!(
        records[0].notes.len() >= 2,
        "both rules left a note: {:?}",
        records[0].notes
    );
    for note in &records[0].notes {
        assert!(
            !note.contains("s3cr3t-z"),
            "a note carries the value: {note}"
        );
        assert!(!note.contains("q1"), "a note carries the query: {note}");
    }
    for line in log.lines() {
        assert!(
            !line.contains("s3cr3t-z"),
            "a log line carries the replaced value: {line}"
        );
    }
}

/// The six counts of `/v1/rurge/http`'s `rewrite`, one request at a time
/// (`phase 4 M3 design 12`, `the phase 4 / M3a plan's P11`): `body` and
/// `mapLocal` are M3b's and stay zero.
#[tokio::test]
async fn notes_and_counts_reach_the_api_shape() {
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let origin = TestServer::spawn().await;
    origin.set("/a", "a");
    origin.set("/gone", "gone");
    let port = origin_addr(&origin).port();
    let sections = format!(
        "[URL Rewrite]\n\
         ^http://a\\.test:\\d+/gone http:/// header\n\
         ^http://a\\.test:\\d+/a http://localhost:{port}/a header\n\
         ^http://r\\.test https://s.test/ 302\n\
         [Header Rewrite]\n\
         ^http://a\\.test:\\d+/a header-add X-A 1\n\
         http-request ^http://f\\.test header-del Content-Length"
    );
    let h = harness_with(
        Profile {
            proxies: &proxies,
            hosts: "a.test = 127.0.0.1\nlocalhost = 127.0.0.1\nr.test = 127.0.0.1\n\
                    s.test = 127.0.0.1\nf.test = 127.0.0.1",
            rules: "DOMAIN,a.test,DIRECT\nDOMAIN,r.test,guard\nDOMAIN,f.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    // 1. `/a`: the URL rule and the header rule both act — url 1, header 1
    let answer = plain_get(
        h.http(),
        &format!("http://a.test:{port}/a"),
        &format!("a.test:{port}"),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(origin.requests()[0].header("x-a"), Some("1"));

    // 2. `/gone`: the first URL rule's result is no URL — skipped 1
    let answer = plain_get(
        h.http(),
        &format!("http://a.test:{port}/gone"),
        &format!("a.test:{port}"),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");

    // 3. `r.test/`: the 302 answers — url 2
    let answer = plain_get(h.http(), "http://r.test/", "r.test").await;
    assert!(answer.starts_with("HTTP/1.1 302"), "{answer}");

    // 4. the POST whose framing changes: header 2, failed 1, and nothing is
    // sent
    let body = "body";
    let answer = exchange(
        h.http(),
        &format!(
            "POST http://f.test:{port}/post HTTP/1.1\r\nHost: f.test:{port}\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");

    // `header` counts the two requests whose fields a rule changed: the first,
    // and the POST whose framing changed before it failed. `body` and
    // `mapLocal` are the M3b steps: 0 until then.
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (
            counts.url,
            counts.header,
            counts.body,
            counts.map_local,
            counts.skipped,
            counts.failed
        ),
        (2, 2, 0, 0, 1, 1),
        "{counts:?}"
    );
    let records = the_records(&h, 4).await;
    assert_eq!(records.len(), 4, "{records:?}");
    assert!(
        records[1]
            .notes
            .contains(&"URL Rewrite skipped: ^http://a\\.test:\\d+/gone: the rewritten URL is not a valid http, https, ws or wss URL".to_string()),
        "{:?}",
        records[1].notes
    );
    assert_eq!(
        guard.connections(),
        0,
        "the 302 answered the request without an upstream"
    );
}

// ---- the data of a `[Map Local]` line that names a URL (`phase 4 M3 design
// ---- 10.3`; the phase 4 / M3b plan's P19): a resource of the generation.

/// As `harness_with`, with downloads on: the GeoIP databases and the data of a
/// `[Map Local]` URL ask `server`, and the listeners are bound as there. A
/// name a case catches is pinned to `127.0.0.1` by `[Host]`.
async fn online_harness(p: Profile<'_>, shared: EngineShared, server: &TestServer) -> Harness {
    let dns = MockDns::spawn().await;
    for name in ["target.test", "alt.test", "m.test"] {
        dns.set(name, &["127.0.0.1"], &[], 60);
    }
    let dir = tempfile::tempdir().unwrap();
    let text = p.text(dns.addr());
    let engine = Engine::new(online_runtime_with(dir.path(), &text, server, shared, |_| {}).await);
    let listeners = engine.bind_listeners().await.unwrap();
    Harness {
        dir,
        engine,
        listeners,
        dns,
    }
}

/// The items of the listing that are the data of a `[Map Local]` URL.
fn map_local_data(h: &Harness) -> Vec<ExternalResource> {
    h.engine
        .external_resources(None)
        .into_iter()
        .filter(|resource| resource.kind == "data")
        .collect()
}

/// The data of a `[Map Local]` line that names a URL is listed by the control
/// plane as `data`, under a label that says where it comes from, and the
/// request is answered from it once it is in: the origin the policy chose is
/// never dialled, and the `Content-Type` follows the path's extension.
#[tokio::test]
async fn map_local_urls_are_listed_as_data() {
    let server = TestServer::spawn().await;
    server.set("/m.json", "{\"a\":1}");
    let data = server.url("/m.json");
    let origin = rurge_config::remote::origin(&data);
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let sections = format!("[Map Local]\n^http://m\\.test/ data=\"{data}\"");
    let h = online_harness(
        Profile {
            proxies: &proxies,
            hosts: "m.test = 127.0.0.1",
            rules: "DOMAIN,m.test,guard",
            sections: &sections,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
        &server,
    )
    .await;

    wait_until("the Map Local data to be downloaded", || {
        map_local_data(&h).iter().any(|resource| resource.ready)
    })
    .await;
    let listed = map_local_data(&h);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0].kind, "data");
    assert!(listed[0].path.starts_with("map local data "), "{listed:?}");
    assert!(
        listed[0].path.ends_with(&format!("({origin})")),
        "{listed:?}"
    );

    let answer = plain_get(h.http(), "http://m.test/", "m.test").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(
        header_value(&answer, "content-type").as_deref(),
        Some("application/json"),
        "{answer}"
    );
    assert!(answer.ends_with("{\"a\":1}"), "{answer}");
    assert_eq!(
        guard.connections(),
        0,
        "Map Local answered the request without an upstream"
    );

    let records = the_records(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Completed, "{records:?}");
    assert_eq!(
        records[0].notes,
        ["Map Local: file by ^http://m\\.test/".to_string()],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.failed), (1, 0), "{counts:?}");
}

/// A `[Map Local]` URL whose download failed answers 502, with the origin and
/// not the data in the record; the policy is not dialled either.
#[tokio::test]
async fn map_local_url_data_that_never_came_answers_502() {
    let server = TestServer::spawn().await;
    let data = server.url("/m.json");
    let origin = rurge_config::remote::origin(&data);
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let sections = format!("[Map Local]\n^http://m\\.test/ data=\"{data}\"");
    let h = online_harness(
        Profile {
            proxies: &proxies,
            hosts: "m.test = 127.0.0.1",
            rules: "DOMAIN,m.test,guard",
            sections: &sections,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
        &server,
    )
    .await;

    wait_until("the Map Local data to fail its first download", || {
        map_local_data(&h)
            .iter()
            .any(|resource| resource.error.is_some())
    })
    .await;

    let answer = plain_get(h.http(), "http://m.test/", "m.test").await;
    assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
    assert_eq!(
        guard.connections(),
        0,
        "a Map Local failure does not dial the policy"
    );

    let records = the_records(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    let unavailable = format!("Map Local: the data from {origin} is not available yet");
    assert_eq!(
        records[0].error.as_deref(),
        Some(unavailable.as_str()),
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.failed), (0, 1), "{counts:?}");
}

/// Two `[Map Local]` lines that name the same URL are one resource: the first
/// registration fixes the kind and the label (`phase 4 M3 design 10.3`).
#[tokio::test]
async fn the_same_map_local_url_is_one_resource() {
    let server = TestServer::spawn().await;
    server.set("/m.json", "{\"a\":1}");
    let data = server.url("/m.json");
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let sections = format!(
        "[Map Local]\n^http://m\\.test/a data=\"{data}\"\n^http://m\\.test/b data=\"{data}\""
    );
    let h = online_harness(
        Profile {
            proxies: &proxies,
            hosts: "m.test = 127.0.0.1",
            rules: "DOMAIN,m.test,guard",
            sections: &sections,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
        &server,
    )
    .await;

    wait_until("the Map Local data to be downloaded", || {
        map_local_data(&h).iter().any(|resource| resource.ready)
    })
    .await;
    let listed = map_local_data(&h);
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(server.hits("/m.json"), 1, "the URL is downloaded once");
}

// ---- the manual's two pages end to end (`the phase 4 / M3b plan's task 7`):
// ---- the origin whose answer a case writes whole, the client side that reads
// ---- a body as bytes, and the cases themselves.

/// An origin that answers every request with the bytes a case wrote, and keeps
/// the head and the body of every request it read (`the phase 4 / M3b plan's
/// P20`): the manual has no example of a chunked body, a compressed one, one
/// with a trailer or one that stops half way, and `TestServer` writes the
/// framing of its own answers, so a case that needs a particular answer writes
/// it whole here. `RecordingOrigin` above keeps the heads alone, for the cases
/// that need only those.
struct ScriptedOrigin {
    addr: SocketAddr,
    heard: Heard,
    accepted: Arc<std::sync::atomic::AtomicUsize>,
}

/// What a `ScriptedOrigin` keeps of the requests it was asked: each one's head
/// and its body.
type Heard = Arc<std::sync::Mutex<Vec<(String, Vec<u8>)>>>;

impl ScriptedOrigin {
    /// Serves `answer` to every request, and keeps the connection open after
    /// it: an answer that stops half way is exactly that.
    async fn spawn(answer: Vec<u8>) -> ScriptedOrigin {
        ScriptedOrigin::spawn_with(None, answer).await
    }

    /// As `spawn`, behind the TLS the acceptor speaks: for the cases whose
    /// request reaches its origin over TLS (`[Map Local]`'s and Body Rewrite's
    /// decrypted requests).
    async fn spawn_tls(acceptor: tokio_rustls::TlsAcceptor, answer: Vec<u8>) -> ScriptedOrigin {
        ScriptedOrigin::spawn_with(Some(acceptor), answer).await
    }

    async fn spawn_with(
        acceptor: Option<tokio_rustls::TlsAcceptor>,
        answer: Vec<u8>,
    ) -> ScriptedOrigin {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let heard: Heard = Arc::default();
        let kept = heard.clone();
        let accepted: Arc<std::sync::atomic::AtomicUsize> = Arc::default();
        let counted = accepted.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let answer = answer.clone();
                let kept = kept.clone();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    match acceptor {
                        // nothing was said if the handshake failed: the
                        // connection is gone
                        Some(acceptor) => {
                            if let Ok(mut stream) = acceptor.accept(stream).await {
                                serve(&mut stream, &answer, &kept).await;
                            }
                        }
                        None => {
                            let mut stream = stream;
                            serve(&mut stream, &answer, &kept).await;
                        }
                    }
                });
            }
        });
        ScriptedOrigin {
            addr,
            heard,
            accepted,
        }
    }

    fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The head and the body of every request it was asked, in order; the body
    /// of a chunked request is its data, with the framing taken off.
    fn heard(&self) -> Vec<(String, Vec<u8>)> {
        self.heard.lock().unwrap().clone()
    }

    /// How many connections it took, whether a request came on them or not. A
    /// case whose request must not even dial the origin asks after it waited
    /// for the records: the accept loop runs on the case's own runtime, and has
    /// taken any connection made by then.
    fn connections(&self) -> usize {
        self.accepted.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// The request loop of a `ScriptedOrigin`: every request read whole, its head
/// and its body kept, and `answer` written to it.
async fn serve<S>(stream: &mut S, answer: &[u8], kept: &Heard)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let Some(head) = read_until(stream, b"\r\n\r\n").await else {
            return;
        };
        let head = String::from_utf8_lossy(&head).into_owned();
        // a request that expects the interim answer waits for it before it
        // sends its body
        let expects_continue = header_value(&head, "expect")
            .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"));
        if expects_continue
            && stream
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .await
                .is_err()
        {
            return;
        }
        let Some(body) = read_body(stream, &head).await else {
            return;
        };
        kept.lock().unwrap().push((head, body));
        if stream.write_all(answer).await.is_err() {
            return;
        }
    }
}

/// The bytes of `s` up to and including `mark`, or `None` when the far end
/// closes or says nothing for five seconds.
async fn read_until<S>(s: &mut S, mark: &[u8]) -> Option<Vec<u8>>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut got = Vec::new();
    let mut byte = [0u8; 1];
    while !got.ends_with(mark) {
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut byte))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        got.push(byte[0]);
    }
    Some(got)
}

/// The body of the request whose head is `head`: by its `Content-Length`, by
/// the chunks of its `Transfer-Encoding`, or nothing.
async fn read_body<S>(s: &mut S, head: &str) -> Option<Vec<u8>>
where
    S: tokio::io::AsyncRead + Unpin,
{
    if let Some(length) = header_value(head, "content-length") {
        let length: usize = length.trim().parse().ok()?;
        let mut body = vec![0u8; length];
        tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut body))
            .await
            .ok()?
            .ok()?;
        return Some(body);
    }
    let chunked = header_value(head, "transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
    if !chunked {
        return Some(Vec::new());
    }
    let mut body = Vec::new();
    loop {
        let line = read_until(s, b"\r\n").await?;
        let size = std::str::from_utf8(&line[..line.len() - 2]).ok()?;
        let size = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        if size == 0 {
            // the trailers, if the request wrote any, then a blank line
            while read_until(s, b"\r\n").await? != b"\r\n" {}
            return Some(body);
        }
        let mut chunk = vec![0u8; size + 2];
        tokio::time::timeout(Duration::from_secs(5), s.read_exact(&mut chunk))
            .await
            .ok()?
            .ok()?;
        body.extend_from_slice(&chunk[..size]);
    }
}

/// One request written whole — its head and its body in one write, as
/// `CLAUDE.md` asks — and the answer read until the far end closes it.
async fn answer_bytes(proxy: SocketAddr, request: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(request).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut buf)).await;
    buf
}

/// An answer split into its head and its body: the body is read by its
/// framing — the chunks of a `Transfer-Encoding: chunked`, the
/// `Content-Length`, or everything up to the end.
fn answer_parts(raw: &[u8]) -> (String, Vec<u8>) {
    let at = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("an answer head: {:?}", String::from_utf8_lossy(raw)));
    let head = String::from_utf8_lossy(&raw[..at]).into_owned();
    let mut rest = &raw[at + 4..];
    let chunked = header_value(&head, "transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
    if chunked {
        let mut body = Vec::new();
        while let Some(end) = rest.windows(2).position(|window| window == b"\r\n") {
            let size = String::from_utf8_lossy(&rest[..end]).into_owned();
            let size = usize::from_str_radix(size.split(';').next().unwrap().trim(), 16)
                .expect("a chunk size");
            if size == 0 {
                break;
            }
            body.extend_from_slice(&rest[end + 2..end + 2 + size]);
            rest = &rest[end + 2 + size + 2..];
        }
        return (head, body);
    }
    if let Some(length) = header_value(&head, "content-length") {
        let length: usize = length.trim().parse().expect("a Content-Length");
        let body = rest[..length.min(rest.len())].to_vec();
        return (head, body);
    }
    (head, rest.to_vec())
}

/// `data` as one gzip member: the two pages have no compressed example of
/// their own, and the case makes the bytes here, not in the repository
/// (`the phase 4 / M3b plan's P20`). The deflate blocks are stored ones, so
/// the bytes are the case's own and nothing has to be computed but the CRC.
fn gzipped(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    let mut left = data;
    loop {
        let take = left.len().min(0xffff);
        let last = take == left.len();
        out.push(u8::from(last));
        out.extend_from_slice(&(take as u16).to_le_bytes());
        out.extend_from_slice(&(!(take as u16)).to_le_bytes());
        out.extend_from_slice(&left[..take]);
        left = &left[take..];
        if last {
            break;
        }
    }
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// The CRC-32 of `data` that a gzip member ends with.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// The manual's `[Body Rewrite]` page, its first example's request line, word
/// for word (`http/body-rewrite.html`; `the phase 4 / M3b plan's P20`): the
/// body's `value` becomes `abc` before the origin is asked, and the field that
/// says how long the body is follows. The request carries the test origin's
/// port, which the manual's `^http(s)?://example\.com` still matches as a
/// prefix, and `example.com` is pinned to the loopback by `[Host]`.
#[tokio::test]
async fn the_manual_body_rewrite_request_example() {
    let origin = ScriptedOrigin::spawn(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec(),
    )
    .await;
    let port = origin.port();
    let general = format!("force-http-engine-hosts = example.com:{port}");
    let h = harness_with(
        Profile {
            general: &general,
            hosts: "example.com = 127.0.0.1",
            rules: "DOMAIN,example.com,DIRECT",
            sections: "[Body Rewrite]\nhttp-request ^http(s)?://example\\.com value abc",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let body = "key=value&value=1";

    // through the HTTP listener, whose request line is absolute
    let request = format!(
        "POST http://example.com:{port}/ HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (head, _) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");

    // through SOCKS5, on a sniffed connection: the request line names a path
    let mut tunnel = connect_via_socks5(h.socks(), "example.com", port).await;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let answer = exchange_over(&mut tunnel, &request).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");

    let heard = origin.heard();
    assert_eq!(heard.len(), 2, "{heard:?}");
    for (head, sent) in &heard {
        assert_eq!(sent, b"key=abc&abc=1", "the rewritten body: {head}");
        assert_eq!(
            header_value(head, "content-length").as_deref(),
            Some("13"),
            "the length follows the rewritten body: {head}"
        );
    }
    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
        assert_eq!(
            record.notes,
            ["Body Rewrite: request by ^http(s)?://example\\.com"],
            "{record:?}"
        );
    }
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.body, counts.failed), (2, 0), "{counts:?}");
}

/// The manual's `[Body Rewrite]` page, its first example's response line, word
/// for word: the answer's `documents` becomes `Surge`, and so does the length
/// that frames it. The answer's `Content-Type` and the port in the request are
/// not the manual's; the pattern still matches the URL as a prefix.
#[tokio::test]
async fn the_manual_body_rewrite_response_example() {
    let answer = b"<p>documents</p>".to_vec();
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        answer.len()
    )
    .into_bytes();
    raw.extend_from_slice(&answer);
    let origin = ScriptedOrigin::spawn(raw).await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "example.com = 127.0.0.1",
            rules: "DOMAIN,example.com,DIRECT",
            sections: "[Body Rewrite]\nhttp-response ^http(s)?://example\\.com documents Surge",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    let request = format!(
        "GET http://example.com:{port}/ HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Connection: close\r\n\r\n"
    );
    let (head, body) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"<p>Surge</p>", "{head}");
    assert_eq!(
        header_value(&head, "content-length").as_deref(),
        Some("12"),
        "the length follows the rewritten body: {head}"
    );
    assert_eq!(
        header_value(&head, "content-type").as_deref(),
        Some("text/html"),
        "a field the rule did not touch stays: {head}"
    );
    let records = the_records(&h, 1).await;
    assert_eq!(
        records[0].notes,
        ["Body Rewrite: response by ^http(s)?://example\\.com"],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.body, counts.failed), (1, 0), "{counts:?}");
}

/// The manual's `[Body Rewrite]` page, its two consecutive-replacement lines,
/// one engine each: the pairs of a line act one after another, and the second
/// line's third pair does too. The manual's lines end their host with `/`,
/// which the test origin's port sits in front of here: the pattern carries
/// `:\d+` so that it still matches (`the phase 4 / M3b plan's P20`).
#[tokio::test]
async fn the_manual_body_rewrite_pairs_example() {
    for (line, before, after) in [
        (
            "http-response ^https?://example\\.com:\\d+/ regex1 replacement1 regex2 replacement2",
            "regex1 regex2",
            "replacement1 replacement2",
        ),
        (
            "http-response ^https?://example\\.com:\\d+/ regex1 replacement1 regex2 replacement2 \
             regex3 replacement3",
            "regex1 regex2 regex3",
            "replacement1 replacement2 replacement3",
        ),
    ] {
        let raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{before}",
            before.len()
        )
        .into_bytes();
        let origin = ScriptedOrigin::spawn(raw).await;
        let port = origin.port();
        let sections = format!("[Body Rewrite]\n{line}");
        let h = harness_with(
            Profile {
                hosts: "example.com = 127.0.0.1",
                rules: "DOMAIN,example.com,DIRECT",
                sections: &sections,
                ..Profile::default()
            },
            shared_on(NetworkFacts::default()),
        )
        .await;
        let request = format!(
            "GET http://example.com:{port}/ HTTP/1.1\r\nHost: example.com:{port}\r\n\
             Connection: close\r\n\r\n"
        );
        let (head, body) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(body, after.as_bytes(), "{line}: {head}");
        let records = the_records(&h, 1).await;
        assert_eq!(
            records[0].notes,
            [format!(
                "Body Rewrite: response by {}",
                line.split_once(' ').unwrap().1.split_once(' ').unwrap().0
            )],
            "{records:?}"
        );
    }
}

/// The manual's `[Body Rewrite]` page: "The replacement supports capture group
/// references such as `$1`". The manual gives no line for it, so the line is
/// written here; like the page's own lines it ends its host with `/`, and the
/// test origin's port sits in front of it here (`:\d+`), as above.
#[tokio::test]
async fn the_manual_body_rewrite_capture_example() {
    let body = "a@b c@d";
    let origin = ScriptedOrigin::spawn(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
    .await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "example.com = 127.0.0.1",
            rules: "DOMAIN,example.com,DIRECT",
            sections: r"[Body Rewrite]
http-response ^http://example\.com:\d+/ (\w+)@(\w+) $2@$1",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let request = format!(
        "GET http://example.com:{port}/ HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Connection: close\r\n\r\n"
    );
    let (head, answer) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(answer, b"b@a d@c", "{head}");
}

/// The manual's `[Body Rewrite]` page: "The `^` and `$` anchors match at the
/// beginning and end of each line of the body". The manual gives no line for
/// it, so the line is written here; the body has a `\r\n` line, a `\n` line and
/// a line of two `x`, of which the anchors catch the first two only.
#[tokio::test]
async fn the_manual_body_rewrite_anchors_example() {
    let body: &[u8] = b"x\r\nx\nxx";
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    let origin = ScriptedOrigin::spawn(raw).await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "example.com = 127.0.0.1",
            rules: "DOMAIN,example.com,DIRECT",
            sections: r"[Body Rewrite]
http-response ^http://example\.com:\d+/ ^x$ y",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let request = format!(
        "GET http://example.com:{port}/ HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Connection: close\r\n\r\n"
    );
    let (head, answer) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(answer, b"y\r\ny\nxx", "{head}");
}

/// The manual's `[Body Rewrite]` page: "Even if the original request does not
/// contain a body, new content may still be generated through body rewrite,
/// such as using the `^$` expression". The manual gives no line for it, so the
/// line is written here; `:\d+` is the test origin's port, as in the pages'
/// own lines above.
#[tokio::test]
async fn the_manual_body_rewrite_empty_body_example() {
    let origin = ScriptedOrigin::spawn(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec(),
    )
    .await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "example.com = 127.0.0.1",
            rules: "DOMAIN,example.com,DIRECT",
            sections: r"[Body Rewrite]
http-request ^http://example\.com:\d+/ ^$ generated",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    // a `GET` with no body at all
    let request = format!(
        "GET http://example.com:{port}/ HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Connection: close\r\n\r\n"
    );
    let (head, _) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let heard = origin.heard();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].1, b"generated", "{}", heard[0].0);
    assert_eq!(
        header_value(&heard[0].0, "content-length").as_deref(),
        Some("9"),
        "the body's length is written: {}",
        heard[0].0
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.body, counts.failed), (1, 0), "{counts:?}");
}

/// The manual's `[Body Rewrite]` page: "The body must be valid UTF-8 text for
/// regex-based rewriting. If it is not, the rule is skipped and a note is added
/// to the request journal". The case's answer is the pattern's first word plus
/// bytes that start no UTF-8 sequence: the rule would hit it if the body were
/// text, so what this shows is the rule skipping it.
#[tokio::test]
async fn the_manual_utf8_requirement() {
    let body: &[u8] = b"documents [\xff][\xfe] ";
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    let origin = ScriptedOrigin::spawn(raw).await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "example.com = 127.0.0.1",
            rules: "DOMAIN,example.com,DIRECT",
            sections: "[Body Rewrite]\nhttp-response ^http(s)?://example\\.com documents Surge",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let request = format!(
        "GET http://example.com:{port}/ HTTP/1.1\r\nHost: example.com:{port}\r\n\
         Connection: close\r\n\r\n"
    );
    let (head, answer) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(answer, body, "the bytes are left as they were: {head}");
    assert_eq!(
        header_value(&head, "content-encoding"),
        None,
        "nothing was decoded, so nothing changed: {head}"
    );
    let records = the_records(&h, 1).await;
    assert_eq!(
        records[0].notes,
        [
            "Body Rewrite skipped: ^http(s)?://example\\.com: the body is not valid UTF-8"
                .to_string()
        ],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.body, counts.skipped), (0, 1), "{counts:?}");
}

/// The manual's `[Map Local]` page, its four lines word for word
/// (`http/map-local.html`; `the phase 4 / M3b plan's P20`): the status, the
/// data and the `Content-Type` each line names, the file read from the
/// profile's own directory, and the headers the third line writes. No request
/// here needs a port: the lines answer it themselves, and the policy they
/// choose is a guard, so the case also shows that nothing was dialled.
#[tokio::test]
async fn the_manual_map_local_examples() {
    let file = "{\"ok\":true}";
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let h = harness_with(
        Profile {
            proxies: &proxies,
            hosts: "surgetest.com = 127.0.0.1",
            rules: "DOMAIN,surgetest.com,guard",
            sections: "[Map Local]\n\
                       ^http://surgetest\\.com/json data-type=text data=\"{}\" status-code=500\n\
                       ^http://surgetest\\.com/gif data-type=tiny-gif status-code=200\n\
                       ^http://surgetest\\.com/file data-type=file data=\"data/map-local.json\" \
                       header=\"a:b|foo:bar\"\n\
                       ^http://surgetest\\.com/base64 data=\"dGVzdA==\" data-type=base64",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    // the manual's relative path is relative to the profile's directory
    std::fs::create_dir_all(h.dir.path().join("data")).unwrap();
    std::fs::write(h.dir.path().join("data/map-local.json"), file).unwrap();

    let tags = ["/json", "/gif", "/file", "/base64"];
    let mut answers = Vec::new();
    for path in tags {
        let request = format!(
            "GET http://surgetest.com{path} HTTP/1.1\r\nHost: surgetest.com\r\n\
             Connection: close\r\n\r\n"
        );
        answers.push(answer_parts(
            &answer_bytes(h.http(), request.as_bytes()).await,
        ));
    }

    let (head, body) = &answers[0];
    assert!(head.starts_with("HTTP/1.1 500"), "{head}");
    assert_eq!(body, b"{}", "{head}");
    assert_eq!(
        header_value(head, "content-type").as_deref(),
        Some("text/plain"),
        "a `text` answer is text/plain: {head}"
    );

    let (head, body) = &answers[1];
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body.len(), 43, "a 1x1 GIF is 43 bytes: {head}");
    assert!(body.starts_with(b"GIF89a"), "{head}");
    assert_eq!(
        header_value(head, "content-type").as_deref(),
        Some("image/gif"),
        "{head}"
    );

    let (head, body) = &answers[2];
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, file.as_bytes(), "{head}");
    assert_eq!(header_value(head, "a").as_deref(), Some("b"), "{head}");
    assert_eq!(header_value(head, "foo").as_deref(), Some("bar"), "{head}");
    assert_eq!(
        header_value(head, "content-type").as_deref(),
        Some("application/json"),
        "the file's extension says what it is: {head}"
    );

    let (head, body) = &answers[3];
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"test", "{head}");
    assert_eq!(
        header_value(head, "content-type").as_deref(),
        Some("application/octet-stream"),
        "{head}"
    );

    assert_eq!(
        guard.connections(),
        0,
        "every line answered its request without an upstream"
    );
    let records = the_records(&h, 4).await;
    assert_eq!(records.len(), 4, "{records:?}");
    let kinds = ["text", "tiny-gif", "file", "base64"];
    for (record, (path, kind)) in records.iter().zip(tags.iter().zip(kinds)) {
        assert_eq!(
            record.notes,
            [format!(
                "Map Local: {kind} by ^http://surgetest\\.com{path}"
            )],
            "{record:?}"
        );
    }
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.failed), (4, 0), "{counts:?}");
}

/// The manual's `[Map Local]` page: "You can use `data-type=text data=\"\"` to
/// return an empty result". The page gives no whole line for it, so the line is
/// written here; its answer has a length and no body.
#[tokio::test]
async fn the_manual_map_local_empty_result() {
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let h = harness_with(
        Profile {
            proxies: &proxies,
            hosts: "surgetest.com = 127.0.0.1",
            rules: "DOMAIN,surgetest.com,guard",
            sections: "[Map Local]\n^http://surgetest\\.com/empty data-type=text data=\"\"",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let request = "GET http://surgetest.com/empty HTTP/1.1\r\nHost: surgetest.com\r\n\
                   Connection: close\r\n\r\n";
    let (head, body) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(
        header_value(&head, "content-length").as_deref(),
        Some("0"),
        "an empty answer still says how long it is: {head}"
    );
    assert!(body.is_empty(), "{head}");
    assert_eq!(
        header_value(&head, "content-type").as_deref(),
        Some("text/plain"),
        "{head}"
    );
    assert_eq!(guard.connections(), 0);
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.failed), (1, 0), "{counts:?}");
}

/// The manual's `[Body Rewrite]` page, "Limitations": "A request body may be
/// buffered up to 32 MB; if it exceeds the limit, the connection is dropped".
/// A body of exactly the limit is rewritten; one byte more and the connection
/// is closed before a byte of the body is read and before the origin is dialled
/// (`phase 4 M3 design 9.3`, `the phase 4 / M3b plan's P14`).
#[tokio::test]
async fn the_request_limit_is_thirty_two_mib() {
    let origin = ScriptedOrigin::spawn(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec(),
    )
    .await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "big.test = 127.0.0.1",
            rules: "DOMAIN,big.test,DIRECT",
            sections: r"[Body Rewrite]
http-request ^http://big\.test:\d+/ ^a b",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let limit = 32 * 1024 * 1024;

    // exactly the limit: the body is rewritten, and the length it goes out with
    // is the one it came in with. The head and the body go in one write, as
    // `CLAUDE.md` asks of every request a case writes by hand.
    let mut request = format!(
        "POST http://big.test:{port}/ HTTP/1.1\r\nHost: big.test:{port}\r\n\
         Content-Length: {limit}\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    request.resize(request.len() + limit, b'a');
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(&request).await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(60), s.read_to_end(&mut buf)).await;
    let (answer, body) = answer_parts(&buf);
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(body, b"ok", "{answer}");
    let heard = origin.heard();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(heard[0].1.len(), limit, "the whole body reached the origin");
    assert_eq!(
        heard[0].1[0], b'b',
        "the `a` at the start of the line became `b`"
    );
    assert!(
        heard[0].1[1..].iter().all(|&byte| byte == b'a'),
        "nothing else changed"
    );
    assert_eq!(
        header_value(&heard[0].0, "content-length").as_deref(),
        Some("33554432"),
        "{}",
        heard[0].0
    );

    // one byte over: the head alone is enough, and the connection is closed
    let over = format!(
        "POST http://big.test:{port}/ HTTP/1.1\r\nHost: big.test:{port}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        limit + 1
    );
    let answer = answer_bytes(h.http(), over.as_bytes()).await;
    assert!(
        answer.is_empty(),
        "the connection is closed without an answer: {}",
        String::from_utf8_lossy(&answer)
    );
    assert_eq!(origin.heard().len(), 1, "the origin was not asked again");
    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[1].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(
        records[1].error.as_deref(),
        Some("Body Rewrite: the request body is over 32 MiB"),
        "{records:?}"
    );
    assert_eq!(
        records[1].notes,
        [
            "Body Rewrite failed: ^http://big\\.test:\\d+/: the request body is over 32 MiB"
                .to_string()
        ],
        "{records:?}"
    );
    // the record is in, so the session is over: a dial it made for the request
    // over the limit would have been taken by now
    assert_eq!(
        origin.connections(),
        1,
        "the origin was dialled for the first request only"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (counts.body, counts.skipped, counts.failed),
        (1, 0, 1),
        "{counts:?}"
    );
}

/// The manual's `[Body Rewrite]` page, "Limitations": "A response body may be
/// modified only up to 1 MB on iOS and 10 MB on macOS. If the response exceeds
/// the limit, Surge gives up the modification and streams the response through
/// unmodified".
///
/// A body of exactly the limit is rewritten; one byte more and every byte of it
/// reaches the client as it came, with the rule's note saying why. The case
/// asserts the bytes, not the framing: an answer that stops growing past the
/// limit is handed over with the read-ahead the case already read
/// (`the phase 4 / M3b plan's execution record row 64`), and the framing of a
/// `ReadAhead` is each hop's own.
#[tokio::test]
async fn the_response_limit_is_ten_mib() {
    let limit = 10 * 1024 * 1024;
    for (size, rewritten) in [(limit, true), (limit + 1, false)] {
        let body = vec![b'a'; size];
        let mut raw = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{size:x}\r\n"
        )
        .into_bytes();
        raw.extend_from_slice(&body);
        raw.extend_from_slice(b"\r\n0\r\n\r\n");
        let origin = ScriptedOrigin::spawn(raw).await;
        let port = origin.port();
        let h = harness_with(
            Profile {
                hosts: "b.test = 127.0.0.1",
                rules: "DOMAIN,b.test,DIRECT",
                sections: r"[Body Rewrite]
http-response ^http://b\.test:\d+/ ^a b",
                ..Profile::default()
            },
            shared_on(NetworkFacts::default()),
        )
        .await;
        let request = format!(
            "GET http://b.test:{port}/ HTTP/1.1\r\nHost: b.test:{port}\r\n\
             Connection: close\r\n\r\n"
        );
        let (head, answer) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        assert_eq!(answer.len(), size, "every byte reached the client: {head}");
        let first = if rewritten { b'b' } else { b'a' };
        assert_eq!(answer[0], first, "{size}: {head}");
        assert!(
            answer[1..].iter().all(|&byte| byte == b'a'),
            "nothing else changed: {head}"
        );
        let records = the_records(&h, 1).await;
        if rewritten {
            assert_eq!(
                records[0].notes,
                ["Body Rewrite: response by ^http://b\\.test:\\d+/".to_string()],
                "{records:?}"
            );
            let counts = h.engine.http_counts().rewrite;
            assert_eq!((counts.body, counts.skipped), (1, 0), "{counts:?}");
        } else {
            assert_eq!(
                records[0].notes,
                [
                    "Body Rewrite skipped: ^http://b\\.test:\\d+/: the body is over 10 MiB"
                        .to_string()
                ],
                "{records:?}"
            );
            let counts = h.engine.http_counts().rewrite;
            assert_eq!((counts.body, counts.skipped), (0, 1), "{counts:?}");
        }
    }
}

/// The manual's `[Body Rewrite]` page, "Limitations": "Request body rewrite is
/// not supported when the request uses `Transfer-Encoding: chunked` or
/// `Expect: 100-continue`. Surge logs a warning and forwards the request
/// unmodified". Two chunked requests over one rule: both reach the origin as
/// they came, and the log has one WARN for the rule and a DEBUG for each
/// request (`phase 4 M3 design 14`, `the phase 4 / M3b plan's P18`).
#[tokio::test]
async fn a_chunked_request_is_forwarded_with_one_warning() {
    let log = LogCapture::start();
    let origin = ScriptedOrigin::spawn(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec(),
    )
    .await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "b.test = 127.0.0.1",
            rules: "DOMAIN,b.test,DIRECT",
            sections: r"[Body Rewrite]
http-request ^http://b\.test:\d+/ value abc",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;

    for _ in 0..2 {
        let request = format!(
            "POST http://b.test:{port}/ HTTP/1.1\r\nHost: b.test:{port}\r\n\
             Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
             5\r\nvalue\r\n0\r\n\r\n"
        );
        let (head, _) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    }

    let heard = origin.heard();
    assert_eq!(heard.len(), 2, "{heard:?}");
    for (head, sent) in &heard {
        assert_eq!(sent, b"value", "the body reached the origin unchanged");
        assert!(
            header_value(head, "transfer-encoding")
                .is_some_and(|value| value.to_ascii_lowercase().contains("chunked")),
            "it is still the chunked body it came as: {head}"
        );
    }

    let lines = log.lines();
    let said: Vec<&String> = lines
        .iter()
        .filter(|line| line.contains("request body not rewritten:"))
        .collect();
    let warns: Vec<&&String> = said
        .iter()
        .filter(|line| line.starts_with("WARN"))
        .collect();
    assert_eq!(warns.len(), 1, "one WARN an hour for the rule: {said:?}");
    assert!(
        warns[0].contains("request body not rewritten: the request body is chunked")
            && warns[0].contains(r#"pattern="^http://b\\.test:\\d+/""#)
            && !warns[0].contains("host="),
        "the WARN names the rule and no host: {}",
        warns[0]
    );
    let debugs: Vec<&&String> = said
        .iter()
        .filter(|line| line.starts_with("DEBUG"))
        .collect();
    assert_eq!(debugs.len(), 2, "one DEBUG for each request: {said:?}");
    for line in debugs {
        assert!(
            line.contains(r#"host="b.test""#),
            "the DEBUG names the host: {line}"
        );
        assert!(
            line.contains(r#"pattern="^http://b\\.test:\\d+/""#),
            "and the rule: {line}"
        );
    }

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    for record in &records {
        assert_eq!(
            record.notes,
            [
                "Body Rewrite skipped: ^http://b\\.test:\\d+/: the request body is chunked"
                    .to_string()
            ],
            "{record:?}"
        );
    }
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.body, counts.skipped), (0, 2), "{counts:?}");
}

/// The manual's `[Body Rewrite]` page, "Limitations": a request that expects
/// `100-continue` is forwarded unmodified, with a note that says why
/// (`phase 4 M3 design 9.2`, `the phase 4 / M3b plan's P10`).
#[tokio::test]
async fn an_expect_request_is_forwarded_unmodified() {
    let origin = ScriptedOrigin::spawn(
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec(),
    )
    .await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "b.test = 127.0.0.1",
            rules: "DOMAIN,b.test,DIRECT",
            sections: r"[Body Rewrite]
http-request ^http://b\.test:\d+/ value abc",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let request = format!(
        "POST http://b.test:{port}/ HTTP/1.1\r\nHost: b.test:{port}\r\n\
         Content-Length: 5\r\nExpect: 100-continue\r\nConnection: close\r\n\r\nvalue"
    );
    let mut raw = answer_bytes(h.http(), request.as_bytes()).await;
    // hyper's server writes the interim answer when the body is read
    if raw.starts_with(b"HTTP/1.1 100") {
        let at = raw
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("an interim answer");
        raw.drain(..at + 4);
    }
    let (head, answer) = answer_parts(&raw);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(answer, b"ok", "{head}");

    let heard = origin.heard();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(
        heard[0].1, b"value",
        "the body reached the origin unchanged"
    );
    assert_eq!(
        header_value(&heard[0].0, "expect").as_deref(),
        Some("100-continue"),
        "it is still the request that expected one: {}",
        heard[0].0
    );
    let records = the_records(&h, 1).await;
    assert_eq!(
        records[0].notes,
        [
            "Body Rewrite skipped: ^http://b\\.test:\\d+/: the request expects 100-continue"
                .to_string()
        ],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.body, counts.skipped), (0, 1), "{counts:?}");
}

/// A body that says nothing about its length, as a request an HTTP/2 client
/// sends without naming one has (`phase 4 M3 design 9.2`, `the phase 4 / M3b
/// plan's P10`): hyper's client writes no `content-length` for a body whose
/// size it cannot tell in advance, and this one tells nothing.
struct Unmeasured(Bytes);

/// A body that says in its head how long it is and then sends one byte: the
/// request over the limit sends its head and no body at all (`the phase 4 /
/// M3b plan's task 7`), and hyper's client writes the length it was told down
/// as the request's `content-length`. One byte rather than nothing is what the
/// client's own writing needs: a body that never ends would leave the stream
/// it belongs to open for good. A body of zero is a request with none.
struct Promised {
    size: u64,
    sent: bool,
}

impl hyper::body::Body for Promised {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        if this.size == 0 || this.sent {
            return std::task::Poll::Ready(None);
        }
        this.sent = true;
        std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(Bytes::from_static(b"x")))))
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        hyper::body::SizeHint::with_exact(self.size)
    }

    fn is_end_stream(&self) -> bool {
        // the default is `false`, and a client that says it may still send a
        // body would leave the engine waiting for a length it has none of
        self.size == 0 || self.sent
    }
}

impl hyper::body::Body for Unmeasured {
    type Data = Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        let this = self.get_mut();
        let data = std::mem::take(&mut this.0);
        if data.is_empty() {
            std::task::Poll::Ready(None)
        } else {
            std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(data))))
        }
    }
}

/// A request of no length on a decrypted HTTP/2 connection cannot be buffered,
/// so it reaches the origin as it came, with a note that says why (`phase 4 M3
/// design 9.2`, `the phase 4 / M3b plan's P10`). The origin speaks HTTP/1.1 so
/// that the case can read what arrived.
#[tokio::test]
async fn an_http2_request_without_a_length_is_forwarded_unmodified() {
    let fixture = TlsFixture::new(&["origin.test"]);
    let origin = ScriptedOrigin::spawn_tls(
        fixture.acceptor_with_alpn(false, &[b"http/1.1"]),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec(),
    )
    .await;
    let port = origin.port();
    let mitm = mitm_ca();
    let sections = format!(
        "{}\nh2 = true\n\
         [Body Rewrite]\n\
         http-request ^https://origin\\.test:\\d+/ value abc",
        mitm_section(&format!("origin.test:{port}"), &mitm.ca_p12_base64("pw"))
    );
    let h = harness_mitm(
        Profile {
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;

    let (settled, tls) = decrypted(&h, mitm.roots(), port, &[b"h2"]).await;
    assert_eq!(settled.as_deref(), Some("h2"), "{settled:?}");
    // the connection is made here, not by `h2_over`: that one carries a body
    // whose length hyper can tell, and this case needs one it cannot
    let (sender, conn): (http2::SendRequest<Unmeasured>, _) =
        http2::Builder::new(TokioExecutor::new())
            .handshake(TokioIo::new(tls))
            .await
            .expect("an HTTP/2 handshake over the decrypted stream");
    // the connection moves only while its own task runs (`h2_over`)
    let conn = tokio::spawn(conn);
    let mut sender = sender;
    // the authority carries the test origin's port: it is what makes the URL of
    // the request here, and the engine dials what it names
    let request = Request::builder()
        .method(Method::POST)
        .uri(format!("https://origin.test:{port}/up"))
        .body(Unmeasured(Bytes::from_static(b"value")))
        .expect("a request");
    let response = tokio::time::timeout(Duration::from_secs(5), sender.send_request(request))
        .await
        .expect("the answer comes within the bound")
        .expect("the request goes through");
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::timeout(Duration::from_secs(5), response.into_body().collect())
        .await
        .expect("the body comes within the bound")
        .expect("a response body");
    drop(sender);
    tokio::time::timeout(Duration::from_secs(5), conn)
        .await
        .expect("the connection ends within the bound")
        .expect("the connection's task did not panic")
        .expect("the client's side of the connection ends cleanly");

    let heard = origin.heard();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert_eq!(
        heard[0].1, b"value",
        "the body reached the origin unchanged"
    );
    assert!(
        heard[0].0.starts_with("POST /up HTTP/1.1"),
        "{}",
        heard[0].0
    );
    assert_eq!(
        header_value(&heard[0].0, "content-length"),
        None,
        "it went out with no length, as it came: {}",
        heard[0].0
    );
    let records = the_records(&h, 1).await;
    assert_eq!(
        records[0].notes,
        [
            "Body Rewrite skipped: ^https://origin\\.test:\\d+/: the request body has no length"
                .to_string()
        ],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.body, counts.skipped), (0, 1), "{counts:?}");
}

/// A request whose head says it is over the limit is closed on HTTP/1.1 and
/// its stream is reset on HTTP/2, and the connection goes on serving the
/// requests after it (`phase 4 M3 design 9.3`, `the phase 4 / M3b plan's
/// P14`): the manual's "the connection is dropped" is the HTTP/1.1 shape of
/// it, and `M2-D11`'s "reset this stream" is the HTTP/2 one.
#[tokio::test]
async fn an_http2_request_over_the_limit_resets_its_stream() {
    let fixture = TlsFixture::new(&["origin.test"]);
    let origin = ScriptedOrigin::spawn_tls(
        fixture.acceptor_with_alpn(false, &[b"http/1.1"]),
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_vec(),
    )
    .await;
    let port = origin.port();
    let mitm = mitm_ca();
    let sections = format!(
        "{}\nh2 = true\n\
         [Body Rewrite]\n\
         http-request ^https://origin\\.test:\\d+/ ^a b",
        mitm_section(&format!("origin.test:{port}"), &mitm.ca_p12_base64("pw"))
    );
    let h = harness_mitm(
        Profile {
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;

    let (settled, tls) = decrypted(&h, mitm.roots(), port, &[b"h2"]).await;
    assert_eq!(settled.as_deref(), Some("h2"), "{settled:?}");
    // the connection is made here, not by `h2_over`: the first request's head
    // promises a body it never sends, which needs a body type of its own
    let (sender, conn): (http2::SendRequest<Promised>, _) =
        http2::Builder::new(TokioExecutor::new())
            .handshake(TokioIo::new(tls))
            .await
            .expect("an HTTP/2 handshake over the decrypted stream");
    let conn = tokio::spawn(conn);
    let mut sender = sender;

    // the head alone promises 32 MiB and one byte, and no body follows
    let over = Request::builder()
        .method(Method::POST)
        .uri(format!("https://origin.test:{port}/big"))
        .body(Promised {
            size: 32 * 1024 * 1024 + 1,
            sent: false,
        })
        .expect("a request");
    // the stream is reset before an answer could come, and within the bound
    let answer = tokio::time::timeout(Duration::from_secs(5), sender.send_request(over))
        .await
        .expect("the stream is reset within the bound");
    assert!(
        answer.is_err(),
        "the stream is reset, not answered: {:?}",
        answer.map(|response| response.status())
    );

    // the same connection still answers the request after it
    let next = Request::builder()
        .method(Method::GET)
        .uri(format!("https://origin.test:{port}/next"))
        .body(Promised {
            size: 0,
            sent: false,
        })
        .expect("a request");
    let response = tokio::time::timeout(Duration::from_secs(5), sender.send_request(next))
        .await
        .expect("the answer comes within the bound")
        .expect("the request after the reset goes through");
    assert_eq!(response.status(), StatusCode::OK);
    let body = tokio::time::timeout(Duration::from_secs(5), response.into_body().collect())
        .await
        .expect("the body comes within the bound")
        .expect("a response body")
        .to_bytes();
    assert_eq!(&body[..], b"ok");
    drop(sender);
    tokio::time::timeout(Duration::from_secs(5), conn)
        .await
        .expect("the connection ends within the bound")
        .expect("the connection's task did not panic")
        .expect("the client's side of the connection ends cleanly");

    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    assert_eq!(records[1].status, RecordStatus::Completed, "{records:?}");
    // the request over the limit was never dialled: the origin heard the one
    // after it alone, on the one connection it took
    let heard = origin.heard();
    assert_eq!(heard.len(), 1, "{heard:?}");
    assert!(
        heard[0].0.starts_with("GET /next HTTP/1.1"),
        "{}",
        heard[0].0
    );
    assert_eq!(origin.connections(), 1, "{heard:?}");
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (counts.body, counts.skipped, counts.failed),
        (0, 0, 1),
        "{counts:?}"
    );
}

/// The manual's `[Body Rewrite]` page: "the body is decompressed first if
/// necessary, and `Content-Length` is recalculated". A gzip answer a rule
/// changes goes out plain, with no `Content-Encoding`; one the rule matches
/// but leaves as it was goes out as the compressed bytes it came as, with the
/// field that says so (`phase 4 M3 design 9.4`, `9.6`; the phase 4 / M3b
/// plan's P11, P13).
#[tokio::test]
async fn a_rewritten_gzip_response_and_an_unchanged_one() {
    for (plain, rewritten) in [("<p>documents</p>", true), ("<p>other</p>", false)] {
        let compressed = gzipped(plain.as_bytes());
        let mut raw = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Encoding: gzip\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            compressed.len()
        )
        .into_bytes();
        raw.extend_from_slice(&compressed);
        let origin = ScriptedOrigin::spawn(raw).await;
        let port = origin.port();
        let h = harness_with(
            Profile {
                hosts: "b.test = 127.0.0.1",
                rules: "DOMAIN,b.test,DIRECT",
                sections: r"[Body Rewrite]
http-response ^http://b\.test:\d+/ documents Surge",
                ..Profile::default()
            },
            shared_on(NetworkFacts::default()),
        )
        .await;
        let request = format!(
            "GET http://b.test:{port}/ HTTP/1.1\r\nHost: b.test:{port}\r\n\
             Connection: close\r\n\r\n"
        );
        let (head, body) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let records = the_records(&h, 1).await;
        if rewritten {
            assert_eq!(body, b"<p>Surge</p>", "the body went out as it became");
            assert_eq!(
                header_value(&head, "content-encoding"),
                None,
                "a rewritten body is sent plain: {head}"
            );
            assert_eq!(
                header_value(&head, "content-length").as_deref(),
                Some("12"),
                "{head}"
            );
            assert_eq!(
                records[0].notes,
                ["Body Rewrite: response by ^http://b\\.test:\\d+/".to_string()],
                "{records:?}"
            );
        } else {
            assert_eq!(body, compressed, "the compressed bytes, as they came");
            assert_eq!(
                header_value(&head, "content-encoding").as_deref(),
                Some("gzip"),
                "a body left as it was keeps its field: {head}"
            );
            assert!(records[0].notes.is_empty(), "{records:?}");
        }
        let counts = h.engine.http_counts().rewrite;
        assert_eq!(
            (counts.body, counts.skipped, counts.failed),
            (u64::from(rewritten), 0, 0),
            "{counts:?}"
        );
    }
}

/// A body compressed with something rurge does not decode is left as it came,
/// with a note that names the coding and never a byte of the body (`phase 4 M3
/// design 9.4`, `the phase 4 / M3b plan's P11`): M3c's `br` and `zstd` are not
/// this plan's, and the page M3-D8 wrote down is the one this case holds to.
#[tokio::test]
async fn a_brotli_response_is_not_rewritten() {
    let body: &[u8] = b"\x1b\x2b\xff\x02xyzw";
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Encoding: br\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    let origin = ScriptedOrigin::spawn(raw).await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "b.test = 127.0.0.1",
            rules: "DOMAIN,b.test,DIRECT",
            sections: r"[Body Rewrite]
http-response ^http://b\.test:\d+/ xyzw Surge",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let request = format!(
        "GET http://b.test:{port}/ HTTP/1.1\r\nHost: b.test:{port}\r\n\
         Connection: close\r\n\r\n"
    );
    let (head, answer) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(answer, body, "the bytes are left as they came: {head}");
    assert_eq!(
        header_value(&head, "content-encoding").as_deref(),
        Some("br"),
        "{head}"
    );
    let records = the_records(&h, 1).await;
    assert_eq!(
        records[0].notes,
        [
            "Body Rewrite skipped: ^http://b\\.test:\\d+/: content-encoding br is not supported"
                .to_string()
        ],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.body, counts.skipped), (0, 1), "{counts:?}");
}

/// The manual's `[Map Local]` page: the `file` type's data is "the path to the
/// data file"; a file's answer is read when the request arrives, so the next
/// request reads what the file says now, without the profile being loaded
/// again (`phase 4 M3 design 10.2`; the phase 4 / M3b plan's P15).
#[tokio::test]
async fn the_file_is_read_on_every_request() {
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let h = harness_with(
        Profile {
            proxies: &proxies,
            hosts: "surgetest.com = 127.0.0.1",
            rules: "DOMAIN,surgetest.com,guard",
            sections: "[Map Local]\n\
                       ^http://surgetest\\.com/file data-type=file data=\"data/map-local.json\"",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let file = h.dir.path().join("data/map-local.json");
    std::fs::create_dir_all(h.dir.path().join("data")).unwrap();
    let request = "GET http://surgetest.com/file HTTP/1.1\r\nHost: surgetest.com\r\n\
                   Connection: close\r\n\r\n";

    std::fs::write(&file, "{\"n\":1}").unwrap();
    let (head, body) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"{\"n\":1}", "{head}");

    std::fs::write(&file, "{\"n\":22}").unwrap();
    let (head, body) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"{\"n\":22}", "the second request read the new file");

    assert_eq!(guard.connections(), 0);
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.failed), (2, 0), "{counts:?}");
}

/// A `[Map Local]` file that is not there answers 502, with the path in the
/// record and no byte of it in the answer (`phase 4 M3 design 10.3`, `8.5`;
/// the phase 4 / M3b plan's P15, P18): the policy the line chose is never
/// dialled either.
#[tokio::test]
async fn a_missing_map_local_file_answers_502() {
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let h = harness_with(
        Profile {
            proxies: &proxies,
            hosts: "surgetest.com = 127.0.0.1",
            rules: "DOMAIN,surgetest.com,guard",
            sections: "[Map Local]\n\
                       ^http://surgetest\\.com/file data-type=file data=\"data/missing.json\"",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let request = "GET http://surgetest.com/file HTTP/1.1\r\nHost: surgetest.com\r\n\
                   Connection: close\r\n\r\n";
    let (head, _) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    assert_eq!(
        guard.connections(),
        0,
        "a Map Local failure does not dial the policy"
    );
    let records = the_records(&h, 1).await;
    assert_eq!(records[0].status, RecordStatus::Failed, "{records:?}");
    let path = h.dir.path().join("data/missing.json");
    assert_eq!(
        records[0].error.as_deref(),
        Some(format!("Map Local: cannot read {}", path.display()).as_str()),
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.failed), (0, 1), "{counts:?}");
}

/// A request whose policy is one of REJECT's is not mapped either: `Map Local`
/// is a step of the engine's HTTP path, and a rejected request never reaches
/// it (`phase 4 M3 design 8.4`, `10.2`; `M3-D27`).
#[tokio::test]
async fn a_rejected_request_is_not_mapped() {
    let guard = Guard::new();
    let proxies = guard.policy("guard");
    let h = harness_with(
        Profile {
            general: "show-error-page-for-reject = true",
            proxies: &proxies,
            hosts: "surgetest.com = 127.0.0.1\nother.test = 127.0.0.1",
            rules: "DOMAIN,surgetest.com,REJECT\nDOMAIN,other.test,guard",
            sections: "[Map Local]\n\
                       ^http://surgetest\\.com/ json2json data-type=text data=\"{}\"",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let answer = plain_get(h.http(), "http://surgetest.com/json2json", "surgetest.com").await;
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    assert!(
        !answer.ends_with("{}"),
        "the answer is the reject page, not the line's data: {answer}"
    );
    assert_eq!(guard.connections(), 0);
    let records = the_records(&h, 1).await;
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(
        records[0].status,
        RecordStatus::Rejected("REJECT".into()),
        "{records:?}"
    );
    assert!(records[0].notes.is_empty(), "{:?}", records[0].notes);
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (
            counts.url,
            counts.header,
            counts.body,
            counts.map_local,
            counts.skipped,
            counts.failed
        ),
        (0, 0, 0, 0, 0, 0),
        "{counts:?}"
    );
}

/// Both steps act on a decrypted `https://` request: one line answers one
/// request without an upstream, and a body rule changes the answer of the
/// next one (`phase 4 M3 design 10.2`, `9.5`; the manual's `[Map Local]` page:
/// "HTTPS requests can only be matched if MITM is enabled for the hostname").
#[tokio::test]
async fn map_local_and_body_rewrite_on_a_decrypted_https_request() {
    let fixture = TlsFixture::new(&["origin.test"]);
    let body = "<p>documents</p>";
    let origin = ScriptedOrigin::spawn_tls(
        fixture.acceptor_with_alpn(false, &[b"http/1.1"]),
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
    .await;
    let port = origin.port();
    let mitm = mitm_ca();
    let sections = format!(
        "{}\n\
         [Map Local]\n\
         ^https://origin\\.test:\\d+/mapped data-type=text data=\"mapped\"\n\
         [Body Rewrite]\n\
         http-response ^https://origin\\.test:\\d+/rewritten documents Surge",
        mitm_section(&format!("origin.test:{port}"), &mitm.ca_p12_base64("pw"))
    );
    let h = harness_mitm(
        Profile {
            hosts: "origin.test = 127.0.0.1",
            rules: "DOMAIN,origin.test,DIRECT",
            sections: &sections,
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;

    let (settled, mut tls) = decrypted(&h, mitm.roots(), port, &[b"http/1.1"]).await;
    assert_eq!(settled.as_deref(), Some("http/1.1"), "{settled:?}");
    let host = format!("origin.test:{port}");

    let answer = decrypted_get(&mut tls, &host, "/mapped").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("mapped"), "{answer}");

    let answer = decrypted_get(&mut tls, &host, "/rewritten").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("<p>Surge</p>"), "{answer}");

    let heard = origin.heard();
    assert_eq!(
        heard.len(),
        1,
        "only the second request was dialled: {heard:?}"
    );
    assert!(
        heard[0].0.starts_with("GET /rewritten HTTP/1.1"),
        "{}",
        heard[0].0
    );
    let records = the_records(&h, 2).await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(
        records[0].notes,
        ["Map Local: text by ^https://origin\\.test:\\d+/mapped".to_string()],
        "{records:?}"
    );
    assert_eq!(
        records[1].notes,
        ["Body Rewrite: response by ^https://origin\\.test:\\d+/rewritten".to_string()],
        "{records:?}"
    );
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.body), (1, 1), "{counts:?}");
}

/// The `rewrite` switch (`phase 4 M3 design 11`) turns both steps off with the
/// ones M3a and M3b added: with it off a `[Map Local]` line does not answer and
/// a body rule changes nothing, neither leaves a note, and no count moves; with
/// it on again both act. The pages say the same of Map Local: "only when the
/// Rewrite feature is enabled".
#[tokio::test]
async fn the_switch_turns_body_rewrite_and_map_local_off() {
    let body = "documents";
    let origin = ScriptedOrigin::spawn(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
    .await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "surgetest.com = 127.0.0.1",
            rules: "DOMAIN,surgetest.com,DIRECT",
            sections: "[Map Local]\n\
                       ^http://surgetest\\.com:\\d+/mapped data-type=text data=\"mapped\"\n\
                       [Body Rewrite]\n\
                       http-response ^http://surgetest\\.com:\\d+/origin documents Surge",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let mapped = format!("http://surgetest.com:{port}/mapped");
    let rewritten = format!("http://surgetest.com:{port}/origin");
    let host = format!("surgetest.com:{port}");

    // on: the line answers, the rule changes the answer
    let answer = plain_get(h.http(), &mapped, &host).await;
    assert!(answer.ends_with("mapped"), "{answer}");
    let answer = plain_get(h.http(), &rewritten, &host).await;
    assert!(answer.ends_with("Surge"), "{answer}");
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.body), (1, 1), "{counts:?}");

    // off: neither acts, and nothing is counted
    h.engine.set_rewrite(false).await;
    assert!(!h.engine.rewrite_on());
    let answer = plain_get(h.http(), &mapped, &host).await;
    assert!(
        answer.ends_with("documents"),
        "the origin answered: {answer}"
    );
    let answer = plain_get(h.http(), &rewritten, &host).await;
    assert!(answer.ends_with("documents"), "{answer}");
    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (counts.map_local, counts.body),
        (1, 1),
        "a switch off changes no count: {counts:?}"
    );

    // on again: both act again
    h.engine.set_rewrite(true).await;
    let answer = plain_get(h.http(), &mapped, &host).await;
    assert!(answer.ends_with("mapped"), "{answer}");
    let answer = plain_get(h.http(), &rewritten, &host).await;
    assert!(answer.ends_with("Surge"), "{answer}");
    let counts = h.engine.http_counts().rewrite;
    assert_eq!((counts.map_local, counts.body), (2, 2), "{counts:?}");

    // the origin answered the two `/origin` requests and the two the switch
    // off let through, and those last two left no note
    let heard = origin.heard();
    assert_eq!(heard.len(), 4, "{heard:?}");
    let records = the_records(&h, 6).await;
    assert_eq!(records.len(), 6, "{records:?}");
    assert!(records[2].notes.is_empty(), "{:?}", records[2].notes);
    assert!(records[3].notes.is_empty(), "{:?}", records[3].notes);
}

/// Neither a body nor a `[Map Local]` line's data nor a query string reaches
/// the notes or the log, which this case keeps down to TRACE (`phase 4 M3
/// design 8.5`, `14`; `M3-D18`, the phase 4 / M3b plan's P2, P5, P18).
#[tokio::test]
async fn no_body_or_data_reaches_the_notes_or_the_log() {
    let log = LogCapture::everything();
    let answer_body = "s3cr3t-b in the answer";
    let origin = ScriptedOrigin::spawn(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{answer_body}",
            answer_body.len()
        )
        .into_bytes(),
    )
    .await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "b.test = 127.0.0.1",
            rules: "DOMAIN,b.test,DIRECT",
            sections: r#"[Body Rewrite]
http-request ^http://b\.test:\d+/in = abc
http-response ^http://b\.test:\d+/out answer Surge
[Map Local]
^http://b\.test:\d+/mapped data-type=text data="s3cr3t-m""#,
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let host = format!("b.test:{port}");

    // 1. a request whose body is rewritten: the body is not in any note
    let body = "s3cr3t-b=1";
    let request = format!(
        "POST http://b.test:{port}/in HTTP/1.1\r\nHost: {host}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (head, _) = answer_parts(&answer_bytes(h.http(), request.as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(
        origin.heard()[0].1,
        b"s3cr3t-babc1",
        "the rewritten body did reach the origin"
    );

    // 2. a response whose body is rewritten
    let (head, answer) = answer_parts(
        &answer_bytes(
            h.http(),
            format!(
                "GET http://b.test:{port}/out HTTP/1.1\r\nHost: {host}\r\n\
                 Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await,
    );
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(answer.ends_with(b"in the Surge"), "{head}");

    // 3. a `[Map Local]` line whose data is the secret, under a query string
    let (head, answer) = answer_parts(
        &answer_bytes(
            h.http(),
            format!(
                "GET http://b.test:{port}/mapped?tok=q1 HTTP/1.1\r\nHost: {host}\r\n\
                 Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await,
    );
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(answer, b"s3cr3t-m", "the data did reach the client");

    let records = the_records(&h, 3).await;
    assert_eq!(records.len(), 3, "{records:?}");
    for record in &records {
        for note in &record.notes {
            for secret in ["s3cr3t-b", "s3cr3t-m", "q1"] {
                assert!(
                    !note.contains(secret),
                    "a note carries {secret}: {note} ({record:?})"
                );
            }
        }
    }
    for line in log.lines() {
        for secret in ["s3cr3t-b", "s3cr3t-m", "q1"] {
            assert!(
                !line.contains(secret),
                "a log line carries {secret}: {line}"
            );
        }
    }
}

/// The six counts of `/v1/rurge/http`'s `rewrite` after M3b's two steps it
/// adds (`phase 4 M3 design 12`, the phase 4 / M3b plan's P18): every one of
/// them moved by the requests that make it, and none by the others.
#[tokio::test]
async fn body_and_map_local_counts_reach_the_api_shape() {
    let answer_body = "the answer";
    let origin = ScriptedOrigin::spawn(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{answer_body}",
            answer_body.len()
        )
        .into_bytes(),
    )
    .await;
    let port = origin.port();
    let h = harness_with(
        Profile {
            hosts: "b.test = 127.0.0.1\nlocalhost = 127.0.0.1",
            rules: "DOMAIN,b.test,DIRECT",
            sections: "[URL Rewrite]\n\
                       ^http://b\\.test:\\d+/moved https://localhost/ 302\n\
                       [Header Rewrite]\n\
                       http-response ^http://b\\.test:\\d+/out header-add X-A 1\n\
                       [Body Rewrite]\n\
                       http-request ^http://b\\.test:\\d+/(in|big) value abc\n\
                       http-request ^http://b\\.test:\\d+/chunked value abc\n\
                       http-response ^http://b\\.test:\\d+/out answer Surge\n\
                       [Map Local]\n\
                       ^http://b\\.test:\\d+/mapped data-type=text data=\"mapped\"\n\
                       ^http://b\\.test:\\d+/missing data-type=file data=\"data/none.json\"",
            ..Profile::default()
        },
        shared_on(NetworkFacts::default()),
    )
    .await;
    let host = format!("b.test:{port}");
    let get = |path: &str| {
        format!(
            "GET http://b.test:{port}{path} HTTP/1.1\r\nHost: {host}\r\n\
             Connection: close\r\n\r\n"
        )
    };

    // mapLocal 1: the line answers
    let answer = plain_get(h.http(), &format!("http://b.test:{port}/mapped"), &host).await;
    assert!(answer.ends_with("mapped"), "{answer}");
    // failed 1: the line's file is not there
    let answer = plain_get(h.http(), &format!("http://b.test:{port}/missing"), &host).await;
    assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
    // body 1: the request's body changes
    let (head, _) = answer_parts(
        &answer_bytes(
            h.http(),
            format!(
                "POST http://b.test:{port}/in HTTP/1.1\r\nHost: {host}\r\n\
                 Content-Length: 5\r\nConnection: close\r\n\r\nvalue"
            )
            .as_bytes(),
        )
        .await,
    );
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    // header 1 and body 1: the field is added and the answer changes
    let (head, body) = answer_parts(&answer_bytes(h.http(), get("/out").as_bytes()).await);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert!(body.ends_with(b"the Surge"), "{head}");
    assert_eq!(header_value(&head, "x-a").as_deref(), Some("1"), "{head}");
    // skipped 1: a chunked request is forwarded, and its rule says why
    let (head, _) = answer_parts(
        &answer_bytes(
            h.http(),
            format!(
                "POST http://b.test:{port}/chunked HTTP/1.1\r\nHost: {host}\r\n\
                 Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                 5\r\nvalue\r\n0\r\n\r\n"
            )
            .as_bytes(),
        )
        .await,
    );
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    // url 1: the redirect answers
    let answer = plain_get(h.http(), &format!("http://b.test:{port}/moved"), &host).await;
    assert!(answer.starts_with("HTTP/1.1 302"), "{answer}");
    // failed 2: a request whose head is over the limit
    let over = format!(
        "POST http://b.test:{port}/big HTTP/1.1\r\nHost: {host}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        32 * 1024 * 1024 + 1
    );
    assert!(answer_bytes(h.http(), over.as_bytes()).await.is_empty());

    let counts = h.engine.http_counts().rewrite;
    assert_eq!(
        (
            counts.url,
            counts.header,
            counts.body,
            counts.map_local,
            counts.skipped,
            counts.failed
        ),
        (1, 1, 2, 1, 1, 2),
        "{counts:?}"
    );
    let records = the_records(&h, 7).await;
    assert_eq!(records.len(), 7, "{records:?}");
}
