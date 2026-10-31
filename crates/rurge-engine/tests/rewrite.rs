//! The rewrites one generation carries (`phase 4 M3 design 8.4`, 12; the phase
//! 4 / M3a plan's task 6): the `rewrite` switch of one engine, a request whose
//! policy is one of REJECT's, the gateway a `302` carries, and the counts that
//! outlive a reload.
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

/// The `rewrite` switch (`phase 4 M3 design 8.4`): with it off neither a
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
