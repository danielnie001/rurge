//! The engine (M3 design §7.2 – 7.3): implements `Dialer` for the listeners,
//! relays bytes, binds listeners from `[General]`, writes the session log.

use crate::observe::{RequestLog, TrafficStats};
use crate::runtime::Runtime;
use arc_swap::ArcSwap;
use rurge_config::Builtin;
use rurge_config::general::General;
use rurge_config::rule::PolicyRef;
use rurge_config::session::{ListenerKind, SessionInfo};
use rurge_inbound::{
    DialError, Dialed, Dialer, FailKind, HttpAuth, HttpListener, ListenerOpts, Running,
    SessionHandle, SessionOutcome, Socks5Listener,
};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::OutboundError;
use rurge_rules::{OutboundMode, Outcome};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
/// Upper bound on tracked hosts before a sweep evicts idle entries.
const ESCALATE_MAX_HOSTS: usize = 4096;

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

    /// Records one escalating reject for `host` at `now`.
    fn record(&self, host: &str, now: Instant) {
        let mut map = self.hosts.lock().expect("escalation");
        if map.len() >= ESCALATE_MAX_HOSTS && !map.contains_key(host) {
            map.retain(|_, times| {
                prune(times, now);
                !times.is_empty()
            });
        }
        let times = map.entry(host.to_string()).or_default();
        times.push_back(now);
        prune(times, now);
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
}

impl Engine {
    pub fn new(runtime: Runtime) -> Arc<Engine> {
        let observe = Arc::new(Observe {
            log: RequestLog::new(runtime.request_log_size),
            traffic: TrafficStats::new(),
        });
        let engine = Arc::new(Engine {
            runtime: ArcSwap::from_pointee(runtime),
            next_session: AtomicU64::new(0),
            sessions_root: CancellationToken::new(),
            accept: CancellationToken::new(),
            tracker: TaskTracker::new(),
            observe,
            escalation: Escalation::new(),
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
            tracing::info!(kind = ?spec.kind, addr = %running.local_addr, "listening");
            out.push((spec, running));
        }
        Ok(out)
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
                // Traffic first: a reader that sees the finished record in
                // `log` next must already find its bytes in `traffic.totals()`.
                o.traffic.record(h);
                o.log.record_finished(h, outcome);
            }
        });
        self.observe.log.mark_active(&handle);
        handle
    }
}

impl Engine {
    /// Dials a DNS upstream connection through the pipeline (Internal session).
    /// Never lets a REJECT break DNS: on reject it warns and connects directly.
    pub async fn dial_internal(
        &self,
        session: SessionInfo,
        fallback: &Arc<dyn Connector>,
    ) -> io::Result<BoxedStream> {
        let rt = self.runtime();
        let handle = self.new_handle(session);
        let policy = match &rt.outbound_mode {
            OutboundMode::Direct => PolicyRef::Builtin(Builtin::Direct),
            OutboundMode::Proxy(p) => p.clone(),
            OutboundMode::Rule => {
                let decision = rt
                    .rules
                    .evaluate(
                        handle.session(),
                        OutboundMode::Rule,
                        rt.stack.resolver.as_ref(),
                    )
                    .await;
                if let Some(i) = decision.matched {
                    handle.set_rule(
                        rt.rules
                            .rules()
                            .iter()
                            .find(|r| r.index == i)
                            .map(|r| r.raw.clone()),
                    );
                }
                match decision.outcome {
                    Outcome::Policy(p) => p,
                    // An IP-literal DNS session never needs resolution; a DnsFailed
                    // here would only come from a misconfigured rule → direct.
                    Outcome::DnsFailed => PolicyRef::Builtin(Builtin::Direct),
                }
            }
        };
        let resolution = rt.policies.resolve(&policy);
        handle.set_policy_chain(resolution.chain.clone());
        let target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
        let opts = ConnectOpts {
            timeout: CONNECT_TIMEOUT,
            prefer_v6: rt.config.general.ipv6,
        };
        match resolution.outbound.connect_tcp(&target, &opts).await {
            Ok(stream) => Ok(crate::dns_pipeline::wrap_internal(stream, handle)),
            Err(OutboundError::Reject(_)) | Err(OutboundError::Unsupported(_)) => {
                tracing::warn!(
                    dst = %target.host,
                    "DNS session routed to a reject/unsupported policy; connecting directly to keep DNS working"
                );
                handle.set_error("dns-follow: reject bypassed to keep DNS working");
                // Finish explicitly on failure, like every other arm: the
                // handle would otherwise be dropped without a finished record.
                let stream = match fallback.connect(&target, &opts).await {
                    Ok(stream) => stream,
                    Err(e) => {
                        handle.finish(SessionOutcome::Failed(e.to_string()));
                        return Err(e);
                    }
                };
                Ok(crate::dns_pipeline::wrap_internal(stream, handle))
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
                        let active = engine.observe.log.active_bytes();
                        engine.observe.traffic.sample(active);
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

impl Dialer for Engine {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        Box::pin(async move {
            let rt = self.runtime();
            let handle = self.new_handle(session);
            let policy = match &rt.outbound_mode {
                OutboundMode::Direct => PolicyRef::Builtin(Builtin::Direct),
                OutboundMode::Proxy(p) => p.clone(),
                OutboundMode::Rule => {
                    let decision = rt
                        .rules
                        .evaluate(
                            handle.session(),
                            OutboundMode::Rule,
                            rt.stack.resolver.as_ref(),
                        )
                        .await;
                    if let Some(i) = decision.matched {
                        handle.set_rule(
                            rt.rules
                                .rules()
                                .iter()
                                .find(|r| r.index == i)
                                .map(|r| r.raw.clone()),
                        );
                    }
                    match decision.outcome {
                        Outcome::Policy(p) => p,
                        Outcome::DnsFailed => {
                            return fail(handle, FailKind::Dns, "dns lookup failed");
                        }
                    }
                }
            };
            let resolution = rt.policies.resolve(&policy);
            handle.set_policy_chain(resolution.chain.clone());
            if let Some(kind) = &resolution.unsupported {
                // The policy is sound but rurge cannot speak it yet, so the
                // outbound below is REJECT; say so in the session log (§7.2).
                handle.set_error(format!("policy protocol not implemented: {kind}"));
            }
            let target = Target::new(handle.session().dst_host.clone(), handle.session().dst_port);
            let opts = ConnectOpts {
                timeout: CONNECT_TIMEOUT,
                prefer_v6: rt.config.general.ipv6,
            };
            match resolution.outbound.connect_tcp(&target, &opts).await {
                Ok(stream) => Ok(Dialed { stream, handle }),
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
}
