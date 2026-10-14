//! The UDP pipeline (phase 2 M5 design §5): the flows of one SOCKS5 UDP
//! association — one per destination, each with its own request record —
//! and the carriers they share, one per outbound. A carrier hands back
//! every datagram it receives, whoever sent it (full cone, M5-D2).

use crate::auto::{EVALUATION_FAILED, resolve_ready};
use crate::engine::{CONNECT_TIMEOUT, Chosen, Engine, REFUSED_STRICT};
use rurge_config::HostName;
use rurge_config::general::{BlockQuicGlobal, UdpFallback};
use rurge_config::policy::Builtin;
use rurge_config::rule::{PolicyRef, ProtocolKind};
use rurge_config::session::SessionInfo;
use rurge_config::spec::Tristate;
use rurge_inbound::{SessionHandle, SessionOutcome, UdpClient};
use rurge_net::connector::{ConnectOpts, PacketSocket, Target, lost_alone};
use rurge_policy::TerminalKind;
use rurge_policy::auto::SelectCtx;
use rurge_proto::{OutboundRef, RejectKind, UdpSupport};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
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
/// A flow whose outbound failed to open, or whose datagrams stopped going
/// out, drops what comes for it this long after the failure — however much
/// keeps coming — and then ends, so the next datagram starts a new flow
/// that is routed and opened afresh.
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// Datagrams waiting for their flow to get going; more are dropped.
const QUEUE: usize = 64;
/// What the flows of one association may hold in all, waiting for their
/// carrier or their turn to go out; more is dropped.
const QUEUED_BYTES: usize = 1 << 20;
/// A send to the outbound not done by then is given up: its stream is stuck
/// behind a peer that reads no more. A send may first dial the target's own
/// connection (vmess, h2-connect).
const SEND_TIMEOUT: Duration = CONNECT_TIMEOUT.saturating_add(Duration::from_secs(5));
/// Room for the largest datagram.
const DATAGRAM: usize = 65536;

/// Whether a QUIC flow is blocked. The global setting overrides the terminal
/// policy's own `block-quic`: `all` blocks every flow, `always-allow` none, and
/// `all-proxy` every flow through a proxy — the manual has it override the
/// proxy policies' parameter, so a DIRECT-kind policy (`DIRECT`, a `direct`
/// alias) keeps its own. `per-policy` asks the policy for every flow; its
/// `auto` blocks proxies and lets DIRECT through.
pub(crate) fn quic_blocked(global: BlockQuicGlobal, policy: Tristate, direct: bool) -> bool {
    match global {
        BlockQuicGlobal::AlwaysAllow => false,
        BlockQuicGlobal::All => true,
        BlockQuicGlobal::AllProxy => !direct || policy == Tristate::On,
        BlockQuicGlobal::PerPolicy => match policy {
            Tristate::On => true,
            Tristate::Off => false,
            Tristate::Auto => !direct,
        },
    }
}

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
/// source; else the oldest flow sent to a name on its port (the server
/// looked the name up: the answer comes from the address); else the oldest
/// flow still on the carrier (full cone).
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
        let oldest = |sent_to: &dyn Fn(&Target) -> bool| {
            routes
                .iter()
                .filter(|(to, _)| sent_to(to))
                .find_map(|(_, f)| f.upgrade())
        };
        oldest(&|to| to == from)
            .or_else(|| {
                oldest(&|to| matches!(to.host, HostName::Domain(_)) && to.port == from.port)
            })
            .or_else(|| oldest(&|_| true))
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
    /// Fires when the receive task has ended — nothing comes back any more —
    /// or a send was given up: the carrier is not used again.
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
        // a server whose socket is dual-stack reports an IPv4 source as
        // `::ffff:a.b.c.d`; the flows are told apart by the IPv4 address the
        // client named, and the client is told the same
        let from = Target::new(from.host.canonical(), from.port);
        if let Some(flow) = routes.flow_for(&from) {
            flow.handle.add_down(n as u64);
            flow.handle.mark_first_byte();
            flow.answered();
        }
        // one datagram the client cannot take (too long once the SOCKS5
        // header is on, a full buffer) is lost alone: anyone may send to
        // the carrier, and the association lives by its control connection
        if let Err(e) = client.send(&buf[..n], &from).await {
            tracing::debug!(error = %e, "udp: a datagram for the client is dropped");
        }
    }
}

