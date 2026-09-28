//! `SshOutbound` (phase 2 M4 design §5): one SSH session per policy, a
//! `direct-tcpip` channel per connection.

use crate::keys::decode_private_key;
use crate::pins::host_key_allowed;
use rurge_config::KeystoreItem;
use rurge_config::spec::{HostKeyPin, ShadowTlsOpts, SshSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::build::shadow_tls_client;
use rurge_proto::transport::Stack;
use rurge_proto::{BuildError, Outbound, OutboundError, RejectKind};
use russh::client::{self, Handle};
use russh::keys::ssh_key::Algorithm;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{Channel, ChannelOpenFailure, ChannelStream, Disconnect, Error, Preferred, cipher};
use rustls::RootCertStore;
use std::borrow::Cow;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Mutex, Notify};
use tokio::time::Instant;

/// A keepalive every 30 seconds while the session is up; three without an
/// answer end it (M4-D10).
const KEEPALIVE: Duration = Duration::from_secs(30);
const KEEPALIVE_MAX: usize = 3;
/// russh's `inactivity_timeout`: it bounds each write, so a connection the
/// server stops taking data from ends within five minutes. Everything the
/// server sends resets it, keepalive answers included, so on a live
/// session idle still means no open channel; a stalled login is bounded by
/// `LOGIN_LIMIT` instead.
const INACTIVITY: Duration = Duration::from_secs(300);
/// The key exchange and the login together take no longer, whatever the
/// dial's own budget. Before the login russh does nothing with a keepalive
/// that falls due, and while the server stays silent it does not re-arm the
/// timer either: its session task spins until data arrives. A login that
/// ends well within `KEEPALIVE` never gets there.
const LOGIN_LIMIT: Duration = Duration::from_secs(20);

/// The failures another attempt right away cannot fix: the credentials, the
/// pinned host keys and the algorithms stay what they are until the profile
/// or the server changes.
const AUTHENTICATION_FAILED: &str = "ssh: authentication failed";
const UNKNOWN_HOST_KEY: &str = "ssh: the server's host key is not one of server-fingerprint";
const NO_COMMON_ALGORITHM: &str =
    "ssh: the handshake failed (no algorithm in common with the server)";
/// Any other failure of the handshake: russh does not tell its stages apart.
const HANDSHAKE_FAILED: &str = "ssh: the handshake failed";

/// After one of those failures, dials fail at once for a minute; each
/// further one in a row doubles that, up to ten minutes. A server's
/// fail2ban (and OpenSSH's own penalties) count failed logins, and every
/// burst of connections would otherwise add one.
const FIRST_BACKOFF: Duration = Duration::from_secs(60);
const LONGEST_BACKOFF: Duration = Duration::from_secs(600);

/// The session's side of the conversation: the server's host key is checked
/// against `server-fingerprint`, and every channel the server opens toward
/// rurge is refused — rurge asks for no forwarding of any kind, as the
/// OpenSSH client refuses what it did not ask for.
struct Client {
    pins: Arc<[HostKeyPin]>,
}

impl client::Handler for Client {
    type Error = Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Error> {
        Ok(host_key_allowed(&self.pins, key))
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        _channel: Channel<client::Msg>,
        _connected_address: &str,
        _connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Error> {
        refuse(reply).await
    }

    async fn server_channel_open_forwarded_streamlocal(
        &mut self,
        _channel: Channel<client::Msg>,
        _socket_path: &str,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Error> {
        refuse(reply).await
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        _channel: Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Error> {
        refuse(reply).await
    }

    async fn server_channel_open_session(
        &mut self,
        _channel: Channel<client::Msg>,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Error> {
        refuse(reply).await
    }

    async fn server_channel_open_direct_tcpip(
        &mut self,
        _channel: Channel<client::Msg>,
        _host_to_connect: &str,
        _port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Error> {
        refuse(reply).await
    }

    async fn server_channel_open_direct_streamlocal(
        &mut self,
        _channel: Channel<client::Msg>,
        _socket_path: &str,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Error> {
        refuse(reply).await
    }

    async fn server_channel_open_x11(
        &mut self,
        _channel: Channel<client::Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: client::ChannelOpenHandle,
        _session: &mut client::Session,
    ) -> Result<(), Error> {
        refuse(reply).await
    }
}

async fn refuse(reply: client::ChannelOpenHandle) -> Result<(), Error> {
    reply
        .reject(ChannelOpenFailure::AdministrativelyProhibited)
        .await;
    Ok(())
}

/// russh's defaults, with the one cipher Surge's manual requires added
/// (`aes128-gcm@openssh.com`, "Algorithm Requirements") and the SHA-1 host
/// key signature (`ssh-rsa`) left out; the pinned host keys' algorithms
/// first (`host_key_algorithms`).
fn preferred(pins: &[HostKeyPin]) -> Preferred {
    Preferred {
        cipher: Cow::Owned(vec![
            cipher::CHACHA20_POLY1305,
            cipher::AES_256_GCM,
            cipher::AES_128_GCM,
            cipher::AES_256_CTR,
            cipher::AES_192_CTR,
            cipher::AES_128_CTR,
        ]),
        key: Cow::Owned(host_key_algorithms(pins)),
        ..Preferred::DEFAULT
    }
}

/// A server with several host keys presents the one for the first algorithm
/// on this list that it has a key for, so the algorithms of the pinned keys
/// come first, in the order the pins are written; then the rest of russh's
/// list, without `ssh-rsa`. An RSA pin (key type `ssh-rsa`) brings
/// rsa-sha2-512 and rsa-sha2-256 forward; a pinned algorithm that is not on
/// the list (`ssh-dss`, `sk-*`) adds nothing.
fn host_key_algorithms(pins: &[HostKeyPin]) -> Vec<Algorithm> {
    let pinned: Vec<Algorithm> = pins
        .iter()
        .filter_map(|pin| Algorithm::new(&pin.algorithm).ok())
        .collect();
    let mut offered: Vec<Algorithm> = Preferred::DEFAULT
        .key
        .iter()
        .filter(|algorithm| !matches!(algorithm, Algorithm::Rsa { hash: None }))
        .cloned()
        .collect();
    // a stable sort: russh's order among the algorithms of one pin, and
    // among those no pin names
    offered.sort_by_key(|algorithm| {
        pinned
            .iter()
            .position(|pin| {
                pin == algorithm
                    || (matches!(pin, Algorithm::Rsa { .. })
                        && matches!(algorithm, Algorithm::Rsa { .. }))
            })
            .unwrap_or(pinned.len())
    });
    offered
}

fn session_config(pins: &[HostKeyPin]) -> client::Config {
    client::Config {
        preferred: preferred(pins),
        inactivity_timeout: Some(INACTIVITY),
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: KEEPALIVE_MAX,
        ..Default::default()
    }
}

struct Session {
    handle: Handle<Client>,
    /// Channels being opened or handed out, and not yet dropped.
    open: Arc<AtomicUsize>,
    /// Woken when a channel opens or closes, and when the session goes.
    activity: Arc<Notify>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // the idle watch ends with its session, not up to `idle-timeout` later
        self.activity.notify_one();
    }
}

/// Counts a channel as open while it lives.
struct OpenChannel {
    open: Arc<AtomicUsize>,
    activity: Arc<Notify>,
}

impl OpenChannel {
    fn new(session: &Session) -> OpenChannel {
        session.open.fetch_add(1, Ordering::SeqCst);
        session.activity.notify_one();
        OpenChannel {
            open: session.open.clone(),
            activity: session.activity.clone(),
        }
    }
}

impl Drop for OpenChannel {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
        self.activity.notify_one();
    }
}

/// A channel as the engine sees it.
struct SshStream {
    channel: ChannelStream<client::Msg>,
    _open: OpenChannel,
}

impl AsyncRead for SshStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().channel).poll_read(cx, buf)
    }
}

impl AsyncWrite for SshStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().channel).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().channel).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().channel).poll_shutdown(cx)
    }
}

