//! When a connection is decrypted, the server-side configuration that does
//! it, and the handshake with the client (`phase 4 M2 design 7`, `8`).
//!
//! `Mitm` is one generation's answer: the hosts `[MITM]` lists, the sources
//! allowed to be decrypted, the ports the list widens sniffing to, and two
//! TLS configurations — one the client sees, one the origin does. Everything
//! that outlives a generation (the CA, its certificates, its sessions) lives
//! in `Authority`.
//!
//! The switch is not here: the engine asks it first, and only then asks
//! `Mitm::decides` (`phase 4 M2 design 7.1`).

use crate::mitm::authority::Authority;
use crate::prefix::Prefixed;
use rurge_config::HostName;
use rurge_config::hostlist::HostList;
use rurge_config::mitm::{MitmConfig, SourceList};
use rurge_config::session::ListenerKind;
use rurge_net::connector::BoxedStream;
use rurge_proto::BuildError;
use rurge_proto::transport::tls::origin_client_config;
use rustls::RootCertStore;
use rustls::client::ClientConfig;
use rustls::pki_types::ServerName;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::{CertifiedKey, SingleCertAndKey};
use rustls::{AlertDescription, ServerConfig};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::TlsAcceptor;

/// What the engine does with a sniffed TLS session (`phase 4 M2 design 7.1`):
/// decrypt it, pass it through, or pass it through because the client offers
/// no protocol rurge decrypts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Decrypt,
    Pass,
    AlpnMismatch,
}

/// The TLS of the origin side of a decrypted connection (`phase 4 M2 design
/// 9.2`): the client configuration, the name it is verified against (the
/// client's SNI, or the target when there is none), and how long the
/// handshake may take — as long as the client's own handshake was given
/// (`phase 4 M2 design 9.2`, the same 30 seconds as `8.2`).
pub struct OriginTls {
    pub config: Arc<ClientConfig>,
    /// The same trust and the same name, offering `http/1.1` only: an upgrade
    /// or a tunnel needs an upstream that speaks it (`phase 4 M2 design 10.4`,
    /// `the phase 4 / M2b plan's P12`).
    pub h1: Arc<ClientConfig>,
    pub name: ServerName<'static>,
    pub wait: Duration,
}

/// A decrypted connection: the TLS stream the HTTP engine serves, the count
/// of what the client sent and whether it closed, the protocol the handshake
/// settled on, and the TLS to the origin.
pub struct Accepted {
    pub stream: BoxedStream,
    pub watch: Arc<Watch>,
    /// Whether the client and rurge settled on HTTP/2 (`phase 4 M2 design
    /// 10.1`, `the phase 4 / M2b plan's P15`).
    pub h2: bool,
    pub origin: OriginTls,
}

/// What a decrypted connection's client did, counted outside the TLS stream
/// (`phase 4 M2 design 8.3`).
#[derive(Debug, Default)]
pub struct Watch {
    read: AtomicU64,
    closed: AtomicBool,
}

impl Watch {
    /// How many bytes of plaintext the client sent.
    pub fn read(&self) -> u64 {
        self.read.load(Ordering::Relaxed)
    }

