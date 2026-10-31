//! What `[URL Rewrite]` does with a request's URL (`phase 4 M3 design 6`): the
//! three URLs a rule is tried against, the new URL a `header` rule leaves, the
//! answer a `302` / `307` / `reject` rule gives, and the request the new URL
//! makes.
//!
//! A new URL is read once, when a rule produced it, and what the request then
//! needs is prepared there too (`phase 4 M3 design 6.3`, the phase 4 / M3a
//! plan's P10): the absolute and the origin form of the request target and the
//! `Host` value are all made by `NewUrl::parse`, so retargeting a request has
//! no way to fail and cannot panic.

use super::*;
use http::{HeaderValue, Request, Response, StatusCode, Uri};
use rurge_inbound::responses::{self, ResponseBody};

/// What the URL rules came to for one request (`phase 4 M3 design 6.2`): go on
/// with the request as it is or as `Some` makes it, answer a redirect, or
/// reject it.
#[derive(Clone, Debug)]
pub(crate) enum UrlOutcome {
    Go(Option<NewUrl>),
    Redirect {
        status: StatusCode,
        location: HeaderValue,
    },
    Reject,
}

/// A rewritten URL that a request can be sent to (`phase 4 M3 design 6.3`).
///
/// The parts are boxed because one of these is the `header` outcome of a
/// request's URL rules: the outcome travels through the engine's request path
/// once per rewritten request, and its largest arm should not set that size.
#[derive(Clone, Debug)]
pub(crate) struct NewUrl {
    parts: Box<Parts>,
}

/// What `NewUrl` keeps.
#[derive(Clone, Debug)]
struct Parts {
    /// The scheme as the URL reads it, lowercased: `http`, `https`, `ws` or
    /// `wss`.
    scheme: String,
    /// The authority a URL and a `Host` value carry: the port left out when it
    /// is the scheme's default, an IPv6 address in brackets.
    authority: String,
    /// The path and query string, the empty one as `/`.
    path: String,
    host: HostName,
    port: u16,
    /// The name an `https` / `wss` origin is verified against; `None` for a
    /// plain one.
    name: Option<ServerName<'static>>,
    /// `{scheme}://{authority}{path}`, what rules after a `header` compare
    /// (`phase 4 M3 design 8.1`).
    url: String,
    /// `{scheme}://{authority}{the path alone}`, what a note writes.
    note: String,
    /// The request target in absolute form, `ws` written as `http` and `wss`
    /// as `https`.
    absolute: Uri,
    /// The request target in origin form: the path and query string.
    origin: Uri,
    /// The authority as a `Host` value.
    host_value: HeaderValue,
}

impl NewUrl {
    /// Reads `text`, or `None` when it is not a URL a request can be sent to
    /// (`phase 4 M3 design 6.3`, the phase 4 / M3a plan's P10).
    pub(crate) fn parse(text: &str) -> Option<NewUrl> {
        let _ = text;
        None
    }

    /// The whole URL, as rules after a `header` compare it.
    pub(crate) fn url(&self) -> &str {
        &self.parts.url
    }

    /// Whether the origin side speaks TLS: `https` or `wss`.
    pub(crate) fn tls(&self) -> bool {
        self.parts.scheme == "https" || self.parts.scheme == "wss"
    }

    /// Where the request is dialled.
    pub(crate) fn target(&self) -> rurge_net::connector::Target {
        rurge_net::connector::Target::new(self.parts.host.clone(), self.parts.port)
    }

    /// The name an `https` / `wss` origin is verified against.
    pub(crate) fn server_name(&self) -> Option<&ServerName<'static>> {
        self.parts.name.as_ref()
    }

    /// The URL a note writes: no query string (`phase 4 M3 design 8.5`).
    pub(crate) fn note(&self) -> String {
        self.parts.note.clone()
    }
}

/// The URLs a rule is tried against, in the manual's order (`phase 4 M3 design
/// 6.2`): the request's own URL, the one rebuilt from `Host`, and the one
/// rebuilt from the connection's target. A URL equal to one already in the
/// list is left out.
pub(crate) fn candidates(
    scheme: &str,
    url: &str,
    host: Option<&HeaderValue>,
    path_and_query: &str,
    target: &HostName,
    port: u16,
) -> Vec<String> {
    let _ = (scheme, url, host, path_and_query, target, port);
    Vec::new()
}

/// Sends `req` to `new`, keeping the form the request came in (`phase 4 M3
/// design 6.3`, the phase 4 / M3a plan's P10): an absolute request line stays
/// absolute, an origin-form one keeps its path and query string. `Host`
/// follows the new authority unless the client speaks HTTP/2 and named none.
pub(crate) fn retarget_request<B>(req: &mut Request<B>, new: &NewUrl, h2_client: bool) {
    let _ = (req, new, h2_client);
}

/// The answer a `302` / `307` rule gives (`phase 4 M3 design 6.4`, the phase 4
/// / M3a plan's P13): the status, `Location` and an empty body. The
/// connection stays open.
pub(crate) fn redirect(status: StatusCode, location: HeaderValue) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .header(http::header::LOCATION, location)
        .header(http::header::CONTENT_LENGTH, "0")
        .body(responses::empty())
        .expect("static response")
}
