//! Shadow TLS, client side (M2 design 5.3): a real TLS handshake with a
//! camouflage site, relayed by the server, and then frames that look like
//! the ApplicationData of that session. The layer sits between the
//! connector and the policy's own TLS.
//!
//! The handshake is driven by hand (`read_tls` / `process_new_packets` /
//! `write_tls`), one record at a time: v2 digests every byte the server sends,
//! v3 has to verify and rewrite records before rustls may see them.
//!
//! The camouflage handshake verifies the certificate like any TLS client
//! would, and none of the policy's TLS parameters applies to it: a client
//! that does not mind a bad certificate is a tell.

pub(crate) mod auth;
mod framed;
pub(crate) mod record;
pub(crate) mod sign;
#[cfg(test)]
mod vectors;

use crate::outbound::untrusted_text;
use crate::{BuildError, OutboundError};
use auth::{Chain, TAG, V2_TAG, same, xor, xor_key};
use framed::{Framed, Mode};
use record::{APPLICATION_DATA, HANDSHAKE, HEADER, RecordReader};
use rurge_config::HostName;
use rurge_config::spec::{ShadowTlsOpts, ShadowTlsVersion};
use rurge_net::connector::BoxedStream;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const SERVER_HELLO: u8 = 2;
/// A ServerHello record: its random sits behind the record header, the
/// handshake header (4) and the version (2); the session id follows.
const SERVER_RANDOM_AT: usize = HEADER + 4 + 2;
const SESSION_ID_LEN_AT: usize = SERVER_RANDOM_AT + 32;
const SUPPORTED_VERSIONS: u16 = 43;
const TLS13: u16 = 0x0304;
/// How long the good-bye to a server that is not ours may take.
const FAREWELL: Duration = Duration::from_secs(2);

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(text.to_string())
}

/// Built once per outbound, used for every connection. No `Debug`: it holds
/// the password.
pub struct ShadowTlsClient {
    version: ShadowTlsVersion,
    password: Vec<u8>,
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
}