    /// Whether the client closed the connection (a `close_notify`, the end
    /// of the stream, or a reset).
    pub fn closed_by_client(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

/// Why a handshake with the client did not finish (`phase 4 M2 design 8.2`):
/// the client does not trust the certificate rurge issued, or anything else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandshakeFailure {
    Untrusted { alert: &'static str },
    Failed { reason: String },
}

/// One generation's MITM: what is decrypted and how (`phase 4 M2 design 5.4`).
pub struct Mitm {
    authority: Arc<Authority>,
    effective: HostList,
    sources: Option<SourceList>,
    explicit_ports: Vec<u16>,
    server: Arc<ServerConfig>,
    origin: Arc<ClientConfig>,
    h1: Arc<ClientConfig>,
    h2: bool,
}

impl Mitm {
    /// What this generation decrypts with. The server-side configuration is
    /// built once here and cloned per connection (`phase 4 M2 design 8.1`).
    pub fn build(
        cfg: &MitmConfig,
        authority: Arc<Authority>,
        roots: Arc<RootCertStore>,
    ) -> Result<Mitm, BuildError> {
        let server = server_config(&authority, cfg.h2)?;
        // what the origin is offered with: `h2` beside `http/1.1` when this
        // generation decrypts HTTP/2, `http/1.1` alone otherwise
        let alpn: &[&[u8]] = if cfg.h2 {
            &[b"h2", b"http/1.1"]
        } else {
            &[b"http/1.1"]
        };
        let origin = origin_client_config(roots.clone(), cfg.skip_server_cert_verify, alpn)?;
        // an upgrade or a tunnel dials with this one, which offers
        // `http/1.1` alone (`the phase 4 / M2b plan's P12`); with `h2` off it
        // is the configuration every request gets
        let h1 = if cfg.h2 {
            origin_client_config(roots, cfg.skip_server_cert_verify, &[b"http/1.1"])?
        } else {
            Arc::clone(&origin)
        };
        Ok(Mitm {
            authority,
            effective: cfg.effective.clone(),
            sources: cfg.client_source_address.clone(),
            explicit_ports: cfg.effective.explicit_ports(),
            server,
            origin,
            h1,
            h2: cfg.h2,
        })
    }

    /// The CA this generation signs with.
    pub fn authority(&self) -> &Arc<Authority> {
        &self.authority
    }

    /// Whether this connection is decrypted (`phase 4 M2 design 7.1`), in the
    /// order of the design: the source, the list, the name, the ALPN.
    pub fn decides(
        &self,
        source: IpAddr,
        sni: Option<&HostName>,
        target: &HostName,
        port: u16,
        alpn: &[Vec<u8>],
    ) -> Decision {
        if self.blocked(source) {
            return Decision::Pass;
        }
        // with an SNI only the SNI is compared; a bare entry covers every
        // port for an SNI (`phase 4 M2 design 7.3`)
        let listed = match sni {
            Some(name) => self.effective.matches_sni(name, port),
            None => self.effective.matches(target, port),
        };
        if listed != Some(true) {
            return Decision::Pass;
        }
        // the name the certificate is issued for: one rustls will not take
        // cannot be signed for, so the connection is passed through
        let name = sni.unwrap_or(target);
        if name
            .as_domain()
            .is_some_and(|d| ServerName::try_from(d.to_string()).is_err())
        {
            return Decision::Pass;
        }
        // a client that offers no ALPN is decrypted; a client offering one is
        // decrypted only if rurge serves it — `h2` counts when this
        // generation turns it on (`phase 4 M2 design 10`, `the phase 4 /
        // M2b plan's P15`)
        if !alpn.is_empty()
            && !alpn
                .iter()
                .any(|p| p.as_slice() == b"http/1.1" || (self.h2 && p.as_slice() == b"h2"))
        {
            return Decision::AlpnMismatch;
        }
        Decision::Decrypt
    }

    /// Whether the list widens sniffing to this session (`phase 4 M2 design
    /// 7.6`): the source is allowed, and either the port is one an entry
    /// writes down or the target is listed.
    pub fn widens(&self, source: IpAddr, target: &HostName, port: u16) -> bool {
        !self.blocked(source)
            && (self.explicit_ports.contains(&port)
                || self.effective.matches(target, port) == Some(true))
    }

    /// Signs the certificate for `name` and completes the TLS handshake with
    /// the client, all of it within `wait` (`phase 4 M2 design 8.2`).
    pub async fn accept(
        &self,
        client: Prefixed<BoxedStream>,
        name: &HostName,
        wait: Duration,
    ) -> Result<Accepted, HandshakeFailure> {
        // the certificate, the handshake and everything it says count as one
        // wait: a client that stalls while the CA signs is stalled too
        match tokio::time::timeout(wait, self.handshake(client, name, wait)).await {
            Ok(result) => result,
            Err(_) => Err(HandshakeFailure::Failed {
                reason: "timed out".into(),
            }),
        }
    }

    /// Whether `[MITM] client-source-address` refuses this source.
    fn blocked(&self, source: IpAddr) -> bool {
        self.sources
            .as_ref()
            .is_some_and(|sources| !sources.allows(source))
    }

    async fn handshake(
        &self,
        client: Prefixed<BoxedStream>,
        name: &HostName,
        wait: Duration,
    ) -> Result<Accepted, HandshakeFailure> {
        let leaf = self
            .authority
            .leaf(name)
            .await
            .map_err(|e| HandshakeFailure::Failed {
                reason: e.to_string(),
            })?;
        let origin = server_name(name)?;
        // the template is shared; only the certificate of this connection is
        // its own, and the session cache behind it is still the authority's
        let mut config = (*self.server).clone();
        config.cert_resolver = Arc::new(SingleCertAndKey::from(leaf));
        let stream = TlsAcceptor::from(Arc::new(config))
            .accept(client)
            .await
            .map_err(|e| classify(&e))?;
        // what the handshake settled on, read before the stream is wrapped:
        // the wrapper counts and does not look at the protocols (`phase 4 M2
        // design 10.1`, `the phase 4 / M2b plan's P15`)
        let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2");
        let watch = Arc::new(Watch::default());
        let stream = Watched {
            inner: stream,
            watch: Arc::clone(&watch),
        };
        Ok(Accepted {
            stream: Box::new(stream) as BoxedStream,
            watch,
            h2,
            origin: OriginTls {
                config: Arc::clone(&self.origin),
                h1: Arc::clone(&self.h1),
                name: origin,
                wait,
            },
        })
    }
}

/// What a client's address is compared as (`phase 4 M2 design 7.2`): a
/// session through the virtual interface, and any session from a loopback
/// address, is `127.0.0.1`; everything else is its own address.
pub fn source_of(listener: ListenerKind, src: SocketAddr) -> IpAddr {
    // the virtual interface takes this machine's own traffic, and a loopback
    // address is this machine's too (`phase 4 M2 design 7.2`)
    let ip = src.ip().to_canonical();
    if matches!(listener, ListenerKind::Tun) || ip.is_loopback() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        ip
    }
}

/// What a failed `TlsAcceptor::accept` was (`phase 4 M2 design 8.2`,
/// `M2-D11`): the three certificate alerts a client sends when it does not
/// trust the certificate, and everything else.
pub(crate) fn classify(error: &io::Error) -> HandshakeFailure {
    let rustls_error = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>());
    let alert = match rustls_error {
        Some(rustls::Error::AlertReceived(alert)) => match alert {
            AlertDescription::UnknownCA => Some("unknown_ca"),
            AlertDescription::BadCertificate => Some("bad_certificate"),
            AlertDescription::CertificateUnknown => Some("certificate_unknown"),
            _ => None,
        },
        _ => None,
    };
    match alert {
        Some(alert) => HandshakeFailure::Untrusted { alert },
        None => HandshakeFailure::Failed {
            reason: match rustls_error {
                Some(e) => e.to_string(),
                None => error.to_string(),
            },
        },
    }
}

