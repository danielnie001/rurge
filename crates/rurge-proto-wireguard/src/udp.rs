//! UDP through the tunnel (phase 2 M5 design §7): one socket of the tunnel's
//! stack for each address family, on the tunnel's address and a free port,
//! for datagrams to and from anywhere the peers take — full cone. Each
//! datagram leaves through the peer its destination routes to.

use crate::device::{Device, closed};
use crate::outbound::Names;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{PacketSocket, Target};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp;
use std::future::poll_fn;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Poll, Waker};

pub(crate) struct TunnelUdp {
    device: Arc<Device>,
    names: Arc<Names>,
    /// The IPv4 socket and the IPv6 one, once a datagram went to its family.
    sockets: Mutex<[Option<SocketHandle>; 2]>,
    /// Whoever waits to receive: woken when a socket opens.
    receiver: Mutex<Option<Waker>>,
}

fn slot(ip: IpAddr) -> usize {
    match ip {
        IpAddr::V4(_) => 0,
        IpAddr::V6(_) => 1,
    }
}

/// One of the carrier's own locks, poisoned or not. `send_to` holds them
/// together with the stack's: a panic there poisons all three, and what
/// these two guard — which sockets are open, who waits — is as valid after
/// it as before. The carrier then says what the stack's lock makes it say,
/// that the tunnel is over, instead of panicking in its turn.
fn lock<T>(of: &Mutex<T>) -> MutexGuard<'_, T> {
    of.lock().unwrap_or_else(PoisonError::into_inner)
}

impl TunnelUdp {
    pub(crate) fn new(device: Arc<Device>, names: Arc<Names>) -> TunnelUdp {
        TunnelUdp {
            device,
            names,
            sockets: Mutex::new([None, None]),
            receiver: Mutex::new(None),
        }
    }

    /// `to`'s address: a name is looked up as for a connection.
    async fn address(&self, to: &Target) -> io::Result<IpAddr> {
        match &to.host {
            HostName::Ip(ip) => Ok(*ip),
            HostName::Domain(_) => self
                .names
                .address(to, &self.device)
                .await
                .map_err(|e| io::Error::other(e.to_string())),
        }
    }
}

impl PacketSocket for TunnelUdp {
    fn resolve<'a>(&'a self, to: &'a Target) -> BoxFuture<'a, io::Result<Target>> {
        Box::pin(async move {
            let ip = self.address(to).await?;
            Ok(Target::new(HostName::Ip(ip), to.port))
        })
    }

    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            if self.device.is_closed() {
                return Err(closed());
            }
            let ip = self.address(to).await?;
            {
                // the tunnel may have ended while the name was looked up
                let mut stack = self.device.shared.running().map_err(|_| closed())?;
                stack.check(ip).map_err(|refusal| {
                    io::Error::new(io::ErrorKind::Unsupported, refusal.to_string())
                })?;
                let mut sockets = lock(&self.sockets);
                let handle = match sockets[slot(ip)] {
                    Some(handle) => handle,
                    None => {
                        let handle = stack.udp_bind(ip).map_err(|refusal| {
                            io::Error::new(io::ErrorKind::AddrNotAvailable, refusal.to_string())
                        })?;
                        sockets[slot(ip)] = Some(handle);
                        // the receiver waits on the sockets it knew of
                        if let Some(waker) = lock(&self.receiver).take() {
                            waker.wake();
                        }
                        handle
                    }
                };
                match stack
                    .udp(handle)
                    .send_slice(buf, SocketAddr::new(ip, to.port))
                {
                    Ok(()) => {}
                    // no room left: dropped, as a full socket drops it
                    Err(udp::SendError::BufferFull) => return Ok(()),
                    Err(udp::SendError::Unaddressable) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrNotAvailable,
                            format!("wireguard: {ip} cannot be sent to"),
                        ));
                    }
                }
            }
            self.device.shared.kick();
            Ok(())
        })
    }

    /// A datagram longer than `buf` is cut to it: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            // the tunnel's end wakes whoever waits on a socket; a carrier
            // that has sent nothing has none, and waits on this as well
            let mut ended = pin!(self.device.shared.ended());
            poll_fn(|cx| {
                let _ = ended.as_mut().poll(cx);
                // told of a socket that opens from here on
                *lock(&self.receiver) = Some(cx.waker().clone());
                let handles = *lock(&self.sockets);
                let mut stack = self.device.shared.stack().map_err(|_| closed())?;
                for handle in handles.into_iter().flatten() {
                    let socket = stack.udp(handle);
                    if let Ok((datagram, meta)) = socket.recv() {
                        let n = datagram.len().min(buf.len());
                        buf[..n].copy_from_slice(&datagram[..n]);
                        let from = Target::new(
                            HostName::Ip(meta.endpoint.addr.into()),
                            meta.endpoint.port,
                        );
                        return Poll::Ready(Ok((n, from)));
                    }
                    socket.register_recv_waker(cx.waker());
                }
                // after the wakers are in: an end that races this poll
                // wakes them
                if self.device.is_closed() {
                    return Poll::Ready(Err(closed()));
                }
                Poll::Pending
            })
            .await
        })
    }
}

impl Drop for TunnelUdp {
    fn drop(&mut self) {
        let handles = *lock(&self.sockets);
        if let Ok(mut stack) = self.device.shared.stack() {
            for handle in handles.into_iter().flatten() {
                stack.udp_close(handle);
            }
        }
    }
}
