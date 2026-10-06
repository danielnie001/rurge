//! TCP over MASQUE (manual: Policies › MASQUE; phase 2 M7 design 7.1):
//! every connection a classic HTTP/3 CONNECT of its own on the policy's
//! QUIC connection (RFC 9114 4.4), its tunnel as `connect` makes it.
//!
//! - The request is `:method CONNECT` and `:authority host:port` — an IPv6
//!   literal in brackets, a name as its A-labels —, with
//!   `proxy-authorization` when the policy has credentials.
//! - 407 is a refused login, any other final status but a 2xx the proxy's
//!   refusal, quoted by its number (`refused`).

use super::proxy;
use http::Request;
use http::header::{HeaderValue, PROXY_AUTHORIZATION};
use rurge_net::connector::Target;
use rurge_proto::OutboundError;
use rurge_proto::http::wire_host;

/// The CONNECT for `target`, carrying the policy's `authorization`: refused,
/// before anything is dialed, for a name that cannot be sent.
pub(super) fn request(
    target: &Target,
    authorization: Option<&HeaderValue>,
) -> Result<Request<()>, OutboundError> {
    let unsendable = || proxy("the host name cannot be sent to the server");
    let host = wire_host(target).ok_or_else(unsendable)?;
    let mut request = Request::connect(format!("{host}:{}", target.port))
        .body(())
        // a name longer than an authority may be
        .map_err(|_| unsendable())?;
    if let Some(authorization) = authorization {
        request
            .headers_mut()
            .insert(PROXY_AUTHORIZATION, authorization.clone());
    }
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::Method;
    use rurge_config::HostName;

    #[test]
    fn a_request_names_its_target_as_host_and_port_and_nothing_else() {
        for (host, authority) in [
            (HostName::parse("192.0.2.1"), "192.0.2.1:443"),
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
        ] {
            let request = request(&Target::new(host, 443), None).unwrap();
            assert_eq!(request.method(), Method::CONNECT);
            let uri = request.uri();
            assert_eq!(uri.authority().map(|a| a.as_str()), Some(authority));
            // a classic CONNECT: no scheme, no path (RFC 9114 4.4)
            assert!(uri.scheme().is_none(), "{uri}");
            assert!(uri.path_and_query().is_none(), "{uri}");
            assert!(request.headers().is_empty());
        }
        // longer than an authority may be
        let long = format!("{}.test", "a".repeat(70_000));
        for name in ["x@blocked.test", "a b.test", "", long.as_str()] {
            let target = Target::new(HostName::Domain(name.into()), 443);
            let err = request(&target, None).unwrap_err();
            assert_eq!(
                err.to_string(),
                "masque: the host name cannot be sent to the server"
            );
        }
    }

    #[test]
    fn the_credentials_go_marked_sensitive() {
        let mut authorization = HeaderValue::from_static("Basic dTpw");
        authorization.set_sensitive(true);
        let target = Target::new(HostName::parse("192.0.2.1"), 443);
        let request = request(&target, Some(&authorization)).unwrap();
        let headers = request.headers();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[PROXY_AUTHORIZATION], "Basic dTpw");
        // QPACK never indexes it (RFC 9204 7.1.3)
        assert!(headers[PROXY_AUTHORIZATION].is_sensitive());
        assert!(!format!("{request:?}").contains("dTpw"));
    }
}