/// The name the origin side is verified against (`phase 4 M2 design 9.2`):
/// the client's SNI, or the target when there is none; an address is one.
pub(crate) fn server_name(name: &HostName) -> Result<ServerName<'static>, HandshakeFailure> {
    match name {
        HostName::Domain(text) => {
            ServerName::try_from(text.clone()).map_err(|_| HandshakeFailure::Failed {
                reason: format!("cannot use {name} as a server name"),
            })
        }
        HostName::Ip(ip) => Ok(ServerName::IpAddress((*ip).into())),
    }
}

/// The template every decrypted connection clones (`phase 4 M2 design 8.1`).
/// `h2` decides what the client may negotiate: rustls picks by its own list's
/// order, so a client offering both gets `h2` (`the phase 4 / M2b plan's
/// P15`).
pub(crate) fn server_config(
    authority: &Arc<Authority>,
    h2: bool,
) -> Result<Arc<ServerConfig>, BuildError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| BuildError::new(format!("cannot set up TLS: {e}")))?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(NoCertificate));
    config.alpn_protocols = if h2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    config.session_storage = authority.sessions();
    // no 0-RTT: a request is replayed only when the client sends it again
    config.max_early_data_size = 0;
    Ok(Arc::new(config))
}

/// The certificate resolver of the template: it has none, and the template is
/// never used for a handshake — every connection clones it and puts the
/// certificate of its own name in.
#[derive(Debug)]
struct NoCertificate;

impl ResolvesServerCert for NoCertificate {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        None
    }
}

/// The stream a decrypted connection is served from: the TLS stream, with
/// what the client sent and whether it closed counted on the way (`phase 4
/// M2 design 8.3`).
struct Watched<S> {
    inner: S,
    watch: Arc<Watch>,
}

impl<S: AsyncRead + Unpin> AsyncRead for Watched<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let had_room = buf.remaining() > 0;
        let polled = Pin::new(&mut this.inner).poll_read(cx, buf);
        match &polled {
            Poll::Ready(Ok(())) => {
                let read = buf.filled().len() - before;
                if read > 0 {
                    this.watch.read.fetch_add(read as u64, Ordering::Relaxed);
                } else if had_room {
                    // a read that had room and gave nothing is the end of the
                    // stream: a `close_notify` reports zero bytes this way
                    this.watch.closed.store(true, Ordering::Relaxed);
                }
            }
            Poll::Ready(Err(e)) => {
                if is_closed(e.kind()) {
                    this.watch.closed.store(true, Ordering::Relaxed);
                }
            }
            Poll::Pending => {}
        }
        polled
    }
}

