//! Sessions answered "connected" before anything is read (phase 4 M1 design 4,
//! 7): which sessions those are (`sniffable_session`), what happens to one of
//! them (`serve_sniffed`), and how the sniffs are counted. A sniffable session
//! is answered at once by whoever accepted it, comes here with its stream, and
//! is sniffed and given the SNI and the protocol it showed: one judged HTTP —
//! or one the settings force into the engine — is served by `serve_http` as the
//! HTTP engine's own requests, and every other is dialed and relayed as any
//! other session is, with the bytes that were read forwarded whole.

use crate::FAKE_ADDRESS_NOT_KNOWN;
use crate::control::Mode;
use crate::engine::{Engine, HANDSHAKE_TIMEOUT, policy_known};
use crate::fake_address::Turned;
use crate::mitm::Hint;
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_dns::fakeip::{Lease, in_space};
use rurge_http::mitm::{HandshakeFailure, Mitm, OriginTls, Watch, source_of};
use rurge_http::{ClientHello, Prefixed, Sniffed};
use rurge_inbound::{
    DialError, Dialer, ProcessSlot, ServeOpts, SessionHandle, SessionOutcome, hold_dropped,
};
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

/// How the sniffs of an engine ended (design 7.5), as `GET /v1/rurge/http`
/// reads them once they are a `SniffedCounts`.
#[derive(Default)]
pub(crate) struct SniffCounts {
    tls: AtomicU64,
    http: AtomicU64,
    other: AtomicU64,
    timeout: AtomicU64,
    /// `Closed` and `Failed` together: a client that closed its side and one
    /// that reset the connection both leave nothing to dial for (plan P4).
    closed: AtomicU64,
}

impl SniffCounts {
    /// Counts one sniff that ended, however it ended.
    pub(crate) fn count(&self, sniffed: &Sniffed) {
        let counter = match sniffed {
            Sniffed::Tls(_) => &self.tls,
            Sniffed::Http => &self.http,
            Sniffed::Other => &self.other,
            Sniffed::Timeout => &self.timeout,
            Sniffed::Closed | Sniffed::Failed => &self.closed,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> SniffedCounts {
        SniffedCounts {
            tls: self.tls.load(Ordering::Relaxed),
            http: self.http.load(Ordering::Relaxed),
            other: self.other.load(Ordering::Relaxed),
            timeout: self.timeout.load(Ordering::Relaxed),
            closed: self.closed.load(Ordering::Relaxed),
        }
    }
}

/// How the sniffs of an engine ended (phase 4 M1 design 7.5): what
/// `GET /v1/rurge/http` reads (the phase 4 / M1a plan's P16).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SniffedCounts {
    pub tls: u64,
    pub http: u64,
    pub other: u64,
    pub timeout: u64,
    /// Closed or failed: the API does not tell the two apart.
    pub closed: u64,
}

/// The HTTP engine's counters (phase 4 M1 design 11), as `GET /v1/rurge/http`
/// reads them: the sniffs of M1a, the requests that reached the rules since
/// M1b, and what the decrypted connections did (`phase 4 M2 design 8.5`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HttpCounts {
    pub sniffed: SniffedCounts,
    /// The requests that reached the rules, refused ones included.
    pub requests: u64,
    /// The MITM counts of the engine (`phase 4 M2 design 12.3`).
    pub mitm: crate::mitm::MitmCounts,
}

/// The parts of one client connection that `serve_http` and `serve_mitm` set
/// up the same way, before its requests are served: the connection's token and
/// its carrier handle, the process behind it, the lease a fake address was
/// turned back with, and the session itself, whose name and SNI are what its
/// requests are routed by.
struct Carried {
    engine: Arc<Engine>,
    session: SessionInfo,
    token: CancellationToken,
    process: ProcessSlot,
    lease: Option<Lease>,
    carrier: Arc<SessionHandle>,
    opts: ServeOpts,
}

/// What judging certificate pinning needs of a decrypted connection (`phase 4
/// M2 design 8.3`): the name the leaf was issued for, which the hint names,
/// and the count of what the client sent and whether it closed — both counted
/// outside the TLS stream, which the HTTP engine has by then.
struct Pinning {
    name: HostName,
    watch: Arc<Watch>,
}

