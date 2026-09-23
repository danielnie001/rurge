//! A Shadow TLS server on a loopback port (v2 or v3) and the camouflage site
//! it relays handshakes to. The server side is written from the protocol
//! documents and the reference server, record by record — it shares the
//! keyed primitives with the client (vectors pin those), not its framing.
//! It never resolves a name.

use super::{AbortOnDrop, TlsFixture};
use crate::transport::shadow_tls::auth::{Chain, TAG, V2_TAG, same, xor, xor_key};
use crate::transport::shadow_tls::record::{
    ALERT, APPLICATION_DATA, HANDSHAKE, HEADER, RecordReader, data_header,
};
use crate::transport::shadow_tls::sign::hello_tag;
use rurge_config::spec::ShadowTlsVersion;
use rustls::SupportedProtocolVersion;
use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, watch};

const CHANGE_CIPHER_SPEC: u8 = 20;
const SERVER_HELLO: u8 = 2;

/// What goes wrong at the start of the data phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowTlsFault {
    /// v3: a data record whose tag is wrong.
    BadTag,
    /// The connection ends in the middle of a record.
    CutRecord,
    /// A record that is neither data nor an alert.
    StrayHandshake,
}

/// No `Debug`: it holds the password.
#[derive(Clone)]
pub struct ShadowTlsScript {
    pub version: ShadowTlsVersion,
    pub password: String,
    /// The handshake server: a TLS server on a loopback port.
    pub camouflage: SocketAddr,
    /// Where the payload of a client that proved itself goes.
    pub connect_to: SocketAddr,
    /// v3: records sealed under the handshake's chain, sent ahead of the
    /// first data record (what a session ticket in flight looks like).
    pub residual: usize,
    /// v2: how many records of the handshake server must have gone down
    /// after the client's Finished before the data phase may start (rustls
    /// packs its session tickets into one record).
    pub late_records: usize,
    /// Answer the client's FIN with an alert record and keep sending (sing-box).
    pub alert_on_fin: bool,
    /// An empty data record ahead of everything else.
    pub empty_record: bool,
    pub fault: Option<ShadowTlsFault>,
}

impl ShadowTlsScript {
    pub fn new(
        version: ShadowTlsVersion,
        password: &str,
        camouflage: SocketAddr,
        connect_to: SocketAddr,
    ) -> ShadowTlsScript {
        ShadowTlsScript {
            version,
            password: password.to_string(),
            camouflage,
            connect_to,
            residual: 0,
            late_records: 0,
            alert_on_fin: false,
            empty_record: false,
            fault: None,
        }
    }
}

/// Written down the moment the server knows: a client that proved itself
/// when its first data record verified, any other when it is turned into a
/// plain relay (v3) or when its connection ends (v2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedShadowTls {
    pub authenticated: bool,
}

type Sessions = Arc<Mutex<Vec<RecordedShadowTls>>>;

fn note(sessions: &Sessions, authenticated: bool) {
    sessions
        .lock()
        .expect("sessions")
        .push(RecordedShadowTls { authenticated });
}

pub struct FakeShadowTls {
    addr: SocketAddr,
    sessions: Sessions,
    _task: AbortOnDrop,
}

impl FakeShadowTls {
    pub async fn spawn(script: ShadowTlsScript) -> FakeShadowTls {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let sessions = Sessions::default();
        let recorded = sessions.clone();
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (script, recorded) = (script.clone(), recorded.clone());
                tokio::spawn(async move {
                    // an error here is the client's doing: the tests look at
                    // what the client saw
                    let _ = match script.version {
                        ShadowTlsVersion::V2 => serve_v2(client, &script, &recorded).await,
                        ShadowTlsVersion::V3 => serve_v3(client, &script, &recorded).await,
                    };
                });
            }
        });
        FakeShadowTls {
            addr,
            sessions,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn sessions(&self) -> Vec<RecordedShadowTls> {
        self.sessions.lock().expect("sessions").clone()
    }
}

/// What a server does with a client that did not prove itself: it is a plain
/// relay in front of the handshake server.
async fn plain_relay(
    mut client: TcpStream,
    mut camouflage: TcpStream,
    sessions: &Sessions,
) -> io::Result<()> {
    note(sessions, false);
    let _ = tokio::io::copy_bidirectional(&mut client, &mut camouflage).await;
    Ok(())
}

