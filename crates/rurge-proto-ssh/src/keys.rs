//! `[Keystore]` OpenSSH private keys, decoded at build time (phase 2 M4
//! design 4.5).

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use rurge_config::KeystoreItem;
use rurge_proto::BuildError;
use russh::keys::ssh_key::{self, Algorithm};
use russh::keys::{Error, PrivateKey, decode_secret_key};

/// Decodes the private key an `ssh` policy names. The error names the item;
/// it never repeats any of the material.
pub fn decode_private_key(item: &KeystoreItem) -> Result<PrivateKey, BuildError> {
    let name = &item.name;
    let file = STANDARD
        .decode(&item.base64)
        .or_else(|_| STANDARD_NO_PAD.decode(&item.base64))
        .map_err(|_| {
            BuildError::new(format!(
                "keystore item `{name}`: `base64` is not valid Base64"
            ))
        })?;
    let not_a_key = || {
        BuildError::new(format!(
            "keystore item `{name}` is not an OpenSSH private key"
        ))
    };
    let unsupported = || {
        BuildError::new(format!(
            "keystore item `{name}` is not an Ed25519, ECDSA or RSA key"
        ))
    };
    let text = String::from_utf8(file).map_err(|_| not_a_key())?;
    let key = decode_secret_key(&text, None).map_err(|e| match e {
        // the manual's `password` is for p12 files only
        Error::KeyIsEncrypted => BuildError::new(format!(
            "keystore item `{name}` is protected by a passphrase, which rurge cannot use; remove the passphrase"
        )),
        Error::UnsupportedKeyType { .. }
        | Error::UnknownAlgorithm(_)
        | Error::SshKey(
            ssh_key::Error::AlgorithmUnsupported { .. } | ssh_key::Error::AlgorithmUnknown,
        ) => unsupported(),
        _ => not_a_key(),
    })?;
    // a DSA key decodes (only signing with one needs the `dsa` feature), and
    // so does a security key that needs its hardware
    match key.algorithm() {
        Algorithm::Ed25519 | Algorithm::Ecdsa { .. } | Algorithm::Rsa { .. } => Ok(key),
        _ => Err(unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{DSA_KEY, ED25519_WITH_PASSPHRASE, RSA_KEY, keystore_item, random_key};
    use russh::keys::ssh_key::{EcdsaCurve, LineEnding};

    #[test]
    fn ed25519_ecdsa_and_rsa_keys_decode() {
        for algorithm in [
            Algorithm::Ed25519,
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP256,
            },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP384,
            },
            Algorithm::Ecdsa {
                curve: EcdsaCurve::NistP521,
            },
        ] {
            let key = random_key(algorithm.clone());
            let text = key.to_openssh(LineEnding::LF).unwrap();
            let decoded = decode_private_key(&keystore_item("key1", &text)).unwrap();
            assert_eq!(decoded.algorithm(), algorithm);
            assert_eq!(decoded.public_key(), key.public_key());
        }
        let rsa = decode_private_key(&keystore_item("key1", RSA_KEY)).unwrap();
        assert!(matches!(rsa.algorithm(), Algorithm::Rsa { .. }));
    }

    /// The failures name the item and what is wrong with it, never the
    /// material.
    #[test]
    fn keys_rurge_cannot_use_are_named_not_quoted() {
        let cases = [
            (
                keystore_item("key1", ED25519_WITH_PASSPHRASE),
                "keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase",
            ),
            (
                keystore_item("key1", DSA_KEY),
                "keystore item `key1` is not an Ed25519, ECDSA or RSA key",
            ),
            (
                keystore_item("key1", "hello, world"),
                "keystore item `key1` is not an OpenSSH private key",
            ),
        ];
        for (item, expected) in cases {
            let message = decode_private_key(&item).unwrap_err().message;
            assert_eq!(message, expected);
            assert!(!message.contains(&item.base64[..16]));
        }
        let mut bad = keystore_item("key1", RSA_KEY);
        bad.base64 = "!!not base64!!".into();
        assert_eq!(
            decode_private_key(&bad).unwrap_err().message,
            "keystore item `key1`: `base64` is not valid Base64"
        );
    }
}
