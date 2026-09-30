//! UDP through the tunnel (phase 2 M5 design §7): one socket of the tunnel's
//! stack for each address family, on the tunnel's address and a free port,
//! for datagrams to and from anywhere the peers take — full cone. Each
//! datagram leaves through the peer its destination routes to.

use crate::device::Device;
use crate::outbound::Names;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{PacketSocket, Target};
use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp;
use std::future::poll_fn;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
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

/// What a carrier of a tunnel that ended says: the flow fails, as a TCP
/// connection does, and the carrier is not used again.
fn closed() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "wireguard: the tunnel has closed",
    )
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
                let mut stack = self.device.shared.stack.lock().expect("the tunnel");
                stack.check(ip).map_err(|refusal| {
                    io::Error::new(io::ErrorKind::Unsupported, refusal.to_string())
                })?;
                let mut sockets = self.sockets.lock().expect("the sockets");
                let handle = match sockets[slot(ip)] {
                    Some(handle) => handle,
                    None => {
                        let handle = stack.udp_bind(ip).map_err(|refusal| {
                            io::Error::new(io::ErrorKind::AddrNotAvailable, refusal.to_string())
                        })?;
                        sockets[slot(ip)] = Some(handle);
                        // the receiver waits on the sockets it knew of
                        if let Some(waker) = self.receiver.lock().expect("the receiver").take() {
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
        Box::pin(poll_fn(move |cx| {
            // told of a socket that opens from here on
            *self.receiver.lock().expect("the receiver") = Some(cx.waker().clone());
            let handles = *self.sockets.lock().expect("the sockets");
            let mut stack = self.device.shared.stack.lock().expect("the tunnel");
            for handle in handles.into_iter().flatten() {
                let socket = stack.udp(handle);
                if let Ok((datagram, meta)) = socket.recv() {
                    let n = datagram.len().min(buf.len());
                    buf[..n].copy_from_slice(&datagram[..n]);
                    let from =
                        Target::new(HostName::Ip(meta.endpoint.addr.into()), meta.endpoint.port);
                    return Poll::Ready(Ok((n, from)));
                }
                socket.register_recv_waker(cx.waker());
            }
            // after the wakers are in: a close that races this poll wakes them
            if self.device.is_closed() {
                return Poll::Ready(Err(closed()));
            }
            Poll::Pending
        }))
    }
}

impl Drop for TunnelUdp {
    fn drop(&mut self) {
        let handles = *self.sockets.lock().expect("the sockets");
        if let Ok(mut stack) = self.device.shared.stack.lock() {
            for handle in handles.into_iter().flatten() {
                stack.udp_close(handle);
            }
        }
    }
}
