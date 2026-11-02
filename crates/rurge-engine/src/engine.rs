//! The engine (M3 design §7.2 – 7.3): implements `Dialer` for the listeners,
//! relays bytes, binds listeners from `[General]`, writes the session log.

use crate::auto::{EVALUATION_FAILED, resolve_ready};
use crate::control::Mode;
use crate::fake_address::Turned;
use crate::observe::{RequestLog, TrafficStats};
use crate::owner::{EXTERNAL, LOOKUPS, NOT_IN_TIME, SLOW, SlotProcess, query_for};
use crate::runtime::Runtime;
use crate::serve::raw_listed;
use crate::shared::EngineShared;
use crate::state::{Features, StateStore};
use arc_swap::{ArcSwap, ArcSwapOption};
use rurge_config::Builtin;
use rurge_config::HostName;
use rurge_config::NetworkFacts;
use rurge_config::general::{General, UdpFallback};
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_config::spec::{PolicySpec, ProtoSpec};
use rurge_dns::fakeip::FakeIpPool;
use rurge_http::mitm::ApiTls;
use rurge_inbound::{
    ClientConn, DialError, Dialed, Dialer, FailKind, HttpAuth, HttpListener, ListenerOpts,
    PlainRequests, ProcessSlot, Running, ServeOpts, SessionHandle, SessionOutcome, Socks5Listener,
    UdpAdmission, UdpClient,
};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedDatagram, BoxedStream, ConnectOpts, Connector, Target};
use rurge_net::packet_datagram::packet_datagram;
use rurge_policy::auto::SelectCtx;
use rurge_policy::{Note, PolicyRegistry, Resolution, TerminalKind};
use rurge_proto::{OutboundError, OutboundRef, UdpSupport};
use rurge_rules::engine::FixedResolve;
use rurge_rules::matcher::{ResolvedAddrs, SubRuleHit};
use rurge_rules::{LazyProcess, LazyResolver, OutboundMode, Outcome};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DROP_HOLD: Duration = Duration::from_secs(30);
/// Upper bound on an inbound handshake (HTTP request head / SOCKS5 negotiation).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_HTTP_PORT: u16 = 6152;
pub const DEFAULT_SOCKS5_PORT: u16 = 6153;
/// Sliding window for REJECT auto-escalation (M3b §7.4).
pub const ESCALATE_WINDOW: Duration = Duration::from_secs(30);
/// Escalating rejects for one host inside `ESCALATE_WINDOW` before REJECT-DROP kicks in.
pub const ESCALATE_COUNT: usize = 50;
/// Hard upper bound on tracked hosts. A full table is first swept of hosts
/// whose window has emptied; a new host that still does not fit is not
/// tracked at all, so it never escalates (same bound as `listener::warn_due`).
const ESCALATE_MAX_HOSTS: usize = 4096;
/// How long a rebind waits for an old listener's socket to close before
/// binding the new one (Windows sets no `SO_REUSEADDR`).
pub const REBIND_WAIT: Duration = Duration::from_secs(2);
/// What a session says when it is refused because the managed profile
/// requires an update (`Engine::suspend`).
pub const REFUSED_STRICT: &str = "refused: the managed profile requires an update (strict)";
/// What a session says when it is refused because it was sent to an address
/// of rurge's space that no name stands behind (`Engine::turn_back`).
pub const FAKE_ADDRESS_NOT_KNOWN: &str = "fake address not known";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerSpec {
    pub kind: ListenerKind,
    pub addr: SocketAddr,
    pub auth: Option<HttpAuth>,
}

/// The request log and traffic counters an `Engine` accumulates across its
/// lifetime (M3 design §7.3); survives config reloads (unlike `Runtime`).
struct Observe {
    log: RequestLog,
    traffic: TrafficStats,
}

