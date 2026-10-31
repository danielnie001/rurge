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