/// Whether an error of the stream is the client closing it (`phase 4 M2 design
/// 8.3`): a client that sends no `close_notify` shows up as the end of the
/// stream or as a reset.
fn is_closed(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Watched<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mitm::testing::{ca_p12, cfg_with_ca, generated};
    use rcgen::PKCS_ECDSA_P256_SHA256;
    use rustls::AlertDescription;
    use rustls::HandshakeKind;
    use std::net::Ipv6Addr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
    use tokio_rustls::TlsConnector;
    use tokio_rustls::client::TlsStream;

    type Client = TlsStream<DuplexStream>;

    /// A `[MITM]` whose only written key is `hostname`.
    fn cfg(hosts: &str) -> MitmConfig {
        let list = HostList::parse(hosts, Some(443));
        MitmConfig {
            hostname: list.clone(),
            effective: list,
            ..MitmConfig::default()
        }
    }

    /// A CA made by rcgen, an `Authority` over it, and the `Mitm` of `cfg`.
    /// The roots hold that CA alone: the client of a case trusts nothing
    /// else (`phase 4 M2 design 17`). The authority runs on the real clock: a
    /// client verifies on it, and no case here pins the time.
    struct Rig {
        roots: Arc<RootCertStore>,
        mitm: Mitm,
    }

    fn rig_of(cfg: MitmConfig) -> Rig {
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test MITM CA");
        let authority = Arc::new(
            Authority::load(&cfg_with_ca(&ca_p12(&ca, "pw"), "pw"), &[])
                .expect("a usable CA")
                .expect("a CA was named"),
        );
        let mut roots = RootCertStore::empty();
        roots.add(ca.cert.clone()).expect("a trust anchor");
        let roots = Arc::new(roots);
        let mitm = Mitm::build(&cfg, authority, Arc::clone(&roots)).expect("a Mitm");
        Rig { roots, mitm }
    }

    fn client_config(roots: Arc<RootCertStore>, alpn: &[&[u8]]) -> Arc<ClientConfig> {
        origin_client_config(roots, false, alpn).expect("a client configuration")
    }

    fn client_name(name: &HostName) -> ServerName<'static> {
        match name {
            HostName::Domain(d) => ServerName::try_from(d.clone()).expect("a name"),
            HostName::Ip(ip) => ServerName::IpAddress((*ip).into()),
        }
    }

    /// Both halves of a handshake over a duplex pipe: the server side goes
    /// into `accept` behind the prefix stream the sniffer leaves, the client
    /// side into a `TlsConnector` (`phase 4 M2 design 17`).
    async fn handshake(
        rig: &Rig,
        name: &HostName,
        config: Arc<ClientConfig>,
        wait: Duration,
    ) -> (Result<Accepted, HandshakeFailure>, io::Result<Client>) {
        let (client, server) = duplex(64 * 1024);
        let server = Prefixed::new(Box::new(server) as BoxedStream);
        let accept = rig.mitm.accept(server, name, wait);
        let connect = TlsConnector::from(config).connect(client_name(name), client);
        tokio::join!(accept, connect)
    }

    /// The source is asked first, then the list, then whether the name can be
    /// signed for, and the ALPN last (`phase 4 M2 design 7.1`).
    #[test]
    fn decides_in_the_design_order() {
        let local = IpAddr::from(Ipv4Addr::LOCALHOST);

        // a listed host from a source that is not allowed
        let mut blocked = cfg("ok.test");
        blocked.client_source_address = Some(SourceList::parse("10.0.0.0/8").0);
        let rig = rig_of(blocked);
        let ok = HostName::parse("ok.test");
        assert_eq!(
            rig.mitm.decides(local, Some(&ok), &ok, 443, &[]),
            Decision::Pass
        );

        // the first entry of the list that matches decides
        let rig = rig_of(cfg("-bad.test, *.test"));
        let bad = HostName::parse("bad.test");
        assert_eq!(
            rig.mitm.decides(local, Some(&bad), &bad, 443, &[]),
            Decision::Pass
        );
        assert_eq!(
            rig.mitm.decides(local, Some(&ok), &ok, 443, &[]),
            Decision::Decrypt
        );

        // with an SNI a bare entry covers every port, without one only 443
        assert_eq!(
            rig.mitm.decides(local, Some(&ok), &ok, 8443, &[]),
            Decision::Decrypt
        );
        assert_eq!(
            rig.mitm.decides(local, None, &ok, 8443, &[]),
            Decision::Pass
        );
        assert_eq!(
            rig.mitm.decides(local, None, &ok, 443, &[]),
            Decision::Decrypt
        );

        // an SNI written as an address is matched as one
        let rig = rig_of(cfg("127.0.0.1"));
        let ip = HostName::parse("127.0.0.1");
        assert_eq!(
            rig.mitm.decides(local, Some(&ip), &ip, 443, &[]),
            Decision::Decrypt
        );

        // a name rustls does not take as a server name cannot be signed for
        let odd = HostName::parse(&format!("{}.test", "a".repeat(64)));
        let rig = rig_of(cfg(&odd.to_string()));
        assert_eq!(
            rig.mitm.decides(local, Some(&odd), &odd, 443, &[]),
            Decision::Pass
        );

        // a client that offers no ALPN, or http/1.1, is decrypted
        let rig = rig_of(cfg("ok.test"));
        let ok = HostName::parse("ok.test");
        for alpn in [
            vec![],
            vec![b"http/1.1".to_vec()],
            vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        ] {
            assert_eq!(
                rig.mitm.decides(local, Some(&ok), &ok, 443, &alpn),
                Decision::Decrypt,
                "{alpn:?}"
            );
        }
        assert_eq!(
            rig.mitm
                .decides(local, Some(&ok), &ok, 443, &[b"h2".to_vec()]),
            Decision::AlpnMismatch
        );

        // the ALPN is asked last: a session an earlier step passes is passed
        // without saying that the client offers no protocol rurge decrypts
        let rig = rig_of(cfg("ok.test"));
        let other = HostName::parse("other.test");
        assert_eq!(
            rig.mitm
                .decides(local, Some(&other), &other, 443, &[b"h2".to_vec()]),
            Decision::Pass,
            "not listed"
        );
        let mut blocked = cfg("ok.test");
        blocked.client_source_address = Some(SourceList::parse("10.0.0.0/8").0);
        let rig = rig_of(blocked);
        assert_eq!(
            rig.mitm
                .decides(local, Some(&ok), &ok, 443, &[b"h2".to_vec()]),
            Decision::Pass,
            "source not allowed"
        );
        let odd = HostName::parse(&format!("{}.test", "a".repeat(64)));
        let rig = rig_of(cfg(&odd.to_string()));
        assert_eq!(
            rig.mitm
                .decides(local, Some(&odd), &odd, 443, &[b"h2".to_vec()]),
            Decision::Pass,
            "no server name"
        );
    }

    /// Sniffing widens to the ports the list writes down and to the targets
    /// it takes, never to another port of a bare entry (`phase 4 M2 design
    /// 7.6`).
    #[test]
    fn widens_on_written_ports_and_on_targets_the_list_takes() {
        let local = IpAddr::from(Ipv4Addr::LOCALHOST);

        let rig = rig_of(cfg("a.test:8443"));
        assert!(rig.mitm.widens(local, &HostName::parse("x.test"), 8443));
        assert!(!rig.mitm.widens(local, &HostName::parse("x.test"), 8444));

        // `:0` widens the target it matches, on every port
        let rig = rig_of(cfg("b.test:0"));
        assert!(rig.mitm.widens(local, &HostName::parse("b.test"), 25));
        assert!(!rig.mitm.widens(local, &HostName::parse("c.test"), 25));

        // an entry without a port does not widen another port
        let rig = rig_of(cfg("d.test"));
        assert!(!rig.mitm.widens(local, &HostName::parse("d.test"), 8443));
        assert!(rig.mitm.widens(local, &HostName::parse("d.test"), 443));

        // an excluded entry widens nothing
        let rig = rig_of(cfg("-e.test:9443"));
        assert!(!rig.mitm.widens(local, &HostName::parse("e.test"), 9443));
        assert!(!rig.mitm.widens(local, &HostName::parse("x.test"), 9443));

        // `*:0` is every target on every port
        let rig = rig_of(cfg("*:0"));
        assert!(rig.mitm.widens(local, &HostName::parse("any.test"), 25));

        // a source that is not allowed widens nothing
        let mut blocked = cfg("a.test:8443");
        blocked.client_source_address = Some(SourceList::parse("10.0.0.0/8").0);
        let rig = rig_of(blocked);
        assert!(!rig.mitm.widens(local, &HostName::parse("x.test"), 8443));
    }

    /// A loopback source and everything from the virtual interface are
    /// compared as `127.0.0.1`; anything else is its own address (`phase 4
    /// M2 design 7.2`).
    #[test]
    fn source_of_maps_loopback_and_the_virtual_interface_to_127_0_0_1() {
        let local = IpAddr::from(Ipv4Addr::LOCALHOST);
        for (listener, src) in [
            (ListenerKind::Http, SocketAddr::from(([127, 0, 0, 2], 5000))),
            (
                ListenerKind::Socks5,
                SocketAddr::from((Ipv6Addr::LOCALHOST, 5000)),
            ),
        ] {
            assert_eq!(source_of(listener, src), local, "{src}");
        }
        // an IPv4 address written in its IPv6 form
        let mapped: SocketAddr = "[::ffff:127.0.0.1]:5000".parse().expect("an address");
        assert_eq!(source_of(ListenerKind::Http, mapped), local);
        // anything through the virtual interface
        let lan = SocketAddr::from(([192, 168, 1, 2], 5000));
        assert_eq!(source_of(ListenerKind::Tun, lan), local);
        // everywhere else its own address
        assert_eq!(
            source_of(ListenerKind::Http, lan),
            IpAddr::from([192, 168, 1, 2])
        );
    }

    /// The server of a decrypted connection offers `http/1.1` and nothing
    /// else: a client that offers none negotiates none, and one that offers
    /// `h2` besides gets `http/1.1` (`phase 4 M2 design 7.4`).
    #[tokio::test]
    async fn a_client_is_served_http1_or_nothing() {
        let rig = rig_of(cfg("ok.test"));
        let name = HostName::parse("ok.test");

        let roots = Arc::clone(&rig.roots);
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(roots, &[]),
            Duration::from_secs(5),
        )
        .await;
        let accepted = accepted.expect("the handshake completes without ALPN");
        assert_eq!(
            accepted.origin.name,
            ServerName::try_from("ok.test").expect("a name")
        );
        // the origin side gets the wait the client's handshake was given
        assert_eq!(accepted.origin.wait, Duration::from_secs(5));
        let client = client.expect("the client completes");
        assert_eq!(client.get_ref().1.alpn_protocol(), None);
        assert!(!accepted.h2);

        let roots = Arc::clone(&rig.roots);
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(roots, &[b"h2", b"http/1.1"]),
            Duration::from_secs(5),
        )
        .await;
        let accepted = accepted.expect("the handshake completes with ALPN");
        assert!(!accepted.h2);
        let client = client.expect("the client completes");
        assert_eq!(client.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));
    }

    /// With `h2` on, a client that offers only `h2` is decrypted, and so is
    /// one that offers it beside `http/1.1`; a client that offers neither is
    /// passed through as it is without `h2` (`phase 4 M2 design 10`, `the
    /// phase 4 / M2b plan's P15`).
    #[test]
    fn with_h2_an_h2_only_client_is_decrypted() {
        let local = IpAddr::from(Ipv4Addr::LOCALHOST);
        let name = HostName::parse("ok.test");
        let mut with_h2 = cfg("ok.test");
        with_h2.h2 = true;
        let rig = rig_of(with_h2);

        for alpn in [
            vec![b"h2".to_vec()],
            vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            vec![b"http/1.1".to_vec()],
            vec![],
        ] {
            assert_eq!(
                rig.mitm.decides(local, Some(&name), &name, 443, &alpn),
                Decision::Decrypt,
                "{alpn:?}"
            );
        }
        assert_eq!(
            rig.mitm
                .decides(local, Some(&name), &name, 443, &[b"spdy/3".to_vec()]),
            Decision::AlpnMismatch
        );

        // without `h2` the same client is still passed through
        let rig = rig_of(cfg("ok.test"));
        assert_eq!(
            rig.mitm
                .decides(local, Some(&name), &name, 443, &[b"h2".to_vec()]),
            Decision::AlpnMismatch
        );
    }

    /// With `h2` on the server offers both protocols and picks by its own
    /// order, so a client that offers both gets `h2`; the protocol the
    /// handshake settled on is handed out with the connection (`phase 4 M2
    /// design 10.1`, `the phase 4 / M2b plan's P15`).
    #[tokio::test]
    async fn with_h2_the_client_gets_h2_when_it_offers_it() {
        let mut with_h2 = cfg("ok.test");
        with_h2.h2 = true;
        let rig = rig_of(with_h2);
        let name = HostName::parse("ok.test");

        let cases = [
            (vec![&b"h2"[..], &b"http/1.1"[..]], Some(&b"h2"[..]), true),
            (vec![&b"http/1.1"[..]], Some(&b"http/1.1"[..]), false),
            (vec![], None, false),
        ];
        for (offered, expected, h2) in cases {
            let roots = Arc::clone(&rig.roots);
            let (accepted, client) = handshake(
                &rig,
                &name,
                client_config(roots, &offered),
                Duration::from_secs(5),
            )
            .await;
            let accepted = accepted.expect("the handshake completes");
            let client = client.expect("the client completes");
            assert_eq!(
                client.get_ref().1.alpn_protocol(),
                expected,
                "the client offered {offered:?}"
            );
            assert_eq!(accepted.h2, h2, "the client offered {offered:?}");
        }
    }

    /// The origin side is offered `h2` only when `h2` is on, and a connection
    /// that upgrades or tunnels finds an `http/1.1`-only configuration either
    /// way; with `h2` off the two are one configuration (`phase 4 M2 design
    /// 10.2`, `the phase 4 / M2b plan's P12`, `P15`).
    #[tokio::test]
    async fn the_origin_side_offers_h2_only_when_h2_is_on() {
        let name = HostName::parse("ok.test");

        let mut with_h2 = cfg("ok.test");
        with_h2.h2 = true;
        let rig = rig_of(with_h2);
        let roots = Arc::clone(&rig.roots);
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(roots, &[b"http/1.1"]),
            Duration::from_secs(5),
        )
        .await;
        let accepted = accepted.expect("the handshake completes");
        client.expect("the client completes");
        assert_eq!(
            accepted.origin.config.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        assert_eq!(
            accepted.origin.h1.alpn_protocols,
            vec![b"http/1.1".to_vec()]
        );

        // `h2` off: both offer `http/1.1` alone, and are the same one
        let rig = rig_of(cfg("ok.test"));
        let roots = Arc::clone(&rig.roots);
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(roots, &[b"http/1.1"]),
            Duration::from_secs(5),
        )
        .await;
        let accepted = accepted.expect("the handshake completes");
        client.expect("the client completes");
        assert_eq!(
            accepted.origin.config.alpn_protocols,
            vec![b"http/1.1".to_vec()]
        );
        assert_eq!(
            accepted.origin.h1.alpn_protocols,
            vec![b"http/1.1".to_vec()]
        );
        assert!(
            Arc::ptr_eq(&accepted.origin.config, &accepted.origin.h1),
            "one configuration when h2 is off"
        );
    }

    /// Without an SNI the certificate names the target, here an address: the
    /// client verifies it as one (`phase 4 M2 design 6`).
    #[tokio::test]
    async fn without_sni_the_leaf_names_the_target() {
        let rig = rig_of(cfg("127.0.0.1"));
        let name = HostName::parse("127.0.0.1");
        let roots = Arc::clone(&rig.roots);
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(roots, &[]),
            Duration::from_secs(5),
        )
        .await;
        let accepted = accepted.expect("the handshake completes");
        assert_eq!(
            accepted.origin.name,
            ServerName::IpAddress(IpAddr::from(Ipv4Addr::LOCALHOST).into())
        );
        client.expect("the client verifies the certificate of the address");
    }

    /// A client that trusts another CA refuses the certificate and says why
    /// (`phase 4 M2 design 8.2`). The two CAs are named differently on
    /// purpose: a verifier looks the trust anchor up by subject first, so an
    /// equally named CA would fail the signature instead of the issuer.
    #[tokio::test]
    async fn a_client_that_does_not_trust_the_ca_sends_unknown_ca() {
        let rig = rig_of(cfg("ok.test"));
        let name = HostName::parse("ok.test");
        let other = generated(&PKCS_ECDSA_P256_SHA256, "rurge test client CA");
        let mut roots = RootCertStore::empty();
        roots.add(other.cert.clone()).expect("a trust anchor");
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(Arc::new(roots), &[]),
            Duration::from_secs(5),
        )
        .await;
        client.expect_err("the client refuses the certificate");
        let failure = accepted.err().expect("the handshake fails");
        assert_eq!(
            failure,
            HandshakeFailure::Untrusted {
                alert: "unknown_ca"
            }
        );
    }

    /// The three certificate alerts are the client not trusting the
    /// certificate; every other alert and every other error is a failure
    /// (`phase 4 M2 design 8.2`, `M2-D11`).
    #[test]
    fn the_three_certificate_alerts_are_untrusted_and_the_rest_failed() {
        for (alert, name) in [
            (AlertDescription::UnknownCA, "unknown_ca"),
            (AlertDescription::BadCertificate, "bad_certificate"),
            (AlertDescription::CertificateUnknown, "certificate_unknown"),
        ] {
            let error = io::Error::new(
                io::ErrorKind::InvalidData,
                rustls::Error::AlertReceived(alert),
            );
            assert_eq!(
                classify(&error),
                HandshakeFailure::Untrusted { alert: name },
                "{alert:?}"
            );
        }

        let alert = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(AlertDescription::HandshakeFailure),
        );
        match classify(&alert) {
            HandshakeFailure::Failed { reason } => {
                assert_eq!(
                    reason,
                    rustls::Error::AlertReceived(AlertDescription::HandshakeFailure).to_string()
                );
            }
            other => panic!("expected a failure, got {other:?}"),
        }

        let eof = io::Error::from(io::ErrorKind::UnexpectedEof);
        match classify(&eof) {
            HandshakeFailure::Failed { reason } => assert_eq!(reason, eof.to_string()),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// A client that sends its ClientHello and leaves is a failed handshake,
    /// not a client that does not trust the certificate (`phase 4 M2 design
    /// 8.2`) — and it fails when the client leaves, rather than waiting out
    /// the handshake's own limit.
    #[tokio::test]
    async fn a_client_that_leaves_mid_handshake_is_failed() {
        let rig = rig_of(cfg("ok.test"));
        let name = HostName::parse("ok.test");
        let config = client_config(Arc::clone(&rig.roots), &[]);
        let mut hello = Vec::new();
        let mut connection =
            rustls::ClientConnection::new(config, client_name(&name)).expect("a client connection");
        connection.write_tls(&mut hello).expect("a ClientHello");
        assert!(!hello.is_empty());

        let (mut client, server) = duplex(64 * 1024);
        client.write_all(&hello).await.expect("the ClientHello");
        drop(client);
        let started = std::time::Instant::now();
        let result = rig
            .mitm
            .accept(
                Prefixed::new(Box::new(server) as BoxedStream),
                &name,
                Duration::from_secs(5),
            )
            .await;
        let took = started.elapsed();
        match result {
            Err(HandshakeFailure::Failed { reason }) => {
                assert!(!reason.is_empty(), "a reason");
                assert_ne!(reason, "timed out", "the client left, it did not stall");
            }
            Err(other) => panic!("expected a failure, got {other:?}"),
            Ok(_) => panic!("the handshake completes although the client left"),
        }
        assert!(took < Duration::from_secs(2), "took {took:?}");
    }

    /// A handshake that stalls ends when its wait does, and says so (`phase 4
    /// M2 design 8.2`).
    #[tokio::test]
    async fn a_stalled_handshake_times_out() {
        let rig = rig_of(cfg("ok.test"));
        let name = HostName::parse("ok.test");
        let config = client_config(Arc::clone(&rig.roots), &[]);
        let mut hello = Vec::new();
        let mut connection =
            rustls::ClientConnection::new(config, client_name(&name)).expect("a client connection");
        connection.write_tls(&mut hello).expect("a ClientHello");

        let (mut client, server) = duplex(64 * 1024);
        client.write_all(&hello).await.expect("the ClientHello");
        // the client stays connected and says nothing more
        let started = std::time::Instant::now();
        let result = rig
            .mitm
            .accept(
                Prefixed::new(Box::new(server) as BoxedStream),
                &name,
                Duration::from_millis(300),
            )
            .await;
        let took = started.elapsed();
        assert_eq!(
            result.err(),
            Some(HandshakeFailure::Failed {
                reason: "timed out".into()
            })
        );
        assert!(took < Duration::from_secs(2), "took {took:?}");
        drop(client);
    }

    /// The watch counts the plaintext the client sent and notices when the
    /// client closed the connection (`phase 4 M2 design 8.3`).
    #[tokio::test]
    async fn the_watch_counts_plaintext_and_sees_the_client_close() {
        let rig = rig_of(cfg("ok.test"));
        let name = HostName::parse("ok.test");

        // a client that sends nothing and closes: a `close_notify`
        let roots = Arc::clone(&rig.roots);
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(roots, &[]),
            Duration::from_secs(5),
        )
        .await;
        let mut accepted = accepted.expect("the handshake completes");
        let mut client = client.expect("the client completes");
        client.shutdown().await.expect("the close_notify");
        drop(client);
        let mut buf = [0u8; 64];
        let n = accepted.stream.read(&mut buf).await.expect("a read");
        assert_eq!(n, 0, "the end of the stream");
        assert_eq!(accepted.watch.read(), 0);
        assert!(accepted.watch.closed_by_client());

        // a client that sends three bytes and stays
        let roots = Arc::clone(&rig.roots);
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(roots, &[]),
            Duration::from_secs(5),
        )
        .await;
        let mut accepted = accepted.expect("the handshake completes");
        let mut client = client.expect("the client completes");
        client.write_all(b"GET").await.expect("three bytes");
        let mut got = [0u8; 3];
        accepted
            .stream
            .read_exact(&mut got)
            .await
            .expect("three bytes");
        assert_eq!(&got, b"GET");
        assert_eq!(accepted.watch.read(), 3);
        assert!(!accepted.watch.closed_by_client());

        // a client that closes without a `close_notify`: the stream ends
        // with an error, and that is the client closing too
        let roots = Arc::clone(&rig.roots);
        let (accepted, client) = handshake(
            &rig,
            &name,
            client_config(roots, &[]),
            Duration::from_secs(5),
        )
        .await;
        let mut accepted = accepted.expect("the handshake completes");
        let client = client.expect("the client completes");
        drop(client);
        let mut buf = [0u8; 64];
        let error = accepted
            .stream
            .read(&mut buf)
            .await
            .expect_err("the end of the stream is an error");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "{error}");
        assert_eq!(accepted.watch.read(), 0);
        assert!(accepted.watch.closed_by_client());
    }

    /// Two connections of one authority, one name and one client
    /// configuration resume the same session: the `Authority`'s session
    /// cache is shared by the server configuration of every connection
    /// (`phase 4 M2 design 8.1`).
    #[tokio::test]
    async fn a_second_connection_resumes_its_session() {
        let rig = rig_of(cfg("resume.test"));
        let name = HostName::parse("resume.test");
        let config = client_config(Arc::clone(&rig.roots), &[b"http/1.1"]);

        // the client reads to the end, so that the ticket of the first
        // session is processed
        let (accepted, client) =
            handshake(&rig, &name, Arc::clone(&config), Duration::from_secs(5)).await;
        let mut accepted = accepted.expect("the first handshake");
        let mut client = client.expect("the first client");
        accepted.stream.shutdown().await.expect("a close");
        drop(accepted);
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).await.expect("the end");
        drop(client);

        let (accepted, client) = handshake(&rig, &name, config, Duration::from_secs(5)).await;
        accepted.expect("the second handshake");
        let client = client.expect("the second client");
        assert_eq!(
            client.get_ref().1.handshake_kind(),
            Some(HandshakeKind::Resumed)
        );
    }
}
