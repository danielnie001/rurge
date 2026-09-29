//! The UDP pipeline (phase 2 M5 design §5): the flows of one SOCKS5 UDP
//! association — one per destination, each with its own request record —
//! and the carriers they share, one per outbound. A carrier hands back
//! every datagram it receives, whoever sent it (full cone, M5-D2).

use crate::auto::{EVALUATION_FAILED, resolve_ready};
use crate::engine::{CONNECT_TIMEOUT, Chosen, Engine};
use rurge_config::policy::Builtin;
use rurge_config::session::SessionInfo;
use rurge_inbound::{SessionHandle, SessionOutcome, UdpClient};
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use rurge_policy::TerminalKind;
use rurge_policy::auto::SelectCtx;
use rurge_proto::{OutboundError, OutboundRef, RejectKind, UdpSupport};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;

/// A flow nothing has gone through for this long is reclaimed (M5-D7).
pub const FLOW_IDLE: Duration = Duration::from_secs(60);
/// A DNS flow (destination port 53) is reclaimed this long after its first
/// answer (M5-D7).
pub const DNS_LINGER: Duration = Duration::from_secs(10);
/// At most this many flows per association (M5-D11).
pub const FLOWS_PER_ASSOCIATION: usize = 1024;
/// At most this many associations at once (M5-D11).
pub const ASSOCIATIONS: usize = 4096;
/// Datagrams waiting for their flow to get going; more are dropped.
const QUEUE: usize = 64;
/// Room for the largest datagram.
const DATAGRAM: usize = 65536;

/// When a flow is reclaimed: `FLOW_IDLE` after its last datagram either
/// way, and a DNS flow `DNS_LINGER` after its first answer.
pub(crate) fn deadline(last: Instant, answered: Option<Instant>, port: u16) -> Instant {
    let idle = last + FLOW_IDLE;
    match answered {
        Some(at) if port == 53 => idle.min(at + DNS_LINGER),
        _ => idle,
    }
}

/// The clock the flows keep time by (tokio's, so tests can pause it).
fn now() -> Instant {
    tokio::time::Instant::now().into_std()
}

struct Times {
    last: Instant,
    answered: Option<Instant>,
}

/// One destination of an association, as its carrier sees it.
struct Flow {
    handle: Arc<SessionHandle>,
    port: u16,
    times: Mutex<Times>,
    /// Woken by an answer, which may bring the deadline forward.
    wake: tokio::sync::Notify,
}

impl Flow {
    fn new(handle: Arc<SessionHandle>, port: u16) -> Flow {
        Flow {
            handle,
            port,
            times: Mutex::new(Times {
                last: now(),
                answered: None,
            }),
            wake: tokio::sync::Notify::new(),
        }
    }

    fn touch(&self) {
        self.times.lock().expect("flow times").last = now();
    }

    fn answered(&self) {
        let now = now();
        {
            let mut times = self.times.lock().expect("flow times");
            times.last = now;
            times.answered.get_or_insert(now);
        }
        self.wake.notify_one();
    }

    /// Returns when the flow is due for reclaim, following the deadline as
    /// answers move it.
    async fn idle(&self) {
        loop {
            let wake = self.deadline();
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep_until(wake.into()) => {
                    if now() >= self.deadline() {
                        return;
                    }
                }
            }
        }
    }

    fn deadline(&self) -> Instant {
        let times = self.times.lock().expect("flow times");
        deadline(times.last, times.answered, self.port)
    }
}

/// Which flow a datagram coming back counts for: the one that sent to its
/// source, else the oldest flow still on the carrier (full cone).
#[derive(Default)]
struct Routes(Mutex<Vec<(Target, Weak<Flow>)>>);

impl Routes {
    fn add(&self, to: Target, flow: &Arc<Flow>) {
        let mut routes = self.0.lock().expect("routes");
        routes.retain(|(_, f)| f.strong_count() > 0);
        routes.push((to, Arc::downgrade(flow)));
    }

    fn remove(&self, flow: &Arc<Flow>) {
        let mut routes = self.0.lock().expect("routes");
        routes
            .retain(|(_, f)| f.strong_count() > 0 && !std::ptr::eq(f.as_ptr(), Arc::as_ptr(flow)));
    }