/// Per-destination sliding-window counter for REJECT auto-escalation (§7.4).
/// A pre-matching refusal counts in it as well (`Engine::pre_match_refusal`).
pub(crate) struct Escalation {
    hosts: std::sync::Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Escalation {
    fn new() -> Escalation {
        Escalation {
            hosts: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Records one escalating reject for `host` at `now`. A full table is
    /// swept of hosts whose window has emptied; if that frees nothing, `host`
    /// is not remembered (and so never escalates) rather than growing the
    /// table — a reject flood to fresh hosts must not cost unbounded memory
    /// nor an O(n) sweep per reject under this lock.
    pub(crate) fn record(&self, host: &str, now: Instant) {
        let mut map = self.hosts.lock().expect("escalation");
        if let Some(times) = map.get_mut(host) {
            times.push_back(now);
            prune(times, now);
            return;
        }
        if map.len() >= ESCALATE_MAX_HOSTS {
            map.retain(|_, times| {
                prune(times, now);
                !times.is_empty()
            });
            if map.len() >= ESCALATE_MAX_HOSTS {
                return;
            }
        }
        map.insert(host.to_string(), VecDeque::from([now]));
    }

    /// What a refusal of `session` of `kind` comes to. `REJECT` and
    /// `REJECT-TINYGIF` count towards the upgrade, by the name or address of the
    /// destination, and become `REJECT-DROP` once the destination has reached
    /// the threshold; any other kind stays what it is and counts for nothing.
    /// A session refused after its rule matched (`connect_session`) and a DNS
    /// query or a connection refused by pre-matching
    /// (`Engine::pre_match_refusal`) come this way both, so that they are
    /// counted together, under one key.
    pub(crate) fn refusal(
        &self,
        session: &SessionInfo,
        kind: rurge_proto::RejectKind,
        now: Instant,
    ) -> rurge_proto::RejectKind {
        if !kind.escalates() {
            return kind;
        }
        let host = session.dst_host.to_string();
        self.record(&host, now);
        if self.should_drop(&host, now) {
            rurge_proto::RejectKind::Drop
        } else {
            kind
        }
    }

    /// Whether `host` has reached the threshold within the window.
    pub(crate) fn should_drop(&self, host: &str, now: Instant) -> bool {
        let mut map = self.hosts.lock().expect("escalation");
        match map.get_mut(host) {
            Some(times) => {
                prune(times, now);
                times.len() >= ESCALATE_COUNT
            }
            None => false,
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.hosts.lock().expect("escalation").len()
    }
}

fn prune(times: &mut VecDeque<Instant>, now: Instant) {
    while let Some(front) = times.front() {
        if now.duration_since(*front) >= ESCALATE_WINDOW {
            times.pop_front();
        } else {
            break;
        }
    }
}

pub struct Engine {
    runtime: ArcSwap<Runtime>,
    next_session: AtomicU64,
    sessions_root: CancellationToken,
    /// Cancelled to stop every listener from accepting new connections.
    accept: CancellationToken,
    /// Tracks CONNECT tunnels and upstream-connection drivers so a graceful
    /// shutdown can wait for them to drain.
    tracker: TaskTracker,
    observe: Arc<Observe>,
    pub(crate) escalation: Escalation,
    /// Runtime outbound mode and global policy (M4 design §4.1). Seeded from
    /// the first `Runtime`, changed by the API / CLI, never touched by reload.
    mode: ArcSwap<Mode>,
    global_policy: ArcSwap<Option<String>>,
    /// Why new sessions are refused (`suspend`); `None` while they are not.
    /// Like the mode, reload does not touch it, and it is not kept in
    /// `state.json`: the caller works it out again at every start.
    suspended: ArcSwap<Option<String>>,
    /// "proxy mode but no usable global policy" is warned once per change.
    global_warned: AtomicBool,
    /// Whether rurge decrypts what `[MITM]` lists (`phase 4 M2 design 11.1`):
    /// on unless `state.json` says otherwise (the general design's P4-D19).
    /// Unlike the mode it is not part of a generation: it takes effect for the
    /// next session and does not end the decrypted connections that are open.
    mitm_on: AtomicBool,
    /// Whether this generation's rewrites act (`phase 4 M3 design 11`): on
    /// unless `state.json` says otherwise. As `mitm_on`, it is not part of a
    /// generation: it is read when a request is routed, so it takes effect for
    /// the next request and does not end the requests that are open.
    rewrite_on: AtomicBool,
    /// Whether this engine captures requests (`phase 4 M4 design 5.1`): off
    /// unless `state.json` says otherwise. As `mitm_on`, it is not part of a
    /// generation: it is read when a request is routed, so it takes effect for
    /// the next request and does not end the requests that are open.
    capture_on: AtomicBool,
    /// The pool of fake addresses of the TUN that runs, if one does (phase 3
    /// M3 design 13): the proxy listeners turn the fake addresses that come in
    /// at them back into names with it (`turn_back`). Set by `start_tun` and
    /// cleared when the TUN has ended and its pool is saved.
    pub(crate) fake_pool: ArcSwapOption<FakeIpPool>,
    /// How the sniffs of this engine ended (phase 4 M1 design 7.5), what
    /// `http_counts` reads. As `fake_pool`, it outlives a reload.
    pub(crate) sniff_counts: crate::serve::SniffCounts,
    /// How the QUIC ClientHellos of this engine were read (`phase 4 M5 design
    /// 5.4`), what `http_counts` reads as `quic`: as `sniff_counts`, a reload
    /// does not clear them.
    pub(crate) quic_counts: crate::serve::QuicCounts,
    /// How the decrypted connections of this engine ended (`phase 4 M2 design
    /// 8.5`), what `http_counts` reads as `mitm`: as `sniff_counts`, a reload
    /// does not clear them, so they live here and not in a generation.
    pub(crate) mitm_counts: crate::mitm::MitmCounters,
    /// The requests that reached the rules (phase 4 M1 design 11): counted by
    /// `ConnUpstreams::route`, which every entry of the engine's requests
    /// shares, and read by `http_counts`.
    pub(crate) request_count: AtomicU64,
    state: OnceLock<Arc<StateStore>>,
    shared: EngineShared,
    /// Held while a registry or a whole generation is published, and while
    /// every dial reads the runtime and the registry as one pair
    /// (`snapshot`).
    generation: std::sync::Mutex<()>,
    /// The engine itself, for the tasks a UDP association spawns.
    me: std::sync::Weak<Engine>,
    /// UDP associations being served (at most `udp::ASSOCIATIONS`).
    udp_associations: std::sync::atomic::AtomicUsize,
    /// When a UDP limit was last warned about.
    udp_warned: std::sync::Mutex<Option<Instant>>,
    /// The places for the lookups of processes that use a blocking thread
    /// (`LOOKUPS`).
    owner_permits: Arc<tokio::sync::Semaphore>,
    /// The engine's one TUN: one permit, taken by `start_tun` and held by the
    /// task that keeps the pool of fake addresses saved, until the TUN has
    /// ended and its last save is written (phase 3 M4 design 3.4, P7).
    /// `Engine::tun_ended` waits for it.
    pub(crate) tun_permits: Arc<tokio::sync::Semaphore>,
    /// The rules waited for a process and gave up, and the engine said so
    /// (`SLOW`) — once.
    process_wait_warned: AtomicBool,
    /// What the engine remembers of the hosts whose QUIC it blocked for a
    /// `block-quic=adaptive` policy (phase 3 M5 design 6.3): it outlives a
    /// reload, and a change of network does not clear it.
    quic_adaptive: crate::quic_adaptive::QuicAdaptive,
}

impl Engine {
    /// Must be called inside a tokio runtime: it always starts the test
    /// scheduler task (`start_tests`), and a generation with subscriptions
    /// also a watcher task (`watch_subscriptions`).
    pub fn new(mut runtime: Runtime) -> Arc<Engine> {
        let observe = Arc::new(Observe {
            log: RequestLog::new(runtime.request_log_size),
            traffic: TrafficStats::new(),
        });
        let (mode, global) = Mode::from_outbound(&runtime.outbound_mode);
        // Publish the first generation before anything can dial through it.
        let shared = runtime.shared.clone();
        shared.cell.store(
            runtime
                .registry
                .take()
                .expect("a generation is published once"),
        );
        shared.resolver.store(runtime.stack.resolver.clone());
        // The first generation is put in use here, without
        // `publish_generation`: what it decrypts with is remembered just the
        // same (the phase 4 / M2a plan's P17).
        if let Some(mitm) = &runtime.mitm {
            shared.mitm.remember(mitm.authority());
        }
        let receivers = runtime.subscriptions.take_receivers();
        let engine = Arc::new_cyclic(|me| Engine {
            runtime: ArcSwap::from_pointee(runtime),
            next_session: AtomicU64::new(0),
            sessions_root: CancellationToken::new(),
            accept: CancellationToken::new(),
            tracker: TaskTracker::new(),
            observe,
            escalation: Escalation::new(),
            mode: ArcSwap::from_pointee(mode),
            global_policy: ArcSwap::from_pointee(global),
            suspended: ArcSwap::from_pointee(None),
            global_warned: AtomicBool::new(false),
            mitm_on: AtomicBool::new(true),
            rewrite_on: AtomicBool::new(true),
            capture_on: AtomicBool::new(false),
            fake_pool: ArcSwapOption::empty(),
            sniff_counts: crate::serve::SniffCounts::default(),
            quic_counts: crate::serve::QuicCounts::default(),
            mitm_counts: crate::mitm::MitmCounters::default(),
            request_count: AtomicU64::new(0),
            state: OnceLock::new(),
            shared,
            generation: std::sync::Mutex::new(()),
            me: me.clone(),
            udp_associations: std::sync::atomic::AtomicUsize::new(0),
            udp_warned: std::sync::Mutex::new(None),
            owner_permits: Arc::new(tokio::sync::Semaphore::new(LOOKUPS)),
            tun_permits: Arc::new(tokio::sync::Semaphore::new(1)),
            process_wait_warned: AtomicBool::new(false),
            quic_adaptive: crate::quic_adaptive::QuicAdaptive::new(),
        });
        let rt = engine.runtime();
        if let Some(pc) = rt.dns_pipeline() {
            pc.attach(Arc::downgrade(&engine));
        }
        engine.watch_subscriptions(&rt, receivers);
        engine.start_tests();
        engine
    }

    /// Held while a registry or a whole generation is published, so a
    /// subscription rebuild of a generation that is going out never publishes
    /// after its successor (M3 design 5.7) — and by every dial's `snapshot`,
    /// to read the runtime and the registry as one pair. Nothing inside it
    /// may do I/O, a name resolution, a dial or a build: that would hold up
    /// every dial. It guards no data. What is done under it runs everything
    /// that is known to be able to fail before its first store; a store also
    /// drops what it replaces when that was its last holder, and nothing in
    /// those drops is known to panic. A holder that panicked is taken to have
    /// published a whole generation or none: the poison is ignored.
    pub(crate) fn generation_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The current config generation (sessions snapshot it once at dial time).
    pub fn runtime(&self) -> Arc<Runtime> {
        self.runtime.load_full()
    }

    /// The registry the engine resolves against right now: the one in
    /// `EngineShared.cell`.
    pub fn registry(&self) -> Arc<PolicyRegistry> {
        self.shared
            .cell
            .load()
            .expect("published before the engine is handed out")
    }

    /// The current generation and the registry in use, as one pair. Both
    /// are published under the generation lock — a reload publishes the
    /// two together, a subscription rebuild a registry for the same
    /// generation — so a session never picks a name by one generation's
    /// rules and resolves it in another generation's registry.
    pub(crate) fn snapshot(&self) -> (Arc<Runtime>, Arc<PolicyRegistry>) {
        let _generation = self.generation_lock();
        (self.runtime(), self.registry())
    }

    /// Stores `next` as the current config generation (M3b §7.4 hot reload).
    pub(crate) fn store_runtime(&self, next: Runtime) {
        self.runtime.store(std::sync::Arc::new(next));
    }

    /// Listeners `[General]` asks for (M3 design §1.2 / matrix `allow-wifi-access`).
    pub fn listener_specs(general: &General) -> Vec<ListenerSpec> {
        let mut specs = Vec::new();
        for l in &general.http_listen {
            specs.push(ListenerSpec {
                kind: ListenerKind::Http,
                addr: l.addr,
                auth: l.password.clone().map(HttpAuth::Password),
            });
        }
        for l in &general.socks5_listen {
            specs.push(ListenerSpec {
                kind: ListenerKind::Socks5,
                addr: l.addr,
                auth: None,
            });
        }
        if specs.is_empty() {
            let (ip, http_port, socks_port, auth) = if general.allow_wifi_access {
                (
                    IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    general.wifi_access_http_port,
                    general.wifi_access_socks5_port,
                    general
                        .wifi_access_http_auth
                        .clone()
                        .map(|(user, password)| HttpAuth::UserPass {
                            user,
                            password: password.expose().clone(),
                        }),
                )
            } else {
                (
                    IpAddr::V4(Ipv4Addr::LOCALHOST),
                    DEFAULT_HTTP_PORT,
                    DEFAULT_SOCKS5_PORT,
                    None,
                )
            };
            specs.push(ListenerSpec {
                kind: ListenerKind::Http,
                addr: SocketAddr::new(ip, http_port),
                auth,
            });
            specs.push(ListenerSpec {
                kind: ListenerKind::Socks5,
                addr: SocketAddr::new(ip, socks_port),
                auth: None,
            });
        }
        specs
    }

    fn listener_opts(general: &General, spec: &ListenerSpec) -> ListenerOpts {
        ListenerOpts {
            kind: spec.kind,
            restrict_to_lan: general.proxy_restricted_to_lan,
            auth: spec.auth.clone(),
            show_error_page: general.show_error_page,
            show_error_page_for_reject: general.show_error_page_for_reject,
            drop_hold: DROP_HOLD,
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }

    /// Binds every listener in `listener_specs` order; the first failure aborts.
    /// Each spec is paired with the listener it produced, so callers never have
    /// to line the two lists up themselves.
    pub async fn bind_listeners(self: &Arc<Self>) -> io::Result<Vec<(ListenerSpec, Running)>> {
        let rt = self.runtime();
        let mut out = Vec::new();
        for spec in Self::listener_specs(&rt.config.general) {
            let opts = Self::listener_opts(&rt.config.general, &spec);
            let dialer: Arc<dyn Dialer> = self.clone();
            let running = match spec.kind {
                ListenerKind::Http => {
                    HttpListener::bind(
                        spec.addr,
                        dialer,
                        opts,
                        self.accept.child_token(),
                        self.tracker.clone(),
                    )
                    .await?
                }
                ListenerKind::Socks5 => {
                    Socks5Listener::bind(spec.addr, dialer, opts, self.accept.child_token()).await?
                }
                // never produced by `listener_specs`; listed so a new kind breaks the build
                ListenerKind::Tun | ListenerKind::Forward | ListenerKind::Internal => continue,
            };
            // DEBUG, not INFO: `rurge run` prints the authoritative
            // `listening on …` line on stdout, and announcing every listener
            // twice at `loglevel = notify` helps nobody.
            tracing::debug!(kind = ?spec.kind, addr = %running.local_addr, "listening");
            out.push((spec, running));
        }
        Ok(out)
    }

    /// Replaces `old` with listeners bound from the current generation: stop
    /// the old accept loops, wait until their sockets are closed (so the
    /// addresses can be bound again; Windows sets no `SO_REUSEADDR`), then let
    /// their in-flight sessions drain in the background — dropping the old
    /// `Running`s would abort them instead.
    ///
    /// The old listeners are gone before the new ones are attempted, so a
    /// failure leaves the caller with none; call again (with an empty `old`)
    /// once the addresses are free to recover.
    pub async fn rebind_listeners(
        self: &Arc<Self>,
        old: Vec<(ListenerSpec, Running)>,
    ) -> io::Result<Vec<(ListenerSpec, Running)>> {
        let olds: Vec<Running> = old.into_iter().map(|(_, r)| r).collect();
        for o in &olds {
            o.stop();
        }
        for o in &olds {
            let _ = tokio::time::timeout(REBIND_WAIT, o.wait_closed()).await;
        }
        for o in olds {
            tokio::spawn(o.join());
        }
        self.bind_listeners().await
    }

    /// A token every session's cancellation reaches (`cancel_sessions`).
    pub(crate) fn session_token(&self) -> CancellationToken {
        self.sessions_root.child_token()
    }

    /// Says, at most once a minute, that a UDP limit dropped something
    /// (M5-D11); `what` names the limit and what was refused.
    pub(crate) fn warn_udp_limit(&self, what: &'static str) {
        let now = Instant::now();
        let mut last = self.udp_warned.lock().expect("udp warning");
        if last.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(60)) {
            *last = Some(now);
            tracing::warn!("{what}");
        }
    }

    /// A handle for a session whose process nobody looks for: rurge's own
    /// sessions, the connectivity tests.
    pub(crate) fn new_handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
        self.new_handle_with(session, ProcessSlot::unknown())
    }

    /// The hosts whose QUIC this engine blocked for a `block-quic=adaptive`
    /// policy, and whether they fell back to TCP (design 6.3).
    pub(crate) fn quic_adaptive(&self) -> &crate::quic_adaptive::QuicAdaptive {
        &self.quic_adaptive
    }

    /// Starts finding the process behind `session`, if it is one that can be
    /// found: it came in through a proxy port or the virtual network card,
    /// from an address that may be this machine's (phase 3 M2 design 4.1, 4.2;
    /// `query_for` says what is asked). The answer lands in the slot, whenever
    /// the lookup ends; the slot is settled with no process at once for a
    /// session that is not looked up.
    pub(crate) fn look_up_owner(&self, session: &SessionInfo) -> ProcessSlot {
        let Some(owners) = &self.shared.owners else {
            return ProcessSlot::unknown();
        };
        let launched = Instant::now();
        let Some(query) = query_for(session, launched) else {
            return ProcessSlot::unknown();
        };
        // a source that is surely not this machine's has no process here
        if owners.is_local(session.src.ip()) == Some(false) {
            return ProcessSlot::unknown();
        }
        let slot = ProcessSlot::pending_at(launched);
        // The task holds the lookup, the places and the slot — not the
        // engine, and not the session.
        tokio::spawn(crate::owner::find(
            owners.clone(),
            self.owner_permits.clone(),
            slot.clone(),
            query,
        ));
        slot
    }

    /// `new_handle` for a session whose process is looked up: `process` is
    /// the slot of that lookup (`look_up_owner`).
    pub(crate) fn new_handle_with(
        &self,
        session: SessionInfo,
        process: ProcessSlot,
    ) -> Arc<SessionHandle> {
        self.new_handle_under(session, process, &self.sessions_root)
    }

    /// `new_handle_with` under a parent token of the caller's: the HTTP
    /// engine's carrier connection hands each request a child of the
    /// connection's token (design 8.11).
    pub(crate) fn new_handle_under(
        &self,
        session: SessionInfo,
        process: ProcessSlot,
        parent: &CancellationToken,
    ) -> Arc<SessionHandle> {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        let handle = SessionHandle::new_with_slot(id, session, parent.child_token(), process);
        self.attach_observers(&handle);
        handle
    }

    /// Hooks the session log and the request record onto `handle` and marks it
    /// active: what a registered handle gets when it is made
    /// (`new_handle_under`), and a carrier connection when it ends with no
    /// request routed (`serve_http`, design 10.2).
    pub(crate) fn attach_observers(&self, handle: &Arc<SessionHandle>) {
        // Weak: the active index holds the handle and the handle holds this
        // hook, so a strong `Arc<Observe>` here would be a reference cycle.
        let observe = Arc::downgrade(&self.observe);
        handle.on_finish(move |h, outcome| {
            log_session(h, outcome);
            if let Some(o) = observe.upgrade() {
                // `record_finished` owns the ordering guarantee (its doc): the
                // bytes land in `traffic` under the active lock, so no reader
                // ever counts this session twice or misses it.
                o.log.record_finished(h, outcome, &o.traffic);
            }
        });
        self.observe.log.mark_active(handle);
    }
}

impl Engine {
    /// The generation-independent objects; every `Runtime` swapped into this
    /// engine must have been built with them.
    pub fn shared(&self) -> EngineShared {
        self.shared.clone()
    }

    /// How long a sniffable session's first bytes are waited for
    /// (`EngineShared::sniff_wait`, phase 4 M1 design 6.3).
    pub(crate) fn sniff_wait(&self) -> Duration {
        self.shared.sniff_wait
    }

    /// Stops every `external` program still running, whatever generation
    /// started it (phase 2 M4 design 8.2): the exit flow calls this before
    /// the runtime ends, rather than trust what exiting would drop.
    pub async fn stop_external_programs(&self) {
        self.shared.externals.stop_all().await;
    }

    /// The network the engine looks at now: the one `network_changed` was last
    /// given, or else what was put in `EngineShared.network` before the engine
    /// was made. Nothing is known until one of the two.
    pub fn network(&self) -> Arc<NetworkFacts> {
        self.shared.network.load()
    }

    /// The machine is on another network now, `facts` (phase 3 M1 design 5):
    /// the rules and the `subnet` groups look at it from their next
    /// evaluation on, and what the generation in use kept of the old network
    /// goes. The resolver drops its cache and its conclusion that the servers
    /// never answer AAAA, and reads the platform's DNS servers again, here and
    /// now (`Resolver::on_network_change`). The outbounds are told, and drop
    /// what is tied to the old network (WireGuard gives every peer a new
    /// carrier); the groups that pick by their tests test again when they are
    /// used next, and the override of a `subnet` group ends
    /// (`PolicyRegistry::network_changed`). A session that is under way and a
    /// lookup in flight finish on what they had.
    ///
    /// The DNS override of the new network is not applied here: a generation
    /// is built with it. Whoever finds that the new network overrides the DNS
    /// differently than the generation in use was built with
    /// (`Runtime::dns_override`) builds another.
    pub fn network_changed(&self, facts: NetworkFacts) {
        self.shared.network.store(facts);
        // the generation in use and its registry, as one pair
        let (runtime, registry) = self.snapshot();
        runtime.stack.resolver.on_network_change();
        registry.network_changed();
    }

    /// The enhanced mode took the system's DNS or gave it back: the resolver in
    /// use reads the system's servers again (`Resolver::reread_system_servers`),
    /// keeping its cache. Not a change of network: `network_changed` is that.
    pub fn system_dns_changed(&self) {
        let (runtime, _) = self.snapshot();
        runtime.stack.resolver.reread_system_servers();
    }

    /// Makes `next` the generation that outlives-a-reload objects see: chain
    /// connectors and dials resolve against its registry, direct connectors
    /// through its resolver; the automatic groups keep what still applies
    /// to its groups (M3 design 6.5).
    pub(crate) fn publish_generation(&self, next: &mut Runtime) {
        assert!(
            Arc::ptr_eq(&self.shared.cell, &next.shared.cell)
                && Arc::ptr_eq(&self.shared.resolver, &next.shared.resolver),
            "the next generation must be built with `Engine::shared()`"
        );
        let registry = next
            .registry
            .take()
            .expect("a generation is published once");
        // the retains come before the first store: they take locks with
        // `expect`, so they can panic, and nothing that is known to fail
        // comes after them. A store also drops what it replaces when this was
        // its last holder (the previous registry, with the outbounds that
        // were not carried over), and nothing in those drops is known to
        // panic
        //
        // what the `smart` groups knew of what is gone goes with it (phase 2
        // M3c design 5.1)
        self.shared
            .auto
            .smart
            .retain(|name| registry.contains(name));
        self.shared.auto.retain(&next.config.group_specs);
        // remembered before the first store as well: it takes a lock with
        // `expect`, and nothing that is known to fail comes after the stores
        if let Some(mitm) = &next.mitm {
            self.shared.mitm.remember(mitm.authority());
        }
        self.shared.cell.store(registry);
        self.shared.resolver.store(next.stack.resolver.clone());
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // registry → outbound → chain connector → cell → registry
        self.shared.cell.clear();
    }
}

#[derive(Debug)]
pub struct UnknownPolicy(pub String);

impl std::fmt::Display for UnknownPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown policy `{}`", self.0)
    }
}
impl std::error::Error for UnknownPolicy {}

pub struct PoliciesView {
    pub proxies: Vec<String>,
    pub groups: Vec<String>,
}

pub struct RuleView {
    pub index: usize,
    pub rule: String,
    pub hits: u64,
}

impl Engine {
    /// Attaches the state store so mode / global-policy changes persist.
    pub fn attach_state(&self, store: Arc<StateStore>) {
        let _ = self.state.set(store);
    }

