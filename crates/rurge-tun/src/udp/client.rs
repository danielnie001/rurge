//! The client of a UDP session, as the dialer's association sees it (phase 3 M3
//! design 7): the datagrams the app sent come out of `recv`, and what the flows
//! get back goes into the app's stack through `send`, with the source the app
//! will recognize. A flow the engine refused is told to the app by `refused`.

use super::{Queued, Session};
use crate::context::Context;
use crate::fragment::ip_packets;
use crate::icmp;
use crate::packet::{Reason, udp_datagram};
use crate::stats::Stats;
use rurge_config::HostName;
use rurge_inbound::UdpClient;
use rurge_net::BoxFuture;
use rurge_net::connector::Target;
use rurge_proto::RejectKind;
use smoltcp::wire::IpProtocol;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, PoisonError};
use std::time::Instant;
use tokio::sync::mpsc;

pub(crate) struct TunUdpClient {
    ctx: Arc<Context>,
    session: Arc<Session>,
    /// The datagrams of the app, in the order they came. The engine takes them
    /// from one task, and the lock is the only one held across an `await`. The
    /// ones still inside when the client is dropped are dropped with it, and
    /// give their bytes back.
    queued: tokio::sync::Mutex<mpsc::Receiver<Queued>>,
}

impl TunUdpClient {
    pub(crate) fn new(
        ctx: Arc<Context>,
        session: Arc<Session>,
        queued: mpsc::Receiver<Queued>,
    ) -> TunUdpClient {
        TunUdpClient {
            ctx,
            session,
            queued: tokio::sync::Mutex::new(queued),
        }
    }

    /// The endpoint a reply is to be shown as coming from (design 7.2): the one
    /// the app sent to, when the reply answers a flow the session has; or else
    /// what the reply says it came from, a name by the address the pool gave it.
    /// `None` for a reply that cannot be shown, which is dropped and counted.
    fn source_of(&self, from: &Target, answers: Option<&Target>) -> Option<SocketAddr> {
        let app = self.session.app;
        if let Some(answers) = answers
            && let Some(entry) = self.session.state().targets.by_target(answers)
        {
            entry.answered(Instant::now());
            return Some(entry.endpoint);
        }
        // a reply that answers no flow of the session is from where it says,
        // or from the flow it says it answers
        let flow = answers.unwrap_or(from);
        match &flow.host {
            HostName::Ip(ip) if ip.is_ipv4() == app.is_ipv4() => {
                Some(SocketAddr::new(*ip, flow.port))
            }
            // the packet could not be written: the app is of the other family
            HostName::Ip(_) => {
                Stats::drop(&self.ctx.stats, Reason::Unsupported, None, Some(app.ip()));
                None
            }
            HostName::Domain(name) => {
                let found = self
                    .ctx
                    .pool
                    .as_ref()
                    .and_then(|pool| pool.address_of(name, app.is_ipv6()));
                if found.is_none() {
                    Stats::drop(&self.ctx.stats, Reason::UnknownFake, None, Some(app.ip()));
                }
                found.map(|ip| SocketAddr::new(ip, flow.port))
            }
        }
    }
}

impl UdpClient for TunUdpClient {
    /// The next datagram of the app, with where it goes: the bytes are copied
    /// into `buf`, and what does not fit is cut off as a socket cuts it off. The
    /// bytes of the datagram go back to the budget of the queues once they are
    /// copied.
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let queued = self.queued.lock().await.recv().await.ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "the UDP session is over")
            })?;
            let len = queued.payload.len().min(buf.len());
            buf[..len].copy_from_slice(&queued.payload[..len]);
            // the rest of `queued`, the payload and the reservation, is dropped
            // as this ends
            Ok((len, queued.target))
        })
    }

    /// A reply to the app, which waits for room in the writer's queue: that is
    /// the back pressure on the engine's task that receives the replies. A
    /// reply that cannot be shown to the app is dropped and counted, and a
    /// reply that cannot be a datagram is an error, as it is for the SOCKS5
    /// client: it is lost alone.
    fn send<'a>(
        &'a self,
        payload: &'a [u8],
        from: &'a Target,
        answers: Option<&'a Target>,
    ) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            let Some(source) = self.source_of(from, answers) else {
                return Ok(());
            };
            let app = self.session.app;
            let datagram = udp_datagram(source, app, payload).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the reply is too long to be a datagram",
                )
            })?;
            let packets = ip_packets(
                source.ip(),
                app.ip(),
                IpProtocol::Udp,
                &datagram,
                self.ctx.mtu,
                &self.ctx.ids,
            );
            // what comes back keeps the session as what goes out does
            self.session.state().last = Instant::now();
            self.ctx
                .writer
                .send(packets)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the TUN has ended"))
        })
    }

    /// Tells the app that the flow to `target` was refused, unless the kind of
    /// refusal is to say nothing, or the limit on these messages is spent. It
    /// is called by the engine for each datagram it drops, synchronously, so it
    /// never waits.
    fn refused(&self, target: &Target, kind: RejectKind) {
        let limited = match kind {
            RejectKind::Drop => return,
            RejectKind::NoDrop => false,
            RejectKind::Reject | RejectKind::TinyGif => true,
        };
        let (endpoint, quote) = {
            let mut state = self.session.state();
            let Some(entry) = state.targets.by_target(target) else {
                return;
            };
            (entry.endpoint, entry.quote)
        };
        let app = self.session.app;
        if limited
            && !self
                .ctx
                .icmp_limit
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .allow(Instant::now())
        {
            Stats::drop(
                &self.ctx.stats,
                Reason::IcmpLimited,
                Some(endpoint.ip()),
                Some(app.ip()),
            );
            return;
        }
        icmp::refuse(&self.ctx, endpoint.ip(), app.ip(), quote.as_slice());
    }
}
