//! rurge's own connections leave by the network outside the virtual
//! interface: before a socket is made, the platform's socket hook is asked
//! for the egress of the destination (M4 design 7.2). The source scan in
//! `crates/rurge/tests/sockets.rs` says where sockets are made; this file
//! says that the connections those places make — every kind of DNS upstream,
//! a pipeline's fallback, the downloads of a generation — ask the hook, and
//! that a hook that refuses stops them before a byte is sent.

mod common;
use common::*;

use rurge_dns::LookupOpts;
use rurge_engine::stack::{Stack, build_stack};
use rurge_net::socket::Family;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// How long a lookup is given before it is dropped: what is observed is the
/// hook, which is asked before the socket is made, and a server that answers
/// nothing holds a lookup until the resolver gives up, thirty seconds on. The
/// bound ends them; it is not a wait for something to happen.
const BOUND: Duration = Duration::from_secs(2);

/// How long a test waits to see that nothing arrived: no socket was made, so
/// nothing can arrive at all, and this is only a margin (a lookup with a hook
/// that refuses ends at once, well within it).
const QUIET: Duration = Duration::from_millis(300);

/// The name a lookup asks for: not `.local`, not a single label and not in a
/// `[Host]` table, so it is asked of the profile's server alone.
const NAME: &str = "hook.test";

/// The address every server of this file is on.
fn loopback() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

/// A socket hook that notes the address of every egress it is asked for, and
/// refuses them all when it is asked to.
struct EgressLog {
    refused: AtomicBool,
    asked: Mutex<Vec<IpAddr>>,
}

impl EgressLog {
    fn recording() -> Arc<EgressLog> {
        Arc::new(EgressLog {
            refused: AtomicBool::new(false),
            asked: Mutex::new(Vec::new()),
        })
    }

    fn refusing() -> Arc<EgressLog> {
        let log = Self::recording();
        log.refused.store(true, Ordering::Relaxed);
        log
    }

    fn asked(&self) -> Vec<IpAddr> {
        self.asked.lock().expect("egress asked").clone()
    }
}

impl SocketHook for EgressLog {
    fn bind_interface(&self, _: &socket2::Socket, _: &str, _: Family) -> std::io::Result<()> {
        Ok(())
    }

    fn set_tos(&self, _: &socket2::Socket, _: Family, _: u8) -> std::io::Result<()> {
        Ok(())
    }

    fn egress(&self, dst: IpAddr) -> std::io::Result<Option<String>> {
        self.asked.lock().expect("egress asked").push(dst);
        if self.refused.load(Ordering::Relaxed) {
            return Err(std::io::Error::other(
                "no network outside the enhanced mode",
            ));
        }
        Ok(None)
    }
}

/// The hook as the stack's.
fn hook(log: &Arc<EgressLog>) -> Arc<dyn SocketHook> {
    log.clone()
}

/// The profile of a generation whose only DNS server is `line`.
fn profile(line: &str) -> String {
    format!(
        "[General]\n{line}\nipv6 = false\nproxy-test-url = {NO_TEST}\ninternet-test-url = {NO_TEST}\n\
         [Rule]\nFINAL,DIRECT\n"
    )
}

/// The stack of `text`, with every socket of its connectors and downloads
/// going through `hook`.
async fn stack_over(dir: &Path, text: &str, hook: Arc<dyn SocketHook>) -> Stack {
    std::fs::write(dir.join("t.conf"), text).unwrap();
    let loaded = from_text(text, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    let mut stack = stack_options(dir);
    stack.socket_hook = hook;
    build_stack(&loaded.config, &stack).await.unwrap()
}

/// What a server of the test's own took: the connections that were accepted
/// (TCP) or the datagrams that were received (UDP).
type Seen = Arc<Mutex<Vec<SocketAddr>>>;

fn taken() -> Seen {
    Arc::new(Mutex::new(Vec::new()))
}

fn took(seen: &Seen) -> usize {
    seen.lock().expect("what the server took").len()
}

/// A loopback listener that takes every connection and answers none of them.
async fn silent_tcp() -> (SocketAddr, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = taken();
    let log = seen.clone();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, peer)) = listener.accept().await {
            log.lock().expect("what the server took").push(peer);
            held.push(stream);
        }
    });
    (addr, seen)
}

/// A loopback socket that receives every datagram and answers none of them.
async fn silent_udp() -> (SocketAddr, Seen) {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();
    let seen = taken();
    let log = seen.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        while let Ok((_, peer)) = socket.recv_from(&mut buf).await {
            log.lock().expect("what the server took").push(peer);
        }
    });
    (addr, seen)
}

/// A loopback DNS server that answers every query as truncated, and the
/// listener on the same port that takes the retry and answers it not: what
/// the listener took, which is the stream of the second step.
async fn truncated() -> (SocketAddr, Seen) {
    let (addr, seen) = silent_tcp().await;
    let socket = tokio::net::UdpSocket::bind(addr).await.unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                return;
            };
            let mut reply = buf[..n].to_vec();
            if reply.len() > 2 {
                reply[2] |= 0x02; // TC: the answer is truncated
            }
            let _ = socket.send_to(&reply, peer).await;
        }
    });
    (addr, seen)
}

