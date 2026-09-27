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
use russh::{ChannelOpenFailure, ChannelStream, Disconnect, Error, Preferred, cipher};
use rustls::RootCertStore;
use std::borrow::Cow;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Mutex, Notify};

/// A keepalive every 30 seconds while the session is up; three without an
/// answer end it (M4-D10).
const KEEPALIVE: Duration = Duration::from_secs(30);
const KEEPALIVE_MAX: usize = 3;

/// The session's side of the conversation: the server's host key is checked
/// against `server-fingerprint`.
struct Client {
    pins: Arc<[HostKeyPin]>,
}

impl client::Handler for Client {
    type Error = Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Error> {
        Ok(host_key_allowed(&self.pins, key))
    }
}

/// russh's defaults, with the one cipher Surge's manual requires added
/// (`aes128-gcm@openssh.com`, "Algorithm Requirements") and the SHA-1 host
/// key signature (`ssh-rsa`) left out.
fn preferred() -> Preferred {
    Preferred {
        cipher: Cow::Owned(vec![
            cipher::CHACHA20_POLY1305,
            cipher::AES_256_GCM,
            cipher::AES_128_GCM,
            cipher::AES_256_CTR,
            cipher::AES_192_CTR,
            cipher::AES_128_CTR,
        ]),
        key: Cow::Owned(
            Preferred::DEFAULT
                .key
                .iter()
                .filter(|algorithm| !matches!(algorithm, Algorithm::Rsa { hash: None }))
                .cloned()
                .collect(),
        ),
        ..Preferred::DEFAULT
    }
}

fn session_config() -> client::Config {
    client::Config {
        preferred: preferred(),
        keepalive_interval: Some(KEEPALIVE),
        keepalive_max: KEEPALIVE_MAX,
        ..Default::default()
    }
}

struct Session {
    handle: Handle<Client>,
    /// Channels handed out and not yet dropped.
    open: Arc<AtomicUsize>,
    /// Woken when a channel opens or closes.
    activity: Arc<Notify>,
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

/// A connection attempt's failure, kept only so dials that were already
/// waiting for it can fail the same way instead of making their own attempt
/// (design 5.1: concurrent dials share one handshake, including its
/// outcome) — a server's fail2ban counts failed logins, so queued dials
/// each retrying the same wrong credentials would just add to the count.
/// `OutboundError` is not `Clone` (`Io` holds an `io::Error`), so only the
/// variant and its fixed text are kept here; nothing else of what the
/// server sent.
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
    /// The most recent attempt's failure, kept while the slot is still empty
    /// because of it; cleared on success. A dial that starts after it was
    /// recorded makes its own attempt instead of replaying this one.
    last_failure: StdMutex<Option<AttemptFailure>>,
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
            last_failure: StdMutex::new(None),
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
        if self.attempts.load(Ordering::SeqCst) != attempt {
            // an attempt finished while we waited and left no live session:
            // share its failure instead of trying the same thing over again
            if let Some(failure) = self.last_failure.lock().unwrap().clone() {
                return Err(failure.into_outbound());
            }
        }
        let result = self.establish(opts).await;
        self.attempts.fetch_add(1, Ordering::SeqCst);
        match result {
            Ok(session) => {
                let session = Arc::new(session);
                watch_idle(Arc::downgrade(&session), self.idle_timeout);
                *slot = Some(session.clone());
                *self.last_failure.lock().unwrap() = None;
                Ok(session)
            }
            Err(e) => {
                *self.last_failure.lock().unwrap() = Some(AttemptFailure::capture(&e));
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
        let stream = self.stack.open(opts).await?;
        let client = Client {
            pins: self.pins.clone(),
        };
        let mut handle = client::connect_stream(Arc::new(session_config()), stream, client)
            .await
            .map_err(handshake_error)?;
        self.authenticate(&mut handle).await?;
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
        Err(OutboundError::Proxy(
            "ssh: authentication failed".to_string(),
        ))
    }
}

async fn open_channel(session: &Session, target: &Target) -> Result<BoxedStream, ChannelError> {
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
            _open: OpenChannel::new(session),
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
    match e {
        Error::UnknownKey => OutboundError::Proxy(
            "ssh: the server's host key is not one of server-fingerprint".to_string(),
        ),
        Error::NoCommonAlgo { .. } => OutboundError::Proxy(
            "ssh: the handshake failed (no algorithm in common with the server)".to_string(),
        ),
        _ => OutboundError::Proxy("ssh: the handshake failed".to_string()),
    }
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

    async fn round_trip(outbound: &SshOutbound, target: &Target) -> Result<(), OutboundError> {
        let mut stream = outbound
            .connect_tcp(target, &ConnectOpts::default())
            .await?;
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        Ok(())
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

    /// Dials that were waiting on the session lock while a wrong-password
    /// handshake ran share that failed attempt instead of trying the same
    /// password again themselves: a server's fail2ban counts failed logins,
    /// so five queued dials each failing their own login would look like
    /// five. A dial that starts afterwards makes a new attempt.
    #[tokio::test]
    async fn concurrent_dials_share_one_failed_attempt_and_a_later_dial_retries() {
        let server = fake(FakeSshOpts {
            password: Some("pw".into()),
            ..Default::default()
        })
        .await;
        let ssh = Arc::new(outbound_to(
            server.addr,
            &spec(Some("hunter2"), None, vec![]),
            &[],
        ));
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
        let failed = round_trip(&ssh, &target).await.unwrap_err();
        assert_eq!(failed.to_string(), "ssh: authentication failed");
        assert_eq!(server.attempts(), 2);
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

    #[test]
    fn the_session_is_kept_alive_every_thirty_seconds() {
        let config = session_config();
        assert_eq!(
            (config.keepalive_interval, config.keepalive_max),
            (Some(Duration::from_secs(30)), 3)
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
}
