//! TLS parameters shared by every TLS-carried protocol (matrix §4.4).

use super::common::Notes;
use super::reader::ParamReader;
use crate::diagnostic::codes;
use crate::keystore::{KeystoreItem, KeystoreType};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sni {
    /// Send the proxy's host name (nothing for an IP literal).
    #[default]
    Default,
    /// `sni = off`: no SNI extension at all.
    Off,
    Name(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TlsOpts {
    pub skip_cert_verify: bool,
    pub sni: Sni,
    /// Verify the certificate against this name instead of the SNI name.
    pub verify_name: Option<String>,
    /// SHA-256 of the pinned leaf certificate (DER); replaces chain validation.
    pub fingerprint_sha256: Option<[u8; 32]>,
    pub alpn: Vec<String>,
    /// Name of a `p12` `[Keystore]` item.
    pub client_cert: Option<String>,
}

pub(crate) const TLS_KEYS: [&str; 6] = [
    "skip-cert-verify",
    "sni",
    "server-cert-verify-name",
    "server-cert-fingerprint-sha256",
    "alpn",
    "client-cert",
];

/// Shadow TLS arrives in M2; until then the parameters are known but inert.
pub(crate) const SHADOW_TLS_KEYS: [&str; 3] = [
    "shadow-tls-password",
    "shadow-tls-sni",
    "shadow-tls-version",
];

fn parse_fingerprint(value: &str) -> Option<[u8; 32]> {
    let value = value.trim();
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub(crate) fn read_tls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TlsOpts {
    let skip_cert_verify = r.bool("skip-cert-verify").unwrap_or(false);
    let mut sni = Sni::Default;
    if let Some(v) = r.str("sni") {
        let v = v.trim();
        if v.is_empty() {
            r.invalid("sni", v, "a host name or off");
        } else if v.eq_ignore_ascii_case("off") {
            sni = Sni::Off;
        } else {
            sni = Sni::Name(v.to_string());
        }
    }
    let mut verify_name = None;
    if let Some(v) = r.str("server-cert-verify-name") {
        if v.trim().is_empty() {
            r.invalid("server-cert-verify-name", v, "a host name");
        } else {
            verify_name = Some(v.trim().to_string());
        }
    }
    let mut fingerprint_sha256 = None;
    if let Some(v) = r.str("server-cert-fingerprint-sha256") {
        match parse_fingerprint(v) {
            Some(fp) => fingerprint_sha256 = Some(fp),
            None => r.invalid(
                "server-cert-fingerprint-sha256",
                v,
                "64 hexadecimal characters",
            ),
        }
    }
    let alpn = r
        .str("alpn")
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut client_cert = None;
    if let Some(v) = r.str("client-cert") {
        let name = v.trim();
        match keystore.iter().find(|k| k.name == name) {
            None => r.error(
                codes::E_KEYSTORE_REF,
                format!("`client-cert` references unknown keystore item `{name}`"),
            ),
            Some(item) if item.kind != KeystoreType::P12 => r.error(
                codes::E_KEYSTORE_REF,
                format!("`client-cert` needs a `p12` keystore item, but `{name}` is `openssh-private-key`"),
            ),
            Some(_) => client_cert = Some(name.to_string()),
        }
    }
    if skip_cert_verify && fingerprint_sha256.is_some() {
        r.warn(
            codes::W_INVALID_VALUE,
            "`skip-cert-verify` is ignored because `server-cert-fingerprint-sha256` is set"
                .to_string(),
        );
    }
    TlsOpts {
        skip_cert_verify,
        sni,
        verify_name,
        fingerprint_sha256,
        alpn,
        client_cert,
    }
}

/// For protocols that do not run over TLS: every TLS parameter present is `W0028`.
pub(crate) fn refuse_tls(r: &mut ParamReader<'_>) {
    let kind = r.policy().kind.keyword();
    let mut present: Vec<&str> = TLS_KEYS.iter().copied().filter(|k| r.has(k)).collect();
    present.sort_unstable();
    for key in present {
        r.touch(key);
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("`{key}` does not apply to `{kind}` policies; ignored"),
        );
    }
}

pub(crate) fn note_shadow_tls(r: &mut ParamReader<'_>, notes: &mut Notes) {
    for key in SHADOW_TLS_KEYS {
        if r.has(key) {
            r.touch(key);
            notes.inert.push(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::codes;
    use crate::keystore::{KeystoreItem, KeystoreType};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::ParamReader;
    use std::path::Path;
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("p.conf")), 1)
    }

    fn keystore() -> Vec<KeystoreItem> {
        let item = |name: &str, kind| KeystoreItem {
            name: name.into(),
            kind,
            base64: "AAAA".into(),
            password: None,
            unknown: Vec::new(),
            span: span(),
        };
        vec![
            item("cert1", KeystoreType::P12),
            item("key1", KeystoreType::OpensshPrivateKey),
        ]
    }

    fn read(def: &str) -> (TlsOpts, Vec<crate::Diagnostic>) {
        let p = parse_policy("P", def, &span()).unwrap();
        let mut r = ParamReader::new(&p);
        let tls = read_tls(&mut r, &keystore());
        (tls, r.finish())
    }

    #[test]
    fn all_six_parameters() {
        let fp = "ab".repeat(32);
        let (tls, diags) = read(&format!(
            "https, h, 443, sni=cdn.example.com, server-cert-verify-name=real.example.com, \
             server-cert-fingerprint-sha256={fp}, alpn=\"h2, http/1.1\", client-cert=cert1"
        ));
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tls.sni, Sni::Name("cdn.example.com".into()));
        assert_eq!(tls.verify_name.as_deref(), Some("real.example.com"));
        assert_eq!(tls.fingerprint_sha256, Some([0xab; 32]));
        assert_eq!(tls.alpn, ["h2", "http/1.1"]);
        assert_eq!(tls.client_cert.as_deref(), Some("cert1"));
        assert!(!tls.skip_cert_verify);
    }

    #[test]
    fn sni_off_and_defaults() {
        let (tls, _) = read("https, h, 443, sni=OFF, skip-cert-verify=true");
        assert_eq!(tls.sni, Sni::Off);
        assert!(tls.skip_cert_verify);
        let (tls, _) = read("https, h, 443");
        assert_eq!(tls, TlsOpts::default());
    }

    #[test]
    fn a_fingerprint_wins_over_skip_cert_verify_with_a_warning() {
        let (tls, diags) = read(&format!(
            "https, h, 443, skip-cert-verify=true, server-cert-fingerprint-sha256={}",
            "00".repeat(32)
        ));
        assert!(tls.fingerprint_sha256.is_some());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, codes::W_INVALID_VALUE);
        assert_eq!(
            diags[0].message,
            "policy `P`: `skip-cert-verify` is ignored because `server-cert-fingerprint-sha256` is set"
        );
    }

    #[test]
    fn invalid_values_and_keystore_references() {
        for (def, code) in [
            (
                "https, h, 443, server-cert-fingerprint-sha256=abcd",
                codes::E_INVALID_POLICY_PARAM,
            ),
            ("https, h, 443, sni=", codes::E_INVALID_POLICY_PARAM),
            (
                "https, h, 443, server-cert-verify-name=",
                codes::E_INVALID_POLICY_PARAM,
            ),
            ("https, h, 443, client-cert=nope", codes::E_KEYSTORE_REF),
            ("https, h, 443, client-cert=key1", codes::E_KEYSTORE_REF),
        ] {
            let (_, diags) = read(def);
            assert_eq!(diags.len(), 1, "{def}: {diags:?}");
            assert_eq!(diags[0].code, code, "{def}");
        }
        let (_, diags) = read("https, h, 443, client-cert=key1");
        assert_eq!(
            diags[0].message,
            "policy `P`: `client-cert` needs a `p12` keystore item, but `key1` is `openssh-private-key`"
        );
    }

    #[test]
    fn tls_parameters_on_a_plain_protocol_do_not_apply() {
        let p = parse_policy(
            "P",
            "http, h, 80, sni=x.example, skip-cert-verify=true",
            &span(),
        )
        .unwrap();
        let mut r = ParamReader::new(&p);
        refuse_tls(&mut r);
        let diags = r.finish();
        let codes_seen: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert_eq!(codes_seen, [codes::W_PARAM_NOT_APPLICABLE; 2]);
        assert_eq!(
            diags[0].message,
            "policy `P`: `skip-cert-verify` does not apply to `http` policies; ignored"
        );
    }
}
