//! The hop-by-hop headers of an HTTP/1.1 exchange (`phase 4 M1 design 8.7`):
//! what a proxy must not pass on in either direction, and what `Host` says
//! once a request has been rewritten for its upstream.

use http::{HeaderMap, HeaderName, HeaderValue, header};

/// Connection-specific headers a proxy must not pass on (RFC 7230 §6.1),
/// plus the two `Proxy-*` headers that belong to this hop only.
const HOP_BY_HOP: [HeaderName; 9] = [
    header::CONNECTION,
    HeaderName::from_static("keep-alive"),
    HeaderName::from_static("proxy-connection"),
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
];

/// The two headers of the list a caller may have a reason to keep.
#[derive(Clone, Copy, Debug)]
pub struct Keep {
    /// An upgrade (WebSocket) keeps `Upgrade`, and `Connection` comes back
    /// naming exactly it (`phase 4 M1 design 8.9`).
    pub upgrade: bool,
    /// A request on a sniffed connection carries the client's credentials for
    /// its own proxy, which are not this hop's to consume (`design 8.7`).
    pub proxy_authorization: bool,
}

impl Keep {
    /// Nothing kept: the ordinary request or response.
    pub const NONE: Keep = Keep {
        upgrade: false,
        proxy_authorization: false,
    };

    /// Whether `name` is one of the headers `Connection` is allowed to name
    /// and this hop keeps anyway.
    fn keeps(&self, name: &HeaderName) -> bool {
        (self.upgrade && name == header::UPGRADE)
            || (self.proxy_authorization && name == header::PROXY_AUTHORIZATION)
    }
}

/// Removes every hop-by-hop header, including the ones `Connection` names.
/// hyper re-frames the body itself, so dropping `Transfer-Encoding` is safe.
pub fn strip(headers: &mut HeaderMap, keep: Keep) {
    let listed: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|t| HeaderName::try_from(t.trim()).ok())
        .filter(|name| !keep.keeps(name))
        .collect();
    for name in listed {
        headers.remove(name);
    }
    for name in HOP_BY_HOP {
        if !keep.keeps(&name) {
            headers.remove(name);
        }
    }
    if keep.upgrade {
        // `Connection` named the upgrade among other things before; what the
        // next hop is told is exactly this one token.
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    }
}

/// The authority without any `user:password@` prefix (RFC 7230 §5.4 forbids
/// userinfo in `Host`, and it must never reach a log or the session record).
pub fn host_port(authority: &http::uri::Authority) -> String {
    match authority.port_u16() {
        Some(p) => format!("{}:{p}", authority.host()),
        None => authority.host().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_by_hop_headers_and_the_ones_connection_names_go() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("keep-alive, X-Hop"));
        headers.insert("x-hop", HeaderValue::from_static("leaked"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("te", HeaderValue::from_static("trailers"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        headers.insert("proxy-connection", HeaderValue::from_static("keep-alive"));
        headers.insert("x-keep", HeaderValue::from_static("kept"));
        strip(&mut headers, Keep::NONE);
        assert_eq!(
            headers.keys().map(|k| k.as_str()).collect::<Vec<_>>(),
            vec!["x-keep"]
        );
    }

    #[test]
    fn an_upgrade_keeps_upgrade_and_connection_upgrade() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("Upgrade, X-Hop"));
        headers.insert("x-hop", HeaderValue::from_static("leaked"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        headers.insert("x-keep", HeaderValue::from_static("kept"));
        strip(
            &mut headers,
            Keep {
                upgrade: true,
                proxy_authorization: false,
            },
        );
        assert_eq!(headers.get("upgrade").unwrap(), "websocket");
        assert_eq!(headers.get("connection").unwrap(), "upgrade");
        assert_eq!(
            headers.get_all("connection").iter().count(),
            1,
            "exactly one Connection comes back"
        );
        assert!(headers.get("x-hop").is_none());
        assert!(headers.get("x-keep").is_some());
    }

    #[test]
    fn proxy_authorization_stays_only_when_asked() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "proxy-authorization",
            HeaderValue::from_static("Basic dXA="),
        );

        let mut kept = headers.clone();
        strip(
            &mut kept,
            Keep {
                upgrade: false,
                proxy_authorization: true,
            },
        );
        assert_eq!(
            kept.get("proxy-authorization").unwrap(),
            "Basic dXA=",
            "a sniffed request's credentials go to the client's own proxy"
        );

        let mut gone = headers;
        strip(&mut gone, Keep::NONE);
        assert!(gone.get("proxy-authorization").is_none());
    }

    #[test]
    fn host_port_has_no_userinfo() {
        let authority: http::uri::Authority = "user:pw@example.test:8080".parse().unwrap();
        assert_eq!(host_port(&authority), "example.test:8080");
        let authority: http::uri::Authority = "example.test".parse().unwrap();
        assert_eq!(host_port(&authority), "example.test");
    }
}
