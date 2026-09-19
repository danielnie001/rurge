//! Why an outbound could not be built from its spec.

use crate::keystore::decode_p12;
use crate::transport::tls::TlsClient;
use rurge_config::spec::TlsOpts;
use rurge_config::{HostName, KeystoreItem};
use rustls::RootCertStore;
use std::fmt;
use std::sync::Arc;

/// The text is shown to the user (`rurge check`): never put a secret in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildError {
    pub message: String,
}

impl BuildError {
    pub fn new(message: impl Into<String>) -> BuildError {
        BuildError {
            message: message.into(),
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BuildError {}

/// The TLS layer of a policy, when it has one. `client-cert` is looked up in
/// `keystore` and decoded here, so a broken p12 surfaces at build time.
pub fn tls_client(
    opts: Option<&TlsOpts>,
    server: &HostName,
    default_alpn: &[&str],
    keystore: &[KeystoreItem],
    roots: Arc<RootCertStore>,
) -> Result<Option<TlsClient>, BuildError> {
    let Some(opts) = opts else {
        return Ok(None);
    };
    let identity = match &opts.client_cert {
        None => None,
        Some(name) => {
            let item = keystore
                .iter()
                .find(|k| &k.name == name)
                .ok_or_else(|| BuildError::new(format!("keystore item `{name}` does not exist")))?;
            Some(decode_p12(item)?)
        }
    };
    TlsClient::build(opts, server, default_alpn, identity, roots).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_errors_are_plain_messages() {
        let e = BuildError::new("keystore item `cert1` cannot be decoded");
        assert_eq!(e.to_string(), "keystore item `cert1` cannot be decoded");
        assert_eq!(
            e,
            BuildError {
                message: "keystore item `cert1` cannot be decoded".into()
            }
        );
        let _: &dyn std::error::Error = &e;
    }
}
