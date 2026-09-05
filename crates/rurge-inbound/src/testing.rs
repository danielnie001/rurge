//! A scripted `Dialer` for listener tests: the destination host decides the
//! outcome, `relay` is a plain bidirectional copy.

use crate::session::{DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
use rurge_config::HostName;
use rurge_config::session::SessionInfo;
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::net::TcpStream;

/// Hosts: `echo.test` → `echo`, `target.test` → `target`, `reject.test`
/// (port 443 REJECT, 444 DROP, 445 NO-DROP, 446 TINYGIF), `fail.test`
/// (connect failure), `dns.test` (dns failure), `slow.test` (timeout).
#[allow(dead_code)] // no caller until Task 5 wires a listener test onto this
pub(crate) struct FakeDialer {
    pub echo: SocketAddr,
    pub target: Option<SocketAddr>,
    next_id: AtomicU64,
    pub handles: Mutex<Vec<Arc<SessionHandle>>>,
}

#[allow(dead_code)] // no caller until Task 5 wires a listener test onto this
impl FakeDialer {
    pub(crate) fn new(echo: SocketAddr, target: Option<SocketAddr>) -> Arc<FakeDialer> {
        Arc::new(FakeDialer {
            echo,
            target,
            next_id: AtomicU64::new(1),
            handles: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn sessions(&self) -> Vec<Arc<SessionHandle>> {
        self.handles.lock().unwrap().clone()
    }

    fn handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
        let h = SessionHandle::new(self.next_id.fetch_add(1, Ordering::Relaxed), session);
        h.set_rule(Some("FAKE,rule".to_string()));
        self.handles.lock().unwrap().push(h.clone());
        h
    }
}

impl Dialer for FakeDialer {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        Box::pin(async move {
            let host = match &session.dst_host {
                HostName::Domain(d) => d.clone(),
                HostName::Ip(ip) => ip.to_string(),
            };
            let port = session.dst_port;
            let handle = self.handle(session);
            let connect_to = match host.as_str() {
                "echo.test" | "127.0.0.1" => Some(self.echo),
                "target.test" => self.target,
                _ => None,
            };
            if let Some(addr) = connect_to {
                handle.set_policy_chain(vec!["DIRECT".to_string()]);
                let stream = TcpStream::connect(addr)
                    .await
                    .map_err(|e| DialError::Failed {
                        kind: FailKind::Connect,
                        message: e.to_string(),
                        rule: handle.rule(),
                        handle: handle.clone(),
                    })?;
                return Ok(Dialed {
                    stream: Box::new(stream),
                    handle,
                });
            }
            let rule = handle.rule();
            match host.as_str() {
                "reject.test" => {
                    let kind = match port {
                        444 => RejectKind::Drop,
                        445 => RejectKind::NoDrop,
                        446 => RejectKind::TinyGif,
                        _ => RejectKind::Reject,
                    };
                    handle.set_policy_chain(vec![kind.name().to_string()]);
                    handle.finish(SessionOutcome::Rejected(kind));
                    Err(DialError::Reject { kind, rule, handle })
                }
                "dns.test" => {
                    handle.finish(SessionOutcome::Failed("dns lookup failed".into()));
                    Err(DialError::Failed {
                        kind: FailKind::Dns,
                        message: "dns lookup failed".into(),
                        rule,
                        handle,
                    })
                }
                "slow.test" => {
                    handle.finish(SessionOutcome::Failed("connect timed out".into()));
                    Err(DialError::Failed {
                        kind: FailKind::Timeout,
                        message: "connect timed out".into(),
                        rule,
                        handle,
                    })
                }
                _ => {
                    handle.finish(SessionOutcome::Failed("connection refused".into()));
                    Err(DialError::Failed {
                        kind: FailKind::Connect,
                        message: "connection refused".into(),
                        rule,
                        handle,
                    })
                }
            }
        })
    }

    fn relay<'a>(
        &'a self,
        mut client: BoxedStream,
        mut upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            match tokio::io::copy_bidirectional(&mut client, &mut upstream).await {
                Ok((up, down)) => {
                    handle.add_up(up);
                    handle.add_down(down);
                    handle.finish(SessionOutcome::Completed);
                }
                Err(e) => handle.finish(SessionOutcome::Failed(e.to_string())),
            }
        })
    }
}

/// A TCP echo server on the loopback.
#[allow(dead_code)] // no caller until Task 5 wires a listener test onto this
pub(crate) async fn echo_server() -> SocketAddr {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if s.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    addr
}
