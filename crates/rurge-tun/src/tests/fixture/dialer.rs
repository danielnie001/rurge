//! The fake `Dialer` and the loopback servers it relays to: what it answers,
//! how it records the end of each relay, and the far ends it can be scripted
//! with.

use crate::testing::{FakeDialer, Script, Upstream};
use crate::tests::support::{WITHIN, addr, eventually, read_pattern, within, write_pattern};
use rurge_config::{HostName, ListenerKind, SessionInfo};
use rurge_inbound::{DialError, Dialed, Dialer, FailKind, SessionHandle, SessionOutcome};
use rurge_proto::RejectKind;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

/// The session of an app connecting to `dst` through the adapter.
fn session(dst: SocketAddr) -> SessionInfo {
    SessionInfo {
        src: addr("198.18.0.1:49152"),
        listener: ListenerKind::Tun,
        ..SessionInfo::tcp(HostName::Ip(dst.ip()), dst.port())
    }
}

fn expect_dialed(result: Result<Dialed, DialError>) -> Dialed {
    match result {
        Ok(dialed) => dialed,
        Err(DialError::Reject { kind, .. }) => panic!("rejected: {kind:?}"),
        Err(DialError::Failed { message, .. }) => panic!("failed: {message}"),
    }
}

/// A session dialed and relayed in a task of its own, the client side of the
/// relay being one end of a pipe: the other end is returned.
async fn relayed(
    dialer: &Arc<FakeDialer>,
    to: SocketAddr,
) -> (
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<()>,
    Arc<SessionHandle>,
) {
    let dialed = expect_dialed(within("the session to be dialed", dialer.dial(session(to))).await);
    let (app, tun_side) = tokio::io::duplex(64 * 1024);
    let handle = dialed.handle.clone();
    let relay = tokio::spawn({
        let dialer = dialer.clone();
        async move {
            dialer
                .relay(Box::new(tun_side), dialed.stream, dialed.handle)
                .await
        }
    });
    (app, relay, handle)
}

#[tokio::test]
async fn the_fake_dialer_answers_what_it_was_told() {
    let echo = Upstream::echo().await;
    let dialer = FakeDialer::new();
    let to_echo = addr("192.0.2.10:80");
    let to_reject = addr("192.0.2.11:80");
    let to_fail = addr("192.0.2.12:80");
    let unscripted = addr("192.0.2.13:80");
    dialer.script(to_echo, Script::Upstream(echo.addr()));
    dialer.script(to_reject, Script::Reject(RejectKind::TinyGif));
    dialer.script(to_fail, Script::Fail);

    // dialed, and relayed to the end
    let (mut app, relay, handle) = relayed(&dialer, to_echo).await;
    assert_eq!(handle.id(), 1);
    app.write_all(b"ping").await.unwrap();
    let mut pong = [0u8; 4];
    within("the echo", app.read_exact(&mut pong)).await.unwrap();
    assert_eq!(&pong, b"ping");
    app.shutdown().await.unwrap();
    within("the relay to end", relay).await.unwrap();
    assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
    assert_eq!(dialer.relay_ends(), [Ok(())]);

    // rejected: the handle is finished before the answer
    let Some(DialError::Reject { kind, handle, .. }) =
        within("the rejection", dialer.dial(session(to_reject)))
            .await
            .err()
    else {
        panic!("not a rejection");
    };
    assert_eq!(kind, RejectKind::TinyGif);
    assert_eq!(handle.id(), 2);
    assert_eq!(
        handle.outcome(),
        Some(SessionOutcome::Rejected(RejectKind::TinyGif))
    );

    // a scripted failure and a destination nobody wrote anything for
    for (to, text, id) in [
        (to_fail, "scripted failure", 3),
        (unscripted, "not scripted", 4),
    ] {
        let Some(DialError::Failed {
            kind,
            message,
            handle,
            ..
        }) = within("the failure", dialer.dial(session(to))).await.err()
        else {
            panic!("not a failure");
        };
        assert_eq!(kind, FailKind::Connect);
        assert_eq!(message, text);
        assert_eq!(handle.id(), id);
        assert_eq!(handle.outcome(), Some(SessionOutcome::Failed(text.into())));
    }

    // every session, in order
    let dialed: Vec<(IpAddr, u16)> = dialer
        .dialed()
        .into_iter()
        .map(|s| match s.dst_host {
            HostName::Ip(ip) => (ip, s.dst_port),
            HostName::Domain(name) => panic!("a name: {name}"),
        })
        .collect();
    let expected: Vec<_> = [to_echo, to_reject, to_fail, unscripted]
        .iter()
        .map(|a| (a.ip(), a.port()))
        .collect();
    assert_eq!(dialed, expected);
}

