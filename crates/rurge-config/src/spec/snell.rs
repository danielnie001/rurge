//! `snell` policy parameters (manual: Policies › Snell; phase 2 M6 design
//! 4.2).

use super::obfs::{ObfsMode, ObfsOpts, read_obfs};
use super::reader::ParamReader;
use super::secret::Secret;
use crate::diagnostic::codes;

/// The versions this version implements: v5 speaks the v4 wire format over
/// TCP (M6-D2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnellVersion {
    V4,
    V5,
}

impl SnellVersion {
    pub fn number(self) -> u8 {
        match self {
            SnellVersion::V4 => 4,
            SnellVersion::V5 => 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnellSpec {
    pub version: SnellVersion,
    pub psk: Secret<String>,
    /// `reuse`: a finished stream hands its connection back for the next
    /// request (ConnectV2).
    pub reuse: bool,
    /// `udp-port`: where UDP goes; `None`: the policy's port.
    pub udp_port: Option<u16>,
    /// `obfs`: only `http` on versions 4 and 5.
    pub obfs: Option<ObfsOpts>,
}

/// What a `snell` line says. With a version that is valid but not
/// implemented (1–3, 6) `not_implemented_version` holds it and `spec` is
/// meaningless: the caller makes no spec of it (M6-D2).
pub struct SnellRead {
    pub spec: SnellSpec,
    pub not_implemented_version: Option<u8>,
}

/// Surge's default when `version` is not written.
const DEFAULT_VERSION: u8 = 1;

/// `mode` on version 6.
const MODES: [(&str, ()); 3] = [("default", ()), ("unshaped", ()), ("unsafe-raw", ())];

const OBFS_KEYS: [&str; 3] = ["obfs", "obfs-host", "obfs-uri"];

/// Everything `snell`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The PSK is named-only (`psk=`), as the manual writes it, and is never
/// quoted in a diagnostic; the version may be, it is no secret.
pub fn read_snell(r: &mut ParamReader<'_>) -> SnellRead {
    let psk = r.str("psk").unwrap_or_default();
    if psk.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`psk` is required".to_string(),
        );
    }
    let number = match r.str("version").map(str::trim) {
        None => Some(DEFAULT_VERSION),
        Some(v) => match v.parse::<u8>() {
            Ok(n @ 1..=6) => Some(n),
            _ => {
                r.invalid("version", v, "an integer from 1 to 6");
                None
            }
        },
    };
    let version = match number {
        Some(5) => SnellVersion::V5,
        _ => SnellVersion::V4,
    };
    if number == Some(6) {
        r.choice("mode", &MODES);
    } else if r.has("mode") {
        r.touch("mode");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`mode` only applies to `snell` version 6; ignored".to_string(),
        );
    }
    let reuse = r.bool("reuse").unwrap_or(false);
    let udp_port = match r.number::<u16>("udp-port", "a port from 1 to 65535") {
        Some(0) => {
            r.invalid("udp-port", "0", "a port from 1 to 65535");
            None
        }
        port => port,
    };
    let obfs = match number {
        Some(6) => {
            for key in OBFS_KEYS {
                if r.has(key) {
                    r.touch(key);
                    r.warn(
                        codes::W_PARAM_NOT_APPLICABLE,
                        format!("`{key}` does not apply to `snell` version 6; ignored"),
                    );
                }
            }
            None
        }
        Some(4 | 5) => read_obfs(r, &[ObfsMode::Http]),
        // versions 1 to 3 had `tls` too (and a bad version is an error
        // already: no second word on the mode)
        _ => read_obfs(r, &[ObfsMode::Http, ObfsMode::Tls]),
    };
    SnellRead {
        spec: SnellSpec {
            version,
            psk: psk.into(),
            reuse,
            udp_port,
            obfs,
        },
        not_implemented_version: number.filter(|n| !matches!(n, 4 | 5)),
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

    fn read(def: &str) -> (SnellRead, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let got = read_snell(&mut r);
        let failed = r.has_errors();
        (got, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    #[test]
    fn the_manuals_example() {
        let (got, failed, diags) = read("snell, 1.2.3.4, 8000, psk=xxx, version=4, obfs=http");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, None);
        let spec = got.spec;
        assert_eq!(spec.version, SnellVersion::V4);
        assert_eq!(spec.psk.expose(), "xxx");
        assert!(!spec.reuse);
        assert_eq!(spec.udp_port, None);
        let obfs = spec.obfs.expect("obfs");
        assert_eq!(
            (obfs.mode, obfs.host, obfs.uri.as_str()),
            (ObfsMode::Http, None, "/")
        );
    }

    #[test]
    fn version_5_with_every_parameter() {
        let (got, failed, diags) = read(
            "snell, h.test, 443, psk=pw, version=5, reuse=true, udp-port=8443, obfs=HTTP, obfs-host=cdn.test, obfs-uri=/x",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, None);
        let spec = got.spec;
        assert_eq!((spec.version, spec.version.number()), (SnellVersion::V5, 5));
        assert!(spec.reuse);
        assert_eq!(spec.udp_port, Some(8443));
        let obfs = spec.obfs.expect("obfs");
        assert_eq!(
            (obfs.mode, obfs.host.as_deref(), obfs.uri.as_str()),
            (ObfsMode::Http, Some("cdn.test"), "/x")
        );
    }

    /// Surge's default is 1, so a line without `version` is valid but not
    /// implemented; so are 2, 3 and 6 (M6-D2).
    #[test]
    fn versions_1_to_3_and_6_are_valid_but_not_implemented() {
        let (got, failed, diags) = read("snell, h.test, 443, psk=pw");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, Some(1));
        for n in [1, 2, 3, 6] {
            let (got, failed, diags) = read(&format!("snell, h.test, 443, psk=pw, version={n}"));
            assert!(!failed && diags.is_empty(), "{n}: {diags:?}");
            assert_eq!(got.not_implemented_version, Some(n));
        }
    }

    #[test]
    fn a_version_out_of_range_is_an_error_that_quotes_it() {
        for bad in ["0", "07", "300", "4.0", "v4", "-1"] {
            let (got, failed, diags) = read(&format!("snell, h.test, 443, psk=pw, version={bad}"));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `version` (expected an integer from 1 to 6)"
                    )
                    .as_str()
                )]
            );
            assert_eq!(got.not_implemented_version, None, "{bad}");
        }
    }

    #[test]
    fn the_psk_is_required_and_never_quoted() {
        let (_, failed, diags) = read("snell, h.test, 443, version=4");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `psk` is required"
            )]
        );
        let (_, failed, diags) = read("snell, h.test, 443, psk=, version=4, hunter2");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `psk` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                )
            ]
        );
        assert!(diags.iter().all(|d| !d.message.contains("hunter2")));
    }

    #[test]
    fn obfs_tls_is_an_error_on_versions_4_and_5() {
        for version in [4, 5] {
            let (_, failed, diags) = read(&format!(
                "snell, h.test, 443, psk=pw, version={version}, obfs=tls, obfs-host=cdn.test"
            ));
            assert!(failed, "{version}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: invalid value `tls` for `obfs` (expected http)"
                )]
            );
        }
        // versions 1 to 3 had it: the line is valid, just not implemented
        let (got, failed, diags) = read("snell, h.test, 443, psk=pw, version=3, obfs=tls");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, Some(3));
    }

    #[test]
    fn obfs_on_version_6_and_mode_elsewhere_are_not_applicable() {
        let (_, failed, diags) = read(
            "snell, h.test, 443, psk=pw, version=6, mode=unshaped, obfs=http, obfs-host=cdn.test",
        );
        assert!(!failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `obfs` does not apply to `snell` version 6; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `obfs-host` does not apply to `snell` version 6; ignored"
                ),
            ]
        );
        for version in ["version=4", "version=5", "version=2"] {
            let (got, failed, diags) = read(&format!(
                "snell, h.test, 443, psk=pw, {version}, mode=default"
            ));
            assert!(!failed, "{version}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `mode` only applies to `snell` version 6; ignored"
                )]
            );
            if version != "version=2" {
                assert_eq!(got.not_implemented_version, None);
            }
        }
        // a mode version 6 does not know is wrong on it
        let (_, failed, diags) = read("snell, h.test, 443, psk=pw, version=6, mode=fast");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid value `fast` for `mode` (expected default / unshaped / unsafe-raw)"
            )]
        );
    }

    #[test]
    fn udp_port_and_reuse() {
        for bad in ["0", "65536", "port"] {
            let (_, failed, diags) = read(&format!(
                "snell, h.test, 443, psk=pw, version=4, udp-port={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `udp-port` (expected a port from 1 to 65535)"
                    )
                    .as_str()
                )]
            );
        }
        let (_, failed, _) = read("snell, h.test, 443, psk=pw, version=4, reuse=maybe");
        assert!(failed);
    }

    #[test]
    fn the_spec_does_not_print_the_psk() {
        let (got, _, _) = read("snell, h.test, 443, psk=s3cretPsk, version=5");
        let printed = format!("{:?}", got.spec);
        assert!(printed.contains("Secret(***)"), "{printed}");
        assert!(!printed.contains("s3cretPsk"), "{printed}");
    }
}
