//! `FakeSsh`: a loopback SSH server for the tests (russh's own server).

use super::random_key;
use russh::keys::ssh_key::Algorithm;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{self, Auth, ChannelOpenHandle, Msg, run_stream};
use russh::{Channel, ChannelOpenFailure, Disconnect, Preferred, cipher, kex};
use std::borrow::Cow;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// Who may log in, and how the server behaves.
#[derive(Clone, Debug, Default)]
pub struct FakeSshOpts {
    pub user: String,
    pub password: Option<String>,
    /// Public keys that may log in.
    pub keys: Vec<PublicKey>,
    /// Host keys besides the Ed25519 one.
    pub extra_host_keys: Vec<PrivateKey>,
    /// Refuse every `direct-tcpip` channel (`connect failed`).
    pub refuse_channels: bool,
    /// Offer only what Surge's manual requires: `curve25519-sha256` and
    /// `aes128-gcm@openssh.com`.
    pub surge_minimum: bool,
    /// Connect every channel here instead of where it asks to go.
    pub connect_to: Option<SocketAddr>,
    /// End the connection when a password arrives, without answering.
    pub hang_up_on_password: bool,
}

struct State {
    opts: FakeSshOpts,
    /// The password that logs in: `opts.password` until `set_password`.
    password: Mutex<Option<String>>,
    logins: AtomicUsize,
    /// Sessions whose handshake finished and that have not ended.
    live: AtomicUsize,
    /// Every `auth_password` or `auth_publickey` call, successful or not.
    attempts: AtomicUsize,
    sessions: Mutex<Vec<server::Handle>>,
    /// Where the channels asked to go, in order.
    requested: Mutex<Vec<(String, u32)>>,
}

/// Serves SSH on a loopback port: password and public-key logins, and
/// `direct-tcpip` channels bridged to the loopback address they name.
pub struct FakeSsh {
    pub addr: SocketAddr,
    /// Its Ed25519 host key.
    pub host_key: PublicKey,
    /// All its host keys, the Ed25519 one first.
    pub host_keys: Vec<PublicKey>,
    state: Arc<State>,
    task: JoinHandle<()>,
}

