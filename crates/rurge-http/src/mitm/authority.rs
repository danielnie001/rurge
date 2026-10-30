//! The certificate authority `[MITM]` signs with (manual: HTTPS › MITM).
//!
//! `Authority::load` reads a PKCS#12 — the one `ca-p12` carries, or the
//! `[Keystore]` item `ca-keystore-name` names — and refuses anything rurge
//! cannot sign with, one `CaProblem` at a time (`phase 4 M2 design 5.1`,
//! `5.2`). Nothing is asked of the network: the certificate paired with the
//! private key is parsed, its BasicConstraints, its KeyUsage and its validity
//! are read, the key is handed to rcgen, and one certificate for a reserved
//! name is issued and verified with rustls' own verifier against this CA
//! alone. `check` turns a problem into `W0039` for the places a profile is
//! loaded (`phase 4 M2 design 5.3`), and names `W0040` when `http-api-tls`
//! would have the API serve HTTPS with a profile that has no usable CA
//! (`phase 4 M2 design 14.1`).
//!
//! No message repeats a credential (`phase 4 M2 design 15`): the Base64, the
//! passphrase and the private key never appear in one, and `Debug` shows the
//! certificate's subject and the first bytes of its fingerprint.
//!
//! `Authority::leaf` is what a decrypted connection is served from: one
//! certificate per name, signed on a blocking thread and cached, so that the
//! connections a browser opens at once for one host sign once (`phase 4 M2
//! design 6`). The TLS sessions of the connections this authority serves are
//! cached here too, so that they resume across connections (`phase 4 M2
//! design 8.1`).

use lru::LruCache;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType, SerialNumber,
};
use ring::rand::SecureRandom as _;
use rurge_config::diagnostic::codes;
use rurge_config::{
    Config, Diagnostic, Diagnostics, HostName, KeystoreItem, KeystoreType, MitmConfig, Span,
};
use rurge_proto::keystore::{base64_bytes, decode_pkcs12};
use rustls::RootCertStore;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::ServerCertVerifier as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::ServerSessionMemoryCache;
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};
use std::fmt;
use std::num::NonZeroUsize;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use time::OffsetDateTime;
use tokio::sync::OnceCell;
use x509_parser::extensions::ParsedExtension;
use x509_parser::parse_x509_certificate;

/// The name of the certificate issued to prove that the CA can issue: it can
/// never be a real host (`phase 4 M2 design 5.2` item 6).
const CHECK_NAME: &str = "rurge-ca-check.invalid";

/// How many names keep their certificate (`phase 4 M2 design 6`).
pub const LEAF_SLOTS: usize = 1000;

/// How many TLS sessions the connections of one authority resume from
/// (`phase 4 M2 design 6`, `8.1`).
pub const SESSION_SLOTS: usize = 1024;

/// How long an issued certificate lives, and how far back it reaches before
/// now (`phase 4 M2 design 6`).
pub const LEAF_DAYS: i64 = 47;
pub const BACKDATE_DAYS: i64 = 2;

/// What `[MITM]` has to say when it lists hosts and sets no CA (`P5` item 1).
const NO_CA: &str =
    "`hostname` lists hosts to decrypt but `[MITM]` sets no CA (`ca-p12` or `ca-keystore-name`)";

/// The message of `W0040`, and of `rurge status` / `reload` / `stop` when
/// they cannot read the CA the API serves HTTPS with (phase 4 M2 design
/// 14.1, the phase 4 / M2c plan's P2).
pub const API_TLS_NO_CA: &str =
    "http-api-tls is on but there is no usable MITM CA: the API does not start with this profile";

/// What tells an `Authority` the time: `OffsetDateTime::now_utc` in
/// production, a pinned instant in tests (`phase 4 / M2a plan P7`). There is
/// no entry point in production code that replaces it.
pub(crate) type Clock = Arc<dyn Fn() -> OffsetDateTime + Send + Sync>;

fn default_clock() -> Clock {
    Arc::new(OffsetDateTime::now_utc)
}

/// Why `[MITM]` has no usable CA: the message says it (and starts with
/// `MITM is off: `), `span` is the line to report it at, and `reported` says
/// the load has already named it — an `E0021` of a `[Keystore]` item whose
/// Base64 does not decode (`P5` item 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaProblem {
    pub message: String,
    pub span: Option<Span>,
    pub reported: bool,
}

impl CaProblem {
    fn new(message: String, span: Option<Span>) -> CaProblem {
        CaProblem {
            message: format!("MITM is off: {message}"),
            span,
            reported: false,
        }
    }
}

/// A certificate authority rurge can issue under, and everything a check or a
/// later signing needs to know about it (`phase 4 M2 design 5`).
pub struct Authority {
    /// The certificate paired with the private key: what clients trust.
    ca: CertificateDer<'static>,
    /// The PKCS#12 chain from that certificate on (leaf first): what an
    /// issued certificate is handed out with (`phase 4 M2 design 5.5`).
    chain: Vec<CertificateDer<'static>>,
    issuer: Issuer<'static, KeyPair>,
    /// Whether the CA carries a subject key identifier: an issued
    /// certificate copies the authority key identifier only when it does
    /// (`M2-D6`).
    has_ski: bool,
    not_before: OffsetDateTime,
    not_after: OffsetDateTime,
    /// The CA certificate's subject, for `Debug` and for nothing else.
    subject: String,
    /// SHA-256 of the CA certificate (DER).
    certificate_sha256: [u8; 32],
    /// SHA-256 of the material as written (`P8`): compared, never shown.
    material: [u8; 32],
    clock: RwLock<Clock>,
    /// One slot per name, least recently used last (`phase 4 M2 design 6`).
    /// The lock is held to pick or replace a slot and never across an await.
    leaves: Mutex<LruCache<HostName, Arc<Slot>>>,
    /// The TLS sessions of the connections this authority serves.
    sessions: Arc<ServerSessionMemoryCache>,
    /// How many certificates were really signed: the trial issue of the load
    /// is not one (`P7`).
    #[cfg(test)]
    signatures: AtomicUsize,
}

impl Authority {
    /// The CA `[MITM]` names, or `None` when it names none. `Err` when it
    /// names one rurge cannot use (`phase 4 M2 design 5.1`, `5.2`, in the
    /// order of `P5`).
    pub fn load(
        cfg: &MitmConfig,
        keystore: &[KeystoreItem],
    ) -> Result<Option<Authority>, CaProblem> {
        Authority::load_with(cfg, keystore, default_clock())
    }

    /// `load`, with the time given: for the cases that pin the clock.
    #[cfg(test)]
    pub(crate) fn load_at(
        cfg: &MitmConfig,
        keystore: &[KeystoreItem],
        clock: Clock,
    ) -> Result<Option<Authority>, CaProblem> {
        Authority::load_with(cfg, keystore, clock)
    }

