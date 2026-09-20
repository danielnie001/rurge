//! `vmess` policy parameters (manual: Policies › VMess).

use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::{TlsOpts, idle_tls, read_tls};
use super::ws::{WsOpts, read_ws};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

/// `encrypt-method`: how the body is sealed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VmessCipher {
    #[default]
    Aes128Gcm,
    ChaCha20Poly1305,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmessSpec {
    /// `username`: the user id.
    pub uuid: Secret<[u8; 16]>,
    pub cipher: VmessCipher,
    /// `Some` with `tls=true`.
    pub tls: Option<TlsOpts>,
    pub ws: Option<WsOpts>,
}

/// What a `vmess` line says. Without `vmess-aead=true` it asks for the legacy
/// handshake, which is not implemented: the caller makes no spec of it
/// (M2 design 4.3).
pub struct VmessRead {
    pub spec: VmessSpec,
    pub aead: bool,
}

/// The usual 8-4-4-4-12 form, or the same 32 digits without hyphens.
fn parse_uuid(text: &str) -> Option<[u8; 16]> {
    let text = text.trim();
    let digits: Vec<u8> = match text.len() {
        36 => {
            let bytes = text.as_bytes();
            if [8, 13, 18, 23].iter().any(|i| bytes[*i] != b'-') {
                return None;
            }
            bytes.iter().copied().filter(|b| *b != b'-').collect()
        }
        32 => text.bytes().collect(),
        _ => return None,
    };
    if digits.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in digits.chunks(2).enumerate() {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        out[i] = (high * 16 + low) as u8;
    }
    Some(out)
}

/// Everything `vmess`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The id is named-only (`username=`), as the manual writes it, and is never
/// quoted in a diagnostic: it is the credential.
pub fn read_vmess(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> VmessRead {
    let uuid = match r.str("username").map(parse_uuid) {
        Some(Some(uuid)) => uuid,
        found => {
            let text = match found {
                None => "`username` is required",
                Some(_) => "`username` is not a valid UUID",
            };
            r.error(codes::E_INVALID_POLICY_PARAM, text.to_string());
            [0; 16]
        }
    };
    let cipher = r
        .choice(
            "encrypt-method",
            &[
                ("aes-128-gcm", VmessCipher::Aes128Gcm),
                ("chacha20-ietf-poly1305", VmessCipher::ChaCha20Poly1305),
            ],
        )
        .unwrap_or_default();
    let aead = r.bool("vmess-aead").unwrap_or(false);
    let tls = if r.bool("tls").unwrap_or(false) {
        Some(read_tls(r, keystore))
    } else {
        idle_tls(r);
        None
    };
    let ws = read_ws(r);
    VmessRead {
        spec: VmessSpec {
            uuid: Secret::new(uuid),
            cipher,
            tls,
            ws,
        },
        aead,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::Sni;
    use std::path::Path;
    use std::sync::Arc;

    const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
    const BYTES: [u8; 16] = [
        0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1,
        0x19,
    ];

    fn read(def: &str) -> (VmessRead, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let read = read_vmess(&mut r, &[]);
        let failed = r.has_errors();
        (read, failed, r.finish())
    }

    #[test]
    fn the_manuals_example_is_a_legacy_line() {
        let (read, failed, diags) = read(&format!("vmess, 1.2.3.4, 8000, username={ID}"));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!read.aead, "`vmess-aead` defaults to false");
        assert_eq!(read.spec.uuid.expose(), &BYTES);
        assert_eq!(read.spec.cipher, VmessCipher::Aes128Gcm);
        assert!(read.spec.tls.is_none() && read.spec.ws.is_none());
    }

    #[test]
    fn every_parameter_of_the_manual() {
        let (read, failed, diags) = read(&format!(
            "vmess, h.test, 443, username={}, vmess-aead=true, encrypt-method=chacha20-ietf-poly1305, tls=true, sni=edge.test, ws=true, ws-path=/v2, ws-headers=Host:example.com|X-Token:abc",
            ID.replace('-', "").to_uppercase()
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(read.aead);
        assert_eq!(
            read.spec.uuid.expose(),
            &BYTES,
            "hyphens and case do not matter"
        );
        assert_eq!(read.spec.cipher, VmessCipher::ChaCha20Poly1305);
        assert_eq!(read.spec.tls.unwrap().sni, Sni::Name("edge.test".into()));
        let ws = read.spec.ws.unwrap();
        assert_eq!(ws.path, "/v2");
        assert_eq!(ws.headers.len(), 2);
    }

    #[test]
    fn the_id_is_required_named_and_never_quoted() {
        let (_, failed, diags) = read("vmess, h.test, 443, vmess-aead=true");
        assert!(failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `username` is required"
            )
        );
        for bad in [
            "not-a-uuid",
            "0233d11c-15a4-47d3-ade3-48ffca0ce11", // one digit short
            "0233d11c15a4-47d3-ade3-48ffca0ce119-", // hyphens in the wrong places
            "0233d11c-15a4-47d3-ade3-48ffca0ce11g", // not hex
        ] {
            let (_, failed, diags) = read(&format!("vmess, h.test, 443, username={bad}"));
            assert!(failed, "{bad}");
            assert_eq!(
                diags[0].message,
                "policy `P`: `username` is not a valid UUID"
            );
            assert!(diags.iter().all(|d| !d.message.contains(bad)), "{diags:?}");
        }
        // a positional value is not read as the id
        let (_, failed, diags) = read(&format!("vmess, h.test, 443, {ID}"));
        assert!(failed);
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: `username` is required",
                "policy `P`: unexpected positional value #1 ignored"
            ]
        );
        assert!(messages.iter().all(|m| !m.contains("0233")));
    }

    #[test]
    fn an_unknown_cipher_is_an_error() {
        let (_, failed, diags) = read(&format!(
            "vmess, h.test, 443, username={ID}, encrypt-method=rc4"
        ));
        assert!(failed);
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid value `rc4` for `encrypt-method` (expected aes-128-gcm / chacha20-ietf-poly1305)"
        );
    }

    #[test]
    fn tls_parameters_without_tls_are_idle() {
        let (read, failed, diags) = read(&format!(
            "vmess, h.test, 443, username={ID}, vmess-aead=true, sni=edge.test, skip-cert-verify=true"
        ));
        assert!(!failed);
        assert!(read.spec.tls.is_none());
        let found: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `skip-cert-verify` has no effect without `tls=true`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `sni` has no effect without `tls=true`; ignored"
                ),
            ]
        );
    }

    #[test]
    fn the_spec_does_not_print_the_id() {
        let (read, _, _) = read(&format!("vmess, h.test, 443, username={ID}"));
        let printed = format!("{:?}", read.spec);
        assert!(printed.contains("Secret(***)"), "{printed}");
        // 0xd1, the third byte, as `Debug` would print it
        assert!(!printed.contains("209"), "{printed}");
    }
}
