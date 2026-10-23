//! The fake `Dialer`'s UDP: the datagrams of an association are taken one by
//! one and answered as the script of their target says.

use crate::testing::{FakeDialer, UdpScript};
use crate::tests::support::{WITHIN, addr, eventually, within};
use rurge_config::{HostName, ListenerKind, SessionInfo};
use rurge_inbound::{Dialer, UdpAdmission, UdpClient};
use rurge_net::BoxFuture;
use rurge_net::connector::Target;
use rurge_proto::RejectKind;
use std::io;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// What a client is sent: the bytes, where they say they came from and the flow
/// they say they answer.
type Sent = (Vec<u8>, Target, Option<Target>);

/// A client that keeps what it is sent and what it is told was refused, and
/// hands out the datagrams a test queues for it.
struct Hears {
    queued: tokio::sync::Mutex<mpsc::UnboundedReceiver<(Vec<u8>, Target)>>,
    sent: std::sync::Mutex<Vec<Sent>>,
    refused: std::sync::Mutex<Vec<(Target, RejectKind)>>,
}

impl UdpClient for Hears {
    fn recv<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let (bytes, to) = self.queued.lock().await.recv().await.ok_or_else(|| {
                io::Error::new(io::ErrorKind::BrokenPipe, "the test has no more to send")
            })?;
            buf[..bytes.len()].copy_from_slice(&bytes);
            Ok((bytes.len(), to))
        })
    }

    fn send<'a>(
        &'a self,
        payload: &'a [u8],
        from: &'a Target,
        answers: Option<&'a Target>,
    ) -> BoxFuture<'a, io::Result<()>> {
        self.sent
            .lock()
            .unwrap()
            .push((payload.to_vec(), from.clone(), answers.cloned()));
        Box::pin(std::future::ready(Ok(())))
    }

    fn refused(&self, target: &Target, kind: RejectKind) {
        self.refused.lock().unwrap().push((target.clone(), kind));
    }
}

/// The fake dialer takes the datagrams of an association one by one, writes
/// each down, and does what the script of its target says; it can be held, can
/// refuse an association, and sends to an app through the association that
/// is its own.
#[tokio::test]
async fn the_fake_dialer_serves_udp_as_it_was_scripted() {
    let dialer = FakeDialer::new();
    let (queue, queued) = mpsc::unbounded_channel();
    let client = Arc::new(Hears {
        queued: tokio::sync::Mutex::new(queued),
        sent: Default::default(),
        refused: Default::default(),
    });
    let app = addr("198.18.0.1:5000");
    let template = SessionInfo {
        src: app,
        listener: ListenerKind::Tun,
        ..SessionInfo::udp(HostName::Ip(addr("198.18.0.253:1").ip()), 0)
    };
    assert_eq!(dialer.admit_udp(), UdpAdmission::Accepted, "to begin with");
    for admission in [
        UdpAdmission::Busy,
        UdpAdmission::NotSupported,
        UdpAdmission::Accepted,
    ] {
        dialer.admit(admission);
        assert_eq!(dialer.admit_udp(), admission);
    }
    let target = |last: u8| Target::new(HostName::Ip(Ipv4Addr::new(192, 0, 2, last).into()), 7);
    let (echo, reply, third_party, refuse, plain) =
        (target(1), target(2), target(3), target(4), target(5));
    dialer.script_udp(echo.clone(), UdpScript::Echo);
    dialer.script_udp(reply.clone(), UdpScript::Reply(b"fixed".to_vec()));
    dialer.script_udp(third_party.clone(), UdpScript::EchoFrom(target(99)));
    dialer.script_udp(refuse.clone(), UdpScript::Refuse(RejectKind::TinyGif));
    let closed = CancellationToken::new();
    let serving = tokio::spawn({
        let (dialer, closed) = (dialer.clone(), closed.clone());
        let client: Arc<dyn UdpClient> = client.clone();
        let template = template.clone();
        async move { dialer.associate(client, template, closed).await }
    });
    for (bytes, to) in [
        (&b"one"[..], &echo),
        (b"two", &reply),
        (b"three", &third_party),
        (b"four", &refuse),
        (b"five", &plain),
    ] {
        queue.send((bytes.to_vec(), to.clone())).unwrap();
    }
    eventually("the datagrams to be taken", WITHIN, || {
        dialer.udp_sent().len() == 5
    })
    .await;
    assert_eq!(
        dialer
            .udp_sent()
            .into_iter()
            .map(|(to, _)| to)
            .collect::<Vec<_>>(),
        [&echo, &reply, &third_party, &refuse, &plain].map(Clone::clone)
    );
    assert_eq!(
        *client.sent.lock().unwrap(),
        [
            (b"one".to_vec(), echo.clone(), Some(echo.clone())),
            (b"fixed".to_vec(), reply.clone(), Some(reply)),
            (b"three".to_vec(), target(99), None),
        ]
    );
    assert_eq!(
        *client.refused.lock().unwrap(),
        [(refuse, RejectKind::TinyGif)]
    );
    assert_eq!(dialer.associations(), [template]);
    assert_eq!(dialer.associations_open(), 1);

    // held, it asks for nothing; released, it takes what waits
    dialer.hold_udp();
    queue.send((b"six".to_vec(), plain.clone())).unwrap();
    // the one fixed wait of this test, for what must not happen: the dialer
    // taking a datagram while it is held. It would take it at once, so 100 ms
    // is a wide margin
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(dialer.udp_sent().len(), 5);
    dialer.release_udp();
    eventually("the held datagram to be taken", WITHIN, || {
        dialer.udp_sent().len() == 6
    })
    .await;

    // to the app, through its association; to an app with none, an error
    dialer
        .send_to_app(
            app,
            third_party.clone(),
            Some(echo.clone()),
            b"push".to_vec(),
        )
        .await
        .expect("sent");
    assert_eq!(
        client.sent.lock().unwrap().last(),
        Some(&(b"push".to_vec(), third_party, Some(echo.clone())))
    );
    let error = dialer
        .send_to_app(addr("198.18.0.1:5001"), echo, None, Vec::new())
        .await
        .expect_err("no association of that app");
    assert_eq!(error.kind(), io::ErrorKind::NotFound);

    closed.cancel();
    within("the association to end", serving)
        .await
        .expect("the association's task");
    assert_eq!(dialer.associations_open(), 0);
}
