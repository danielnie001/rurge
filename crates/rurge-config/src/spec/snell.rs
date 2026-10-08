//! `snell` policy parameters (manual: Policies › Snell; phase 2 M6 design
//! 4.2, M8 design 4.3).

use super::obfs::{ObfsMode, ObfsOpts, read_obfs};
use super::reader::ParamReader;
use super::secret::Secret;
use crate::diagnostic::codes;

/// The versions rurge speaks: v1 to v3 share the record format of the
/// original protocol (phase 2 M8 design 4.3), and v5 speaks v4's over TCP
/// (M6-D2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnellVersion {
    V1,
    V2,
    V3,
    V4,
    V5,
}

impl SnellVersion {
    pub fn number(self) -> u8 {
        match self {
            SnellVersion::V1 => 1,
            SnellVersion::V2 => 2,
            SnellVersion::V3 => 3,
            SnellVersion::V4 => 4,
            SnellVersion::V5 => 5,
        }
    }

    /// v1 to v3: the chunked record format of the original protocol.
    pub fn is_chunked(self) -> bool {
        matches!(self, SnellVersion::V1 | SnellVersion::V2 | SnellVersion::V3)
    }

    /// v3 and later relay UDP.
    pub fn carries_udp(self) -> bool {
        !matches!(self, SnellVersion::V1 | SnellVersion::V2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnellSpec {
    pub version: SnellVersion,
    pub psk: Secret<String>,
    /// A finished request hands its connection back for the next one
    /// (ConnectV2). The value in effect: version 2 always does and versions
    /// 1 and 3 never do, whatever `reuse` says; 4 and 5 do as it says.
    pub reuse: bool,
    /// `obfs`: `http` or `tls` on versions 1 to 3, only `http` on 4 and 5.
    pub obfs: Option<ObfsOpts>,
}

/// What a `snell` line says. Version 6 is valid but not supported:
/// `not_implemented_version` holds it and `spec` is meaningless, the caller
/// makes no spec of it (phase 2 M8 design 4.4).
pub struct SnellRead {
    pub spec: SnellSpec,
    pub not_implemented_version: Option<u8>,
    /// `udp-port` is written. It serves v5's QUIC proxy mode, which is not
    /// implemented: UDP keeps to the policy's own port and layers (behind
    /// Shadow TLS, `udp-port` names the bare server), so the caller reports
    /// it as having no effect (`W0029`).
    pub udp_port: bool,
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
        Some(2) => SnellVersion::V2,
        Some(3) => SnellVersion::V3,
        Some(4) => SnellVersion::V4,
        Some(5) => SnellVersion::V5,
        // 1, Surge's default (version 6 and a bad version make no spec)
        _ => SnellVersion::V1,
    };
    match number {
        Some(6) => {
            r.choice("mode", &MODES);
        }
        // the manual: on a lower version it is a profile error
        Some(_) if r.has("mode") => {
            r.touch("mode");
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "`mode` only applies to `snell` version 6".to_string(),
            );
        }
        // a bad version is an error already, and it leaves open which
        // version `mode` is on: no second word
        _ => r.touch("mode"),
    }
    let written = r.bool("reuse");
    // the manual: "Snell v2 always reuses connections, and v1/v3 never do"
    let reuse = match number {
        Some(2) => true,
        Some(1 | 3) => false,
        _ => written.unwrap_or(false),
    };
    if let (Some(n @ 1..=3), Some(written)) = (number, written)
        && written != reuse
    {
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!(
                "`reuse` does not apply to `snell` version {n}: version 2 always reuses a connection, versions 1 and 3 never do"
            ),
        );
    }
    let udp_port = match r.number::<u16>("udp-port", "a port from 1 to 65535") {
        Some(0) => {
            r.invalid("udp-port", "0", "a port from 1 to 65535");
            false
        }
        port => port.is_some(),
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
        // versions 1 to 3 take `tls` too (and a bad version is an error
        // already: no second word on the mode)
        _ => read_obfs(r, &[ObfsMode::Http, ObfsMode::Tls]),
    };
    SnellRead {
        spec: SnellSpec {
            version,
            psk: psk.into(),
            reuse,
            obfs,
        },
        not_implemented_version: number.filter(|n| *n == 6),
        udp_port,
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
        assert!(!got.udp_port);
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
        assert!(got.udp_port);
        let obfs = spec.obfs.expect("obfs");
        assert_eq!(
            (obfs.mode, obfs.host.as_deref(), obfs.uri.as_str()),
            (ObfsMode::Http, Some("cdn.test"), "/x")
        );
    }