async fn serve_v3(
    mut client: TcpStream,
    script: &ShadowTlsScript,
    sessions: &Sessions,
) -> io::Result<()> {
    let password = script.password.as_bytes();
    let mut from_client = RecordReader::default();
    if !from_client.next(&mut client).await? {
        return Ok(());
    }
    let hello = from_client.record().clone();
    from_client.consume();
    // type, ClientHello, a 32-byte session id whose last 4 bytes are the tag
    let signed = hello.len() >= 76
        && hello[0] == HANDSHAKE
        && hello[HEADER] == 1
        && hello[43] == 32
        && same(&hello_tag(password, &hello), &hello[72..76]);
    let mut camouflage = TcpStream::connect(script.camouflage).await?;
    camouflage.write_all(&hello).await?;
    if !signed {
        return plain_relay(client, camouflage, sessions).await;
    }
    let mut from_camouflage = RecordReader::default();
    if !from_camouflage.next(&mut camouflage).await? {
        return Ok(());
    }
    let first = from_camouflage.record().clone();
    from_camouflage.consume();
    client.write_all(&first).await?;
    if first.len() < 43 || first[0] != HANDSHAKE || first[HEADER] != SERVER_HELLO {
        return plain_relay(client, camouflage, sessions).await;
    }
    let random: [u8; 32] = first[11..43].try_into().expect("32 bytes");
    let key = xor_key(password, &random);
    let mut handshake_chain = Chain::new(password, &[&random]);
    let (mut client_r, mut client_w) = tokio::io::split(client);
    let (mut camouflage_r, mut camouflage_w) = tokio::io::split(camouflage);
    let stop = Notify::new();
    let mut verify = Chain::new(password, &[&random, b"C"]);
    let upwards = async {
        // the client's records go to the handshake server until one of them
        // verifies under the client's chain: that one is the first payload
        let found = loop {
            if !from_client.next(&mut client_r).await? {
                break None;
            }
            let record = from_client.record();
            if record[0] == APPLICATION_DATA && record.len() > HEADER + TAG {
                let mut attempt = verify.clone();
                let tag = attempt.frame_tag(&record[HEADER + TAG..]);
                if same(&tag, &record[HEADER..HEADER + TAG]) {
                    verify = attempt;
                    let payload = record[HEADER + TAG..].to_vec();
                    from_client.consume();
                    break Some(payload);
                }
            }
            camouflage_w.write_all(record).await?;
            from_client.consume();
        };
        stop.notify_one();
        io::Result::Ok(found)
    };
    let downwards = async {
        loop {
            // stopped between two records only: a record is never cut
            tokio::select! {
                biased;
                _ = stop.notified() => return io::Result::Ok(()),
                more = from_camouflage.next(&mut camouflage_r) => {
                    if !more? {
                        return Ok(());
                    }
                }
            }
            let record = from_camouflage.record();
            if record[0] == APPLICATION_DATA {
                xor(&mut record[HEADER..], &key);
                handshake_chain.update(&record[HEADER..]);
                let tag = handshake_chain.digest::<TAG>();
                client_w
                    .write_all(&data_header(TAG + record.len() - HEADER))
                    .await?;
                client_w.write_all(&tag).await?;
                client_w.write_all(&record[HEADER..]).await?;
            } else {
                client_w.write_all(record).await?;
            }
            from_camouflage.consume();
        }
    };
    let (found, relayed) = tokio::join!(upwards, downwards);
    relayed?;
    let Some(first_payload) = found? else {
        note(sessions, false);
        return Ok(());
    };
    note(sessions, true);
    drop((camouflage_r, camouflage_w));
    for n in 0..script.residual {
        let mut payload = vec![n as u8; 40 + n];
        xor(&mut payload, &key);
        handshake_chain.update(&payload);
        client_w
            .write_all(&data_header(TAG + payload.len()))
            .await?;
        client_w.write_all(&handshake_chain.digest::<TAG>()).await?;
        client_w.write_all(&payload).await?;
    }
    let phase = DataPhase {
        client_r,
        client_w,
        from_client,
        up: Tagging::V3(verify),
        down: Tagging::V3(Chain::new(password, &[&random, b"S"])),
        first_payload,
    };
    data_phase(phase, script).await
}

