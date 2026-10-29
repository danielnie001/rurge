//! What a TUN does with a session its dialer would sniff (phase 4 M1 design
//! 5.3): once the handshake with the app is over the connection is handed to
//! `Dialer::serve` whole, with the TUN's stop and its holds, instead of dialing
//! and relaying here. The lease on a fake address is held until `serve`
//! returns.

use super::assert_nothing_left;
use crate::device::MemoryDevice;
use crate::testing::{AppStack, FakeDialer, Script, Upstream};
use crate::tests::support::{
    Extras, WITHIN, addr, eventually, hello, in_five_seconds, sock, tun_dialing, within,
};
use crate::{Ending, TunOptions};
use rurge_config::SessionInfo;
use rurge_dns::fakeip::{FakeIpPool, v4_of};
use rurge_inbound::{DialError, Dialed, Dialer, ServeOpts, SessionHandle};
use rurge_net::BoxFuture;
use rurge_net::connector::BoxedStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// What the wrapping dialer's `serve` does once it has written the options
/// down.
enum Then {
    /// What the default `serve` does: dial, relay, and cancel the session at
    /// the stop.
    Serve,
    /// Only wait for the stop.
    UntilStop,
    /// Wait until the test opens the gate, and return then, letting the
    /// connection go.
    UntilOpen(watch::Sender<bool>),
}

/// A dialer that sniffs every session: `dial` and `relay` are the fake
/// dialer's, and `serve` writes down what the TUN handed it before it does
/// what `Then` says.
struct Sniffing {
    inner: Arc<FakeDialer>,
    opts: Mutex<Vec<ServeOpts>>,
    then: Then,
}

impl Sniffing {
    fn new(inner: Arc<FakeDialer>, then: Then) -> Arc<Sniffing> {
        Arc::new(Sniffing {
            inner,
            opts: Mutex::new(Vec::new()),
            then,
        })
    }

    /// The options `serve` was given, in order.
    fn taken(&self) -> Vec<ServeOpts> {
        self.opts
            .lock()
            .expect("the options serve was given")
            .clone()
    }

    /// Opens the gate of `Then::UntilOpen`.
    fn open(&self) {
        match &self.then {
            Then::UntilOpen(gate) => {
                gate.send_replace(true);
            }
            Then::Serve | Then::UntilStop => panic!("this dialer's serve waits on no gate"),
        }
    }
}

impl Dialer for Sniffing {
    fn dial<'a>(&'a self, session: SessionInfo) -> BoxFuture<'a, Result<Dialed, DialError>> {
        self.inner.dial(session)
    }

    fn relay<'a>(
        &'a self,
        client: BoxedStream,
        upstream: BoxedStream,
        handle: Arc<SessionHandle>,
    ) -> BoxFuture<'a, ()> {
        self.inner.relay(client, upstream, handle)
    }

    fn sniffable(&self, _session: &SessionInfo) -> bool {
        true
    }

    fn serve<'a>(
        &'a self,
        session: SessionInfo,
        client: BoxedStream,
        opts: ServeOpts,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.opts
                .lock()
                .expect("the options serve was given")
                .push(opts.clone());
            match &self.then {
                // the fake dialer overrides neither method, so this is the
                // default `serve`, with the fake dialer's `dial` and `relay`
                Then::Serve => self.inner.serve(session, client, opts).await,
                Then::UntilStop => {
                    let stop = opts.stop.expect("the TUN hands serve a stop");
                    stop.cancelled().await;
                }
                Then::UntilOpen(gate) => {
                    let mut gate = gate.subscribe();
                    let _ = gate.wait_for(|open| *open).await;
                }
            }
        })
    }
}

/// A connection the dialer sniffs is served by it, and the app still gets its
/// answer: `serve` is called once, with the TUN's own stop and the holds the
/// TUN was started with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sniffable_connection_is_served_with_the_tuns_holds() {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    let fake = FakeDialer::new();
    let echo = Upstream::echo().await;
    let dst = addr("192.0.2.1:80");
    fake.script(dst, Script::Upstream(echo.addr()));
    let sniffing = Sniffing::new(fake.clone(), Then::Serve);
    let options = TunOptions {
        drop_hold: Duration::from_secs(7),
        linger: Duration::from_secs(9),
        ..TunOptions::default()
    };
    let tun = tun_dialing(
        Arc::new(device),
        sniffing.clone(),
        &Extras::default(),
        options,
    );
    let app = AppStack::start(end);
    let mut tcp = within("the connection to be made", app.connect(dst))
        .await
        .expect("connect");
    hello(&mut tcp).await;
    let taken = sniffing.taken();
    assert_eq!(taken.len(), 1, "serve was called once: {taken:?}");
    let opts = &taken[0];
    assert!(opts.stop.is_some(), "the TUN hands serve its own stop");
    assert_eq!(opts.drop_hold, Duration::from_secs(7));
    assert_eq!(opts.linger, Duration::from_secs(9));
    assert_eq!(fake.dialed().len(), 1, "serve dialed the session");
    tun.stop();
    let ending = in_five_seconds("the TUN to end", tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert_nothing_left(&tun);
}

/// The stop reaches a connection that `serve` is holding: the token it was
/// given is cancelled with the TUN, `serve` returns, and the TUN ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stop_reaches_a_served_connection() {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    let fake = FakeDialer::new();
    let sniffing = Sniffing::new(fake, Then::UntilStop);
    let tun = tun_dialing(
        Arc::new(device),
        sniffing.clone(),
        &Extras::default(),
        TunOptions::default(),
    );
    let app = AppStack::start(end);
    let _tcp = within(
        "the connection to be made",
        app.connect(addr("192.0.2.1:80")),
    )
    .await
    .expect("connect");
    eventually("the connection to be served", WITHIN, || {
        !sniffing.taken().is_empty()
    })
    .await;
    tun.stop();
    let ending = in_five_seconds("the TUN to end", tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert_nothing_left(&tun);
}

/// A connection to a fake address keeps the address leased while `serve`
/// holds it, and gives it back once `serve` returns: the lease is the TUN's
/// and is held to the end of the session, however long that is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_served_connection_keeps_its_fake_address_leased() {
    let (device, end) = MemoryDevice::pair(1500, 4096);
    let pool = Arc::new(FakeIpPool::new());
    let index = pool
        .assign("a.test", Instant::now())
        .expect("an address for the name");
    let fake = sock(v4_of(index), 80);
    let sniffing = Sniffing::new(FakeDialer::new(), Then::UntilOpen(watch::channel(false).0));
    let extras = Extras {
        pool: Some(pool.clone()),
        ..Extras::default()
    };
    let tun = tun_dialing(
        Arc::new(device),
        sniffing.clone(),
        &extras,
        TunOptions::default(),
    );
    let app = AppStack::start(end);
    let _tcp = within("the connection to be made", app.connect(fake))
        .await
        .expect("connect");
    eventually("the connection to be served", WITHIN, || {
        !sniffing.taken().is_empty()
    })
    .await;
    assert_eq!(
        pool.leased(),
        1,
        "the fake address is leased while serve holds the connection"
    );
    sniffing.open();
    eventually("the address to be given back", WITHIN, || {
        pool.leased() == 0
    })
    .await;
    tun.stop();
    let ending = in_five_seconds("the TUN to end", tun.join()).await;
    assert_eq!(ending, Ending::Stopped);
    assert_nothing_left(&tun);
}