const SETTING_UP: u8 = 0;
const ABANDONED: u8 = 1;
const UP: u8 = 2;

/// Where the connection under a session stands while `establish` sets the
/// session up.
struct Setup {
    stage: AtomicU8,
    /// The pending read to wake when the dial goes away.
    reader: StdMutex<Option<Waker>>,
}

/// The connection handed to russh. russh's session task does not look at
/// its `Handle` while a key exchange runs, so a dial that gives up then
/// would leave it waiting on the server until the server hangs up — and
/// spinning once a keepalive falls due (see `LOGIN_LIMIT`). Reads fail once
/// the dial is gone, which ends the task; after the key exchange the
/// connection is passed through untouched.
struct Abandonable {
    inner: BoxedStream,
    setup: Arc<Setup>,
}

/// Held by `establish` while the key exchange runs: dropped, it abandons
/// the connection; `key_exchange_done` hands the connection over for good.
struct Armed(Option<Arc<Setup>>);

fn abandonable(inner: BoxedStream) -> (Abandonable, Armed) {
    let setup = Arc::new(Setup {
        stage: AtomicU8::new(SETTING_UP),
        reader: StdMutex::new(None),
    });
    (
        Abandonable {
            inner,
            setup: setup.clone(),
        },
        Armed(Some(setup)),
    )
}

impl Armed {
    fn key_exchange_done(mut self) {
        if let Some(setup) = self.0.take() {
            setup.stage.store(UP, Ordering::SeqCst);
            // nothing will need waking any more
            setup.reader.lock().unwrap().take();
        }
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        if let Some(setup) = self.0.take() {
            setup.stage.store(ABANDONED, Ordering::SeqCst);
            let reader = setup.reader.lock().unwrap().take();
            if let Some(reader) = reader {
                reader.wake();
            }
        }
    }
}

