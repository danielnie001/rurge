//! `ss` policy parameters (manual: Policies › Shadowsocks; phase 2 M6
//! design 3.1).

use super::obfs::{ObfsMode, ObfsOpts, read_obfs};
use super::reader::ParamReader;
use super::secret::Secret;
use crate::diagnostic::codes;
use base64::Engine as _;
use base64::alphabet::STANDARD;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

/// `encrypt-method`: the ciphers this version implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsMethod {
    /// No encryption: the address, then the payload as is.
    None,
    Aes128Gcm,
    Aes192Gcm,
    Aes256Gcm,
    ChaCha20IetfPoly1305,
    XChaCha20IetfPoly1305,
    /// SS 2022 (SIP022).
    Blake3Aes128Gcm,
    Blake3Aes256Gcm,
}

const METHODS: [(&str, SsMethod); 8] = [
    ("aes-128-gcm", SsMethod::Aes128Gcm),
    ("aes-192-gcm", SsMethod::Aes192Gcm),
    ("aes-256-gcm", SsMethod::Aes256Gcm),
    ("chacha20-ietf-poly1305", SsMethod::ChaCha20IetfPoly1305),
    ("xchacha20-ietf-poly1305", SsMethod::XChaCha20IetfPoly1305),
    ("2022-blake3-aes-128-gcm", SsMethod::Blake3Aes128Gcm),
    ("2022-blake3-aes-256-gcm", SsMethod::Blake3Aes256Gcm),
    ("none", SsMethod::None),
];

/// The stream ciphers of the original protocol: valid, but not implemented
/// before M8 (`W0007`, and the policy behaves as REJECT).
const STREAM_CIPHERS: [&str; 20] = [
    "rc4",
    "rc4-md5",
    "aes-128-cfb",
    "aes-192-cfb",
    "aes-256-cfb",
    "aes-128-ctr",
    "aes-192-ctr",
    "aes-256-ctr",
    "bf-cfb",
    "camellia-128-cfb",
    "camellia-192-cfb",
    "camellia-256-cfb",
    "cast5-cfb",
    "des-cfb",
    "idea-cfb",
    "rc2-cfb",
    "seed-cfb",
    "salsa20",
    "chacha20",
    "chacha20-ietf",
];

/// SS 2022 keys: the standard alphabet, padding optional (as
/// shadowsocks-rust reads them).
const KEY_BASE64: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

