//! TLS towards a proxy: the six TLS parameters of the compatibility matrix §4.4.

use crate::BuildError;
use rurge_config::HostName;
use rurge_config::spec::{Sni, TlsOpts};
use rurge_net::connector::BoxedStream;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};
use std::io;
use std::sync::Arc;
use tokio_rustls::TlsConnector;

/// A client certificate chain (leaf first) and its private key.
pub struct ClientIdentity {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

#[derive(Debug)]
enum Mode {
    Standard {
        inner: Arc<WebPkiServerVerifier>,
        /// `server-cert-verify-name`: verified instead of the connection's name.
        verify_name: Option<ServerName<'static>>,
    },
    /// SHA-256 of the leaf certificate; replaces chain validation (manual).
    Pinned([u8; 32]),
    Insecure,
}

#[derive(Debug)]
struct Verifier {
    mode: Mode,
    provider: Arc<CryptoProvider>,
}

fn same(a: &[u8], b: &[u8]) -> bool {
    // compares every byte, no early exit on the first difference
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.mode {
            Mode::Standard { inner, verify_name } => inner.verify_server_cert(
                end_entity,
                intermediates,
                verify_name.as_ref().unwrap_or(server_name),
                ocsp_response,
                now,
            ),
            Mode::Pinned(expected) => {
                if same(&Sha256::digest(end_entity.as_ref()), expected) {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(rustls::Error::General(
                        "the server certificate does not match server-cert-fingerprint-sha256"
                            .into(),
                    ))
                }
            }
            Mode::Insecure => Ok(ServerCertVerified::assertion()),
        }
    }

    // Every mode still proves that the peer holds the certificate's key.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn dns_name(name: &str) -> Result<ServerName<'static>, BuildError> {
    ServerName::try_from(name.to_string())
        .map_err(|_| BuildError::new(format!("`{name}` is not a valid TLS server name")))
}

/// Everything that can be prepared ahead of a connection: built once per
/// outbound, used for every handshake.
pub struct TlsClient {
    connector: TlsConnector,
    name: ServerName<'static>,
}