impl Engine {
    /// Whether `session` is to be answered "connected" first and dialed only
    /// after its first bytes were read (design 4.1).
    ///
    /// The lists and the keywords come before everything else, the port
    /// decides without them, and the probe of pre-matching is asked last: it
    /// is the only condition that looks at the rules, and it is only asked
    /// for the sessions whose refusal would otherwise come after the answer.
    pub(crate) fn sniffable_session(&self, session: &SessionInfo) -> bool {
        // design 4.1 (1): a TCP session of a client's entry
        if session.transport != Transport::Tcp
            || !matches!(
                session.listener,
                ListenerKind::Http | ListenerKind::Socks5 | ListenerKind::Tun
            )
        {
            return false;
        }
        // design 4.1 (2): a paused engine refuses its sessions before it
        // answers them, and the client gets today's answer and error page
        if self.is_suspended() {
            return false;
        }
        // design 4.1: at a proxy port, an address of rurge's space is the name
        // behind it before the lists and the probe look at it. The probe only
        // asks the pool; the dial is what leases the address (`turn_back`).
        // An address that no name stands behind is not sniffed: its dial fails
        // as it does today (the plan's P5).
        let at_a_proxy_port = matches!(session.listener, ListenerKind::Http | ListenerKind::Socks5);
        let Some(probed) = self.named_at_a_proxy_port(session) else {
            return false;
        };
        // the settings are the ones of the generation in use now, as the rules
        // will be when the session is dialed
        let runtime = self.runtime();
        let general = &runtime.config.general;
        let (host, port) = (&probed.dst_host, probed.dst_port);
        // design 4.1 (4), which the general design puts before everything else
        if general.always_raw_tcp_hosts.matches(host, port) == Some(true) {
            return false;
        }
        // design 4.1 (4) and 4.2: an IP literal is not a hostname, so no
        // keyword is looked for in one; the name is lowercase already
        if let HostName::Domain(name) = host
            && general
                .always_raw_tcp_keywords
                .iter()
                .any(|k| !k.is_empty() && name.contains(&k.to_ascii_lowercase()))
        {
            return false;
        }
        // design 4.1 (5): the two ports of the design, and whatever
        // `force-http-engine-hosts` adds (whose default port is 80) — and a
        // port `[MITM]` widens sniffing to (`phase 4 M2 design 7.6`)
        if port != 80
            && port != 443
            && general.force_http_engine_hosts.matches(host, port) != Some(true)
            && !self.widened_by_mitm(session, runtime.mitm.as_deref())
        {
            return false;
        }
        // design 4.3: the proxy ports have no SYN for pre-matching to refuse
        // at, so the session is probed before it is answered — and only when
        // the dial will ask the rules, and only for a session the probe does
        // not count (plan P6)
        if at_a_proxy_port && self.reaches_the_rules() && runtime.rules.pre_refuses(&probed) {
            return false;
        }
        true
    }

