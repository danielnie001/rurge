//! `BuildError` (why an outbound could not be built from its spec) and the
//! helpers shared by the outbounds' `from_spec` (currently `tls_client`).

use crate::keystore::decode_p12;
use crate::transport::tls::TlsClient;
use rurge_config::spec::{PolicySpec, TlsOpts};
use rurge_config::{HostName, KeystoreItem};
use rurge_net::connector::Target;
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

/// The proxy server `spec` names. Every protocol that dials a server needs
/// both halves; a spec without them never came out of `rurge-config`.
pub fn server_of(spec: &PolicySpec) -> Result<Target, BuildError> {
    match (&spec.server, spec.port) {
        (Some(host), Some(port)) => Ok(Target::new(host.clone(), port)),
        _ => Err(BuildError::new(format!(
            "a {} policy needs a server and a port",
            spec.kind.keyword()
        ))),
    }
}

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
    use rurge_config::{KeystoreType, Span};
    use std::path::Path;

    #[test]
    fn a_spec_without_a_server_is_refused_by_name_of_its_protocol() {
        use rurge_config::config::{LoadOptions, from_text};
        let text = "[Proxy]\nT = trojan, proxy.test, 443, password=pw\n[Rule]\nFINAL,DIRECT\n";
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        let mut spec = loaded.config.spec("T").expect("a spec").clone();
        let server = server_of(&spec).unwrap();
        assert_eq!(
            (server.host.to_string(), server.port),
            ("proxy.test".to_string(), 443)
        );
        spec.port = None;
        assert_eq!(
            server_of(&spec).unwrap_err().message,
            "a trojan policy needs a server and a port"
        );
    }

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

    fn no_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    #[test]
    fn no_tls_options_means_no_tls_client() {
        let result = tls_client(None, &HostName::parse("proxy.test"), &[], &[], no_roots());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn client_cert_must_exist_in_the_keystore() {
        let opts = TlsOpts {
            client_cert: Some("missing".to_string()),
            ..TlsOpts::default()
        };
        let err = tls_client(
            Some(&opts),
            &HostName::parse("proxy.test"),
            &[],
            &[],
            no_roots(),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "keystore item `missing` does not exist");
    }

    #[test]
    fn a_broken_client_cert_surfaces_at_build_time() {
        let item = KeystoreItem {
            name: "cert1".into(),
            kind: KeystoreType::P12,
            base64: "AAAA".into(),
            password: Some("x".into()),
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 1),
        };
        let opts = TlsOpts {
            client_cert: Some("cert1".to_string()),
            ..TlsOpts::default()
        };
        let err = tls_client(
            Some(&opts),
            &HostName::parse("proxy.test"),
            &[],
            &[item],
            no_roots(),
        )
        .map(|_| ())
        .unwrap_err();
        assert!(
            err.message
                .starts_with("keystore item `cert1` cannot be decoded"),
            "{}",
            err.message
        );
    }
}