/// The association's carriers, one per outbound object, opened once each.
#[derive(Default)]
struct Carriers(Mutex<HashMap<usize, Arc<Slot>>>);

/// One outbound's carrier within the association: opened by one flow at a
/// time, while the others wait.
#[derive(Default)]
struct Slot {
    /// How many openings have failed.
    failures: AtomicU64,
    held: tokio::sync::Mutex<Held>,
}

#[derive(Default)]
struct Held {
    carrier: Weak<Carrier>,
    /// Why the last opening failed.
    why: String,
}

impl Carriers {
    /// The carrier of `outbound`, opened now unless there is one; `Err`
    /// says why it could not be opened.
    async fn get(
        &self,
        outbound: &OutboundRef,
        client: &Arc<dyn UdpClient>,
    ) -> Result<Arc<Carrier>, String> {
        let key = Arc::as_ptr(outbound) as *const () as usize;
        let slot = self
            .0
            .lock()
            .expect("carriers")
            .entry(key)
            .or_default()
            .clone();
        let failures = slot.failures.load(Ordering::Acquire);
        let mut held = slot.held.lock().await;
        if let Some(carrier) = held
            .carrier
            .upgrade()
            .filter(|c| Arc::ptr_eq(&c.outbound, outbound) && !c.dead.is_cancelled())
        {
            return Ok(carrier);
        }
        // the opening this flow waited for failed: so does the flow — a try
        // of its own would hold up every flow behind it as long again
        if slot.failures.load(Ordering::Acquire) != failures {
            return Err(held.why.clone());
        }
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
        let socket: Arc<dyn PacketSocket> = match outbound.open_udp(&opts).await {
            Ok(socket) => Arc::from(socket),
            Err(e) => {
                held.why = e.to_string();
                slot.failures.fetch_add(1, Ordering::Release);
                return Err(held.why.clone());
            }
        };
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
        held.carrier = Arc::downgrade(&carrier);
        Ok(carrier)
    }
}

/// A datagram from the client on its way out: its bytes count against the
/// association's `QUEUED_BYTES` until it is gone.
struct Queued {
    datagram: Vec<u8>,
    queued: Arc<AtomicUsize>,
}

impl Queued {
    /// `None`: the association holds as much as it may.
    fn new(datagram: Vec<u8>, queued: &Arc<AtomicUsize>) -> Option<Queued> {
        let len = datagram.len();
        if queued.fetch_add(len, Ordering::AcqRel) + len > QUEUED_BYTES {
            queued.fetch_sub(len, Ordering::AcqRel);
            return None;
        }
        Some(Queued {
            datagram,
            queued: queued.clone(),
        })
    }
}

