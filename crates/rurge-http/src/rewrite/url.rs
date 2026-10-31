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
use rurge_config::rewrite::{GATEWAY_ADDRESS, UrlMode};
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
    /// (`phase 4 M3 design 6.3`, the phase 4 / M3a plan's P10): a scheme of
    /// `http`, `https`, `ws` or `wss`, a host that is not empty, a written port
    /// that reads and is not zero, and — for `https` / `wss` — a host the
    /// origin side can verify a certificate against.
    pub(crate) fn parse(text: &str) -> Option<NewUrl> {
        let uri: Uri = text.parse().ok()?;
        let authority = uri.authority()?;
        // `ws` and `wss` are not schemes `http` names, so `scheme_str` keeps
        // them as written (`http` 1.5.0)
        let scheme = uri.scheme_str()?.to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" | "ws" => 80,
            "https" | "wss" => 443,
            _ => return None,
        };
        let port = if writes_a_port(authority) {
            match authority.port_u16() {
                Some(port) if port != 0 => port,
                // a port that does not read is not one a request can be
                // dialled to: `Authority::port` only keeps 16-bit numbers
                _ => return None,
            }
        } else {
            default_port
        };
        let host = HostName::parse(authority.host()).canonical();
        if matches!(&host, HostName::Domain(name) if name.is_empty()) {
            return None;
        }
        let host_authority = crate::engine::origin_authority(&host, port, default_port);
        // the origin side verifies a TLS name: a host it cannot use is a URL
        // whose handshake would fail after the dial (`phase 4 M3 design 8.2`)
        let name = match scheme.as_str() {
            "https" | "wss" => Some(crate::mitm::decrypt::server_name(&host).ok()?),
            _ => None,
        };
        // the empty path is `/`, and a path that starts at the query string
        // takes a `/` before it so that the URI reads back
        let path = match uri.path_and_query() {
            Some(path) if path.as_str().starts_with('?') => format!("/{}", path.as_str()),
            Some(path) => path.as_str().to_string(),
            None => "/".to_string(),
        };
        let bare = path.split('?').next().unwrap_or("/");
        // what retargeting a request needs, made here so that it cannot fail
        // there (`the phase 4 / M3a plan's execution record row 31`)
        let absolute = match scheme.as_str() {
            "ws" => format!("http://{host_authority}{path}"),
            "wss" => format!("https://{host_authority}{path}"),
            _ => format!("{scheme}://{host_authority}{path}"),
        }
        .parse::<Uri>()
        .ok()?;
        let origin = path.parse::<Uri>().ok()?;
        let host_value = HeaderValue::from_bytes(host_authority.as_bytes()).ok()?;
        Some(NewUrl {
            parts: Box::new(Parts {
                url: format!("{scheme}://{host_authority}{path}"),
                note: format!("{scheme}://{host_authority}{bare}"),
                scheme,
                host,
                port,
                name,
                absolute,
                origin,
                host_value,
            }),
        })
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

