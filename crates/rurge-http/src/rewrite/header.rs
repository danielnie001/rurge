//! What `[Header Rewrite]` does with a header table (`phase 4 M3 design 7`):
//! the four actions, the framing headers that may not change, and the `Host`
//! a request sends on.
//!
//! The table is changed in place, one rule after another, so that a later rule
//! sees what an earlier one left (`phase 4 M3 design 7.2`). Nothing here reads
//! a value into a note: the notes say which rule acted, and a value may not
//! leave (`phase 4 M3 design 8.5`).

use super::*;
use http::{HeaderMap, HeaderValue, Request, Version};
use rurge_config::rewrite::Direction;

/// What the header rules of one direction came to (`phase 4 M3 design 7.2` –
/// `7.4`): whether the table changed, whether the message's framing changed,
/// and what `Host` became.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Headers {
    pub(crate) changed: bool,
    pub(crate) framing: bool,
    pub(crate) host: HostChange,
}

/// The `Host` a request sends on after the rules of its direction
/// (`phase 4 M3 design 7.4`, the phase 4 / M3a plan's P9).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum HostChange {
    #[default]
    Same,
    Changed(HeaderValue),
}

/// Writes the rewritten `Host` back after the request's form is settled
/// (`phase 4 M3 design 7.4`): `Host` for an HTTP/1.1 upstream, the authority of
/// the request target for an HTTP/2 one. `false` when the value cannot be an
/// authority there.
pub(crate) fn write_back_host<B>(
    req: &mut Request<B>,
    host: &HeaderValue,
    version: Version,
) -> bool {
    if version != Version::HTTP_2 {
        req.headers_mut().insert(http::header::HOST, host.clone());
        return true;
    }
    // an HTTP/2 upstream names its target in `:authority`, which is the URI's;
    // a value that is no authority is left for the note the caller leaves
    let Some(scheme) = req.uri().scheme_str() else {
        return false;
    };
    let Some(authority) = host
        .to_str()
        .ok()
        .and_then(|text| text.parse::<http::uri::Authority>().ok())
    else {
        return false;
    };
    let path = crate::engine::path_and_query(req.uri());
    let Ok(uri) = format!("{scheme}://{authority}{path}").parse::<http::Uri>() else {
        return false;
    };
    *req.uri_mut() = uri;
    req.headers_mut().remove(http::header::HOST);
    true
}

/// Evaluates the generation's header rules of one direction, in order (`phase
/// 4 M3 design 7.2` – `7.4`, the phase 4 / M3a plan's P9): each rule sees what
/// the ones before it left.
pub(super) fn evaluate(
    rewrite: &Rewrite,
    direction: Direction,
    url: &str,
    headers: &mut HeaderMap,
    notes: &mut Vec<Note>,
) -> Headers {
    let framing = framing_lines(headers);
    // the `Host` lines a request came with, to give back if a rule would leave
    // none (an HTTP/1.1 request must name its host)
    let host = if direction == Direction::Request {
        values(headers, &http::header::HOST)
    } else {
        Vec::new()
    };
    let mut out = Headers::default();
    for rule in &rewrite.header {
        if rule.direction != direction {
            continue;
        }
        match rule.pattern.regex.is_match(url) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => {
                notes.push(Note::Skipped(backtracking(&rule.pattern.source)));
                continue;
            }
        }
        let name = rule.action.header();
        let before = values(headers, name);
        match &rule.action {
            HeaderRuleAction::Add { value, .. } => {
                headers.append(name.clone(), value.clone());
            }
            HeaderRuleAction::Del { .. } => {
                headers.remove(name);
            }
            HeaderRuleAction::Replace { value, .. } => {
                if let http::header::Entry::Occupied(mut entry) = headers.entry(name.clone()) {
                    // the first line keeps its place among the names
                    entry.insert(value.clone());
                }
            }
            HeaderRuleAction::ReplaceRegex {
                regex, template, ..
            } => {
                let mut skipped = None;
                if let http::header::Entry::Occupied(mut entry) = headers.entry(name.clone()) {
                    for value in entry.iter_mut() {
                        // a value with bytes that are not UTF-8 is left alone
                        let Ok(text) = std::str::from_utf8(value.as_bytes()) else {
                            continue;
                        };
                        match super::replace_all(regex, template, text) {
                            Ok(replaced) => match HeaderValue::from_bytes(replaced.as_bytes()) {
                                Ok(replaced) => *value = replaced,
                                Err(_) => {
                                    skipped = Some(Note::Skipped(format!(
                                        "Header Rewrite skipped: {}: header-replace-regex made a value a header cannot carry",
                                        rule.pattern.source
                                    )));
                                }
                            },
                            Err(()) => {
                                skipped = Some(Note::Skipped(backtracking(&regex.source)));
                            }
                        }
                    }
                }
                if let Some(note) = skipped {
                    notes.push(note);
                }
            }
        }
        if values(headers, name) != before {
            out.changed = true;
            notes.push(Note::Done(format!(
                "Header Rewrite: {} {} {} by {}",
                direction_name(direction),
                rule.action.verb(),
                rule.name,
                rule.pattern.source
            )));
        }
        // a message's framing may not change (`phase 4 M3 design 7.3`)
        if framing_lines(headers) != framing {
            out.framing = true;
            notes.push(Note::Failed(format!(
                "Header Rewrite failed: {}: Content-Length or Transfer-Encoding changed",
                rule.pattern.source
            )));
            return out;
        }
    }
    if direction == Direction::Request {
        let now = values(headers, &http::header::HOST);
        if now != host {
            if now.is_empty() {
                // no `Host` at all is no request an HTTP/1.1 origin takes
                for value in host {
                    headers.append(http::header::HOST, value);
                }
                notes.push(Note::Skipped(
                    "Header Rewrite skipped: Host cannot be removed".to_string(),
                ));
            } else {
                // the last line is the one an added line put there
                let last = now.last().cloned().expect("a line is left");
                headers.insert(http::header::HOST, last.clone());
                out.host = HostChange::Changed(last);
            }
        }
    }
    out
}

