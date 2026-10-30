//! The four files `rurge mitm ca generate` writes for a new certificate
//! authority (`phase 4 M2 design 13`).
//!
//! One call makes everything a new CA needs: an ECDSA P-256 key, a self-signed
//! certificate that may sign certificates (ten years long, backdated like an
//! issued one), the PKCS#12 that carries the key and the certificate in the
//! legacy scheme older importers read (3DES with a SHA-1 MAC), and the
//! `[MITM]` section that names both.
//!
//! Nothing here writes a file: the caller writes the four strings and bytes,
//! reads the PKCS#12 back and asks `Generated::check` whether it is a CA rurge
//! can sign with. No message and no `Debug` carries the passphrase or the
//! material (`phase 4 M2 design 15`).

use crate::mitm::authority::{Authority, BACKDATE_DAYS, CaProblem};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use p12_keystore::{
    Certificate, EncryptionAlgorithm, KeyStore, KeyStoreEntry, MacAlgorithm, PrivateKeyChain,
};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
    PKCS_ECDSA_P256_SHA256, SerialNumber,
};
use ring::digest;
use ring::rand::{SecureRandom as _, SystemRandom};
use rurge_config::mitm::MitmConfig;
use std::fmt;
use time::OffsetDateTime;

/// How long a generated CA lives: about ten years (phase 4 M2 design 13).
pub const CA_DAYS: i64 = 3650;

/// The name the PKCS#12 gives its one entry.
const P12_ALIAS: &str = "rurge MITM CA";

/// The alphabet of a generated passphrase: `A-Z`, `a-z` and `0-9`.
const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/// How many characters a generated passphrase has.
const PASSPHRASE_LEN: usize = 24;

/// A byte at or above this is dropped: 248 is the largest multiple of 62 that
/// fits a byte, so every byte below it maps onto the alphabet without bias.
const REJECT: u8 = 248;

/// The characters of one `certificate_pem` line.
const PEM_LINE: usize = 64;

/// A new CA and the four files `rurge mitm ca generate` writes (phase 4 M2
/// design 13). `Debug` shows the common name and the end of validity alone.
pub struct Generated {
    pub pem: String,  // rurge-ca.pem
    pub der: Vec<u8>, // rurge-ca.cer
    pub p12: Vec<u8>, // rurge-ca.p12
    pub conf: String, // rurge-mitm.conf
    pub common_name: String,
    pub not_after: OffsetDateTime,
    passphrase: String,
}

impl Generated {
    /// Whether `p12` — the file as read back — is a CA rurge can sign with:
    /// `Authority::load` with this CA's passphrase says `Ok(Some(_))`.
    pub fn check(&self, p12: &[u8]) -> Result<(), CaProblem> {
        let cfg = MitmConfig {
            ca_p12: Some(STANDARD.encode(p12)),
            ca_passphrase: Some(self.passphrase.clone()),
            ..MitmConfig::default()
        };
        match Authority::load(&cfg, &[]) {
            Ok(Some(_)) => Ok(()),
            Err(problem) => Err(problem),
            // a PKCS#12 without a certificate and a key in it is no CA; the
            // load does not say that (`the phase 4 / M2c plan's P4`)
            Ok(None) => Err(CaProblem {
                message: "MITM is off: the PKCS#12 holds no CA".into(),
                span: None,
                reported: false,
            }),
        }
    }
}

impl fmt::Debug for Generated {
    /// The common name and the end of validity: enough to tell two CAs apart,
    /// and nothing of the material.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Generated")
            .field("common_name", &self.common_name)
            .field("not_after", &self.not_after)
            .finish_non_exhaustive()
    }
}

/// Why a CA cannot be generated: the reason a library gave, and never a
/// credential (`phase 4 M2 design 15`).
#[derive(Debug)]
pub struct GenerateError(String);

impl fmt::Display for GenerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot generate a CA: {}", self.0)
    }
}

impl std::error::Error for GenerateError {}

/// A new CA, made now.
pub fn generate() -> Result<Generated, GenerateError> {
    // a certificate carries whole seconds, so `not_after` is the time the
    // certificate itself says only when the part below a second is gone
    let now = OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .map_err(|e| GenerateError(format!("the clock cannot be read: {e}")))?;
    new_ca(now)
}

/// `generate`, with the time given: for the cases that pin the clock.
#[cfg(test)]
pub(crate) fn generate_at(now: OffsetDateTime) -> Result<Generated, GenerateError> {
    new_ca(now)
}