async fn serve_v2(
    client: TcpStream,
    script: &ShadowTlsScript,
    sessions: &Sessions,
) -> io::Result<()> {
    let password = script.password.as_bytes();
    let camouflage = TcpStream::connect(script.camouflage).await?;
    let (mut client_r, mut client_w) = tokio::io::split(client);
    let (mut camouflage_r, mut camouflage_w) = tokio::io::split(camouflage);
    // every byte written to the client while the handshake is relayed
    let written = Mutex::new(Chain::new(password, &[]));
    // how many records went down after the client's first ApplicationData
    // record (its Finished, in TLS 1.3)
    let (after_finished, mut progress) = watch::channel(0usize);
    let finished = std::sync::atomic::AtomicBool::new(false);
    let stop = Notify::new();
    let mut from_client = RecordReader::default();
    let upwards = async {
        let (mut seen_handshake, mut seen_ccs) = (false, false);
        // the digest as it stood at each of the client's recent records: what
        // the client saw may be older than what has been relayed since
        let mut digests: VecDeque<[u8; V2_TAG]> = VecDeque::with_capacity(10);
        let found = loop {
            if !from_client.next(&mut client_r).await? {
                break None;
            }
            let record = from_client.record();
            seen_handshake |= record[0] == HANDSHAKE;
            seen_ccs |= record[0] == CHANGE_CIPHER_SPEC;
            if record[0] == APPLICATION_DATA
                && seen_handshake
                && seen_ccs
                && record.len() >= HEADER + V2_TAG
            {
                if digests.len() == 10 {
                    digests.pop_front();
                }
                digests.push_back(written.lock().expect("digest").digest::<V2_TAG>());
                let claimed = &record[HEADER..HEADER + V2_TAG];
                if digests.iter().any(|d| same(d, claimed)) {
                    let payload = record[HEADER + V2_TAG..].to_vec();
                    from_client.consume();
                    break Some(payload);
                }
                finished.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            camouflage_w.write_all(record).await?;
            from_client.consume();
        };
        if found.is_some() {
            // what the script wants the client to have met first
            let _ = progress.wait_for(|n| *n >= script.late_records).await;
        }
        stop.notify_one();
        io::Result::Ok(found)
    };
    let downwards = async {
        let mut from_camouflage = RecordReader::default();
        loop {
            tokio::select! {
                biased;
                _ = stop.notified() => return io::Result::Ok(()),
                more = from_camouflage.next(&mut camouflage_r) => {
                    if !more? {
                        return Ok(());
                    }
                }
            }
            let record = from_camouflage.record();
            client_w.write_all(record).await?;
            written.lock().expect("digest").update(record);
            if finished.load(std::sync::atomic::Ordering::SeqCst) {
                after_finished.send_modify(|n| *n += 1);
            }
            from_camouflage.consume();
        }
    };
    let (found, relayed) = tokio::join!(upwards, downwards);
    relayed?;
    let Some(first_payload) = found? else {
        note(sessions, false);
        return Ok(());
    };
    note(sessions, true);
    drop((camouflage_r, camouflage_w));
    let phase = DataPhase {
        client_r,
        client_w,
        from_client,
        up: Tagging::V2,
        down: Tagging::V2,
        first_payload,
    };
    data_phase(phase, script).await
}

/// One direction of the data phase: a v2 frame carries nothing but payload,
/// a v3 frame starts with a tag from this direction's chain.
enum Tagging {
    V2,
    V3(Chain),
}

impl Tagging {
    fn seal(&mut self, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + TAG + payload.len());
        match self {
            Tagging::V2 => out.extend_from_slice(&data_header(payload.len())),
            Tagging::V3(chain) => {
                out.extend_from_slice(&data_header(TAG + payload.len()));
                out.extend_from_slice(&chain.frame_tag(payload));
            }
        }
        out.extend_from_slice(payload);
        out
    }

    /// The payload of a client record, or `None` when it does not verify.
    fn open<'a>(&mut self, record: &'a [u8]) -> Option<&'a [u8]> {
        if record[0] != APPLICATION_DATA {
            return None;
        }
        match self {
            Tagging::V2 => Some(&record[HEADER..]),
            Tagging::V3(chain) => {
                let (tag, payload) = record[HEADER..].split_at_checked(TAG)?;
                same(&chain.frame_tag(payload), tag).then_some(payload)
            }
        }
    }
}

