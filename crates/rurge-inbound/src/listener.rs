//! Shared accept loop (M3 design §6.4): one task per connection, panics
//! isolated by a `JoinSet`, source restriction, accept-error backoff.

use crate::restrict::source_allowed;
use rurge_config::session::ListenerKind;
use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

/// Basic credentials accepted by an HTTP listener.
#[derive(Clone, PartialEq, Eq)]
pub enum HttpAuth {
    /// `password@` from `http-listen`: any user name, this password.
    Password(String),
    /// `wifi-access-http-auth`: both must match.
    UserPass { user: String, password: String },
}

/// The passwords are left out, whatever prints the options of a listener.
impl std::fmt::Debug for HttpAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpAuth::Password(_) => f.debug_tuple("Password").field(&"<redacted>").finish(),
            HttpAuth::UserPass { user, .. } => f
                .debug_struct("UserPass")
                .field("user", user)
                .field("password", &"<redacted>")
                .finish(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ListenerOpts {
    pub kind: ListenerKind,
    pub restrict_to_lan: bool,
    pub auth: Option<HttpAuth>,
    pub show_error_page: bool,
    pub show_error_page_for_reject: bool,
    /// How long REJECT-DROP keeps a connection open without answering.
    pub drop_hold: Duration,
    /// Upper bound on reading the HTTP request head / completing the SOCKS5
    /// handshake. Dialing and relaying are not covered by it.
    pub handshake_timeout: Duration,
}

impl Default for ListenerOpts {
    fn default() -> Self {
        ListenerOpts {
            kind: ListenerKind::Http,
            restrict_to_lan: true,
            auth: None,
            show_error_page: true,
            show_error_page_for_reject: false,
            drop_hold: Duration::from_secs(30),
            handshake_timeout: Duration::from_secs(30),
        }
    }
}

/// A bound listener. Dropping it aborts the accept loop and every session its
/// own `JoinSet` is still tracking (CONNECT tunnels and upstream-connection
/// drivers are spawned onto the engine's `TaskTracker` instead, so they are
/// not covered by that abort). For a graceful shutdown, call `stop` then
/// `join`: the accept loop stops taking new connections, closes the socket,
/// and drains its in-flight sessions instead of aborting them.
pub struct Running {
    pub local_addr: SocketAddr,
    task: Option<JoinHandle<()>>,
    stop: CancellationToken,
    /// Latched once the accept loop has closed its socket.
    closed: CancellationToken,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(t) = &self.task {
            t.abort();
        }
    }
}

impl Running {
    /// Stop accepting on this listener only; its in-flight sessions finish on
    /// their own (awaited by `join`).
    pub fn stop(&self) {
        self.stop.cancel();
    }

    /// Resolves once the accept loop has closed its socket, i.e. the address
    /// can be bound again (Windows sets no `SO_REUSEADDR`). Sessions may still
    /// be draining; see `join`.
    pub async fn wait_closed(&self) {
        self.closed.cancelled().await
    }

    /// Waits for the accept loop to stop and its in-flight sessions to drain.
    /// Consumes `self` so the `Drop` abort only fires on an already-finished task.
    pub async fn join(mut self) {
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }
}

const REJECTED_SOURCE_LOG_INTERVAL: Duration = Duration::from_secs(60);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);
/// Upper bound on the rejected-source table so a spoofed-source flood cannot
/// grow it without limit.
const MAX_WARNED_SOURCES: usize = 1024;

/// Records that `source` was warned about at `now` and answers whether the
/// warning is due. Full tables are swept of stale entries first; if that frees
/// nothing the source is not remembered but is still warned about once.
fn warn_due(map: &mut HashMap<IpAddr, Instant>, source: IpAddr, now: Instant) -> bool {
    let due = map
        .get(&source)
        .is_none_or(|t| now.duration_since(*t) >= REJECTED_SOURCE_LOG_INTERVAL);
    if !due {
        return false;
    }
    if map.len() >= MAX_WARNED_SOURCES && !map.contains_key(&source) {
        map.retain(|_, t| now.duration_since(*t) < REJECTED_SOURCE_LOG_INTERVAL);
        if map.len() >= MAX_WARNED_SOURCES {
            return true;
        }
    }
    map.insert(source, now);
    true
}