/// The PEM text of a certificate: base64 lines of 64, LF line ends.
pub fn certificate_pem(der: &[u8]) -> String {
    let text = STANDARD.encode(der);
    let mut pem = String::with_capacity(text.len() + text.len() / PEM_LINE + 56);
    pem.push_str("-----BEGIN CERTIFICATE-----\n");
    let mut start = 0;
    while start < text.len() {
        let end = (start + PEM_LINE).min(text.len());
        pem.push_str(&text[start..end]);
        pem.push('\n');
        start = end;
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

/// A CA made by `now`: its key, its certificate, the PKCS#12 the certificate
/// and the key go in, and the `[MITM]` that names them (`phase 4 M2 design`
/// `13`).
fn new_ca(now: OffsetDateTime) -> Result<Generated, GenerateError> {
    let key =
        KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|e| GenerateError(e.to_string()))?;

    let common_name = format!("rurge MITM CA {}", now.date());
    let mut params = CertificateParams::default();
    // `CertificateParams::default` writes a common name of its own
    params.distinguished_name = DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, common_name.clone());
    params
        .distinguished_name
        .push(DnType::OrganizationName, "rurge");
    // a CA, and one that signs at most end certificates
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    // backdated like an issued certificate, so that a client whose clock is a
    // little behind still accepts it
    params.not_before = now - time::Duration::days(BACKDATE_DAYS);
    let not_after = now + time::Duration::days(CA_DAYS);
    params.not_after = not_after;
    params.serial_number = Some(SerialNumber::from_slice(&serial()?));

    let certificate = params
        .self_signed(&key)
        .map_err(|e| GenerateError(e.to_string()))?;
    let der = certificate.der().to_vec();
    let passphrase = passphrase()?;
    let p12 = pkcs12(&key, &der, &passphrase)?;
    let conf = format!(
        "[MITM]\nca-p12 = {}\nca-passphrase = {passphrase}\n",
        STANDARD.encode(&p12)
    );
    Ok(Generated {
        pem: certificate_pem(&der),
        der,
        p12,
        conf,
        common_name,
        not_after,
        passphrase,
    })
}

/// 16 random bytes with the top bit cleared: a positive serial number, as
/// `Authority::issue` writes one (`the phase 4 / M2c plan's P4`).
fn serial() -> Result<[u8; 16], GenerateError> {
    let mut serial = [0u8; 16];
    SystemRandom::new()
        .fill(&mut serial)
        .map_err(|e| GenerateError(format!("the random number generator failed: {e}")))?;
    serial[0] &= 0x7f;
    Ok(serial)
}

/// 24 characters of the alphabet, each with the same chance: a byte at or
/// above `REJECT` is dropped and the rest is taken modulo 62.
fn passphrase() -> Result<String, GenerateError> {
    let rng = SystemRandom::new();
    let mut block = [0u8; 64];
    let mut passphrase = String::with_capacity(PASSPHRASE_LEN);
    while passphrase.len() < PASSPHRASE_LEN {
        rng.fill(&mut block)
            .map_err(|e| GenerateError(format!("the random number generator failed: {e}")))?;
        for byte in block {
            if byte < REJECT && passphrase.len() < PASSPHRASE_LEN {
                let index = usize::from(byte % ALPHABET.len() as u8);
                passphrase.push(char::from(ALPHABET[index]));
            }
        }
    }
    Ok(passphrase)
}