    fn load_with(
        cfg: &MitmConfig,
        keystore: &[KeystoreItem],
        clock: Clock,
    ) -> Result<Option<Authority>, CaProblem> {
        // which material, and what it is called: `ca-keystore-name` wins and
        // does not fall back to `ca-p12` (`phase 4 M2 design 5.1`)
        let source = match &cfg.ca_keystore_name {
            Some(name) => {
                let item = keystore.iter().find(|i| &i.name == name).ok_or_else(|| {
                    CaProblem::new(
                        format!(
                            "`ca-keystore-name` names `{name}`, which [Keystore] does not have"
                        ),
                        cfg.spans.ca_keystore_name.clone(),
                    )
                })?;
                if item.kind != KeystoreType::P12 {
                    return Err(CaProblem::new(
                        format!("`ca-keystore-name` names `{name}`, which is not a PKCS#12 item"),
                        cfg.spans.ca_keystore_name.clone(),
                    ));
                }
                Source::Item {
                    name: name.clone(),
                    base64: item.base64.clone(),
                    password: item.password.clone().unwrap_or_default(),
                }
            }
            None => match &cfg.ca_p12 {
                Some(text) => Source::P12 {
                    text: text.clone(),
                    password: cfg.ca_passphrase.clone().unwrap_or_default(),
                },
                // no CA written is no CA, whatever `hostname` lists: whether
                // that is a problem is `check`'s to say (`P5` item 1)
                None => return Ok(None),
            },
        };
        let span = match &source {
            Source::P12 { .. } => cfg.spans.ca_p12.clone(),
            Source::Item { .. } => cfg.spans.ca_keystore_name.clone(),
        };
        let label = source.label();
        let src = source.described();
        let material = source.fingerprint();

        // Base64 and PKCS#12 (`P5` items 4 and 5). A keystore item whose
        // Base64 does not decode is already named by `E0021` when the profile
        // is read, so `check` stays quiet about it (`P5` item 4).
        let der = match source.der() {
            Ok(der) => der,
            Err(CaProblem { message, .. }) => {
                return Err(CaProblem {
                    message,
                    span,
                    reported: matches!(source, Source::Item { .. }),
                });
            }
        };
        let identity = decode_pkcs12(&label, &der, source.password())
            .map_err(|e| CaProblem::new(e.message, span.clone()))?;
        let ca = identity.chain[0].clone();
        let chain = identity.chain;

        let (_, cert) = parse_x509_certificate(ca.as_ref()).map_err(|e| {
            CaProblem::new(
                format!("the certificate of {src} cannot be read: {e}"),
                span.clone(),
            )
        })?;
        let unreadable = |e: x509_parser::error::X509Error| {
            CaProblem::new(
                format!("the certificate of {src} cannot be read: {e}"),
                span.clone(),
            )
        };

        // a CA (`P5` item 7), allowed to sign certificates (`P5` item 8) and
        // valid now (`P5` items 9 and 10)
        let basic = cert.basic_constraints().map_err(unreadable)?;
        if basic.is_none_or(|b| !b.value.ca) {
            return Err(CaProblem::new(
                format!("the certificate of {src} is not a CA (no basic constraints with CA:TRUE)"),
                span,
            ));
        }
        let usage = cert.key_usage().map_err(unreadable)?;
        if usage.is_some_and(|u| !u.value.key_cert_sign()) {
            return Err(CaProblem::new(
                format!(
                    "the certificate of {src} may not sign certificates (its key usage lacks keyCertSign)"
                ),
                span,
            ));
        }
        let not_before = OffsetDateTime::from_unix_timestamp(
            cert.validity().not_before.timestamp(),
        )
        .map_err(|e| {
            CaProblem::new(
                format!("the certificate of {src} cannot be read: {e}"),
                span.clone(),
            )
        })?;
        let not_after = OffsetDateTime::from_unix_timestamp(cert.validity().not_after.timestamp())
            .map_err(|e| {
                CaProblem::new(
                    format!("the certificate of {src} cannot be read: {e}"),
                    span.clone(),
                )
            })?;
        let now = clock();
        // the period runs through `notAfter` (RFC 5280 4.1.2.5), which is
        // also what the verifiers clients use refuse on
        if now > not_after {
            return Err(CaProblem::new(
                format!("{src} expired on {}", not_after.date()),
                span.clone(),
            ));
        }
        if now < not_before {
            return Err(CaProblem::new(
                format!("{src} is not valid until {}", not_before.date()),
                span.clone(),
            ));
        }
        let has_ski = cert.iter_extensions().any(|e| {
            matches!(
                e.parsed_extension(),
                ParsedExtension::SubjectKeyIdentifier(_)
            )
        });

        // a key rcgen's ring backend signs with (`P5` item 11): the reason
        // ring gives is gone by the time rcgen answers (`P1`), so the message
        // lists what rurge signs with instead
        let key = KeyPair::try_from(&identity.key).map_err(|_| {
            CaProblem::new(
                format!(
                    "rurge cannot sign with the private key of {src}: it signs with ECDSA P-256 or P-384 (a PKCS#8 key that carries its public key), RSA of 2048 to 8192 bits, or Ed25519"
                ),
                span.clone(),
            )
        })?;
        // and a certificate rcgen can rebuild the issuer of (`P5` item 12)
        let issuer = Issuer::from_ca_cert_der(&ca, key).map_err(|e| {
            CaProblem::new(
                format!(
                    "rurge cannot issue certificates under {src} ({e}); a subject with several attributes in one RDN is one cause"
                ),
                span.clone(),
            )
        })?;

        let authority = Authority {
            chain,
            issuer,
            has_ski,
            not_before,
            not_after,
            subject: cert.subject().to_string(),
            certificate_sha256: Sha256::digest(ca.as_ref()).into(),
            ca,
            material,
            clock: RwLock::new(clock),
            leaves: Mutex::new(LruCache::new(
                NonZeroUsize::new(LEAF_SLOTS).expect("a cache with slots"),
            )),
            sessions: ServerSessionMemoryCache::new(SESSION_SLOTS),
            #[cfg(test)]
            signatures: AtomicUsize::new(0),
        };
        authority.trial_issue(&src, span)?;
        Ok(Some(authority))
    }

    /// Whether `cfg` names the material this authority was loaded from
    /// (`phase 4 M2 design 5.4`, `P8`). Naming no CA, or a `[Keystore]` item
    /// that is not there, is no.
    pub fn same_material(&self, cfg: &MitmConfig, keystore: &[KeystoreItem]) -> bool {
        material_of_config(cfg, keystore) == Some(self.material)
    }

    /// Whether the CA is inside its own validity now (`P16`). The last
    /// second of `notAfter` is still inside it (RFC 5280 4.1.2.5).
    pub fn usable_now(&self) -> bool {
        let now = self.now();
        self.not_before <= now && now <= self.not_after
    }