struct DataPhase {
    client_r: ReadHalf<TcpStream>,
    client_w: WriteHalf<TcpStream>,
    from_client: RecordReader,
    /// Verifies what the client sends.
    up: Tagging,
    /// Seals what goes to the client.
    down: Tagging,
    first_payload: Vec<u8>,
}

async fn data_phase(phase: DataPhase, script: &ShadowTlsScript) -> io::Result<()> {
    let DataPhase {
        mut client_r,
        mut client_w,
        mut from_client,
        mut up,
        mut down,
        first_payload,
    } = phase;
    if script.empty_record {
        client_w.write_all(&down.seal(&[])).await?;
    }
    match script.fault {
        Some(ShadowTlsFault::BadTag) => {
            let mut record = down.seal(b"not what the tag says");
            record[HEADER] ^= 0x01;
            client_w.write_all(&record).await?;
        }
        Some(ShadowTlsFault::CutRecord) => {
            let record = down.seal(&[7u8; 100]);
            client_w.write_all(&record[..40]).await?;
            client_w.shutdown().await?;
            // read the client out, so the close is a FIN and not a reset
            while from_client.next(&mut client_r).await? {
                from_client.consume();
            }
            return Ok(());
        }
        Some(ShadowTlsFault::StrayHandshake) => {
            client_w.write_all(&[HANDSHAKE, 3, 3, 0, 1, 0]).await?;
        }
        None => {}
    }
    let mut target = TcpStream::connect(script.connect_to).await?;
    target.write_all(&first_payload).await?;
    let (mut target_r, mut target_w) = target.split();
    // two loops that never wait for each other: a relay that writes from
    // inside one `select!` loop deadlocks as soon as both sides push back.
    // Only the alert makes them share the client's write half.
    let client_w = tokio::sync::Mutex::new(client_w);
    let upwards = async {
        while from_client.next(&mut client_r).await? {
            let Some(payload) = up.open(from_client.record()) else {
                return Err(io::Error::other("a client record does not verify"));
            };
            target_w.write_all(payload).await?;
            from_client.consume();
        }
        target_w.shutdown().await?;
        if script.alert_on_fin {
            let mut alert = vec![ALERT, 3, 3, 0, 26];
            alert.extend_from_slice(&[0x5a; 26]);
            client_w.lock().await.write_all(&alert).await?;
        }
        Ok(())
    };
    let downwards = async {
        let mut buf = vec![0u8; 8192];
        loop {
            let n = target_r.read(&mut buf).await?;
            if n == 0 {
                return client_w.lock().await.shutdown().await;
            }
            let record = down.seal(&buf[..n]);
            client_w.lock().await.write_all(&record).await?;
        }
    };
    let (sent, received) = tokio::join!(upwards, downwards);
    sent.and(received)
}

/// The site a Shadow TLS server borrows its handshake from: completes TLS
/// handshakes (the fixture records them), keeps what clients say, and says
/// nothing itself.
pub struct Camouflage {
    addr: SocketAddr,
    received: Arc<Mutex<Vec<u8>>>,
    _task: AbortOnDrop,
}

impl Camouflage {
    pub async fn spawn(
        fixture: &Arc<TlsFixture>,
        versions: &[&'static SupportedProtocolVersion],
        tickets: usize,
    ) -> Camouflage {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let acceptor = fixture.camouflage_acceptor(versions, tickets);
        let received: Arc<Mutex<Vec<u8>>> = Arc::default();
        let (fixture, heard) = (fixture.clone(), received.clone());
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (fixture, acceptor, heard) = (fixture.clone(), acceptor.clone(), heard.clone());
                tokio::spawn(async move {
                    let Ok(mut stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        heard.lock().expect("received").extend_from_slice(&buf[..n]);
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });
        Camouflage {
            addr,
            received,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Everything clients sent inside their TLS sessions, in arrival order.
    pub fn received(&self) -> Vec<u8> {
        self.received.lock().expect("received").clone()
    }
}