/// `der` and the key behind it as a PKCS#12 in the legacy scheme older
/// importers read: 3DES for both packages, a SHA-1 MAC, the iterations the
/// library defaults to (`phase 4 M2 design 13`). The local key identifier is
/// the SHA-1 of the certificate, as OpenSSL writes it.
fn pkcs12(key: &KeyPair, der: &[u8], passphrase: &str) -> Result<Vec<u8>, GenerateError> {
    let id = digest::digest(&digest::SHA1_FOR_LEGACY_USE_ONLY, der);
    let certificate = Certificate::from_der(der).map_err(|e| GenerateError(e.to_string()))?;
    let chain = PrivateKeyChain::new(key.serialize_der(), id.as_ref(), [certificate]);
    let mut store = KeyStore::new();
    store.add_entry(P12_ALIAS, KeyStoreEntry::PrivateKeyChain(chain));
    store
        .writer(passphrase)
        .encryption_algorithm(EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc)
        .mac_algorithm(MacAlgorithm::HmacSha1)
        .write()
        .map_err(|e| GenerateError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mitm::testing::{DAY, NOW, at};
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_config::{Config, Diagnostics, HostName};
    use rustls::pki_types::{CertificateDer, ServerName};
    use rustls::sign::SingleCertAndKey;
    use rustls::{ClientConfig, RootCertStore, ServerConfig};
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio_rustls::{TlsAcceptor, TlsConnector};
    use x509_parser::extensions::ParsedExtension;
    use x509_parser::oid_registry::{
        OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY, OID_SIG_ECDSA_WITH_SHA256, OID_X509_COMMON_NAME,
        OID_X509_ORGANIZATION_NAME,
    };
    use x509_parser::prelude::*;

    /// The OIDs the PKCS#12 is checked for, as DER writes them: the PBE with
    /// SHA-1 and 3DES the design asks for (phase 4 M2 design 13), SHA-1 (the
    /// MAC) and PBES2 (what the writer defaults to and what must not be used).
    const PBE_3DES: &[u8] = &[
        0x06, 0x0A, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x0C, 0x01, 0x03,
    ];
    const SHA1: &[u8] = &[0x06, 0x05, 0x2B, 0x0E, 0x03, 0x02, 0x1A];
    const PBES2: &[u8] = &[
        0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x05, 0x0D,
    ];

    /// The certificate `der` holds; nothing may follow it.
    fn cert_of(der: &[u8]) -> X509Certificate<'_> {
        let (rest, cert) = parse_x509_certificate(der).expect("a certificate");
        assert!(rest.is_empty(), "the DER has bytes after the certificate");
        cert
    }

    /// How many times `needle` occurs in `hay`.
    fn occurrences(hay: &[u8], needle: &[u8]) -> usize {
        hay.windows(needle.len()).filter(|w| *w == needle).count()
    }

    /// The `[MITM]` of `g`, read the way a profile is.
    fn loaded(g: &Generated) -> (Config, Diagnostics) {
        let text = format!("{}hostname = a.test\n[Rule]\nFINAL,DIRECT\n", g.conf);
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        (loaded.config, loaded.diagnostics)
    }

    /// A certificate that may sign certificates, backdated two days, ten
    /// years long, under the name the day gives it (phase 4 M2 design 13).
    #[test]
    fn a_generated_ca_is_p256_backdated_and_ten_years_long() {
        let g = generate_at(at(NOW)).expect("a generated CA");
        let cert = cert_of(&g.der);

        assert_eq!(g.common_name, "rurge MITM CA 2027-01-01");
        assert_eq!(
            cert.subject().to_string(),
            cert.issuer().to_string(),
            "a self-signed certificate names itself as its issuer"
        );
        let mut names: Vec<(String, String)> = cert
            .subject()
            .iter_attributes()
            .map(|a| {
                (
                    a.attr_type().to_id_string(),
                    a.as_str().expect("a text attribute").to_owned(),
                )
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                (
                    OID_X509_ORGANIZATION_NAME.to_id_string(),
                    "rurge".to_owned()
                ),
                (
                    OID_X509_COMMON_NAME.to_id_string(),
                    "rurge MITM CA 2027-01-01".to_owned()
                ),
            ],
            "the subject is the common name and the organization alone"
        );

        assert_eq!(cert.validity().not_before.timestamp(), NOW - 2 * DAY);
        assert_eq!(cert.validity().not_after.timestamp(), NOW + CA_DAYS * DAY);
        assert_eq!(g.not_after.unix_timestamp(), NOW + CA_DAYS * DAY);

        let basic = cert
            .basic_constraints()
            .expect("a readable extension")
            .expect("a basic constraints extension");
        assert!(basic.critical, "the basic constraints are critical");
        assert!(basic.value.ca, "the certificate is a CA");
        assert_eq!(basic.value.path_len_constraint, Some(0));

        let usage = cert
            .key_usage()
            .expect("a readable extension")
            .expect("a key usage extension");
        assert!(usage.critical, "the key usage is critical");
        let uses = usage.value;
        assert!(uses.key_cert_sign() && uses.crl_sign());
        assert!(
            !(uses.digital_signature()
                || uses.non_repudiation()
                || uses.key_encipherment()
                || uses.data_encipherment()
                || uses.key_agreement()
                || uses.encipher_only()
                || uses.decipher_only()),
            "the CA may sign certificates and revocations and nothing else"
        );

        let mut ski = false;
        let mut aki = false;
        for extension in cert.iter_extensions() {
            match extension.parsed_extension() {
                ParsedExtension::SubjectKeyIdentifier(_) => ski = true,
                ParsedExtension::AuthorityKeyIdentifier(_) => aki = true,
                _ => {}
            }
        }
        assert!(ski, "the CA carries a subject key identifier");
        assert!(!aki, "the CA carries no authority key identifier");

        assert_eq!(
            cert.public_key().algorithm.algorithm,
            OID_KEY_TYPE_EC_PUBLIC_KEY
        );
        let curve = cert
            .public_key()
            .algorithm
            .parameters
            .as_ref()
            .expect("the curve the key is on")
            .as_oid()
            .expect("an OID");
        assert_eq!(curve, OID_EC_P256);
        assert_eq!(
            cert.signature_algorithm.algorithm,
            OID_SIG_ECDSA_WITH_SHA256
        );

        let serial = cert.raw_serial();
        assert!(!serial.is_empty(), "the serial number is written");
        assert!(serial.len() <= 16, "the serial number is at most 16 bytes");
        assert_eq!(serial[0] & 0x80, 0, "the serial number is positive");
        assert!(
            serial.iter().any(|b| *b != 0),
            "the serial number is not zero"
        );
    }

    /// The PKCS#12 is the legacy scheme older importers read, and rurge reads
    /// it back as a CA it can sign with (phase 4 M2 design 13).
    #[test]
    fn the_p12_is_3des_with_a_sha1_mac_and_reads_back() {
        let g = generate().expect("a generated CA");

        assert!(
            occurrences(&g.p12, PBE_3DES) >= 2,
            "the certificate bag and the key bag are both 3DES"
        );
        assert!(occurrences(&g.p12, SHA1) >= 1, "the MAC is SHA-1");
        assert_eq!(occurrences(&g.p12, PBES2), 0, "no PBES2 anywhere");

        g.check(&g.p12).expect("the PKCS#12 reads back as a CA");

        // the MAC is the last thing the file holds: changing it must make the
        // file unreadable, which is what a corrupted file looks like
        let mut broken = g.p12.clone();
        let last = broken.len() - 1;
        broken[last] ^= 0xff;
        assert!(
            g.check(&broken).is_err(),
            "a changed MAC must not read back"
        );
    }

    /// The `[MITM]` section names the file and its passphrase, and what it
    /// names loads and is the CA that was made.
    #[test]
    fn the_conf_is_a_mitm_section_that_loads() {
        let g = generate().expect("a generated CA");

        let lines: Vec<&str> = g.conf.lines().collect();
        assert_eq!(lines.len(), 3, "the section is three lines");
        assert_eq!(lines[0], "[MITM]");
        assert!(
            lines[1] == format!("ca-p12 = {}", STANDARD.encode(&g.p12)),
            "the second line carries the PKCS#12 as Base64"
        );
        assert!(
            lines[2] == format!("ca-passphrase = {}", g.passphrase),
            "the third line carries the passphrase"
        );
        assert!(g.conf.ends_with('\n'), "the last line ends with a newline");

        assert_eq!(g.passphrase.len(), 24, "the passphrase is 24 characters");
        assert!(
            g.passphrase.chars().all(|c| c.is_ascii_alphanumeric()),
            "the passphrase is letters and digits"
        );

        let (config, diagnostics) = loaded(&g);
        assert!(!diagnostics.has_errors(), "the section loads with no error");
        assert!(crate::mitm::check(&config).is_empty(), "the CA is usable");
        let authority = Authority::load(&config.mitm, &config.keystore)
            .expect("a usable CA")
            .expect("a CA was named");
        assert_eq!(authority.ca_der(), g.der.as_slice());
    }

    /// A client that trusts this CA alone completes a handshake with a
    /// certificate signed under it (phase 4 M2 design 17 item 6).
    #[tokio::test]
    async fn a_generated_ca_serves_a_handshake() {
        let g = generate().expect("a generated CA");
        let (config, _) = loaded(&g);
        let authority = Arc::new(
            Authority::load(&config.mitm, &config.keystore)
                .expect("a usable CA")
                .expect("a CA was named"),
        );
        let leaf = authority
            .leaf(&HostName::parse("a.test"))
            .await
            .expect("a certificate for the name");

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server = ServerConfig::builder_with_provider(Arc::clone(&provider))
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .expect("TLS 1.3 and 1.2")
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SingleCertAndKey::from(leaf)));

        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(g.der.clone()))
            .expect("a trust anchor");
        let client = ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
            .expect("TLS 1.3 and 1.2")
            .with_root_certificates(roots)
            .with_no_client_auth();

        // in memory, over a duplex pipe: no socket, no name to resolve
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let accept = TlsAcceptor::from(Arc::new(server)).accept(server_io);
        let connect = TlsConnector::from(Arc::new(client))
            .connect(ServerName::try_from("a.test").expect("a name"), client_io);
        let (accepted, connected) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(accept, connect)
        })
        .await
        .expect("the handshake finishes");

        assert!(
            accepted.is_ok(),
            "the server side of the handshake succeeds"
        );
        assert!(
            connected.is_ok(),
            "the client side of the handshake succeeds"
        );
    }

    /// Two generated CAs are two authorities: different certificates,
    /// different serial numbers, different passphrases.
    #[test]
    fn two_cas_differ() {
        let first = generate().expect("a generated CA");
        let second = generate().expect("a generated CA");

        assert_ne!(first.der, second.der, "two CAs, two certificates");
        assert_ne!(
            first.passphrase, second.passphrase,
            "two CAs, two passphrases"
        );
        assert_ne!(
            cert_of(&first.der).raw_serial().to_vec(),
            cert_of(&second.der).raw_serial().to_vec(),
            "two CAs, two serial numbers"
        );
    }

    /// `Debug` names the CA and holds neither the passphrase nor the material
    /// (phase 4 M2 design 15).
    #[test]
    fn the_debug_of_a_generated_ca_shows_no_secret() {
        let g = generate().expect("a generated CA");
        let debug = format!("{g:?}");

        assert!(debug.contains(&g.common_name), "the Debug names the CA");
        assert!(
            !debug.contains(g.passphrase.as_str()),
            "the Debug holds no passphrase"
        );
        assert!(!debug.contains("PRIVATE KEY"), "the Debug holds no key");
        let base64 = STANDARD.encode(&g.p12);
        assert!(!debug.contains(&base64), "the Debug holds no PKCS#12");
        assert!(
            !debug.contains(&base64[..40]),
            "the Debug holds no start of the PKCS#12"
        );
        for line in g.conf.lines().skip(1) {
            let value = line.split_once(" = ").expect("a value").1;
            assert!(
                !debug.contains(value),
                "the Debug holds no value of the conf"
            );
        }
    }

    /// The PEM text is the standard one: 64 characters a line, LF line ends.
    #[test]
    fn certificate_pem_is_the_standard_text() {
        let g = generate().expect("a generated CA");
        let pem = certificate_pem(&g.der);

        assert_eq!(g.pem, pem, "the field is the same text");
        assert!(
            pem.starts_with("-----BEGIN CERTIFICATE-----\n"),
            "a PEM header"
        );
        assert!(pem.ends_with("-----END CERTIFICATE-----\n"), "a PEM footer");
        let body = pem
            .strip_prefix("-----BEGIN CERTIFICATE-----\n")
            .expect("a header")
            .strip_suffix("-----END CERTIFICATE-----\n")
            .expect("a footer");
        let lines: Vec<&str> = body.lines().collect();
        assert!(!lines.is_empty(), "the body is not empty");
        assert!(
            lines[..lines.len() - 1].iter().all(|l| l.len() == 64),
            "every line but the last is full"
        );
        assert!(
            lines.iter().all(|l| l.len() <= 64),
            "no line is longer than 64 characters"
        );
        assert_eq!(
            STANDARD.decode(lines.concat()).expect("the Base64"),
            g.der,
            "the text decodes back to the certificate"
        );

        // 100 bytes is not a multiple of 48: the last line is short and still
        // ends with a newline
        let short = certificate_pem(&[0u8; 100]);
        let body = short
            .strip_prefix("-----BEGIN CERTIFICATE-----\n")
            .expect("a header")
            .strip_suffix("-----END CERTIFICATE-----\n")
            .expect("a footer");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "two full lines and a short one");
        assert_eq!(lines.last().map(|l| l.len()), Some(8));
        assert!(short.ends_with("-----\n"), "the footer ends with a newline");
    }
}