impl FakeSsh {
    pub async fn start(opts: FakeSshOpts) -> FakeSsh {
        let host = random_key(Algorithm::Ed25519);
        let host_key = host.public_key().clone();
        let mut keys = vec![host];
        keys.extend(opts.extra_host_keys.iter().cloned());
        let host_keys = keys.iter().map(|k| k.public_key().clone()).collect();
        let mut preferred = Preferred::default();
        if opts.surge_minimum {
            preferred.kex = Cow::Owned(vec![
                kex::CURVE25519,
                kex::EXTENSION_SUPPORT_AS_SERVER,
                kex::EXTENSION_OPENSSH_STRICT_KEX_AS_SERVER,
            ]);
            preferred.cipher = Cow::Owned(vec![cipher::AES_128_GCM]);
        }
        let config = Arc::new(server::Config {
            keys,
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: None,
            preferred,
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(State {
            password: Mutex::new(opts.password.clone()),
            opts,
            logins: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
            attempts: AtomicUsize::new(0),
            sessions: Mutex::new(Vec::new()),
            requested: Mutex::new(Vec::new()),
        });
        let accepting = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let peer = Peer {
                    state: accepting.clone(),
                };
                let config = config.clone();
                let state = accepting.clone();
                tokio::spawn(async move {
                    if let Ok(running) = run_stream(config, stream, peer).await {
                        state.live.fetch_add(1, Ordering::SeqCst);
                        state.sessions.lock().unwrap().push(running.handle());
                        let _ = running.await;
                        state.live.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });
        FakeSsh {
            addr,
            host_key,
            host_keys,
            state,
            task,
        }
    }

    /// Logins that succeeded so far: one per session.
    pub fn logins(&self) -> usize {
        self.state.logins.load(Ordering::SeqCst)
    }

    /// Where the channels asked to go: host and port, as sent.
    pub fn requested(&self) -> Vec<(String, u32)> {
        self.state.requested.lock().unwrap().clone()
    }

    /// Sessions that are up now.
    pub fn live_sessions(&self) -> usize {
        self.state.live.load(Ordering::SeqCst)
    }

    /// Authentication attempts so far (password or public key), successful
    /// or not: one per login try, regardless of how many connections it
    /// took.
    pub fn attempts(&self) -> usize {
        self.state.attempts.load(Ordering::SeqCst)
    }

    /// The password that logs in from now on.
    pub fn set_password(&self, password: &str) {
        *self.state.password.lock().unwrap() = Some(password.to_string());
    }

    /// Opens one channel of each kind a server can open toward its client,
    /// on the newest session, and says how the client answered each: the
    /// reason it refused it, or `None` if it took it.
    pub async fn open_channels_toward_client(&self) -> Vec<Option<ChannelOpenFailure>> {
        let session = self.state.sessions.lock().unwrap().last().cloned();
        let session = session.expect("a session");
        let answer = |opened: Result<Channel<Msg>, russh::Error>| match opened {
            Ok(_) => None,
            Err(russh::Error::ChannelOpenFailure(reason)) => Some(reason),
            Err(e) => panic!("the channel was not answered: {e}"),
        };
        vec![
            answer(session.channel_open_session().await),
            answer(
                session
                    .channel_open_direct_tcpip("127.0.0.1", 9, "127.0.0.1", 9)
                    .await,
            ),
            answer(session.channel_open_direct_streamlocal("/s").await),
            answer(
                session
                    .channel_open_forwarded_tcpip("127.0.0.1", 9, "127.0.0.1", 9)
                    .await,
            ),
            answer(session.channel_open_forwarded_streamlocal("/s").await),
            answer(session.channel_open_x11("127.0.0.1", 9).await),
            answer(session.channel_open_agent().await),
        ]
    }

    /// Ends every session, as a server restart would.
    pub async fn end_sessions(&self) {
        let sessions = std::mem::take(&mut *self.state.sessions.lock().unwrap());
        for session in sessions {
            let _ = session
                .disconnect(Disconnect::ByApplication, String::new(), String::new())
                .await;
        }
    }
}

impl Drop for FakeSsh {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Peer {
    state: Arc<State>,
}

impl Peer {
    fn verdict(&self, ok: bool) -> Auth {
        if ok {
            self.state.logins.fetch_add(1, Ordering::SeqCst);
            Auth::Accept
        } else {
            Auth::reject()
        }
    }
}

impl server::Handler for Peer {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        self.state.attempts.fetch_add(1, Ordering::SeqCst);
        if self.state.opts.hang_up_on_password {
            return Err(russh::Error::Disconnect);
        }
        let known = self.state.password.lock().unwrap().as_deref() == Some(password);
        Ok(self.verdict(user == self.state.opts.user && known))
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        self.state.attempts.fetch_add(1, Ordering::SeqCst);
        let opts = &self.state.opts;
        let known = opts.keys.iter().any(|k| k.key_data() == key.key_data());
        Ok(self.verdict(user == opts.user && known))
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.state
            .requested
            .lock()
            .unwrap()
            .push((host_to_connect.to_string(), port_to_connect));
        let target = u16::try_from(port_to_connect)
            .ok()
            .filter(|_| !self.state.opts.refuse_channels);
        let tcp = match (target, self.state.opts.connect_to) {
            (None, _) => None,
            (Some(_), Some(addr)) => TcpStream::connect(addr).await.ok(),
            (Some(port), None) => TcpStream::connect((host_to_connect, port)).await.ok(),
        };
        match tcp {
            Some(mut tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut channel = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut channel, &mut tcp).await;
                });
            }
            None => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }
}