impl TlsClient {
    /// `server` is the proxy's host. `default_alpn` applies when the policy
    /// sets none. `roots` is only consulted by standard verification.
    pub fn build(
        opts: &TlsOpts,
        server: &HostName,
        default_alpn: &[&str],
        identity: Option<ClientIdentity>,
        roots: Arc<RootCertStore>,
    ) -> Result<TlsClient, BuildError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let name = match (&opts.sni, server) {
            (Sni::Name(custom), _) => dns_name(custom)?,
            (_, HostName::Domain(d)) => dns_name(d)?,
            // rustls sends no SNI for an IP address
            (_, HostName::Ip(ip)) => ServerName::IpAddress((*ip).into()),
        };
        // parsed whatever the mode: a name that is not one is a build error
        let verify_name = opts.verify_name.as_deref().map(dns_name).transpose()?;
        let mode = if let Some(fingerprint) = opts.fingerprint_sha256 {
            Mode::Pinned(fingerprint)
        } else if opts.skip_cert_verify {
            Mode::Insecure
        } else {
            let inner = WebPkiServerVerifier::builder_with_provider(roots, provider.clone())
                .build()
                .map_err(|e| {
                    BuildError::new(format!("cannot set up certificate verification: {e}"))
                })?;
            Mode::Standard { inner, verify_name }
        };
        let builder = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| BuildError::new(format!("cannot set up TLS: {e}")))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Verifier { mode, provider }));
        let mut config = match identity {
            Some(identity) => builder
                .with_client_auth_cert(identity.chain, identity.key)
                .map_err(|e| {
                    BuildError::new(format!("the client certificate cannot be used: {e}"))
                })?,
            None => builder.with_no_client_auth(),
        };
        config.enable_sni = opts.sni != Sni::Off;
        config.alpn_protocols = if opts.alpn.is_empty() {
            default_alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
        } else {
            opts.alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
        };
        Ok(TlsClient {
            connector: TlsConnector::from(Arc::new(config)),
            name,
        })
    }

    pub async fn wrap(&self, stream: BoxedStream) -> io::Result<BoxedStream> {
        let tls = self.connector.connect(self.name.clone(), stream).await?;
        Ok(Box::new(tls))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{SeenHandshake, TlsFixture};
    use rurge_config::spec::{Sni, TlsOpts};
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    fn empty_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    /// Handshakes with the fixture's echo server and proves the tunnel works.
    async fn talk(
        addr: SocketAddr,
        opts: &TlsOpts,
        host: &str,
        roots: Arc<RootCertStore>,
    ) -> io::Result<()> {
        let client = TlsClient::build(opts, &HostName::parse(host), &[], None, roots)
            .expect("the options build");
        let tcp = TcpStream::connect(addr).await?;
        let mut stream = client.wrap(Box::new(tcp)).await?;
        stream.write_all(b"ping").await?;
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await?;
        assert_eq!(&buf, b"ping");
        Ok(())
    }

    async fn last_seen(fixture: &TlsFixture, count: usize) -> SeenHandshake {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while fixture.seen().len() < count {
            assert!(
                tokio::time::Instant::now() < deadline,
                "handshake never recorded"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        fixture.seen()[count - 1].clone()
    }

    #[tokio::test]
    async fn standard_verification_uses_the_server_name() {
        let fixture = TlsFixture::new(&["localhost", "proxy.test"]);
        let addr = fixture.spawn_echo(false).await;
        talk(addr, &TlsOpts::default(), "localhost", fixture.roots())
            .await
            .unwrap();
        assert_eq!(
            last_seen(&fixture, 1).await.sni.as_deref(),
            Some("localhost")
        );
        // a name the certificate does not cover
        let err = talk(addr, &TlsOpts::default(), "wrong.test", fixture.roots())
            .await
            .unwrap_err();
        // rustls 0.23.43's prose for `CertificateError::NotValidForNameContext`
        // (the `Display` impl spells it out; the variant name itself never
        // appears in the text)
        assert!(err.to_string().contains("not valid for name"), "{err}");
        // an unknown CA
        assert!(
            talk(
                addr,
                &TlsOpts::default(),
                "localhost",
                empty_roots_with_one_other_ca()
            )
            .await
            .is_err()
        );
    }

    fn empty_roots_with_one_other_ca() -> Arc<RootCertStore> {
        TlsFixture::new(&["other.test"]).roots()
    }

    #[tokio::test]
    async fn verify_name_and_sni_are_independent() {
        let fixture = TlsFixture::new(&["localhost", "proxy.test"]);
        let addr = fixture.spawn_echo(false).await;
        // verify against another name than the one connected to
        let opts = TlsOpts {
            verify_name: Some("localhost".into()),
            ..TlsOpts::default()
        };
        talk(addr, &opts, "wrong.test", fixture.roots())
            .await
            .unwrap();
        assert_eq!(
            last_seen(&fixture, 1).await.sni.as_deref(),
            Some("wrong.test")
        );
        // a custom SNI is what gets sent, and what gets verified by default
        let opts = TlsOpts {
            sni: Sni::Name("proxy.test".into()),
            ..TlsOpts::default()
        };
        talk(addr, &opts, "wrong.test", fixture.roots())
            .await
            .unwrap();
        assert_eq!(
            last_seen(&fixture, 2).await.sni.as_deref(),
            Some("proxy.test")
        );
        // sni = off: nothing is sent, verification still uses the host name
        let opts = TlsOpts {
            sni: Sni::Off,
            ..TlsOpts::default()
        };
        talk(addr, &opts, "localhost", fixture.roots())
            .await
            .unwrap();
        assert_eq!(last_seen(&fixture, 3).await.sni, None);
    }

    #[tokio::test]
    async fn an_ip_literal_sends_no_sni() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let addr = fixture.spawn_echo(false).await;
        talk(addr, &TlsOpts::default(), "127.0.0.1", fixture.roots())
            .await
            .unwrap();
        assert_eq!(last_seen(&fixture, 1).await.sni, None);
    }

    #[tokio::test]
    async fn a_pinned_fingerprint_replaces_chain_validation() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(false).await;
        let pinned = TlsOpts {
            fingerprint_sha256: Some(fixture.leaf_fingerprint()),
            // loses against the fingerprint
            skip_cert_verify: true,
            ..TlsOpts::default()
        };
        // no trust anchors and a name the certificate does not cover: still fine
        talk(addr, &pinned, "wrong.test", empty_roots())
            .await
            .unwrap();
        let wrong = TlsOpts {
            fingerprint_sha256: Some([7u8; 32]),
            skip_cert_verify: true,
            ..TlsOpts::default()
        };
        let err = talk(addr, &wrong, "localhost", fixture.roots())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("server-cert-fingerprint-sha256"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn skip_cert_verify_accepts_anything() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(false).await;
        let opts = TlsOpts {
            skip_cert_verify: true,
            ..TlsOpts::default()
        };
        talk(addr, &opts, "wrong.test", empty_roots())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn alpn_comes_from_the_options_or_the_protocol_default() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(false).await;
        let opts = TlsOpts {
            alpn: vec!["http/1.1".into()],
            ..TlsOpts::default()
        };
        talk(addr, &opts, "localhost", fixture.roots())
            .await
            .unwrap();
        assert_eq!(
            last_seen(&fixture, 1).await.alpn.as_deref(),
            Some("http/1.1")
        );
        let client = TlsClient::build(
            &TlsOpts::default(),
            &HostName::parse("localhost"),
            &["h2"],
            None,
            fixture.roots(),
        )
        .unwrap();
        let tcp = TcpStream::connect(addr).await.unwrap();
        let _stream = client.wrap(Box::new(tcp)).await.unwrap();
        assert_eq!(last_seen(&fixture, 2).await.alpn.as_deref(), Some("h2"));
    }

    #[test]
    fn unusable_names_are_build_errors() {
        let opts = TlsOpts {
            sni: Sni::Name("not a name".into()),
            ..TlsOpts::default()
        };
        let err = TlsClient::build(
            &opts,
            &HostName::parse("localhost"),
            &[],
            None,
            empty_roots(),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "`not a name` is not a valid TLS server name");
        // standard verification needs trust anchors
        let err = TlsClient::build(
            &TlsOpts::default(),
            &HostName::parse("localhost"),
            &[],
            None,
            empty_roots(),
        )
        .map(|_| ())
        .unwrap_err();
        assert!(
            err.message
                .starts_with("cannot set up certificate verification"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_bad_verify_name_is_a_build_error_in_every_mode() {
        for (skip, fingerprint) in [(false, None), (true, None), (false, Some([7u8; 32]))] {
            let opts = TlsOpts {
                skip_cert_verify: skip,
                fingerprint_sha256: fingerprint,
                verify_name: Some("not a name".into()),
                ..TlsOpts::default()
            };
            let err = TlsClient::build(
                &opts,
                &HostName::parse("proxy.example"),
                &[],
                None,
                // standard verification cannot even be set up without a root
                TlsFixture::new(&["proxy.example"]).roots(),
            )
            .err()
            .expect("the name is refused");
            assert_eq!(err.message, "`not a name` is not a valid TLS server name");
        }
    }

    /// `DigitallySignedStruct::new` is `pub(crate)` in rustls 0.23.43, so the
    /// handshake-signature verifiers cannot be unit-tested with a hand-made
    /// struct: this proves the behaviour instead, against a server that
    /// presents a genuine, trusted leaf certificate but cannot prove it
    /// holds the matching private key. Every mode must still refuse it,
    /// because `verify_tls12_signature` / `verify_tls13_signature` are what
    /// prove key possession; `verify_server_cert` (or its absence, in the
    /// pinned / insecure modes) only ever validates the certificate itself.
    #[tokio::test]
    async fn a_server_without_the_private_key_is_refused_in_every_mode() {
        let fixture = TlsFixture::new(&["localhost"]);
        for versions in [
            [&rustls::version::TLS13].as_slice(),
            [&rustls::version::TLS12].as_slice(),
        ] {
            let addr = fixture.spawn_impostor(versions).await;
            let standard = TlsOpts::default();
            let pinned = TlsOpts {
                fingerprint_sha256: Some(fixture.leaf_fingerprint()),
                ..TlsOpts::default()
            };
            let insecure = TlsOpts {
                skip_cert_verify: true,
                ..TlsOpts::default()
            };
            for (opts, roots) in [
                (&standard, fixture.roots()),
                (&pinned, empty_roots()),
                (&insecure, empty_roots()),
            ] {
                let err = talk(addr, opts, "localhost", roots).await.unwrap_err();
                // rustls 0.23.43's `CertificateError::BadSignature` has no
                // custom `Display` arm, so it falls back to `{:?}`, which for
                // a unit variant is just its name: this substring is stable
                // and appears nowhere else in rustls' error text.
                assert!(err.to_string().contains("BadSignature"), "{err}");
            }
        }
    }
}