/// A stream whose reads fail.
struct Broken;

impl AsyncRead for Broken {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "the app went away",
        )))
    }
}

impl AsyncWrite for Broken {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_failed_relay_is_recorded_with_its_text() {
    let dialer = FakeDialer::new();
    let handle = SessionHandle::new(77, session(addr("192.0.2.14:80")));
    // what `relay_ends` holds when the handle is told it is over
    let ends_then = Arc::new(AtomicUsize::new(usize::MAX));
    handle.on_finish({
        let (dialer, ends_then) = (dialer.clone(), ends_then.clone());
        move |_, _| ends_then.store(dialer.relay_ends().len(), Ordering::SeqCst)
    });
    let (_far, near) = tokio::io::duplex(64);
    within(
        "the relay to end",
        dialer.relay(Box::new(Broken), Box::new(near), handle.clone()),
    )
    .await;
    assert_eq!(dialer.relay_ends(), [Err("the app went away".to_string())]);
    assert_eq!(
        handle.outcome(),
        Some(SessionOutcome::Failed("the app went away".into()))
    );
    assert_eq!(
        ends_then.load(Ordering::SeqCst),
        1,
        "the end is recorded before the handle finishes"
    );
}

/// The fake dialer's relay ends when the session's token is cancelled, as the
/// engine's does, with an app that says nothing and an upstream that is held:
/// it is recorded as cancelled, and the handle finishes completed.
#[tokio::test]
async fn a_cancelled_relay_ends_and_is_recorded_as_cancelled() {
    let dialer = FakeDialer::new();
    let to = addr("192.0.2.17:443");
    dialer.script(to, Script::Hold { buffer: 1024 });
    let (_app, relay, handle) = relayed(&dialer, to).await;
    handle.token().cancel();
    within("the relay to end", relay).await.unwrap();
    assert_eq!(dialer.relay_ends(), [Err("cancelled".to_string())]);
    assert_eq!(handle.outcome(), Some(SessionOutcome::Completed));
}

#[tokio::test]
async fn a_memory_upstream_sends_what_it_was_told_and_counts_what_it_gets() {
    let dialer = FakeDialer::new();
    let to = addr("192.0.2.15:80");
    dialer.script(
        to,
        Script::Memory {
            buffer: 1024,
            send: 5000,
        },
    );
    let (mut app, relay, _handle) = relayed(&dialer, to).await;
    within("what the far end sends", read_pattern(&mut app, 5000))
        .await
        .unwrap();
    write_pattern(&mut app, 3000).await.unwrap();
    eventually("the far end to read it", WITHIN, || {
        dialer.received(to) == 3000
    })
    .await;
    app.shutdown().await.unwrap();
    within("the relay to end", relay).await.unwrap();
    assert_eq!(dialer.relay_ends(), [Ok(())]);
    assert_eq!(dialer.received(to), 3000);
}

/// A wait for what the far ends have read is woken by the reading, and ends
/// when its time is up.
#[tokio::test]
async fn waiting_for_what_a_far_end_has_read_is_woken_and_bounded() {
    let dialer = FakeDialer::new();
    let to = addr("192.0.2.16:80");
    dialer.script(
        to,
        Script::Memory {
            buffer: 1024,
            send: 0,
        },
    );
    let (mut app, relay, _handle) = relayed(&dialer, to).await;
    // begun before the far end reads the bytes, so only the reading can end it
    let waiting = tokio::spawn({
        let dialer = dialer.clone();
        async move { dialer.wait_for(to, 3000, WITHIN).await }
    });
    write_pattern(&mut app, 3000).await.unwrap();
    assert!(
        within("the wait to end", waiting).await.unwrap(),
        "woken by the bytes"
    );
    // waits for: the time to run out, as a 3001st byte never comes; margin:
    // none needed, whenever it ends the answer is false
    assert!(!dialer.wait_for(to, 3001, Duration::from_millis(50)).await);
    // and an answer for what is already there does not wait
    assert!(dialer.wait_for(to, 3000, Duration::ZERO).await);
    app.shutdown().await.unwrap();
    within("the relay to end", relay).await.unwrap();
}

#[tokio::test]
async fn a_held_upstream_reads_only_once_released() {
    let dialer = FakeDialer::new();
    let to = addr("192.0.2.20:443");
    dialer.script(to, Script::Hold { buffer: 1024 });
    let (mut app, relay, _handle) = relayed(&dialer, to).await;
    app.write_all(&[0xaa; 4096]).await.unwrap();
    // waits for: the far end to read something before it was released;
    // margin: a far end that reads takes the first 1024 bytes within microseconds
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(dialer.received(to), 0);
    dialer.release(to);
    eventually(
        "the far end to read all of it",
        Duration::from_secs(5),
        || dialer.received(to) == 4096,
    )
    .await;
    app.shutdown().await.unwrap();
    within("the relay to end", relay).await.unwrap();
}

#[tokio::test]
#[should_panic(expected = "loopback")]
async fn the_fake_dialer_dials_only_the_loopback() {
    FakeDialer::new().script(addr("192.0.2.9:80"), Script::Upstream(addr("192.0.2.1:80")));
}

#[tokio::test]
async fn the_upstream_servers_do_what_they_say() {
    // the echo sends back what it gets, and closes when the client has
    let echo = Upstream::echo().await;
    let mut stream = TcpStream::connect(echo.addr()).await.unwrap();
    stream.write_all(b"echo").await.unwrap();
    let mut back = [0u8; 4];
    within("the echo", stream.read_exact(&mut back))
        .await
        .unwrap();
    assert_eq!(&back, b"echo");
    stream.shutdown().await.unwrap();
    assert_eq!(
        within("the echo's end", stream.read(&mut back))
            .await
            .unwrap(),
        0
    );

    // the sink counts
    let sink = Upstream::sink().await;
    let mut stream = TcpStream::connect(sink.addr()).await.unwrap();
    within(
        "the pattern to be written",
        write_pattern(&mut stream, 100_000),
    )
    .await
    .unwrap();
    assert!(sink.wait_for(100_000, WITHIN).await);
    assert_eq!(sink.received(), 100_000);
    // waits for: `wait_for` to give up after 1 ms; margin: nothing can come
    // to a sink nobody is connected to, so there is no race to lose
    assert!(
        !Upstream::sink()
            .await
            .wait_for(1, Duration::from_millis(1))
            .await,
        "nothing came, and the wait is over"
    );

    // the source writes the pattern and then closes
    let source = Upstream::source(300_000).await;
    let mut stream = TcpStream::connect(source.addr()).await.unwrap();
    within("the pattern", read_pattern(&mut stream, 300_000))
        .await
        .unwrap();
    let mut rest = Vec::new();
    within("the end", stream.read_to_end(&mut rest))
        .await
        .unwrap();
    assert!(rest.is_empty());
    assert_eq!(source.sent(), 300_000);

    // dropping a server ends its connections and its listener
    let kept = Upstream::echo().await;
    let to = kept.addr();
    let mut stream = TcpStream::connect(to).await.unwrap();
    drop(kept);
    let end = within("the connection to end", stream.read(&mut back)).await;
    assert!(matches!(end, Ok(0) | Err(_)), "{end:?}");
    within("the listener to go", async {
        while TcpStream::connect(to).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
}

/// A session is dialed to where the app connected to: its `local` end when it
/// has one, which for a connection to a fake address is that address and not
/// the name the session is for; the address it names when it has none.
#[tokio::test]
async fn the_fake_dialer_finds_a_session_by_where_the_app_connected_to() {
    let echo = Upstream::echo().await;
    let dialer = FakeDialer::new();
    let (fake, elsewhere) = (addr("198.18.1.1:80"), addr("192.0.2.9:80"));
    dialer.script(fake, Script::Upstream(echo.addr()));
    dialer.script(elsewhere, Script::Fail);
    // the session of a name behind a fake address
    let mut named = session(fake);
    named.dst_host = HostName::Domain("a.test".to_owned());
    named.local = Some(fake);
    expect_dialed(within("the dial", dialer.dial(named)).await);
    // `local` comes first: the address `dst_host` names is another one
    let mut moved = session(elsewhere);
    moved.local = Some(fake);
    expect_dialed(within("the dial", dialer.dial(moved)).await);
    // with none the address it names is the one
    let result = within("the dial", dialer.dial(session(elsewhere))).await;
    assert!(matches!(result, Err(DialError::Failed { .. })));
    // and a name with no `local` has nothing to be found by
    let mut nameless = session(fake);
    nameless.dst_host = HostName::Domain("a.test".to_owned());
    let result = within("the dial", dialer.dial(nameless)).await;
    let Err(DialError::Failed { message, .. }) = result else {
        panic!("a session with nothing to find it by is not dialed");
    };
    assert_eq!(message, "not scripted");
}