impl Drop for Queued {
    fn drop(&mut self) {
        self.queued.fetch_sub(self.datagram.len(), Ordering::AcqRel);
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
    // the flows end with the association, however this future ends
    let _ended = association.ended.clone().drop_guard();
    let mut flows: HashMap<Target, (u64, mpsc::Sender<Queued>)> = HashMap::new();
    let queued = Arc::new(AtomicUsize::new(0));
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
                let Some(mut datagram) = Queued::new(buf[..n].to_vec(), &queued) else {
                    engine.warn_udp_limit("udp: an association holds 1 MiB waiting to go out; datagrams are dropped");
                    continue;
                };
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
                    engine.warn_udp_limit("udp: too many flows on this association; new flows are dropped");
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
}

async fn run_flow(
    engine: Arc<Engine>,
    association: Arc<Association>,
    to: Target,
    first: Queued,
    mut waiting: mpsc::Receiver<Queued>,
    id: u64,
) {
    let mut session = association.template.clone();
    session.dst_host = to.host.clone();
    session.dst_port = to.port;
    // the rules see what the first datagram is (`PROTOCOL,QUIC`)
    session.protocol = Some(
        if to.port == 443 && crate::sniff::is_quic_initial(&first.datagram) {
            ProtocolKind::Quic
        } else {
            ProtocolKind::Udp
        },
    );
    let handle = engine.new_handle(session);
    let trail = handle.trail().clone();
    let opened = tokio::select! {
        _ = association.ended.cancelled() => Err(SessionOutcome::Completed),
        _ = handle.token().cancelled() => Err(SessionOutcome::Completed),
        opened = trail.follow(open(&engine, &association, &handle, &to)) => opened,
    };
    match opened {
        Ok((flow, carrier, send_to)) => {
            let outcome =
                forward(&association, &flow, &carrier, &send_to, first, &mut waiting).await;
            carrier.routes.remove(&flow);
            // a flow whose datagrams stopped going out is tried again after
            // `RETRY_AFTER`, as one that could not open
            let retry = matches!(outcome, SessionOutcome::Failed(_)).then(|| now() + RETRY_AFTER);
            handle.finish(outcome);
            if retry.is_some() {
                drain(&association, &handle, &mut waiting, retry).await;
            }
            waiting.close();
        }
        Err(outcome) => {
            // it holds no room while the flow stays
            drop(first);
            // a refused flow stays until it is idle; a failed one is tried
            // again after `RETRY_AFTER`
            let retry = matches!(outcome, SessionOutcome::Failed(_)).then(|| now() + RETRY_AFTER);
            handle.finish(outcome);
            drain(&association, &handle, &mut waiting, retry).await;
            waiting.close();
        }
    }
    let _ = association.done.send((to, id));
}

/// A failure, with whatever note is already on the record in front of it.
pub(crate) fn failed(handle: &SessionHandle, message: impl Into<String>) -> SessionOutcome {
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
    // Refused before anything is asked of the rules, a lookup or a `smart`
    // group, and as a failure, not a rejection: a failed flow is tried again
    // after `RETRY_AFTER`, so it is served as soon as the suspension ends,
    // where a rejected one stays until its client has been quiet for
    // `FLOW_IDLE`.
    if engine.is_suspended() {
        return Err(failed(handle, REFUSED_STRICT));
    }
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
    let direct = resolution.terminal == TerminalKind::Direct;
    let setting = resolution
        .chain
        .last()
        .and_then(|name| registry.spec(name))
        .map(|spec| spec.common.block_quic)
        .unwrap_or_default();
    if handle.session().protocol == Some(ProtocolKind::Quic)
        && quic_blocked(rt.config.general.block_quic, setting, direct)
    {
        // the browser falls back to TCP
        handle.set_error("QUIC blocked");
        return Err(SessionOutcome::Rejected(RejectKind::Reject));
    }
    let mut outbound = resolution.outbound.clone();
    // a `smart` group's member tells the book how it did (phase 2 M5
    // design 8.3); not when the flow goes through DIRECT in its place, nor
    // when its target is a name no member can send (phase 2 M8 design 5.5)
    let mut smart = resolution
        .smart
        .clone()
        .filter(|_| !crate::smart::unsendable(to));
    if outbound.udp() == UdpSupport::Unsupported {
        match rt.config.general.udp_policy_not_supported_behaviour {
            UdpFallback::Reject => {
                handle.set_error("policy does not support UDP");
                return Err(SessionOutcome::Rejected(RejectKind::Reject));
            }
            UdpFallback::Direct => {
                handle.set_error("policy does not support UDP; sent through DIRECT");
                smart = None;
                outbound = registry
                    .resolve_with(&PolicyRef::Builtin(Builtin::Direct), &ctx)
                    .outbound;
            }
        }
    }
    let host = to.host.to_string();
    let book = &registry.auto().smart;
    let carrier = match association
        .carriers
        .get(&outbound, &association.client)
        .await
    {
        Ok(carrier) => carrier,
        Err(e) => {
            if let Some(pick) = &smart {
                book.report_failure(&pick.member, &outbound, Some(&host), Instant::now());
            }
            return Err(failed(handle, e.to_string()));
        }
    };
    // within the dial's time, as a TCP session's: a tunnel's DNS waits for
    // the tunnel to come up first
    let send_to = tokio::time::timeout(CONNECT_TIMEOUT, carrier.socket.resolve(to))
        .await
        .unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("looking up {} timed out", to.host),
            ))
        })
        .map_err(|e| failed(handle, e.to_string()))?;
    handle.mark_connected();
    if let Some(pick) = &smart {
        book.used(&pick.group, &pick.member, Instant::now());
        crate::smart::watch(
            handle,
            book.clone(),
            &pick.member,
            &outbound,
            &host,
            silence_tells(&send_to),
        );
    }
    let flow = Arc::new(Flow::new(handle.clone(), to.port));
    carrier.routes.add(send_to.clone(), &flow);
    Ok((flow, carrier, send_to))
}