    /// The CA certificate (DER): what `GET /v1/mitm/ca` gives (`M2-D19`).
    pub fn ca_der(&self) -> &[u8] {
        self.ca.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_clock(&self, clock: Clock) {
        *self.clock.write().expect("the clock is not poisoned") = clock;
    }

    fn now(&self) -> OffsetDateTime {
        (*self.clock.read().expect("the clock is not poisoned"))()
    }

    /// One certificate for `name`, signed on a blocking thread unless another
    /// connection for the same name is already at it (`phase 4 M2 design 6`,
    /// `M2-D7`). A name keeps its certificate until a third of its life is
    /// left; a failed signature leaves the slot empty, and the next
    /// connection tries again.
    pub async fn leaf(self: &Arc<Self>, name: &HostName) -> Result<Arc<CertifiedKey>, LeafError> {
        let now = self.now();
        let slot = {
            let mut leaves = self.leaves.lock().expect("the leaf cache is not poisoned");
            // a slot with a certificate near its end is dropped, so that the
            // connections that come next sign a new one; the ones that already
            // hold the old certificate keep it, it is not expired yet
            let stale = leaves
                .get(name)
                .is_some_and(|slot| slot.get().is_some_and(|leaf| leaf.is_due(now)));
            if stale {
                leaves.pop(name);
            }
            match leaves.get(name) {
                Some(slot) => Arc::clone(slot),
                None => {
                    let slot = Arc::new(Slot::default());
                    leaves.put(name.clone(), Arc::clone(&slot));
                    slot
                }
            }
        };
        let authority = Arc::clone(self);
        let signed = name.clone();
        let leaf = slot
            .get_or_try_init(|| async move {
                let signing = Arc::clone(&authority);
                let fallback = signed.clone();
                match tokio::task::spawn_blocking(move || signing.sign(&signed, now)).await {
                    Ok(result) => result.map(Arc::new),
                    Err(e) => Err(LeafError::new(fallback, e.to_string())),
                }
            })
            .await?;
        Ok(Arc::clone(&leaf.key))
    }

    /// The session cache the server configuration of every decrypted
    /// connection shares (`phase 4 M2 design 8.1`).
    pub(crate) fn sessions(&self) -> Arc<ServerSessionMemoryCache> {
        Arc::clone(&self.sessions)
    }

    /// Signs one certificate for `name` and makes it a `CertifiedKey` with the
    /// whole PKCS#12 chain behind it (`phase 4 M2 design 5.5`). This runs on a
    /// blocking thread.
    fn sign(&self, name: &HostName, now: OffsetDateTime) -> Result<Leaf, LeafError> {
        let issued = self
            .issue(name, now)
            .map_err(|e| LeafError::new(name.clone(), e.to_string()))?;
        #[cfg(test)]
        self.signatures.fetch_add(1, Ordering::Relaxed);
        let key = rustls::crypto::ring::sign::any_ecdsa_type(&PrivateKeyDer::Pkcs8(issued.key))
            .map_err(|e| LeafError::new(name.clone(), e.to_string()))?;
        let mut chain = Vec::with_capacity(self.chain.len() + 1);
        chain.push(issued.cert);
        chain.extend(self.chain.iter().cloned());
        Ok(Leaf {
            key: Arc::new(CertifiedKey::new(chain, key)),
            not_before: issued.not_before,
            not_after: issued.not_after,
        })
    }

    #[cfg(test)]
    pub(crate) fn signature_count(&self) -> usize {
        self.signatures.load(Ordering::Relaxed)
    }

    /// One certificate for `name`, and a fresh P-256 key for it
    /// (`phase 4 M2 design 6`, `P9`). The caller signs on a blocking thread
    /// (`M2-D7`); this is also what the trial issue of the load uses.
    pub(crate) fn issue(
        &self,
        name: &HostName,
        now: OffsetDateTime,
    ) -> Result<Issued, rcgen::Error> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::default();
        // `CertificateParams::default` writes a common name of its own
        params.distinguished_name = DistinguishedName::new();
        let text = name.to_string();
        if text.chars().count() <= 64 {
            params.distinguished_name.push(DnType::CommonName, text);
        }
        params.subject_alt_names = vec![match name {
            HostName::Domain(d) => SanType::DnsName(d.as_str().try_into().map_err(|_| {
                rcgen::Error::InvalidAsn1String(rcgen::InvalidAsn1String::Ia5String(d.clone()))
            })?),
            HostName::Ip(ip) => SanType::IpAddress(*ip),
        }];
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = self.has_ski;
        // 16 random bytes with the top bit cleared: a positive serial number
        let mut serial = [0u8; 16];
        ring::rand::SystemRandom::new()
            .fill(&mut serial)
            .map_err(|_| rcgen::Error::RingUnspecified)?;
        serial[0] &= 0x7f;
        params.serial_number = Some(SerialNumber::from_slice(&serial));
        // the leaf does not reach outside what the CA covers
        let not_before = (now - time::Duration::days(BACKDATE_DAYS)).max(self.not_before);
        let not_after = (not_before + time::Duration::days(LEAF_DAYS)).min(self.not_after);
        params.not_before = not_before;
        params.not_after = not_after;
        let cert = params.signed_by(&key, &self.issuer)?;
        Ok(Issued {
            cert: cert.der().clone(),
            key: PrivatePkcs8KeyDer::from(key.serialize_der()),
            not_before,
            not_after,
        })
    }

    /// Signs one certificate for the reserved name and verifies it against
    /// this CA alone (`phase 4 M2 design 5.2` item 6, `P5` item 13). This is
    /// what catches the failures that cannot be listed one by one: an issuer
    /// rcgen rebuilds differently from the CA's subject (a subject with
    /// several attributes in one RDN, an attribute type written twice), a
    /// certificate that does not match the private key, a signature algorithm
    /// webpki does not accept.
    fn trial_issue(&self, src: &str, span: Option<Span>) -> Result<(), CaProblem> {
        let fails = |e: String| {
            CaProblem::new(
                format!("a certificate rurge issued under {src} does not verify: {e}"),
                span.clone(),
            )
        };
        let (leaf, _key) = self
            .issue(&HostName::parse(CHECK_NAME), self.now())
            .map(|issued| (issued.cert, issued.key))
            .map_err(|e| fails(e.to_string()))?;
        let mut roots = RootCertStore::empty();
        roots
            .add(self.ca.clone())
            .map_err(|e| fails(e.to_string()))?;
        let verifier = WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .map_err(|e| fails(e.to_string()))?;
        let name = ServerName::try_from(CHECK_NAME).expect("a name for the check");
        let now = self.now();
        let now = UnixTime::since_unix_epoch(std::time::Duration::from_secs(
            u64::try_from(now.unix_timestamp()).unwrap_or(0),
        ));
        verifier
            .verify_server_cert(&leaf, &self.chain[1..], &name, &[], now)
            .map_err(|e| fails(e.to_string()))?;
        Ok(())
    }
}

/// Why an authority has no certificate for a name: what a failed handshake
/// says (`phase 4 M2 design 8.2`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeafError {
    name: HostName,
    why: String,
}

impl LeafError {
    fn new(name: HostName, why: String) -> LeafError {
        LeafError { name, why }
    }
}

impl fmt::Display for LeafError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "cannot issue a certificate for {}: {}",
            self.name, self.why
        )
    }
}

impl std::error::Error for LeafError {}

/// What `issue` signed: the certificate, its key, and the validity it was
/// given, which tells a cache when to sign a new one.
pub(crate) struct Issued {
    cert: CertificateDer<'static>,
    key: PrivatePkcs8KeyDer<'static>,
    not_before: OffsetDateTime,
    not_after: OffsetDateTime,
}

/// One name's certificate, as the connections of that name use it.
pub(crate) struct Leaf {
    key: Arc<CertifiedKey>,
    not_before: OffsetDateTime,
    not_after: OffsetDateTime,
}

impl Leaf {
    /// Whether a third of the certificate's life is left, or less: the
    /// connection that comes next signs a new one (`phase 4 M2 design 6`).
    fn is_due(&self, now: OffsetDateTime) -> bool {
        let life = self.not_after - self.not_before;
        now >= self.not_after - life / 3_i32
    }
}

/// One name's place in the leaf cache: the first connection for the name
/// signs, and the ones that arrive meanwhile wait for it (`M2-D7`).
type Slot = OnceCell<Arc<Leaf>>;

/// Where the certificate authority comes from, and nothing read off the
/// certificate yet.
enum Source {
    P12 {
        text: String,
        password: String,
    },
    Item {
        name: String,
        base64: String,
        password: String,
    },
}

impl Source {
    fn password(&self) -> &str {
        match self {
            Source::P12 { password, .. } | Source::Item { password, .. } => password,
        }
    }

    /// What `decode_pkcs12` names the material by, in the form its messages
    /// have always used.
    fn label(&self) -> String {
        match self {
            Source::P12 { .. } => "`ca-p12`".into(),
            Source::Item { name, .. } => format!("keystore item `{name}`"),
        }
    }

    /// How a message of this module names the CA.
    fn described(&self) -> String {
        match self {
            Source::P12 { .. } => "the CA in `ca-p12`".into(),
            Source::Item { name, .. } => format!("the CA in keystore item `{name}`"),
        }
    }

    /// The Base64 text as written and the passphrase (`P8`): compared, never
    /// shown.
    fn fingerprint(&self) -> [u8; 32] {
        match self {
            Source::P12 { text, password } => material_fingerprint(text, password),
            Source::Item {
                base64, password, ..
            } => material_fingerprint(base64, password),
        }
    }