/// Whether the authority writes a port. `Authority::port` cannot tell a
/// written `:65536` from no port at all (`http` 1.5.0), so the text decides: an
/// address in brackets ends at its `]`, anything else at its last `:`, and a
/// `:` after that is a written port. The `user:password@` prefix is not part
/// of it.
fn writes_a_port(authority: &http::uri::Authority) -> bool {
    let text = authority.as_str().rsplit('@').next().unwrap_or_default();
    match text.rfind(']') {
        Some(end) => text[end + 1..].starts_with(':'),
        None => text.contains(':'),
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
    let mut urls = vec![url.to_string()];
    // the `Host` value as written: a header may carry bytes `to_str` refuses
    // although they are UTF-8 (`http` 1.5.0)
    if let Some(value) = host
        && let Ok(text) = std::str::from_utf8(value.as_bytes())
    {
        let text = text.trim();
        if !text.is_empty() {
            let rebuilt = format!("{scheme}://{text}{path_and_query}");
            if !urls.contains(&rebuilt) {
                urls.push(rebuilt);
            }
        }
    }
    let rebuilt = format!(
        "{scheme}://{}{path_and_query}",
        crate::engine::origin_authority(target, port, crate::engine::default_port(scheme))
    );
    if !urls.contains(&rebuilt) {
        urls.push(rebuilt);
    }
    urls
}

/// Sends `req` to `new`, keeping the form the request came in (`phase 4 M3
/// design 6.3`, the phase 4 / M3a plan's P10): an absolute request line stays
/// absolute, an origin-form one keeps its path and query string. `Host`
/// follows the new authority unless the client speaks HTTP/2 and named none.
pub(crate) fn retarget_request<B>(req: &mut Request<B>, new: &NewUrl, h2_client: bool) {
    // the two forms were read when the URL was, so neither decided here can
    // fail (`the phase 4 / M3a plan's execution record row 31`)
    let absolute = req.uri().scheme().is_some();
    *req.uri_mut() = if absolute {
        new.parts.absolute.clone()
    } else {
        new.parts.origin.clone()
    };
    // an HTTP/2 client that named no host has its `:authority` in the URI
    if req.headers().contains_key(http::header::HOST) || !h2_client {
        req.headers_mut()
            .insert(http::header::HOST, new.parts.host_value.clone());
    }
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

/// Evaluates the generation's URL rules in order (`phase 4 M3 design 6.2`, the
/// phase 4 / M3a plan's P9): the first rule that matches a candidate answers
/// or retargets; a `header` rule after one that took effect is not evaluated
/// and leaves no note, because the request already goes to the new URL.
pub(super) fn evaluate(
    rewrite: &Rewrite,
    candidates: &[String],
    gateway: &dyn Fn() -> Option<Ipv4Addr>,
    notes: &mut Vec<Note>,
) -> UrlOutcome {
    let mut current: Option<NewUrl> = None;
    for rule in &rewrite.url {
        if current.is_some() && rule.mode == UrlMode::Header {
            continue;
        }
        // after a `header` took effect only the new URL is left to compare:
        // the request is going there, and the other two candidates describe a
        // target it has left
        let texts: Vec<&str> = match &current {
            Some(new) => vec![new.url()],
            None => candidates.iter().map(String::as_str).collect(),
        };
        let matched = match first_match(&rule.pattern, &texts) {
            Ok(matched) => matched,
            Err(()) => {
                notes.push(Note::Skipped(backtracking(&rule.pattern.source)));
                continue;
            }
        };
        let Some(text) = matched else {
            continue;
        };
        let replaced = match super::replace_all(&rule.pattern, &rule.replacement, text) {
            Ok(replaced) => replaced,
            Err(()) => {
                notes.push(Note::Skipped(backtracking(&rule.pattern.source)));
                continue;
            }
        };
        match rule.mode {
            UrlMode::Header => match NewUrl::parse(&replaced) {
                Some(new) => {
                    notes.push(Note::Done(format!(
                        "URL Rewrite: header by {} -> {}",
                        rule.pattern.source,
                        new.note()
                    )));
                    current = Some(new);
                }
                None => notes.push(Note::Skipped(format!(
                    "URL Rewrite skipped: {}: the rewritten URL is not a valid http, https, ws or wss URL",
                    rule.pattern.source
                ))),
            },
            UrlMode::Found | UrlMode::Temporary => {
                let location = if rule.gateway {
                    match gateway() {
                        Some(address) => replaced.replace(GATEWAY_ADDRESS, &address.to_string()),
                        None => {
                            notes.push(Note::Skipped(format!(
                                "URL Rewrite skipped: {}: there is no IPv4 gateway",
                                rule.pattern.source
                            )));
                            continue;
                        }
                    }
                } else {
                    replaced
                };
                match HeaderValue::from_bytes(location.as_bytes()) {
                    Ok(location) => {
                        let status = if rule.mode == UrlMode::Found {
                            StatusCode::FOUND
                        } else {
                            StatusCode::TEMPORARY_REDIRECT
                        };
                        notes.push(Note::Done(format!(
                            "URL Rewrite: {} by {}",
                            status.as_u16(),
                            rule.pattern.source
                        )));
                        return UrlOutcome::Redirect { status, location };
                    }
                    Err(_) => notes.push(Note::Skipped(format!(
                        "URL Rewrite skipped: {}: the rewritten Location cannot be a header value",
                        rule.pattern.source
                    ))),
                }
            }
            UrlMode::Reject => {
                notes.push(Note::Done(format!(
                    "URL Rewrite: reject by {}",
                    rule.pattern.source
                )));
                return UrlOutcome::Reject;
            }
        }
    }
    UrlOutcome::Go(current)
}

/// The first of `texts` the expression matches, or `None` for none of them.
/// `Err` is the expression hitting its backtracking limit, which ends the rule
/// (nothing of it applies).
fn first_match<'a>(
    pattern: &rurge_config::rule::Pattern,
    texts: &[&'a str],
) -> Result<Option<&'a str>, ()> {
    for text in texts {
        match pattern.regex.is_match(text) {
            Ok(true) => return Ok(Some(text)),
            Ok(false) => {}
            Err(_) => return Err(()),
        }
    }
    Ok(None)
}

/// The note a rule that ran into the expression's backtracking limit leaves
/// (`the phase 4 / M3a plan's P11`).
fn backtracking(source: &str) -> String {
    format!("URL Rewrite skipped: {source}: the regular expression hit its backtracking limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rewrite::rewrite_of;
    use rurge_config::HostName;
    use rurge_net::connector::Target;
    use std::cell::Cell;
    use std::net::{IpAddr, Ipv6Addr};

    /// `text` read as a URL a request can be sent to.
    fn parsed(text: &str) -> NewUrl {
        NewUrl::parse(text).unwrap_or_else(|| panic!("{text} is a URL a request can be sent to"))
    }

    #[test]
    fn which_rewritten_urls_are_valid() {
        let url = parsed("http://a.test/");
        assert_eq!(url.url(), "http://a.test/");
        assert_eq!(url.note(), "http://a.test/");
        assert!(!url.tls());
        assert_eq!(
            url.target(),
            Target::new(HostName::Domain("a.test".into()), 80)
        );
        assert!(url.server_name().is_none());

        let url = parsed("HTTPS://a.test:8443/p?q");
        assert!(url.tls());
        assert_eq!(
            url.target(),
            Target::new(HostName::Domain("a.test".into()), 8443)
        );
        assert_eq!(url.url(), "https://a.test:8443/p?q");
        assert_eq!(url.note(), "https://a.test:8443/p");
        assert!(url.server_name().is_some());

        let url = parsed("ws://a.test");
        assert_eq!(url.url(), "ws://a.test/");
        assert_eq!(url.target().port, 80);
        assert!(!url.tls());

        let url = parsed("wss://[::1]:9/");
        assert_eq!(
            url.target(),
            Target::new(HostName::Ip(IpAddr::V6(Ipv6Addr::LOCALHOST)), 9)
        );
        assert_eq!(url.url(), "wss://[::1]:9/");
        assert!(url.tls());

        let url = parsed("http://u:p@a.test/");
        assert_eq!(url.url(), "http://a.test/");
        assert_eq!(url.note(), "http://a.test/");
        assert!(!url.note().contains("u:p"));

        let url = parsed("http://a.test/p#f");
        assert_eq!(url.url(), "http://a.test/p");

        let url = parsed("http://a.test:80/");
        assert_eq!(url.url(), "http://a.test/");

        for text in [
            "ftp://a.test/",
            "http:///x",
            "http://a.test:0/",
            "http://a.test:65536/",
            "/relative",
            "a.test",
            "",
        ] {
            assert!(NewUrl::parse(text).is_none(), "{text}");
        }
    }

    #[test]
    fn candidates_are_the_url_the_host_and_the_target() {
        let c = HostName::Domain("c.test".into());
        let a = HostName::Domain("a.test".into());
        let host = HeaderValue::from_static("b.test");
        assert_eq!(
            candidates("http", "http://a.test/x?q=1", Some(&host), "/x?q=1", &c, 80),
            [
                "http://a.test/x?q=1",
                "http://b.test/x?q=1",
                "http://c.test/x?q=1"
            ]
        );

        // the host of the URL and the target below it say nothing new
        let same = HeaderValue::from_static("a.test");
        assert_eq!(
            candidates("http", "http://a.test/x?q=1", Some(&same), "/x?q=1", &c, 80),
            ["http://a.test/x?q=1", "http://c.test/x?q=1"]
        );
        assert_eq!(
            candidates("http", "http://a.test/x?q=1", Some(&same), "/x?q=1", &a, 80),
            ["http://a.test/x?q=1"]
        );

        // a port that is not the scheme's default is written
        assert_eq!(
            candidates("http", "http://a.test/x?q=1", None, "/x?q=1", &c, 8080),
            ["http://a.test/x?q=1", "http://c.test:8080/x?q=1"]
        );
        assert_eq!(
            candidates("https", "https://a.test/x?q=1", None, "/x?q=1", &c, 443),
            ["https://a.test/x?q=1", "https://c.test/x?q=1"]
        );

        // a `Host` that is not UTF-8 names no URL
        let bytes = HeaderValue::from_bytes(b"b\xfftest").expect("a name");
        assert_eq!(
            candidates(
                "http",
                "http://a.test/x?q=1",
                Some(&bytes),
                "/x?q=1",
                &c,
                80
            ),
            ["http://a.test/x?q=1", "http://c.test/x?q=1"]
        );
    }

    /// A request with `uri` and, when `host` is given, that `Host`.
    fn request(uri: &str, host: Option<&str>) -> Request<()> {
        let mut builder = Request::builder().uri(uri);
        if let Some(host) = host {
            builder = builder.header(http::header::HOST, host);
        }
        builder.body(()).expect("a request")
    }

    /// The `Host` lines of `req`.
    fn hosts(req: &Request<()>) -> Vec<&str> {
        req.headers()
            .get_all(http::header::HOST)
            .iter()
            .map(|value| value.to_str().expect("ASCII"))
            .collect()
    }

    #[test]
    fn retargeting_keeps_the_form_and_sets_host() {
        let target = parsed("http://b.test:8080/p?q");
        let mut req = request("/a?x", Some("a.test"));
        retarget_request(&mut req, &target, false);
        assert_eq!(req.uri().to_string(), "/p?q");
        assert_eq!(hosts(&req), ["b.test:8080"]);

        let mut req = request("/a?x", None);
        retarget_request(&mut req, &target, false);
        assert_eq!(req.uri().to_string(), "/p?q");
        assert_eq!(hosts(&req), ["b.test:8080"]);

        let mut req = request("http://a.test/a", Some("a.test"));
        retarget_request(&mut req, &target, false);
        assert_eq!(req.uri().to_string(), "http://b.test:8080/p?q");
        assert_eq!(hosts(&req), ["b.test:8080"]);

        // an HTTP/2 client names its host in `:authority` alone, which the URI
        // carries: no `Host` is added
        let secure = parsed("https://b.test/p");
        let mut req = request("https://a.test/a", None);
        retarget_request(&mut req, &secure, true);
        assert_eq!(req.uri().to_string(), "https://b.test/p");
        assert!(hosts(&req).is_empty());

        // an upgrade's request line is an `http` or an `https` URL; a `ws`
        // URL is reached from an absolute request line, as an HTTP listener
        // sends it
        let mut req = request("http://a.test/a", Some("a.test"));
        retarget_request(&mut req, &parsed("ws://b.test/s"), false);
        assert_eq!(req.uri().to_string(), "http://b.test/s");
        let mut req = request("http://a.test/a", Some("a.test"));
        retarget_request(&mut req, &parsed("wss://b.test/s"), false);
        assert_eq!(req.uri().to_string(), "https://b.test/s");
    }

    /// The candidates of `url`, as the engine builds them for a plain
    /// connection to `target`.
    fn one(url: &str) -> Vec<String> {
        vec![url.to_string()]
    }

    #[test]
    fn only_the_first_header_rule_applies() {
        let rewrite = rewrite_of(
            "[URL Rewrite]\n\
             ^http://a\\.test http://b.test header\n\
             ^http://a\\.test http://c.test header\n\
             ^http://b\\.test https://d.test/ 302",
        );
        let mut notes = Vec::new();
        let outcome = rewrite.url(&one("http://a.test/x"), &|| None, &mut notes);
        match outcome {
            UrlOutcome::Redirect { status, location } => {
                assert_eq!(status, StatusCode::FOUND);
                assert_eq!(location, "https://d.test//x");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            notes,
            [
                Note::Done(
                    "URL Rewrite: header by ^http://a\\.test -> http://b.test/x".to_string()
                ),
                Note::Done("URL Rewrite: 302 by ^http://b\\.test".to_string()),
            ]
        );
    }

    #[test]
    fn an_invalid_header_result_does_not_count_as_the_first() {
        let rewrite = rewrite_of(
            "[URL Rewrite]\n\
             ^http://a\\.test ftp://b.test header\n\
             ^http://a\\.test http://c.test header",
        );
        let mut notes = Vec::new();
        let outcome = rewrite.url(&one("http://a.test/x"), &|| None, &mut notes);
        match outcome {
            UrlOutcome::Go(Some(new)) => assert_eq!(new.url(), "http://c.test/x"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            notes,
            [
                Note::Skipped(
                    "URL Rewrite skipped: ^http://a\\.test: the rewritten URL is not a valid http, https, ws or wss URL"
                        .to_string()
                ),
                Note::Done(
                    "URL Rewrite: header by ^http://a\\.test -> http://c.test/x".to_string()
                ),
            ]
        );
    }

    #[test]
    fn redirects_put_the_gateway_in_and_give_up_without_one() {
        let rewrite = rewrite_of(
            "[URL Rewrite]\n\
             ^http://r\\.test http://{{{GATEWAY_ADDRESS}}}/login 307\n\
             ^http://r\\.test https://s.test/ 302",
        );

        let asked = Cell::new(0);
        let gateway = || {
            asked.set(asked.get() + 1);
            Some(Ipv4Addr::new(127, 0, 0, 2))
        };
        let mut notes = Vec::new();
        let outcome = rewrite.url(&one("http://r.test/x"), &gateway, &mut notes);
        match outcome {
            UrlOutcome::Redirect { status, location } => {
                assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
                assert_eq!(location, "http://127.0.0.2/login/x");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(asked.get(), 1);
        assert_eq!(
            notes,
            [Note::Done(
                "URL Rewrite: 307 by ^http://r\\.test".to_string()
            )]
        );

        let asked = Cell::new(0);
        let none = || {
            asked.set(asked.get() + 1);
            None
        };
        let mut notes = Vec::new();
        let outcome = rewrite.url(&one("http://r.test/x"), &none, &mut notes);
        match outcome {
            UrlOutcome::Redirect { status, location } => {
                assert_eq!(status, StatusCode::FOUND);
                assert_eq!(location, "https://s.test//x");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(asked.get(), 1);
        assert_eq!(
            notes,
            [
                Note::Skipped(
                    "URL Rewrite skipped: ^http://r\\.test: there is no IPv4 gateway".to_string()
                ),
                Note::Done("URL Rewrite: 302 by ^http://r\\.test".to_string()),
            ]
        );
    }

    #[test]
    fn a_location_that_cannot_be_a_header_value_is_skipped() {
        // a control character reaches the replacement as written: the loader
        // only checks header values, and a `Location` is made here
        let rewrite = rewrite_of("[URL Rewrite]\n^http://a\\.test a\u{1}b 302");
        let mut notes = Vec::new();
        let outcome = rewrite.url(&one("http://a.test/x"), &|| None, &mut notes);
        assert!(matches!(outcome, UrlOutcome::Go(None)), "{outcome:?}");
        assert_eq!(
            notes,
            [Note::Skipped(
                "URL Rewrite skipped: ^http://a\\.test: the rewritten Location cannot be a header value"
                    .to_string()
            )]
        );
    }

    #[test]
    fn reject_answers_and_ends_the_rules() {
        let rewrite = rewrite_of(
            "[URL Rewrite]\n\
             ^http://ad\\.com/ad\\.png _ reject\n\
             ^http://ad\\.com https://elsewhere.test/ header",
        );
        let mut notes = Vec::new();
        let outcome = rewrite.url(&one("http://ad.com/ad.png"), &|| None, &mut notes);
        assert!(matches!(outcome, UrlOutcome::Reject), "{outcome:?}");
        assert_eq!(
            notes,
            [Note::Done(
                "URL Rewrite: reject by ^http://ad\\.com/ad\\.png".to_string()
            )]
        );
    }

    #[test]
    fn a_rule_that_hits_the_backtracking_limit_is_skipped_with_a_note() {
        let lines = "[URL Rewrite]\n(?i)(a|b|ab)*(?=c) http://b.test header";
        let cfg = crate::rewrite::config_of(lines);
        let url = format!("http://x.test/{}", "ab".repeat(40));
        // the input really does run the expression over its limit
        assert!(
            cfg.url[0].pattern.regex.is_match(&url).is_err(),
            "the expression gives up on this input"
        );
        let rewrite = Rewrite::build(
            &cfg,
            &MitmConfig::default(),
            crate::rewrite::test_roots(),
            Arc::new(RewriteCounters::default()),
        )
        .expect("a generation")
        .expect("the line makes a rule");
        let mut notes = Vec::new();
        let outcome = rewrite.url(&one(&url), &|| None, &mut notes);
        assert!(matches!(outcome, UrlOutcome::Go(None)), "{outcome:?}");
        assert_eq!(
            notes,
            [Note::Skipped(
                "URL Rewrite skipped: (?i)(a|b|ab)*(?=c): the regular expression hit its backtracking limit"
                    .to_string()
            )]
        );
    }
}
