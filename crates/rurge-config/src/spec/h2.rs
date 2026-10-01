//! `h2-connect` and `trust-tunnel` policy parameters (manual: Policies ›
//! HTTP and HTTP/2, Policies › Trust Tunnel; phase 2 M6 design 5.1). Both
//! carry each connection as a CONNECT stream over a TLS + HTTP/2 connection.

use super::http::HeaderTemplate;
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::{TlsOpts, read_tls};
use super::{read_credentials, read_headers};
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
    /// Trust Tunnel always runs over TLS; `alpn` is always `h2`.
    pub tls: TlsOpts,
    pub username: Secret<String>,
    pub password: Secret<String>,
    pub headers: Vec<HeaderTemplate>,
    /// `max-streams`: CONNECT streams one HTTP/2 connection carries at once.
    pub max_streams: u32,
}

/// What a `trust-tunnel` line says: the spec, and whether it asks for
/// `h3=true`, which is parsed and reported (`W0029`) but connects over
/// HTTP/2 until the QUIC family lands (phase 2 M6 design 5.1).
pub struct TrustTunnelRead {
    pub spec: TrustTunnelSpec,
    pub h3: bool,
}

/// `max-streams` when the line has none (manual).
pub const DEFAULT_MAX_STREAMS: u32 = 3;

/// Connection-specific header fields HTTP/2 forbids (RFC 9113 8.2.2); the
/// `h2` crate refuses a request that carries any of them.
const CONNECTION_SPECIFIC: [&str; 6] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
    "te",
];

/// The TLS options with ALPN pinned to `h2`: an `alpn` that says anything
/// else is `W0028`.
fn read_h2_tls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TlsOpts {
    let mut tls = read_tls(r, keystore);
    if !tls.alpn.is_empty() && tls.alpn != ["h2"] {
        let kind = r.policy().kind.keyword();
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("`alpn` is always `h2` on `{kind}` policies; ignored"),
        );
    }
    tls.alpn = vec!["h2".to_string()];
    tls
}

/// `headers`, without the fields HTTP/2 forbids (`W0028` each).
fn read_h2_headers(r: &mut ParamReader<'_>) -> Vec<HeaderTemplate> {
    let mut headers = read_headers(r);
    headers.retain(|header| {
        let name = header.name.to_ascii_lowercase();
        if !CONNECTION_SPECIFIC.contains(&name.as_str()) {
            return true;
        }
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("header `{name}` in `headers` is not allowed in HTTP/2; ignored"),
        );
        false
    });
    headers
}

/// A named credential that must be there; its value is never quoted.
fn required(r: &mut ParamReader<'_>, key: &str) -> Secret<String> {
    let value = r.str(key).unwrap_or_default();
    if value.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            format!("`{key}` is required"),
        );
    }
    Secret::from(value)
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
    let tls = read_h2_tls(r, keystore);
    let (username, password) = read_credentials(r);
    let headers = read_h2_headers(r);
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

/// Everything `trust-tunnel`-specific on the line. After an error was
/// reported the returned value is meaningless: the caller checks
/// `r.has_errors()`.
///
/// The credentials are named-only and required, as the manual writes them,
/// and never quoted in a diagnostic: a positional value stays unread and is
/// reported as an extra positional value.
pub fn read_trust_tunnel(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TrustTunnelRead {
    let tls = read_h2_tls(r, keystore);
    let username = required(r, "username");
    let password = required(r, "password");
    let headers = read_h2_headers(r);
    let max_streams = read_max_streams(r);
    let h3 = r.bool("h3").unwrap_or(false);
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`udp-relay` does not apply to `trust-tunnel` policies (no UDP); ignored".to_string(),
        );
    }
    TrustTunnelRead {
        spec: TrustTunnelSpec {
            tls,
            username,
            password,
            headers,
            max_streams,
        },
        h3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
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

    fn tt(def: &str) -> (TrustTunnelRead, bool, Vec<Diagnostic>) {
        reader_for(def, |r| read_trust_tunnel(r, &[]))
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
            let (got, failed, _) = tt(&format!(
                "trust-tunnel, h.test, 443, username=u, password=p, max-streams={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(got.spec.max_streams, DEFAULT_MAX_STREAMS);
        }
        let (spec, failed, _) = h2("h2-connect, h.test, 443, max-streams=1");
        assert!(!failed);
        assert_eq!(spec.max_streams, 1);
    }

    #[test]
    fn the_manuals_trust_tunnel_example() {
        let (got, failed, diags) =
            tt("trust-tunnel, 192.168.20.62, 443, username=test, password=test");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!got.h3);
        let spec = got.spec;
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
        let (got, failed, diags) = tt(
            "trust-tunnel, tt.test, 443, username=u, password=p, headers=X-Padding:<random-string(16-32)>, max-streams=5, sni=tt.test, client-cert=nope",
        );
        assert!(failed, "an unknown keystore item is an error");
        assert_eq!(diags[0].code, codes::E_KEYSTORE_REF);
        assert_eq!(got.spec.max_streams, 5);
        assert_eq!(got.spec.headers.len(), 1);
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

    /// `h3=true` is read (the loader says once that it has no effect yet)
    /// and the policy connects over HTTP/2; `udp-relay` has no place here.
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
        let (got, _, diags) = tt("trust-tunnel, h.test, 443, username=u, password=p, alpn=h3");
        assert_eq!(got.spec.tls.alpn, ["h2"]);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `alpn` is always `h2` on `trust-tunnel` policies; ignored"
            )]
        );
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
        for printed in [format!("{spec:?}"), format!("{:?}", got.spec)] {
            assert!(printed.contains("Secret(***)"), "{printed}");
            assert!(
                !printed.contains("s3cretUser") && !printed.contains("hunter2"),
                "{printed}"
            );
        }
    }
}