    /// The session with the name behind a fake address it names at a proxy
    /// port: `None` when the address is rurge's and no name stands behind it.
    /// The pool is only asked, not leased: the dial (`turn_back`) is what
    /// leases the address. Both the judgement of `sniffable_session` and the
    /// settings' forced hosts of `serve_sniffed` read the same name through
    /// this, so the two cannot disagree about one connection (design 6.4).
    fn named_at_a_proxy_port<'s>(&self, session: &'s SessionInfo) -> Option<Cow<'s, SessionInfo>> {
        let at_a_proxy_port = matches!(session.listener, ListenerKind::Http | ListenerKind::Socks5);
        match &session.dst_host {
            HostName::Ip(ip) if at_a_proxy_port && in_space(*ip) => {
                match self.fake_pool.load_full() {
                    // no TUN runs: the address is an address like any other
                    None => Some(Cow::Borrowed(session)),
                    Some(pool) => pool.name_of(ip.to_canonical()).map(|name| {
                        Cow::Owned(SessionInfo {
                            dst_host: HostName::Domain(name),
                            ..session.clone()
                        })
                    }),
                }
            }
            _ => Some(Cow::Borrowed(session)),
        }
    }

    /// Whether the settings force `session` into the engine even though the
    /// sniff did not say HTTP (design 6.4): `force-http-engine-hosts` is
    /// matched against the name behind a fake address, if that is what came
    /// in.
    fn forces_http_engine(&self, session: &SessionInfo) -> bool {
        let Some(named) = self.named_at_a_proxy_port(session) else {
            return false;
        };
        self.runtime()
            .config
            .general
            .force_http_engine_hosts
            .matches(&named.dst_host, named.dst_port)
            == Some(true)
    }

    /// The name the session is about (`phase 4 M2 design 7.6`): the name behind
    /// an address of rurge's space, at whatever entry, or else the target as it
    /// came in. `None` when the address is rurge's and no name stands behind
    /// it — such a session is dialed as it is, and the dial fails as
    /// `turn_back` says. Only asked, never leased: the dial leases.
    fn named_target(&self, session: &SessionInfo) -> Option<HostName> {
        match &session.dst_host {
            HostName::Ip(ip) if in_space(*ip) => match self.fake_pool.load_full() {
                // no TUN runs: the address is an address like any other
                None => Some(session.dst_host.clone()),
                Some(pool) => pool.name_of(ip.to_canonical()).map(HostName::Domain),
            },
            host => Some(host.clone()),
        }
    }

    /// Whether `[MITM]` widens sniffing to this session (`phase 4 M2 design
    /// 7.6`, `M2-D9`): the switch is on, this generation has a usable CA, and
    /// the source and the list allow the session. The list allows it when the
    /// port is one a non-negated entry writes down, or when the target is
    /// listed.
    fn widened_by_mitm(&self, session: &SessionInfo, mitm: Option<&Mitm>) -> bool {
        if !self.mitm_on() {
            return false;
        }
        let Some(mitm) = mitm else {
            return false;
        };
        let Some(target) = self.named_target(session) else {
            return false;
        };
        mitm.widens(
            source_of(session.listener, session.src),
            &target,
            session.dst_port,
        )
    }

    /// What a sniffed TLS session is decrypted with (`phase 4 M2 design 7.1`,
    /// in the order of the phase 4 / M2a plan's P18): the switch, the
    /// session's name, and this generation's CA, list and ALPN. `None` passes
    /// the session through as `M1` does.
    ///
    /// The two lines of DEBUG say why a session the list covers was not
    /// decrypted: a generation without a usable CA, and a client that offers
    /// no protocol rurge decrypts. Every other session passed through is the
    /// usual state of TLS.
    fn decrypting(
        &self,
        session: &SessionInfo,
        hello: &ClientHello,
    ) -> Option<(Arc<Mitm>, HostName)> {
        if !self.mitm_on() {
            return None;
        }
        let target = self.named_target(session)?;
        let sni = hello
            .sni
            .as_deref()
            .map(|name| HostName::parse(name).canonical());
        // the name the truth is told about, and the one a leaf is issued for:
        // the SNI, or the target when there is none (`phase 4 M2 design 7.3`)
        let name = sni.clone().unwrap_or_else(|| target.clone());
        let runtime = self.runtime();
        let Some(mitm) = runtime.mitm.clone() else {
            let listed = match &sni {
                Some(sni) => runtime
                    .config
                    .mitm
                    .effective
                    .matches_sni(sni, session.dst_port),
                None => runtime
                    .config
                    .mitm
                    .effective
                    .matches(&target, session.dst_port),
            };
            if listed == Some(true) {
                tracing::debug!(
                    "{name} is listed for MITM but this generation has no usable CA; passing it through"
                );
            }
            return None;
        };
        match mitm.decides(
            source_of(session.listener, session.src),
            sni.as_ref(),
            &target,
            session.dst_port,
            &hello.alpn,
        ) {
            rurge_http::mitm::Decision::Decrypt => Some((mitm, name)),
            rurge_http::mitm::Decision::AlpnMismatch => {
                let alpn: Vec<String> = hello
                    .alpn
                    .iter()
                    .map(|p| String::from_utf8_lossy(p).into_owned())
                    .collect();
                tracing::debug!(
                    "{name} is listed for MITM but the client offers no protocol rurge decrypts ({}); passing it through",
                    alpn.join(", ")
                );
                None
            }
            rurge_http::mitm::Decision::Pass => None,
        }
    }

    /// Serves a connection the engine judges HTTP (`P9`, `P10`): a sniffed one
    /// whose first bytes are a request, or one `force-http-engine-hosts` keeps
    /// in the engine whatever it sent. Every request is routed and dialed on
    /// its own (`ConnUpstreams`); the connection itself gets a record only
    /// when none of them reached the rules (design 10.2).
    async fn serve_http(
        &self,
        mut session: SessionInfo,
        client: Prefixed<BoxedStream>,
        opts: ServeOpts,
    ) {
        let Some(engine) = self.self_arc() else {
            return;
        };
        let turned = self.turn_back(&mut session);
        let process = self.look_up_owner(&session);
        // The connection's token: each request's is its child, and an upgraded
        // relay's too. The carrier handle is not registered: it gets a record
        // only if no request reached the rules (design 10.2, P10).
        let token = self.session_token();
        let carrier = SessionHandle::new_with_slot(
            self.next_id(),
            session.clone(),
            token.clone(),
            process.clone(),
        );
        let lease = match turned {
            Turned::Unchanged => None,
            Turned::Name(lease) => Some(lease),
            Turned::Unknown => {
                self.attach_observers(&carrier);
                carrier.finish(SessionOutcome::Failed(FAKE_ADDRESS_NOT_KNOWN.to_string()));
                return;
            }
        };
        // a client's TCP session to a host whose QUIC the engine blocked is
        // that client falling back (phase 3 M5 design 6.3): once for the
        // connection, as `dial` does for a session of its own
        self.quic_adaptive()
            .on_tcp(&session.dst_host.to_string(), Instant::now());
        self.serve_requests(
            Carried {
                engine,
                session,
                token,
                process,
                lease,
                carrier,
                opts,
            },
            Box::new(client),
            None,
            None,
        )
        .await;
    }

    /// Serves a sniffed TLS session rurge decrypts (`phase 4 M2 design 7.1`,
    /// `8.2`): the handshake with the client is raced with the stop, and only
    /// once it is done is there a connection to record anything about.
    ///
    /// A handshake a stop interrupts leaves nothing behind: no record, no
    /// count, no line — the client's bytes are dropped with the task, as a
    /// sniff a stop interrupts is.
    async fn serve_mitm(
        &self,
        mut session: SessionInfo,
        client: Prefixed<BoxedStream>,
        mitm: Arc<Mitm>,
        name: HostName,
        opts: ServeOpts,
    ) {
        let Some(engine) = self.self_arc() else {
            return;
        };
        let turned = self.turn_back(&mut session);
        let process = self.look_up_owner(&session);
        // the connection's token, as `serve_http` makes it: the requests'
        // tokens are its children
        let token = self.session_token();
        let carrier = SessionHandle::new_with_slot(
            self.next_id(),
            session.clone(),
            token.clone(),
            process.clone(),
        );
        let lease = match turned {
            Turned::Unchanged => None,
            Turned::Name(lease) => Some(lease),
            Turned::Unknown => {
                self.attach_observers(&carrier);
                carrier.finish(SessionOutcome::Failed(FAKE_ADDRESS_NOT_KNOWN.to_string()));
                return;
            }
        };
        // the client's TCP session to this host, as `serve_http` tells it
        self.quic_adaptive()
            .on_tcp(&session.dst_host.to_string(), Instant::now());
        // Nothing is held yet: the handshake holds no handle and no record, so
        // a stop ends it at once (`phase 4 M2 design 8.2`).
        let accepted = tokio::select! {
            biased;
            _ = token.cancelled() => return,
            _ = stopped(opts.stop.as_ref()) => return,
            accepted = mitm.accept(client, &name, HANDSHAKE_TIMEOUT) => accepted,
        };
        let accepted = match accepted {
            Ok(accepted) => accepted,
            Err(failure) => {
                // the connection's own record, as a connection whose requests
                // never reached the rules gets one (`phase 4 M2 design 8.2`,
                // the plan's P21) — with the line and the count the failure
                // has. Only the WARN is limited: one per kind and host and
                // hour, and what it holds back is written at DEBUG (`8.4`).
                // The count and the record are there every time, and the line
                // of a failure that is not about trust is not limited at all.
                let reason = match failure {
                    HandshakeFailure::Untrusted { alert } => {
                        self.mitm_counts.untrusted();
                        self.log_untrusted(session.listener, &name, session.dst_port, alert);
                        format!(
                            "the client does not trust the certificate rurge issued (alert {alert})"
                        )
                    }
                    HandshakeFailure::Failed { reason } => {
                        self.mitm_counts.failed();
                        self.log_handshake_failed(
                            session.listener,
                            &name,
                            session.dst_port,
                            &reason,
                        );
                        format!("TLS handshake with the client failed: {reason}")
                    }
                };
                self.attach_observers(&carrier);
                carrier.finish(SessionOutcome::Failed(reason));
                return;
            }
        };
        self.mitm_counts.decrypted();
        self.serve_requests(
            Carried {
                engine,
                session,
                token,
                process,
                lease,
                carrier,
                opts,
            },
            accepted.stream,
            Some(accepted.origin),
            Some(Pinning {
                name,
                watch: accepted.watch,
            }),
        )
        .await;
    }

    /// The requests of one client connection (`phase 4 M1 design 10.2`), which
    /// `serve_http` and `serve_mitm` set up alike: every request is routed and
    /// dialed on its own (`ConnUpstreams`), and the connection itself gets a
    /// record only when none of them reached the rules. A decrypted connection
    /// (`origin` is `Some`, `phase 4 M2 design 9`) is the same in everything
    /// but the TLS to the origin and the URL each request is named by, and it
    /// is `pinning` that tells the one ending of it that is a hint about the
    /// host (`phase 4 M2 design 8.3`).
    async fn serve_requests(
        &self,
        carried: Carried,
        client: BoxedStream,
        origin: Option<OriginTls>,
        pinning: Option<Pinning>,
    ) {
        let Carried {
            engine,
            session,
            token,
            process,
            lease,
            carrier,
            opts,
        } = carried;
        // what a line about the connection says of it, taken before the
        // connection takes the session (`phase 4 M2 design 15`)
        let (listener, port) = (session.listener, session.dst_port);
        let runtime = self.runtime();
        let conn = rurge_http::Conn {
            session,
            entry: rurge_http::Entry::Sniffed,
            origin,
            idle: runtime.idle_timeout,
            head_wait: runtime.idle_timeout,
            drop_hold: opts.drop_hold,
            // the engine's word that the connection has to go at once; not a
            // child of the connection's token (design 8.10, 8.11)
            abort: CancellationToken::new(),
            token: token.clone(),
            tracker: self.tracker().clone(),
        };
        let upstreams = crate::http::ConnUpstreams::new(
            engine,
            token.clone(),
            process,
            lease,
            opts.stop.clone(),
            false,
        );
        let mut serving = std::pin::pin!(rurge_http::serve_connection(
            client,
            rurge_http::Requests::new(conn, upstreams),
        ));
        // A stop cancels the connection's token and then waits for the same
        // future, not for another one: it is what ends the requests (design
        // 8.11). A connection that ends by itself (the client left, a request
        // was killed, hyper gave an upgraded connection up) does not cancel
        // the token: an upgraded relay's is its child and goes on.
        let served = tokio::select! {
            biased;
            _ = stopped(opts.stop.as_ref()) => {
                token.cancel();
                serving.await
            }
            served = &mut serving => served,
        };
        if served.routed == 0 {
            // nothing of the connection reached the rules: it gets a record of
            // its own. A decrypted connection whose client finished the
            // handshake and closed without sending a byte is what certificate
            // pinning looks like (`phase 4 M2 design 8.3`, the plan's P21);
            // anything else is hyper's reason for ending the connection.
            let pinned = pinning.filter(|pinning| {
                pinning.watch.read() == 0
                    && pinning.watch.closed_by_client()
                    && !token.is_cancelled()
                    && opts.stop.as_ref().is_none_or(|stop| !stop.is_cancelled())
            });
            let reason = match pinned {
                Some(pinning) => {
                    self.mitm_counts.no_request();
                    self.log_pinning(listener, &pinning.name, port);
                    "no request after the TLS handshake (likely certificate pinning)".to_string()
                }
                None => served.error.unwrap_or_else(|| "no request".to_string()),
            };
            self.attach_observers(&carrier);
            carrier.finish(SessionOutcome::Failed(reason));
        }
    }

    /// The line about a client that does not trust the certificate rurge
    /// issued (`phase 4 M2 design 8.2`, the plan's P21). A session through a
    /// listener names the host and its port; one through the virtual
    /// interface names neither, and the host follows in a line of DEBUG
    /// (`phase 4 M2 design 15`, stage 3 `M3-D26`). A second hint about the same
    /// host within the hour is the DEBUG line alone (`8.4`).
    fn log_untrusted(&self, listener: ListenerKind, name: &HostName, port: u16, alert: &str) {
        let named = format!(
            "the client does not trust the certificate rurge issued for {name}: trust rurge's MITM CA on the client, or exclude the host with -{name} in [MITM] hostname"
        );
        let quiet =
            !self
                .shared()
                .mitm
                .warn_now(Hint::Untrusted, &name.to_string(), Instant::now());
        if matches!(listener, ListenerKind::Tun) {
            if !quiet {
                tracing::warn!(
                    "the client does not trust the certificate rurge issued for a host reached through the virtual interface (the name is in the DEBUG log and the request record): trust rurge's MITM CA on the client, or exclude the host in [MITM] hostname"
                );
            }
            tracing::debug!("{}", named);
        } else if quiet {
            tracing::debug!(port = %port, alert = %alert, "{}", named);
        } else {
            tracing::warn!(port = %port, alert = %alert, "{}", named);
        }
    }

    /// The hint that a host is likely protected by certificate pinning
    /// (`phase 4 M2 design 8.3`, the plan's P21): the client finished the
    /// handshake and closed without a request. A session through the virtual
    /// interface leaves the host and the port out, and the host follows in a
    /// line of DEBUG (`phase 4 M2 design 15`); a second hint about the same
    /// host within the hour is DEBUG alone (`8.4`).
    fn log_pinning(&self, listener: ListenerKind, name: &HostName, port: u16) {
        let named = format!(
            "{name} is likely protected by certificate pinning: the client finished the TLS handshake and closed without a request; exclude it with -{name} in [MITM] hostname"
        );
        let quiet = !self
            .shared()
            .mitm
            .warn_now(Hint::Pinning, &name.to_string(), Instant::now());
        if matches!(listener, ListenerKind::Tun) {
            if !quiet {
                tracing::warn!(
                    "a host reached through the virtual interface is likely protected by certificate pinning (the name is in the DEBUG log and the request record): the client finished the TLS handshake and closed without a request"
                );
            }
            tracing::debug!("{}", named);
        } else if quiet {
            tracing::debug!(port = %port, "{}", named);
        } else {
            tracing::warn!(port = %port, "{}", named);
        }
    }

    /// The line about a handshake that failed for any reason but the client's
    /// trust (`phase 4 M2 design 8.2`, the plan's P21). It is not limited: the
    /// reasons differ from one another, and a repeat is a new fact. A session
    /// through the virtual interface leaves the host and the port out, and the
    /// host follows in a line of DEBUG (`phase 4 M2 design 15`).
    fn log_handshake_failed(
        &self,
        listener: ListenerKind,
        name: &HostName,
        port: u16,
        reason: &str,
    ) {
        let named = format!("the TLS handshake with the client failed for {name}: {reason}");
        if matches!(listener, ListenerKind::Tun) {
            tracing::info!(
                "the TLS handshake with the client failed for a host reached through the virtual interface: {reason}"
            );
            tracing::debug!("{}", named);
        } else {
            tracing::info!(port = %port, "{}", named);
        }
    }

    /// Serves a session its entry answered "connected" already, to its end
    /// (design 7.1): sniff it, say what it showed, and then either hand it to
    /// `serve_http` (judged HTTP, or forced in by `force-http-engine-hosts`)
    /// or dial it and relay it.
    ///
    /// The sniff is raced with the stop and with the engine's sessions root:
    /// nothing is held and no record made while it waits, so a stop ends it at
    /// once and lets the stream go (a reset on the TUN). The dial is not raced
    /// with anything: it holds the session's handle, and a dial dropped half
    /// way would leave its record open.
    pub(crate) async fn serve_sniffed(
        &self,
        session: SessionInfo,
        client: BoxedStream,
        opts: ServeOpts,
    ) {
        let mut client = Prefixed::new(client);
        let root = self.session_token();
        let sniffed = tokio::select! {
            biased;
            _ = root.cancelled() => return,
            _ = stopped(opts.stop.as_ref()) => return,
            sniffed = rurge_http::sniff(&mut client, self.sniff_wait()) => sniffed,
        };
        self.sniff_counts.count(&sniffed);
        // whether the settings keep the connection in the engine whatever it
        // showed, judged by the same name `sniffable_session` judged (design
        // 6.4)
        let forced = self.forces_http_engine(&session);
        let mut session = session;
        let mut into_engine = false;
        match sniffed {
            Sniffed::Tls(hello) => {
                // the ClientHello's SNI is lowercase already, or absent when it
                // was not ASCII
                session.sni = hello.sni.clone();
                session.protocol = Some(ProtocolKind::Https);
                // what a sniffed TLS session is decrypted with is judged on
                // the generation and the switch of this moment (`phase 4 M2
                // design 7.1`); a session that is not decrypted carries on as
                // `M1` passes it through
                if let Some((mitm, name)) = self.decrypting(&session, &hello) {
                    self.serve_mitm(session, client, mitm, name, opts).await;
                    return;
                }
            }
            Sniffed::Http => {
                session.protocol = Some(ProtocolKind::Http);
                into_engine = true;
            }
            // design 7.1: forwarded as it is, with no protocol on the record;
            // the settings may still keep it in the engine (design 6.4)
            Sniffed::Other | Sniffed::Closed => into_engine = forced,
            Sniffed::Timeout => {}
            // the client is gone: nothing to dial for, nothing to record
            Sniffed::Failed => return,
        }
        if into_engine {
            self.serve_http(session, client, opts).await;
            return;
        }
        let client: BoxedStream = Box::new(client);
        match self.dial(session).await {
            Ok(dialed) => {
                let token = dialed.handle.token().clone();
                let mut relay = self.relay(client, dialed.stream, dialed.handle);
                // The relay is raced with the stop and then waited for, not
                // dropped: it is what finishes the session's handle.
                tokio::select! {
                    biased;
                    _ = stopped(opts.stop.as_ref()) => {
                        token.cancel();
                        relay.await;
                    }
                    _ = &mut relay => {}
                }
            }
            Err(DialError::Reject {
                kind: RejectKind::Drop,
                ..
            }) => {
                // design 7.4: the answer went out already, so the connection
                // itself is held — until the engine's sessions end as well
                // (plan P10)
                tokio::select! {
                    biased;
                    _ = root.cancelled() => {}
                    _ = hold_dropped(client, &opts) => {}
                }
            }
            // any other rejection and a failed dial let the stream go
            Err(_) => {}
        }
    }

    /// Whether a session that begins now is routed by the rules, which is what
    /// `choose_policy_with` decides before it evaluates them. Worked out
    /// without a snapshot: this is an estimate, and the dial reads the pair
    /// again for real.
    fn reaches_the_rules(&self) -> bool {
        match self.mode() {
            Mode::Direct => false,
            Mode::Proxy => {
                !matches!(self.global_policy(), Some(name) if policy_known(&self.registry(), &name))
            }
            Mode::Rule => true,
        }
    }
}

/// `stop` as a future: never done when there is none (the listeners stop their
/// sessions with the engine's root, not with their own).
pub(crate) async fn stopped(stop: Option<&CancellationToken>) {
    match stop {
        Some(stop) => stop.cancelled().await,
        None => std::future::pending().await,
    }
}
