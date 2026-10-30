//! The form of a request on the side it is sent to (`phase 4 M2 design 10.3`):
//! the two versions differ in where the authority lives, how `cookie` is
//! spelled, and which headers may cross at all. A request keeps what it names
//! across a fitting, so that a request sent again on a connection of the other
//! version goes out as the same request.

use crate::engine::path_and_query;
use crate::headers;
use http::{HeaderValue, Request, Uri, Version, header};

/// What a request names as its authority, and whether it may see trailers
/// (`phase 4 M2 design 10.3`, `the phase 4 / M2b plan's P8`). Read once, before
/// the hop-by-hop headers are stripped: `TE` is one of them.
pub(crate) struct Form {
    pub(crate) authority: String,
    pub(crate) trailers: bool,
}

impl Form {
    /// The form of `req`: the authority of its URI, else of its `Host`, else
    /// the connection's target; and whether its `TE` asks for trailers.
    pub(crate) fn of<B>(req: &Request<B>, fallback: String) -> Form {
        let named = req.uri().authority().map(headers::host_port).or_else(|| {
            req.headers()
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<http::uri::Authority>().ok())
                .map(|authority| headers::host_port(&authority))
        });
        // `trailers` is one of the items `TE` lists, in any case (RFC 9110
        // §10.1.4), and `TE` may come as several fields.
        let trailers = req
            .headers()
            .get_all(header::TE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|item| item.trim().eq_ignore_ascii_case("trailers"));
        Form {
            authority: named.unwrap_or(fallback),
            trailers,
        }
    }
}

/// Puts `req` into the form `to` takes it in (`phase 4 M2 design 10.3`, `the
/// phase 4 / M2b plan's P8`). A request already in that form is left alone.
///
/// To HTTP/1.1: the target becomes origin form, `Host` is the authority the
/// request names (RFC 9113 §8.3.1: a middlebox writes the `:authority` there),
/// several `cookie` fields become one joined with `; ` (RFC 9113 §8.2.3), and
/// `TE` is dropped: `trailers` is a promise only HTTP/2 asks for.
///
/// To HTTP/2: the target becomes an absolute `https://` one, which hyper's
/// HTTP/2 client writes as `:scheme` and `:authority`; `Host` and
/// `Transfer-Encoding` go (h2 refuses them), and `TE` is either `trailers` or
/// gone, because nothing else of `TE` may cross.
pub(crate) fn fit<B>(req: &mut Request<B>, to: Version, form: &Form) -> Result<(), http::Error> {
    match to {
        Version::HTTP_11 if req.version() == Version::HTTP_2 => {
            let path = path_and_query(req.uri()).to_string();
            *req.uri_mut() = path.parse::<Uri>()?;
            match HeaderValue::from_str(&form.authority) {
                Ok(authority) => {
                    req.headers_mut().insert(header::HOST, authority);
                }
                Err(_) => {
                    req.headers_mut().remove(header::HOST);
                }
            }
            *req.version_mut() = Version::HTTP_11;
            join_cookies(req);
            req.headers_mut().remove(header::TE);
        }
        Version::HTTP_2 => {
            let path = path_and_query(req.uri()).to_string();
            *req.uri_mut() = format!("https://{}{}", form.authority, path).parse::<Uri>()?;
            req.headers_mut().remove(header::HOST);
            req.headers_mut().remove(header::TRANSFER_ENCODING);
            req.headers_mut().remove(header::TE);
            if form.trailers {
                req.headers_mut()
                    .insert(header::TE, HeaderValue::from_static("trailers"));
            }
            *req.version_mut() = Version::HTTP_2;
        }
        _ => {}
    }
    Ok(())
}