    fn flow_for(&self, from: &Target) -> Option<Arc<Flow>> {
        let routes = self.0.lock().expect("routes");
        routes
            .iter()
            .find(|(to, _)| to == from)
            .and_then(|(_, f)| f.upgrade())
            .or_else(|| routes.iter().find_map(|(_, f)| f.upgrade()))
    }
}

/// An outbound's carrier within one association, and the task handing its
/// datagrams back to the client.
struct Carrier {
    /// Keeps the outbound this carrier belongs to alive, so the cache key
    /// (its address) cannot be reused by another while the carrier is.
    outbound: OutboundRef,
    socket: Arc<dyn PacketSocket>,
    routes: Arc<Routes>,
    /// Fires when the receive task has ended: nothing comes back any more.
    dead: CancellationToken,
    _receive: AbortOnDropHandle<()>,
}

async fn receive(
    socket: Arc<dyn PacketSocket>,
    routes: Arc<Routes>,
    client: Arc<dyn UdpClient>,
    dead: CancellationToken,
) {
    let _dead = dead.drop_guard();
    let mut buf = vec![0u8; DATAGRAM];
    while let Ok((n, from)) = socket.recv_from(&mut buf).await {
        if let Some(flow) = routes.flow_for(&from) {
            flow.handle.add_down(n as u64);
            flow.handle.mark_first_byte();
            flow.answered();
        }
        if client.send(&buf[..n], &from).await.is_err() {
            return;
        }
    }
}

/// The association's carriers, one per outbound object, opened once each.
#[derive(Default)]
struct Carriers(Mutex<HashMap<usize, Arc<tokio::sync::Mutex<Weak<Carrier>>>>>);

impl Carriers {
    async fn get(
        &self,
        outbound: &OutboundRef,
        client: &Arc<dyn UdpClient>,
    ) -> Result<Arc<Carrier>, OutboundError> {
        let key = Arc::as_ptr(outbound) as *const () as usize;
        let slot = self
            .0
            .lock()
            .expect("carriers")
            .entry(key)
            .or_default()
            .clone();
        let mut held = slot.lock().await;
        if let Some(carrier) = held
            .upgrade()
            .filter(|c| Arc::ptr_eq(&c.outbound, outbound) && !c.dead.is_cancelled())
        {
            return Ok(carrier);
        }
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
        let socket: Arc<dyn PacketSocket> = Arc::from(outbound.open_udp(&opts).await?);
        let routes = Arc::new(Routes::default());
        let dead = CancellationToken::new();
        let task = tokio::spawn(receive(
            socket.clone(),
            routes.clone(),
            client.clone(),
            dead.clone(),
        ));
        let carrier = Arc::new(Carrier {
            outbound: outbound.clone(),
            socket,
            routes,
            dead,
            _receive: AbortOnDropHandle::new(task),
        });
        *held = Arc::downgrade(&carrier);
        Ok(carrier)
    }
}

/// What every flow of an association shares.
struct Association {
    client: Arc<dyn UdpClient>,
    template: SessionInfo,
    carriers: Carriers,
    /// Fires when the association ends: the flows finish.
    ended: CancellationToken,
    done: mpsc::UnboundedSender<(Target, u64)>,
}

/// Serves one association until the client's control connection ends
/// (`closed`) or the engine cancels every session.
pub(crate) async fn serve(
    engine: Arc<Engine>,
    client: Arc<dyn UdpClient>,
    template: SessionInfo,
    closed: CancellationToken,
) {
    let (done, mut finished) = mpsc::unbounded_channel();
    let association = Arc::new(Association {
        client: client.clone(),
        template,
        carriers: Carriers::default(),
        ended: engine.session_token(),
        done,
    });
    let mut flows: HashMap<Target, (u64, mpsc::Sender<Vec<u8>>)> = HashMap::new();
    let mut next = 0u64;
    let mut buf = vec![0u8; DATAGRAM];
    loop {
        tokio::select! {
            _ = closed.cancelled() => break,
            _ = association.ended.cancelled() => break,
            Some((to, id)) = finished.recv() => {
                if flows.get(&to).is_some_and(|(current, _)| *current == id) {
                    flows.remove(&to);
                }
            }
            got = client.recv(&mut buf) => {
                let Ok((n, to)) = got else { break };
                let mut datagram = buf[..n].to_vec();
                if let Some((_, queue)) = flows.get(&to) {
                    match queue.try_send(datagram) {
                        Ok(()) | Err(TrySendError::Full(_)) => continue,
                        // the flow is ending: this datagram starts a new one
                        Err(TrySendError::Closed(back)) => {
                            flows.remove(&to);
                            datagram = back;
                        }
                    }
                }
                if flows.len() >= FLOWS_PER_ASSOCIATION {
                    engine.warn_udp_limit("udp: too many flows on this association");
                    continue;
                }
                next += 1;
                let (queue, waiting) = mpsc::channel(QUEUE);
                flows.insert(to.clone(), (next, queue));
                engine.tracker().spawn(run_flow(
                    engine.clone(),
                    association.clone(),
                    to,
                    datagram,
                    waiting,
                    next,
                ));
            }
        }
    }
    association.ended.cancel();
}

