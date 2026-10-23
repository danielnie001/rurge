//! The resolver of a test's DNS responder: what `[Host]` maps names to, and the
//! upstreams' replies to the queries the responder forwards, scripted by the
//! test, with nothing in it that sends a query anywhere (a forwarded query is
//! written down and answered from memory).

use crate::TunDns;
use rurge_dns::message::wire_question;
use rurge_net::BoxFuture;
use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::watch;

/// What a [`FakeDns`] answers a forwarded query with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardScript {
    /// At once, the query itself as a reply: QR and RA set, `pad` zero bytes
    /// after it to make it as long as the test wants. Whatever else the query
    /// said stays, its response code too.
    Echo { pad: usize },
    /// At once, with an error.
    Fail,
    /// Not until [`FakeDns::release`]; then as `Echo { pad: 0 }`.
    Hold,
}

#[derive(Default)]
struct Book {
    mapped: HashMap<String, Vec<IpAddr>>,
    scripts: HashMap<String, ForwardScript>,
    /// Every query `forward` was asked for, in order, scripted or not.
    forwarded: Vec<Vec<u8>>,
}

/// The `TunDns` of a test: `mapped` knows the names a test gave it and no
/// others, and `forward` does what the test scripted for the query's name,
/// `Echo { pad: 0 }` unless it scripted something else.
pub struct FakeDns {
    book: Mutex<Book>,
    /// Becomes true when the test releases the queries it held.
    released: watch::Sender<bool>,
}

impl FakeDns {
    pub fn new() -> Arc<FakeDns> {
        Arc::new(FakeDns {
            book: Mutex::default(),
            released: watch::channel(false).0,
        })
    }

    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().expect("the fake DNS's book")
    }

    /// `[Host]` maps `name` to `addrs`, in this order, from now on; what an
    /// earlier call gave it is replaced.
    pub fn map(&self, name: &str, addrs: &[IpAddr]) {
        self.book()
            .mapped
            .insert(name.to_ascii_lowercase(), addrs.to_vec());
    }

    /// A query for `name` (as the query writes it, in any case) gets `script`
    /// from now on.
    pub fn script(&self, name: &str, script: ForwardScript) {
        self.book()
            .scripts
            .insert(name.to_ascii_lowercase(), script);
    }

    /// Every query the responder forwarded, as it came, in order: whatever
    /// became of it.
    pub fn forwarded(&self) -> Vec<Vec<u8>> {
        self.book().forwarded.clone()
    }

    /// From now on every query that is held answers as `Echo { pad: 0 }`, and
    /// so do those that were held already.
    pub fn release(&self) {
        self.released.send_replace(true);
    }
}

/// `query` as a reply: QR and RA set, `pad` zero bytes at the end.
fn echo(query: &[u8], pad: usize) -> Vec<u8> {
    let mut reply = query.to_vec();
    reply.resize(query.len() + pad, 0);
    if reply.len() >= 12 {
        reply[2] |= 0x80;
        reply[3] |= 0x80;
    }
    reply
}

impl TunDns for FakeDns {
    fn mapped(&self, name: &str) -> Option<Vec<IpAddr>> {
        self.book().mapped.get(&name.to_ascii_lowercase()).cloned()
    }

    fn forward<'a>(&'a self, query: &'a [u8]) -> BoxFuture<'a, io::Result<Vec<u8>>> {
        let script = {
            let mut book = self.book();
            book.forwarded.push(query.to_vec());
            wire_question(query)
                .and_then(|(name, _, _)| book.scripts.get(&name).copied())
                .unwrap_or(ForwardScript::Echo { pad: 0 })
        };
        Box::pin(async move {
            match script {
                ForwardScript::Echo { pad } => Ok(echo(query, pad)),
                ForwardScript::Fail => Err(io::Error::other("scripted failure")),
                ForwardScript::Hold => {
                    let mut gate = self.released.subscribe();
                    if gate.wait_for(|released| *released).await.is_err() {
                        return Err(io::Error::other("the fake DNS is gone"));
                    }
                    Ok(echo(query, 0))
                }
            }
        })
    }
}