impl ShadowTlsClient {
    /// `fallback` is the name the certificate is checked against when the
    /// policy has no `shadow-tls-sni`; no SNI is sent then (manual).
    pub fn build(
        opts: &ShadowTlsOpts,
        fallback: &HostName,
        roots: Arc<RootCertStore>,
    ) -> Result<ShadowTlsClient, BuildError> {
        let provider = match opts.version {
            ShadowTlsVersion::V2 => Arc::new(rustls::crypto::ring::default_provider()),
            ShadowTlsVersion::V3 => sign::provider(),
        };
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| BuildError::new(format!("cannot set up the Shadow TLS handshake: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        // a resumed session would change what the ClientHello draws (appendix A)
        config.resumption = rustls::client::Resumption::disabled();
        config.enable_sni = opts.sni.is_some();
        let name = match (&opts.sni, fallback) {
            (Some(name), _) => name.clone(),
            (None, HostName::Domain(domain)) => domain.clone(),
            (None, HostName::Ip(ip)) => ip.to_string(),
        };
        let name = ServerName::try_from(name)
            .map_err(|_| BuildError::new("the Shadow TLS handshake has no valid server name"))?;
        let client = ShadowTlsClient {
            version: opts.version,
            password: opts.password.expose().as_bytes().to_vec(),
            config: Arc::new(config),
            name,
        };
        // the four assumptions of appendix A, once, before anything connects
        if client.version == ShadowTlsVersion::V3
            && sign::signed_hello(&client.config, &client.name, &client.password).is_none()
        {
            return Err(BuildError::new("shadow-tls: cannot sign the ClientHello"));
        }
        Ok(client)
    }

    /// The camouflage handshake over `stream`, and then the framed stream.
    /// No timeout of its own, but for the good-bye to a server that turned
    /// out not to be ours.
    pub async fn wrap(&self, stream: BoxedStream) -> Result<BoxedStream, OutboundError> {
        match self.version {
            ShadowTlsVersion::V2 => self.wrap_v2(stream).await,
            ShadowTlsVersion::V3 => self.wrap_v3(stream).await,
        }
    }

    async fn wrap_v2(&self, mut stream: BoxedStream) -> Result<BoxedStream, OutboundError> {
        let conn = ClientConnection::new(self.config.clone(), self.name.clone())
            .map_err(handshake_failed)?;
        let mut shake = Handshake::new(conn);
        let mut digest = Chain::new(&self.password, &[]);
        while shake.step(&mut stream).await? {
            // every byte the server sends while the handshake lasts
            digest.update(shake.reader.record());
            shake.feed(&mut stream).await?;
        }
        let mode = Mode::V2 {
            first: Some(digest.digest::<V2_TAG>()),
            session: Some(Box::new(shake.conn)),
        };
        Ok(Box::new(Framed::new(stream, shake.reader, mode)))
    }

    async fn wrap_v3(&self, mut stream: BoxedStream) -> Result<BoxedStream, OutboundError> {
        let Some((conn, hello)) = sign::signed_hello(&self.config, &self.name, &self.password)
        else {
            return Err(proxy("shadow-tls: cannot sign the ClientHello"));
        };
        stream.write_all(&hello).await?;
        stream.flush().await?;
        let mut shake = Handshake::new(conn);
        let mut server: Option<ServerSide> = None;
        let mut tls13 = false;
        // every ApplicationData record of the handshake carried a good tag
        let mut verified = 0usize;
        let mut genuine = true;
        while shake.step(&mut stream).await? {
            let record = shake.reader.record();
            match record[0] {
                HANDSHAKE if server.is_none() => {
                    if let Some(random) = server_random(record) {
                        tls13 = is_tls13(record);
                        server = Some(ServerSide::new(&self.password, &random));
                    }
                }
                APPLICATION_DATA if genuine => {
                    if server.as_mut().is_some_and(|side| side.restore(record)) {
                        verified += 1;
                    } else {
                        // not ours: let the handshake run its course untouched
                        genuine = false;
                    }
                }
                _ => {}
            }
            shake.feed(&mut stream).await?;
        }
        let refusal = if !tls13 {
            Some("shadow-tls: the handshake server does not support TLS 1.3")
        } else if !genuine || verified == 0 {
            Some("shadow-tls: the server did not authenticate itself")
        } else {
            None
        };
        if let Some(text) = refusal {
            // what we reached is the camouflage site itself, or someone in
            // between: behave like a client that wanted a page (the
            // reference client's way out), then leave
            let _ = tokio::time::timeout(FAREWELL, shake.farewell(&mut stream, &self.name)).await;
            return Err(proxy(text));
        }
        let side = server.expect("a verified record implies a ServerHello");
        let mode = Mode::V3 {
            add: Chain::new(&self.password, &[&side.random, b"C"]),
            verify: Chain::new(&self.password, &[&side.random, b"S"]),
            ignore: Some(side.chain),
        };
        Ok(Box::new(Framed::new(stream, shake.reader, mode)))
    }
}

/// What v3 learns from the ServerHello.
struct ServerSide {
    random: [u8; 32],
    chain: Chain,
    key: [u8; 32],
}

impl ServerSide {
    fn new(password: &[u8], random: &[u8; 32]) -> ServerSide {
        ServerSide {
            random: *random,
            chain: Chain::new(password, &[random]),
            key: xor_key(password, random),
        }
    }

    /// An ApplicationData record of the handshake as a v3 server sends it:
    /// `<4-byte tag><payload XOR key>`. Verified, it becomes the record the
    /// handshake server wrote. `false`: not ours, and left as it came.
    fn restore(&mut self, record: &mut Vec<u8>) -> bool {
        if record.len() <= HEADER + TAG {
            return false;
        }
        self.chain.update(&record[HEADER + TAG..]);
        if !same(&self.chain.digest::<TAG>(), &record[HEADER..HEADER + TAG]) {
            return false;
        }
        record.drain(HEADER..HEADER + TAG);
        xor(&mut record[HEADER..], &self.key);
        let len = u16::try_from(record.len() - HEADER).expect("it was read as one record");
        record[3..HEADER].copy_from_slice(&len.to_be_bytes());
        true
    }
}

/// The random of a record that starts with a ServerHello.
fn server_random(record: &[u8]) -> Option<[u8; 32]> {
    if record.len() <= SESSION_ID_LEN_AT || record[HEADER] != SERVER_HELLO {
        return None;
    }
    record[SERVER_RANDOM_AT..SESSION_ID_LEN_AT].try_into().ok()
}

/// Whether the ServerHello in `record` selects TLS 1.3 (`supported_versions`).
fn is_tls13(record: &[u8]) -> bool {
    fn take<'a>(rest: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        let (head, tail) = rest.split_at_checked(n)?;
        *rest = tail;
        Some(head)
    }
    fn u16_of(bytes: &[u8]) -> usize {
        usize::from(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
    let scan = || {
        let mut rest = record.get(SESSION_ID_LEN_AT..)?;
        let session_id = usize::from(take(&mut rest, 1)?[0]);
        // the session id, the cipher suite (2) and the compression method (1)
        take(&mut rest, session_id + 3)?;
        let extensions = u16_of(take(&mut rest, 2)?);
        let mut rest = rest.get(..extensions)?;
        while !rest.is_empty() {
            let kind = u16_of(take(&mut rest, 2)?);
            let len = u16_of(take(&mut rest, 2)?);
            let body = take(&mut rest, len)?;
            if kind == usize::from(SUPPORTED_VERSIONS) {
                return Some(body.len() == 2 && u16_of(body) == usize::from(TLS13));
            }
        }
        Some(false)
    };
    scan().unwrap_or(false)
}

fn handshake_failed(error: rustls::Error) -> OutboundError {
    // the text may quote names the server presented
    OutboundError::Proxy(format!(
        "shadow-tls: the camouflage handshake failed: {}",
        untrusted_text(&error.to_string(), 200)
    ))
}

/// A rustls client connection driven one record at a time.
struct Handshake {
    conn: ClientConnection,
    reader: RecordReader,
}

impl Handshake {
    fn new(conn: ClientConnection) -> Handshake {
        Handshake {
            conn,
            reader: RecordReader::default(),
        }
    }

    async fn send(&mut self, stream: &mut BoxedStream) -> io::Result<()> {
        let mut out = Vec::new();
        while self.conn.wants_write() {
            self.conn.write_tls(&mut out)?;
        }
        if !out.is_empty() {
            stream.write_all(&out).await?;
            stream.flush().await?;
        }
        Ok(())
    }

    /// Sends what rustls wants sent; then, while the handshake lasts, reads
    /// the next record into `self.reader`. `false`: the handshake is done.
    async fn step(&mut self, stream: &mut BoxedStream) -> Result<bool, OutboundError> {
        self.reader.consume();
        self.send(stream).await?;
        if !self.conn.is_handshaking() {
            return Ok(false);
        }
        if !self.reader.next(stream).await? {
            return Err(proxy(
                "shadow-tls: the server closed the connection during the handshake",
            ));
        }
        Ok(true)
    }

    /// Hands the reader's record (as the caller left it) to rustls.
    async fn feed(&mut self, stream: &mut BoxedStream) -> Result<(), OutboundError> {
        let mut rest = &self.reader.record()[..];
        let mut outcome = Ok(());
        while outcome.is_ok() && !rest.is_empty() {
            outcome = match self.conn.read_tls(&mut rest) {
                Ok(_) => self
                    .conn
                    .process_new_packets()
                    .map(drop)
                    .map_err(handshake_failed),
                Err(e) => Err(e.into()),
            };
        }
        if outcome.is_err() {
            // the alert rustls queued: a TLS client says why it leaves
            let _ = self.send(stream).await;
        }
        outcome
    }

    /// One plausible request over the finished session, then whatever comes
    /// back until the other side is done. Nothing of it is kept.
    async fn farewell(&mut self, stream: &mut BoxedStream, name: &ServerName<'static>) {
        let mut pad = [0u8; 48];
        let _ = getrandom::fill(&mut pad);
        // 16 to 47 characters: the request has no constant length
        let session: String = pad[1..17 + usize::from(pad[0] % 32)]
            .iter()
            .map(|b| char::from(b"abcdefghijklmnopqrstuvwxyz0123456789"[usize::from(b % 36)]))
            .collect();
        let host = match name {
            ServerName::DnsName(dns) => dns.as_ref().to_string(),
            ServerName::IpAddress(ip) => std::net::IpAddr::from(*ip).to_string(),
            _ => String::new(),
        };
        let request = format!(
            "GET / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: curl/8.5.0\r\nAccept: */*\r\n\
             Cookie: sessionid={session}\r\nConnection: close\r\n\r\n"
        );
        if self.conn.writer().write_all(request.as_bytes()).is_err() {
            return;
        }
        self.conn.send_close_notify();
        if self.send(stream).await.is_err() {
            return;
        }
        loop {
            self.reader.consume();
            if !matches!(self.reader.next(stream).await, Ok(true)) {
                return;
            }
            let mut rest = &self.reader.record()[..];
            while !rest.is_empty() {
                if self.conn.read_tls(&mut rest).is_err() {
                    return;
                }
                let Ok(state) = self.conn.process_new_packets() else {
                    return;
                };
                let mut sink = vec![0; state.plaintext_bytes_to_read()];
                let _ = self.conn.reader().read(&mut sink);
                if state.peer_has_closed() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        Camouflage, FakeShadowTls, ShadowTlsFault, ShadowTlsScript, TlsFixture, echo_server,
    };
    use rurge_config::spec::Secret;
    use rustls::version::{TLS12, TLS13};
    use std::net::SocketAddr;
    use std::pin::Pin;
    use std::task::{Context, Poll, ready};
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
    use tokio::net::{TcpListener, TcpStream};

    const SITE: &str = "camouflage.test";
    const BOUND: Duration = Duration::from_secs(30);

    fn opts(version: ShadowTlsVersion, password: &str, sni: Option<&str>) -> ShadowTlsOpts {
        ShadowTlsOpts {
            password: Secret::from(password),
            sni: sni.map(str::to_string),
            version,
        }
    }

    fn client(version: ShadowTlsVersion, password: &str, fixture: &TlsFixture) -> ShadowTlsClient {
        let fallback = HostName::parse("127.0.0.1");
        ShadowTlsClient::build(
            &opts(version, password, Some(SITE)),
            &fallback,
            fixture.roots(),
        )
        .expect("the client builds")
    }

    async fn open(
        client: &ShadowTlsClient,
        server: SocketAddr,
    ) -> Result<BoxedStream, OutboundError> {
        let tcp = TcpStream::connect(server).await?;
        client.wrap(Box::new(tcp)).await
    }

    /// A stream that queues every written byte and only forwards it to
    /// `inner` when `poll_flush` runs: what a `write_all` without an
    /// explicit `flush` looks like to the code writing to it. Catches a
    /// write that relies on some other call flushing for it.
    struct FlushGated {
        inner: TcpStream,
        pending: Vec<u8>,
    }

    impl FlushGated {
        fn new(inner: TcpStream) -> FlushGated {
            FlushGated {
                inner,
                pending: Vec::new(),
            }
        }
    }

    impl AsyncRead for FlushGated {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for FlushGated {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.get_mut().pending.extend_from_slice(data);
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let this = self.get_mut();
            while !this.pending.is_empty() {
                let n = ready!(Pin::new(&mut this.inner).poll_write(cx, &this.pending))?;
                this.pending.drain(..n);
            }
            Pin::new(&mut this.inner).poll_flush(cx)
        }

        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
        }
    }

    async fn open_over_wrapper(
        client: &ShadowTlsClient,
        server: SocketAddr,
    ) -> Result<BoxedStream, OutboundError> {
        let tcp = TcpStream::connect(server).await?;
        client.wrap(Box::new(FlushGated::new(tcp))).await
    }

    /// One direction of the engine's relay: read, `write_all`, `flush`, and
    /// `shutdown` when the source ends.
    async fn copy_half<R, W>(from: &mut R, to: &mut W) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut buf = vec![0u8; 8192];
        loop {
            let n = from.read(&mut buf).await?;
            if n == 0 {
                return to.shutdown().await;
            }
            to.write_all(&buf[..n]).await?;
            to.flush().await?;
        }
    }

    /// Puts `outbound` behind a loopback socket the way the engine does:
    /// both directions polled from ONE task, over `tokio::io::split` halves.
    async fn behind_a_relay(outbound: BoxedStream) -> TcpStream {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (inbound, _) = listener.accept().await.unwrap();
            let (mut in_r, mut in_w) = tokio::io::split(inbound);
            let (mut out_r, mut out_w) = tokio::io::split(outbound);
            let _ = tokio::join!(
                copy_half(&mut in_r, &mut out_w),
                copy_half(&mut out_r, &mut in_w)
            );
        });
        TcpStream::connect(addr).await.unwrap()
    }

    /// Sends `len` bytes while reading them back, then half-closes and
    /// expects the end of the stream.
    async fn echo_round_trip(stream: TcpStream, len: usize) {
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        let (mut r, mut w) = stream.into_split();
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w
        });
        let mut back = vec![0u8; len];
        r.read_exact(&mut back).await.unwrap();
        assert!(back == data, "the echo differs");
        let mut w = writer.await.unwrap();
        w.shutdown().await.unwrap();
        let mut rest = Vec::new();
        r.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }

    /// The fake's record of its `n`th session, waited for within a bound.
    async fn session(fake: &FakeShadowTls, n: usize) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while fake.sessions().len() <= n {
            assert!(tokio::time::Instant::now() < deadline, "no session {n}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        fake.sessions()[n].authenticated
    }

    struct World {
        fixture: Arc<TlsFixture>,
        camouflage: Camouflage,
        fake: FakeShadowTls,
    }

    /// A camouflage site, an echo target and a fake server in front of both.
    async fn world(
        version: ShadowTlsVersion,
        versions: &[&'static rustls::SupportedProtocolVersion],
        tickets: usize,
        tune: impl FnOnce(&mut ShadowTlsScript),
    ) -> World {
        let fixture = TlsFixture::new(&[SITE]);
        let camouflage = Camouflage::spawn(&fixture, versions, tickets).await;
        let mut script =
            ShadowTlsScript::new(version, "right", camouflage.addr(), echo_server().await);
        tune(&mut script);
        let fake = FakeShadowTls::spawn(script).await;
        World {
            fixture,
            camouflage,
            fake,
        }
    }

    #[tokio::test]
    async fn the_handshake_does_not_hang_on_a_stream_that_only_forwards_bytes_on_flush() {
        for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
            let w = world(version, &[&TLS13], 0, |_| {}).await;
            let client = client(version, "right", &w.fixture);
            let stream = tokio::time::timeout(
                Duration::from_secs(5),
                open_over_wrapper(&client, w.fake.addr()),
            )
            .await
            .unwrap_or_else(|_| panic!("{version:?}: the handshake hung without an explicit flush"))
            .unwrap_or_else(|e| panic!("{version:?}: {e}"));
            echo_round_trip(behind_a_relay(stream).await, 1000).await;
        }
    }

    #[tokio::test]
    async fn v3_carries_a_megabyte_both_ways_in_the_relay_s_shape() {
        tokio::time::timeout(BOUND, async {
            // two real session tickets may still be on their way when the
            // data phase begins, and three fabricated records certainly are
            let w = world(ShadowTlsVersion::V3, &[&TLS13], 2, |s| s.residual = 3).await;
            let client = client(ShadowTlsVersion::V3, "right", &w.fixture);
            let stream = open(&client, w.fake.addr()).await.unwrap();
            echo_round_trip(behind_a_relay(stream).await, 1 << 20).await;
            let seen = w.fixture.seen_at_least(1).await;
            assert_eq!(seen[0].sni.as_deref(), Some(SITE));
            assert_eq!(seen[0].alpn, None);
            assert!(session(&w.fake, 0).await);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v2_carries_a_megabyte_both_ways_and_swallows_the_session_tickets() {
        tokio::time::timeout(BOUND, async {
            // the fake holds the data phase back until the record with the
            // site's two session tickets went down
            let w = world(ShadowTlsVersion::V2, &[&TLS13], 2, |s| s.late_records = 1).await;
            let client = client(ShadowTlsVersion::V2, "right", &w.fixture);
            let stream = open(&client, w.fake.addr()).await.unwrap();
            echo_round_trip(behind_a_relay(stream).await, 1 << 20).await;
            assert!(session(&w.fake, 0).await);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v2_works_over_a_tls_1_2_handshake_too() {
        tokio::time::timeout(BOUND, async {
            let w = world(ShadowTlsVersion::V2, &[&TLS12], 0, |_| {}).await;
            let client = client(ShadowTlsVersion::V2, "right", &w.fixture);
            let stream = open(&client, w.fake.addr()).await.unwrap();
            echo_round_trip(behind_a_relay(stream).await, 100_000).await;
            assert!(session(&w.fake, 0).await);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn without_shadow_tls_sni_no_sni_is_sent_and_the_fallback_name_is_verified() {
        tokio::time::timeout(BOUND, async {
            let w = world(ShadowTlsVersion::V2, &[&TLS13], 0, |_| {}).await;
            let fallback = HostName::parse(SITE);
            let client = ShadowTlsClient::build(
                &opts(ShadowTlsVersion::V2, "right", None),
                &fallback,
                w.fixture.roots(),
            )
            .unwrap();
            let stream = open(&client, w.fake.addr()).await.unwrap();
            echo_round_trip(behind_a_relay(stream).await, 1000).await;
            assert_eq!(w.fixture.seen_at_least(1).await[0].sni, None);
            // and a name the certificate does not cover is refused
            let other = HostName::parse("elsewhere.test");
            let client = ShadowTlsClient::build(
                &opts(ShadowTlsVersion::V2, "right", None),
                &other,
                w.fixture.roots(),
            )
            .unwrap();
            let err = open(&client, w.fake.addr()).await.err().expect("refused");
            assert!(
                err.to_string().starts_with(
                    "shadow-tls: the camouflage handshake failed: invalid peer certificate"
                ),
                "{err}"
            );
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn the_policy_s_own_tls_runs_inside_the_frames() {
        tokio::time::timeout(BOUND, async {
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let fixture = TlsFixture::new(&[SITE, "proxy.test"]);
                let camouflage = Camouflage::spawn(&fixture, &[&TLS13], 2).await;
                // behind the Shadow TLS server: a TLS echo server
                let target = fixture.spawn_echo(false).await;
                let fake = FakeShadowTls::spawn(ShadowTlsScript::new(
                    version,
                    "right",
                    camouflage.addr(),
                    target,
                ))
                .await;
                let client = client(version, "right", &fixture);
                let framed = open(&client, fake.addr()).await.unwrap();
                let config = ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(fixture.roots())
                .with_no_client_auth();
                let inner = tokio_rustls::TlsConnector::from(Arc::new(config))
                    .connect(ServerName::try_from("proxy.test").unwrap(), framed)
                    .await
                    .expect("the inner handshake");
                echo_round_trip(behind_a_relay(Box::new(inner)).await, 300_000).await;
                // the site's handshake, then the proxy's own
                let seen = fixture.seen_at_least(2).await;
                assert_eq!(seen[0].sni.as_deref(), Some(SITE));
                assert_eq!(seen[1].sni.as_deref(), Some("proxy.test"));
            }
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v3_says_good_bye_like_a_browser_when_the_server_is_not_ours() {
        tokio::time::timeout(BOUND, async {
            // a wrong password: the server relays the site untouched
            let w = world(ShadowTlsVersion::V3, &[&TLS13], 2, |_| {}).await;
            let client = client(ShadowTlsVersion::V3, "wrong", &w.fixture);
            let err = open(&client, w.fake.addr()).await.err().expect("refused");
            assert_eq!(
                err.to_string(),
                "shadow-tls: the server did not authenticate itself"
            );
            let heard = String::from_utf8(w.camouflage.received()).unwrap();
            assert!(
                heard.starts_with("GET / HTTP/1.1\r\nHost: camouflage.test\r\n")
                    && heard.ends_with("Connection: close\r\n\r\n"),
                "{heard}"
            );
            assert!(!session(&w.fake, 0).await);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v3_needs_a_handshake_server_that_speaks_tls_1_3() {
        tokio::time::timeout(BOUND, async {
            let w = world(ShadowTlsVersion::V3, &[&TLS12], 0, |_| {}).await;
            let client = client(ShadowTlsVersion::V3, "right", &w.fixture);
            let err = open(&client, w.fake.addr()).await.err().expect("refused");
            assert_eq!(
                err.to_string(),
                "shadow-tls: the handshake server does not support TLS 1.3"
            );
            // the good-bye went through the TLS 1.2 session all the same
            let heard = w.camouflage.received();
            assert!(heard.starts_with(b"GET / HTTP/1.1\r\n"), "{heard:?}");
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v2_with_a_wrong_password_ends_when_the_site_ends_the_session() {
        tokio::time::timeout(BOUND, async {
            let w = world(ShadowTlsVersion::V2, &[&TLS13], 2, |_| {}).await;
            let client = client(ShadowTlsVersion::V2, "wrong", &w.fixture);
            // the handshake itself cannot tell
            let mut stream = open(&client, w.fake.addr()).await.unwrap();
            stream.write_all(b"this goes to the site").await.unwrap();
            let mut buf = [0u8; 16];
            let err = stream.read(&mut buf).await.expect_err("the site hangs up");
            assert_eq!(
                err.to_string(),
                "shadow-tls: the handshake server closed the session"
            );
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn a_site_with_an_unknown_certificate_is_refused() {
        tokio::time::timeout(BOUND, async {
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let w = world(version, &[&TLS13], 0, |_| {}).await;
                let stranger = TlsFixture::new(&[SITE]);
                let client = client(version, "right", &stranger);
                let err = open(&client, w.fake.addr()).await.err().expect("refused");
                // which way the chain fails is webpki's business
                assert!(
                    err.to_string().starts_with(
                        "shadow-tls: the camouflage handshake failed: invalid peer certificate: "
                    ),
                    "{err}"
                );
            }
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn a_server_that_hangs_up_during_the_handshake_is_reported_as_such() {
        tokio::time::timeout(BOUND, async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                while let Ok((mut tcp, _)) = listener.accept().await {
                    // take the ClientHello, then a FIN and not a reset
                    let mut sink = [0u8; 4096];
                    let _ = tcp.read(&mut sink).await;
                    let _ = tcp.shutdown().await;
                    while matches!(tcp.read(&mut sink).await, Ok(n) if n > 0) {}
                }
            });
            let fixture = TlsFixture::new(&[SITE]);
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let client = client(version, "right", &fixture);
                let err = open(&client, addr).await.err().expect("refused");
                assert_eq!(
                    err.to_string(),
                    "shadow-tls: the server closed the connection during the handshake"
                );
            }
        })
        .await
        .expect("bounded");
    }

    async fn first_read_error(version: ShadowTlsVersion, fault: ShadowTlsFault) -> io::Error {
        let w = world(version, &[&TLS13], 0, |s| s.fault = Some(fault)).await;
        let client = client(version, "right", &w.fixture);
        let mut stream = open(&client, w.fake.addr()).await.unwrap();
        stream.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 64];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => panic!("the stream ended without an error"),
                Ok(_) => {}
                Err(e) => return e,
            }
        }
    }

    #[tokio::test]
    async fn what_a_broken_data_phase_looks_like() {
        tokio::time::timeout(BOUND, async {
            let e = first_read_error(ShadowTlsVersion::V3, ShadowTlsFault::BadTag).await;
            assert_eq!(e.kind(), io::ErrorKind::InvalidData);
            assert_eq!(
                e.to_string(),
                "shadow-tls: a record cannot be authenticated"
            );
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let e = first_read_error(version, ShadowTlsFault::CutRecord).await;
                assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
                assert_eq!(
                    e.to_string(),
                    "shadow-tls: the connection ended in the middle of a record"
                );
                let e = first_read_error(version, ShadowTlsFault::StrayHandshake).await;
                assert_eq!(e.to_string(), "shadow-tls: unexpected record type");
            }
        })
        .await
        .expect("bounded");
    }

    /// Answers only once the client has finished talking.
    async fn answers_after_the_fin() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut said = Vec::new();
                    tcp.read_to_end(&mut said).await.unwrap();
                    said.reverse();
                    tcp.write_all(&said).await.unwrap();
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn an_alert_after_our_fin_and_an_empty_record_do_not_end_the_stream() {
        tokio::time::timeout(BOUND, async {
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let fixture = TlsFixture::new(&[SITE]);
                let camouflage = Camouflage::spawn(&fixture, &[&TLS13], 0).await;
                let mut script = ShadowTlsScript::new(
                    version,
                    "right",
                    camouflage.addr(),
                    answers_after_the_fin().await,
                );
                script.alert_on_fin = true;
                script.empty_record = true;
                let fake = FakeShadowTls::spawn(script).await;
                let client = client(version, "right", &fixture);
                let mut stream = open(&client, fake.addr()).await.unwrap();
                stream.write_all(b"abcdef").await.unwrap();
                // sing-box answers this FIN with an alert record, and the
                // answer of the target comes after it
                stream.shutdown().await.unwrap();
                let mut answer = Vec::new();
                stream.read_to_end(&mut answer).await.unwrap();
                assert_eq!(answer, b"fedcba");
            }
        })
        .await
        .expect("bounded");
    }

    #[test]
    fn a_name_rustls_cannot_use_is_a_build_error_and_v3_checks_itself_at_build_time() {
        let roots = Arc::new(RootCertStore::empty());
        let fallback = HostName::parse("127.0.0.1");
        let bad = opts(ShadowTlsVersion::V2, "pw", Some("not a name"));
        let err = ShadowTlsClient::build(&bad, &fallback, roots.clone())
            .err()
            .expect("refused");
        assert_eq!(
            err.message,
            "the Shadow TLS handshake has no valid server name"
        );
        // v3: the two-pass ClientHello is rehearsed once
        let good = opts(ShadowTlsVersion::V3, "pw", Some(SITE));
        assert!(ShadowTlsClient::build(&good, &fallback, roots).is_ok());
    }

    #[test]
    fn the_server_hello_parsers_take_only_what_is_there() {
        // type, version, length, ServerHello, its length, version, random …
        let mut record = vec![22, 3, 3, 0, 0, 2, 0, 0, 0, 3, 3];
        record.extend(1..=32u8);
        assert_eq!(server_random(&record), None, "no session id length yet");
        // … no session id …
        record.push(0);
        assert_eq!(server_random(&record).unwrap()[..3], [1, 2, 3]);
        assert!(!is_tls13(&record), "cut short");
        // … a cipher suite, no compression, and two extensions
        record.extend([0x13, 0x01, 0]);
        record.extend([0, 10, 0, 51, 0, 0, 0, 43, 0, 2, 3, 4]);
        assert!(is_tls13(&record));
        let at = record.len() - 1;
        record[at] = 3;
        assert!(!is_tls13(&record), "supported_versions says TLS 1.2");
        record.truncate(at);
        assert!(!is_tls13(&record), "an extension longer than the record");
        let mut hello_retry = record.clone();
        hello_retry[5] = 1;
        assert_eq!(server_random(&hello_retry), None, "not a ServerHello");
    }
}
