//! `server-fingerprint` (phase 2 M4 design 5.2).

use rurge_config::spec::HostKeyPin;
use russh::keys::PublicKeyOrCertificate;
use russh::keys::ssh_key::encoding::Encode as _;

/// Whether the server's host key is one of `pins`; with no pins, every key
/// is. A host certificate is judged by the key it certifies.
pub fn host_key_allowed(pins: &[HostKeyPin], key: &PublicKeyOrCertificate) -> bool {
    if pins.is_empty() {
        return true;
    }
    let blob = match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.to_bytes(),
        PublicKeyOrCertificate::Certificate(cert) => cert
            .public_key()
            .encode_vec()
            .map_err(russh::keys::ssh_key::Error::from),
    };
    blob.is_ok_and(|blob| pins.iter().any(|pin| pin.blob == blob))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::random_key;
    use russh::keys::ssh_key::certificate::{Builder, CertType};
    use russh::keys::ssh_key::{Algorithm, PublicKey};

    fn pin_of(key: &PublicKey) -> HostKeyPin {
        HostKeyPin {
            algorithm: key.algorithm().to_string(),
            blob: key.to_bytes().unwrap(),
        }
    }

    fn plain(key: &PublicKey) -> PublicKeyOrCertificate {
        PublicKeyOrCertificate::PublicKey {
            key: key.clone(),
            hash_alg: None,
        }
    }

    #[test]
    fn a_pinned_key_passes_and_any_other_does_not() {
        let server = random_key(Algorithm::Ed25519);
        let other = random_key(Algorithm::Ed25519);
        let pins = [pin_of(other.public_key()), pin_of(server.public_key())];
        assert!(host_key_allowed(&pins, &plain(server.public_key())));
        assert!(!host_key_allowed(&pins[..1], &plain(server.public_key())));
    }

    #[test]
    fn without_pins_every_key_passes() {
        let server = random_key(Algorithm::Ed25519);
        assert!(host_key_allowed(&[], &plain(server.public_key())));
    }

    /// A host certificate is judged by the key it certifies, whoever signed
    /// it.
    #[test]
    fn a_host_certificate_is_judged_by_its_key() {
        let host = random_key(Algorithm::Ed25519);
        let ca = random_key(Algorithm::Ed25519);
        let mut builder = Builder::new_with_random_nonce(
            &mut rand::rng(),
            host.public_key().key_data().clone(),
            0,
            u64::MAX,
        )
        .unwrap();
        builder.cert_type(CertType::Host).unwrap();
        builder.valid_principal("s.test").unwrap();
        let cert = builder.sign(&ca).unwrap();
        let shown = PublicKeyOrCertificate::Certificate(cert);
        assert!(host_key_allowed(&[pin_of(host.public_key())], &shown));
        assert!(!host_key_allowed(&[pin_of(ca.public_key())], &shown));
    }
}