impl SsMethod {
    pub fn name(self) -> &'static str {
        METHODS
            .iter()
            .find(|(_, m)| *m == self)
            .map(|(name, _)| *name)
            .expect("every method is in the table")
    }

    /// The key length in bytes, which is also the salt length; 0 for `none`.
    pub fn key_len(self) -> usize {
        match self {
            SsMethod::None => 0,
            SsMethod::Aes128Gcm | SsMethod::Blake3Aes128Gcm => 16,
            SsMethod::Aes192Gcm => 24,
            SsMethod::Aes256Gcm
            | SsMethod::ChaCha20IetfPoly1305
            | SsMethod::XChaCha20IetfPoly1305
            | SsMethod::Blake3Aes256Gcm => 32,
        }
    }

    /// SS 2022: the password is Base64 keys, not a password.
    pub fn is_2022(self) -> bool {
        matches!(self, SsMethod::Blake3Aes128Gcm | SsMethod::Blake3Aes256Gcm)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SsSpec {
    pub method: SsMethod,
    /// As written; empty for `none` without one. The AEAD methods derive
    /// their key from it.
    pub password: Secret<String>,
    /// SS 2022: what `password` decodes to, each key of the method's length.
    /// The identity keys come first, outermost first (SIP023), and the user
    /// key last; a single-user server has the user key only. Empty for the
    /// other methods.
    pub keys: Secret<Vec<Vec<u8>>>,
    pub udp_relay: bool,
    /// `udp-port`: where UDP goes; `None`: the policy's port.
    pub udp_port: Option<u16>,
    pub obfs: Option<ObfsOpts>,
}

/// What an `ss` line says. With a stream cipher `stream_cipher` names it and
/// `spec` is meaningless: the caller makes no spec of it (phase 2 M6 design
/// 3.1).
pub struct SsRead {
    pub spec: SsSpec,
    pub stream_cipher: Option<&'static str>,
}

/// The keys of an SS 2022 password, or the 1-based position of the first
/// one that is not Base64 of `len` bytes.
fn decode_keys(password: &str, len: usize) -> Result<Vec<Vec<u8>>, usize> {
    password
        .split(':')
        .enumerate()
        .map(|(i, part)| match KEY_BASE64.decode(part.trim()) {
            Ok(key) if key.len() == len => Ok(key),
            _ => Err(i + 1),
        })
        .collect()
}

/// Everything `ss`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The password is named-only (`password=`), as the manual writes it, and is
/// never quoted in a diagnostic; the method may be, it is no secret.
pub fn read_ss(r: &mut ParamReader<'_>) -> SsRead {
    let mut method = SsMethod::None;
    let mut stream_cipher = None;
    match r.str("encrypt-method").map(str::trim) {
        None => r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`encrypt-method` is required".to_string(),
        ),
        Some(v) => {
            if let Some((_, m)) = METHODS.iter().find(|(n, _)| n.eq_ignore_ascii_case(v)) {
                method = *m;
            } else if let Some(name) = STREAM_CIPHERS.iter().find(|n| n.eq_ignore_ascii_case(v)) {
                stream_cipher = Some(*name);
            } else {
                let names: Vec<&str> = METHODS.iter().map(|(name, _)| *name).collect();
                r.invalid("encrypt-method", v, &names.join(" / "));
            }
        }
    }
    let password = r.str("password").unwrap_or_default();
    let needs_password = method != SsMethod::None || stream_cipher.is_some();
    let mut keys = Vec::new();
    if password.is_empty() {
        if needs_password {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`password` is required".to_string(),
            );
        }
    } else if method.is_2022() {
        let len = method.key_len();
        match decode_keys(password, len) {
            Ok(decoded) => keys = decoded,
            Err(position) => r.error(
                codes::E_INVALID_POLICY_PARAM,
                format!(
                    "key #{position} of `password` is not a Base64 key of {len} bytes, as `{}` requires",
                    method.name()
                ),
            ),
        }
    }
    let udp_relay = r.bool("udp-relay").unwrap_or(false);
    let udp_port = match r.number::<u16>("udp-port", "a port from 1 to 65535") {
        Some(0) => {
            r.invalid("udp-port", "0", "a port from 1 to 65535");
            None
        }
        port => port,
    };
    let obfs = read_obfs(r, &[ObfsMode::Http, ObfsMode::Tls]);
    SsRead {
        spec: SsSpec {
            method,
            password: password.into(),
            keys: Secret::new(keys),
            udp_relay,
            udp_port,
            obfs,
        },
        stream_cipher,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    /// 16 and 32 bytes of Base64 (`openssl rand -base64 16` / `32`).
    const KEY16: &str = "tn6UbJ3OzpVCTU1RlQzm2g==";
    const KEY32: &str = "YctPZ6U7xPPcU+gp3u+0tx/tRizJN9K8y+uKlW2qjlI=";

    fn read(def: &str) -> (SsRead, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let got = read_ss(&mut r);
        let failed = r.has_errors();
        (got, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<&str> {
        diags.iter().map(|d| d.message.as_str()).collect()
    }

    #[test]
    fn the_manuals_example() {
        let (got, failed, diags) = read(
            "ss, 1.2.3.4, 8000, encrypt-method=chacha20-ietf-poly1305, password=abcd1234, obfs=http, obfs-host=bing.com, obfs-uri=/resource/file, udp-relay=true",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(got.stream_cipher.is_none());
        let spec = got.spec;
        assert_eq!(spec.method, SsMethod::ChaCha20IetfPoly1305);
        assert_eq!(spec.password.expose(), "abcd1234");
        assert!(spec.keys.expose().is_empty());
        assert!(spec.udp_relay);
        assert_eq!(spec.udp_port, None);
        let obfs = spec.obfs.expect("obfs");
        assert_eq!(
            (obfs.mode, obfs.host.as_deref(), obfs.uri.as_str()),
            (ObfsMode::Http, Some("bing.com"), "/resource/file")
        );
    }

    #[test]
    fn every_method_by_name_with_its_key_length() {
        for (name, method, len) in [
            ("aes-128-gcm", SsMethod::Aes128Gcm, 16),
            ("AES-192-GCM", SsMethod::Aes192Gcm, 24),
            ("aes-256-gcm", SsMethod::Aes256Gcm, 32),
            ("chacha20-ietf-poly1305", SsMethod::ChaCha20IetfPoly1305, 32),
            (
                "xchacha20-ietf-poly1305",
                SsMethod::XChaCha20IetfPoly1305,
                32,
            ),
            ("2022-blake3-aes-128-gcm", SsMethod::Blake3Aes128Gcm, 16),
            ("2022-blake3-aes-256-gcm", SsMethod::Blake3Aes256Gcm, 32),
            ("none", SsMethod::None, 0),
        ] {
            let password = match len {
                16 if method.is_2022() => KEY16,
                32 if method.is_2022() => KEY32,
                _ => "pw",
            };
            let (got, failed, diags) = read(&format!(
                "ss, h.test, 8388, encrypt-method={name}, password={password}"
            ));
            assert!(!failed && diags.is_empty(), "{name}: {diags:?}");
            assert_eq!(got.spec.method, method, "{name}");
            assert_eq!(method.key_len(), len, "{name}");
            assert_eq!(method.name(), name.to_ascii_lowercase());
        }
        // `none` needs no password
        let (got, failed, diags) = read("ss, h.test, 8388, encrypt-method=none");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.spec.password.expose(), "");
    }

    #[test]
    fn ss_2022_keys_are_decoded_last_one_the_user_key() {
        let (got, failed, diags) = read(&format!(
            "ss, h.test, 8388, encrypt-method=2022-blake3-aes-256-gcm, password={KEY32}"
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        let keys = got.spec.keys.expose();
        assert_eq!(keys.len(), 1);
        assert_eq!(&keys[0][..4], &[0x61, 0xcb, 0x4f, 0x67]);
        // an identity key, then the user key; padding is optional
        let bare = KEY16.trim_end_matches('=');
        let other = "AAECAwQFBgcICQoLDA0ODw";
        let (got, failed, diags) = read(&format!(
            "ss, h.test, 8388, encrypt-method=2022-blake3-aes-128-gcm, password={bare}:{other}"
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        let keys = got.spec.keys.expose();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0][..2], [0xb6, 0x7e], "the identity key");
        assert_eq!(keys[1], (0u8..16).collect::<Vec<u8>>(), "the user key");
        assert!(keys.iter().all(|k| k.len() == 16));
    }

    #[test]
    fn a_bad_ss_2022_key_is_an_error_that_never_quotes_the_password() {
        for (method, password, text) in [
            (
                "2022-blake3-aes-128-gcm",
                KEY32.to_string(),
                "policy `P`: key #1 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
            ),
            (
                "2022-blake3-aes-256-gcm",
                KEY16.to_string(),
                "policy `P`: key #1 of `password` is not a Base64 key of 32 bytes, as `2022-blake3-aes-256-gcm` requires",
            ),
            (
                "2022-blake3-aes-128-gcm",
                "hunter2".to_string(),
                "policy `P`: key #1 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
            ),
            (
                "2022-blake3-aes-256-gcm",
                format!("{KEY16}:{KEY32}"),
                "policy `P`: key #1 of `password` is not a Base64 key of 32 bytes, as `2022-blake3-aes-256-gcm` requires",
            ),
            (
                "2022-blake3-aes-128-gcm",
                format!("{KEY16}:"),
                "policy `P`: key #2 of `password` is not a Base64 key of 16 bytes, as `2022-blake3-aes-128-gcm` requires",
            ),
        ] {
            let (_, failed, diags) = read(&format!(
                "ss, h.test, 8388, encrypt-method={method}, password={password}"
            ));
            assert!(failed, "{method} {password}");
            assert_eq!(messages(&diags), [text]);
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
            for part in password.split(':').filter(|p| !p.is_empty()) {
                assert!(!diags[0].message.contains(part), "{}", diags[0].message);
            }
        }
    }

    #[test]
    fn the_method_and_the_password_are_required() {
        let (_, failed, diags) = read("ss, h.test, 8388, password=pw");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            ["policy `P`: `encrypt-method` is required"]
        );
        let (_, failed, diags) = read("ss, h.test, 8388, encrypt-method=aes-128-gcm, hunter2");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                "policy `P`: `password` is required",
                "policy `P`: unexpected positional value #1 ignored"
            ]
        );
        assert!(diags.iter().all(|d| !d.message.contains("hunter2")));
        // a stream cipher needs one too
        let (_, failed, _) = read("ss, h.test, 8388, encrypt-method=rc4-md5");
        assert!(failed);
    }

    #[test]
    fn an_unknown_method_is_an_error_that_quotes_it() {
        let (_, failed, diags) = read("ss, h.test, 8388, encrypt-method=aes-512-gcm, password=pw");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                "policy `P`: invalid value `aes-512-gcm` for `encrypt-method` (expected aes-128-gcm / aes-192-gcm / aes-256-gcm / chacha20-ietf-poly1305 / xchacha20-ietf-poly1305 / 2022-blake3-aes-128-gcm / 2022-blake3-aes-256-gcm / none)"
            ]
        );
    }

    #[test]
    fn a_stream_cipher_is_named_and_not_an_error() {
        for (written, name) in [
            ("rc4-md5", "rc4-md5"),
            ("AES-256-CFB", "aes-256-cfb"),
            ("chacha20-ietf", "chacha20-ietf"),
            ("camellia-128-cfb", "camellia-128-cfb"),
        ] {
            let (got, failed, diags) = read(&format!(
                "ss, h.test, 8388, encrypt-method={written}, password=pw"
            ));
            assert!(!failed && diags.is_empty(), "{written}: {diags:?}");
            assert_eq!(got.stream_cipher, Some(name));
        }
    }

    #[test]
    fn udp_parameters() {
        let (got, failed, diags) =
            read("ss, h.test, 8388, encrypt-method=none, udp-relay=true, udp-port=8389");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(got.spec.udp_relay);
        assert_eq!(got.spec.udp_port, Some(8389));
        for bad in ["0", "65536", "port"] {
            let (_, failed, diags) = read(&format!(
                "ss, h.test, 8388, encrypt-method=none, udp-port={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [format!(
                    "policy `P`: invalid value `{bad}` for `udp-port` (expected a port from 1 to 65535)"
                )
                .as_str()]
            );
        }
        let (_, failed, _) = read("ss, h.test, 8388, encrypt-method=none, udp-relay=maybe");
        assert!(failed);
    }

    #[test]
    fn the_spec_does_not_print_the_password_or_the_keys() {
        let (got, _, _) = read(&format!(
            "ss, h.test, 8388, encrypt-method=2022-blake3-aes-256-gcm, password={KEY32}"
        ));
        let printed = format!("{:?}", got.spec);
        assert!(printed.contains("Secret(***)"), "{printed}");
        assert!(!printed.contains("YctP"), "{printed}");
        // 0x61, the first key byte, as `Debug` would print it
        assert!(!printed.contains("97"), "{printed}");
    }
}
