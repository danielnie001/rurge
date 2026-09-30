//! simple-obfs parameters (`obfs`, `obfs-host`, `obfs-uri`; manual:
//! Policies › Shadowsocks, Policies › Snell): a camouflage layer right below
//! the protocol (phase 2 M6 design 3.1 / 3.2).

use super::reader::ParamReader;
use crate::diagnostic::codes;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObfsMode {
    /// The first packet each way looks like an HTTP upgrade.
    Http,
    /// The first packets look like a TLS handshake, the rest like TLS records.
    Tls,
}

impl ObfsMode {
    pub fn name(self) -> &'static str {
        match self {
            ObfsMode::Http => "http",
            ObfsMode::Tls => "tls",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObfsOpts {
    pub mode: ObfsMode,
    /// `obfs-host`: the `Host` header of `http`, the SNI of `tls`. `None`:
    /// the policy's server host (a rurge default, the manual gives none).
    pub host: Option<String>,
    /// `obfs-uri`: the request path of `http`; `/` unless written.
    pub uri: String,
}

const KEYS: [&str; 2] = ["obfs-host", "obfs-uri"];

/// The longest `obfs-host`: a TLS server name holds at most 255 bytes.
const MAX_HOST: usize = 255;

/// What goes into a request line or a header as is: printable ASCII, no
/// space.
fn printable(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_graphic())
}

/// `None` without `obfs`; `obfs-host` / `obfs-uri` are then `W0028`.
/// `allowed` is what the protocol supports (`http` / `tls` for `ss`); any
/// other value is `E0018`. After an error was reported the returned value is
/// meaningless: the caller checks `r.has_errors()`. Neither the host nor the
/// path is quoted in a diagnostic: the camouflage host can identify the user.
pub fn read_obfs(r: &mut ParamReader<'_>, allowed: &[ObfsMode]) -> Option<ObfsOpts> {
    if !r.has("obfs") {
        for key in KEYS {
            if r.has(key) {
                r.touch(key);
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` has no effect without `obfs`; ignored"),
                );
            }
        }
        return None;
    }
    let table: Vec<(&str, ObfsMode)> = allowed.iter().map(|m| (m.name(), *m)).collect();
    let Some(mode) = r.choice("obfs", &table) else {
        // `choice` reported the value; the other two are not unknown
        for key in KEYS {
            r.touch(key);
        }
        return None;
    };
    let mut host = None;
    if let Some(v) = r.str("obfs-host") {
        let v = v.trim();
        if printable(v) && v.len() <= MAX_HOST {
            host = Some(v.to_string());
        } else if printable(v) {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                format!("invalid `obfs-host` (expected at most {MAX_HOST} bytes)"),
            );
        } else {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "invalid `obfs-host` (expected a host name without space or control character)"
                    .to_string(),
            );
        }
    }
    let mut uri = "/".to_string();
    if let Some(v) = r.str("obfs-uri") {
        let v = v.trim();
        if mode == ObfsMode::Tls {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                "`obfs-uri` has no effect with `obfs=tls`; ignored".to_string(),
            );
        } else if v.starts_with('/') && printable(v) {
            uri = v.to_string();
        } else {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "invalid `obfs-uri` (expected an ASCII path that starts with `/` and holds no space or control character)"
                    .to_string(),
            );
        }
    }
    Some(ObfsOpts { mode, host, uri })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    const BOTH: [ObfsMode; 2] = [ObfsMode::Http, ObfsMode::Tls];

    fn read(def: &str, allowed: &[ObfsMode]) -> (Option<ObfsOpts>, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let opts = read_obfs(&mut r, allowed);
        let failed = r.has_errors();
        (opts, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    #[test]
    fn the_manuals_examples_and_the_defaults() {
        let (opts, failed, diags) = read(
            "ss, h.test, 8388, obfs=http, obfs-host=bing.com, obfs-uri=/a/b?c=1",
            &BOTH,
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            opts,
            Some(ObfsOpts {
                mode: ObfsMode::Http,
                host: Some("bing.com".into()),
                uri: "/a/b?c=1".into(),
            })
        );
        let (opts, failed, diags) = read("ss, h.test, 8388, obfs=TLS", &BOTH);
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            opts,
            Some(ObfsOpts {
                mode: ObfsMode::Tls,
                host: None,
                uri: "/".into(),
            })
        );
        let (opts, _, _) = read("ss, h.test, 8388", &BOTH);
        assert_eq!(opts, None);
    }

    #[test]
    fn the_other_two_without_obfs_are_not_applicable() {
        let (opts, failed, diags) =
            read("ss, h.test, 8388, obfs-host=cdn.test, obfs-uri=/x", &BOTH);
        assert!(opts.is_none() && !failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `obfs-host` has no effect without `obfs`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `obfs-uri` has no effect without `obfs`; ignored"
                ),
            ]
        );
    }

    #[test]
    fn a_path_with_tls_is_ignored() {
        let (opts, failed, diags) = read("ss, h.test, 8388, obfs=tls, obfs-uri=/x", &BOTH);
        assert!(!failed);
        assert_eq!(opts.unwrap().uri, "/");
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `obfs-uri` has no effect with `obfs=tls`; ignored"
            )]
        );
    }

    #[test]
    fn an_unknown_or_unsupported_mode_is_an_error() {
        let (opts, failed, diags) = read(
            "ss, h.test, 8388, obfs=websocket, obfs-host=cdn.test",
            &BOTH,
        );
        assert!(opts.is_none() && failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid value `websocket` for `obfs` (expected http / tls)"
            )]
        );
        // a protocol that supports `http` only
        let (_, failed, diags) = read("snell, h.test, 443, obfs=tls", &[ObfsMode::Http]);
        assert!(failed);
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid value `tls` for `obfs` (expected http)"
        );
    }

    #[test]
    fn a_bad_host_or_path_is_an_error_and_never_quoted() {
        for (def, text) in [
            (
                "ss, h.test, 8388, obfs=http, obfs-uri=s3cret",
                "policy `P`: invalid `obfs-uri` (expected an ASCII path that starts with `/` and holds no space or control character)",
            ),
            (
                "ss, h.test, 8388, obfs=http, obfs-uri=/s3cret path",
                "policy `P`: invalid `obfs-uri` (expected an ASCII path that starts with `/` and holds no space or control character)",
            ),
            (
                "ss, h.test, 8388, obfs=http, obfs-host=s3cret host",
                "policy `P`: invalid `obfs-host` (expected a host name without space or control character)",
            ),
            (
                "ss, h.test, 8388, obfs=tls, obfs-host=\"\"",
                "policy `P`: invalid `obfs-host` (expected a host name without space or control character)",
            ),
        ] {
            let (_, failed, diags) = read(def, &BOTH);
            assert!(failed, "{def}");
            assert_eq!(
                messages(&diags),
                [(codes::E_INVALID_POLICY_PARAM, text)],
                "{def}"
            );
            assert!(!diags[0].message.contains("s3cret"), "{def}");
        }
    }

    #[test]
    fn a_host_longer_than_255_bytes_is_an_error_and_never_quoted() {
        let longest = format!("s3cret{}", "a".repeat(249));
        let (opts, failed, diags) = read(
            &format!("ss, h.test, 8388, obfs=tls, obfs-host={longest}"),
            &BOTH,
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(opts.unwrap().host, Some(longest.clone()));
        let (_, failed, diags) = read(
            &format!("ss, h.test, 8388, obfs=http, obfs-host={longest}a"),
            &BOTH,
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid `obfs-host` (expected at most 255 bytes)"
            )]
        );
        assert!(!diags[0].message.contains("s3cret"));
    }
}