    /// The attached state store, if any (`attach_state`).
    pub(crate) fn state_store(&self) -> Option<&Arc<StateStore>> {
        self.state.get()
    }

    pub fn mode(&self) -> Mode {
        **self.mode.load()
    }

    pub async fn set_mode(&self, mode: Mode) {
        self.mode.store(Arc::new(mode));
        self.global_warned.store(false, Ordering::Relaxed);
        if let Some(store) = self.state.get() {
            store
                .update(|s| s.outbound_mode = Some(mode.as_str().to_string()))
                .await;
        }
    }

    pub fn global_policy(&self) -> Option<String> {
        (**self.global_policy.load()).clone()
    }

    /// Whether rurge decrypts the sessions `[MITM]` lists (`phase 4 M2 design
    /// 11.1`). On unless `state.json` said otherwise at start.
    pub fn mitm_on(&self) -> bool {
        self.mitm_on.load(Ordering::Relaxed)
    }

    /// Sets the `mitm` switch, and keeps it in `state.json` when one is
    /// attached (the plan's P20: the shape of `set_mode`). A decrypted
    /// connection that is open goes on; the next TLS session judged is the
    /// first this reaches.
    pub async fn set_mitm(&self, on: bool) {
        self.mitm_on.store(on, Ordering::Relaxed);
        if let Some(store) = self.state.get() {
            store.update(|s| s.features.mitm = Some(on)).await;
        }
    }

    /// Whether this generation's `[URL Rewrite]` and `[Header Rewrite]` act
    /// (`phase 4 M3 design 11`). On unless `state.json` said otherwise at
    /// start.
    pub fn rewrite_on(&self) -> bool {
        self.rewrite_on.load(Ordering::Relaxed)
    }

    /// Sets the `rewrite` switch, and keeps it in `state.json` when one is
    /// attached (the shape of `set_mitm`). A request that is already being
    /// served goes on with the rules it was routed with; the next one reads
    /// this.
    pub async fn set_rewrite(&self, on: bool) {
        self.rewrite_on.store(on, Ordering::Relaxed);
        if let Some(store) = self.state.get() {
            store.update(|s| s.features.rewrite = Some(on)).await;
        }
    }

    /// Whether this engine captures the requests it serves (`phase 4 M4 design
    /// 5.1`). Off unless `state.json` said otherwise at start.
    pub fn capture_on(&self) -> bool {
        self.capture_on.load(Ordering::Relaxed)
    }

    /// Sets the `capture` switch, and keeps it in `state.json` when one is
    /// attached (the shape of `set_rewrite`). A request that is already being
    /// served goes on uncaptured if nothing armed it; the next one reads this.
    pub async fn set_capture(&self, on: bool) {
        self.capture_on.store(on, Ordering::Relaxed);
        if let Some(store) = self.state.get() {
            store.update(|s| s.features.capture = Some(on)).await;
        }
    }

    /// The store the captures of every generation go into (`phase 4 M4 design
    /// 4.1`): one per engine, so a reload keeps them.
    pub fn capture(&self) -> Arc<rurge_http::capture::CaptureStore> {
        self.shared.capture.clone()
    }

    /// The capture a session goes into, if it is captured at all (`phase 4 M4
    /// design 5.3`): the name the matched rule gave it, else the default
    /// session while the switch is on, else nothing.
    pub(crate) fn capture_for(
        &self,
        handle: &SessionHandle,
    ) -> Option<rurge_http::capture::Capture> {
        let session = match handle.capture_session() {
            Some(name) => name,
            None if self.capture_on() => Arc::from(rurge_http::capture::DEFAULT_SESSION),
            None => return None,
        };
        Some(rurge_http::capture::Capture {
            store: self.shared.capture.clone(),
            session,
        })
    }

    /// The switches `state.json` recorded at start (the plan's P20): only what
    /// was set is taken, and nothing is written — a start does not write the
    /// file, so a file of the first version stays as it is until something is
    /// switched.
    pub fn restore_switches(&self, features: &Features) {
        if let Some(on) = features.mitm {
            self.mitm_on.store(on, Ordering::Relaxed);
        }
        if let Some(on) = features.rewrite {
            self.rewrite_on.store(on, Ordering::Relaxed);
        }
        if let Some(on) = features.capture {
            self.capture_on.store(on, Ordering::Relaxed);
        }
    }

    /// The DER of the CA this generation signs with (`phase 4 M2 design
    /// 12.2`): what `GET /v1/mitm/ca` serves. `None` when this generation
    /// has no usable CA.
    pub fn mitm_ca_der(&self) -> Option<Vec<u8>> {
        let runtime = self.runtime();
        runtime
            .mitm
            .as_ref()
            .map(|mitm| mitm.authority().ca_der().to_vec())
    }

    /// The TLS of the API: the authority of the last published generation
    /// that had one (`MitmShared::last_live`), or `None` when none had. It is
    /// asked once per new connection, so that a reload that brought another
    /// CA signs the connections after it with that one (the phase 4 / M2c
    /// plan's P9).
    pub fn api_tls(&self) -> Option<ApiTls> {
        let authority = self.shared.mitm.last_live()?;
        match ApiTls::new(authority) {
            Ok(tls) => Some(tls),
            Err(e) => {
                tracing::debug!("the TLS of the API cannot be set up: {e}");
                None
            }
        }
    }

    /// Refuses the sessions that begin from now on (`Some(why)`), or ends the
    /// refusal (`None`). Only new sessions are refused, at the four places a
    /// session begins: a client's TCP connection, a UDP flow, and — under
    /// `encrypted-dns-follow-outbound-mode` — a DNS server's connection and
    /// its QUIC carrier. A session that is open goes on, and the listeners,
    /// the API and the connectivity tests are not touched. A refused session
    /// fails with `REFUSED_STRICT` and says nothing of `why`, which is for the
    /// log and the API: whoever calls this logs the change. `Some` while
    /// already suspended replaces the reason; `None` while not suspended
    /// changes nothing.
    pub fn suspend(&self, why: Option<String>) {
        self.suspended.store(Arc::new(why));
    }

    /// Why new sessions are refused (`suspend`); `None` while they are not.
    pub fn suspended(&self) -> Option<String> {
        (**self.suspended.load()).clone()
    }

    /// Whether a session that begins now is refused.
    pub(crate) fn is_suspended(&self) -> bool {
        self.suspended.load().is_some()
    }

    /// `true` for the built-in policies and every policy / group of the
    /// current registry (for API / CLI callers).
    pub fn policy_exists(&self, name: &str) -> bool {
        policy_known(&self.registry(), name)
    }

