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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpAuth {
    /// `password@` from `http-listen`: any user name, this password.
    Password(String),
    /// `wifi-access-http-auth`: both must match.
    UserPass { user: String, password: String },
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
                        if restrict_to_lan && !source_allowed(local_addr, peer.ip()) {
                            let mut map = warned.lock().expect("warned sources");
                            if warn_due(&mut map, peer.ip(), Instant::now()) {
                                tracing::warn!(listener = name, source = %peer.ip(), "connection refused: source is not on the LAN (proxy-restricted-to-lan)");
                            }
                            drop(stream);
                            continue;
                        }
                        let _ = stream.set_nodelay(true);
                        let h = handler.clone();
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
    use std::net::Ipv4Addr;

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
}