async fn run_flow(
    engine: Arc<Engine>,
    association: Arc<Association>,
    to: Target,
    first: Vec<u8>,
    mut waiting: mpsc::Receiver<Vec<u8>>,
    id: u64,
) {
    let mut session = association.template.clone();
    session.dst_host = to.host.clone();
    session.dst_port = to.port;
    let handle = engine.new_handle(session);
    let opened = tokio::select! {
        _ = association.ended.cancelled() => Err(SessionOutcome::Completed),
        _ = handle.token().cancelled() => Err(SessionOutcome::Completed),
        opened = open(&engine, &association, &handle, &to) => opened,
    };
    match opened {
        Ok((flow, carrier, send_to)) => {
            let outcome =
                forward(&association, &flow, &carrier, &send_to, first, &mut waiting).await;
            carrier.routes.remove(&flow);
            waiting.close();
            handle.finish(outcome);
        }
        Err(outcome) => {
            handle.finish(outcome);
            // the flow stays, dropping what comes for it, until it is idle
            drain(&association, &handle, &mut waiting).await;
            waiting.close();
        }
    }
    let _ = association.done.send((to, id));
}

/// A failure, with whatever note is already on the record in front of it.
fn failed(handle: &SessionHandle, message: impl Into<String>) -> SessionOutcome {
    let message = message.into();
    if let Some(note) = handle.error() {
        handle.set_error(format!("{note}; {message}"));
    }
    SessionOutcome::Failed(message)
}

/// Rules → policy → the outbound's carrier (M5 design 5.2).
async fn open(
    engine: &Engine,
    association: &Association,
    handle: &Arc<SessionHandle>,
    to: &Target,
) -> Result<(Arc<Flow>, Arc<Carrier>, Target), SessionOutcome> {
    let (rt, registry) = engine.snapshot();
    let policy = match engine.choose_policy(&rt, &registry, handle).await {
        Chosen::Policy(p) => p,
        Chosen::DnsFailed => return Err(failed(handle, "dns lookup failed")),
    };
    let ctx = SelectCtx {
        host: Some(to.host.to_string()),
    };
    let resolution = match resolve_ready(&registry, &policy, &ctx).await {
        Ok(resolution) => resolution,
        Err(chain) => {
            handle.set_policy_chain(chain);
            return Err(failed(handle, EVALUATION_FAILED));
        }
    };
    handle.set_policy_chain(resolution.chain.clone());
    if let Some(note) = &resolution.note {
        handle.set_error(note.to_string());
    }
    if resolution.terminal == TerminalKind::Reject {
        return Err(SessionOutcome::Rejected(reject_kind(&resolution.chain)));
    }
    let outbound = resolution.outbound.clone();
    if outbound.udp() == UdpSupport::Unsupported {
        handle.set_error("policy does not support UDP");
        return Err(SessionOutcome::Rejected(RejectKind::Reject));
    }
    let carrier = association
        .carriers
        .get(&outbound, &association.client)
        .await
        .map_err(|e| failed(handle, e.to_string()))?;
    let send_to = carrier
        .socket
        .resolve(to)
        .await
        .map_err(|e| failed(handle, e.to_string()))?;
    handle.mark_connected();
    let flow = Arc::new(Flow::new(handle.clone(), to.port));
    carrier.routes.add(send_to.clone(), &flow);
    Ok((flow, carrier, send_to))
}

/// The REJECT flavour at the end of `chain`.
fn reject_kind(chain: &[String]) -> RejectKind {
    chain
        .last()
        .and_then(|name| Builtin::parse(name))
        .and_then(RejectKind::from_builtin)
        .unwrap_or(RejectKind::Reject)
}

