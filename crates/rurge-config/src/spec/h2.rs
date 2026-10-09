//! `h2-connect` and `trust-tunnel` policy parameters (manual: Policies ›
//! HTTP and HTTP/2, Policies › Trust Tunnel; phase 2 M6 design 5.1). Both
//! carry each connection as a CONNECT stream over a TLS + HTTP/2
//! connection; `trust-tunnel` with `h3=true` over HTTP/3 (QUIC) instead
//! (phase 2 M7 design 7.2).

use super::common::CommonOpts;
use super::http::HeaderTemplate;
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::{TlsOpts, read_tls};
use super::{read_credentials, read_headers, required};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct H2ConnectSpec {
    /// HTTP/2 CONNECT always runs over TLS; `alpn` is always `h2`.
    pub tls: TlsOpts,
    pub username: Option<Secret<String>>,
    pub password: Option<Secret<String>>,
    pub headers: Vec<HeaderTemplate>,
    /// `max-streams`: CONNECT streams one HTTP/2 connection carries at once.
    pub max_streams: u32,
    /// `udp-relay`: UDP as CONNECT-UDP (RFC 9298) over extended CONNECT.
    pub udp_relay: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustTunnelSpec {
    /// Trust Tunnel always runs over TLS; `alpn` is always the transport's:
    /// `h2`, or `h3` with `h3=true`.
    pub tls: TlsOpts,
    pub username: Secret<String>,
    pub password: Secret<String>,
    pub headers: Vec<HeaderTemplate>,
    /// `max-streams`: CONNECT streams one connection carries at once.
    pub max_streams: u32,
    /// `h3=true`: the tunnel runs over HTTP/3 (QUIC) instead of HTTP/2 over
    /// TLS (manual).
    pub h3: bool,
}

/// `max-streams` when the line has none (manual).
pub const DEFAULT_MAX_STREAMS: u32 = 3;

/// Connection-specific header fields HTTP/2 and HTTP/3 forbid (RFC 9113
/// 8.2.2, RFC 9114 4.2); the `h2` crate refuses a request that carries any
/// of them.
const CONNECTION_SPECIFIC: [&str; 6] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
    "te",
];

/// The TLS options with ALPN pinned to `alpn`, the one protocol the
/// transport speaks: an `alpn` that says anything else is `W0028`, which
/// names the policies — those of the line's type, narrowed by `which` when
/// the type has more than one transport.
fn read_pinned_tls(
    r: &mut ParamReader<'_>,
    keystore: &[KeystoreItem],
    alpn: &str,
    which: &str,
) -> TlsOpts {
    let mut tls = read_tls(r, keystore);
    // an `h3` that is an error leaves open which one a `trust-tunnel` speaks
    if !tls.alpn.is_empty() && tls.alpn != [alpn] && !r.rejected("h3") {
        let kind = r.policy().kind.keyword();
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("`alpn` is always `{alpn}` on `{kind}` policies{which}; ignored"),
        );
    }
    tls.alpn = vec![alpn.to_string()];
    tls
}

/// `headers`, without the fields `http` (HTTP/2 or HTTP/3) forbids
/// (`W0028` each). `host` is one of them here: `:authority` names the
/// CONNECT target, and a `Host` that differs from it makes the request
/// malformed (RFC 9113 8.3.1, RFC 9114 4.3.1).
fn read_connect_headers(r: &mut ParamReader<'_>, http: &str) -> Vec<HeaderTemplate> {
    let mut headers = read_headers(r);
    // an `h3` that is an error leaves open which HTTP the line speaks
    let open = r.rejected("h3");
    headers.retain(|header| {
        let name = header.name.to_ascii_lowercase();
        let why = if CONNECTION_SPECIFIC.contains(&name.as_str()) {
            ""
        } else if name == "host" {
            " (`:authority` names the target)"
        } else {
            return true;
        };
        if !open {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("header `{name}` in `headers` is not allowed in {http}{why}; ignored"),
            );
        }
        false
    });
    headers
}

fn read_max_streams(r: &mut ParamReader<'_>) -> u32 {
    let Some(v) = r.str("max-streams") else {
        return DEFAULT_MAX_STREAMS;
    };
    match v.trim().parse::<u32>() {
        Ok(n) if n >= 1 => n,
        _ => {
            r.invalid("max-streams", v, "an integer, at least 1");
            DEFAULT_MAX_STREAMS
        }
    }
}