/// How a case's server is reached, which decides what the test holds for it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// A listener that takes the connection and answers nothing.
    Tcp,
    /// A socket that receives the datagram and answers nothing.
    Udp,
    /// A server that answers the datagram as truncated, and a listener that
    /// takes the stream after it.
    Truncated,
}

/// One way a DNS upstream is reached (design 7.2): its name, the line of the
/// profile that reaches it that way (with the address of the test's own
/// server), and what the test holds for it.
struct Upstream {
    name: &'static str,
    line: fn(SocketAddr) -> String,
    reach: Reach,
}

fn upstreams() -> [Upstream; 7] {
    [
        Upstream {
            name: "dns-server",
            line: |addr| format!("dns-server = {addr}"),
            reach: Reach::Udp,
        },
        Upstream {
            name: "a truncated answer",
            line: |addr| format!("dns-server = {addr}"),
            reach: Reach::Truncated,
        },
        Upstream {
            name: "tcp://",
            line: |addr| format!("encrypted-dns-server = tcp://{addr}"),
            reach: Reach::Tcp,
        },
        Upstream {
            name: "tls://",
            line: |addr| format!("encrypted-dns-server = tls://{addr}"),
            reach: Reach::Tcp,
        },
        Upstream {
            name: "https://",
            line: |addr| format!("encrypted-dns-server = https://{addr}/dns-query"),
            reach: Reach::Tcp,
        },
        Upstream {
            name: "quic://",
            line: |addr| format!("encrypted-dns-server = quic://{addr}"),
            reach: Reach::Udp,
        },
        Upstream {
            name: "h3://",
            line: |addr| format!("encrypted-dns-server = h3://{addr}/dns-query"),
            reach: Reach::Udp,
        },
    ]
}

/// Holds a server of `reach` for the test and returns its address and what it
/// took.
async fn hold(reach: Reach) -> (SocketAddr, Seen) {
    match reach {
        Reach::Tcp => silent_tcp().await,
        Reach::Udp => silent_udp().await,
        Reach::Truncated => truncated().await,
    }
}

/// Every kind of DNS upstream asks the hook for the egress of the server's
/// address before the socket to it is made. The servers answer nothing (or,
/// truncated, ask for a second one), so what a lookup does is not observed:
/// only that the hook was asked, which happens first.
#[tokio::test]
async fn every_kind_of_dns_upstream_asks_the_hook_before_it_dials() {
    for Upstream { name, line, reach } in upstreams() {
        let log = EgressLog::recording();
        let dir = tempfile::tempdir().unwrap();
        let (addr, seen) = hold(reach).await;
        let stack = stack_over(dir.path(), &profile(&line(addr)), hook(&log)).await;

        let _ =
            tokio::time::timeout(BOUND, stack.resolver.lookup(NAME, LookupOpts::default())).await;

        assert!(
            log.asked().contains(&loopback()),
            "{name}: the hook was asked {:?}",
            log.asked()
        );
        if reach == Reach::Truncated {
            // the datagram asked once; the stream after it is a socket of its
            // own, and it was made only if the server took the connection
            assert!(
                log.asked().len() >= 2,
                "{name}: the hook was asked {:?}",
                log.asked()
            );
            assert!(took(&seen) >= 1, "{name}: the stream after the truncation");
        }
    }
}

/// With `encrypted-dns-follow-outbound-mode` on, a generation's DNS
/// connections take the dial pipeline, and the pipeline has no engine to ask
/// until the daemon attaches one: the fallback it dials with is rurge's own
/// connector, and that carries the hook (design 7.2, 4.2).
#[tokio::test]
async fn the_pipeline_fallback_asks_the_hook() {
    let (addr, _seen) = silent_tcp().await;
    let log = EgressLog::recording();
    let dir = tempfile::tempdir().unwrap();
    let text = profile(&format!(
        "encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tls://{addr}"
    ));
    std::fs::write(dir.path().join("t.conf"), &text).unwrap();
    let loaded = from_text(&text, &dir.path().join("t.conf"), &LoadOptions::for_tests());
    assert!(!loaded.diagnostics.has_errors());
    let mut stack = stack_options(dir.path());
    stack.socket_hook = hook(&log);
    let runtime = Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack,
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            shared: EngineShared::default(),
            request_log_size: 100,
        },
    )
    .await
    .unwrap();
    assert!(
        runtime.dns_pipeline().is_some(),
        "the profile follows the outbound mode"
    );

    let _ = tokio::time::timeout(
        BOUND,
        runtime.stack.resolver.lookup(NAME, LookupOpts::default()),
    )
    .await;

    assert!(
        log.asked().contains(&loopback()),
        "the hook was asked {:?}",
        log.asked()
    );
}