    /// The PKCS#12 DER, or the problem a Base64 that does not decode is.
    fn der(&self) -> Result<Vec<u8>, CaProblem> {
        match self {
            Source::P12 { text, .. } => base64_bytes(text)
                .ok_or_else(|| CaProblem::new("`ca-p12` is not valid Base64".into(), None)),
            Source::Item { name, base64, .. } => base64_bytes(base64).ok_or_else(|| CaProblem {
                message: format!(
                    "MITM is off: keystore item `{name}`: `base64` is not valid Base64"
                ),
                span: None,
                reported: true,
            }),
        }
    }
}

/// The material of the CA `cfg` names, as the fingerprint compares it (`P8`).
fn material_of_config(cfg: &MitmConfig, keystore: &[KeystoreItem]) -> Option<[u8; 32]> {
    if let Some(name) = &cfg.ca_keystore_name {
        let item = keystore.iter().find(|i| &i.name == name)?;
        return Some(material_fingerprint(
            &item.base64,
            item.password.as_deref().unwrap_or(""),
        ));
    }
    cfg.ca_p12
        .as_ref()
        .map(|text| material_fingerprint(text, cfg.ca_passphrase.as_deref().unwrap_or("")))
}

/// SHA-256 over the Base64 text, a `0x00` and the passphrase (`P8`): the
/// same material is the same credential, whatever it is written as.
fn material_fingerprint(base64: &str, password: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(base64.as_bytes());
    hash.update([0u8]);
    hash.update(password.as_bytes());
    hash.finalize().into()
}

impl fmt::Debug for Authority {
    /// The subject and the first four bytes of the certificate's fingerprint:
    /// enough to tell two CAs apart, and nothing of the material.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fingerprint: String = self
            .certificate_sha256
            .iter()
            .take(4)
            .map(|b| format!("{b:02x}"))
            .collect();
        f.debug_struct("Authority")
            .field("subject", &self.subject)
            .field("certificate_sha256", &fingerprint)
            .finish()
    }
}

/// The one thing `[MITM]` has to say about its CA when a profile is loaded:
/// `W0039`, once; and `W0040` when `http-api-tls` is on, so that the API is
/// to serve HTTPS with this profile, and the profile has no CA it could use
/// (`phase 4 M2 design 5.3`, `14.1`).
pub fn check(config: &Config) -> Diagnostics {
    check_with(config, default_clock())
}