/// The client the way the rules and the request log name it. An IPv4 client
/// of a listener bound to an IPv6 address (dual-stack on Linux and macOS)
/// shows up as `::ffff:a.b.c.d`, which no IPv4 `SRC-IP` network contains.
fn canonical_peer(peer: SocketAddr) -> SocketAddr {
    match peer.ip().to_canonical() {
        IpAddr::V4(v4) => SocketAddr::new(IpAddr::V4(v4), peer.port()),
        // as it is, scope and all
        IpAddr::V6(_) => peer,
    }
}

pub(crate) fn serve<F, Fut>(
    listener: TcpListener,
    name: &'static str,
    restrict_to_lan: bool,
    stop: CancellationToken,
    handler: F,
) -> Running
where
    F: Fn(TcpStream, SocketAddr) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    serve_checking(
        listener,
        name,
        restrict_to_lan,
        source_allowed,
        stop,
        handler,
    )
}

/// `serve` with the rule that tells the sources a restricted listener takes
/// given. It is `source_allowed`; a test, which has only the loopback to
/// connect from — a source that rule always lets in —, puts another in its
/// place to see what the accept loop does with a source that is refused.
fn serve_checking<F, Fut>(
    listener: TcpListener,
    name: &'static str,
    restrict_to_lan: bool,
    allowed: fn(SocketAddr, IpAddr) -> bool,
    stop: CancellationToken,
    handler: F,
) -> Running
where
    F: Fn(TcpStream, SocketAddr) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let local_addr = listener
        .local_addr()
        .expect("bound listener has an address");
    let handler = Arc::new(handler);
    let stop_for_running = stop.clone();
    let closed = CancellationToken::new();
    let closed_signal = closed.clone();
    let task = tokio::spawn(async move {
        let mut sessions: JoinSet<()> = JoinSet::new();
        let warned: Mutex<HashMap<IpAddr, Instant>> = Mutex::new(HashMap::new());
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        if restrict_to_lan && !allowed(local_addr, peer.ip()) {
                            let mut map = warned.lock().expect("warned sources");
                            if warn_due(&mut map, peer.ip(), Instant::now()) {
                                tracing::warn!(listener = name, source = %peer.ip(), "connection refused: source is not on the LAN (proxy-restricted-to-lan)");
                            }
                            drop(stream);
                            continue;
                        }
                        let _ = stream.set_nodelay(true);
                        rurge_net::socket::keep_alive(&stream);
                        let h = handler.clone();
                        let peer = canonical_peer(peer);
                        sessions.spawn(async move { h(stream, peer).await });
                    }
                    Err(e) => {
                        tracing::warn!(listener = name, error = %e, "accept failed");
                        tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                    }
                },
                Some(joined) = sessions.join_next(), if !sessions.is_empty() => {
                    if let Err(e) = joined
                        && e.is_panic()
                    {
                        tracing::error!(listener = name, "session task panicked: {e}");
                    }
                }
            }
        }
        // Stopped accepting: close the socket so nothing queues in the backlog,
        // then drain the in-flight sessions. A relay only returns once its own
        // token is cancelled (graceful) or its peers close; the daemon cancels
        // the session root after the grace period.
        drop(listener);
        closed_signal.cancel();
        while let Some(joined) = sessions.join_next().await {
            if let Err(e) = joined
                && e.is_panic()
            {
                tracing::error!(listener = name, "session task panicked: {e}");
            }
        }
    });
    Running {
        local_addr,
        task: Some(task),
        stop: stop_for_running,
        closed,
    }
}

