//! The engine (M3 design §7.2 – 7.3): implements `Dialer` for the listeners,
//! relays bytes, binds listeners from `[General]`, writes the session log.

use crate::control::Mode;
use crate::observe::{RequestLog, TrafficStats};
use crate::runtime::Runtime;
use crate::shared::EngineShared;
use crate::state::StateStore;
use arc_swap::ArcSwap;
use rurge_config::Builtin;
use rurge_config::HostName;
use rurge_config::general::General;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_config::spec::PolicySpec;
use rurge_inbound::{
    DialError, Dialed, Dialer, FailKind, HttpAuth, HttpListener, ListenerOpts, Running,
    SessionHandle, SessionOutcome, Socks5Listener,
};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_policy::TerminalKind;
use rurge_proto::OutboundError;
use rurge_rules::{OutboundMode, Outcome};
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
struct Escalation {
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
    fn record(&self, host: &str, now: Instant) {
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

    /// Whether `host` has reached the threshold within the window.
    fn should_drop(&self, host: &str, now: Instant) -> bool {
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
    escalation: Escalation,
    /// Runtime outbound mode and global policy (M4 design §4.1). Seeded from
    /// the first `Runtime`, changed by the API / CLI, never touched by reload.
    mode: ArcSwap<Mode>,
    global_policy: ArcSwap<Option<String>>,
    /// "proxy mode but no usable global policy" is warned once per change.
    global_warned: AtomicBool,
    state: OnceLock<Arc<StateStore>>,
    shared: EngineShared,
}

impl Engine {
    pub fn new(runtime: Runtime) -> Arc<Engine> {
        let observe = Arc::new(Observe {
            log: RequestLog::new(runtime.request_log_size),
            traffic: TrafficStats::new(),
        });
        let (mode, global) = Mode::from_outbound(&runtime.outbound_mode);
        // Publish the first generation before anything can dial through it.
        let shared = runtime.shared.clone();
        shared.cell.store(runtime.policies.clone());
        shared.resolver.store(runtime.stack.resolver.clone());
        let engine = Arc::new(Engine {
            runtime: ArcSwap::from_pointee(runtime),
            next_session: AtomicU64::new(0),
            sessions_root: CancellationToken::new(),
            accept: CancellationToken::new(),
            tracker: TaskTracker::new(),
            observe,
            escalation: Escalation::new(),
            mode: ArcSwap::from_pointee(mode),
            global_policy: ArcSwap::from_pointee(global),
            global_warned: AtomicBool::new(false),
            state: OnceLock::new(),
            shared,
        });
        if let Some(pc) = engine.runtime().dns_pipeline() {
            pc.attach(Arc::downgrade(&engine));
        }
        engine
    }

    /// The current config generation (sessions snapshot it once at dial time).
    pub fn runtime(&self) -> Arc<Runtime> {
        self.runtime.load_full()
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
                        .map(|(user, password)| HttpAuth::UserPass { user, password }),
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

    fn new_handle(&self, session: SessionInfo) -> Arc<SessionHandle> {
        let id = self.next_session.fetch_add(1, Ordering::Relaxed) + 1;
        let handle = SessionHandle::new_with_token(id, session, self.sessions_root.child_token());
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
        self.observe.log.mark_active(&handle);
        handle
    }
}

impl Engine {
    /// The generation-independent objects; every `Runtime` swapped into this
    /// engine must have been built with them.
    pub fn shared(&self) -> EngineShared {
        self.shared.clone()
    }

    /// Makes `next` the generation that outlives-a-reload objects see: chain
    /// connectors resolve against its registry, direct connectors through
    /// its resolver.
    pub(crate) fn publish_generation(&self, next: &Runtime) {
        assert!(
            Arc::ptr_eq(&self.shared.cell, &next.shared.cell)
                && Arc::ptr_eq(&self.shared.resolver, &next.shared.resolver),
            "the next generation must be built with `Engine::shared()`"
        );
        self.shared.cell.store(next.policies.clone());
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

    /// `true` for the built-in policies and every configured policy / group,
    /// against the *current* runtime generation (for API / CLI callers).
    pub fn policy_exists(&self, name: &str) -> bool {
        policy_known(&self.runtime(), name)
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
        let rt = self.runtime();
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
        proxies.extend(rt.config.policies.iter().map(|p| p.name.clone()));
        let groups = rt.config.groups.iter().map(|g| g.name.clone()).collect();
        PoliciesView { proxies, groups }
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

    /// The main profile file of the current config generation.
    pub fn profile_path(&self) -> std::path::PathBuf {
        self.runtime().config.source.main.clone()
    }

    /// The current main profile text; secrets redacted unless `sensitive`.
    pub async fn config_text(&self, sensitive: bool) -> io::Result<String> {
        let path = self.profile_path();
        let text = tokio::fs::read_to_string(&path).await?;
        Ok(if sensitive {
            text
        } else {
            rurge_config::redact::redact_profile(&text)
        })
    }

    /// Mode / rule → policy, shared by `dial` and `dial_internal` (M4 §4.1).
    async fn choose_policy(&self, rt: &Runtime, handle: &SessionHandle) -> Chosen {
        match self.mode() {
            Mode::Direct => return Chosen::Policy(PolicyRef::Builtin(Builtin::Direct)),
            Mode::Proxy => match self.global_policy() {
                Some(name) if policy_known(rt, &name) => {
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
            .evaluate(
                handle.session(),
                OutboundMode::Rule,
                rt.stack.resolver.as_ref(),
            )
            .await;
        if let Some(i) = decision.matched {
            handle.set_rule(rule_raw(&rt.rules, i));
        }
        match decision.outcome {
            Outcome::Policy(p) => Chosen::Policy(p),
            Outcome::DnsFailed => Chosen::DnsFailed,
        }
    }
}

enum Chosen {
    Policy(PolicyRef),
    DnsFailed,
}

/// `true` for the built-in policies and every policy / group configured in
/// `rt` — checked against the *session's own* runtime snapshot, not whatever
/// generation is current when this runs, so a reload racing a dial can never
/// approve a name against one generation's registry and then resolve it
/// (`PolicyRegistry::resolve`) against another's.
fn policy_known(rt: &Runtime, name: &str) -> bool {
    matches!(PolicyRef::parse(name), PolicyRef::Builtin(_)) || rt.policies.contains(name)
}

/// The policy whose SERVER this machine has to reach — and so, if that server
/// is a host name, to resolve — when `name` is dialled: `name` itself, or,
/// following `underlying-proxy` (through a group's current member), the last
/// proxy hop of its chain. A hop whose `underlying-proxy` currently resolves
/// to DIRECT is that last hop: DIRECT opens the socket, and the name it looks
/// up is that hop's own server. `None` when the chain ends at REJECT (it fails
/// fast, it does not loop) or is deeper than the registry allows.
fn socket_opener<'a>(rt: &'a Runtime, name: &str) -> Option<&'a PolicySpec> {
    let mut current = name.to_string();
    for _ in 0..rurge_policy::registry::MAX_DEPTH {
        let spec = rt.config.spec(&current)?;
        let Some(under) = spec.common.underlying_proxy.as_deref() else {
            return Some(spec);
        };
        let below = rt.policies.resolve(&PolicyRef::Named(under.to_string()));
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

/// The bypass both anti-loop arms of `dial_internal` take: note `why` on the
/// session, connect with `fallback`, and finish the handle explicitly when
/// even that fails — it would otherwise be dropped without a finished record.
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

impl Engine {
    /// Dials a DNS upstream connection through the pipeline (Internal session).
    /// Never lets the routing break DNS: a REJECT, and a proxy that would have
    /// to be resolved first, each warn and connect directly instead.
    pub async fn dial_internal(
        &self,
        session: SessionInfo,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedStream> {
        let rt = self.runtime();
        let handle = self.new_handle(session);
        let policy = match self.choose_policy(&rt, &handle).await {
            Chosen::Policy(p) => p,
            // An IP-literal DNS session never needs resolution; a DnsFailed here
            // would only come from a misconfigured rule → direct.
            Chosen::DnsFailed => PolicyRef::Builtin(Builtin::Direct),
        };
        let resolution = rt.policies.resolve(&policy);
        handle.set_policy_chain(resolution.chain.clone());
        let target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
        };
        // Reaching a proxy that is configured by host name would need the very
        // lookup this session is carrying (M3 design §7.4), so bypass it before
        // dialling. A proxy configured by IP literal has no such loop.
        if resolution.terminal == TerminalKind::Proxy
            && let Some(terminal) = resolution.chain.last()
            && let Some(spec) = socket_opener(&rt, terminal)
            && matches!(spec.server, Some(HostName::Domain(_)))
        {
            tracing::warn!(
                policy = %spec.name,
                "DNS session routed to a proxy configured by host name; connecting directly to avoid a resolution loop"
            );
            return bypass_to_direct(
                handle,
                fallback,
                &target,
                &opts,
                "dns-follow: proxy configured by host name bypassed to avoid a resolution loop",
            )
            .await;
        }
        match resolution.outbound.connect_tcp(&target, &opts).await {
            Ok(stream) => Ok(crate::dns_pipeline::wrap_internal(stream, handle)),
            Err(OutboundError::Reject(_)) | Err(OutboundError::Unsupported(_)) => {
                tracing::warn!(
                    dst = %target.host,
                    "DNS session routed to a reject/unsupported policy; connecting directly to keep DNS working"
                );
                bypass_to_direct(
                    handle,
                    fallback,
                    &target,
                    &opts,
                    "dns-follow: reject bypassed to keep DNS working",
                )
                .await
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
}

impl Engine {
    /// Stop accepting new connections (in-flight sessions keep running).
    pub fn stop_accepting(&self) {
        self.accept.cancel();
    }
    /// Force every in-flight relay to end now.
    pub fn cancel_sessions(&self) {
        self.sessions_root.cancel();
    }
    pub fn tracker(&self) -> &TaskTracker {
        &self.tracker
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

fn log_session(h: &SessionHandle, outcome: &SessionOutcome) {
    let s = h.session();
    let (up, down) = h.bytes();
    let dst = format!("{}:{}", s.dst_host, s.dst_port);
    let rule = h.rule().unwrap_or_default();
    let policy = h.policy_chain().join(" > ");
    let elapsed_ms = h.elapsed().as_millis() as u64;
    match outcome {
        SessionOutcome::Completed => tracing::debug!(
            session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
            up, down, elapsed_ms, "session completed"
        ),
        SessionOutcome::Rejected(kind) => tracing::info!(
            session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
            elapsed_ms, error = ?h.error(), "session rejected by {}", kind.name()
        ),
        SessionOutcome::Failed(error) => tracing::info!(
            session = h.id(), listener = ?s.listener, src = %s.src, dst = %dst, rule = %rule, policy = %policy,
            up, down, elapsed_ms, error = %error, "session failed"
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
    // both, or it hides the failure behind the note (fix round 1, F3). The
    // `DialError` below keeps the plain failure message.
    if let Some(note) = handle.error() {
        handle.set_error(format!("{note}; {message}"));
    }
    handle.finish(SessionOutcome::Failed(message.clone()));
    let rule = handle.rule();
    Err(DialError::Failed {
        kind,
        message,
        rule,
        handle,
    })
}

fn reject(handle: Arc<SessionHandle>, kind: rurge_proto::RejectKind) -> Result<Dialed, DialError> {
    handle.finish(SessionOutcome::Rejected(kind));
    let rule = handle.rule();
    Err(DialError::Reject { kind, rule, handle })
}

/// Looks up the raw text of the rule at `index` (M2a `CompiledRule.index` is
/// the rule's position in `Config.rules`, and `RuleEngine::rules()` returns
/// them in that ascending order), so a binary search finds it.
fn rule_raw(rules: &rurge_rules::RuleEngine, index: usize) -> Option<String> {
    let all = rules.rules();
    all.binary_search_by_key(&index, |r| r.index)
        .ok()
        .map(|pos| all[pos].raw.clone())
}

impl Dialer for Engine {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        Box::pin(async move {
            let rt = self.runtime();
            let handle = self.new_handle(session);
            let policy = match self.choose_policy(&rt, &handle).await {
                Chosen::Policy(p) => p,
                Chosen::DnsFailed => return fail(handle, FailKind::Dns, "dns lookup failed"),
            };
            let resolution = rt.policies.resolve(&policy);
            handle.set_policy_chain(resolution.chain.clone());
            if let Some(note) = &resolution.note {
                // A protocol rurge cannot speak yet, a group cycle, a group
                // without members: say why in the session log (§7.2; phase 2
                // M3 design 5.6).
                handle.set_error(note.to_string());
            }
            let mut target =
                Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
            let opts = ConnectOpts {
                timeout: CONNECT_TIMEOUT,
            };
            // FR-DNS-07: a proxy normally gets the name and resolves it itself;
            // with `use-local-host-item-for-proxy`, an address pinned in [Host]
            // goes to the proxy instead. Alias / server items leave the name alone.
            let pinned_ip = match &target.host {
                HostName::Domain(name)
                    if resolution.terminal == rurge_policy::TerminalKind::Proxy
                        && rt.config.general.use_local_host_item_for_proxy =>
                {
                    rt.stack
                        .resolver
                        .host_lookup(name)
                        .and_then(|hit| match hit.action {
                            rurge_dns::hosts::HostAction::Ips(ips) => ips.first().copied(),
                            _ => None,
                        })
                }
                _ => None,
            };
            let pinned = pinned_ip.is_some();
            if let Some(ip) = pinned_ip {
                target = Target::new(HostName::Ip(ip), target.port);
            }
            // A plain request of the HTTP listener can go to an HTTP proxy in
            // absolute form instead of through a tunnel (M1 design 6.5). The
            // inbound writes the request line, so the target is checked here
            // first, and the headers are rendered once for this connection
            // (the two obligations `HttpForward` puts on its caller).
            let plain_http =
                handle.session().listener == ListenerKind::Http && handle.session().url.is_some();
            // a pinned address only reaches the proxy through a tunnel: in
            // absolute form the request URI carries the name
            let forward = if plain_http && !pinned {
                resolution.outbound.http_forward()
            } else {
                None
            };
            let connected = match forward {
                Some(proxy) if rurge_proto::http::valid_target(&target) => proxy
                    .connect(&opts)
                    .await
                    .map(|stream| (stream, Some(proxy.request_headers()))),
                Some(_) => Err(OutboundError::Proxy(
                    "the target host name is not valid for an HTTP proxy request".to_string(),
                )),
                None => resolution
                    .outbound
                    .connect_tcp(&target, &opts)
                    .await
                    .map(|stream| (stream, None)),
            };
            match connected {
                Ok((stream, forward)) => Ok(Dialed {
                    stream,
                    handle,
                    forward,
                }),
                Err(OutboundError::Reject(kind)) => {
                    let effective = if kind.escalates() {
                        let host = handle.session().dst_host.to_string();
                        let now = Instant::now();
                        self.escalation.record(&host, now);
                        if self.escalation.should_drop(&host, now) {
                            rurge_proto::RejectKind::Drop
                        } else {
                            kind
                        }
                    } else {
                        kind
                    };
                    reject(handle, effective)
                }
                Err(OutboundError::Unsupported(_)) => {
                    reject(handle, rurge_proto::RejectKind::Reject)
                }
                Err(OutboundError::Dns(m)) => fail(handle, FailKind::Dns, m),
                Err(OutboundError::Io(e)) => fail(handle, FailKind::Connect, e.to_string()),
                Err(OutboundError::Timeout) => fail(handle, FailKind::Timeout, "connect timed out"),
                Err(
                    e @ (OutboundError::Proxy(_)
                    | OutboundError::Tls(_)
                    | OutboundError::Unavailable(_)),
                ) => fail(handle, FailKind::Connect, e.to_string()),
            }
        })
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
}
