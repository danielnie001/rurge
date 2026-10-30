//! The TLS of the API (`http-api-tls`, phase 4 M2 design 14.1): the same
//! template the decrypted connections are served from, with a certificate of
//! the authority's own cache per name or address, and what a client of the
//! API trusts.

use crate::mitm::authority::{Authority, LeafError};
use crate::mitm::decrypt::server_config;
use rurge_config::HostName;
use rurge_proto::BuildError;
use rustls::pki_types::CertificateDer;
use rustls::sign::SingleCertAndKey;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::fmt;
use std::sync::Arc;

/// The TLS of the API (`http-api-tls`, phase 4 M2 design 14.1).
#[derive(Clone)]
pub struct ApiTls {
    authority: Arc<Authority>,
    template: Arc<ServerConfig>,
}

impl ApiTls {
    /// The TLS of `authority`: the template every connection clones, and the
    /// authority its certificates and sessions come from.
    pub fn new(authority: Arc<Authority>) -> Result<ApiTls, BuildError> {
        // the same template the decrypted connections are served from when
        // they do not offer `h2` (phase 4 M2 design 8.1, `14.1`)
        let template = server_config(&authority, false)?;
        Ok(ApiTls {
            authority,
            template,
        })
    }

    /// The configuration of one connection: the template with the
    /// certificate of `name`, from the authority's cache (phase 4 M2 design 6).
    pub async fn config(&self, name: &HostName) -> Result<Arc<ServerConfig>, LeafError> {
        let leaf = self.authority.leaf(name).await?;
        // the template is shared; only the certificate of this connection is
        // its own, and the leaf and session caches behind it are the same
        // authority's the decrypted connections sign and resume from
        let mut config = (*self.template).clone();
        config.cert_resolver = Arc::new(SingleCertAndKey::from(leaf));
        Ok(Arc::new(config))
    }

    /// The CA certificate (DER) the API's certificates chain to.
    pub fn ca_der(&self) -> &[u8] {
        self.authority.ca_der()
    }
}

impl fmt::Debug for ApiTls {
    /// The authority's `Debug` alone: a subject and four bytes of a
    /// fingerprint, nothing of the material.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ApiTls").field(&self.authority).finish()
    }
}

