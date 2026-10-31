//! The rewrites one generation carries (`phase 4 M3 design 8.4, 11, 12`; the
//! phase 4 / M3a plan's task 6): the `rewrite` switch of one engine, a request
//! whose policy is one of REJECT's, the gateway a `302` carries, and the counts
//! that outlive a reload.
//!
//! Nothing here goes beyond the loopback: every origin and guard is a loopback
//! server, and every name a rule names is pinned to `127.0.0.1` by `[Host]`.

mod common;
use common::*;
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
/// request line *with its query string*, and `TestServer` records the path
/// alone (`rurge-net`'s `RecordingOrigin` stands beside it; `the phase 4 / M3a
/// plan's execution record row 64`).
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
