//! The hooks and the pinger of a test: what a TUN is given besides its device
//! and its `Dialer`, scripted by the test, with nothing in them that leaves
//! the process (an echo request goes to a [`FakePinger`], which sends it
//! nowhere).

use crate::{Hooks, Pinger, TunSettings};
use rurge_config::{HostName, PreMatchLayer, SessionInfo};
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
use rurge_proto::RejectKind;
use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};

/// `Hooks` whose settings the test sets, as the engine sets them with each
/// generation it publishes, and whose pre-matching rules the test scripts.
pub struct FakeHooks {
    settings: Mutex<Arc<TunSettings>>,
    pre_match: Mutex<PreMatches>,
}

#[derive(Default)]
struct PreMatches {
    /// The kind each destination at each layer is refused with.
    scripts: Vec<(PreMatchLayer, HostName, RejectKind)>,
    /// Every session `pre_match` was asked about, in order, scripted or not.
    asked: Vec<(PreMatchLayer, SessionInfo)>,
}

impl FakeHooks {
    pub fn new(settings: TunSettings) -> Arc<FakeHooks> {
        Arc::new(FakeHooks {
            settings: Mutex::new(Arc::new(settings)),
            pre_match: Mutex::default(),
        })
    }

    /// From now on `settings` are the generation in use: what a TUN asks next
    /// gets them.
    pub fn set(&self, settings: TunSettings) {
        *self.settings.lock().expect("the fake hooks' settings") = Arc::new(settings);
    }

    fn pre_matches(&self) -> MutexGuard<'_, PreMatches> {
        self.pre_match.lock().expect("the fake hooks' pre-matching")
    }

    /// A session at `layer` whose destination is `dst` is refused with `kind`
    /// from now on; what an earlier call gave the same layer and destination is
    /// replaced. A session that nothing was scripted for is not refused.
    pub fn script_pre_match(&self, layer: PreMatchLayer, dst: HostName, kind: RejectKind) {
        let mut book = self.pre_matches();
        book.scripts.retain(|(l, d, _)| !(*l == layer && *d == dst));
        book.scripts.push((layer, dst, kind));
    }

    /// Every session `pre_match` was asked about, with the layer, in order:
    /// whatever it answered.
    pub fn pre_matches_asked(&self) -> Vec<(PreMatchLayer, SessionInfo)> {
        self.pre_matches().asked.clone()
    }
}

impl Hooks for FakeHooks {
    fn settings(&self) -> Arc<TunSettings> {
        self.settings
            .lock()
            .expect("the fake hooks' settings")
            .clone()
    }

    fn pre_match(&self, layer: PreMatchLayer, session: &SessionInfo) -> Option<RejectKind> {
        let mut book = self.pre_matches();
        book.asked.push((layer, session.clone()));
        book.scripts
            .iter()
            .find(|(l, d, _)| *l == layer && *d == session.dst_host)
            .map(|(_, _, kind)| *kind)
    }
}

/// A resolver that knows the names a test gave it and no others. A name is
/// found as it is written: the pool gives names in lower case.
#[derive(Default)]
pub struct FakeResolve {
    names: Mutex<HashMap<String, Vec<IpAddr>>>,
}

impl FakeResolve {
    pub fn new() -> Arc<FakeResolve> {
        Arc::new(FakeResolve::default())
    }

    /// `name` resolves to `addrs`, in this order, from now on; what an earlier
    /// call gave it is replaced.
    pub fn insert(&self, name: &str, addrs: Vec<IpAddr>) {
        self.names
            .lock()
            .expect("the fake resolver's names")
            .insert(name.to_owned(), addrs);
    }
}

impl Resolve for FakeResolve {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        let found = self
            .names
            .lock()
            .expect("the fake resolver's names")
            .get(host)
            .cloned();
        Box::pin(async move {
            found.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "the fake resolver has no such name",
                )
            })
        })
    }
}

/// What a [`FakePinger`] does with the echo requests to a destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingScript {
    /// Answers at once with the payload it was sent.
    Echo,
    /// Answers at once with these bytes, whatever it was sent.
    Reply(&'static [u8]),
    /// Fails at once.
    Fail,
    /// Never answers.
    Silent,
}

#[derive(Default)]
struct Book {
    scripts: HashMap<IpAddr, PingScript>,
    pinged: Vec<(IpAddr, Vec<u8>)>,
}

/// A `Pinger` that sends nothing anywhere: what an echo request to a
/// destination gets is what the test scripted for it (`Fail` unless it did),
/// and every request is written down.
#[derive(Default)]
pub struct FakePinger {
    book: Mutex<Book>,
}

impl FakePinger {
    pub fn new() -> Arc<FakePinger> {
        Arc::new(FakePinger::default())
    }

    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().expect("the fake pinger's book")
    }

    /// Echo requests to `dst` get `script` from now on.
    pub fn script(&self, dst: IpAddr, script: PingScript) {
        self.book().scripts.insert(dst, script);
    }

    /// Every echo request the pinger was asked for, in order: where it was to
    /// go and the payload, whatever became of it.
    pub fn pinged(&self) -> Vec<(IpAddr, Vec<u8>)> {
        self.book().pinged.clone()
    }
}

impl Pinger for FakePinger {
    fn echo<'a>(&'a self, dst: IpAddr, payload: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>> {
        let script = {
            let mut book = self.book();
            book.pinged.push((dst, payload.to_vec()));
            book.scripts.get(&dst).copied().unwrap_or(PingScript::Fail)
        };
        Box::pin(async move {
            match script {
                PingScript::Echo => Ok(payload.to_vec()),
                PingScript::Reply(bytes) => Ok(bytes.to_vec()),
                PingScript::Fail => Err(io::Error::new(
                    io::ErrorKind::HostUnreachable,
                    "scripted failure",
                )),
                PingScript::Silent => std::future::pending().await,
            }
        })
    }
}
