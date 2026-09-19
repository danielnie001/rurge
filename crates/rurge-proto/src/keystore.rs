//! `[Keystore]` material decoded at build time.

use crate::BuildError;
use crate::transport::tls::ClientIdentity;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use rurge_config::KeystoreItem;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// Decodes a `p12` keystore item into a client identity (leaf first).
/// The error names the item; it never repeats the password or the material.
pub fn decode_p12(item: &KeystoreItem) -> Result<ClientIdentity, BuildError> {
    let name = &item.name;
    let der = STANDARD
        .decode(&item.base64)
        .or_else(|_| STANDARD_NO_PAD.decode(&item.base64))
        .map_err(|_| {
            BuildError::new(format!(
                "keystore item `{name}`: `base64` is not valid Base64"
            ))
        })?;
    let store = p12_keystore::KeyStore::from_pkcs12(&der, item.password.as_deref().unwrap_or(""))
        .map_err(|e| {
        BuildError::new(format!(
            "keystore item `{name}` cannot be decoded (wrong password or unsupported PKCS#12): {e}"
        ))
    })?;
    let (_, chain) = store.private_key_chain().ok_or_else(|| {
        BuildError::new(format!(
            "keystore item `{name}` holds no private key with a certificate"
        ))
    })?;
    if chain.chain().is_empty() {
        return Err(BuildError::new(format!(
            "keystore item `{name}` holds no certificate"
        )));
    }
    Ok(ClientIdentity {
        chain: chain
            .chain()
            .iter()
            .map(|c| CertificateDer::from(c.as_der().to_vec()))
            .collect(),
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(chain.key().to_vec())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TlsFixture;
    use crate::transport::tls::TlsClient;
    use base64::engine::general_purpose::STANDARD;
    use p12_keystore::{
        Certificate, EncryptionAlgorithm, KeyStore, KeyStoreEntry, MacAlgorithm, PrivateKeyChain,
    };
    use rurge_config::spec::TlsOpts;
    use rurge_config::{HostName, KeystoreType, Span};
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// PBES2 / PBKDF2 / AES-256-CBC, MAC sha256 (the OpenSSL 3 default).
    const OPENSSL_MODERN: &str = "MIIEaQIBAzCCBB8GCSqGSIb3DQEHAaCCBBAEggQMMIIECDCCApoGCSqGSIb3DQEHBqCCAoswggKHAgEAMIICgAYJKoZIhvcNAQcBMF8GCSqGSIb3DQEFDTBSMDEGCSqGSIb3DQEFDDAkBBB6HUp0PGsWgLTI6W8R02dlAgIIADAMBggqhkiG9w0CCQUAMB0GCWCGSAFlAwQBKgQQM/JUD6j7ZhYhBzjwpZJDkoCCAhAPafxNG+tkEs1U7CV/HKK4SebsswJeAwQiAaZJwbsAwnz8/FX5Gr/m6RhUClFLm7sPnWMxm1AGYk2pR05hwGbyl4URBQviViYNXuJxdaQJmv0/eMGrSjvTXjAqAKFCrFA5Xl2e0VwUoTYWzMr2V4ejIeQTW4mz5biEQySUd0ll1p8cu4f822nkov9iIhFD/IbEjK+5X2QEAW4TvidOIe3xhE+vzCSA+959erv4A84po8C0DzVjP9Ph3W1X+EY6boclV7lAYp1VYyr67MC2D4a++XcGjMd1yhdvGfpfy23p4JABbJRYMe0O/oDLCF5pGcwjIk8g6+uFVqe+wMWCL8E2i+q9xJCPVmk2aWEMqxrWaukmLmxK5C6oLVMYStCuWdi/S6TLyDMEHD/4tZJTQvEyu653yZHzHjOXS6yR56j+0bc2h8V6VzhVttrMv0xCjYJUDNmf+2vWiJy1sUZkZc8uCw/LS1nUojCXCLBvaJQUd9dNwvv1MWxWD4gESNzSvs1dH8meaiAx0LC/wk9aqpocwFI+mzvsnyXd1l3ob2ecdqxUZByjsUvEuEBR1D1bWuX8HkM83J7WcVt2IOfXwYL81igHWoREOFADb1cPYpFM1ToWdLdnEddgfhy+I9rmye68+sgaQZVOkFwUiEGOPSzMHptolrMPKLMylZKMMkiF3EBY03aPBDAxCkl0ufnL/dcwggFmBgkqhkiG9w0BBwGgggFXBIIBUzCCAU8wggFLBgsqhkiG9w0BDAoBAqCB9zCB9DBfBgkqhkiG9w0BBQ0wUjAxBgkqhkiG9w0BBQwwJAQQY48lLjTXFIbuo58TlShk1gICCAAwDAYIKoZIhvcNAgkFADAdBglghkgBZQMEASoEEJaHl3v8mHNKgSGU4zkSBlIEgZAXK2FgrzL88deZuiabDTuH6vauWw/yH85wKB/1IrAw1goymBhvHQxAaKVbqV3Wm2zMlJkFwVhe1XSVRt5yI47pVgJ1BRh8Y0w6+hHf5SCcnq4es3a3JSS9tYGI2ENMixWQhtMl2IMqR48gsHtRJaGHJN/oCtvXOmYclN020DqW2TWQ85rR7NTztFPIS2a21+sxQjAbBgkqhkiG9w0BCRQxDh4MAGMAbABpAGUAbgB0MCMGCSqGSIb3DQEJFTEWBBRTsNY1tmgfOAeFrNhGIM2POMM6/jBBMDEwDQYJYIZIAWUDBAIBBQAEIGmDsVPr0Drmu9yRYgK1vDM7SW0HwC2j+CjsfNx4ZkXzBAir+S/IETZiQgICCAA=";
    /// pbeWithSHA1And40BitRC2-CBC certificate bag, pbeWithSHA1And3-KeyTripleDES-CBC key bag, MAC sha1 (`-legacy`).
    const OPENSSL_LEGACY: &str = "MIID0wIBAzCCA5kGCSqGSIb3DQEHAaCCA4oEggOGMIIDgjCCAlcGCSqGSIb3DQEHBqCCAkgwggJEAgEAMIICPQYJKoZIhvcNAQcBMBwGCiqGSIb3DQEMAQYwDgQIcuV1FKxH1JcCAggAgIICEGlkyJYANwRYwejjupnou1kGiu2SEPCNrsyPJLbMHyPoOc1plcCGVJNcRYUUi1HDgQlEEvMEWJGLfszKe8PRik01hLi6AGLdxwnrptqND/rPzpMRJd2HtNEs3OCDPqaDQapp4i31ZtldNbM2dzSqZQxBZrIMytLoJZZnBveOApV5uOTuHCUYn5hFUbERpZ+ua0vkeSa6yM3yC4UBCkwwN3TCqxjMO/f4qKawdy4/FnYB81P2jOjh0kJOcdRGRPUYiKQnoFzsWH29nus+2TJMYjZLFZwGAJpFyy5PMGQ+4u/FmrYPodHxvlnFem5Gfyt9T8oxEIIoGzul8ixHb1drGI14ZcN7lkqitJWDmqpBgoYeNrWb1Eyo3iruwFu2ITGRVk8r3EoxKMrjtwUHQhBGT/GYHVjBtCtZpjSBZZcWnV11nmyxScdPvlzb/0LCR8eJv6AQm399ZHL23GMslJrhr9yj+bCetpy0uq0P7Z2VYWQ4dfUuBvPE6aDPCc35kn+aRUkjgCh5lFObJQ5cd9aOssg9fAEPWZdaIx4C7wSssf/WkEVibw57Rc5KyZPIIXPKAv+Y+HniqsMt2ivYuo3JWhleW5y9GkQFm21MrjU1mo4suoXw9BRRxDcwyjeF/26DA16tDkLu8+v4qbdddkBnZLGMk9rgSGVEeF+MommhoW8lcvjvnj4/zgH8nWqfQtwpMjCCASMGCSqGSIb3DQEHAaCCARQEggEQMIIBDDCCAQgGCyqGSIb3DQEMCgECoIG0MIGxMBwGCiqGSIb3DQEMAQMwDgQIQvYVnv7KY8kCAggABIGQeCPLHIb2vKaFBaaneTHI8U8p/S9QQnF99LuPFy18zM2COf+D725SdUP46f+ItEkISnC2n8rWA4Cz2F1miYhQXdt9k/xaKibcM0w85GVgLLfwnoHl3bEmEX7uOZzVETKR+s/ND7xY3ubbCyImeT5Z3DI9fwa6tf95gTThSJvnmvgfy3X+skvKcvuUcPesQjC3MUIwGwYJKoZIhvcNAQkUMQ4eDABjAGwAaQBlAG4AdDAjBgkqhkiG9w0BCRUxFgQUU7DWNbZoHzgHhazYRiDNjzjDOv4wMTAhMAkGBSsOAwIaBQAEFH0qoFogC0j/D3eS5xuzYaKHhDyCBAhqWJ84rvZ7aQICCAA=";
    /// SHA-256 of the certificate (DER) inside both files.
    const OPENSSL_CERT_SHA256: &str =
        "308381cecb58ed0ed0920b78e04320ecd49b880c48ffbe025580b74a96c41f0b";

    fn item(base64: &str, password: Option<&str>) -> KeystoreItem {
        KeystoreItem {
            name: "cert1".into(),
            kind: KeystoreType::P12,
            base64: base64.into(),
            password: password.map(str::to_string),
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 1),
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn files_written_by_openssl_decode_with_either_encryption() {
        for fixture in [OPENSSL_MODERN, OPENSSL_LEGACY] {
            let identity = decode_p12(&item(fixture, Some("rurge-test"))).unwrap();
            assert_eq!(identity.chain.len(), 1);
            assert_eq!(
                hex(&Sha256::digest(identity.chain[0].as_ref())),
                OPENSSL_CERT_SHA256
            );
            // rustls can sign with the key
            rustls::crypto::ring::default_provider()
                .key_provider
                .load_private_key(identity.key)
                .expect("a usable PKCS#8 key");
        }
    }

    #[test]
    fn failures_name_the_item_and_never_the_secret() {
        let err = decode_p12(&item(OPENSSL_MODERN, Some("hunter2-wrong")))
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.message
                .starts_with("keystore item `cert1` cannot be decoded"),
            "{}",
            err.message
        );
        assert!(!err.message.contains("hunter2-wrong"));
        let err = decode_p12(&item("@@@", Some("x"))).map(|_| ()).unwrap_err();
        assert_eq!(
            err.message,
            "keystore item `cert1`: `base64` is not valid Base64"
        );
        let err = decode_p12(&item("AAAA", Some("x")))
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.message
                .starts_with("keystore item `cert1` cannot be decoded")
        );
    }

    /// A p12 holding a client certificate signed by the fixture's CA.
    fn p12_for(fixture: &TlsFixture, algorithm: EncryptionAlgorithm, mac: MacAlgorithm) -> String {
        let (cert, key) = fixture.issue_client("rurge mtls client");
        let chain =
            PrivateKeyChain::new(key, [1u8, 2, 3, 4], [Certificate::from_der(&cert).unwrap()]);
        let mut store = KeyStore::new();
        store.add_entry("client", KeyStoreEntry::PrivateKeyChain(chain));
        let der = store
            .writer("pw")
            .encryption_algorithm(algorithm)
            .mac_algorithm(mac)
            .write()
            .unwrap();
        STANDARD.encode(der)
    }

    #[tokio::test]
    async fn a_decoded_identity_completes_mutual_tls() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(true).await;
        for (algorithm, mac) in [
            (
                EncryptionAlgorithm::PbeWithHmacSha256AndAes256,
                MacAlgorithm::HmacSha256,
            ),
            (
                EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc,
                MacAlgorithm::HmacSha1,
            ),
        ] {
            let identity =
                decode_p12(&item(&p12_for(&fixture, algorithm, mac), Some("pw"))).unwrap();
            let client = TlsClient::build(
                &TlsOpts::default(),
                &HostName::parse("localhost"),
                &[],
                Some(identity),
                fixture.roots(),
            )
            .unwrap();
            let tcp = TcpStream::connect(addr).await.unwrap();
            let mut stream = client.wrap(Box::new(tcp)).await.unwrap();
            stream.write_all(b"ping").await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
        }
        assert!(fixture.seen().iter().all(|h| h.client_cert));
        // without a certificate the server refuses
        let bare = TlsClient::build(
            &TlsOpts::default(),
            &HostName::parse("localhost"),
            &[],
            None,
            fixture.roots(),
        )
        .unwrap();
        let tcp = TcpStream::connect(addr).await.unwrap();
        let refused = async {
            let mut s = bare.wrap(Box::new(tcp)).await?;
            s.write_all(b"ping").await?;
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.map(|_| ())
        };
        assert!(refused.await.is_err());
    }
}