    /// Empty name clears the global policy.
    pub async fn set_global_policy(&self, name: &str) -> Result<(), UnknownPolicy> {
        let name = name.trim();
        let value = if name.is_empty() {
            None
        } else if self.policy_exists(name) {
            Some(name.to_string())
        } else {
            return Err(UnknownPolicy(name.to_string()));
        };
        self.global_policy.store(Arc::new(value.clone()));
        self.global_warned.store(false, Ordering::Relaxed);
        if let Some(store) = self.state.get() {
            store.update(|s| s.global_policy = value).await;
        }
        Ok(())
    }

    pub fn policies_view(&self) -> PoliciesView {
        let registry = self.registry();
        let mut proxies: Vec<String> = [
            "DIRECT",
            "REJECT",
            "REJECT-DROP",
            "REJECT-NO-DROP",
            "REJECT-TINYGIF",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        // the profile's, the imported and the derived ones (M3 design §8)
        proxies.extend(registry.policy_names());
        PoliciesView {
            proxies,
            groups: registry.group_names(),
        }
    }

    pub fn rules_view(&self) -> Vec<RuleView> {
        self.runtime()
            .rules
            .rules()
            .iter()
            .map(|r| RuleView {
                index: r.index,
                rule: r.raw.clone(),
                hits: r.hits(),
            })
            .collect()
    }

    /// The resolver of the current config generation.
    pub fn resolver(&self) -> Arc<rurge_dns::Resolver> {
        self.runtime().stack.resolver.clone()
    }

    /// `internet-test-url` of the current config generation.
    pub fn internet_test_url(&self) -> String {
        self.runtime().config.general.internet_test_url.clone()
    }

    /// The main profile file of the current config generation: the local
    /// file, even when the text in effect is a downloaded copy of it.
    pub fn profile_path(&self) -> std::path::PathBuf {
        self.runtime().config.source.main.clone()
    }

    /// Where this engine keeps its caches.
    pub fn data_dir(&self) -> std::path::PathBuf {
        self.runtime().stack.resources.root().to_path_buf()
    }

    /// The text of the profile in effect: the local file, or the downloaded
    /// copy of a managed profile while that is in effect (`profile_path` is
    /// the local file either way); secrets redacted unless `sensitive`.
    pub async fn config_text(&self, sensitive: bool) -> io::Result<String> {
        let path = self.runtime().config.source.main_text.clone();
        let text = tokio::fs::read_to_string(&path).await?;
        Ok(if sensitive {
            text
        } else {
            rurge_config::redact::redact_profile(&text)
        })
    }

    /// Mode / rule → policy, shared by `dial`, `dial_internal` and the UDP
    /// flows (M4 §4.1). When a line of a set decided the rule, the session's
    /// log line and its notes say which. The IP rules ask the engine's own
    /// resolver; `choose_policy_with` is the same with rules that ask another
    /// (the address an internal DNS session comes with, phase 3 / M5 design
    /// 9.3).
    pub(crate) async fn choose_policy(
        &self,
        rt: &Runtime,
        registry: &PolicyRegistry,
        handle: &SessionHandle,
    ) -> Chosen {
        self.choose_policy_with(rt, registry, handle, rt.stack.resolver.as_ref())
            .await
    }

    /// `choose_policy` with the IP rules asking `resolver`.
    pub(crate) async fn choose_policy_with(
        &self,
        rt: &Runtime,
        registry: &PolicyRegistry,
        handle: &SessionHandle,
        resolver: &dyn LazyResolver,
    ) -> Chosen {
        // the process of the session, waited for only while an `external`
        // program runs (M4 design 11, M4b plan P16): the same `SlotProcess`
        // the rules go on with, so neither waits twice
        let process = SlotProcess::new(handle.process());
        // a session of an external program or of what it started is DIRECT,
        // whatever the rules and the outbound mode say (M4-D21)
        if self.shared.externals.any_running()
            && let Some(info) = process.process().await
            && let Some(pid) = info.pid
            && self.shared.externals.runs(pid)
        {
            handle.add_note(EXTERNAL);
            tracing::debug!(session = handle.id(), pid, "{EXTERNAL}");
            return Chosen::Policy(PolicyRef::Builtin(Builtin::Direct));
        }
        match self.mode() {
            Mode::Direct => return Chosen::Policy(PolicyRef::Builtin(Builtin::Direct)),
            Mode::Proxy => match self.global_policy() {
                Some(name) if policy_known(registry, &name) => {
                    return Chosen::Policy(PolicyRef::parse(&name));
                }
                other => {
                    if !self.global_warned.swap(true, Ordering::Relaxed) {
                        tracing::warn!(
                            policy = ?other,
                            "outbound mode is proxy but the global policy is unset or unknown; routing by rules"
                        );
                    }
                }
            },
            Mode::Rule => {}
        }
        let decision = rt
            .rules
            .evaluate_with_process(handle.session(), OutboundMode::Rule, resolver, &process)
            .await;
        if let Some(i) = decision.matched {
            handle.set_rule(rule_raw(&rt.rules, i));
            // the session the matched line names, if any (`phase 4 M4 design
            // 5.2`): the three early returns above read no rule, so they name
            // none, and their requests are captured only while the switch is
            // on, into the default session
            if let Some(name) = always_capture(&rt.rules, i) {
                handle.set_capture_session(name);
            }
        }
        if process.timed_out() {
            // the rules went on without the process: the record says why a
            // rule that names it did not match
            handle.add_note(NOT_IN_TIME);
            tracing::debug!(session = handle.id(), "{NOT_IN_TIME}");
            if !self.process_wait_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!("{SLOW}");
            }
        }
        if let Some(hit) = &decision.sub_rule {
            note_sub_rule(handle, hit);
        }
        match decision.outcome {
            Outcome::Policy(p) => Chosen::Policy(p),
            Outcome::DnsFailed => Chosen::DnsFailed,
        }
    }
}

pub(crate) enum Chosen {
    Policy(PolicyRef),
    DnsFailed,
}

/// Says which line of a set made the rule match: in the session's notes, and
/// in a line at DEBUG.
pub(crate) fn note_sub_rule(handle: &SessionHandle, hit: &SubRuleHit) {
    let note = format!("Sub-rule matched: {} (in {})", hit.entry, hit.set);
    tracing::debug!(session = handle.id(), "{note}");
    handle.add_note(note);
}

/// `true` for the built-in policies and every policy / group of `registry` —
/// the one the session loaded once, not whatever is current when this runs,
/// so a reload or a subscription update racing a dial can never approve a
/// name against one registry and then resolve it (`PolicyRegistry::resolve`)
/// against another.
pub(crate) fn policy_known(registry: &PolicyRegistry, name: &str) -> bool {
    matches!(PolicyRef::parse(name), PolicyRef::Builtin(_)) || registry.contains(name)
}

/// The policy whose SERVER this machine has to reach — and so, if that server
/// is a host name, to resolve — when `name` is dialled: `name` itself, or,
/// following `underlying-proxy` (through a group's current member), the last
/// proxy hop of its chain. A hop whose `underlying-proxy` currently resolves
/// to DIRECT is that last hop: DIRECT opens the socket, and the name it looks
/// up is that hop's own server. `None` when the chain ends at REJECT (it fails
/// fast, it does not loop), or the walk runs past `MAX_DEPTH` hops — its own
/// bound, not the registry's: the registry's `MAX_DEPTH` bounds group nesting
/// within one resolution, and each hop here (like a real dial's
/// `ChainConnector`) resolves fresh at depth 0; a chain that long is treated
/// the same as one ending at REJECT.
fn socket_opener<'a>(registry: &'a PolicyRegistry, name: &str) -> Option<&'a PolicySpec> {
    let mut current = name.to_string();
    for _ in 0..rurge_policy::registry::MAX_DEPTH {
        let spec = registry.spec(&current)?;
        let Some(under) = spec.common.underlying_proxy.as_deref() else {
            return Some(spec);
        };
        // as the hop's `ChainConnector` will: a relay group without members
        // refuses there, so it is REJECT here too
        let below = registry.resolve_relay(under);
        match below.terminal {
            // a `Proxy` terminal is the hop the last chain element names; this
            // hop's server travels to it as a target and is never looked up here
            TerminalKind::Proxy => current = below.chain.last()?.clone(),
            // DIRECT opens the socket itself, with this hop's server as the target
            TerminalKind::Direct => return Some(spec),
            TerminalKind::Reject => return None,
        }
    }
    None
}

/// Whether reaching `spec`'s server takes a name lookup: the server, or the
/// endpoint of one of a `wireguard` policy's peers, is a host name.
fn named_server(spec: &PolicySpec) -> bool {
    match &spec.proto {
        ProtoSpec::WireGuard(wireguard) => wireguard
            .section
            .peers
            .iter()
            .any(|peer| peer.endpoint.host.as_domain().is_some()),
        _ => matches!(spec.server, Some(HostName::Domain(_))),
    }
}

/// The note on an internal DNS session that went directly instead of to a
/// REJECT, or to a chain that ends at one.
const REJECT_BYPASSED: &str = "dns-follow: reject bypassed to keep DNS working";

/// The refusal of an internal DNS session while the engine is suspended: the
/// session fails with the sentence, and the resolver hears the same — no
/// policy was chosen, so none is named.
fn refused_internal(handle: &SessionHandle) -> io::Error {
    handle.finish(SessionOutcome::Failed(REFUSED_STRICT.to_string()));
    io::Error::other(REFUSED_STRICT)
}

/// The bypass every anti-loop arm of `dial_internal` takes (a REJECT, a
/// proxy configured by host name, a chain that ends at REJECT): note `why`
/// on the session, connect with `fallback`, and finish the handle explicitly
/// when even that fails — it would otherwise be dropped without a finished
/// record.
async fn bypass_to_direct(
    handle: Arc<SessionHandle>,
    fallback: &Arc<dyn Connector>,
    target: &Target,
    opts: &ConnectOpts,
    why: &str,
) -> io::Result<BoxedStream> {
    handle.set_error(why);
    match fallback.connect(target, opts).await {
        Ok(stream) => Ok(crate::dns_pipeline::wrap_internal(stream, handle)),
        Err(e) => {
            handle.finish(SessionOutcome::Failed(e.to_string()));
            Err(e)
        }
    }
}

/// `bypass_to_direct` for the QUIC carrier of `dial_internal_udp`: the
/// carrier is `fallback`'s.
async fn bypass_udp_to_direct(
    handle: Arc<SessionHandle>,
    fallback: &Arc<dyn Connector>,
    target: &Target,
    opts: &ConnectOpts,
    why: &str,
) -> io::Result<BoxedDatagram> {
    handle.set_error(why);
    match fallback.connect_udp(target, opts).await {
        Ok(datagram) => Ok(crate::dns_pipeline::wrap_internal_udp(
            datagram, handle, None,
        )),
        Err(e) => {
            handle.finish(SessionOutcome::Failed(e.to_string()));
            Err(e)
        }
    }
}