fn check_with(config: &Config, clock: Clock) -> Diagnostics {
    let mut diags = Diagnostics::default();
    // one load answers both: `W0039` says why the CA is unusable and `W0040`
    // that the API has none
    let loaded = Authority::load_with(&config.mitm, &config.keystore, clock);
    let usable = matches!(loaded, Ok(Some(_)));
    let problems = match loaded {
        Err(p) => (!p.reported).then_some(p),
        Ok(None) if !config.mitm.effective.is_empty() => Some(CaProblem::new(
            NO_CA.into(),
            config.mitm.spans.hostname.clone(),
        )),
        Ok(_) => None,
    };
    if let Some(p) = problems {
        let mut d = Diagnostic::warning(codes::W_MITM_CA_UNUSABLE, p.message);
        if let Some(span) = p.span {
            d = d.at(span);
        }
        diags.push(d);
    }
    if config.general.http_api_tls && config.general.http_api.is_some() && !usable {
        diags.push(Diagnostic::warning(codes::W_API_TLS_NO_CA, API_TLS_NO_CA));
    }
    diags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mitm::fixtures;
    use crate::mitm::testing::{
        DAY, NOW, TestCa, at, ca_p12, ca_params, cfg_with_ca, clock_at, fixture_key, generated,
        make_ca,
    };
    use rcgen::{CustomExtension, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384, PKCS_ED25519};
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_config::diagnostic::Severity;
    use rurge_proto::testing::{p12_base64, p12_cert_base64};
    use std::path::Path;
    use x509_parser::extensions::GeneralName;
    use x509_parser::oid_registry::{
        OID_PKCS1_SHA256WITHRSA, OID_SIG_ECDSA_WITH_SHA384, OID_SIG_ED25519,
    };

    fn date(ts: i64) -> String {
        at(ts).date().to_string()
    }

    fn cfg_with_item(name: &str) -> MitmConfig {
        MitmConfig {
            ca_keystore_name: Some(name.into()),
            ..MitmConfig::default()
        }
    }

    /// A profile read from text, its `[MITM]` replaced by `mitm`.
    fn config_with(text: &str, mitm: MitmConfig) -> Config {
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        let mut config = loaded.config;
        config.mitm = mitm;
        config
    }

    fn keystore_of(text: &str) -> (Config, Diagnostics) {
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        (loaded.config, loaded.diagnostics)
    }

    fn p12_item(name: &str, p12: &str, password: &str) -> String {
        format!(
            "[Keystore]\n{name} = type=p12, base64={p12}, password={password}\n[Rule]\nFINAL,DIRECT\n"
        )
    }

    /// P-256, P-384, Ed25519 and an RSA CA made self-signed with the fixture
    /// key all load, and so does the P-256 one read from a `[Keystore]` item.
    #[test]
    fn good_cas_of_every_supported_key_load() {
        let clock = clock_at(NOW);
        let p256 = generated(&PKCS_ECDSA_P256_SHA256, "rurge test p256 CA");
        let p384 = generated(&PKCS_ECDSA_P384_SHA384, "rurge test p384 CA");
        let ed25519 = generated(&PKCS_ED25519, "rurge test ed25519 CA");
        let rsa = make_ca(
            ca_params("rurge test rsa CA"),
            fixture_key(fixtures::RSA_2048_KEY),
        );
        for ca in [&p256, &p384, &ed25519, &rsa] {
            let p12 = ca_p12(ca, "pw");
            let authority = Authority::load_at(&cfg_with_ca(&p12, "pw"), &[], clock.clone())
                .expect("a usable CA")
                .expect("a CA was named");
            assert_eq!(authority.ca_der(), ca.cert.as_ref());
        }

        let p12 = ca_p12(&p256, "pw");
        let (config, diags) = keystore_of(&p12_item("ca", &p12, "pw"));
        assert!(
            !diags.iter().any(|d| d.severity == Severity::Error),
            "{:?}",
            diags.iter().map(|d| d.to_string()).collect::<Vec<_>>()
        );
        let authority = Authority::load_at(&cfg_with_item("ca"), &config.keystore, clock)
            .expect("a usable CA")
            .expect("a CA was named");
        assert_eq!(authority.ca_der(), p256.cert.as_ref());
    }

    /// `ca-keystore-name` takes precedence, and there is no fallback to
    /// `ca-p12` when the item cannot be used (`phase 4 M2 design 5.1`).
    #[test]
    fn a_keystore_item_wins_and_does_not_fall_back() {
        let clock = clock_at(NOW);
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test precedence CA");
        let p12 = ca_p12(&ca, "pw");
        let (config, _) = keystore_of(&p12_item("ca", &p12, "pw"));

        // the item is used although `ca-p12` is not a PKCS#12 at all
        let cfg = MitmConfig {
            ca_p12: Some("QUJD".into()),
            ..cfg_with_item("ca")
        };
        let authority = Authority::load_at(&cfg, &config.keystore, clock.clone())
            .expect("the item is usable")
            .expect("a CA was named");
        assert_eq!(authority.ca_der(), ca.cert.as_ref());

        // an item that is not there is the problem, even with a good ca-p12
        let cfg = MitmConfig {
            ca_p12: Some(p12.clone()),
            ..cfg_with_item("missing")
        };
        let err = Authority::load_at(&cfg, &config.keystore, clock).unwrap_err();
        assert!(
            err.message
                .contains("`ca-keystore-name` names `missing`, which [Keystore] does not have"),
            "{}",
            err.message
        );
        assert!(err.message.starts_with("MITM is off: "), "{}", err.message);
    }

    #[test]
    fn a_keystore_item_that_is_not_pkcs12_is_a_problem() {
        let text = "[Keystore]\nkey = type=openssh-private-key, base64=QUJD, password=pw\n[Rule]\nFINAL,DIRECT\n";
        let (config, _) = keystore_of(text);
        let err =
            Authority::load_at(&cfg_with_item("key"), &config.keystore, clock_at(NOW)).unwrap_err();
        assert!(
            err.message
                .contains("`ca-keystore-name` names `key`, which is not a PKCS#12 item"),
            "{}",
            err.message
        );
        assert!(!err.reported);
    }

    /// A `[Keystore]` item whose Base64 does not decode is already named by
    /// `E0021` when the profile is read; `check` does not repeat it (`P5`
    /// item 4).
    #[test]
    fn a_keystore_item_with_bad_base64_is_left_to_e0021() {
        let text = "[Keystore]\nca = type=p12, base64=@@@, password=pw\n[Rule]\nFINAL,DIRECT\n";
        let (config, diags) = keystore_of(text);
        assert_eq!(
            diags
                .iter()
                .filter(|d| d.code == codes::E_KEYSTORE_BASE64)
                .count(),
            1,
            "{:?}",
            diags.iter().map(|d| d.to_string()).collect::<Vec<_>>()
        );
        let err =
            Authority::load_at(&cfg_with_item("ca"), &config.keystore, clock_at(NOW)).unwrap_err();
        assert!(err.reported);
        assert_eq!(
            err.message,
            "MITM is off: keystore item `ca`: `base64` is not valid Base64"
        );
        let config = config_with(text, cfg_with_item("ca"));
        assert!(
            check_with(&config, clock_at(NOW)).is_empty(),
            "{:?}",
            check_with(&config, clock_at(NOW))
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// An empty passphrase is a passphrase: a PKCS#12 written with one opens
    /// whether `ca-passphrase` is left out or written empty (`P2` item 5).
    #[test]
    fn an_empty_passphrase_opens_a_p12_written_with_one() {
        let clock = clock_at(NOW);
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test empty passphrase CA");
        let p12 = ca_p12(&ca, "");
        for passphrase in [None, Some(String::new())] {
            let cfg = MitmConfig {
                ca_p12: Some(p12.clone()),
                ca_passphrase: passphrase.clone(),
                ..MitmConfig::default()
            };
            let authority = Authority::load_at(&cfg, &[], clock.clone())
                .expect("the empty passphrase opens it")
                .expect("a CA was named");
            assert_eq!(authority.ca_der(), ca.cert.as_ref());
        }
    }

    /// One row per way a CA can be unusable: one `W0039` each, saying which,
    /// and none of them quoting the material or the passphrase.
    #[test]
    fn every_ca_problem_is_named_and_quotes_no_secret() {
        let clock = clock_at(NOW);
        let passphrase = "pw-7d1f";
        let good = generated(&PKCS_ECDSA_P256_SHA256, "rurge test problem CA");
        let good_p12 = ca_p12(&good, passphrase);
        let mut rows: Vec<(&str, MitmConfig, &str, String)> = Vec::new();

        rows.push((
            "bad Base64",
            cfg_with_ca("QUJD$$", passphrase),
            "not valid Base64",
            "QUJD$$".into(),
        ));
        rows.push((
            "wrong passphrase",
            cfg_with_ca(&good_p12, "pw-wrong-5e2b"),
            "cannot be decoded",
            good_p12.clone(),
        ));
        let cert_only = p12_cert_base64(good.cert.as_ref(), passphrase);
        rows.push((
            "no private key",
            cfg_with_ca(&cert_only, passphrase),
            "holds no private key",
            cert_only.clone(),
        ));

        let mut params = ca_params("rurge test not a CA");
        params.is_ca = IsCa::NoCa;
        let not_ca = make_ca(
            params,
            KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key"),
        );
        let p12 = ca_p12(&not_ca, passphrase);
        rows.push((
            "not a CA",
            cfg_with_ca(&p12, passphrase),
            "is not a CA",
            p12.clone(),
        ));

        let mut params = ca_params("rurge test no keyCertSign CA");
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        let no_sign = make_ca(
            params,
            KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key"),
        );
        let p12 = ca_p12(&no_sign, passphrase);
        rows.push((
            "lacks keyCertSign",
            cfg_with_ca(&p12, passphrase),
            "lacks keyCertSign",
            p12.clone(),
        ));

        let mut params = ca_params("rurge test expired CA");
        params.not_before = at(NOW - 40 * DAY);
        params.not_after = at(NOW - DAY);
        let expired = make_ca(
            params,
            KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key"),
        );
        let p12 = ca_p12(&expired, passphrase);
        rows.push((
            "expired",
            cfg_with_ca(&p12, passphrase),
            "expired on",
            p12.clone(),
        ));

        let mut params = ca_params("rurge test future CA");
        params.not_before = at(NOW + DAY);
        params.not_after = at(NOW + 40 * DAY);
        let future = make_ca(
            params,
            KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key"),
        );
        let p12 = ca_p12(&future, passphrase);
        rows.push((
            "not yet valid",
            cfg_with_ca(&p12, passphrase),
            "not valid until",
            p12.clone(),
        ));

        for (name, ca, key, phrase) in [
            (
                "P-521",
                fixtures::P521_CA,
                fixtures::P521_KEY,
                "cannot sign with the private key",
            ),
            (
                "no public key",
                fixtures::NO_PUBLIC_CA,
                fixtures::NO_PUBLIC_KEY,
                "cannot sign with the private key",
            ),
            (
                "multi-valued RDN",
                fixtures::MULTI_RDN_CA,
                fixtures::MULTI_RDN_KEY,
                "cannot issue certificates under",
            ),
            (
                "duplicate attribute",
                fixtures::DUP_ATTR_CA,
                fixtures::DUP_ATTR_KEY,
                "does not verify",
            ),
        ] {
            let cert = base64_bytes(ca).expect("the fixture is Base64");
            let key = base64_bytes(key).expect("the fixture key is Base64");
            let p12 = p12_base64(&[cert], &key, passphrase);
            rows.push((name, cfg_with_ca(&p12, passphrase), phrase, p12.clone()));
        }

        // a key that does not belong to the certificate beside it
        let other = generated(&PKCS_ECDSA_P256_SHA256, "rurge test other CA");
        let mismatched = p12_base64(
            &[good.cert.as_ref().to_vec()],
            &other.key.serialize_der(),
            passphrase,
        );
        rows.push((
            "mismatched pair",
            cfg_with_ca(&mismatched, passphrase),
            "does not verify",
            mismatched.clone(),
        ));

        for (name, cfg, phrase, secret) in rows {
            // the passphrase this row writes, if it writes one
            let written = cfg.ca_passphrase.clone().unwrap_or_default();
            let config = config_with("", cfg);
            let diags = check_with(&config, clock.clone()).sorted();
            let said: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
            assert_eq!(said.len(), 1, "{name}: {said:?}");
            let d = diags.iter().next().expect("one problem");
            assert_eq!(d.code, codes::W_MITM_CA_UNUSABLE, "{name}");
            assert!(
                d.message.starts_with("MITM is off: "),
                "{name}: {}",
                d.message
            );
            assert!(d.message.contains(phrase), "{name}: {}", d.message);
            assert!(!d.message.contains(&secret), "{name}: {}", d.message);
            assert!(!d.message.contains(passphrase), "{name}: {}", d.message);
            assert!(
                d.hint
                    .is_none_or(|h| !h.contains(&secret) && !h.contains(passphrase)),
                "{name}: {}",
                d.message
            );
            if !written.is_empty() {
                assert!(!d.message.contains(&written), "{name}: {}", d.message);
                assert!(
                    d.hint.is_none_or(|h| !h.contains(&written)),
                    "{name}: {}",
                    d.message
                );
            }
        }
    }

    /// No CA is only a problem when hosts are listed for decryption.
    #[test]
    fn no_ca_warns_only_when_hosts_are_listed() {
        let clock = clock_at(NOW);
        for (text, warned) in [
            ("[MITM]\nhostname = a.test\n", true),
            ("[MITM]\n", false),
            (
                "[MITM]\nhostname = a.test\nhostname-disabled = a.test\n",
                false,
            ),
        ] {
            let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
            // a `[MITM]` that names no CA is `Ok(None)` whatever the list
            // says: the warning is `check`'s, not `load`'s
            assert!(
                matches!(
                    Authority::load_at(&loaded.config.mitm, &loaded.config.keystore, clock.clone()),
                    Ok(None)
                ),
                "{text}"
            );
            let diags = check_with(&loaded.config, clock.clone()).sorted();
            assert_eq!(diags.iter().count(), usize::from(warned), "{text}");
            for d in diags.iter() {
                assert_eq!(d.code, codes::W_MITM_CA_UNUSABLE);
                assert!(
                    d.message.contains("lists hosts to decrypt"),
                    "{}",
                    d.message
                );
                assert_eq!(d.span.as_ref().map(|s| s.line), Some(2));
            }
        }
    }

    #[test]
    fn the_debug_of_an_authority_shows_the_certificate_not_the_material() {
        let clock = clock_at(NOW);
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test debug CA");
        let p12 = ca_p12(&ca, "pw-7d1f");
        let authority = Authority::load_at(&cfg_with_ca(&p12, "pw-7d1f"), &[], clock)
            .expect("a usable CA")
            .expect("a CA was named");
        let shown = format!("{authority:?}");
        assert!(shown.contains("rurge test debug CA"), "{shown}");
        let fingerprint: String = authority
            .certificate_sha256
            .iter()
            .take(4)
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(shown.contains(&fingerprint), "{shown}");
        assert!(!shown.contains(&p12), "{shown}");
        assert!(!shown.contains("pw-7d1f"), "{shown}");
        let material: String = authority
            .material
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert!(!shown.contains(&material), "{shown}");
    }

    /// The same material is the Base64 as written and the passphrase (`P8`);
    /// nothing named at all is never the same.
    #[test]
    fn same_material_compares_the_base64_and_the_passphrase() {
        let clock = clock_at(NOW);
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test material CA");
        let p12 = ca_p12(&ca, "pw-7d1f");
        let cfg = cfg_with_ca(&p12, "pw-7d1f");
        let authority = Authority::load_at(&cfg, &[], clock.clone())
            .expect("a usable CA")
            .expect("a CA was named");
        assert!(authority.same_material(&cfg, &[]));
        assert!(!authority.same_material(&cfg_with_ca(&p12, "another-pw"), &[]));

        // the same material as a [Keystore] item
        let (config, _) = keystore_of(&p12_item("ca", &p12, "pw-7d1f"));
        assert!(authority.same_material(&cfg_with_item("ca"), &config.keystore));
        assert!(!authority.same_material(&cfg_with_item("missing"), &config.keystore));

        assert!(!authority.same_material(&MitmConfig::default(), &[]));
        assert!(!authority.same_material(&cfg_with_item("ca"), &[]));
    }

    /// A CA that expires stops being usable when it does (`P16`).
    #[test]
    fn an_expired_authority_is_not_usable_now() {
        let mut params = ca_params("rurge test expiry CA");
        params.not_before = at(NOW - 30 * DAY);
        params.not_after = at(NOW + 30 * DAY);
        let ca = make_ca(
            params,
            KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key"),
        );
        let p12 = ca_p12(&ca, "pw");
        let authority = Authority::load_at(&cfg_with_ca(&p12, "pw"), &[], clock_at(NOW))
            .expect("a usable CA")
            .expect("a CA was named");
        assert!(authority.usable_now());
        // the period runs through `notAfter`, for rurge and for the verifiers
        // clients use (RFC 5280 4.1.2.5; rustls-webpki only refuses
        // `time > not_after`)
        authority.set_clock(clock_at(NOW + 30 * DAY));
        assert!(authority.usable_now());
        assert!(
            matches!(
                Authority::load_at(&cfg_with_ca(&p12, "pw"), &[], clock_at(NOW + 30 * DAY)),
                Ok(Some(_))
            ),
            "a CA is still usable at the instant it expires"
        );
        authority.set_clock(clock_at(NOW + 30 * DAY + 1));
        assert!(!authority.usable_now());
    }

    /// The authority is shared across tasks from the next task on: it has to
    /// go between threads.
    #[test]
    fn an_authority_goes_between_threads() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<Authority>();
        send_and_sync::<Arc<Authority>>();
        send_and_sync::<CaProblem>();
    }

    /// The dates `P5` writes into the messages are the ones read off the
    /// certificate, in `YYYY-MM-DD`.
    #[test]
    fn a_problem_names_the_certificates_own_dates() {
        let clock = clock_at(NOW);
        let mut params = ca_params("rurge test dated CA");
        params.not_before = at(NOW - 40 * DAY);
        params.not_after = at(NOW - 10 * DAY);
        let ca = make_ca(
            params,
            KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("a key"),
        );
        let p12 = ca_p12(&ca, "pw");
        let err = Authority::load_at(&cfg_with_ca(&p12, "pw"), &[], clock).unwrap_err();
        assert!(
            err.message
                .ends_with(&format!("expired on {}", date(NOW - 10 * DAY))),
            "{}",
            err.message
        );
    }

    /// `P5`: items 2 and 3 are reported at the `ca-keystore-name` line, the
    /// others at the line of the source the CA came from — not at the
    /// passphrase's line.
    #[test]
    fn a_problem_is_reported_at_the_line_of_the_key_that_names_the_ca() {
        let clock = clock_at(NOW);
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test span CA");
        let p12 = ca_p12(&ca, "pw-7d1f");

        // the CA is `ca-p12` (line 4); the passphrase is line 3
        let text = "[MITM]\nhostname = a.test\nca-passphrase = pw-7d1f\nca-p12 = QUJDRA\n";
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        let diags = check_with(&loaded.config, clock.clone()).sorted();
        let said: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert_eq!(said, [codes::W_MITM_CA_UNUSABLE], "{text}");
        assert_eq!(
            diags.iter().next().unwrap().span.as_ref().map(|s| s.line),
            Some(4),
            "{text}"
        );

        // an item that is not there (line 2), although `ca-p12` (line 4)
        // holds a CA that would load
        let text =
            format!("[MITM]\nca-keystore-name = missing\nhostname = a.test\nca-p12 = {p12}\n");
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        let diags = check_with(&loaded.config, clock).sorted();
        let said: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].contains("which [Keystore] does not have"),
            "{}",
            said[0]
        );
        assert_eq!(
            diags.iter().next().unwrap().span.as_ref().map(|s| s.line),
            Some(2),
            "the ca-keystore-name line"
        );
    }

    /// `W0040`: `http-api-tls` is on and `http-api` names an address, but the
    /// profile has no CA that the API could serve HTTPS with — the API does
    /// not start with it (`phase 4 M2 design 14.1`, `the phase 4 / M2c plan's
    /// P2`). It is one more warning than `W0039` when the CA cannot be read,
    /// and it carries no line: `[General]` does not record where
    /// `http-api-tls` stands.
    #[test]
    fn an_api_that_cannot_start_is_named() {
        let clock = clock_at(NOW);
        let general = "[General]\nhttp-api = k@127.0.0.1:6171\nhttp-api-tls = true\n";
        let rule = "[Rule]\nFINAL,DIRECT\n";

        // no `[MITM]` at all: one warning, the message and the hint of the
        // code, and no line
        let text = format!("{general}{rule}");
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        let diags = check_with(&loaded.config, clock.clone());
        let said: Vec<&Diagnostic> = diags.iter().collect();
        assert_eq!(said.len(), 1, "{text}: {said:?}");
        assert_eq!(said[0].code, codes::W_API_TLS_NO_CA, "{text}");
        assert_eq!(said[0].severity, Severity::Warning, "{text}");
        assert_eq!(
            said[0].message,
            "http-api-tls is on but there is no usable MITM CA: the API does not start with this profile",
            "{text}"
        );
        assert_eq!(said[0].message, API_TLS_NO_CA, "{text}");
        assert_eq!(said[0].span, None, "{text}");
        assert_eq!(
            said[0].hint,
            Some("configure a usable MITM CA, or turn http-api-tls off"),
            "{text}"
        );

        // a CA that cannot be read: `W0039` at its line first, then this one,
        // and neither quotes the material or the passphrase
        let text = format!("{general}[MITM]\nca-p12 = QUJDRA\nca-passphrase = pw-7d1f\n{rule}");
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        let diags = check_with(&loaded.config, clock.clone());
        let said: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert_eq!(
            said,
            [codes::W_MITM_CA_UNUSABLE, codes::W_API_TLS_NO_CA],
            "{text}"
        );
        for d in diags.iter() {
            for secret in ["QUJDRA", "pw-7d1f"] {
                assert!(!d.message.contains(secret), "{text}: {}", d.message);
                assert!(d.hint.is_none_or(|h| !h.contains(secret)), "{text}: {d}");
            }
        }

        // a CA the load has already named (`E0021`): this one alone
        let text = format!(
            "{general}[Keystore]\nca = type=p12, base64=@@@, password=pw\n\
             [MITM]\nca-keystore-name = ca\n{rule}"
        );
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert_eq!(
            loaded
                .diagnostics
                .iter()
                .filter(|d| d.code == codes::E_KEYSTORE_BASE64)
                .count(),
            1,
            "{text}"
        );
        let diags = check_with(&loaded.config, clock.clone());
        let said: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert_eq!(said, [codes::W_API_TLS_NO_CA], "{text}");

        // a CA that loads: the API has one to serve HTTPS with
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test API CA");
        let p12 = ca_p12(&ca, "pw-7d1f");
        let text = format!("{general}[MITM]\nca-p12 = {p12}\nca-passphrase = pw-7d1f\n{rule}");
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            check_with(&loaded.config, clock.clone()).is_empty(),
            "a profile whose CA loads warns about nothing"
        );

        // no `http-api`: no API is to start, so there is nothing to warn of
        let text = format!("[General]\nhttp-api-tls = true\n{rule}");
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            check_with(&loaded.config, clock.clone()).is_empty(),
            "{text}"
        );

        // `http-api-tls` off, or not written: the API speaks plain HTTP
        for general in [
            "[General]\nhttp-api = k@127.0.0.1:6171\nhttp-api-tls = false\n",
            "[General]\nhttp-api = k@127.0.0.1:6171\n",
        ] {
            let text = format!("{general}{rule}");
            let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
            assert!(
                check_with(&loaded.config, clock.clone()).is_empty(),
                "{text}"
            );
        }
    }

    /// One certificate for `name`, signed by `ca`.
    async fn leaf_of(ca: &TestCa, name: &str) -> Arc<CertifiedKey> {
        let authority = Arc::new(
            Authority::load_at(&cfg_with_ca(&ca_p12(ca, "pw"), "pw"), &[], clock_at(NOW))
                .expect("a usable CA")
                .expect("a CA was named"),
        );
        authority
            .leaf(&HostName::parse(name))
            .await
            .expect("a leaf")
    }

    /// The extension an issued certificate has of `kind`, as raw bytes.
    fn extension(
        cert: &x509_parser::certificate::X509Certificate<'_>,
        which: &str,
    ) -> Option<Vec<u8>> {
        cert.iter_extensions()
            .find_map(|e| match (which, e.parsed_extension()) {
                ("ski", ParsedExtension::SubjectKeyIdentifier(key)) => Some(key.0.to_vec()),
                ("aki", ParsedExtension::AuthorityKeyIdentifier(key)) => {
                    key.key_identifier.as_ref().map(|k| k.0.to_vec())
                }
                _ => None,
            })
    }

    /// Every item `phase 4 M2 design 6` writes into an issued certificate,
    /// read back with x509-parser (`P9`).
    #[tokio::test]
    async fn a_leaf_says_what_the_design_says() {
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test leaf CA");

        let leaf = leaf_of(&ca, "site.test").await;
        let (_, cert) = parse_x509_certificate(leaf.cert[0].as_ref()).expect("a certificate");

        let san = cert
            .subject_alternative_name()
            .expect("the extensions parse")
            .expect("a subject alternative name");
        assert_eq!(san.value.general_names.len(), 1, "one name");
        assert!(
            matches!(san.value.general_names[0], GeneralName::DNSName(name) if name == "site.test"),
            "{:?}",
            san.value.general_names
        );

        let common: Vec<String> = cert
            .subject()
            .iter_common_name()
            .map(|cn| cn.as_str().expect("a string").to_string())
            .collect();
        assert_eq!(common, ["site.test"]);

        let eku = cert
            .extended_key_usage()
            .expect("the extensions parse")
            .expect("an extended key usage");
        assert!(eku.value.server_auth, "serverAuth");
        assert!(!eku.value.client_auth && !eku.value.any, "serverAuth alone");

        let ku = cert
            .key_usage()
            .expect("the extensions parse")
            .expect("a key usage");
        assert!(ku.value.digital_signature(), "digitalSignature");
        assert!(
            !ku.value.key_cert_sign() && !ku.value.key_encipherment() && !ku.value.crl_sign(),
            "digitalSignature alone"
        );

        // the authority key identifier is the CA's subject key identifier
        let (_, ca_cert) = parse_x509_certificate(ca.cert.as_ref()).expect("the CA certificate");
        let ca_ski = extension(&ca_cert, "ski").expect("the CA has a subject key identifier");
        assert_eq!(extension(&cert, "aki"), Some(ca_ski));

        // a leaf carries neither a subject key identifier nor basic constraints
        assert_eq!(extension(&cert, "ski"), None);
        assert!(
            cert.basic_constraints()
                .expect("the extensions parse")
                .is_none(),
            "no basic constraints"
        );

        // a positive serial number of 16 bytes
        let serial = cert.raw_serial();
        assert_eq!(serial.len(), 16);
        assert_eq!(serial[0] & 0x80, 0, "a positive serial number");

        // an address is an address
        let leaf = leaf_of(&ca, "127.0.0.1").await;
        let (_, cert) = parse_x509_certificate(leaf.cert[0].as_ref()).expect("a certificate");
        let san = cert
            .subject_alternative_name()
            .expect("the extensions parse")
            .expect("a subject alternative name");
        assert!(
            matches!(san.value.general_names[0], GeneralName::IPAddress(_)),
            "{:?}",
            san.value.general_names
        );

        // a name of more than 64 characters has no common name
        let long = format!("{}.test", "a".repeat(60));
        assert_eq!(long.chars().count(), 65);
        let leaf = leaf_of(&ca, &long).await;
        let (_, cert) = parse_x509_certificate(leaf.cert[0].as_ref()).expect("a certificate");
        assert_eq!(cert.subject().iter_common_name().count(), 0);

        // the signature is the CA's own
        let p384 = generated(&PKCS_ECDSA_P384_SHA384, "rurge test p384 leaf CA");
        let rsa = make_ca(
            ca_params("rurge test rsa leaf CA"),
            fixture_key(fixtures::RSA_2048_KEY),
        );
        let ed25519 = generated(&PKCS_ED25519, "rurge test ed25519 leaf CA");
        for (ca, signature) in [
            (&p384, OID_SIG_ECDSA_WITH_SHA384),
            (&rsa, OID_PKCS1_SHA256WITHRSA),
            (&ed25519, OID_SIG_ED25519),
        ] {
            let leaf = leaf_of(ca, "site.test").await;
            let (_, cert) = parse_x509_certificate(leaf.cert[0].as_ref()).expect("a certificate");
            assert_eq!(
                cert.signature_algorithm.algorithm.to_id_string(),
                signature.to_id_string()
            );
        }
    }

    /// An issued certificate starts two days before now and lives 47 days,
    /// and never reaches outside the CA's own validity (`phase 4 M2 design
    /// 6`).
    #[tokio::test]
    async fn a_leaf_lives_47_days_within_its_ca() {
        let flimsy =
            |params: CertificateParams| make_ca(params, KeyPair::generate().expect("a key"));

        // a CA that covers both ends: two days back, 47 days on
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test dated leaf CA");
        let leaf = leaf_of(&ca, "site.test").await;
        let (_, cert) = parse_x509_certificate(leaf.cert[0].as_ref()).expect("a certificate");
        assert_eq!(cert.validity().not_before.timestamp(), NOW - 2 * DAY);
        assert_eq!(
            cert.validity().not_after.timestamp(),
            NOW - 2 * DAY + LEAF_DAYS * DAY
        );

        // a CA that begins yesterday: the leaf begins with it
        let mut params = ca_params("rurge test recent leaf CA");
        params.not_before = at(NOW - DAY);
        params.not_after = at(NOW + 40 * DAY);
        let ca = flimsy(params);
        let leaf = leaf_of(&ca, "site.test").await;
        let (_, cert) = parse_x509_certificate(leaf.cert[0].as_ref()).expect("a certificate");
        assert_eq!(cert.validity().not_before.timestamp(), NOW - DAY);

        // a CA that ends in ten days: the leaf ends with it
        let mut params = ca_params("rurge test short leaf CA");
        params.not_after = at(NOW + 10 * DAY);
        let ca = flimsy(params);
        let leaf = leaf_of(&ca, "site.test").await;
        let (_, cert) = parse_x509_certificate(leaf.cert[0].as_ref()).expect("a certificate");
        assert_eq!(cert.validity().not_after.timestamp(), NOW + 10 * DAY);
    }

    /// Without a subject key identifier on the CA the leaf carries no
    /// authority key identifier (`phase 4 M2 design 6`, `M2-D6`).
    #[tokio::test]
    async fn a_ca_without_a_subject_key_identifier_gives_leaves_without_an_authority_key_identifier()
     {
        // rcgen writes a subject key identifier for `IsCa::Ca` and never for
        // `IsCa::NoCa`, so the CA flag comes from an extension of our own
        let mut params = ca_params("rurge test no-SKI CA");
        params.is_ca = IsCa::NoCa;
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let mut basic =
            CustomExtension::from_oid_content(&[2, 5, 29, 19], vec![0x30, 0x03, 0x01, 0x01, 0xff]);
        basic.set_criticality(true);
        params.custom_extensions = vec![basic];
        let ca = make_ca(params, KeyPair::generate().expect("a key"));

        let (_, ca_cert) = parse_x509_certificate(ca.cert.as_ref()).expect("the CA certificate");
        assert_eq!(extension(&ca_cert, "ski"), None, "a CA without an SKI");

        let leaf = leaf_of(&ca, "site.test").await;
        let (_, cert) = parse_x509_certificate(leaf.cert[0].as_ref()).expect("a certificate");
        assert_eq!(extension(&cert, "aki"), None);
    }

    /// One name keeps one certificate until a third of its life is left; then
    /// the next connection signs a new one (`phase 4 M2 design 6`).
    #[tokio::test]
    async fn one_name_one_leaf_until_a_third_of_its_life_is_left() {
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test cache CA");
        let authority = Arc::new(
            Authority::load_at(&cfg_with_ca(&ca_p12(&ca, "pw"), "pw"), &[], clock_at(NOW))
                .expect("a usable CA")
                .expect("a CA was named"),
        );
        let name = HostName::parse("cache.test");

        let first = authority.leaf(&name).await.expect("a leaf");
        let again = authority.leaf(&name).await.expect("a leaf");
        assert!(Arc::ptr_eq(&first, &again), "the same certificate");
        assert_eq!(authority.signature_count(), 1, "signed once");

        // the instant a third of the life is left: one second earlier is
        // still the same certificate
        let (_, cert) = parse_x509_certificate(first.cert[0].as_ref()).expect("a certificate");
        let not_before = cert.validity().not_before.timestamp();
        let not_after = cert.validity().not_after.timestamp();
        let due = not_after - (not_after - not_before) / 3;
        authority.set_clock(clock_at(due - 1));
        let still = authority.leaf(&name).await.expect("a leaf");
        assert!(Arc::ptr_eq(&first, &still), "still the same certificate");
        assert_eq!(authority.signature_count(), 1);

        authority.set_clock(clock_at(due));
        let second = authority.leaf(&name).await.expect("a leaf");
        assert!(!Arc::ptr_eq(&first, &second), "a new certificate");
        assert_eq!(authority.signature_count(), 2);
    }

    /// The cache holds a thousand names; the next one pushes the oldest out
    /// (`phase 4 M2 design 6`).
    #[tokio::test]
    async fn the_thousand_and_first_name_pushes_out_the_oldest() {
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test slots CA");
        let authority = Arc::new(
            Authority::load_at(&cfg_with_ca(&ca_p12(&ca, "pw"), "pw"), &[], clock_at(NOW))
                .expect("a usable CA")
                .expect("a CA was named"),
        );

        let oldest = authority
            .leaf(&HostName::parse("n0.test"))
            .await
            .expect("a leaf");
        for i in 1..=LEAF_SLOTS {
            let name = HostName::parse(&format!("n{i}.test"));
            authority.leaf(&name).await.expect("a leaf");
        }
        assert_eq!(authority.signature_count(), LEAF_SLOTS + 1);

        let again = authority
            .leaf(&HostName::parse("n0.test"))
            .await
            .expect("a leaf");
        assert_eq!(authority.signature_count(), LEAF_SLOTS + 2);
        assert!(!Arc::ptr_eq(&oldest, &again), "the oldest slot was reused");
    }

    /// Connections that arrive together for one name sign once and share the
    /// certificate (`phase 4 M2 design 6`, `M2-D7`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_requests_for_one_name_sign_once() {
        let ca = generated(&PKCS_ECDSA_P256_SHA256, "rurge test concurrent CA");
        let authority = Arc::new(
            Authority::load_at(&cfg_with_ca(&ca_p12(&ca, "pw"), "pw"), &[], clock_at(NOW))
                .expect("a usable CA")
                .expect("a CA was named"),
        );
        let name = HostName::parse("concurrent.test");
        let mut tasks = Vec::new();
        for _ in 0..16 {
            let authority = Arc::clone(&authority);
            let name = name.clone();
            tasks.push(tokio::spawn(async move { authority.leaf(&name).await }));
        }
        let mut leaves = Vec::new();
        for task in tasks {
            leaves.push(task.await.expect("a task").expect("a leaf"));
        }
        assert_eq!(authority.signature_count(), 1, "signed once");
        for leaf in &leaves {
            assert!(Arc::ptr_eq(&leaves[0], leaf), "one certificate");
        }
    }
}