/// Whether three silent seconds of a flow sending to `send_to` count
/// against its `smart` member: only where an answer always comes — DNS, and
/// the port QUIC answers on; a game may only send — and only to an address:
/// the answers for a name that the server looks up may count for another
/// flow on the same port.
fn silence_tells(send_to: &Target) -> bool {
    matches!(send_to.port, 53 | 443) && matches!(send_to.host, HostName::Ip(_))
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
    first: Queued,
    waiting: &mut mpsc::Receiver<Queued>,
) -> SessionOutcome {
    let mut next = Some(first);
    loop {
        if let Some(datagram) = next.take() {
            // the end of the association cuts a send short (its carriers go
            // with it), a kill waits for it: one cut short may leave half a
            // datagram on a stream the other flows share
            let sending = tokio::time::timeout(
                SEND_TIMEOUT,
                carrier.socket.send_to(&datagram.datagram, send_to),
            );
            let sent = tokio::select! {
                _ = association.ended.cancelled() => return SessionOutcome::Completed,
                sent = sending => sent,
            };
            match sent {
                Ok(Ok(())) => {
                    flow.handle.add_up(datagram.datagram.len() as u64);
                    flow.touch();
                }
                Ok(Err(e)) if lost_alone(&e) => {
                    tracing::debug!(error = %e, "udp: a datagram for the target is dropped");
                    if flow.handle.error().is_none() {
                        flow.handle.set_error(e.to_string());
                    }
                }
                Ok(Err(e)) => return upstream_failed(&flow.handle, e.to_string()),
                Err(_) => {
                    carrier.dead.cancel();
                    return upstream_failed(&flow.handle, "sending to the outbound timed out");
                }
            }
        }
        tokio::select! {
            _ = association.ended.cancelled() => return SessionOutcome::Completed,
            _ = flow.handle.token().cancelled() => return SessionOutcome::Completed,
            _ = carrier.dead.cancelled() => {
                return upstream_failed(&flow.handle, "the outbound's UDP socket has closed");
            }
            got = waiting.recv() => match got {
                Some(datagram) => next = Some(datagram),
                None => return SessionOutcome::Completed,
            },
            _ = flow.idle() => return SessionOutcome::Completed,
        }
    }
}

/// The flow failed upstream: a `smart` group's member answers for it.
pub(crate) fn upstream_failed(
    handle: &SessionHandle,
    message: impl Into<String>,
) -> SessionOutcome {
    handle.mark_upstream_failed();
    failed(handle, message)
}

