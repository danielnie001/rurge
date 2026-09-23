//! A throw-away CA, a server certificate and a TLS acceptor that records
//! what each client sent.

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rurge_net::connector::BoxedStream;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::{CertifiedKey, SigningKey};
use rustls::{RootCertStore, ServerConfig, SupportedProtocolVersion};
use sha2::{Digest, Sha256};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

/// Always resolves to the fixture's real leaf certificate, but signed with a
/// key that does not match it: what `TlsFixture::impostor_acceptor` serves.
#[derive(Debug)]
struct ImpostorResolver {
    leaf: CertificateDer<'static>,
    key: Arc<dyn SigningKey>,
}

impl ResolvesServerCert for ImpostorResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        // `CertifiedKey::new` performs no key / certificate consistency
        // check (unlike `with_single_cert`): exactly the mismatch this
        // fixture exists to serve.
        Some(Arc::new(CertifiedKey::new(
            vec![self.leaf.clone()],
            self.key.clone(),
        )))
    }
}

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

    pub fn ca_pem(&self) -> String {
        pem("CERTIFICATE", self.ca.as_ref())
    }

    pub fn leaf_pem(&self) -> String {
        pem("CERTIFICATE", self.leaf.as_ref())
    }

    /// PKCS#8.
    pub fn leaf_key_pem(&self) -> String {
        pem("PRIVATE KEY", &self.leaf_key)
    }

    /// A client certificate signed by the fixture's CA as a Base64 PKCS#12
    /// with the password `pw`: the value of a `[Keystore]` item.
    pub fn client_p12_base64(&self, common_name: &str) -> String {
        use p12_keystore::{Certificate, KeyStore, KeyStoreEntry, PrivateKeyChain};
        let (cert, key) = self.issue_client(common_name);
        let chain = PrivateKeyChain::new(
            key,
            [1u8, 2, 3, 4],
            [Certificate::from_der(&cert).expect("our own certificate")],
        );
        let mut store = KeyStore::new();
        store.add_entry("client", KeyStoreEntry::PrivateKeyChain(chain));
        let der = store.writer("pw").write().expect("p12");
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der)
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

    /// What a Shadow TLS server relays a handshake to: the given protocol
    /// versions, no ALPN, and `tickets` session tickets after every TLS 1.3
    /// handshake (rustls' own default is 2).
    pub fn camouflage_acceptor(
        &self,
        versions: &[&'static SupportedProtocolVersion],
        tickets: usize,
    ) -> TlsAcceptor {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.leaf_key.clone()));
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![self.leaf.clone()], key)
            .expect("server certificate");
        config.send_tls13_tickets = tickets;
        TlsAcceptor::from(Arc::new(config))
    }

    /// A server that presents the fixture's real leaf certificate but signs
    /// the handshake with a DIFFERENT key: what an attacker holding a copy
    /// of the (public) certificate can do. Every verification mode must
    /// refuse it.
    pub fn impostor_acceptor(&self, versions: &[&'static SupportedProtocolVersion]) -> TlsAcceptor {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        // a fresh key, unrelated to `self.leaf_key`; same algorithm as the
        // leaf (rcgen's default, ECDSA P-256), so the handshake fails at the
        // signature and not at scheme selection
        let impostor_key = KeyPair::generate().expect("impostor key");
        let der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(impostor_key.serialize_der()));
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&der)
            .expect("signing key from a fresh keypair");
        let resolver = ImpostorResolver {
            leaf: self.leaf.clone(),
            key: signing_key,
        };
        let config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .expect("protocol versions")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver));
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

    /// `seen()`, once it holds at least `count` handshakes: the server side
    /// writes one down after the client already has its stream. Panics when
    /// they do not show up within five seconds.
    pub async fn seen_at_least(&self, count: usize) -> Vec<SeenHandshake> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let seen = self.seen();
            if seen.len() >= count {
                return seen;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{} of {count} handshakes were recorded",
                seen.len()
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Runs the echo accept loop behind `acceptor`; failed handshakes are
    /// dropped silently. Shared by `spawn_echo` and `spawn_impostor` so the
    /// loop is not duplicated.
    async fn spawn_with_acceptor(self: &Arc<Self>, acceptor: TlsAcceptor) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let fixture = self.clone();
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (fixture, acceptor) = (fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(mut stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stream.read(&mut buf).await {
                        // `flush`: tokio-rustls reports a write as done while
                        // ciphertext may still sit in its own buffer, and
                        // with nothing more to echo that tail would stay there
                        if n == 0
                            || stream.write_all(&buf[..n]).await.is_err()
                            || stream.flush().await.is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }

    /// A TLS echo server on a loopback port; failed handshakes are dropped silently.
    pub async fn spawn_echo(self: &Arc<Self>, require_client_cert: bool) -> SocketAddr {
        self.spawn_with_acceptor(self.acceptor(require_client_cert))
            .await
    }

    /// The same echo server, but behind `impostor_acceptor(versions)`.
    pub async fn spawn_impostor(
        self: &Arc<Self>,
        versions: &[&'static SupportedProtocolVersion],
    ) -> SocketAddr {
        self.spawn_with_acceptor(self.impostor_acceptor(versions))
            .await
    }
}

/// RFC 7468 text for a DER blob.
fn pem(label: &str, der: &[u8]) -> String {
    let body = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_and_p12_exports_are_well_formed() {
        let fixture = TlsFixture::new(&["localhost", "127.0.0.1"]);
        for (pem, label) in [
            (fixture.ca_pem(), "CERTIFICATE"),
            (fixture.leaf_pem(), "CERTIFICATE"),
            (fixture.leaf_key_pem(), "PRIVATE KEY"),
        ] {
            assert!(
                pem.starts_with(&format!("-----BEGIN {label}-----\n")),
                "{pem}"
            );
            assert!(pem.ends_with(&format!("-----END {label}-----\n")), "{pem}");
            assert!(
                pem.lines().all(|l| l.len() <= 64),
                "lines are wrapped at 64"
            );
        }
        let item = rurge_config::KeystoreItem {
            name: "mtls".into(),
            kind: rurge_config::KeystoreType::P12,
            base64: fixture.client_p12_base64("interop client"),
            password: Some("pw".into()),
            unknown: Vec::new(),
            span: rurge_config::Span::new(std::sync::Arc::from(std::path::Path::new("t.conf")), 1),
        };
        crate::keystore::decode_p12(&item).expect("our own p12 decodes");
    }
}
