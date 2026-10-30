//! The parts of the cases under `mitm` that more than one of them needs: a
//! certificate authority made by rcgen, the instant the cases that use the
//! OpenSSL fixtures pin (`P7`), and the `[MITM]` that hands an `Authority` a
//! CA.
//!
//! A CA, its key and a PKCS#12 are made in memory; nothing here reads a file
//! or touches a trust store (`phase 4 M2 design 17`).

use super::authority::Clock;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rurge_config::mitm::MitmConfig;
use rurge_proto::keystore::base64_bytes;
use rurge_proto::testing::p12_base64;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::sync::Arc;
use time::OffsetDateTime;

/// 2027-01-01T00:00:00Z: the instant every case that uses the OpenSSL
/// fixtures pins (`P7`).
pub(crate) const NOW: i64 = 1_798_761_600;
pub(crate) const DAY: i64 = 86_400;

pub(crate) fn at(ts: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(ts).expect("a timestamp")
}

pub(crate) fn clock_at(ts: i64) -> Clock {
    Arc::new(move || at(ts))
}

/// A certificate authority made by rcgen, and its key.
pub(crate) struct TestCa {
    pub(crate) cert: CertificateDer<'static>,
    pub(crate) key: KeyPair,
}

/// The shape of every CA the cases make: a CA that may sign certificates.
pub(crate) fn ca_params(cn: &str) -> CertificateParams {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.distinguished_name.push(DnType::CommonName, cn);
    params
}

pub(crate) fn make_ca(params: CertificateParams, key: KeyPair) -> TestCa {
    let cert = params.self_signed(&key).expect("a CA certificate");
    TestCa {
        cert: cert.der().clone(),
        key,
    }
}

pub(crate) fn generated(algorithm: &'static rcgen::SignatureAlgorithm, cn: &str) -> TestCa {
    make_ca(
        ca_params(cn),
        KeyPair::generate_for(algorithm).expect("a CA key"),
    )
}

/// A key of the OpenSSL fixtures (`P6`), as rcgen takes it.
pub(crate) fn fixture_key(base64: &str) -> KeyPair {
    let der = base64_bytes(base64).expect("the fixture is Base64");
    KeyPair::try_from(&PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der)))
        .expect("a usable fixture key")
}

/// The CA and its key as a Base64 PKCS#12, the shape `ca-p12` has.
pub(crate) fn ca_p12(ca: &TestCa, password: &str) -> String {
    p12_base64(
        &[ca.cert.as_ref().to_vec()],
        &ca.key.serialize_der(),
        password,
    )
}

/// A `[MITM]` whose CA is the Base64 PKCS#12 `base64`, opened with
/// `passphrase`.
pub(crate) fn cfg_with_ca(base64: &str, passphrase: &str) -> MitmConfig {
    MitmConfig {
        ca_p12: Some(base64.into()),
        ca_passphrase: Some(passphrase.into()),
        ..MitmConfig::default()
    }
}