/// The lines `name` has, in order.
fn values(headers: &HeaderMap, name: &http::HeaderName) -> Vec<HeaderValue> {
    headers.get_all(name).iter().cloned().collect()
}

/// The lines of the two names that say how a message is framed (`phase 4 M3
/// design 7.3`): comparing this before and after a rule finds every change.
fn framing_lines(headers: &HeaderMap) -> [Vec<HeaderValue>; 2] {
    [
        values(headers, &http::header::CONTENT_LENGTH),
        values(headers, &http::header::TRANSFER_ENCODING),
    ]
}

/// The note a rule that ran into the expression's backtracking limit leaves
/// (`the phase 4 / M3a plan's P11`). `source` is the URL's expression, or the
/// `header-replace-regex` one when that is what ran out.
fn backtracking(source: &str) -> String {
    format!("Header Rewrite skipped: {source}: the regular expression hit its backtracking limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rewrite::rewrite_of;
    use http::HeaderName;
    use rurge_config::rewrite::Direction;

    /// A header table with the lines of `list`, in that order, duplicates and
    /// all.
    fn map(list: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in list {
            map.append(
                HeaderName::from_bytes(name.as_bytes()).expect("a name"),
                HeaderValue::from_str(value).expect("a value"),
            );
        }
        map
    }

    /// The lines `name` has in `map`, in order.
    fn values_of(map: &HeaderMap, name: &str) -> Vec<String> {
        map.get_all(name)
            .iter()
            .map(|value| value.to_str().expect("ASCII").to_string())
            .collect()
    }

    #[test]
    fn the_four_actions() {
        // header-add keeps the line that is there and appends another one
        let add = rewrite_of("[Header Rewrite]\n^http://a\\.test header-add dnt 1");
        let mut headers = map(&[("DNT", "0")]);
        let mut notes = Vec::new();
        let out = add.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(values_of(&headers, "DNT"), ["0", "1"]);
        assert!(out.changed);
        assert!(!out.framing);
        assert_eq!(
            notes,
            [Note::Done(
                "Header Rewrite: request header-add dnt by ^http://a\\.test".to_string()
            )]
        );

        // header-del removes every line of the name
        let del = rewrite_of("[Header Rewrite]\n^http://a\\.test header-del Cookie");
        let mut headers = map(&[("Cookie", "a=1"), ("X-Other", "x"), ("Cookie", "b=2")]);
        let mut notes = Vec::new();
        del.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert!(values_of(&headers, "Cookie").is_empty());
        assert_eq!(values_of(&headers, "X-Other"), ["x"]);

        // header-replace does nothing without the name, and one line where the
        // first of several was
        let replace =
            rewrite_of("[Header Rewrite]\n^http://a\\.test header-replace User-Agent Unknown");
        let mut headers = map(&[("X-Other", "x")]);
        let mut notes = Vec::new();
        let out = replace.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert!(!out.changed);
        assert!(notes.is_empty());

        let mut headers = map(&[("User-Agent", "u1"), ("X-Other", "x"), ("User-Agent", "u2")]);
        let order: Vec<String> = headers
            .keys()
            .map(|name| name.as_str().to_string())
            .collect();
        let mut notes = Vec::new();
        replace.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(values_of(&headers, "User-Agent"), ["Unknown"]);
        assert_eq!(
            headers
                .keys()
                .map(|name| name.as_str().to_string())
                .collect::<Vec<_>>(),
            order
        );

        // header-replace-regex: the manual's User-Agent, verbatim
        let regex = rewrite_of(
            "[Header Rewrite]\n\
             http-request ^http://example.com header-replace-regex User-Agent Safari Chrome",
        );
        let before = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_12_4) AppleWebKit/603.1.30 (KHTML, like Gecko) Version/10.1 Safari/603.1.30";
        let after = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_12_4) AppleWebKit/603.1.30 (KHTML, like Gecko) Version/10.1 Chrome/603.1.30";
        let mut headers = map(&[("User-Agent", before)]);
        let mut notes = Vec::new();
        regex.headers(
            Direction::Request,
            "http://example.com/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(values_of(&headers, "User-Agent"), [after]);

        // header-del and header-add together, as the manual says to use them
        let both = rewrite_of(
            "[Header Rewrite]\n\
             ^http://a\\.test header-del DNT\n\
             ^http://a\\.test header-add DNT 1",
        );
        let mut headers = map(&[("DNT", "0")]);
        let mut notes = Vec::new();
        both.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(values_of(&headers, "DNT"), ["1"]);
    }

    /// A rule of one direction leaves the other direction's table alone
    /// (`phase 4 M3 design 7.2`): the direction written on the line decides.
    #[test]
    fn a_rule_of_one_direction_leaves_the_other_alone() {
        let rules = rewrite_of(
            "[Header Rewrite]\n\
             http-request ^http://a\\.test header-add X-Req 1\n\
             http-response ^http://a\\.test header-add X-Resp 1",
        );

        let mut headers = map(&[("X", "0")]);
        let mut notes = Vec::new();
        rules.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(values_of(&headers, "X-Req"), ["1"]);
        assert!(values_of(&headers, "X-Resp").is_empty());

        let mut headers = map(&[("X", "0")]);
        let mut notes = Vec::new();
        rules.headers(
            Direction::Response,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(values_of(&headers, "X-Resp"), ["1"]);
        assert!(values_of(&headers, "X-Req").is_empty());
    }

    #[test]
    fn a_replace_regex_result_that_is_no_header_value_leaves_the_value() {
        let lines = "[Header Rewrite]\n^http://a\\.test header-replace-regex X-Token v4lue \u{1}";
        let rule = rewrite_of(lines);
        let mut headers = map(&[("X-Token", "v4lue-x")]);
        let mut notes = Vec::new();
        let out = rule.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(values_of(&headers, "X-Token"), ["v4lue-x"]);
        assert!(!out.changed);
        assert_eq!(
            notes,
            [Note::Skipped(
                "Header Rewrite skipped: ^http://a\\.test: header-replace-regex made a value a header cannot carry"
                    .to_string()
            )]
        );
    }

    #[test]
    fn framing_changes_are_found() {
        // a rule that removes `Transfer-Encoding` fails the request, and the
        // rules after it do not act
        let rule = rewrite_of(
            "[Header Rewrite]\n\
             ^http://a\\.test header-del Transfer-Encoding\n\
             ^http://a\\.test header-add X-Later 1",
        );
        let mut headers = map(&[("Transfer-Encoding", "chunked")]);
        let mut notes = Vec::new();
        let out = rule.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert!(out.framing);
        assert!(values_of(&headers, "X-Later").is_empty());
        assert_eq!(
            notes.last(),
            Some(&Note::Failed(
                "Header Rewrite failed: ^http://a\\.test: Content-Length or Transfer-Encoding changed"
                    .to_string()
            ))
        );

        // the same rule on a message that has none changes nothing
        let alone = rewrite_of("[Header Rewrite]\n^http://a\\.test header-del Transfer-Encoding");
        let mut headers = map(&[("X-Other", "x")]);
        let mut notes = Vec::new();
        let out = alone.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert!(!out.framing);
        assert!(!out.changed);
        assert!(notes.is_empty());

        // adding one is a change too
        let rule = rewrite_of("[Header Rewrite]\n^http://a\\.test header-add Content-Length 5");
        let mut headers = map(&[("X-Other", "x")]);
        let mut notes = Vec::new();
        let out = rule.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert!(out.framing);

        // a replace that writes the same value is not
        let rule = rewrite_of("[Header Rewrite]\n^http://a\\.test header-replace Content-Length 5");
        let mut headers = map(&[("Content-Length", "5")]);
        let mut notes = Vec::new();
        let out = rule.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert!(!out.framing);
        assert!(!out.changed);
    }

    #[test]
    fn a_changed_host_is_the_last_line_and_a_removed_one_comes_back() {
        let replace = rewrite_of("[Header Rewrite]\n^http://a\\.test header-replace Host b.test");
        let mut headers = map(&[("Host", "a.test")]);
        let mut notes = Vec::new();
        let out = replace.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(
            out.host,
            HostChange::Changed(HeaderValue::from_static("b.test"))
        );
        assert_eq!(values_of(&headers, "Host"), ["b.test"]);

        // an added line is the one the request sends on
        let add = rewrite_of("[Header Rewrite]\n^http://a\\.test header-add Host c.test");
        let mut headers = map(&[("Host", "a.test")]);
        let mut notes = Vec::new();
        let out = add.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(
            out.host,
            HostChange::Changed(HeaderValue::from_static("c.test"))
        );
        assert_eq!(values_of(&headers, "Host"), ["c.test"]);

        // the last line of Host cannot be removed: the original ones come back
        let del = rewrite_of("[Header Rewrite]\n^http://a\\.test header-del Host");
        let mut headers = map(&[("Host", "a.test")]);
        let mut notes = Vec::new();
        let out = del.headers(
            Direction::Request,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(out.host, HostChange::Same);
        assert_eq!(values_of(&headers, "Host"), ["a.test"]);
        assert_eq!(
            notes.last(),
            Some(&Note::Skipped(
                "Header Rewrite skipped: Host cannot be removed".to_string()
            ))
        );

        // a response never leaves a Host to write back
        let del = rewrite_of("[Header Rewrite]\nhttp-response ^http://a\\.test header-del Host");
        let mut headers = map(&[("Host", "a.test")]);
        let mut notes = Vec::new();
        let out = del.headers(
            Direction::Response,
            "http://a.test/x",
            &mut headers,
            &mut notes,
        );
        assert_eq!(out.host, HostChange::Same);
        assert!(values_of(&headers, "Host").is_empty());
    }

    #[test]
    fn a_header_rule_that_hits_the_backtracking_limit_is_skipped() {
        let lines = "[Header Rewrite]\n(?i)(a|b|ab)*(?=c) header-add X 1";
        let cfg = crate::rewrite::config_of(lines);
        let url = format!("http://x.test/{}", "ab".repeat(40));
        assert!(
            cfg.header[0].pattern.regex.is_match(&url).is_err(),
            "the expression gives up on this input"
        );
        let rule = rewrite_of(lines);
        let mut headers = map(&[("X", "0")]);
        let mut notes = Vec::new();
        let out = rule.headers(Direction::Request, &url, &mut headers, &mut notes);
        assert_eq!(values_of(&headers, "X"), ["0"]);
        assert!(!out.changed);
        assert_eq!(
            notes,
            [Note::Skipped(
                "Header Rewrite skipped: (?i)(a|b|ab)*(?=c): the regular expression hit its backtracking limit"
                    .to_string()
            )]
        );
    }

    #[test]
    fn writing_host_back() {
        let host = HeaderValue::from_static("b.test:8443");

        // an HTTP/1.1 upstream takes it as `Host`
        let mut req = Request::builder()
            .uri("/p?q")
            .header(http::header::HOST, "a.test")
            .body(())
            .expect("a request");
        assert!(write_back_host(&mut req, &host, Version::HTTP_11));
        assert_eq!(req.headers()[http::header::HOST], "b.test:8443");

        // an HTTP/2 upstream carries it as the authority of the target, and
        // names no `Host`
        let mut req = Request::builder()
            .uri("https://a.test/p?q")
            .header(http::header::HOST, "a.test")
            .body(())
            .expect("a request");
        assert!(write_back_host(&mut req, &host, Version::HTTP_2));
        assert_eq!(req.uri().to_string(), "https://b.test:8443/p?q");
        assert!(req.headers().get(http::header::HOST).is_none());

        // a value that is no authority leaves the request alone
        let mut req = Request::builder()
            .uri("https://a.test/p?q")
            .body(())
            .expect("a request");
        let bad = HeaderValue::from_static("a b");
        assert!(!write_back_host(&mut req, &bad, Version::HTTP_2));
        assert_eq!(req.uri().to_string(), "https://a.test/p?q");
    }
}