/// Everything `h2-connect`-specific on the line. After an error was reported
/// the returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The manual's syntax line has no positional credentials but its text says
/// they "may be given positionally after the port": both forms are read, the
/// named one winning, as on `http` / `https`.
pub fn read_h2_connect(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> H2ConnectSpec {
    let tls = read_pinned_tls(r, keystore, "h2", "");
    let (username, password) = read_credentials(r);
    let headers = read_connect_headers(r, "HTTP/2");
    let max_streams = read_max_streams(r);
    let udp_relay = r.bool("udp-relay").unwrap_or(false);
    H2ConnectSpec {
        tls,
        username,
        password,
        headers,
        max_streams,
        udp_relay,
    }
}

/// Everything `trust-tunnel`-specific on the line, and with `h3=true` `tfo`
/// taken out of `common`. After an error was reported the returned value is
/// meaningless: the caller checks `r.has_errors()`.
///
/// The credentials are named-only and required, as the manual writes them,
/// and never quoted in a diagnostic: a positional value stays unread and is
/// reported as an extra positional value. `h3` picks the transport, and
/// with it the ALPN (TrustTunnel protocol 3.1, 3.2: `h2` over HTTP/2, `h3`
/// over HTTP/3). The manual gives `trust-tunnel` no port hopping, over
/// HTTP/3 either.
pub fn read_trust_tunnel(
    r: &mut ParamReader<'_>,
    common: &mut CommonOpts,
    keystore: &[KeystoreItem],
) -> TrustTunnelSpec {
    let h3 = r.bool("h3").unwrap_or(false);
    let (alpn, which, http) = if h3 {
        ("h3", " with `h3=true`", "HTTP/3")
    } else {
        ("h2", " without `h3=true`", "HTTP/2")
    };
    let tls = read_pinned_tls(r, keystore, alpn, which);
    if h3 {
        // HTTP/3 runs over QUIC: there is no TCP connection to open fast
        if r.has("tfo") && !r.rejected("tfo") {
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                "`tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored"
                    .to_string(),
            );
        }
        common.tfo = false;
    }
    let username = required(r, "username");
    let password = required(r, "password");
    let headers = read_connect_headers(r, http);
    let max_streams = read_max_streams(r);
    for key in ["port-hopping", "port-hopping-interval"] {
        if r.has(key) {
            r.touch(key);
            r.warn(
                codes::W_PARAM_NOT_APPLICABLE,
                format!("`{key}` does not apply to `trust-tunnel` policies; ignored"),
            );
        }
    }
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`udp-relay` does not apply to `trust-tunnel` policies (no UDP); ignored".to_string(),
        );
    }
    TrustTunnelSpec {
        tls,
        username,
        password,
        headers,
        max_streams,
        h3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::common::{Applies, Notes, read_common};
    use crate::spec::{HeaderPart, Sni};
    use std::path::Path;
    use std::sync::Arc;

    fn reader_for<T>(
        def: &str,
        read: impl Fn(&mut ParamReader<'_>) -> T,
    ) -> (T, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let got = read(&mut r);
        let failed = r.has_errors();
        (got, failed, r.finish())
    }

    fn h2(def: &str) -> (H2ConnectSpec, bool, Vec<Diagnostic>) {
        reader_for(def, |r| read_h2_connect(r, &[]))
    }

    fn tt(def: &str) -> (TrustTunnelSpec, bool, Vec<Diagnostic>) {
        let ((spec, _), failed, diags) = tt_and_common(def);
        (spec, failed, diags)
    }

    /// `tt`, and the common parameters as the reader leaves them.
    fn tt_and_common(def: &str) -> ((TrustTunnelSpec, CommonOpts), bool, Vec<Diagnostic>) {
        reader_for(def, |r| {
            let mut common = read_common(r, Applies::Proxy, &mut Notes::default());
            let spec = read_trust_tunnel(r, &mut common, &[]);
            (spec, common)
        })
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn exposed(secret: &Option<Secret<String>>) -> Option<&str> {
        secret.as_ref().map(|s| s.expose().as_str())
    }

    #[test]
    fn the_manuals_h2_connect_example_and_the_defaults() {
        let (spec, failed, diags) = h2("h2-connect, 1.2.3.4, 443, max-streams=5");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.max_streams, 5);
        assert!(!spec.udp_relay);
        assert_eq!(
            (exposed(&spec.username), exposed(&spec.password)),
            (None, None)
        );
        assert!(spec.headers.is_empty());
        assert_eq!(
            spec.tls,
            TlsOpts {
                alpn: vec!["h2".into()],
                ..TlsOpts::default()
            }
        );
        let (spec, failed, diags) = h2("h2-connect, example.com, 443");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.max_streams, DEFAULT_MAX_STREAMS);
    }

    /// Named credentials as the manual's example writes them, positional
    /// ones as its text allows; named win (as on `http`).
    #[test]
    fn h2_connect_credentials_named_or_positional() {
        let (spec, failed, diags) =
            h2("h2-connect, example.com, 443, username=user, password=pass");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            (exposed(&spec.username), exposed(&spec.password)),
            (Some("user"), Some("pass"))
        );
        let (spec, failed, diags) = h2("h2-connect, example.com, 443, posuser, pospass");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            (exposed(&spec.username), exposed(&spec.password)),
            (Some("posuser"), Some("pospass"))
        );
        let (spec, _, diags) = h2("h2-connect, example.com, 443, posuser, pospass, password=named");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            (exposed(&spec.username), exposed(&spec.password)),
            (Some("posuser"), Some("named"))
        );
    }

    #[test]
    fn h2_connect_with_every_parameter() {
        let (spec, failed, diags) = h2(
            "h2-connect, example.com, 443, headers=X-Padding:<random-string(16-32)>;X-Client:rurge, max-streams=\"8\", udp-relay=true, sni=edge.test, skip-cert-verify=true, server-cert-fingerprint-sha256=0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(!failed, "{diags:?}");
        assert_eq!(
            messages(&diags),
            [(
                codes::W_INVALID_VALUE,
                "policy `P`: `skip-cert-verify` is ignored because `server-cert-fingerprint-sha256` is set"
            )]
        );
        assert_eq!(spec.max_streams, 8);
        assert!(spec.udp_relay);
        assert_eq!(spec.headers.len(), 2);
        assert_eq!(spec.headers[0].name, "X-Padding");
        assert_eq!(
            spec.headers[0].value,
            [HeaderPart::Random { min: 16, max: 32 }]
        );
        assert_eq!(spec.tls.sni, Sni::Name("edge.test".into()));
        assert!(spec.tls.skip_cert_verify && spec.tls.fingerprint_sha256.is_some());
    }

    #[test]
    fn max_streams_is_an_integer_of_at_least_one() {
        for bad in ["0", "-1", "two", "1.5", "4294967296", ""] {
            let (_, failed, diags) = h2(&format!("h2-connect, h.test, 443, max-streams={bad}"));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `max-streams` (expected an integer, at least 1)"
                    )
                    .as_str()
                )]
            );
            let (spec, failed, _) = tt(&format!(
                "trust-tunnel, h.test, 443, username=u, password=p, max-streams={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(spec.max_streams, DEFAULT_MAX_STREAMS);
        }
        let (spec, failed, _) = h2("h2-connect, h.test, 443, max-streams=1");
        assert!(!failed);
        assert_eq!(spec.max_streams, 1);
    }

    #[test]
    fn the_manuals_trust_tunnel_example() {
        let (spec, failed, diags) =
            tt("trust-tunnel, 192.168.20.62, 443, username=test, password=test");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!spec.h3);
        assert_eq!(
            (
                spec.username.expose().as_str(),
                spec.password.expose().as_str()
            ),
            ("test", "test")
        );
        assert_eq!(spec.max_streams, DEFAULT_MAX_STREAMS);
        assert!(spec.headers.is_empty());
        assert_eq!(spec.tls.alpn, ["h2"]);
        let (spec, failed, diags) = tt(
            "trust-tunnel, tt.test, 443, username=u, password=p, headers=X-Padding:<random-string(16-32)>, max-streams=5, sni=tt.test, client-cert=nope",
        );
        assert!(failed, "an unknown keystore item is an error");
        assert_eq!(diags[0].code, codes::E_KEYSTORE_REF);
        assert_eq!(spec.max_streams, 5);
        assert_eq!(spec.headers.len(), 1);
    }

    #[test]
    fn trust_tunnel_credentials_are_required_named_and_never_quoted() {
        let (_, failed, diags) = tt("trust-tunnel, h.test, 443");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `username` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
            ]
        );
        let (_, failed, diags) = tt("trust-tunnel, h.test, 443, s3cretUser, hunter2, password=");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `username` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #2 ignored"
                ),
            ]
        );
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("s3cretUser") && !d.message.contains("hunter2"))
        );
    }

    /// `h3=true` is read: the policy runs over HTTP/3 (phase 2 M7 design
    /// 7.2); `udp-relay` has no place here.
    #[test]
    fn trust_tunnel_h3_and_udp_relay() {
        let (got, failed, diags) = tt("trust-tunnel, h.test, 443, username=u, password=p, h3=true");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(got.h3);
        let (got, failed, diags) =
            tt("trust-tunnel, h.test, 443, username=u, password=p, h3=false");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!got.h3);
        let (_, failed, _) = tt("trust-tunnel, h.test, 443, username=u, password=p, h3=maybe");
        assert!(failed);
        let (_, failed, diags) =
            tt("trust-tunnel, h.test, 443, username=u, password=p, udp-relay=true");
        assert!(!failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `udp-relay` does not apply to `trust-tunnel` policies (no UDP); ignored"
            )]
        );
    }

    /// ALPN is `h2` whatever the line says: `alpn=h2` is welcome, anything
    /// else is said and ignored.
    #[test]
    fn alpn_is_always_h2() {
        let (spec, failed, diags) = h2("h2-connect, h.test, 443, alpn=h2");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.tls.alpn, ["h2"]);
        let (spec, failed, diags) = h2("h2-connect, h.test, 443, alpn=http/1.1,h2");
        assert!(!failed);
        assert_eq!(spec.tls.alpn, ["h2"]);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `alpn` is always `h2` on `h2-connect` policies; ignored"
            )]
        );
        let (spec, _, diags) = tt("trust-tunnel, h.test, 443, username=u, password=p, alpn=h3");
        assert_eq!(spec.tls.alpn, ["h2"]);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `alpn` is always `h2` on `trust-tunnel` policies without `h3=true`; ignored"
            )]
        );
    }

    /// `:authority` names the CONNECT target: a `Host` that differs from it
    /// makes the request malformed (RFC 9113 8.3.1).
    #[test]
    fn a_host_header_is_dropped() {
        let (spec, failed, diags) = h2("h2-connect, h.test, 443, headers=Host:cdn.test;X-A:1");
        let (read, tt_failed, tt_diags) =
            tt("trust-tunnel, h.test, 443, username=u, password=p, headers=host:cdn.test;X-A:1");
        assert!(!failed && !tt_failed);
        for (headers, diags) in [(&spec.headers, &diags), (&read.headers, &tt_diags)] {
            let names: Vec<&str> = headers.iter().map(|h| h.name.as_str()).collect();
            assert_eq!(names, ["X-A"]);
            assert_eq!(
                messages(diags),
                [(
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `host` in `headers` is not allowed in HTTP/2 (`:authority` names the target); ignored"
                )]
            );
        }
    }

    #[test]
    fn connection_specific_headers_are_dropped() {
        let (spec, failed, diags) = h2(
            "h2-connect, h.test, 443, headers=Connection:close;X-A:1;Transfer-Encoding:chunked;TE:trailers",
        );
        assert!(!failed);
        assert_eq!(
            spec.headers
                .iter()
                .map(|h| h.name.as_str())
                .collect::<Vec<_>>(),
            ["X-A"]
        );
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `connection` in `headers` is not allowed in HTTP/2; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `transfer-encoding` in `headers` is not allowed in HTTP/2; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `te` in `headers` is not allowed in HTTP/2; ignored"
                ),
            ]
        );
        // a malformed list is an error, its entry named by position
        let (_, failed, diags) = tt(
            "trust-tunnel, h.test, 443, username=u, password=p, headers=X-A:1;Authorization Bearer s3cret",
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid `headers`: header #2 has no `:`"
            )]
        );
    }

    #[test]
    fn the_specs_do_not_print_credentials() {
        let (spec, _, _) = h2("h2-connect, h.test, 443, username=s3cretUser, password=hunter2");
        let (got, _, _) = tt("trust-tunnel, h.test, 443, username=s3cretUser, password=hunter2");
        for printed in [format!("{spec:?}"), format!("{got:?}")] {
            assert!(printed.contains("Secret(***)"), "{printed}");
            assert!(
                !printed.contains("s3cretUser") && !printed.contains("hunter2"),
                "{printed}"
            );
        }
    }

    /// A header may carry a token: its value is never printed (phase 2 M6
    /// design 6); its name may be.
    #[test]
    fn the_specs_do_not_print_header_values() {
        let (spec, _, _) = h2("h2-connect, h.test, 443, headers=Authorization:Bearer t0ken");
        let (got, _, _) = tt(
            "trust-tunnel, h.test, 443, username=u, password=p, headers=Authorization:Bearer t0ken",
        );
        for printed in [format!("{spec:?}"), format!("{got:?}")] {
            assert!(printed.contains("Authorization"), "{printed}");
            assert!(!printed.contains("t0ken"), "{printed}");
        }
    }

    /// Over HTTP/3 the ALPN is `h3` whatever the line says (TrustTunnel
    /// protocol 3.2): `alpn=h3` is welcome, anything else is said and
    /// ignored.
    #[test]
    fn with_h3_alpn_is_always_h3() {
        for def in [
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true, alpn=h3",
        ] {
            let (spec, failed, diags) = tt(def);
            assert!(!failed && diags.is_empty(), "{def}: {diags:?}");
            assert!(spec.h3, "{def}");
            assert_eq!(spec.tls.alpn, ["h3"], "{def}");
        }
        let (spec, failed, diags) =
            tt("trust-tunnel, h.test, 443, username=u, password=p, h3=true, alpn=h2");
        assert!(!failed);
        assert_eq!(spec.tls.alpn, ["h3"]);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `alpn` is always `h3` on `trust-tunnel` policies with `h3=true`; ignored"
            )]
        );
    }

    /// HTTP/3 runs over QUIC: TCP Fast Open has nothing to act on there.
    /// Over HTTP/2 the reader leaves `tfo` alone (the loader says it has no
    /// effect yet).
    #[test]
    fn with_h3_tfo_does_not_apply() {
        let ((_, common), failed, diags) =
            tt_and_common("trust-tunnel, h.test, 443, username=u, password=p, h3=true, tfo=true");
        assert!(!failed);
        assert!(!common.tfo);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `tfo` does not apply to `trust-tunnel` policies with `h3=true`; ignored"
            )]
        );
        let ((_, common), failed, diags) =
            tt_and_common("trust-tunnel, h.test, 443, username=u, password=p, tfo=true");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(common.tfo);
    }

    /// The manual gives `trust-tunnel` no port hopping, over HTTP/3 either.
    #[test]
    fn port_hopping_does_not_apply() {
        for h3 in ["false", "true"] {
            let (_, failed, diags) = tt(&format!(
                "trust-tunnel, h.test, 443, username=u, password=p, h3={h3}, port-hopping=5000-6000, port-hopping-interval=10"
            ));
            assert!(!failed, "h3={h3}");
            assert_eq!(
                messages(&diags),
                [
                    (
                        codes::W_PARAM_NOT_APPLICABLE,
                        "policy `P`: `port-hopping` does not apply to `trust-tunnel` policies; ignored"
                    ),
                    (
                        codes::W_PARAM_NOT_APPLICABLE,
                        "policy `P`: `port-hopping-interval` does not apply to `trust-tunnel` policies; ignored"
                    ),
                ],
                "h3={h3}"
            );
        }
    }

    /// HTTP/3 forbids what HTTP/2 does (RFC 9114 4.2, 4.3.1); the warning
    /// names the version the tunnel runs over.
    #[test]
    fn with_h3_the_headers_http3_forbids_are_dropped() {
        let (spec, failed, diags) = tt(
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true, headers=Connection:close;Host:cdn.test;X-A:1",
        );
        assert!(!failed);
        let names: Vec<&str> = spec.headers.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["X-A"]);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `connection` in `headers` is not allowed in HTTP/3; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `host` in `headers` is not allowed in HTTP/3 (`:authority` names the target); ignored"
                ),
            ]
        );
    }
}