/// Where the routing sends an internal DNS session (`Engine::route_internal`).
enum InternalRoute {
    /// Through its policy.
    Policy(Resolution),
    /// Directly instead, for the reason noted on the session.
    Bypass(&'static str),
}

/// The one address a named internal DNS session comes with, in the shape the
/// IP rules ask their resolver for: its own family, the other empty (phase 3
/// / M5 design 9.3).
fn resolved_addrs(address: IpAddr) -> ResolvedAddrs {
    match address {
        IpAddr::V4(v4) => ResolvedAddrs {
            v4: vec![v4],
            v6: Vec::new(),
        },
        IpAddr::V6(v6) => ResolvedAddrs {
            v4: Vec::new(),
            v6: vec![v6],
        },
    }
}

/// What an internal DNS session dials: the address its server was resolved to
/// when it came with one (a server written as a name), the session's own
/// destination otherwise. Everything dials that address — DIRECT, a proxy, a
/// bypass — so a named session never needs a lookup of the engine's own
/// (phase 3 / M5 design 9.4).
fn internal_target(handle: &SessionHandle, address: Option<IpAddr>) -> Target {
    let session = handle.session();
    let host = match address {
        Some(address) => HostName::Ip(address),
        None => session.dst_host.clone(),
    };
    Target::new(host, session.dst_port)
}

/// What an internal DNS session tells a `smart` group of the member it went
/// through (M3c design 4.3): that it could not connect, or — once its
/// outbound is ready — its first byte back, or three seconds without one;
/// a DNS session tries no other member (M3c-D5). Silence tells: a DNS
/// server answers at once what a session sends first, a query or a QUIC
/// handshake.
fn report_internal(
    registry: &PolicyRegistry,
    resolution: &Resolution,
    handle: &Arc<SessionHandle>,
    connected: Result<(), &OutboundError>,
) {
    let Some(pick) = &resolution.smart else {
        return;
    };
    let (book, now) = (&registry.auto().smart, Instant::now());
    let host = handle.session().dst_host.to_string();
    let outbound = &resolution.outbound;
    match connected {
        Ok(()) => {
            book.used(&pick.group, &pick.member, now);
            crate::smart::watch(handle, book.clone(), &pick.member, outbound, &host, true);
        }
        Err(e) if crate::smart::retryable(e) => {
            book.report_failure(&pick.member, outbound, Some(&host), now);
        }
        Err(_) => {}
    }
}

impl Engine {
    /// Mode / rule → policy for an internal DNS session (`dial_internal`,
    /// `dial_internal_udp`), and the bypasses decided before anything is
    /// dialled, each warned about. `address` is the address the session's
    /// server was resolved to, when the session was named: the IP rules then
    /// match by it, never by a lookup of the engine's, which would come back
    /// to this very session (phase 3 / M5 design 9.3).
    async fn route_internal(
        &self,
        rt: &Runtime,
        registry: &PolicyRegistry,
        handle: &SessionHandle,
        address: Option<IpAddr>,
    ) -> InternalRoute {
        let chosen = match address {
            Some(address) => {
                let fixed = FixedResolve(resolved_addrs(address));
                self.choose_policy_with(rt, registry, handle, &fixed).await
            }
            None => self.choose_policy(rt, registry, handle).await,
        };
        let policy = match chosen {
            Chosen::Policy(p) => p,
            // Neither an IP-literal DNS session nor a named one needs a lookup
            // here (the named one's rules ask its own address); a DnsFailed
            // would only come from a misconfigured rule → direct.
            Chosen::DnsFailed => PolicyRef::Builtin(Builtin::Direct),
        };
        // the session's own destination, not what is dialled: a `smart` group
        // knows the server by its name when it was written as one (phase 3 /
        // M5 design 9.3)
        let ctx = SelectCtx {
            host: Some(handle.session().dst_host.to_string()),
        };
        // Not `resolve_ready`: the round an `evaluate-before-use` group
        // would wait for may itself need this very lookup to reach a
        // host-named member's server (M3 design §7.4) — waiting here would
        // wait on itself. The dial still asks for a round (`choose`); it
        // just resolves with whatever the group already has.
        let resolution = registry.resolve_with(&policy, &ctx);
        handle.set_policy_chain(resolution.chain.clone());
        // Reaching a proxy that is configured by host name would need the very
        // lookup this session is carrying (M3 design §7.4), so bypass it before
        // dialling. A proxy configured by IP literal has no such loop. A chain
        // that ends at REJECT below the proxy is bypassed like a REJECT of the
        // session itself.
        if resolution.terminal == TerminalKind::Proxy
            && let Some(terminal) = resolution.chain.last()
        {
            match socket_opener(registry, terminal) {
                Some(spec) if named_server(spec) => {
                    tracing::warn!(
                        policy = %spec.name,
                        "DNS session routed to a proxy configured by host name; connecting directly to avoid a resolution loop"
                    );
                    return InternalRoute::Bypass(
                        "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
                    );
                }
                None => {
                    tracing::warn!(
                        policy = %terminal,
                        "DNS session routed to a proxy chain that ends at a reject; connecting directly to keep DNS working"
                    );
                    return InternalRoute::Bypass(REJECT_BYPASSED);
                }
                Some(_) => {}
            }
        }
        InternalRoute::Policy(resolution)
    }

    /// Dials a DNS upstream connection through the pipeline (Internal session).
    /// Never lets the routing break DNS: a REJECT, and a proxy that would have
    /// to be resolved first, each warn and connect directly instead.
    ///
    /// `address` is the address the session's server was resolved to, when
    /// the server was written as a name (`rurge-dns`'s bootstrap): the IP
    /// rules match by it, and what is dialled is that address, whatever the
    /// rules pick (phase 3 / M5 design 9.3, 9.4).
    pub async fn dial_internal(
        &self,
        session: SessionInfo,
        address: Option<IpAddr>,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedStream> {
        let (rt, registry) = self.snapshot();
        let handle = self.new_handle(session);
        let trail = handle.trail().clone();
        trail
            .follow(self.connect_internal(rt, registry, handle, address, fallback))
            .await
    }

    /// `dial_internal` from the session's handle on, run inside the trail of
    /// the session.
    async fn connect_internal(
        &self,
        rt: Arc<Runtime>,
        registry: Arc<PolicyRegistry>,
        handle: Arc<SessionHandle>,
        address: Option<IpAddr>,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedStream> {
        if self.is_suspended() {
            return Err(refused_internal(&handle));
        }
        let target = internal_target(&handle, address);
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
        let resolution = match self.route_internal(&rt, &registry, &handle, address).await {
            InternalRoute::Policy(resolution) => resolution,
            InternalRoute::Bypass(why) => {
                return bypass_to_direct(handle, fallback, &target, &opts, why).await;
            }
        };
        let connected = resolution.outbound.connect_tcp(&target, &opts).await;
        report_internal(
            &registry,
            &resolution,
            &handle,
            connected.as_ref().map(|_| ()),
        );
        match connected {
            Ok(stream) => Ok(crate::dns_pipeline::wrap_internal(stream, handle)),
            Err(OutboundError::Reject(_)) | Err(OutboundError::Unsupported(_)) => {
                tracing::warn!(
                    dst = %handle.session().dst_host,
                    "DNS session routed to a reject/unsupported policy; connecting directly to keep DNS working"
                );
                bypass_to_direct(handle, fallback, &target, &opts, REJECT_BYPASSED).await
            }
            Err(OutboundError::Dns(m)) => {
                handle.finish(SessionOutcome::Failed(m.clone()));
                Err(io::Error::other(m))
            }
            Err(OutboundError::Io(e)) => {
                let msg = e.to_string();
                handle.finish(SessionOutcome::Failed(msg));
                Err(e)
            }
            Err(OutboundError::Timeout) => {
                handle.finish(SessionOutcome::Failed("connect timed out".into()));
                Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out"))
            }
            Err(
                e @ (OutboundError::Proxy(_)
                | OutboundError::Tls(_)
                | OutboundError::Unavailable(_)),
            ) => {
                let message = e.to_string();
                handle.finish(SessionOutcome::Failed(message.clone()));
                Err(io::Error::other(message))
            }
        }
    }

    /// Opens the QUIC carrier of a DNS server (DoQ, DoH3) through the
    /// pipeline, as an Internal UDP session (phase 2 M7 design 7.3): the UDP
    /// of the policy the rules pick — DIRECT's with its socket settings, as
    /// a TCP session gets DIRECT's TCP. The routing keeps DNS working as
    /// `dial_internal`'s does: a REJECT, and a proxy that would have to be
    /// resolved first, each warn and go directly instead. A proxy that
    /// carries no UDP is dealt with as for any UDP flow
    /// (`udp-policy-not-supported-behaviour`): the server fails this time —
    /// the resolver asks its other servers —, or the carrier goes through
    /// DIRECT's UDP, and the record says so.
    pub async fn dial_internal_udp(
        &self,
        session: SessionInfo,
        address: Option<IpAddr>,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedDatagram> {
        let (rt, registry) = self.snapshot();
        let handle = self.new_handle(session);
        let trail = handle.trail().clone();
        trail
            .follow(self.open_internal_udp(rt, registry, handle, address, fallback))
            .await
    }

    /// `dial_internal_udp` from the session's handle on, run inside the trail
    /// of the session.
    async fn open_internal_udp(
        &self,
        rt: Arc<Runtime>,
        registry: Arc<PolicyRegistry>,
        handle: Arc<SessionHandle>,
        address: Option<IpAddr>,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedDatagram> {
        if self.is_suspended() {
            return Err(refused_internal(&handle));
        }
        let target = internal_target(&handle, address);
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
        let resolution = match self.route_internal(&rt, &registry, &handle, address).await {
            InternalRoute::Policy(resolution) => resolution,
            InternalRoute::Bypass(why) => {
                return bypass_udp_to_direct(handle, fallback, &target, &opts, why).await;
            }
        };
        // a TCP session learns of a REJECT from `connect_tcp`; `open_udp`
        // says of it only what it says of any outbound without UDP
        if resolution.terminal == TerminalKind::Reject {
            tracing::warn!(
                dst = %handle.session().dst_host,
                "DNS session routed to a reject/unsupported policy; connecting directly to keep DNS working"
            );
            return bypass_udp_to_direct(handle, fallback, &target, &opts, REJECT_BYPASSED).await;
        }
        // what the resolver hears names the policy: the text alone could
        // pass for the DNS server's own (`tls: …`)
        let mut via = resolution
            .chain
            .last()
            .map(String::as_str)
            .unwrap_or_default();
        let mut outbound = resolution.outbound.clone();
        // the member of a `smart` group is told how its carrier opened — not
        // when DIRECT carries it in the member's place
        let mut report = true;
        if outbound.udp() == UdpSupport::Unsupported {
            match rt.config.general.udp_policy_not_supported_behaviour {
                UdpFallback::Reject => {
                    let why = "policy does not support UDP";
                    handle.finish(SessionOutcome::Failed(why.to_string()));
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        format!("via {via}: {why}"),
                    ));
                }
                UdpFallback::Direct => {
                    handle.set_error("policy does not support UDP; sent through DIRECT");
                    let ctx = SelectCtx {
                        host: Some(handle.session().dst_host.to_string()),
                    };
                    let direct = PolicyRef::Builtin(Builtin::Direct);
                    outbound = registry.resolve_with(&direct, &ctx).outbound;
                    via = "DIRECT";
                    report = false;
                }
            }
        }
        let opened = outbound.open_udp(&opts).await;
        if report {
            report_internal(&registry, &resolution, &handle, opened.as_ref().map(|_| ()));
        }
        match opened {
            // the server is an address: nothing for the carrier to look up
            Ok(socket) => Ok(crate::dns_pipeline::wrap_internal_udp(
                packet_datagram(socket, target),
                handle,
                Some(via),
            )),
            Err(e) => {
                // the session opened no carrier: the TCP connection its dial
                // rode first is no far end of it (the phase 4 / M4b plan's
                // P8 ③)
                handle.trail().clear_peer();
                handle.finish(crate::udp::failed(&handle, e.to_string()));
                Err(io::Error::other(format!("via {via}: {e}")))
            }
        }
    }
}

impl Engine {
    /// Stop accepting new connections (in-flight sessions keep running).
    pub fn stop_accepting(&self) {
        self.accept.cancel();
    }
    /// A token that fires when `stop_accepting` is called (at once if it
    /// has been): what ends with the engine's accepting waits on it, as the
    /// listeners do.
    pub(crate) fn accepting_token(&self) -> CancellationToken {
        self.accept.child_token()
    }
    /// Force every in-flight relay to end now.
    pub fn cancel_sessions(&self) {
        self.sessions_root.cancel();
    }
    pub fn tracker(&self) -> &TaskTracker {
        &self.tracker
    }
    /// The next session number, for a handle that is not made by
    /// `new_handle_with` (the HTTP engine's carrier connection).
    pub(crate) fn next_id(&self) -> u64 {
        self.next_session.fetch_add(1, Ordering::Relaxed) + 1
    }
    /// The engine itself, for what has to hold an owned `Arc`: the
    /// connection's `Upstreams`. `None` while the engine is going away.
    pub(crate) fn self_arc(&self) -> Option<Arc<Engine>> {
        self.me.upgrade()
    }
}

impl Engine {
    /// Finished and in-flight requests (M4 `GET /v1/requests`, `GET /v1/active`).
    pub fn request_log(&self) -> &RequestLog {
        &self.observe.log
    }
    /// Cumulative and per-second traffic counters (M4 `GET /v1/traffic`).
    pub fn traffic(&self) -> &TrafficStats {
        &self.observe.traffic
    }
    /// The HTTP engine's counters (phase 4 M1 `GET /v1/rurge/http`).
    pub fn http_counts(&self) -> crate::serve::HttpCounts {
        crate::serve::HttpCounts {
            sniffed: self.sniff_counts.snapshot(),
            requests: self.request_count.load(Ordering::Relaxed),
            mitm: self.mitm_counts.snapshot(),
            rewrite: self.shared.rewrite.snapshot(),
            quic: self.quic_counts.snapshot(),
        }
    }
    /// Kills an in-flight session by id (M4 `POST /v1/requests/{id}/kill`).
    pub fn kill(&self, id: u64) -> bool {
        self.observe.log.kill(id)
    }
    /// Starts the 1 Hz traffic-rate sampler on the engine's tracker. Call once
    /// after `new`, inside a tokio runtime.
    pub fn start_sampler(self: &Arc<Self>) {
        let engine = self.clone();
        self.tracker.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = engine.accept.cancelled() => break,
                    _ = tick.tick() => {
                        let total = engine.observe.log.snapshot_bytes(&engine.observe.traffic);
                        engine.observe.traffic.sample(total);
                    }
                }
            }
        });
    }
}

