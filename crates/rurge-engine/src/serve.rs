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
use crate::engine::{Engine, policy_known};
use crate::fake_address::Turned;
use rurge_config::HostName;
use rurge_config::rule::ProtocolKind;
use rurge_config::session::{ListenerKind, SessionInfo, Transport};
use rurge_dns::fakeip::in_space;
use rurge_http::{Prefixed, Sniffed};
use rurge_inbound::{DialError, Dialer, ServeOpts, SessionHandle, SessionOutcome, hold_dropped};
use rurge_net::connector::BoxedStream;
use rurge_proto::RejectKind;
use std::borrow::Cow;
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
/// reads them: the sniffs of M1a, and the requests that reached the rules
/// since M1b.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HttpCounts {
    pub sniffed: SniffedCounts,
    /// The requests that reached the rules, refused ones included.
    pub requests: u64,
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
        // `force-http-engine-hosts` adds (whose default port is 80)
        if port != 80
            && port != 443
            && general.force_http_engine_hosts.matches(host, port) != Some(true)
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
        let runtime = self.runtime();
        let conn = rurge_http::Conn {
            session,
            entry: rurge_http::Entry::Sniffed,
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
            Box::new(client),
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
            // its own, and the reason hyper had for ending it
            self.attach_observers(&carrier);
            carrier.finish(SessionOutcome::Failed(
                served.error.unwrap_or_else(|| "no request".to_string()),
            ));
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
                session.sni = hello.sni;
                session.protocol = Some(ProtocolKind::Https);
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