/// Sends the flow's datagrams until it is idle or the association ends.
async fn forward(
    association: &Association,
    flow: &Flow,
    carrier: &Carrier,
    send_to: &Target,
    first: Vec<u8>,
    waiting: &mut mpsc::Receiver<Vec<u8>>,
) -> SessionOutcome {
    let mut next = Some(first);
    loop {
        if let Some(datagram) = next.take() {
            if let Err(e) = carrier.socket.send_to(&datagram, send_to).await {
                return failed(&flow.handle, e.to_string());
            }
            flow.handle.add_up(datagram.len() as u64);
            flow.touch();
        }
        tokio::select! {
            _ = association.ended.cancelled() => return SessionOutcome::Completed,
            _ = flow.handle.token().cancelled() => return SessionOutcome::Completed,
            _ = carrier.dead.cancelled() => {
                return failed(&flow.handle, "the outbound's UDP socket has closed");
            }
            got = waiting.recv() => match got {
                Some(datagram) => next = Some(datagram),
                None => return SessionOutcome::Completed,
            },
            _ = flow.idle() => return SessionOutcome::Completed,
        }
    }
}

/// Drops what comes for a flow that could not start, until it is idle.
async fn drain(
    association: &Association,
    handle: &SessionHandle,
    waiting: &mut mpsc::Receiver<Vec<u8>>,
) {
    let mut last = now();
    loop {
        tokio::select! {
            _ = association.ended.cancelled() => return,
            _ = handle.token().cancelled() => return,
            got = waiting.recv() => match got {
                Some(_) => last = now(),
                None => return,
            },
            _ = tokio::time::sleep_until((last + FLOW_IDLE).into()) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flow lives `FLOW_IDLE` past its last datagram; a DNS flow no longer
    /// than `DNS_LINGER` past its first answer.
    #[test]
    fn when_a_flow_is_reclaimed() {
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(5);
        assert_eq!(deadline(later, None, 443), later + FLOW_IDLE);
        assert_eq!(deadline(later, Some(t0), 443), later + FLOW_IDLE);
        assert_eq!(deadline(later, None, 53), later + FLOW_IDLE);
        assert_eq!(deadline(later, Some(t0), 53), t0 + DNS_LINGER);
        // a long-quiet DNS flow goes at its idle time, if that comes first
        let answered = t0 + Duration::from_secs(100);
        assert_eq!(deadline(t0, Some(answered), 53), t0 + FLOW_IDLE);
    }

    /// An answer to a DNS query brings the flow's end forward to
    /// `DNS_LINGER` later, though it was already waiting for `FLOW_IDLE`.
    #[tokio::test(start_paused = true)]
    async fn a_dns_flow_ends_soon_after_its_answer() {
        let flow = Arc::new(Flow::new(
            SessionHandle::new(
                1,
                SessionInfo::udp(rurge_config::HostName::parse("10.0.0.1"), 53),
            ),
            53,
        ));
        let waiting = tokio::spawn({
            let flow = flow.clone();
            async move { flow.idle().await }
        });
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!waiting.is_finished());
        flow.answered();
        tokio::time::sleep(DNS_LINGER - Duration::from_secs(1)).await;
        assert!(!waiting.is_finished());
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(waiting.is_finished(), "ended long before FLOW_IDLE");
    }

    /// Answers count for the flow that wrote to their source, or else the
    /// oldest flow still there.
    #[test]
    fn answers_find_their_flow() {
        let routes = Routes::default();
        let handle = |port| {
            SessionHandle::new(
                u64::from(port),
                SessionInfo::udp(rurge_config::HostName::parse("10.0.0.1"), port),
            )
        };
        let target = |port| Target::new(rurge_config::HostName::parse("10.0.0.1"), port);
        let a = Arc::new(Flow::new(handle(1), 1));
        let b = Arc::new(Flow::new(handle(2), 2));
        routes.add(target(1), &a);
        routes.add(target(2), &b);
        assert!(Arc::ptr_eq(&routes.flow_for(&target(2)).unwrap(), &b));
        assert!(Arc::ptr_eq(&routes.flow_for(&target(9)).unwrap(), &a));
        routes.remove(&a);
        assert!(Arc::ptr_eq(&routes.flow_for(&target(9)).unwrap(), &b));
        drop(b);
        assert!(routes.flow_for(&target(2)).is_none());
    }
}
