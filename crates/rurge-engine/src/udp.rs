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
use rurge_inbound::{ProcessSlot, SessionHandle, SessionOutcome, UdpClient};
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
/// flow still on the carrier (full cone). The datagram is known to answer
/// that flow when no other could be the one: it is the only flow that sent to
/// the source, or the only one sent to a name on the port. Among several the
/// datagram may be for any, and the oldest flow of all is only a guess.
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

    /// The flow a datagram from `from` counts for, and whether it is known to
    /// answer that flow (see `Routes`). A flow that is gone is none of them.
    fn flow_for(&self, from: &Target) -> Option<(Arc<Flow>, bool)> {
        let routes = self.0.lock().expect("routes");
        // the oldest of the flows sent to a target `sent_to` takes, and
        // whether it is the only one
        let oldest = |sent_to: &dyn Fn(&Target) -> bool| {
            let mut flows = routes
                .iter()
                .filter(|(to, _)| sent_to(to))
                .filter_map(|(_, f)| f.upgrade());
            let first = flows.next()?;
            Some((first, flows.next().is_none()))
        };
        oldest(&|to| to == from)
            .or_else(|| {
                oldest(&|to| matches!(to.host, HostName::Domain(_)) && to.port == from.port)
            })
            .or_else(|| oldest(&|_| true).map(|(flow, _)| (flow, false)))
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
        // the client may be told which flow this answers, by the target it
        // wrote for it, when there is one it is known to be
        let mut answers = None;
        if let Some((flow, answered)) = routes.flow_for(&from) {
            flow.handle.add_down(n as u64);
            flow.handle.mark_first_byte();
            flow.answered();
            if answered {
                let session = flow.handle.session();
                answers = Some(Target::new(session.dst_host.clone(), session.dst_port));
            }
        }
        // one datagram the client cannot take (too long once the SOCKS5
        // header is on, a full buffer) is lost alone: anyone may send to
        // the carrier, and the association lives by its control connection
        if let Err(e) = client.send(&buf[..n], &from, answers.as_ref()).await {
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
    /// The process of the association's client, looked up once: its control
    /// connection is the client's own socket, and every flow carries this
    /// slot (phase 3 M2 design 4.1).
    process: ProcessSlot,
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
    let process = engine.look_up_owner(&template);
    let association = Arc::new(Association {
        client: client.clone(),
        template,
        process,
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
    let handle = engine.new_handle_with(session, association.process.clone());
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
                drain(&association, &handle, &mut waiting, retry, None).await;
            }
            waiting.close();
        }
        Err(outcome) => {
            // it holds no room while the flow stays
            drop(first);
            // a refused flow stays until it is idle, and its client is told of
            // each datagram it drops, this first one too; a failed one is tried
            // again after `RETRY_AFTER`, and its client is told nothing: it
            // would give up on a destination that may be reachable by then
            let retry = matches!(outcome, SessionOutcome::Failed(_)).then(|| now() + RETRY_AFTER);
            let refused = match &outcome {
                SessionOutcome::Rejected(kind) => Some((&to, *kind)),
                _ => None,
            };
            if let Some((to, kind)) = refused {
                association.client.refused(to, kind);
            }
            handle.finish(outcome);
            drain(&association, &handle, &mut waiting, retry, refused).await;
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
    // Refused before anything is asked of the rules, a name lookup or a `smart`
    // group, and as a failure, not a rejection: a failed flow is tried again
    // after `RETRY_AFTER`, so it is served as soon as the suspension ends,
    // where a rejected one stays until its client has been quiet for
    // `FLOW_IDLE`. The lookup of the association's process, which `serve`
    // started with the association, is left to finish: the record of a refused
    // flow still names its process.
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
/// the meantime. `refused`: the flow was refused, to this target and in this
/// way, and the client is told of each datagram dropped.
async fn drain(
    association: &Association,
    handle: &SessionHandle,
    waiting: &mut mpsc::Receiver<Queued>,
    retry: Option<Instant>,
    refused: Option<(&Target, RejectKind)>,
) {
    let mut last = now();
    loop {
        let end = retry.unwrap_or(last + FLOW_IDLE);
        tokio::select! {
            _ = association.ended.cancelled() => return,
            _ = handle.token().cancelled() => return,
            got = waiting.recv() => match got {
                Some(_) => {
                    if let Some((to, kind)) = refused {
                        association.client.refused(to, kind);
                    }
                    last = now();
                }
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
        assert!(Arc::ptr_eq(&routes.flow_for(&target(2)).unwrap().0, &b));
        assert!(Arc::ptr_eq(&routes.flow_for(&target(9)).unwrap().0, &a));
        routes.remove(&a);
        assert!(Arc::ptr_eq(&routes.flow_for(&target(9)).unwrap().0, &b));
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
        assert!(Arc::ptr_eq(&routes.flow_for(&from(443)).unwrap().0, &c));
        assert!(Arc::ptr_eq(&routes.flow_for(&from(53)).unwrap().0, &b));
        assert!(Arc::ptr_eq(&routes.flow_for(&from(7)).unwrap().0, &a));
    }

    /// What a datagram says about the flow it answers. The flow whose carrier
    /// sent to the datagram's source, or else the flow sent to a name on its
    /// port, is the one it answers when there is exactly one such flow: the
    /// client is then told which target that was. With several of them the
    /// oldest gets the bytes counted, but the datagram may be for any of them,
    /// and with none of them it is for none (full cone): the client is told
    /// nothing. A flow that is gone counts for nothing.
    #[test]
    fn which_flow_a_reply_answers() {
        let flow = |id: u16| {
            Arc::new(Flow::new(
                SessionHandle::new(
                    u64::from(id),
                    SessionInfo::udp(HostName::parse("10.0.0.1"), 53),
                ),
                53,
            ))
        };
        // (the flow's id, whether it is known to be the one answered)
        let found = |routes: &Routes, from: &Target| {
            routes
                .flow_for(from)
                .map(|(flow, answered)| (flow.handle.id(), answered))
        };
        let address = |host: &str, port: u16| Target::new(HostName::parse(host), port);
        let routes_of = |sent: &[(&Target, &Arc<Flow>)]| {
            let routes = Routes::default();
            for (to, flow) in sent {
                routes.add((*to).clone(), flow);
            }
            routes
        };

        // exactly one flow sent to the source: it is the one
        let (a, b) = (flow(1), flow(2));
        let (here, there) = (address("10.0.0.9", 53), address("10.0.0.8", 53));
        let routes = routes_of(&[(&here, &a), (&there, &b)]);
        assert_eq!(found(&routes, &here), Some((1, true)));
        assert_eq!(found(&routes, &there), Some((2, true)));

        // two flows with the same key (DIRECT looked two names up to one
        // address): the oldest, and no telling which of them it is for
        let (a, b) = (flow(1), flow(2));
        let routes = routes_of(&[(&here, &a), (&here, &b)]);
        assert_eq!(found(&routes, &here), Some((1, false)));

        // a flow sent to the source itself wins over names on its port, and
        // is the one answered however many of them there are
        let (a, b, c) = (flow(1), flow(2), flow(3));
        let (named, other) = (address("one.test", 53), address("two.test", 53));
        let routes = routes_of(&[(&named, &a), (&other, &b), (&here, &c)]);
        assert_eq!(found(&routes, &here), Some((3, true)));

        // no flow sent to the source, exactly one to a name on its port: the
        // server looked the name up
        let (a, b) = (flow(1), flow(2));
        let elsewhere = address("10.0.0.7", 9);
        let routes = routes_of(&[(&elsewhere, &a), (&named, &b)]);
        assert_eq!(found(&routes, &here), Some((2, true)));

        // two flows sent to names on its port: the oldest, and no telling
        let (a, b, c) = (flow(1), flow(2), flow(3));
        let routes = routes_of(&[(&elsewhere, &a), (&named, &b), (&other, &c)]);
        assert_eq!(found(&routes, &here), Some((2, false)));

        // a flow to a name on another port is no help; neither is one to an
        // address on the same port, which is no name: anyone may be answering
        let (a, b) = (flow(1), flow(2));
        let (near, name_elsewhere) = (address("10.0.0.7", 53), address("three.test", 9));
        let routes = routes_of(&[(&near, &a), (&name_elsewhere, &b)]);
        assert_eq!(found(&routes, &here), Some((1, false)));

        // the one flow there is, which nothing says this is for: still not
        // known to be the one
        let a = flow(1);
        let routes = routes_of(&[(&elsewhere, &a)]);
        assert_eq!(found(&routes, &here), Some((1, false)));

        // a flow that is gone does not count among those sent to the source:
        // the other is then the only one
        let (a, b) = (flow(1), flow(2));
        let routes = routes_of(&[(&here, &a), (&here, &b)]);
        drop(a);
        assert_eq!(found(&routes, &here), Some((2, true)));
        // nor among the names on its port
        let (a, b) = (flow(1), flow(2));
        let routes = routes_of(&[(&named, &a), (&other, &b)]);
        drop(a);
        assert_eq!(found(&routes, &here), Some((2, true)));
        // nor as the oldest of all
        let (a, b, c) = (flow(1), flow(2), flow(3));
        let routes = routes_of(&[(&elsewhere, &a), (&near, &b), (&name_elsewhere, &c)]);
        drop(a);
        assert_eq!(found(&routes, &here), Some((2, false)));
        // and with no flow left there is none
        drop((b, c));
        assert_eq!(found(&routes, &here), None);
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

    /// A datagram a client is sent: its payload, where it says it came from,
    /// and the target of the flow it says it answers.
    type Heard = (Vec<u8>, Target, Option<Target>);

    /// A client that keeps what it is sent, and what it is told was refused.
    #[derive(Default)]
    struct Hears {
        heard: Mutex<Vec<Heard>>,
        refused: Mutex<Vec<(Target, RejectKind)>>,
    }

    impl UdpClient for Hears {
        fn recv<'a>(&'a self, _: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
            Box::pin(std::future::pending())
        }

        fn send<'a>(
            &'a self,
            payload: &'a [u8],
            from: &'a Target,
            answers: Option<&'a Target>,
        ) -> BoxFuture<'a, io::Result<()>> {
            self.heard
                .lock()
                .unwrap()
                .push((payload.to_vec(), from.clone(), answers.cloned()));
            Box::pin(std::future::ready(Ok(())))
        }

        fn refused(&self, target: &Target, kind: RejectKind) {
            self.refused.lock().unwrap().push((target.clone(), kind));
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
        let ours = Target::new(HostName::parse("10.0.0.1"), 53);
        assert_eq!(
            *client.heard.lock().unwrap(),
            [(b"answer".to_vec(), ours.clone(), Some(ours))]
        );
    }

    /// The client is told the target its flow was written to — not the
    /// address its carrier sent to, which is where the answer comes from —
    /// and only when that flow is the one answered.
    #[tokio::test]
    async fn the_client_is_told_what_the_flow_was_written_to() {
        let flow = |name: &str| {
            Arc::new(Flow::new(
                SessionHandle::new(1, SessionInfo::udp(HostName::parse(name), 53)),
                53,
            ))
        };
        let resolved = Target::new(HostName::parse("10.0.0.1"), 53);
        for (names, answers) in [
            (&["dns.test"][..], Some("dns.test")),
            // DIRECT looked both names up to one address: whose answer is it?
            (&["one.test", "two.test"][..], None),
        ] {
            let flows: Vec<_> = names.iter().map(|name| flow(name)).collect();
            let routes = Arc::new(Routes::default());
            for flow in &flows {
                routes.add(resolved.clone(), flow);
            }
            let client = Arc::new(Hears::default());
            let socket = HearsOnce(Mutex::new(Some((b"answer".to_vec(), resolved.clone()))));
            receive(
                Arc::new(socket),
                routes,
                client.clone(),
                CancellationToken::new(),
            )
            .await;
            let answers = answers.map(|name| Target::new(HostName::parse(name), 53));
            assert_eq!(
                *client.heard.lock().unwrap(),
                [(b"answer".to_vec(), resolved.clone(), answers)],
                "{names:?}"
            );
        }
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

        fn send<'a>(
            &'a self,
            _: &'a [u8],
            _: &'a Target,
            _: Option<&'a Target>,
        ) -> BoxFuture<'a, io::Result<()>> {
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
            process: ProcessSlot::unknown(),
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

    /// What a client is told while a flow drops three datagrams (`refused`:
    /// the flow was refused, to this target, in this way).
    async fn told_of_three_drops(
        refused: Option<(&Target, RejectKind)>,
    ) -> Vec<(Target, RejectKind)> {
        let session = SessionInfo::udp(HostName::parse("10.0.0.1"), 9);
        let client = Arc::new(Hears::default());
        let association = Association {
            client: client.clone(),
            template: session.clone(),
            process: ProcessSlot::unknown(),
            carriers: Carriers::default(),
            ended: CancellationToken::new(),
            done: mpsc::unbounded_channel().0,
        };
        let handle = SessionHandle::new(1, session);
        let (queue, mut waiting) = mpsc::channel(QUEUE);
        for datagram in [b"a", b"b", b"c"] {
            queue.try_send(queued(datagram)).expect("room");
        }
        // `drain` ends once the datagrams are dropped and the queue is closed
        drop(queue);
        tokio::time::timeout(
            Duration::from_secs(5),
            drain(&association, &handle, &mut waiting, None, refused),
        )
        .await
        .expect("the drain ends with its queue");
        client.refused.lock().unwrap().clone()
    }

    /// A flow that was refused tells its client of every datagram it drops,
    /// in the way it was refused. One that merely failed says nothing: it is
    /// tried again soon.
    #[tokio::test]
    async fn a_drained_flow_tells_its_client_only_when_it_was_refused() {
        let to = Target::new(HostName::parse("10.0.0.1"), 9);
        for kind in [
            RejectKind::Reject,
            RejectKind::Drop,
            RejectKind::NoDrop,
            RejectKind::TinyGif,
        ] {
            assert_eq!(
                told_of_three_drops(Some((&to, kind))).await,
                vec![(to.clone(), kind); 3]
            );
        }
        assert_eq!(told_of_three_drops(None).await, []);
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