/// Drops what comes for a flow that could not start, or whose datagrams
/// stopped going out, until it is idle — or until `retry`, whatever comes in
/// the meantime.
async fn drain(
    association: &Association,
    handle: &SessionHandle,
    waiting: &mut mpsc::Receiver<Queued>,
    retry: Option<Instant>,
) {
    let mut last = now();
    loop {
        let end = retry.unwrap_or(last + FLOW_IDLE);
        tokio::select! {
            _ = association.ended.cancelled() => return,
            _ = handle.token().cancelled() => return,
            got = waiting.recv() => match got {
                Some(_) => last = now(),
                None => return,
            },
            _ = tokio::time::sleep_until(end.into()) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::BoxFuture;
    use rurge_proto::OutboundError;

    #[test]
    fn which_quic_flows_are_blocked() {
        use BlockQuicGlobal::*;
        use Tristate::*;
        // (global, policy's block-quic, terminal is DIRECT) → blocked
        for (global, policy, direct, blocked) in [
            (PerPolicy, Auto, false, true),
            (PerPolicy, Auto, true, false),
            (PerPolicy, On, true, true),
            (PerPolicy, Off, false, false),
            (AllProxy, Off, false, true),
            (AllProxy, On, true, true),
            (AllProxy, Auto, true, false),
            (AllProxy, Off, true, false),
            (All, Off, true, true),
            (AlwaysAllow, On, false, false),
        ] {
            assert_eq!(
                quic_blocked(global, policy, direct),
                blocked,
                "{global:?} {policy:?} direct={direct}"
            );
        }
    }

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

    /// An answer from an address no flow sent to counts for the oldest flow
    /// sent to a name on its port — the server looked the name up — before
    /// the oldest flow of all.
    #[test]
    fn answers_for_a_name_find_a_flow_on_their_port() {
        let routes = Routes::default();
        let flow = |port| {
            Arc::new(Flow::new(
                SessionHandle::new(
                    u64::from(port),
                    SessionInfo::udp(HostName::parse("10.0.0.1"), port),
                ),
                port,
            ))
        };
        let (a, b, c) = (flow(1), flow(53), flow(443));
        routes.add(Target::new(HostName::parse("10.0.0.1"), 1), &a);
        routes.add(Target::new(HostName::parse("dns.test"), 53), &b);
        routes.add(Target::new(HostName::parse("quic.test"), 443), &c);
        let from = |port| Target::new(HostName::parse("10.0.0.9"), port);
        assert!(Arc::ptr_eq(&routes.flow_for(&from(443)).unwrap(), &c));
        assert!(Arc::ptr_eq(&routes.flow_for(&from(53)).unwrap(), &b));
        assert!(Arc::ptr_eq(&routes.flow_for(&from(7)).unwrap(), &a));
    }

    /// A carrier that hears one datagram, from `from`, and nothing after it.
    struct HearsOnce(Mutex<Option<(Vec<u8>, Target)>>);

    impl PacketSocket for HearsOnce {
        fn send_to<'a>(&'a self, _: &'a [u8], _: &'a Target) -> BoxFuture<'a, io::Result<()>> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn recv_from<'a>(
            &'a self,
            buf: &'a mut [u8],
        ) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            Box::pin(std::future::ready(match self.0.lock().unwrap().take() {
                Some((payload, from)) => {
                    buf[..payload.len()].copy_from_slice(&payload);
                    Ok((payload.len(), from))
                }
                // ends `receive`
                None => Err(io::ErrorKind::ConnectionAborted.into()),
            }))
        }
    }

    /// A client that keeps what it is sent.
    #[derive(Default)]
    struct Hears(Mutex<Vec<(Vec<u8>, Target)>>);

    impl UdpClient for Hears {
        fn recv<'a>(&'a self, _: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            Box::pin(std::future::pending())
        }

        fn send<'a>(
            &'a self,
            payload: &'a [u8],
            from: &'a Target,
        ) -> BoxFuture<'a, io::Result<()>> {
            self.0
                .lock()
                .unwrap()
                .push((payload.to_vec(), from.clone()));
            Box::pin(std::future::ready(Ok(())))
        }
    }

    /// A server whose socket is dual-stack reports the source of an answer
    /// from an IPv4 host as `::ffff:a.b.c.d`. The flow that sent to the IPv4
    /// address owns that answer — not the oldest flow of the carrier — and the
    /// client hears it from the IPv4 address.
    #[tokio::test]
    async fn an_answer_from_a_source_in_ipv6_form_finds_its_flow() {
        let flow = |port: u16| {
            Arc::new(Flow::new(
                SessionHandle::new(
                    u64::from(port),
                    SessionInfo::udp(HostName::parse("10.0.0.1"), port),
                ),
                port,
            ))
        };
        let (oldest, wanted) = (flow(9), flow(53));
        let routes = Arc::new(Routes::default());
        routes.add(Target::new(HostName::parse("10.0.0.9"), 9), &oldest);
        routes.add(Target::new(HostName::parse("10.0.0.1"), 53), &wanted);
        let client = Arc::new(Hears::default());
        let socket = HearsOnce(Mutex::new(Some((
            b"answer".to_vec(),
            Target::new(HostName::parse("::ffff:10.0.0.1"), 53),
        ))));
        receive(
            Arc::new(socket),
            routes,
            client.clone(),
            CancellationToken::new(),
        )
        .await;
        assert_eq!((oldest.handle.bytes().1, wanted.handle.bytes().1), (0, 6));
        assert_eq!(
            *client.0.lock().unwrap(),
            [(
                b"answer".to_vec(),
                Target::new(HostName::parse("10.0.0.1"), 53)
            )]
        );
    }

    /// A carrier whose sends never finish: a stream stuck behind a peer that
    /// reads no more.
    struct Stuck;

    impl PacketSocket for Stuck {
        fn send_to<'a>(&'a self, _: &'a [u8], _: &'a Target) -> BoxFuture<'a, io::Result<()>> {
            Box::pin(std::future::pending())
        }

        fn recv_from<'a>(&'a self, _: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            Box::pin(std::future::pending())
        }
    }

    struct NoClient;

    impl UdpClient for NoClient {
        fn recv<'a>(&'a self, _: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            Box::pin(std::future::pending())
        }

        fn send<'a>(&'a self, _: &'a [u8], _: &'a Target) -> BoxFuture<'a, io::Result<()>> {
            Box::pin(std::future::ready(Ok(())))
        }
    }

    fn queued(datagram: &[u8]) -> Queued {
        Queued::new(datagram.to_vec(), &Arc::default()).expect("room")
    }

    /// An association, one of its flows and a carrier that never sends.
    fn stuck() -> (Association, Arc<Flow>, Carrier, Target) {
        let to = Target::new(HostName::parse("10.0.0.1"), 9);
        let session = SessionInfo::udp(to.host.clone(), to.port);
        let association = Association {
            client: Arc::new(NoClient),
            template: session.clone(),
            carriers: Carriers::default(),
            ended: CancellationToken::new(),
            done: mpsc::unbounded_channel().0,
        };
        let flow = Arc::new(Flow::new(SessionHandle::new(1, session), to.port));
        let carrier = Carrier {
            outbound: Arc::new(rurge_proto::Reject::new(RejectKind::Reject)),
            socket: Arc::new(Stuck),
            routes: Arc::default(),
            dead: CancellationToken::new(),
            _receive: AbortOnDropHandle::new(tokio::spawn(std::future::pending())),
        };
        (association, flow, carrier, to)
    }

    /// A send that does not finish in time fails the flow, and the carrier
    /// — whose stream may hold half a datagram now — is not used again.
    #[tokio::test(start_paused = true)]
    async fn a_send_that_hangs_fails_the_flow_and_its_carrier() {
        let (association, flow, carrier, to) = stuck();
        let (_queue, mut waiting) = mpsc::channel(QUEUE);
        let sending = forward(
            &association,
            &flow,
            &carrier,
            &to,
            queued(b"x"),
            &mut waiting,
        );
        let outcome = tokio::time::timeout(Duration::from_secs(60), sending)
            .await
            .expect("the send gives up");
        assert!(matches!(outcome, SessionOutcome::Failed(_)), "{outcome:?}");
        assert!(flow.handle.upstream_failed());
        assert!(carrier.dead.is_cancelled());
    }

    /// The end of the association does not wait for a send that hangs.
    #[tokio::test(start_paused = true)]
    async fn the_end_of_the_association_cuts_a_send_short() {
        let (association, flow, carrier, to) = stuck();
        let (_queue, mut waiting) = mpsc::channel(QUEUE);
        association.ended.cancel();
        let sending = forward(
            &association,
            &flow,
            &carrier,
            &to,
            queued(b"x"),
            &mut waiting,
        );
        let outcome = tokio::time::timeout(Duration::from_secs(1), sending)
            .await
            .expect("the flow ends at once");
        assert_eq!(outcome, SessionOutcome::Completed);
    }

    /// An outbound whose carrier fails to open after `delay`; counts the
    /// tries.
    struct SlowToFail {
        delay: Duration,
        tries: std::sync::atomic::AtomicUsize,
    }

    impl rurge_proto::Outbound for SlowToFail {
        fn name(&self) -> &str {
            "Slow"
        }

        fn connect_tcp<'a>(
            &'a self,
            _: &'a Target,
            _: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<rurge_net::connector::BoxedStream, OutboundError>> {
            Box::pin(std::future::ready(Err(OutboundError::Timeout)))
        }

        fn udp(&self) -> UdpSupport {
            UdpSupport::Native
        }

        fn open_udp<'a>(
            &'a self,
            _: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<rurge_net::connector::BoxedPacketSocket, OutboundError>> {
            Box::pin(async move {
                self.tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tokio::time::sleep(self.delay).await;
                Err(OutboundError::Timeout)
            })
        }
    }

    /// The flows waiting on one opening of a carrier take what came of it:
    /// when it fails they fail with it, rather than each after a try of its
    /// own (the k-th waiting k times as long). A flow that comes after the
    /// failure tries again.
    #[tokio::test(start_paused = true)]
    async fn the_flows_waiting_on_an_opening_that_fails_fail_with_it() {
        let carriers = Carriers::default();
        let slow = Arc::new(SlowToFail {
            delay: Duration::from_secs(10),
            tries: Default::default(),
        });
        let outbound: OutboundRef = slow.clone();
        let client: Arc<dyn UdpClient> = Arc::new(NoClient);
        let begun = tokio::time::Instant::now();
        let (a, b, c) = tokio::join!(
            carriers.get(&outbound, &client),
            carriers.get(&outbound, &client),
            carriers.get(&outbound, &client),
        );
        assert!(a.is_err() && b.is_err() && c.is_err());
        assert_eq!(begun.elapsed(), Duration::from_secs(10));
        let tries = || slow.tries.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(tries(), 1);
        assert!(carriers.get(&outbound, &client).await.is_err());
        assert_eq!(tries(), 2);
    }

    /// Three silent seconds tell on DNS and QUIC's port, and only for a
    /// flow that sent to an address: the answers for a name that the
    /// server looks up may count for another flow on the same port.
    #[test]
    fn where_silence_tells() {
        let target = |host, port| Target::new(HostName::parse(host), port);
        assert!(silence_tells(&target("10.0.0.1", 443)));
        assert!(silence_tells(&target("10.0.0.1", 53)));
        assert!(!silence_tells(&target("10.0.0.1", 9)));
        assert!(!silence_tells(&target("quic.test", 443)));
    }
}