/// Joins several `cookie` fields into one, separated by `; ` (RFC 9113
/// §8.2.3): HTTP/1.1 has one field, HTTP/2 may have any number.
fn join_cookies<B>(req: &mut Request<B>) {
    let cookies: Vec<HeaderValue> = req
        .headers()
        .get_all(header::COOKIE)
        .iter()
        .cloned()
        .collect();
    if cookies.len() < 2 {
        return;
    }
    let mut joined = Vec::new();
    for (at, cookie) in cookies.iter().enumerate() {
        if at > 0 {
            joined.extend_from_slice(b"; ");
        }
        joined.extend_from_slice(cookie.as_bytes());
    }
    // the parts were header values and the separator is visible ASCII, so the
    // joined value is one too; a value the http crate still refuses is dropped
    // rather than sent malformed
    req.headers_mut().remove(header::COOKIE);
    if let Ok(value) = HeaderValue::from_bytes(&joined) {
        req.headers_mut().insert(header::COOKIE, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request with the headers a form is read from or written to.
    fn request(version: Version, uri: &str, headers: &[(&str, &str)]) -> Request<()> {
        let mut req = Request::builder()
            .version(version)
            .uri(uri)
            .body(())
            .expect("a request");
        for (name, value) in headers {
            req.headers_mut().append(
                http::HeaderName::try_from(*name).expect("a header name"),
                HeaderValue::from_str(value).expect("a header value"),
            );
        }
        req
    }

    fn header<B>(req: &Request<B>, name: header::HeaderName) -> Option<&str> {
        req.headers().get(name).map(|v| v.to_str().unwrap())
    }

    #[test]
    fn a_form_takes_the_uri_then_host_then_the_target() {
        let req = request(
            Version::HTTP_2,
            "https://u:p@a.test:8443/p",
            &[("host", "b.test")],
        );
        assert_eq!(
            Form::of(&req, "fallback.test".into()).authority,
            "a.test:8443",
            "the authority of the URI, without the userinfo"
        );
        let req = request(Version::HTTP_11, "/p", &[("host", "b.test")]);
        assert_eq!(Form::of(&req, "fallback.test".into()).authority, "b.test");
        let req = request(Version::HTTP_11, "/p", &[]);
        assert_eq!(
            Form::of(&req, "fallback.test".into()).authority,
            "fallback.test"
        );
        for (te, trailers) in [
            ("trailers", true),
            ("deflate, Trailers", true),
            ("deflate", false),
        ] {
            let req = request(Version::HTTP_2, "https://a.test/p", &[("te", te)]);
            assert_eq!(Form::of(&req, String::new()).trailers, trailers, "{te}");
        }
        let req = request(Version::HTTP_2, "https://a.test/p", &[]);
        assert!(!Form::of(&req, String::new()).trailers);
    }

    #[test]
    fn an_h2_request_is_put_into_http1_form() {
        let mut req = request(
            Version::HTTP_2,
            "https://a.test:8443/p?q=1",
            &[("cookie", "a=1"), ("cookie", "b=2"), ("te", "trailers")],
        );
        let form = Form::of(&req, "fallback.test".into());
        fit(&mut req, Version::HTTP_11, &form).expect("a fitting request");
        assert_eq!(req.uri(), "/p?q=1");
        assert_eq!(req.version(), Version::HTTP_11);
        assert_eq!(header(&req, header::HOST), Some("a.test:8443"));
        let cookies: Vec<&str> = req
            .headers()
            .get_all(header::COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap())
            .collect();
        assert_eq!(cookies, vec!["a=1; b=2"]);
        assert_eq!(header(&req, header::TE), None);
    }

    #[test]
    fn an_http1_request_is_put_into_h2_form() {
        let mut req = request(
            Version::HTTP_11,
            "/p",
            &[("host", "a.test"), ("transfer-encoding", "chunked")],
        );
        let form = Form {
            authority: "a.test".into(),
            trailers: true,
        };
        fit(&mut req, Version::HTTP_2, &form).expect("a fitting request");
        assert_eq!(req.uri(), "https://a.test/p");
        assert_eq!(req.version(), Version::HTTP_2);
        assert_eq!(header(&req, header::HOST), None);
        assert_eq!(header(&req, header::TRANSFER_ENCODING), None);
        assert_eq!(header(&req, header::TE), Some("trailers"));
    }

    #[test]
    fn an_http1_request_to_an_http1_upstream_is_left_alone() {
        let mut req = request(
            Version::HTTP_11,
            "http://a.test/p",
            &[("cookie", "a=1"), ("cookie", "b=2")],
        );
        let before = req.clone();
        let form = Form::of(&req, "fallback.test".into());
        fit(&mut req, Version::HTTP_11, &form).expect("a fitting request");
        assert_eq!(req.uri(), before.uri());
        assert_eq!(req.version(), before.version());
        assert_eq!(req.headers(), before.headers());
    }

    #[test]
    fn fitting_into_h2_then_http1_then_h2_is_fitting_into_h2_once() {
        // Joining several `cookie` fields is the one step that does not undo
        // itself, so a request that has been through HTTP/1.1 carries one
        // field; this one starts that way.
        let mut round = request(
            Version::HTTP_2,
            "https://a.test:8443/p?q=1",
            &[("cookie", "a=1; b=2"), ("te", "trailers")],
        );
        let form = Form::of(&round, "fallback.test".into());
        fit(&mut round, Version::HTTP_2, &form).expect("a fitting request");
        let once = round.clone();
        fit(&mut round, Version::HTTP_11, &form).expect("a fitting request");
        fit(&mut round, Version::HTTP_2, &form).expect("a fitting request");
        assert_eq!(round.uri(), once.uri());
        assert_eq!(round.version(), once.version());
        assert_eq!(round.headers(), once.headers());
    }
}