/// The URL of a request as the session log names it: the part before its
/// query string and its fragment. A log line keeps the path, which names the
/// request, not what follows it — a query string often carries a token
/// (`phase 4 M1 design 12`, `phase 4 design P4-D25`). A session that is no
/// HTTP request has no URL, and none is written.
fn logged_url(url: Option<&str>) -> String {
    let Some(url) = url else {
        return String::new();
    };
    url.split(['?', '#']).next().unwrap_or(url).to_string()
}

fn log_session(h: &SessionHandle, outcome: &SessionOutcome) {
    let s = h.session();
    let (up, down) = h.bytes();
    let dst = crate::observe::destination(&s.dst_host, s.dst_port);
    let rule = h.rule().unwrap_or_default();
    let policy = h.policy_chain().join(" > ");
    let elapsed_ms = h.elapsed().as_millis() as u64;
    // a session that is no HTTP request has neither, and an empty value is
    // written as `rule` is when no rule decided the session
    let method = h.method().unwrap_or_default();
    let status = h.status().map(|code| code.to_string()).unwrap_or_default();
    match outcome {
        SessionOutcome::Completed => {
            // only this line carries the path, and only at DEBUG; the rejected
            // and failed lines are INFO and leave it out (design 12)
            let url = logged_url(s.url.as_deref());
            tracing::debug!(
                session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
                up, down, elapsed_ms, method = %method, status = %status, url = %url, "session completed"
            )
        }
        SessionOutcome::Rejected(kind) => tracing::info!(
            session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
            elapsed_ms, method = %method, status = %status, error = ?h.error(), "session rejected by {}", kind.name()
        ),
        SessionOutcome::Failed(error) => tracing::info!(
            session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
            up, down, elapsed_ms, method = %method, status = %status, error = %error, "session failed"
        ),
    }
}

fn fail(
    handle: Arc<SessionHandle>,
    kind: FailKind,
    message: impl Into<String>,
) -> Result<Dialed, DialError> {
    let message = message.into();
    // A note already on the handle (a group cycle, an unsupported protocol,
    // an empty group standing in for DIRECT) explains why the session was
    // routed this way, not why the dial itself then failed: the record needs
    // both, or it hides the failure behind the note. The `DialError` below
    // keeps the plain failure message.
    settle(
        &handle,
        &Refused::Fail {
            kind,
            message: message.clone(),
        },
    );
    handle.finish(SessionOutcome::Failed(message.clone()));
    let rule = handle.rule();
    Err(DialError::Failed {
        kind,
        message,
        rule,
        handle,
    })
}

/// Finishes a session its routing refused, the way `connect_session` did
/// (`P4`): a failure sets the error and ends, a rejection only ends.
fn refused_dial(handle: Arc<SessionHandle>, refused: Refused) -> Result<Dialed, DialError> {
    match refused {
        Refused::Fail { kind, message } => fail(handle, kind, message),
        Refused::Reject(kind) => reject(handle, kind),
    }
}

/// Connects `outbound` to `target`: a plain request goes in absolute form to
/// an HTTP proxy when `may_forward` (M1 design 6.5), anything else through a
/// tunnel.
async fn connect_through(
    outbound: &OutboundRef,
    target: &Target,
    may_forward: bool,
    opts: &ConnectOpts,
) -> Result<(BoxedStream, Option<Vec<(String, String)>>), OutboundError> {
    match outbound.http_forward().filter(|_| may_forward) {
        Some(proxy) if rurge_proto::http::valid_target(target) => proxy
            .connect(opts)
            .await
            .map(|stream| (stream, Some(proxy.request_headers()))),
        Some(_) => Err(OutboundError::Proxy(
            "the target host name is not valid for an HTTP proxy request".to_string(),
        )),
        None => outbound
            .connect_tcp(target, opts)
            .await
            .map(|stream| (stream, None)),
    }
}

fn reject(handle: Arc<SessionHandle>, kind: rurge_proto::RejectKind) -> Result<Dialed, DialError> {
    handle.finish(SessionOutcome::Rejected(kind));
    let rule = handle.rule();
    Err(DialError::Reject { kind, rule, handle })
}

/// Looks up the raw text of the rule at `index` (M2a `CompiledRule.index` is
/// the rule's position in `Config.rules`, and `RuleEngine::rules()` returns
/// them in that ascending order), so a binary search finds it.
pub(crate) fn rule_raw(rules: &rurge_rules::RuleEngine, index: usize) -> Option<String> {
    let all = rules.rules();
    all.binary_search_by_key(&index, |r| r.index)
        .ok()
        .map(|pos| all[pos].raw.clone())
}

/// The `always-capture` of the rule at `index` (phase 4 M4 design 5.2): the
/// session the line names, or nothing when the line names none — an empty one
/// is one of the latter, and the profile check says so (`W0012`).
pub(crate) fn always_capture(rules: &rurge_rules::RuleEngine, index: usize) -> Option<Arc<str>> {
    let all = rules.rules();
    let pos = all.binary_search_by_key(&index, |r| r.index).ok()?;
    let name = all[pos].params.always_capture.as_deref()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(Arc::from(name))
}

/// A session its routing refused, before or while dialing: whoever asked for
/// the routing answers it and finishes the handle (`P4`).
pub(crate) enum Refused {
    Reject(rurge_proto::RejectKind),
    Fail { kind: FailKind, message: String },
}

/// What `route_session` worked out and `dial_route` needs (`P21` V11).
pub(crate) struct Way {
    pub(crate) target: Target,
    pub(crate) may_forward: bool,
    pub(crate) attempts: Vec<Resolution>,
    pub(crate) book: Arc<rurge_policy::smart::SmartBook>,
}

/// Sets the handle's error the way `fail` does, without finishing it (`P4`):
/// a note already on the handle (a group cycle, an unsupported protocol, an
/// empty group standing in for DIRECT) explains why the session was routed
/// this way, not why the dial itself then failed, so a failure gets both, or
/// it hides behind the note.
pub(crate) fn settle(handle: &SessionHandle, refused: &Refused) {
    if let Refused::Fail { message, .. } = refused
        && let Some(note) = handle.error()
    {
        handle.set_error(format!("{note}; {message}"));
    }
}

impl Engine {
    /// The rules and the policy (design 8.4, routing), up to the attempts in
    /// line; the dial itself is `dial_route`. A refusal comes back unfinished:
    /// the caller answers and finishes the handle (`P4`).
    pub(crate) async fn route_session(
        &self,
        rt: &Arc<Runtime>,
        registry: &Arc<PolicyRegistry>,
        handle: &Arc<SessionHandle>,
        forwardable: bool,
    ) -> Result<Way, Refused> {
        // Refused before anything is asked of the rules, a name lookup or a
        // `smart` group: each would count a session that goes nowhere. The
        // lookup of the session's process, which `dial` started with the
        // session, is left to finish: the record of a refused session still
        // names its process.
        if self.is_suspended() {
            return Err(Refused::Fail {
                kind: FailKind::Other,
                message: REFUSED_STRICT.to_string(),
            });
        }
        let policy = match self.choose_policy(rt, registry, handle).await {
            Chosen::Policy(p) => p,
            Chosen::DnsFailed => {
                return Err(Refused::Fail {
                    kind: FailKind::Dns,
                    message: "dns lookup failed".to_string(),
                });
            }
        };
        // `load-balance` with `persistent=true` picks by the host
        let ctx = SelectCtx {
            host: Some(handle.session().dst_host.to_string()),
        };
        let mut resolution = match resolve_ready(registry, &policy, &ctx).await {
            Ok(resolution) => resolution,
            Err(chain) => {
                handle.set_policy_chain(chain);
                return Err(Refused::Fail {
                    kind: FailKind::Connect,
                    message: EVALUATION_FAILED.to_string(),
                });
            }
        };
        handle.set_policy_chain(resolution.chain.clone());
        if let Some(note) = &resolution.note {
            // A protocol rurge does not speak, a group cycle, a group
            // without members: say why in the session log (§7.2; phase 2
            // M3 design 5.6).
            handle.set_error(note.to_string());
        }
        let mut target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
        // FR-DNS-07: a proxy normally gets the name and resolves it itself;
        // with `use-local-host-item-for-proxy`, an address pinned in [Host]
        // goes to the proxy instead: one of them, drawn anew for every
        // session, when the entry lists several. Alias / server items leave
        // the name alone.
        let pinned_ip = match &target.host {
            HostName::Domain(name)
                if resolution.terminal == rurge_policy::TerminalKind::Proxy
                    && rt.config.general.use_local_host_item_for_proxy =>
            {
                rt.stack
                    .resolver
                    .host_lookup(name)
                    .and_then(|hit| match hit.action {
                        rurge_dns::hosts::HostAction::Ips(ips) => {
                            ips.get(rurge_policy::random_below(ips.len())).copied()
                        }
                        _ => None,
                    })
            }
            _ => None,
        };
        let pinned = pinned_ip.is_some();
        if let Some(ip) = pinned_ip {
            target = Target::new(HostName::Ip(ip), target.port);
        }
        // a name no member can send is refused by all of them alike: the
        // session goes through the one the group picked as through a
        // policy of its own — no other is tried, and the group is told
        // nothing of it (phase 2 M8 design 5.5)
        if resolution.smart.is_some() && crate::smart::unsendable(&target) {
            resolution.smart = None;
        }
        // a pinned address only reaches the proxy through a tunnel: in
        // absolute form the request URI carries the name
        let may_forward = forwardable && !pinned;
        let book = registry.auto().smart.clone();
        // the member a `smart` group picked, then the next ones in line
        // while they do not connect (phase 2 M3c design §7)
        let attempts = crate::smart::attempts(registry, resolution);
        Ok(Way {
            target,
            may_forward,
            attempts,
            book,
        })
    }