/// What a client of the API trusts: this CA alone (`rurge status` over
/// HTTPS, and the tests).
pub fn client_config(ca_der: &[u8]) -> Result<Arc<ClientConfig>, BuildError> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca_der.to_vec()))
        .map_err(|e| BuildError::new(format!("the MITM CA cannot be trusted: {e}")))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
        .map_err(|e| BuildError::new(format!("cannot set up TLS: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    // the API is an HTTP/1.1 server, so the client offers nothing else
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mitm::testing::{ca_p12, cfg_with_ca, generated};
    use rcgen::PKCS_ECDSA_P256_SHA256;
    use rustls::pki_types::ServerName;
    use std::io;
    use std::net::{IpAddr, Ipv4Addr};
    use std::time::Duration;
    use tokio::io::{DuplexStream, duplex};
    use tokio_rustls::TlsAcceptor;
    use tokio_rustls::TlsConnector;
    use tokio_rustls::client::TlsStream;
    use x509_parser::extensions::GeneralName;
    use x509_parser::parse_x509_certificate;

    type Client = TlsStream<DuplexStream>;
    type Server = tokio_rustls::server::TlsStream<DuplexStream>;

    /// An `Authority` over a CA made by rcgen, and the TLS of an API signing
    /// with it. The authority runs on the real clock: a client verifies on it,
    /// and no case here pins the time.
    fn api_of(cn: &str) -> (Arc<Authority>, ApiTls) {
        let ca = generated(&PKCS_ECDSA_P256_SHA256, cn);
        let authority = Arc::new(
            Authority::load(&cfg_with_ca(&ca_p12(&ca, "pw"), "pw"), &[])
                .expect("a usable CA")
                .expect("a CA was named"),
        );
        let api = ApiTls::new(Arc::clone(&authority)).expect("the TLS of the API");
        (authority, api)
    }

    /// The name a client asks for, as rustls takes it.
    fn client_name(name: &HostName) -> ServerName<'static> {
        match name {
            HostName::Domain(d) => ServerName::try_from(d.clone()).expect("a name"),
            HostName::Ip(ip) => ServerName::IpAddress((*ip).into()),
        }
    }

    /// Both halves of one handshake over a duplex pipe: the server side is
    /// `api`'s configuration of `name`, the client side the one trusting
    /// `client`. The whole exchange is bounded, as every TLS step of these
    /// cases is (phase 4 M2 design 17).
    async fn handshake(
        api: &ApiTls,
        name: &HostName,
        client: Arc<ClientConfig>,
    ) -> (io::Result<Server>, io::Result<Client>) {
        let (client_io, server_io) = duplex(64 * 1024);
        let config = api
            .config(name)
            .await
            .expect("a configuration for the name");
        let accept = TlsAcceptor::from(config).accept(server_io);
        let connect = TlsConnector::from(client).connect(client_name(name), client_io);
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(accept, connect)
        })
        .await
        .expect("the handshake finishes within the bound")
    }

    /// The subject alternative names of the leaf `stream` was served, an
    /// address written the way `IpAddr` writes it.
    fn san_of(stream: &Client) -> Vec<String> {
        let leaf = stream
            .get_ref()
            .1
            .peer_certificates()
            .expect("a certificate")[0]
            .clone();
        let (_, cert) = parse_x509_certificate(leaf.as_ref()).expect("a certificate");
        let san = cert
            .subject_alternative_name()
            .expect("the extensions parse")
            .expect("a subject alternative name");
        san.value
            .general_names
            .iter()
            .map(|name| match name {
                GeneralName::DNSName(name) => name.to_string(),
                GeneralName::IPAddress(raw) => {
                    let bytes = <[u8; 4]>::try_from(*raw).expect("an IPv4 address");
                    IpAddr::V4(bytes.into()).to_string()
                }
                other => panic!("a name of another kind: {other:?}"),
            })
            .collect()
    }

    /// The API serves a certificate for the address or the name the client
    /// asked for, offering `http/1.1` alone (phase 4 M2 design 14.1, `6`).
    #[tokio::test]
    async fn the_api_is_served_a_certificate_for_its_name_or_address() {
        let (authority, api) = api_of("rurge test API CA");
        let roots = client_config(authority.ca_der()).expect("a client configuration");

        // a client that connects by address: rustls sends no SNI for one, and
        // the API names the address in the leaf
        let address = HostName::Ip(IpAddr::from(Ipv4Addr::LOCALHOST));
        let (accepted, connected) = handshake(&api, &address, Arc::clone(&roots)).await;
        accepted.expect("the server completes the handshake");
        let tls = connected.expect("the client trusts the CA, and the leaf names the address");
        assert_eq!(
            tls.get_ref().1.alpn_protocol(),
            Some(&b"http/1.1"[..]),
            "the API speaks HTTP/1.1"
        );
        assert_eq!(
            san_of(&tls),
            ["127.0.0.1"],
            "the leaf names the address it was asked for"
        );

        let domain = HostName::Domain("api.test".to_string());
        let (accepted, connected) = handshake(&api, &domain, roots).await;
        accepted.expect("the server completes the handshake");
        let tls = connected.expect("the client trusts the CA, and the leaf names the domain");
        assert_eq!(
            san_of(&tls),
            ["api.test"],
            "the leaf names the domain it was asked for"
        );
    }

    /// The template every connection clones offers `http/1.1` and no early
    /// data, as the `h2`-off template of the decrypted connections does
    /// (phase 4 M2 design 14.1).
    #[test]
    fn the_api_template_offers_http1_alone_and_no_early_data() {
        let (_authority, api) = api_of("rurge test API CA");
        assert_eq!(api.template.alpn_protocols, vec![b"http/1.1".to_vec()]);
        assert_eq!(api.template.max_early_data_size, 0, "no 0-RTT");
    }

    /// A client that trusts another CA is refused (phase 4 M2 design 14.2:
    /// the command line trusts the CA the profile names and nothing else).
    /// The other CA has another subject: webpki takes a certificate of the
    /// same subject for the issuer and reports `BadSignature` instead of
    /// `UnknownIssuer`, which is not the refusal this case is about.
    #[tokio::test]
    async fn a_client_trusting_another_ca_is_refused() {
        let (_authority, api) = api_of("rurge test API CA");
        let other = generated(&PKCS_ECDSA_P256_SHA256, "rurge test other CA");
        let roots = client_config(other.cert.as_ref()).expect("a client configuration");
        let name = HostName::Domain("api.test".to_string());
        let (_accepted, connected) = handshake(&api, &name, roots).await;
        assert!(
            connected.is_err(),
            "the client does not trust the CA the API signs with"
        );
    }

    /// The API's connections and the decrypted ones sign from one cache:
    /// three asks of one name, one signature (phase 4 M2 design 6, `14.1`).
    #[tokio::test]
    async fn the_api_and_the_decrypted_connections_share_the_leaves() {
        let (authority, api) = api_of("rurge test API CA");
        let name = HostName::Domain("api.test".to_string());
        api.config(&name).await.expect("a configuration");
        api.config(&name).await.expect("a configuration");
        authority.leaf(&name).await.expect("a leaf");
        assert_eq!(authority.signature_count(), 1, "signed once");
    }

    /// What is not a certificate is refused before any connection is made
    /// (phase 4 M2 design 14.2).
    #[test]
    fn client_config_wants_a_certificate() {
        assert!(client_config(b"not a certificate").is_err());
    }
}