/// A generation's downloads — its external resources and its databases — ask
/// the hook as well: they are rurge's own connections, and the daemon's two
/// download clients are made with it (M4 design 7.3).
#[tokio::test]
async fn resource_downloads_ask_the_hook() {
    let server = TestServer::spawn().await;
    server.set("/sets/a.list", "DOMAIN,one.example,DIRECT\n");
    let log = EgressLog::recording();
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "[General]\nproxy-test-url = {NO_TEST}\ninternet-test-url = {NO_TEST}\n\
         [Rule]\nRULE-SET,{},DIRECT\nFINAL,DIRECT\n",
        server.url("/sets/a.list")
    );
    let _runtime = online_runtime_with(
        dir.path(),
        &text,
        &server,
        EngineShared::default(),
        |stack| stack.socket_hook = hook(&log),
    )
    .await;

    wait_until("the downloads to ask the hook", || !log.asked().is_empty()).await;
    assert!(
        log.asked().contains(&loopback()),
        "the hook was asked {:?}",
        log.asked()
    );
}

/// The Profile sources of a generation — the managed profile, the remote
/// includes — ask the hook as well: `ProfileSources::production` dials
/// through the client it is given and makes no client of its own (M4 design
/// 7.6; the phase 3 / M4b plan's execution record row 72).
#[tokio::test]
async fn profile_sources_ask_the_hook() {
    use rurge_config::Remote;
    use rurge_config::config::load;
    use rurge_engine::profile::{ProfileSources, disk_remote};
    use rurge_engine::stack::resource_client_with;
    use rurge_net::connector::SystemResolve;

    let server = TestServer::spawn().await;
    server.set("/inc.list", "[Rule]\nDOMAIN,two.example,DIRECT\n");
    let log = EgressLog::recording();
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("t.conf");
    let data = dir.path().join("data");
    // the address of the include is an IP address: no name reaches the
    // system's resolver library
    let text = format!(
        "[General]\nproxy-test-url = {NO_TEST}\ninternet-test-url = {NO_TEST}\n\
         [Rule]\n#!include {}\nFINAL,DIRECT\n",
        server.url("/inc.list")
    );
    std::fs::write(&profile, &text).unwrap();

    let sources = ProfileSources::production(
        &profile,
        &LoadOptions::for_tests(),
        &data,
        false,
        resource_client_with(hook(&log), Arc::new(SystemResolve)).unwrap(),
    )
    .unwrap();

    // what a generation's load comes across, with nothing downloaded yet
    let mut opts = LoadOptions::for_tests();
    opts.remote = Some(disk_remote(&data));
    let info = load(&profile, &opts).unwrap().config.source;
    sources.track(&info);
    // the download is the sources' own task: the copy is what says it arrived
    let copy = || disk_remote(&data).get(&server.url("/inc.list"));
    wait_until("the include to be downloaded", || {
        matches!(copy(), Remote::Text { .. })
    })
    .await;

    assert!(
        log.asked().contains(&loopback()),
        "the hook was asked {:?}",
        log.asked()
    );
    match copy() {
        Remote::Text { text, .. } => {
            assert!(text.contains("two.example"), "{text}");
        }
        other => panic!("the include was not downloaded: {other:?}"),
    }
}

/// A hook that refuses a destination stops the connection before anything is
/// sent: no connection reaches the listener, no datagram the socket.
#[tokio::test]
async fn an_egress_error_stops_each_kind_before_anything_is_sent() {
    for Upstream { name, line, reach } in upstreams() {
        let log = EgressLog::refusing();
        let dir = tempfile::tempdir().unwrap();
        // the server, held by the test: nothing is ever answered, and nothing
        // is expected to arrive
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let datagrams = match reach {
            Reach::Tcp => None,
            Reach::Udp | Reach::Truncated => Some(tokio::net::UdpSocket::bind(addr).await.unwrap()),
        };
        let stack = stack_over(dir.path(), &profile(&line(addr)), hook(&log)).await;

        // the refusal ends the lookup at once, or the bound does: either way
        // nothing was sent, which is what the quiet below shows
        let _ =
            tokio::time::timeout(BOUND, stack.resolver.lookup(NAME, LookupOpts::default())).await;

        assert!(
            log.asked().contains(&loopback()),
            "{name}: the hook was asked {:?}",
            log.asked()
        );
        // nothing was sent, so nothing can arrive at all: this is a wait for
        // the nothing, and the margin is what an arrival would have needed
        match &datagrams {
            None => assert!(
                tokio::time::timeout(QUIET, listener.accept())
                    .await
                    .is_err(),
                "{name}: a connection came to {addr}"
            ),
            Some(socket) => {
                let mut buf = [0u8; 2048];
                assert!(
                    tokio::time::timeout(QUIET, socket.recv_from(&mut buf))
                        .await
                        .is_err(),
                    "{name}: a datagram came to {addr}"
                );
            }
        }
    }
}
