//! `FakeSsh`: a loopback SSH server for the tests (russh's own server).

use super::random_key;
use russh::keys::PublicKey;
use russh::keys::ssh_key::Algorithm;
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
    /// Refuse every `direct-tcpip` channel (`connect failed`).
    pub refuse_channels: bool,
    /// Offer only what Surge's manual requires: `curve25519-sha256` and
    /// `aes128-gcm@openssh.com`.
    pub surge_minimum: bool,
}

struct State {
    opts: FakeSshOpts,
    logins: AtomicUsize,
    sessions: Mutex<Vec<server::Handle>>,
}

/// Serves SSH on a loopback port: password and public-key logins, and
/// `direct-tcpip` channels bridged to the loopback address they name.
pub struct FakeSsh {
    pub addr: SocketAddr,
    /// Its host key.
    pub host_key: PublicKey,
    state: Arc<State>,
    task: JoinHandle<()>,
}

impl FakeSsh {
    pub async fn start(opts: FakeSshOpts) -> FakeSsh {
        let host = random_key(Algorithm::Ed25519);
        let host_key = host.public_key().clone();
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
            keys: vec![host],
            auth_rejection_time: Duration::ZERO,
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: None,
            preferred,
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(State {
            opts,
            logins: AtomicUsize::new(0),
            sessions: Mutex::new(Vec::new()),
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
                        state.sessions.lock().unwrap().push(running.handle());
                        let _ = running.await;
                    }
                });
            }
        });
        FakeSsh {
            addr,
            host_key,
            state,
            task,
        }
    }

    /// Logins that succeeded so far: one per session.
    pub fn logins(&self) -> usize {
        self.state.logins.load(Ordering::SeqCst)
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
        let opts = &self.state.opts;
        Ok(self.verdict(user == opts.user && opts.password.as_deref() == Some(password)))
    }

    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
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
        let target = u16::try_from(port_to_connect)
            .ok()
            .filter(|_| !self.state.opts.refuse_channels);
        let tcp = match target {
            Some(port) => TcpStream::connect((host_to_connect, port)).await.ok(),
            None => None,
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