    /// Versions 1 to 5 all have a spec, and Surge's default is 1: so is a
    /// line without `version` (phase 2 M8 design 4.3).
    #[test]
    fn versions_1_to_5_have_a_spec_and_the_default_is_1() {
        let (got, failed, diags) = read("snell, h.test, 443, psk=pw");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, None);
        assert_eq!(got.spec.version, SnellVersion::V1);
        for (n, version, chunked, udp) in [
            (1, SnellVersion::V1, true, false),
            (2, SnellVersion::V2, true, false),
            (3, SnellVersion::V3, true, true),
            (4, SnellVersion::V4, false, true),
            (5, SnellVersion::V5, false, true),
        ] {
            let (got, failed, diags) = read(&format!("snell, h.test, 443, psk=pw, version={n}"));
            assert!(!failed && diags.is_empty(), "{n}: {diags:?}");
            assert_eq!(got.not_implemented_version, None, "{n}");
            assert_eq!(got.spec.version, version);
            assert_eq!(
                (
                    version.number(),
                    version.is_chunked(),
                    version.carries_udp()
                ),
                (n, chunked, udp)
            );
        }
    }

    /// Version 6 is a version of the manual's, and none rurge speaks.
    #[test]
    fn version_6_is_valid_but_not_supported() {
        let (got, failed, diags) = read("snell, h.test, 443, psk=pw, version=6");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(got.not_implemented_version, Some(6));
    }

    /// Version 2 always reuses a connection and versions 1 and 3 never do,
    /// whatever the line says: `reuse` written against that is `W0028`. On
    /// versions 4 and 5 it is the line's to choose.
    #[test]
    fn reuse_is_the_versions_own_before_version_4() {
        let (got, ..) = read("snell, h.test, 443, psk=pw");
        assert!(!got.spec.reuse, "the default version is 1");
        for (line, expected) in [
            ("version=1", false),
            ("version=1, reuse=false", false),
            ("version=2", true),
            ("version=2, reuse=true", true),
            ("version=3", false),
            ("version=3, reuse=false", false),
            ("version=4", false),
            ("version=4, reuse=true", true),
            ("version=5, reuse=true", true),
            ("version=5, reuse=false", false),
        ] {
            let (got, failed, diags) = read(&format!("snell, h.test, 443, psk=pw, {line}"));
            assert!(!failed && diags.is_empty(), "{line}: {diags:?}");
            assert_eq!(got.spec.reuse, expected, "{line}");
        }
        for (line, n, expected) in [
            ("version=1, reuse=true", 1, false),
            ("reuse=true", 1, false),
            ("version=2, reuse=false", 2, true),
            ("version=3, reuse=true", 3, false),
        ] {
            let (got, failed, diags) = read(&format!("snell, h.test, 443, psk=pw, {line}"));
            assert!(!failed, "{line}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!(
                        "policy `P`: `reuse` does not apply to `snell` version {n}: version 2 always reuses a connection, versions 1 and 3 never do"
                    )
                    .as_str()
                )],
                "{line}"
            );
            assert_eq!(got.spec.reuse, expected, "{line}");
        }
        // version 6 has a word of its own, and a line that is wrong already
        // gets no second one
        for line in ["version=6, reuse=true", "version=7, reuse=true"] {
            let (_, _, diags) = read(&format!("snell, h.test, 443, psk=pw, {line}"));
            assert!(
                diags
                    .iter()
                    .all(|d| d.code != codes::W_PARAM_NOT_APPLICABLE),
                "{line}: {diags:?}"
            );
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
    }

    /// Versions 1 to 3 take `tls` as well as `http`, `version` written or
    /// not.
    #[test]
    fn versions_1_to_3_take_obfs_tls() {
        for line in ["", ", version=1", ", version=2", ", version=3"] {
            for (mode, expected) in [("tls", ObfsMode::Tls), ("http", ObfsMode::Http)] {
                let (got, failed, diags) = read(&format!(
                    "snell, h.test, 443, psk=pw{line}, obfs={mode}, obfs-host=cdn.test"
                ));
                assert!(!failed && diags.is_empty(), "{line} {mode}: {diags:?}");
                assert_eq!(got.not_implemented_version, None, "{line}");
                let obfs = got.spec.obfs.expect("obfs");
                assert_eq!(
                    (obfs.mode, obfs.host.as_deref()),
                    (expected, Some("cdn.test")),
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn obfs_does_not_apply_to_version_6() {
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
    }

    /// `mode` is version 6's: on a lower version the manual calls it a
    /// profile error, whatever it says and whether `version` is written.
    #[test]
    fn mode_on_another_version_than_6_is_an_error() {
        for line in [
            "version=4, mode=default",
            "version=5, mode=unshaped",
            "version=2, mode=unsafe-raw",
            "mode=default",
            "version=4, mode=fast",
        ] {
            let (_, failed, diags) = read(&format!("snell, h.test, 443, psk=pw, {line}"));
            assert!(failed, "{line}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `mode` only applies to `snell` version 6"
                )],
                "{line}"
            );
        }
        for mode in ["default", "unshaped", "unsafe-raw"] {
            let (got, failed, diags) = read(&format!(
                "snell, h.test, 443, psk=pw, version=6, mode={mode}"
            ));
            assert!(!failed && diags.is_empty(), "{mode}: {diags:?}");
            assert_eq!(got.not_implemented_version, Some(6), "{mode}");
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
        // a `version` that is none is the line's error: which version `mode`
        // is on cannot be told
        let (_, failed, diags) = read("snell, h.test, 443, psk=pw, version=7, mode=default");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid value `7` for `version` (expected an integer from 1 to 6)"
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