    /// Dials what `route_session` chose (`P4`): the attempts in line, each
    /// with its share of the time that is left, and the failure mapped the way
    /// it was.
    pub(crate) async fn dial_route(
        &self,
        handle: &Arc<SessionHandle>,
        way: Way,
    ) -> Result<(BoxedStream, Option<Vec<(String, String)>>), Refused> {
        let Way {
            target,
            may_forward,
            attempts,
            book,
        } = way;
        // the `smart` group is told of the dial by the name the rules saw
        let host = handle.session().dst_host.to_string();
        let begun = Instant::now();
        let mut tried: Vec<String> = Vec::new();
        let mut failure = None;
        for (k, attempt) in attempts.iter().enumerate() {
            // what the attempt before it connected to is not this one's
            handle.trail().clear_peer();
            if k > 0 {
                handle.set_policy_chain(attempt.chain.clone());
            }
            // every try gets its share of the time that is left
            let timeout = match attempts.len() {
                1 => CONNECT_TIMEOUT,
                n => CONNECT_TIMEOUT.saturating_sub(begun.elapsed()) / (n - k) as u32,
            };
            let opts = ConnectOpts { timeout };
            let connecting = connect_through(&attempt.outbound, &target, may_forward, &opts);
            let connected = if attempts.len() == 1 {
                connecting.await
            } else {
                tokio::time::timeout(timeout, connecting)
                    .await
                    .unwrap_or(Err(OutboundError::Timeout))
            };
            let e = match connected {
                Ok((stream, forward)) => {
                    handle.mark_connected();
                    if let Some(pick) = &attempt.smart {
                        book.used(&pick.group, &pick.member, Instant::now());
                        let outbound = &attempt.outbound;
                        crate::smart::watch(
                            handle,
                            book.clone(),
                            &pick.member,
                            outbound,
                            &host,
                            true,
                        );
                        if !tried.is_empty() {
                            handle.set_error(format!(
                                "smart group `{}`: {} failed to connect, used `{}`",
                                pick.group,
                                crate::smart::quoted(&tried),
                                pick.member
                            ));
                        }
                    }
                    return Ok((stream, forward));
                }
                Err(e) => e,
            };
            let retry = crate::smart::retryable(&e);
            if let Some(pick) = &attempt.smart
                && retry
            {
                let (outbound, now) = (&attempt.outbound, Instant::now());
                book.report_failure(&pick.member, outbound, Some(&host), now);
                tried.push(pick.member.clone());
            }
            failure = Some(e);
            if !retry {
                break;
            }
        }
        if tried.len() > 1
            && let Some(pick) = attempts.first().and_then(|a| a.smart.as_ref())
        {
            handle.set_error(format!(
                "smart group `{}`: tried {}",
                pick.group,
                crate::smart::quoted(&tried)
            ));
        }
        Err(match failure.expect("a dial was tried") {
            OutboundError::Reject(kind) => {
                let effective = self
                    .escalation
                    .refusal(handle.session(), kind, Instant::now());
                Refused::Reject(effective)
            }
            OutboundError::Unsupported(what) => {
                // what the outbound cannot do here (M4-D7): say so
                handle.set_error(Note::Unsupported(what).to_string());
                Refused::Reject(rurge_proto::RejectKind::Reject)
            }
            OutboundError::Dns(m) => Refused::Fail {
                kind: FailKind::Dns,
                message: m,
            },
            OutboundError::Io(e) => Refused::Fail {
                kind: FailKind::Connect,
                message: e.to_string(),
            },
            OutboundError::Timeout => Refused::Fail {
                kind: FailKind::Timeout,
                message: "connect timed out".to_string(),
            },
            e @ (OutboundError::Proxy(_)
            | OutboundError::Tls(_)
            | OutboundError::Unavailable(_)) => Refused::Fail {
                kind: FailKind::Connect,
                message: e.to_string(),
            },
        })
    }

    /// `Dialer::dial` from the session's handle on, run inside the trail of
    /// the session.
    async fn connect_session(
        &self,
        rt: Arc<Runtime>,
        registry: Arc<PolicyRegistry>,
        handle: Arc<SessionHandle>,
    ) -> Result<Dialed, DialError> {
        // Sessions that come through `Dialer::dial` are tunnels (design 8.4):
        // the HTTP listener's plain requests reach the engine through
        // `plain_requests` instead, where a request may be forwarded in
        // absolute form.
        let way = match self.route_session(&rt, &registry, &handle, false).await {
            Ok(way) => way,
            Err(refused) => return refused_dial(handle, refused),
        };
        match self.dial_route(&handle, way).await {
            Ok((stream, forward)) => Ok(Dialed {
                stream,
                handle,
                forward,
            }),
            Err(refused) => refused_dial(handle, refused),
        }
    }

    /// `Dialer::dial` and the dial of a sniffed session: the handle is made
    /// here, and the note the record carries is written on it before anything
    /// is dialed (`phase 4 M4 design 10.2`, the phase 4 / M4b plan's P6 ⑤).
    pub(crate) async fn dial_noted(
        &self,
        mut session: SessionInfo,
        passed: PassedBy,
    ) -> Result<Dialed, DialError> {
        // loaded once, together: the name is picked, approved and
        // resolved against the same generation
        let (rt, registry) = self.snapshot();
        // a fake address that came in at a proxy port is the name behind
        // it before anything else sees the session (phase 3 M3 design 13)
        let turned = self.turn_back(&mut session);
        let process = self.look_up_owner(&session);
        let handle = self.new_handle_with(session, process);
        match turned {
            Turned::Unchanged => {}
            // the address stays the name's as long as the session lasts
            Turned::Name(lease) => handle.on_finish(move |_, _| drop(lease)),
            // nothing was dialed, so there is nothing to say about it
            Turned::Unknown => return fail(handle, FailKind::Other, FAKE_ADDRESS_NOT_KNOWN),
        }
        // the reason the session is passed through, if there is one: what the
        // sniff decided for a session it saw, and the two lists for one no
        // one sniffed (`phase 4 M4 design 10.2`). A session the lists took is
        // noted here too when pre-matching refused it: the lists are asked
        // before the probe (`the phase 4 / M4b plan's P6`).
        let note = match passed {
            PassedBy::Sniff(note) => note,
            PassedBy::Lists if self.judged_at_the_entry(handle.session()) => raw_listed(
                &rt.config.general,
                &handle.session().dst_host,
                handle.session().dst_port,
            ),
            PassedBy::Lists => None,
        };
        if let Some(note) = note {
            handle.add_note(note);
        }
        // A client's TCP session to a host whose QUIC the engine blocked
        // is that client falling back (phase 3 M5 design 6.3): the table
        // of `block-quic=adaptive` hears of it. The engine's own sessions
        // come through `dial_internal` instead, so they cannot pass for a
        // client's.
        self.quic_adaptive
            .on_tcp(&handle.session().dst_host.to_string(), Instant::now());
        let trail = handle.trail().clone();
        trail
            .follow(self.connect_session(rt, registry, handle))
            .await
    }

    /// The first two steps of `sniffable_session`: a TCP session of a client's
    /// entry, in an engine that is not paused. These are the sessions the two
    /// lists judge at all (`phase 4 M4 design 10.2`, the phase 4 / M4b plan's
    /// P6 ⑤).
    fn judged_at_the_entry(&self, session: &SessionInfo) -> bool {
        session.transport == Transport::Tcp
            && matches!(
                session.listener,
                ListenerKind::Http | ListenerKind::Socks5 | ListenerKind::Tun
            )
            && !self.is_suspended()
    }
}

/// How a session came to be passed through, or that no one sniffed it
/// (`phase 4 M4 design 10.2`, the phase 4 / M4b plan's P6 ⑤): what
/// `dial_noted` writes on the record when it makes the handle.
pub(crate) enum PassedBy {
    /// A session no one sniffed: the two lists are asked at the dial, in the
    /// order `sniffable_session` asks them.
    Lists,
    /// A sniffed session: the reason the sniff or the decision left, if any.
    Sniff(Option<&'static str>),
}

impl Dialer for Engine {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        Box::pin(self.dial_noted(session, PassedBy::Lists))
    }

    fn relay<'a>(
        &'a self,
        client: BoxedStream,
        upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()> {
        let idle = self.runtime().idle_timeout;
        Box::pin(crate::relay::pump(client, upstream, handle, idle))
    }

    fn sniffable(&self, session: &SessionInfo) -> bool {
        self.sniffable_session(session)
    }

