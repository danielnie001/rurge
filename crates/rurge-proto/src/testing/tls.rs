//! A throw-away CA, a server certificate and a TLS acceptor that records
//! what each client sent.

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rurge_net::connector::BoxedStream;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use sha2::{Digest, Sha256};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenHandshake {
    pub sni: Option<String>,
    pub alpn: Option<String>,
    pub client_cert: bool,
}

pub struct TlsFixture {
    issuer: Issuer<'static, KeyPair>,
    ca: CertificateDer<'static>,
    leaf: CertificateDer<'static>,
    /// PKCS#8
    leaf_key: Vec<u8>,
    seen: Arc<Mutex<Vec<SeenHandshake>>>,
}

impl TlsFixture {
    /// A fresh CA and a server certificate for `names` (DNS names or IP literals).
    pub fn new(names: &[&str]) -> Arc<TlsFixture> {
        let ca_key = KeyPair::generate().expect("ca key");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "rurge test CA");
        let ca = ca_params
            .self_signed(&ca_key)
            .expect("ca cert")
            .der()
            .clone();
        let issuer = Issuer::new(ca_params, ca_key);
        let leaf_key = KeyPair::generate().expect("leaf key");
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let leaf = CertificateParams::new(names)
            .expect("leaf params")
            .signed_by(&leaf_key, &issuer)
            .expect("leaf cert")
            .der()
            .clone();
        Arc::new(TlsFixture {
            issuer,
            ca,
            leaf,
            leaf_key: leaf_key.serialize_der(),
            seen: Arc::default(),
        })
    }

    /// Trust anchors holding only the fixture's CA.
    pub fn roots(&self) -> Arc<RootCertStore> {
        let mut roots = RootCertStore::empty();
        roots
            .add(self.ca.clone())
            .expect("ca is a valid trust anchor");
        Arc::new(roots)
    }

    /// SHA-256 of the server certificate (DER).
    pub fn leaf_fingerprint(&self) -> [u8; 32] {
        Sha256::digest(self.leaf.as_ref()).into()
    }

    /// A client certificate signed by the fixture's CA: (certificate DER, PKCS#8 key DER).
    pub fn issue_client(&self, common_name: &str) -> (Vec<u8>, Vec<u8>) {
        let key = KeyPair::generate().expect("client key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("client params");
        params
            .distinguished_name
            .push(DnType::CommonName, common_name);
        let cert = params.signed_by(&key, &self.issuer).expect("client cert");
        (cert.der().to_vec(), key.serialize_der())
    }

    /// With `require_client_cert`, only clients presenting a certificate
    /// signed by the fixture's CA complete the handshake.
    pub fn acceptor(&self, require_client_cert: bool) -> TlsAcceptor {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("protocol versions");
        let builder = if require_client_cert {
            let verifier = WebPkiClientVerifier::builder_with_provider(self.roots(), provider)
                .build()
                .expect("client verifier");
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.leaf_key.clone()));
        let mut config = builder
            .with_single_cert(vec![self.leaf.clone()], key)
            .expect("server certificate");
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        TlsAcceptor::from(Arc::new(config))
    }

    /// Completes the server side of a handshake and records what the client sent.
    pub async fn accept(&self, acceptor: &TlsAcceptor, tcp: TcpStream) -> io::Result<BoxedStream> {
        let tls = acceptor.accept(tcp).await?;
        let (_, conn) = tls.get_ref();
        self.seen.lock().expect("seen").push(SeenHandshake {
            sni: conn.server_name().map(str::to_string),
            alpn: conn
                .alpn_protocol()
                .map(|p| String::from_utf8_lossy(p).into_owned()),
            client_cert: conn.peer_certificates().is_some(),
        });
        Ok(Box::new(tls))
    }

    pub fn seen(&self) -> Vec<SeenHandshake> {
        self.seen.lock().expect("seen").clone()
    }

    /// A TLS echo server on a loopback port; failed handshakes are dropped silently.
    pub async fn spawn_echo(self: &Arc<Self>, require_client_cert: bool) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let fixture = self.clone();
        let acceptor = self.acceptor(require_client_cert);
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (fixture, acceptor) = (fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(mut stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }
}