pub(crate) async fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::testing::Lines;
    use std::net::Ipv4Addr;
    use tokio::io::AsyncReadExt;

    fn ip(n: u32) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(n))
    }

    #[test]
    fn warned_sources_are_rate_limited_and_bounded() {
        let mut map = HashMap::new();
        let t0 = Instant::now();
        assert!(warn_due(&mut map, ip(1), t0), "first warning is due");
        assert!(!warn_due(&mut map, ip(1), t0), "repeat within the interval");
        assert!(warn_due(&mut map, ip(1), t0 + REJECTED_SOURCE_LOG_INTERVAL));
        // a flood of fresh sources warns every time but never grows past the cap
        let t1 = t0 + REJECTED_SOURCE_LOG_INTERVAL;
        for n in 0..(MAX_WARNED_SOURCES as u32 + 50) {
            assert!(warn_due(&mut map, ip(1000 + n), t1));
        }
        assert!(map.len() <= MAX_WARNED_SOURCES, "{}", map.len());
        // once the flood ages out the table is swept instead of staying full
        let t2 = t1 + REJECTED_SOURCE_LOG_INTERVAL;
        assert!(warn_due(&mut map, ip(7), t2));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn an_ipv4_client_of_a_dual_stack_listener_is_named_by_its_ipv4_address() {
        let addr = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert_eq!(
            canonical_peer(addr("[::ffff:192.168.1.7]:5555")),
            addr("192.168.1.7:5555")
        );
        // what is no IPv4 client stays as the socket reported it
        for same in ["192.168.1.7:5555", "[2001:db8::1]:6", "[::1]:7"] {
            assert_eq!(canonical_peer(addr(same)), addr(same), "{same}");
        }
    }

    /// What a listener takes keeps itself alive: a client gone without a
    /// word (asleep, off the Wi-Fi) holds its UDP association not for ever.
    #[tokio::test]
    async fn an_accepted_connection_keeps_itself_alive() {
        let listener = bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let tx = Mutex::new(Some(tx));
        let _running = serve(
            listener,
            "test",
            false,
            CancellationToken::new(),
            move |stream, _| {
                let on = socket2::SockRef::from(&stream).keepalive().unwrap();
                if let Some(tx) = tx.lock().unwrap().take() {
                    let _ = tx.send(on);
                }
                async {}
            },
        );
        let _client = TcpStream::connect(addr).await.unwrap();
        let on = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("the connection is taken")
            .unwrap();
        assert!(on);
    }

    /// What prints the options of a listener — a log line, a failed
    /// assertion — does not print its passwords.
    #[test]
    fn the_passwords_of_a_listener_are_not_printed() {
        let opts = ListenerOpts {
            auth: Some(HttpAuth::Password("S3CR3T-listen".to_string())),
            ..ListenerOpts::default()
        };
        let printed = format!("{opts:?}");
        assert!(
            printed.contains("Password(\"<redacted>\")") && !printed.contains("S3CR3T"),
            "{printed}"
        );
        let wifi = HttpAuth::UserPass {
            user: "alice".to_string(),
            password: "S3CR3T-wifi".to_string(),
        };
        let printed = format!("{wifi:?}");
        assert!(
            printed.contains("alice")
                && printed.contains("<redacted>")
                && !printed.contains("S3CR3T"),
            "{printed}"
        );
    }

    /// A listener that keeps to the LAN closes a connection from any other
    /// source without a word and says so in the log, naming the source; one
    /// that does not keep to it takes the same connection. Which sources are
    /// local is `restrict`'s to say, tested apart; here no source is, which
    /// the loopback, the only place a test can connect from, cannot show.
    #[tokio::test]
    async fn a_listener_that_keeps_to_the_lan_closes_what_is_not_local_and_a_free_one_takes_it() {
        let lines = Lines::default();
        // held across the awaits: the accept loops run on this thread
        let _capturing = lines.capture();
        for (restrict_to_lan, taken) in [(true, false), (false, true)] {
            let listener = bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (seen, mut sources) = tokio::sync::mpsc::unbounded_channel();
            let _running = serve_checking(
                listener,
                "test",
                restrict_to_lan,
                |_, _| false,
                CancellationToken::new(),
                move |_, peer| {
                    let _ = seen.send(peer);
                    async {}
                },
            );
            let mut client = TcpStream::connect(addr).await.unwrap();
            if taken {
                let peer = tokio::time::timeout(Duration::from_secs(5), sources.recv())
                    .await
                    .expect("the connection is taken")
                    .unwrap();
                assert_eq!(peer, client.local_addr().unwrap());
            } else {
                let mut byte = [0u8; 1];
                let read = tokio::time::timeout(Duration::from_secs(5), client.read(&mut byte))
                    .await
                    .expect("the connection is closed");
                assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
                // the loop has dropped the connection, and the handler never had it
                assert!(sources.try_recv().is_err());
            }
        }
        let refused = lines.said("proxy-restricted-to-lan");
        assert_eq!(refused.len(), 1, "{:?}", lines.said(""));
        assert!(
            refused[0].contains("listener=\"test\"") && refused[0].contains("source=127.0.0.1"),
            "{}",
            refused[0]
        );
    }
}