    fn serve<'a>(
        &'a self,
        session: SessionInfo,
        client: BoxedStream,
        opts: ServeOpts,
    ) -> BoxFuture<'a, ()> {
        Box::pin(self.serve_sniffed(session, client, opts))
    }

    fn admit_udp(&self) -> UdpAdmission {
        if self.udp_associations.load(Ordering::Relaxed) < crate::udp::ASSOCIATIONS {
            UdpAdmission::Accepted
        } else {
            self.warn_udp_limit("udp: too many associations; the new association is refused");
            UdpAdmission::Busy
        }
    }

    fn associate<'a>(
        &'a self,
        client: Arc<dyn UdpClient>,
        session: SessionInfo,
        closed: CancellationToken,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            let Some(engine) = self.me.upgrade() else {
                return;
            };
            /// One association counted while it is served.
            struct Counted<'e>(&'e std::sync::atomic::AtomicUsize);
            impl Drop for Counted<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::Relaxed);
                }
            }
            let before = self.udp_associations.fetch_add(1, Ordering::Relaxed);
            let _counted = Counted(&self.udp_associations);
            if before >= crate::udp::ASSOCIATIONS {
                self.warn_udp_limit("udp: too many associations; the new association is refused");
                return;
            }
            crate::udp::serve(engine, client, session, closed).await;
        })
    }

    fn plain_requests(&self, conn: ClientConn) -> Option<Arc<dyn PlainRequests>> {
        let engine = self.me.upgrade()?;
        // One session for the connection; every request replaces its target
        // with its URL's (`rurge-http`), so this target is only a placeholder
        // until the first request arrives (`phase 4 M1 design 9`).
        let mut session = SessionInfo::tcp(HostName::Ip(Ipv4Addr::UNSPECIFIED.into()), 0);
        session.src = conn.src;
        session.in_port = conn.in_port;
        session.local = conn.local;
        session.listener = ListenerKind::Http;
        session.transport = Transport::Tcp;
        // The process behind the connection is looked up once, for all its
        // requests: the lookup reads the listener, the transport, the source,
        // the local address and the listener port, never the target
        // (`owner.rs`, `query_for`).
        let process = self.look_up_owner(&session);
        let token = self.session_token();
        let conn = rurge_http::Conn {
            session,
            entry: rurge_http::Entry::Listener,
            origin: None,
            idle: self.runtime().idle_timeout,
            // The listener's own hyper service times a request head with its
            // `handshake_timeout`; `head_wait` is only for
            // `rurge_http::serve_connection`, which this path does not use.
            head_wait: HANDSHAKE_TIMEOUT,
            drop_hold: DROP_HOLD,
            abort: conn.abort,
            token: token.clone(),
            tracker: self.tracker().clone(),
            // the listener speaks HTTP/1.1 (`phase 4 M2 design 10.1`)
            h2: false,
        };
        // The listener has no stop of its own: an upgraded relay is ended by
        // the connection's own token, which a stop of the engine cancels.
        let upstreams = crate::http::ConnUpstreams::new(engine, token, process, None, None, true);
        Some(Arc::new(crate::http::PlainRequests::new(
            rurge_http::Requests::new(conn, upstreams),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use std::path::Path;

    fn general(text: &str) -> General {
        let loaded = from_text(
            &format!("[General]\n{text}\n[Proxy]\n[Rule]\nFINAL,DIRECT\n"),
            Path::new("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(!loaded.diagnostics.has_errors());
        loaded.config.general
    }

    #[test]
    fn listener_specs_follow_the_profile_and_defaults() {
        let specs = Engine::listener_specs(&general(""));
        assert_eq!(specs.len(), 2);
        assert_eq!(
            (specs[0].kind, specs[0].addr.to_string()),
            (ListenerKind::Http, "127.0.0.1:6152".to_string())
        );
        assert_eq!(
            (specs[1].kind, specs[1].addr.to_string()),
            (ListenerKind::Socks5, "127.0.0.1:6153".to_string())
        );
        let specs = Engine::listener_specs(&general(
            "http-listen = s3cret@127.0.0.1:7000, [::1]:7001\nsocks5-listen = 127.0.0.1:7002",
        ));
        assert_eq!(specs.len(), 3);
        assert_eq!(
            specs[0].auth,
            Some(HttpAuth::Password("s3cret".to_string()))
        );
        assert_eq!(specs[1].addr.to_string(), "[::1]:7001");
        assert_eq!(specs[2].kind, ListenerKind::Socks5);
        let specs = Engine::listener_specs(&general(
            "allow-wifi-access = true\nwifi-access-http-port = 8080\nwifi-access-socks5-port = 8081\nwifi-access-http-auth = alice:pw",
        ));
        assert_eq!(specs[0].addr.to_string(), "0.0.0.0:8080");
        assert_eq!(
            specs[0].auth,
            Some(HttpAuth::UserPass {
                user: "alice".into(),
                password: "pw".into()
            })
        );
        assert_eq!(specs[1].addr.to_string(), "0.0.0.0:8081");
    }

    /// `allow-wifi-access` and the `wifi-access-*` keys stand in for the
    /// listeners only when the profile writes none: one that writes a kind of
    /// listener gets that kind and no other.
    #[test]
    fn the_wifi_access_keys_stand_in_only_where_no_listener_is_written() {
        let wifi = "allow-wifi-access = true\nwifi-access-http-port = 8080\nwifi-access-socks5-port = 8081\nwifi-access-http-auth = alice:pw\n";
        let specs =
            Engine::listener_specs(&general(&format!("{wifi}http-listen = 127.0.0.1:7000")));
        let found: Vec<_> = specs.iter().map(|s| (s.kind, s.addr.to_string())).collect();
        assert_eq!(found, [(ListenerKind::Http, "127.0.0.1:7000".to_string())]);
        assert_eq!(specs[0].auth, None);
        let specs =
            Engine::listener_specs(&general(&format!("{wifi}socks5-listen = 127.0.0.1:7002")));
        let found: Vec<_> = specs.iter().map(|s| (s.kind, s.addr.to_string())).collect();
        assert_eq!(
            found,
            [(ListenerKind::Socks5, "127.0.0.1:7002".to_string())]
        );
    }

    /// What a listener is bound with comes from the profile: the switches of
    /// `[General]`, and the credentials of the address it listens on.
    #[test]
    fn the_listener_options_follow_the_profile() {
        let spec = |auth| ListenerSpec {
            kind: ListenerKind::Http,
            addr: "127.0.0.1:6152".parse().unwrap(),
            auth,
        };
        let kept = Engine::listener_opts(&general(""), &spec(None));
        assert!(kept.restrict_to_lan && kept.show_error_page && !kept.show_error_page_for_reject);
        assert!(kept.auth.is_none());
        assert_eq!(kept.drop_hold, Duration::from_secs(30));
        let changed = Engine::listener_opts(
            &general(
                "proxy-restricted-to-lan = false\nshow-error-page = false\nshow-error-page-for-reject = true",
            ),
            &spec(Some(HttpAuth::Password("pw".to_string()))),
        );
        assert!(
            !changed.restrict_to_lan
                && !changed.show_error_page
                && changed.show_error_page_for_reject
        );
        assert_eq!(changed.auth, Some(HttpAuth::Password("pw".to_string())));
        assert_eq!(changed.kind, ListenerKind::Http);
    }

    /// A change in any option the listeners were bound with is a change of
    /// the surface a reload compares, so that the listeners are bound again
    /// with it rather than left as they were.
    #[test]
    fn the_listener_surface_changes_with_each_option_the_listeners_are_bound_with() {
        use crate::reload::listener_surface;
        let surface = |text: &str| listener_surface(&general(text));
        assert_eq!(surface(""), surface(""));
        for changed in [
            "proxy-restricted-to-lan = false",
            "show-error-page = false",
            "show-error-page-for-reject = true",
        ] {
            assert_ne!(surface(changed), surface(""), "{changed}");
        }
        assert_ne!(
            surface("http-listen = a@127.0.0.1:7000"),
            surface("http-listen = b@127.0.0.1:7000"),
            "a password rotated"
        );
    }

    #[test]
    fn escalation_after_threshold_within_window() {
        let esc = Escalation::new();
        let t0 = std::time::Instant::now();
        for i in 0..(ESCALATE_COUNT - 1) {
            esc.record("ads.test", t0 + Duration::from_millis(i as u64));
        }
        assert!(!esc.should_drop("ads.test", t0 + Duration::from_secs(1)));
        esc.record("ads.test", t0 + Duration::from_secs(1));
        assert!(esc.should_drop("ads.test", t0 + Duration::from_secs(1)));
        // a different host is unaffected
        assert!(!esc.should_drop("other.test", t0 + Duration::from_secs(1)));
        // once the window slides past, the count decays
        assert!(!esc.should_drop("ads.test", t0 + ESCALATE_WINDOW + Duration::from_secs(1)));
    }

    /// The manual's threshold, in its own numbers: the fiftieth reject of one
    /// host within thirty seconds is the first to be upgraded, and the count
    /// falls again when the first of the fifty is thirty seconds old.
    #[test]
    fn fifty_rejects_within_thirty_seconds_upgrade_and_not_before() {
        let esc = Escalation::new();
        let t0 = std::time::Instant::now();
        for i in 0..49 {
            esc.record("ads.test", t0 + Duration::from_millis(i));
        }
        assert!(!esc.should_drop("ads.test", t0 + Duration::from_secs(1)));
        esc.record("ads.test", t0 + Duration::from_secs(1));
        assert!(esc.should_drop("ads.test", t0 + Duration::from_secs(1)));
        assert!(esc.should_drop("ads.test", t0 + Duration::from_millis(29_999)));
        assert!(!esc.should_drop("ads.test", t0 + Duration::from_secs(30)));
    }

    /// `refusal` counts the kinds that escalate by the destination of the
    /// session, upgrades the fiftieth within the window, and leaves the other
    /// kinds as they are without counting them.
    #[test]
    fn a_refusal_is_counted_by_the_destination_of_its_session() {
        use rurge_proto::RejectKind;
        let esc = Escalation::new();
        let t0 = std::time::Instant::now();
        let ads = SessionInfo::tcp(HostName::parse("ads.test"), 80);
        let other = SessionInfo::tcp(HostName::parse("other.test"), 80);
        for kind in [RejectKind::Drop, RejectKind::NoDrop] {
            for _ in 0..(ESCALATE_COUNT + 1) {
                assert_eq!(esc.refusal(&ads, kind, t0), kind);
            }
        }
        assert_eq!(esc.len(), 0, "a kind that does not escalate is not counted");
        for i in 0..(ESCALATE_COUNT - 1) {
            let kind = if i % 2 == 0 {
                RejectKind::Reject
            } else {
                RejectKind::TinyGif
            };
            assert_eq!(esc.refusal(&ads, kind, t0), kind, "refusal {i}");
        }
        assert_eq!(
            esc.refusal(&other, RejectKind::Reject, t0),
            RejectKind::Reject,
            "another destination has its own count"
        );
        assert_eq!(
            esc.refusal(&ads, RejectKind::TinyGif, t0),
            RejectKind::Drop,
            "the fiftieth, of the two kinds together"
        );
        assert_eq!(
            esc.refusal(&ads, RejectKind::Reject, t0 + ESCALATE_WINDOW),
            RejectKind::Reject,
            "past the window the count has fallen"
        );
    }

    /// A reject flood to distinct hosts (a retry storm against random
    /// subdomains) must not grow the table without limit: nothing is
    /// reclaimable inside the window, so the hosts past the cap are simply
    /// not tracked.
    #[test]
    fn escalation_map_is_bounded() {
        let esc = Escalation::new();
        let t0 = std::time::Instant::now();
        for i in 0..(ESCALATE_MAX_HOSTS + 100) {
            esc.record(&format!("h{i}.test"), t0);
        }
        assert!(esc.len() <= ESCALATE_MAX_HOSTS, "{} tracked", esc.len());
        // the first host past the cap is untracked, so it can never escalate
        let overflow = format!("h{ESCALATE_MAX_HOSTS}.test");
        for _ in 0..ESCALATE_COUNT {
            esc.record(&overflow, t0);
        }
        assert!(!esc.should_drop(&overflow, t0));
        assert!(esc.len() <= ESCALATE_MAX_HOSTS);
        // a host recorded before the cap was reached still escalates
        for _ in 1..ESCALATE_COUNT {
            esc.record("h0.test", t0);
        }
        assert!(esc.should_drop("h0.test", t0));
    }

    /// A client's TCP session tells the table of `block-quic=adaptive` that
    /// the client fell back (design 6.3). The entry is put an hour away from
    /// the end of its window, so that the test depends on no clock of the
    /// machine.
    #[tokio::test]
    async fn a_client_tcp_session_tells_the_table() {
        use crate::quic_adaptive::Adaptive;
        use crate::runtime::RuntimeOptions;
        use crate::stack::StackOptions;
        use rurge_dns::system::StaticSystemDns;
        use rurge_net::socket::NoopSocketHook;
        use rurge_rules::GeoUrls;

        let dir = tempfile::tempdir().unwrap();
        let loaded = from_text(
            "[General]\nipv6 = false\n[Proxy]\n[Rule]\nFINAL,REJECT\n",
            &dir.path().join("t.conf"),
            &LoadOptions::for_tests(),
        );
        assert!(!loaded.diagnostics.has_errors());
        let runtime = Runtime::build(
            loaded.config,
            RuntimeOptions {
                stack: StackOptions {
                    data_dir: dir.path().to_path_buf(),
                    no_network: true,
                    geo_urls: GeoUrls::default(),
                    dns_cache_size: 16,
                    system: Arc::new(StaticSystemDns::default()),
                    wait: Duration::ZERO,
                    dns_connector: None,
                    socket_hook: Arc::new(NoopSocketHook),
                    resource_resolve: None,
                    dns_override: Default::default(),
                },
                outbound_mode: OutboundMode::Rule,
                idle_timeout: Duration::from_secs(60),
                shared: EngineShared::default(),
                request_log_size: 16,
            },
        )
        .await
        .unwrap();
        let engine = Engine::new(runtime);
        let trying = Instant::now() + Duration::from_secs(3600);
        engine
            .quic_adaptive()
            .set("x.test", Adaptive::Trying(trying));
        // a client's session, which the rules of the profile reject: the
        // table hears of it on the way, whatever becomes of the session
        let session = SessionInfo {
            listener: ListenerKind::Socks5,
            ..SessionInfo::tcp(HostName::parse("x.test"), 80)
        };
        assert!(engine.dial(session).await.is_err(), "FINAL,REJECT");
        assert_eq!(
            engine.quic_adaptive().state("x.test"),
            Some(Adaptive::FellBack)
        );
    }
}