impl AsyncRead for Abandonable {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.setup.stage.load(Ordering::SeqCst) == SETTING_UP {
            // the waker first, then the stage: a dial going away in between
            // either is seen here or finds the waker
            *this.setup.reader.lock().unwrap() = Some(cx.waker().clone());
        }
        if this.setup.stage.load(Ordering::SeqCst) == ABANDONED {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Abandonable {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Ends the session once no channel has been open on it for `idle`
/// (`idle-timeout`: idle means no channel, not no traffic, M4-D10). Holds
/// the session weakly: a session replaced or dropped ends the watch.
fn watch_idle(session: Weak<Session>, idle: Duration) {
    tokio::spawn(async move {
        loop {
            let Some((open, activity)) = session
                .upgrade()
                .map(|s| (s.open.clone(), s.activity.clone()))
            else {
                return;
            };
            if open.load(Ordering::SeqCst) > 0 {
                activity.notified().await;
                continue;
            }
            if tokio::time::timeout(idle, activity.notified())
                .await
                .is_ok()
            {
                // a channel opened or closed meanwhile: look again
                continue;
            }
            if open.load(Ordering::SeqCst) == 0 {
                if let Some(session) = session.upgrade() {
                    let _ = session
                        .handle
                        .disconnect(Disconnect::ByApplication, "", "")
                        .await;
                }
                return;
            }
        }
    });
}

enum ChannelError {
    /// The session is gone: another one may do.
    Gone,
    Refused(ChannelOpenFailure),
}

impl ChannelError {
    fn into_outbound(self) -> OutboundError {
        match self {
            ChannelError::Gone => OutboundError::Proxy("ssh: the session closed".to_string()),
            // the reason code only: the server's own text is not repeated
            ChannelError::Refused(reason) => OutboundError::Proxy(format!(
                "ssh: the server refused the channel ({})",
                match reason {
                    ChannelOpenFailure::AdministrativelyProhibited =>
                        "administratively prohibited".to_string(),
                    ChannelOpenFailure::ConnectFailed => "connect failed".to_string(),
                    ChannelOpenFailure::UnknownChannelType => "unknown channel type".to_string(),
                    ChannelOpenFailure::ResourceShortage => "resource shortage".to_string(),
                    ChannelOpenFailure::Other { code, .. } => format!("code {code}"),
                }
            )),
        }
    }
}

/// A connection attempt's failure, kept only so that other dials fail the
/// same way instead of making their own attempt: those that were already
/// waiting for it (design 5.1: concurrent dials share one handshake,
/// including its outcome) and, after a failure another attempt cannot fix,
/// those of the back-off that follows — a server's fail2ban counts failed
/// logins, so dials each retrying the same wrong credentials would just add
/// to the count. `OutboundError` is not `Clone` (`Io` holds an
/// `io::Error`), so only the variant and its fixed text are kept here;
/// nothing else of what the server sent.
#[derive(Clone)]
enum AttemptFailure {
    Reject(RejectKind),
    Unsupported(String),
    Dns(String),
    Io(io::ErrorKind, String),
    Timeout,
    Proxy(String),
    Tls(String),
    Unavailable(String),
}

impl AttemptFailure {
    fn capture(e: &OutboundError) -> AttemptFailure {
        match e {
            OutboundError::Reject(k) => AttemptFailure::Reject(*k),
            OutboundError::Unsupported(s) => AttemptFailure::Unsupported(s.clone()),
            OutboundError::Dns(s) => AttemptFailure::Dns(s.clone()),
            OutboundError::Io(e) => AttemptFailure::Io(e.kind(), e.to_string()),
            OutboundError::Timeout => AttemptFailure::Timeout,
            OutboundError::Proxy(s) => AttemptFailure::Proxy(s.clone()),
            OutboundError::Tls(s) => AttemptFailure::Tls(s.clone()),
            OutboundError::Unavailable(s) => AttemptFailure::Unavailable(s.clone()),
        }
    }

    fn into_outbound(self) -> OutboundError {
        match self {
            AttemptFailure::Reject(k) => OutboundError::Reject(k),
            AttemptFailure::Unsupported(s) => OutboundError::Unsupported(s),
            AttemptFailure::Dns(s) => OutboundError::Dns(s),
            AttemptFailure::Io(kind, text) => OutboundError::Io(io::Error::new(kind, text)),
            AttemptFailure::Timeout => OutboundError::Timeout,
            AttemptFailure::Proxy(s) => OutboundError::Proxy(s),
            AttemptFailure::Tls(s) => OutboundError::Tls(s),
            AttemptFailure::Unavailable(s) => OutboundError::Unavailable(s),
        }
    }

    /// One of the failures another attempt right away cannot fix.
    fn is_persistent(&self) -> bool {
        matches!(self, AttemptFailure::Proxy(text)
            if [AUTHENTICATION_FAILED, UNKNOWN_HOST_KEY, NO_COMMON_ALGORITHM]
                .contains(&text.as_str()))
    }
}

/// What failed attempts leave for the dials after them.
struct Failures {
    /// The most recent attempt's failure; cleared on success.
    last: Option<AttemptFailure>,
    /// Set by a persistent failure: until then every dial fails with `last`
    /// at once, without connecting.
    until: Option<Instant>,
    /// The back-off the latest persistent failure started; the next one
    /// doubles it. Only a success clears it: a transient failure in between
    /// neither starts, extends nor resets a back-off.
    period: Option<Duration>,
    /// The first back-off: `FIRST_BACKOFF` (shorter in this module's tests).
    first: Duration,
}

impl Failures {
    fn new() -> Failures {
        Failures {
            last: None,
            until: None,
            period: None,
            first: FIRST_BACKOFF,
        }
    }

    /// The failure a dial fails with instead of making its own attempt: the
    /// one of the attempt it waited for (`waited`), or the one a back-off
    /// runs for.
    fn shared(&self, waited: bool, now: Instant) -> Option<AttemptFailure> {
        let backing_off = self.until.is_some_and(|until| now < until);
        if waited || backing_off {
            self.last.clone()
        } else {
            None
        }
    }

    fn failed(&mut self, failure: AttemptFailure, now: Instant) {
        if failure.is_persistent() {
            let period = self
                .period
                .map_or(self.first, |period| (period * 2).min(LONGEST_BACKOFF));
            self.period = Some(period);
            self.until = Some(now + period);
        }
        self.last = Some(failure);
    }

    fn succeeded(&mut self) {
        self.last = None;
        self.until = None;
        self.period = None;
    }
}

pub struct SshOutbound {
    name: String,
    stack: Stack,
    user: String,
    password: Option<String>,
    key: Option<Arc<PrivateKey>>,
    pins: Arc<[HostKeyPin]>,
    idle_timeout: Duration,
    /// One handshake at a time: dials that come in meanwhile wait for it.
    session: Mutex<Option<Arc<Session>>>,
    unpinned_warned: AtomicBool,
    /// Bumped after every attempt, success or failure: tells a dial that was
    /// waiting for the lock whether one finished while it waited.
    attempts: AtomicU64,
    /// The latest failure and the back-off after persistent ones; read and
    /// written with the session lock held, never across an `.await`.
    failures: StdMutex<Failures>,
    /// `LOGIN_LIMIT` (shorter in the tests).
    login_limit: Duration,
}

impl SshOutbound {
    /// Decodes the private key now, so that `rurge check` reports a key rurge
    /// cannot use.
    pub fn new(
        name: &str,
        server: Target,
        ssh: &SshSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<SshOutbound, BuildError> {
        let key = match &ssh.private_key {
            None => None,
            Some(item) => {
                let item = keystore.iter().find(|k| &k.name == item).ok_or_else(|| {
                    BuildError::new(format!("keystore item `{item}` does not exist"))
                })?;
                Some(Arc::new(decode_private_key(item)?))
            }
        };
        let shadow_tls = shadow_tls_client(shadow_tls, None, &server.host, roots)?;
        Ok(SshOutbound {
            name: name.to_string(),
            stack: Stack::new(connector, server, shadow_tls, None, None),
            user: ssh.username.expose().clone(),
            password: ssh.password.as_ref().map(|p| p.expose().clone()),
            key,
            pins: ssh.host_keys.clone().into(),
            idle_timeout: ssh.idle_timeout,
            session: Mutex::new(None),
            unpinned_warned: AtomicBool::new(false),
            attempts: AtomicU64::new(0),
            failures: StdMutex::new(Failures::new()),
            login_limit: LOGIN_LIMIT,
        })
    }

    async fn dial(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let session = self.session(opts).await?;
        match open_channel(&session, target).await {
            Err(ChannelError::Gone) => {
                // the session ended since it was last used: one new one
                self.forget(&session).await;
                let session = self.session(opts).await?;
                open_channel(&session, target)
                    .await
                    .map_err(ChannelError::into_outbound)
            }
            other => other.map_err(ChannelError::into_outbound),
        }
    }

    async fn session(&self, opts: &ConnectOpts) -> Result<Arc<Session>, OutboundError> {
        // read before contending for the lock: tells us whether an attempt
        // finished while we waited for it
        let attempt = self.attempts.load(Ordering::SeqCst);
        let mut slot = self.session.lock().await;
        if let Some(session) = slot.as_ref().filter(|s| !s.handle.is_closed()) {
            return Ok(session.clone());
        }
        // no live session: an attempt that finished while we waited shares
        // its failure instead of us trying the same thing over again, and so
        // does the one a back-off runs for
        let waited = self.attempts.load(Ordering::SeqCst) != attempt;
        let shared = self.failures.lock().unwrap().shared(waited, Instant::now());
        if let Some(failure) = shared {
            return Err(failure.into_outbound());
        }
        let result = self.establish(opts).await;
        self.attempts.fetch_add(1, Ordering::SeqCst);
        match result {
            Ok(session) => {
                let session = Arc::new(session);
                watch_idle(Arc::downgrade(&session), self.idle_timeout);
                *slot = Some(session.clone());
                self.failures.lock().unwrap().succeeded();
                Ok(session)
            }
            Err(e) => {
                let failure = AttemptFailure::capture(&e);
                self.failures
                    .lock()
                    .unwrap()
                    .failed(failure, Instant::now());
                Err(e)
            }
        }
    }

    async fn forget(&self, dead: &Arc<Session>) {
        let mut slot = self.session.lock().await;
        if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, dead)) {
            *slot = None;
        }
    }

    async fn establish(&self, opts: &ConnectOpts) -> Result<Session, OutboundError> {
        let (stream, armed) = abandonable(self.stack.open(opts).await?);
        let client = Client {
            pins: self.pins.clone(),
        };
        let config = session_config(&self.pins);
        let login = async move {
            let handshake = client::connect_stream(Arc::new(config), stream, client).await;
            armed.key_exchange_done();
            let mut handle = handshake.map_err(handshake_error)?;
            self.authenticate(&mut handle).await?;
            Ok::<_, OutboundError>(handle)
        };
        // given up at the limit, the login takes the session with it: the
        // connection is abandoned during the key exchange, the handle is
        // dropped after it
        let handle = tokio::time::timeout(self.login_limit, login)
            .await
            .map_err(|_| OutboundError::Timeout)??;
        if self.first_unpinned() {
            // the policy name only
            tracing::warn!(
                policy = %self.name,
                "ssh: no server-fingerprint; the server's host key is not verified"
            );
        }
        Ok(Session {
            handle,
            open: Arc::new(AtomicUsize::new(0)),
            activity: Arc::new(Notify::new()),
        })
    }

    /// True once, at the first session of a policy without
    /// `server-fingerprint` (manual: a one-time security warning).
    fn first_unpinned(&self) -> bool {
        self.pins.is_empty() && !self.unpinned_warned.swap(true, Ordering::Relaxed)
    }

    /// The key first, then the password, as the OpenSSH client does.
    async fn authenticate(&self, handle: &mut Handle<Client>) -> Result<(), OutboundError> {
        if let Some(key) = &self.key {
            let hash = if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
                let listed = handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(handshake_error)?;
                Some(rsa_hash(listed))
            } else {
                None
            };
            let key = PrivateKeyWithHashAlg::new(key.clone(), hash);
            let result = handle
                .authenticate_publickey(&self.user, key)
                .await
                .map_err(handshake_error)?;
            if result.success() {
                return Ok(());
            }
        }
        if let Some(password) = &self.password {
            let result = handle
                .authenticate_password(&self.user, password)
                .await
                .map_err(handshake_error)?;
            if result.success() {
                return Ok(());
            }
        }
        // russh reports a login as not accepted also when the session ended
        // while it waited for the answer: that is the connection failing
        let failed = if handle.is_closed() {
            HANDSHAKE_FAILED
        } else {
            AUTHENTICATION_FAILED
        };
        Err(OutboundError::Proxy(failed.to_string()))
    }
}

async fn open_channel(session: &Session, target: &Target) -> Result<BoxedStream, ChannelError> {
    // counted from the request on, so the idle watch never finds the session
    // unused while a channel is being opened; a failed open drops it
    let open = OpenChannel::new(session);
    let opened = session
        .handle
        .channel_open_direct_tcpip(
            target.host.to_string(),
            u32::from(target.port),
            "127.0.0.1",
            0,
        )
        .await;
    match opened {
        Ok(channel) => Ok(Box::new(SshStream {
            channel: channel.into_stream(),
            _open: open,
        })),
        Err(Error::ChannelOpenFailure(reason)) => Err(ChannelError::Refused(reason)),
        Err(_) => Err(ChannelError::Gone),
    }
}

/// The hash an RSA key signs with: SHA-512 when the server lists it in
/// `server-sig-algs`, else SHA-256 — also when the server lists nothing.
/// Never SHA-1 (`ssh-rsa`), not even for a server that lists only that.
fn rsa_hash(listed: Option<Option<HashAlg>>) -> HashAlg {
    match listed {
        Some(Some(HashAlg::Sha512)) => HashAlg::Sha512,
        _ => HashAlg::Sha256,
    }
}

/// Fixed texts: what the server sent is not repeated. The connection is up
/// by now, so an I/O error too is the handshake failing.
fn handshake_error(e: Error) -> OutboundError {
    OutboundError::Proxy(
        match e {
            Error::UnknownKey => UNKNOWN_HOST_KEY,
            Error::NoCommonAlgo { .. } => NO_COMMON_ALGORITHM,
            _ => HANDSHAKE_FAILED,
        }
        .to_string(),
    )
}

impl Outbound for SshOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.dial(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeSsh, FakeSshOpts, RSA_KEY, keystore_item, random_key};
    use rurge_config::HostName;
    use rurge_config::spec::ssh::DEFAULT_IDLE_TIMEOUT;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use russh::keys::PublicKey;
    use russh::keys::ssh_key::{EcdsaCurve, LineEnding};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A loopback server that echoes what it reads.
    async fn echo() -> Target {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    fn spec(password: Option<&str>, key: Option<&str>, host_keys: Vec<HostKeyPin>) -> SshSpec {
        SshSpec {
            username: "u".into(),
            password: password.map(Into::into),
            private_key: key.map(str::to_string),
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            host_keys,
        }
    }

    fn pin(key: &PublicKey) -> HostKeyPin {
        HostKeyPin {
            algorithm: key.algorithm().to_string(),
            blob: key.to_bytes().unwrap(),
        }
    }

    fn outbound_to(
        addr: std::net::SocketAddr,
        ssh: &SshSpec,
        keystore: &[KeystoreItem],
    ) -> SshOutbound {
        SshOutbound::new(
            "S",
            Target::new(HostName::Ip(addr.ip()), addr.port()),
            ssh,
            None,
            keystore,
            Arc::new(RootCertStore::empty()),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    async fn fake(opts: FakeSshOpts) -> FakeSsh {
        FakeSsh::start(FakeSshOpts {
            user: "u".into(),
            ..opts
        })
        .await
    }

    /// Four bytes there and back through `outbound`; bounded, so that a
    /// regression fails instead of hanging.
    async fn round_trip(outbound: &SshOutbound, target: &Target) -> Result<(), OutboundError> {
        let echoed = async {
            let mut stream = outbound
                .connect_tcp(target, &ConnectOpts::default())
                .await?;
            stream.write_all(b"ping").await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
            Ok(())
        };
        tokio::time::timeout(Duration::from_secs(5), echoed)
            .await
            .expect("the round trip took over five seconds")
    }

    #[tokio::test]
    async fn password_and_key_logins_forward_a_connection() {
        let ed25519 = random_key(Algorithm::Ed25519);
        let ecdsa = random_key(Algorithm::Ecdsa {
            curve: EcdsaCurve::NistP256,
        });
        let rsa = crate::decode_private_key(&keystore_item("rsa", RSA_KEY)).unwrap();
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            keys: vec![
                ed25519.public_key().clone(),
                ecdsa.public_key().clone(),
                rsa.public_key().clone(),
            ],
            ..Default::default()
        })
        .await;
        let keystore = [
            keystore_item("ed", &ed25519.to_openssh(LineEnding::LF).unwrap()),
            keystore_item("ec", &ecdsa.to_openssh(LineEnding::LF).unwrap()),
            keystore_item("rsa", RSA_KEY),
        ];
        let target = echo().await;
        for ssh in [
            spec(Some("pw"), None, vec![]),
            spec(None, Some("ed"), vec![]),
            spec(None, Some("ec"), vec![]),
            spec(None, Some("rsa"), vec![]),
        ] {
            round_trip(&outbound_to(server.addr, &ssh, &keystore), &target)
                .await
                .unwrap();
        }
        assert_eq!(server.logins(), 4);
    }

    /// The manual's "Algorithm Requirements": `curve25519-sha256` and
    /// `aes128-gcm@openssh.com` are enough.
    #[tokio::test]
    async fn a_server_with_only_surges_algorithms_is_reached() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            surge_minimum: true,
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]);
        round_trip(&ssh, &echo().await).await.unwrap();
    }

    #[tokio::test]
    async fn one_session_carries_every_connection() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = Arc::new(outbound_to(
            server.addr,
            &spec(Some("pw"), None, vec![]),
            &[],
        ));
        let target = echo().await;
        let mut dials = tokio::task::JoinSet::new();
        for _ in 0..5 {
            let (ssh, target) = (ssh.clone(), target.clone());
            dials.spawn(async move { round_trip(&ssh, &target).await });
        }
        while let Some(dialed) = dials.join_next().await {
            dialed.unwrap().unwrap();
        }
        assert_eq!(server.logins(), 1);
    }

    #[tokio::test]
    async fn a_session_the_server_ended_is_replaced() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]);
        let target = echo().await;
        round_trip(&ssh, &target).await.unwrap();
        server.end_sessions().await;
        round_trip(&ssh, &target).await.unwrap();
        assert_eq!(server.logins(), 2);
    }

    #[tokio::test]
    async fn a_host_key_outside_server_fingerprint_is_refused() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let other = random_key(Algorithm::Ed25519);
        let target = echo().await;
        let pinned_elsewhere = spec(Some("pw"), None, vec![pin(other.public_key())]);
        let refused = round_trip(&outbound_to(server.addr, &pinned_elsewhere, &[]), &target)
            .await
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "ssh: the server's host key is not one of server-fingerprint"
        );
        let pinned = spec(
            Some("pw"),
            None,
            vec![pin(other.public_key()), pin(&server.host_key)],
        );
        round_trip(&outbound_to(server.addr, &pinned, &[]), &target)
            .await
            .unwrap();
        assert_eq!(server.logins(), 1);
    }

    /// A server with several host keys presents the one whose algorithm the
    /// client lists first: the pinned keys' algorithms come first, so any
    /// one of its host keys may be pinned alone.
    #[tokio::test]
    async fn any_one_of_the_servers_host_keys_may_be_pinned_alone() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            extra_host_keys: vec![
                random_key(Algorithm::Ecdsa {
                    curve: EcdsaCurve::NistP256,
                }),
                crate::decode_private_key(&keystore_item("rsa", RSA_KEY)).unwrap(),
            ],
            ..Default::default()
        })
        .await;
        let target = echo().await;
        for host_key in &server.host_keys {
            let pinned = spec(Some("pw"), None, vec![pin(host_key)]);
            round_trip(&outbound_to(server.addr, &pinned, &[]), &target)
                .await
                .unwrap_or_else(|e| panic!("pinned {}: {e}", host_key.algorithm()));
        }
        assert_eq!(server.logins(), 3);
    }

    /// The pinned keys' algorithms first, in the order they are written; an
    /// RSA key (`ssh-rsa`) stands for rsa-sha2-512 then rsa-sha2-256, never
    /// SHA-1; an algorithm rurge does not offer adds nothing.
    #[test]
    fn pinned_host_key_algorithms_are_offered_first() {
        let offered = |algorithms: &[&str]| {
            let pins: Vec<HostKeyPin> = algorithms
                .iter()
                .map(|algorithm| HostKeyPin {
                    algorithm: algorithm.to_string(),
                    blob: Vec::new(),
                })
                .collect();
            preferred(&pins)
                .key
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        let unpinned = [
            "ssh-ed25519",
            "ecdsa-sha2-nistp256",
            "ecdsa-sha2-nistp384",
            "ecdsa-sha2-nistp521",
            "rsa-sha2-512",
            "rsa-sha2-256",
        ];
        assert_eq!(offered(&[]), unpinned);
        assert_eq!(
            offered(&["ssh-rsa", "ecdsa-sha2-nistp384", "ssh-rsa"]),
            [
                "rsa-sha2-512",
                "rsa-sha2-256",
                "ecdsa-sha2-nistp384",
                "ssh-ed25519",
                "ecdsa-sha2-nistp256",
                "ecdsa-sha2-nistp521",
            ]
        );
        assert_eq!(
            offered(&["ssh-dss", "sk-ssh-ed25519@openssh.com"]),
            unpinned
        );
    }

    #[tokio::test]
    async fn a_wrong_password_fails_without_repeating_it() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("hunter2"), None, vec![]), &[]);
        let failed = round_trip(&ssh, &echo().await).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: authentication failed");
        assert_eq!(server.logins(), 0);
    }

    /// The first back-off in these tests: a second instead of a minute.
    const BACKOFF: Duration = Duration::from_secs(1);

    fn backing_off(ssh: SshOutbound) -> SshOutbound {
        ssh.failures.lock().unwrap().first = BACKOFF;
        ssh
    }

    /// Dials that were waiting on the session lock while a wrong-password
    /// handshake ran share that failed attempt instead of trying the same
    /// password again themselves, and so do the dials during the back-off
    /// that follows: a server's fail2ban counts failed logins, so each dial
    /// failing its own login would add to the count. The first dial after
    /// the back-off tries again; a second failure in a row doubles it.
    #[tokio::test]
    async fn concurrent_dials_share_a_failed_login_and_later_ones_back_off() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = Arc::new(backing_off(outbound_to(
            server.addr,
            &spec(Some("hunter2"), None, vec![]),
            &[],
        )));
        let target = echo().await;
        let mut dials = tokio::task::JoinSet::new();
        for _ in 0..5 {
            let (ssh, target) = (ssh.clone(), target.clone());
            dials.spawn(async move { round_trip(&ssh, &target).await });
        }
        while let Some(dialed) = dials.join_next().await {
            let failed = dialed.unwrap().unwrap_err();
            assert_eq!(failed.to_string(), "ssh: authentication failed");
        }
        assert_eq!(server.attempts(), 1);
        // during the back-off: the same failure at once, no login
        let failed = round_trip(&ssh, &target).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: authentication failed");
        assert_eq!(server.attempts(), 1);
        // after it: a login again, whose failure doubles the back-off
        tokio::time::sleep(BACKOFF).await;
        let failed = round_trip(&ssh, &target).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: authentication failed");
        assert_eq!(server.attempts(), 2);
        assert_eq!(ssh.failures.lock().unwrap().period, Some(2 * BACKOFF));
    }

    /// A login cut off by the server hanging up is the connection failing,
    /// not the credentials: no back-off, the next dial tries again at once.
    #[tokio::test]
    async fn a_login_the_server_hangs_up_on_is_retried_at_once() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            hang_up_on_password: true,
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]);
        let target = echo().await;
        for attempts in 1..=2 {
            let failed = round_trip(&ssh, &target).await.unwrap_err();
            assert_eq!(failed.to_string(), "ssh: the handshake failed");
            assert_eq!(server.attempts(), attempts);
        }
    }

    /// A login that succeeds ends the back-off: the next failure backs off
    /// for the first period again, not for twice as long.
    #[tokio::test]
    async fn a_successful_login_resets_the_back_off() {
        let server = fake(FakeSshOpts {
            password: Some("old".into()),
            ..Default::default()
        })
        .await;
        let ssh = backing_off(outbound_to(
            server.addr,
            &spec(Some("pw"), None, vec![]),
            &[],
        ));
        let target = echo().await;
        let failed = round_trip(&ssh, &target).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: authentication failed");
        // the server takes rurge's password now: in after the back-off
        server.set_password("pw");
        tokio::time::sleep(BACKOFF).await;
        round_trip(&ssh, &target).await.unwrap();
        assert_eq!(server.attempts(), 2);
        // it changes again and the session ends: a new back-off ...
        server.set_password("new");
        server.end_sessions().await;
        eventually(|| server.live_sessions() == 0).await;
        let failed = round_trip(&ssh, &target).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: authentication failed");
        round_trip(&ssh, &target).await.unwrap_err();
        assert_eq!(server.attempts(), 3);
        // ... over after the first period, well before a doubled one
        tokio::time::sleep(BACKOFF * 3 / 2).await;
        round_trip(&ssh, &target).await.unwrap_err();
        assert_eq!(server.attempts(), 4);
    }

    /// The key is tried first; a server that does not take it still takes
    /// the password.
    #[tokio::test]
    async fn a_key_the_server_does_not_take_is_followed_by_the_password() {
        let key = random_key(Algorithm::Ed25519);
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let keystore = [keystore_item("k", &key.to_openssh(LineEnding::LF).unwrap())];
        let ssh = outbound_to(server.addr, &spec(Some("pw"), Some("k"), vec![]), &keystore);
        round_trip(&ssh, &echo().await).await.unwrap();
        // the key's login, then the password's
        assert_eq!((server.attempts(), server.logins()), (2, 1));
    }

    /// Records `failure` at `at`; for how many seconds from then on dials
    /// fail at once.
    fn backoff_after(failures: &mut Failures, failure: AttemptFailure, at: Instant) -> u64 {
        failures.failed(failure, at);
        (0..=LONGEST_BACKOFF.as_secs())
            .take_while(|s| {
                failures
                    .shared(false, at + Duration::from_secs(*s))
                    .is_some()
            })
            .count() as u64
    }

    /// A minute after a failure another attempt cannot fix, doubled by each
    /// further one up to ten minutes; only a success starts over. Any other
    /// failure is shared only by the dials that waited for it.
    #[test]
    fn persistent_failures_back_off_a_minute_doubling_up_to_ten() {
        let proxy = |text: &str| AttemptFailure::Proxy(text.to_string());
        let start = Instant::now();
        for text in [AUTHENTICATION_FAILED, UNKNOWN_HOST_KEY, NO_COMMON_ALGORITHM] {
            assert_eq!(backoff_after(&mut Failures::new(), proxy(text), start), 60);
        }
        for transient in [
            proxy(HANDSHAKE_FAILED),
            proxy("ssh: the session closed"),
            AttemptFailure::Timeout,
        ] {
            let mut failures = Failures::new();
            assert_eq!(backoff_after(&mut failures, transient, start), 0);
            assert!(failures.shared(true, start).is_some());
        }
        // each failure comes when the back-off before it has run out
        let (mut failures, mut at, mut periods) = (Failures::new(), start, Vec::new());
        for failure in [
            proxy(AUTHENTICATION_FAILED),
            proxy(HANDSHAKE_FAILED),
            proxy(AUTHENTICATION_FAILED),
            proxy(UNKNOWN_HOST_KEY),
            proxy(AUTHENTICATION_FAILED),
            proxy(AUTHENTICATION_FAILED),
            proxy(AUTHENTICATION_FAILED),
        ] {
            let period = backoff_after(&mut failures, failure, at);
            at += Duration::from_secs(period);
            periods.push(period);
        }
        // the transient failure in between neither backs off nor resets
        assert_eq!(periods, [60, 0, 120, 240, 480, 600, 600]);
        failures.succeeded();
        assert_eq!(
            backoff_after(&mut failures, proxy(AUTHENTICATION_FAILED), at),
            60
        );
    }

    #[test]
    fn an_rsa_key_never_signs_with_sha1() {
        assert_eq!(rsa_hash(Some(Some(HashAlg::Sha512))), HashAlg::Sha512);
        assert_eq!(rsa_hash(Some(Some(HashAlg::Sha256))), HashAlg::Sha256);
        // a server that lists only `ssh-rsa`, and one that lists nothing
        assert_eq!(rsa_hash(Some(None)), HashAlg::Sha256);
        assert_eq!(rsa_hash(None), HashAlg::Sha256);
    }

    /// The reason code is named; the session stays for the next connection.
    #[tokio::test]
    async fn a_refused_channel_names_the_reason_and_keeps_the_session() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            refuse_channels: true,
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]);
        let target = echo().await;
        for _ in 0..2 {
            let refused = round_trip(&ssh, &target).await.unwrap_err();
            assert_eq!(
                refused.to_string(),
                "ssh: the server refused the channel (connect failed)"
            );
        }
        assert_eq!(server.logins(), 1);
    }

    /// rurge asks for no forwarding: a channel the server opens toward it is
    /// refused, whatever its kind.
    #[tokio::test]
    async fn channels_the_server_opens_are_refused() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]);
        let _held = ssh
            .connect_tcp(&echo().await, &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(
            server.open_channels_toward_client().await,
            vec![Some(ChannelOpenFailure::AdministrativelyProhibited); 7]
        );
    }

    fn with_idle(mut ssh: SshSpec, secs: u64) -> SshSpec {
        ssh.idle_timeout = Duration::from_secs(secs);
        ssh
    }

    /// Polls `check` until it holds, for at most five seconds.
    async fn eventually(check: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(tokio::time::Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_session_without_channels_closes_after_the_idle_timeout() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(
            server.addr,
            &with_idle(spec(Some("pw"), None, vec![]), 1),
            &[],
        );
        let target = echo().await;
        round_trip(&ssh, &target).await.unwrap();
        eventually(|| server.live_sessions() == 0).await;
        round_trip(&ssh, &target).await.unwrap();
        assert_eq!(server.logins(), 2);
    }

    /// Idle means no open channel, not no traffic: a quiet connection keeps
    /// its session.
    #[tokio::test]
    async fn an_open_channel_keeps_the_session_past_the_idle_timeout() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = outbound_to(
            server.addr,
            &with_idle(spec(Some("pw"), None, vec![]), 1),
            &[],
        );
        let mut held = ssh
            .connect_tcp(&echo().await, &ConnectOpts::default())
            .await
            .unwrap();
        // the window being observed: twice the idle timeout
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(server.live_sessions(), 1);
        held.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        held.read_exact(&mut buf).await.unwrap();
        drop(held);
        eventually(|| server.live_sessions() == 0).await;
    }

    /// Also: russh's five-minute inactivity limit, which bounds a write the
    /// server does not take.
    #[test]
    fn the_session_is_kept_alive_every_thirty_seconds() {
        let config = session_config(&[]);
        assert_eq!(
            (
                config.keepalive_interval,
                config.keepalive_max,
                config.inactivity_timeout
            ),
            (
                Some(Duration::from_secs(30)),
                3,
                Some(Duration::from_secs(300))
            )
        );
    }

    #[test]
    fn a_policy_without_server_fingerprint_is_warned_about_once() {
        let addr = "127.0.0.1:9".parse().unwrap();
        let unpinned = outbound_to(addr, &spec(Some("pw"), None, vec![]), &[]);
        assert!(unpinned.first_unpinned());
        assert!(!unpinned.first_unpinned());
        let key = random_key(Algorithm::Ed25519);
        let pinned = outbound_to(
            addr,
            &spec(Some("pw"), None, vec![pin(key.public_key())]),
            &[],
        );
        assert!(!pinned.first_unpinned());
    }

    #[tokio::test]
    async fn a_server_that_does_not_speak_ssh_fails_the_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
            }
        });
        let ssh = outbound_to(addr, &spec(Some("pw"), None, vec![]), &[]);
        let failed = round_trip(&ssh, &echo().await).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: the handshake failed");
    }

    /// A server that sends its identification and then says nothing more,
    /// in the middle of the key exchange; the receiver fires once the
    /// client closes the connection.
    async fn stalling_server() -> (std::net::SocketAddr, tokio::sync::oneshot::Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (closed, closed_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            s.write_all(b"SSH-2.0-OpenSSH_9.9\r\n").await.unwrap();
            let mut buf = [0u8; 4096];
            while let Ok(read) = s.read(&mut buf).await {
                if read == 0 {
                    break;
                }
            }
            let _ = closed.send(());
        });
        (addr, closed_rx)
    }

    /// A dial that gives up during the key exchange takes the connection
    /// with it: russh's session task does not look at its `Handle` while a
    /// key exchange runs, and it would otherwise wait on — and spin once
    /// its first keepalive falls due — until the server hangs up.
    #[tokio::test]
    async fn a_handshake_whose_dial_gave_up_does_not_live_on() {
        let (addr, closed) = stalling_server().await;
        let ssh = outbound_to(addr, &spec(Some("pw"), None, vec![]), &[]);
        let opts = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
        match ssh.connect_tcp(&echo().await, &opts).await {
            Err(OutboundError::Timeout) => {}
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("the server never finishes the handshake"),
        }
        tokio::time::timeout(Duration::from_secs(5), closed)
            .await
            .expect("the connection was left open")
            .unwrap();
    }

    fn with_login_limit(mut ssh: SshOutbound, limit: Duration) -> SshOutbound {
        ssh.login_limit = limit;
        ssh
    }

    /// A dial may be allowed longer than the first keepalive (a
    /// connectivity test's `test-timeout` may be), but a stalled login still
    /// ends at the login limit, before that keepalive falls due while
    /// nobody is logged in.
    #[tokio::test]
    async fn a_login_ends_at_its_limit_however_long_the_dial_may_take() {
        let (addr, closed) = stalling_server().await;
        let ssh = with_login_limit(
            outbound_to(addr, &spec(Some("pw"), None, vec![]), &[]),
            Duration::from_millis(300),
        );
        let opts = ConnectOpts {
            timeout: Duration::from_secs(60),
        };
        let dialed = tokio::time::timeout(
            Duration::from_secs(5),
            ssh.connect_tcp(&echo().await, &opts),
        )
        .await
        .expect("the login went on past its limit");
        match dialed {
            Err(OutboundError::Timeout) => {}
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("the server never finishes the handshake"),
        }
        tokio::time::timeout(Duration::from_secs(5), closed)
            .await
            .expect("the connection was left open")
            .unwrap();
    }

    /// The limit covers the login too: once the key exchange is done, a
    /// server that sits on a password is given up on at the limit as well.
    #[tokio::test]
    async fn a_login_the_server_sits_on_ends_at_the_limit_too() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            stall_on_password: Some(Duration::from_secs(10)),
            ..Default::default()
        })
        .await;
        let ssh = with_login_limit(
            outbound_to(server.addr, &spec(Some("pw"), None, vec![]), &[]),
            Duration::from_secs(2),
        );
        let opts = ConnectOpts {
            timeout: Duration::from_secs(60),
        };
        let dialed = tokio::time::timeout(
            Duration::from_secs(8),
            ssh.connect_tcp(&echo().await, &opts),
        )
        .await
        .expect("the login went on past its limit");
        match dialed {
            Err(OutboundError::Timeout) => {}
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("the server never answers the password in time"),
        }
        // the key exchange was done: the password had gone out
        assert_eq!(server.attempts(), 1);
    }
}
