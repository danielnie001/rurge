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
        }
    }
}

/// A bound listener; dropping it stops accepting (live sessions finish on their own).
pub struct Running {
    pub local_addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const REJECTED_SOURCE_LOG_INTERVAL: Duration = Duration::from_secs(60);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(50);

#[allow(dead_code)] // no caller until Task 5 wires an HTTP/SOCKS5 listener onto this
pub(crate) fn serve<F, Fut>(
    listener: TcpListener,
    name: &'static str,
    restrict_to_lan: bool,
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
    let task = tokio::spawn(async move {
        let mut sessions: JoinSet<()> = JoinSet::new();
        let warned: Mutex<HashMap<IpAddr, Instant>> = Mutex::new(HashMap::new());
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, peer)) => {
                        if restrict_to_lan && !source_allowed(local_addr, peer.ip()) {
                            let mut map = warned.lock().expect("warned sources");
                            let now = Instant::now();
                            let due = map.get(&peer.ip()).is_none_or(|t| now.duration_since(*t) >= REJECTED_SOURCE_LOG_INTERVAL);
                            if due {
                                map.insert(peer.ip(), now);
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
    });
    Running { local_addr, task }
}

#[allow(dead_code)] // no caller until Task 5 wires an HTTP/SOCKS5 listener onto this
pub(crate) async fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    TcpListener::bind(addr).await
}
