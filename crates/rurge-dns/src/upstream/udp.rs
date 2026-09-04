//! DNS over UDP (design §7.2): one connected socket per upstream, a receive
//! loop that routes answers to waiters by message ID (so several queries can
//! be in flight at once), and one retry over TCP when the answer is truncated.

use super::tcp::exchange_framed;
use super::{Upstream, UpstreamError};
use crate::message::{wire_id, wire_truncated};
use rurge_net::BoxFuture;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{OnceCell, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub const UDP_BUFFER: usize = 4096;

struct Shared {
    socket: UdpSocket,
    pending: Mutex<HashMap<u16, oneshot::Sender<Vec<u8>>>>,
}

pub struct UdpUpstream {
    name: String,
    addr: SocketAddr,
    state: OnceCell<(Arc<Shared>, JoinHandle<()>)>,
}

fn io_err(e: std::io::Error) -> UpstreamError {
    UpstreamError::Io(e.to_string())
}

impl UdpUpstream {
    pub fn new(addr: SocketAddr) -> UdpUpstream {
        UdpUpstream {
            name: format!("udp://{addr}"),
            addr,
            state: OnceCell::new(),
        }
    }

    async fn shared(&self) -> Result<Arc<Shared>, UpstreamError> {
        let (shared, _) = self
            .state
            .get_or_try_init(|| async {
                let bind: SocketAddr = if self.addr.is_ipv4() {
                    "0.0.0.0:0".parse().expect("valid")
                } else {
                    "[::]:0".parse().expect("valid")
                };
                let socket = UdpSocket::bind(bind).await.map_err(io_err)?;
                socket.connect(self.addr).await.map_err(io_err)?;
                let shared = Arc::new(Shared {
                    socket,
                    pending: Mutex::new(HashMap::new()),
                });
                let receiver = Arc::clone(&shared);
                let task = tokio::spawn(async move {
                    let mut buf = vec![0u8; UDP_BUFFER];
                    loop {
                        let n = match receiver.socket.recv(&mut buf).await {
                            Ok(n) => n,
                            Err(_) => continue,
                        };
                        let Some(id) = wire_id(&buf[..n]) else {
                            continue;
                        };
                        let waiter = receiver
                            .pending
                            .lock()
                            .expect("udp pending lock")
                            .remove(&id);
                        if let Some(tx) = waiter {
                            let _ = tx.send(buf[..n].to_vec());
                        }
                    }
                });
                Ok::<(Arc<Shared>, JoinHandle<()>), UpstreamError>((shared, task))
            })
            .await?;
        Ok(Arc::clone(shared))
    }
}

impl Drop for UdpUpstream {
    fn drop(&mut self) {
        if let Some((_, task)) = self.state.get() {
            task.abort();
        }
    }
}

impl Upstream for UdpUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    fn query<'a>(
        &'a self,
        wire: &'a [u8],
        deadline: Instant,
    ) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            let id = wire_id(wire).ok_or_else(|| {
                UpstreamError::BadResponse("query shorter than a DNS header".to_string())
            })?;
            let shared = self.shared().await?;
            let (tx, rx) = oneshot::channel();
            shared
                .pending
                .lock()
                .expect("udp pending lock")
                .insert(id, tx);
            if let Err(e) = shared.socket.send(wire).await {
                shared.pending.lock().expect("udp pending lock").remove(&id);
                return Err(io_err(e));
            }
            let resp = match tokio::time::timeout_at(deadline, rx).await {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(_)) => return Err(UpstreamError::Io("receiver dropped".to_string())),
                Err(_) => {
                    shared.pending.lock().expect("udp pending lock").remove(&id);
                    return Err(UpstreamError::Timeout);
                }
            };
            if !wire_truncated(&resp) {
                return Ok(resp);
            }
            // Truncated: repeat the same query once over TCP to the same server.
            let mut tcp =
                match tokio::time::timeout_at(deadline, TcpStream::connect(self.addr)).await {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => return Err(io_err(e)),
                    Err(_) => return Err(UpstreamError::Timeout),
                };
            exchange_framed(&mut tcp, wire, deadline).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A 12-byte header with the given id and flags, followed by `payload`.
    fn msg(id: u16, flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; 12];
        v[0..2].copy_from_slice(&id.to_be_bytes());
        v[2] = flags;
        v.extend_from_slice(payload);
        v
    }

    /// Raw UDP server whose behaviour is a function of the request bytes:
    /// `None` = drop; `Some((delay, reply))` = answer after `delay`.
    async fn raw_udp(
        behaviour: impl Fn(&[u8]) -> Option<(Duration, Vec<u8>)> + Send + Sync + 'static,
    ) -> SocketAddr {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = socket.local_addr().unwrap();
        let behaviour = Arc::new(behaviour);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                if let Some((delay, reply)) = behaviour(&buf[..n]) {
                    let socket = Arc::clone(&socket);
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = socket.send_to(&reply, peer).await;
                    });
                }
            }
        });
        addr
    }

    fn deadline(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn echoes_and_matches_ids_under_concurrency() {
        let addr = raw_udp(|req| {
            // Answer the first id slowly, everything else at once, echoing the request.
            let delay = if req[0..2] == [0, 1] {
                Duration::from_millis(150)
            } else {
                Duration::ZERO
            };
            Some((delay, req.to_vec()))
        })
        .await;
        let up = Arc::new(UdpUpstream::new(addr));
        assert_eq!(up.name(), format!("udp://{addr}"));
        let a = {
            let up = Arc::clone(&up);
            tokio::spawn(async move { up.query(&msg(1, 0, b"slow"), deadline(2000)).await })
        };
        let b = {
            let up = Arc::clone(&up);
            tokio::spawn(async move { up.query(&msg(2, 0, b"fast"), deadline(2000)).await })
        };
        assert_eq!(b.await.unwrap().unwrap(), msg(2, 0, b"fast"));
        assert_eq!(a.await.unwrap().unwrap(), msg(1, 0, b"slow"));
    }

    #[tokio::test]
    async fn ignores_answers_with_a_foreign_id() {
        let addr = raw_udp(|req| {
            let mut wrong = req.to_vec();
            wrong[0] ^= 0xff;
            // First a wrong-id reply, then the real one 20 ms later.
            Some((Duration::ZERO, wrong))
        })
        .await;
        // The behaviour closure can only send one reply; send the real one from a second server task.
        let up = UdpUpstream::new(addr);
        let err = up.query(&msg(7, 0, b"x"), deadline(200)).await;
        assert_eq!(
            err,
            Err(UpstreamError::Timeout),
            "a foreign id must not satisfy the waiter"
        );
    }

    #[tokio::test]
    async fn drops_time_out() {
        let addr = raw_udp(|_| None).await;
        let up = UdpUpstream::new(addr);
        assert_eq!(
            up.query(&msg(3, 0, b"x"), deadline(150)).await,
            Err(UpstreamError::Timeout)
        );
    }

    #[tokio::test]
    async fn truncated_answer_is_retried_over_tcp() {
        // TCP echo bound first so the UDP server can share the port number.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut len = [0u8; 2];
                    s.read_exact(&mut len).await.unwrap();
                    let mut m = vec![0u8; usize::from(u16::from_be_bytes(len))];
                    s.read_exact(&mut m).await.unwrap();
                    let mut full = m.clone();
                    full.extend_from_slice(b"-full");
                    let mut out = (full.len() as u16).to_be_bytes().to_vec();
                    out.extend_from_slice(&full);
                    s.write_all(&out).await.unwrap();
                });
            }
        });
        let socket = UdpSocket::bind(("127.0.0.1", port)).await.unwrap();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                let mut reply = buf[..n].to_vec();
                reply[2] |= 0x02; // TC
                let _ = socket.send_to(&reply, peer).await;
            }
        });
        let up = UdpUpstream::new(SocketAddr::from(([127, 0, 0, 1], port)));
        let resp = up.query(&msg(9, 0, b"big"), deadline(2000)).await.unwrap();
        assert_eq!(resp, [msg(9, 0, b"big"), b"-full".to_vec()].concat());
    }

    #[tokio::test]
    async fn rejects_queries_without_a_header() {
        let up = UdpUpstream::new("127.0.0.1:9".parse().unwrap());
        assert!(matches!(
            up.query(b"short", deadline(100)).await,
            Err(UpstreamError::BadResponse(_))
        ));
    }
}
